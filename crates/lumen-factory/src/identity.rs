//! Identity ingest: canonical companion art -> [`IdentityRecord`].
//!
//! Contract summary:
//! - [`ingest`] locates, inside a caller-supplied art directory, the
//!   companion portrait (`companions/<name>_portrait.jpg`), the base
//!   sprite GIF (`companions/<name>_animated_base.gif`, frame 0), the
//!   optional mature sprite GIF (`<name>_animated_mature.gif`), and any
//!   expression PNGs (`expressions/<name>_*.png`). Every located file is
//!   hashed (SHA-256) and recorded with a role tag. The expressions
//!   directory is provenance only: its absence is tolerated.
//! - Palette extraction is a deterministic in-crate median-cut
//!   quantizer (bounded iterations, pixels sorted before every split,
//!   output sorted by population then RGB). Signature anchors are
//!   derived from documented image regions with named threshold
//!   constants; every derivation is deterministic.
//! - Proportions come from the sprite frame-0 alpha channel: a
//!   row-alpha profile gives total height, the head band (top to the
//!   neck, defined below), the hip row, and the leg span. A fully
//!   transparent or otherwise degenerate sprite fails closed.
//! - The record serializes as canonical JSON (sorted keys) and carries
//!   `record_hash`: the SHA-256 of the canonical JSON with the hash
//!   field itself empty. Re-ingesting the same art yields the same
//!   hash (the Phase 2 determinism gate).
//!
//! Region definitions (portrait, fractions of width/height):
//! - Background estimate: median of the four 16x16 corner patches.
//! - Hair: median-cut palette of the portrait's upper third
//!   (`y < 1/3`), excluding boxes whose center matches the background
//!   estimate; the winner is the most saturated remaining box whose
//!   population share clears [`HAIR_MIN_SHARE`].
//! - Face region: `x in [0.34, 0.66]`, `y in [0.24, 0.46]` (the central
//!   face box for the canonical front-facing portrait framing).
//!   Skin is the median of skin-like pixels inside it (low saturation,
//!   high value, warm channel order), falling back to the region
//!   median when too few pixels qualify.
//! - Eye band: `x in [0.36, 0.64]`, `y in [0.33, 0.40]` (inside the
//!   face region, at eye height). The eye color is the median of the
//!   dominant saturated cluster that matches neither skin nor hair;
//!   best-effort (`None` when no cluster qualifies).
//! - Mouth band: `x in [0.44, 0.56]`, `y in [0.42, 0.47]`. Lips are
//!   the median of reddish saturated pixels there; best-effort.
//! - Outfit primaries: the two most populous sprite colors (median-cut)
//!   that match neither skin nor hair.
//!
//! Neck definition (sprite): the head's widest row is found in the
//! upper 40% of the alpha bounding box; the neck is the narrowest row
//! from that row down to the 40% line. (The naive "narrowest row in
//! the upper 40%" is the hair tip at the very top of the sprite and
//! is never the neck.)

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use image::{AnimationDecoder, DynamicImage, GenericImageView, RgbaImage};
use serde::{Deserialize, Serialize};

use crate::base::base_proportions;
use crate::{FactoryError, sha256_hex};

/// Schema identifier emitted in every record.
pub const IDENTITY_SCHEMA: &str = "lumen.factory-identity";
/// Schema version emitted in every record.
pub const IDENTITY_VERSION: u32 = 1;

/// Minimum accepted proportion band scale (see
/// [`IdentityRecord::applied_scales`]). The range was widened from
/// [0.8, 1.25] to [0.5, 2.0] in Phase 2 — Séraphine's measured
/// proportions fall outside the narrower range; the fail-closed
/// behavior is unchanged. `project.rs` re-exports these constants
/// and `params.rs` validates against them: this is the single
/// source of truth for the scale range.
pub const SCALE_MIN: f32 = 0.5;
/// Maximum accepted proportion band scale (see [`SCALE_MIN`]).
pub const SCALE_MAX: f32 = 2.0;

/// Maximum accepted size of any single source art file.
pub const MAX_ART_FILE_BYTES: usize = 64 * 1024 * 1024;
/// Maximum decoded pixel count (width * height) for any single image.
pub const MAX_ART_PIXELS: u64 = 50_000_000;
/// Minimum portrait edge length; anything smaller cannot carry the
/// documented face regions and fails closed.
pub const MIN_PORTRAIT_EDGE: u32 = 16;
/// Minimum sprite edge length for a meaningful alpha profile.
pub const MIN_SPRITE_EDGE: u32 = 8;
/// Alpha threshold (0..=255) at or above which a sprite pixel counts
/// as body for the row profile and palette sampling.
pub const ALPHA_OPAQUE: u8 = 128;
/// Number of median-cut palette entries kept for the combined palette.
pub const PALETTE_ENTRIES: usize = 16;
/// Number of median-cut boxes used per anchor extraction.
pub const ANCHOR_BOXES: usize = 8;
/// Minimum population share for a hair candidate box.
pub const HAIR_MIN_SHARE: f64 = 0.02;
/// Minimum saturation (HSV, 0..=1) for a hair candidate box.
pub const HAIR_MIN_SATURATION: f64 = 0.20;
/// RGB distance below which two colors are considered the same anchor
/// (background exclusion, skin/hair exclusion for outfit colors).
pub const COLOR_MATCH_DIST: f64 = 48.0;
/// Minimum skin-like pixel count inside the face region before the
/// filtered median is trusted over the plain region median.
pub const SKIN_MIN_PIXELS: usize = 24;
/// Portrait sampling stride for palette collection (deterministic).
pub const PORTRAIT_SAMPLE_STRIDE: u32 = 4;
/// Total collected-sample bound across all palette inputs.
pub const MAX_PALETTE_SAMPLES: usize = 400_000;

/// Face region: `(x0, x1, y0, y1)` fractions of the portrait.
pub const FACE_REGION: (f64, f64, f64, f64) = (0.34, 0.66, 0.24, 0.46);
/// Eye band: `(x0, x1, y0, y1)` fractions of the portrait.
pub const EYE_BAND: (f64, f64, f64, f64) = (0.36, 0.64, 0.33, 0.40);
/// Mouth band: `(x0, x1, y0, y1)` fractions of the portrait.
pub const MOUTH_BAND: (f64, f64, f64, f64) = (0.44, 0.56, 0.42, 0.47);

/// One RGB color with its hex rendering.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColorEntry {
    /// `#rrggbb` lowercase hex.
    pub hex: String,
    /// `[r, g, b]` channels, 0..=255.
    pub rgb: [u8; 3],
}

impl ColorEntry {
    /// Build an entry from channels, computing the hex form.
    pub fn new(rgb: [u8; 3]) -> Self {
        Self {
            hex: format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2]),
            rgb,
        }
    }
}

/// One median-cut palette entry: a representative color and its share
/// of the sampled pixels (0..=1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PaletteEntry {
    /// Population share of sampled pixels.
    pub share: f64,
    /// Representative (mean) color of the box.
    #[serde(flatten)]
    pub color: ColorEntry,
}

/// Derived signature anchors for one companion.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Palette {
    /// Dominant eye color, when a qualifying cluster was found.
    pub eye: Option<ColorEntry>,
    /// Hair anchor color.
    pub hair: ColorEntry,
    /// Lip color, when a qualifying cluster was found.
    pub lips: Option<ColorEntry>,
    /// Outfit primaries from the sprite (most populous first).
    pub outfit: Vec<ColorEntry>,
    /// Skin anchor color.
    pub skin: ColorEntry,
}

/// Body proportions as ratios of total height (each 0..=1, summing
/// to approximately 1).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Proportions {
    /// Head band height / total height.
    pub head_ratio: f64,
    /// Leg span / total height.
    pub leg_ratio: f64,
    /// Torso span / total height.
    pub torso_ratio: f64,
}

/// Raw sprite measurements behind the ratios (pixels).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpriteMeasurements {
    /// Widest alpha row inside the 40..60% band (row index from top
    /// of the alpha bounding box).
    pub hip_row: u32,
    /// Head band height (neck row minus top row).
    pub head_height_px: u32,
    /// Leg span (bottom row minus hip row).
    pub leg_span_px: u32,
    /// Narrowest row at or below the head's widest row, inside the
    /// upper 40% (row index from top of the bounding box).
    pub neck_row: u32,
    /// Torso span (hip row minus neck row).
    pub torso_height_px: u32,
    /// Alpha bounding-box height.
    pub total_height_px: u32,
    /// Sprite frame width.
    pub width_px: u32,
}

/// One hashed source art file.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SourceFile {
    /// File name (not a full path, so records stay portable).
    pub path: String,
    /// Role tag: `portrait`, `sprite_base`, `sprite_mature`, or
    /// `expression`.
    pub role: String,
    /// SHA-256 hex of the file bytes.
    pub sha256: String,
}

/// The canonical identity record (schema `lumen.factory-identity` v1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IdentityRecord {
    /// Companion name (the ingest key).
    pub companion: String,
    /// Derived palette anchors plus the combined dominant palette.
    pub palette: Palette,
    /// Combined dominant colors (portrait + sprite), population order.
    pub palette_dominant: Vec<PaletteEntry>,
    /// Measured body proportions.
    pub proportions: Proportions,
    /// SHA-256 of the canonical JSON with this field empty.
    pub record_hash: String,
    /// Schema identifier ([`IDENTITY_SCHEMA`]).
    pub schema: String,
    /// Hashed source files, sorted by role then path.
    pub sources: Vec<SourceFile>,
    /// Raw sprite measurements behind the ratios.
    pub sprite_measurements: SpriteMeasurements,
    /// Schema version ([`IDENTITY_VERSION`]).
    pub version: u32,
}

/// Write a `serde_json::Value` in the record's canonical form:
/// object keys sorted, no insignificant whitespace, and floats
/// rendered at a fixed 12-decimal precision (trailing zeros
/// trimmed). The fixed float form is load-bearing: it makes the
/// canonical bytes a pure function of the stored decimal values, so
/// a serialize -> parse -> serialize round trip is byte-stable
/// regardless of the float formatter's shortest-round-trip choices
/// (measured on the real Séraphine record: `leg_ratio` re-serialized
/// differently after a parse without this rule, which broke hash
/// verification). All floats in the record are ratios/shares in
/// [0, 1], far inside the safe range of the 12-decimal form.
fn canonical_write(value: &serde_json::Value, out: &mut String) {
    match value {
        serde_json::Value::Null => out.push_str("null"),
        serde_json::Value::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        serde_json::Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                out.push_str(&i.to_string());
            } else if let Some(u) = n.as_u64() {
                out.push_str(&u.to_string());
            } else if let Some(f) = n.as_f64() {
                let fixed = format!("{f:.12}");
                let trimmed = fixed.trim_end_matches('0').trim_end_matches('.');
                out.push_str(if trimmed.is_empty() || trimmed == "-" {
                    "0"
                } else {
                    trimmed
                });
            } else {
                out.push('0');
            }
        }
        serde_json::Value::String(s) => {
            out.push_str(&serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string()));
        }
        serde_json::Value::Array(items) => {
            out.push('[');
            for (idx, item) in items.iter().enumerate() {
                if idx > 0 {
                    out.push(',');
                }
                canonical_write(item, out);
            }
            out.push(']');
        }
        serde_json::Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (idx, key) in keys.iter().enumerate() {
                if idx > 0 {
                    out.push(',');
                }
                out.push_str(&serde_json::to_string(key).unwrap_or_else(|_| "\"\"".to_string()));
                out.push(':');
                canonical_write(&map[*key], out);
            }
            out.push('}');
        }
    }
}

impl IdentityRecord {
    /// Canonical JSON: sorted keys, no insignificant whitespace,
    /// fixed-precision floats (see [`canonical_write`]).
    pub fn canonical_json(&self) -> Result<String, FactoryError> {
        let value = serde_json::to_value(self)
            .map_err(|e| FactoryError::BadParam(format!("record serialize: {e}")))?;
        let mut out = String::new();
        canonical_write(&value, &mut out);
        Ok(out)
    }

    /// The hash this record should carry for its current contents.
    pub fn compute_hash(&self) -> Result<String, FactoryError> {
        let mut probe = self.clone();
        probe.record_hash = String::new();
        Ok(sha256_hex(probe.canonical_json()?.as_bytes()))
    }

    /// Verify `record_hash` matches the record contents.
    pub fn verify_hash(&self) -> Result<bool, FactoryError> {
        Ok(self.compute_hash()? == self.record_hash)
    }

    /// The proportion band scales `(head, torso, leg)` this record
    /// applies to the canonical skeleton: each measured ratio divided
    /// by the canonical base ratio from
    /// [`crate::base::base_proportions`]. This is the single source
    /// of truth for applied scales — `project.rs`'s `compute_scales`
    /// delegates here and `params.rs` derives its `proportions` from
    /// here. Fails closed when any ratio is non-finite/degenerate or
    /// any scale falls outside [`SCALE_MIN`]..=[`SCALE_MAX`] (never
    /// silently clamped).
    pub fn applied_scales(&self) -> Result<(f32, f32, f32), FactoryError> {
        let p = &self.proportions;
        scales_for_ratios(p.head_ratio, p.torso_ratio, p.leg_ratio)
    }
}

/// The applied-scales law for any measured `(head, torso, leg)` ratio
/// triple: each ratio divided by the canonical base ratio from
/// [`crate::base::base_proportions`]. Fails closed when a ratio is
/// non-finite/degenerate, when the triple does not sum to ~1, or when
/// any scale falls outside [`SCALE_MIN`]..=[`SCALE_MAX`] (never
/// silently clamped). [`IdentityRecord::applied_scales`] delegates
/// here, and the VRM proportions bridge in `params.rs` uses the same
/// law, so a ratio triple means the same scales wherever it was
/// measured — sprite alpha bands or a VRM humanoid skeleton.
pub fn scales_for_ratios(
    head_ratio: f64,
    torso_ratio: f64,
    leg_ratio: f64,
) -> Result<(f32, f32, f32), FactoryError> {
    let ratios = [head_ratio, torso_ratio, leg_ratio];
    if ratios
        .iter()
        .any(|r| !r.is_finite() || *r <= 0.0 || *r >= 1.0)
    {
        return Err(FactoryError::BadParam(format!(
            "proportion ratios degenerate: {ratios:?}"
        )));
    }
    let sum: f64 = ratios.iter().sum();
    if (sum - 1.0).abs() > 0.05 {
        return Err(FactoryError::BadParam(format!(
            "proportion ratios sum to {sum}, expected ~1"
        )));
    }
    let (base_head, base_torso, base_leg) = base_proportions();
    let scales = (
        (head_ratio as f32 / base_head),
        (torso_ratio as f32 / base_torso),
        (leg_ratio as f32 / base_leg),
    );
    for (name, scale) in [("head", scales.0), ("torso", scales.1), ("leg", scales.2)] {
        if !scale.is_finite() || !(SCALE_MIN..=SCALE_MAX).contains(&scale) {
            return Err(FactoryError::BadParam(format!(
                "{name} scale {scale} outside [{SCALE_MIN}, {SCALE_MAX}] (fail closed, not clamped)"
            )));
        }
    }
    Ok(scales)
}

// --- color helpers ---

fn rgb_to_hsv(rgb: [u8; 3]) -> (f64, f64, f64) {
    let (r, g, b) = (
        rgb[0] as f64 / 255.0,
        rgb[1] as f64 / 255.0,
        rgb[2] as f64 / 255.0,
    );
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    let hue = if delta <= f64::EPSILON {
        0.0
    } else if (max - r).abs() <= f64::EPSILON {
        60.0 * (((g - b) / delta) % 6.0)
    } else if (max - g).abs() <= f64::EPSILON {
        60.0 * ((b - r) / delta + 2.0)
    } else {
        60.0 * ((r - g) / delta + 4.0)
    };
    let hue = if hue < 0.0 { hue + 360.0 } else { hue };
    let sat = if max <= f64::EPSILON {
        0.0
    } else {
        delta / max
    };
    (hue, sat, max)
}

fn color_dist(a: [u8; 3], b: [u8; 3]) -> f64 {
    let dr = a[0] as f64 - b[0] as f64;
    let dg = a[1] as f64 - b[1] as f64;
    let db = a[2] as f64 - b[2] as f64;
    (dr * dr + dg * dg + db * db).sqrt()
}

fn median_u8(values: &mut [u8]) -> u8 {
    if values.is_empty() {
        return 0;
    }
    values.sort_unstable();
    values[values.len() / 2]
}

fn median_color(pixels: &[[u8; 3]]) -> Option<[u8; 3]> {
    if pixels.is_empty() {
        return None;
    }
    let mut rs: Vec<u8> = pixels.iter().map(|p| p[0]).collect();
    let mut gs: Vec<u8> = pixels.iter().map(|p| p[1]).collect();
    let mut bs: Vec<u8> = pixels.iter().map(|p| p[2]).collect();
    Some([median_u8(&mut rs), median_u8(&mut gs), median_u8(&mut bs)])
}

// --- deterministic median-cut quantizer ---

/// One median-cut box: member pixels plus cached channel ranges.
struct CutBox {
    pixels: Vec<[u8; 3]>,
}

impl CutBox {
    fn mean_color(&self) -> [u8; 3] {
        let n = self.pixels.len().max(1) as u64;
        let mut sums = [0u64; 3];
        for px in &self.pixels {
            for ch in 0..3 {
                sums[ch] += px[ch] as u64;
            }
        }
        [
            (sums[0] / n) as u8,
            (sums[1] / n) as u8,
            (sums[2] / n) as u8,
        ]
    }

    fn widest_channel(&self) -> Option<(usize, u8)> {
        let mut mins = [u8::MAX; 3];
        let mut maxs = [u8::MIN; 3];
        for px in &self.pixels {
            for ch in 0..3 {
                mins[ch] = mins[ch].min(px[ch]);
                maxs[ch] = maxs[ch].max(px[ch]);
            }
        }
        let mut best: Option<(usize, u8)> = None;
        for ch in 0..3 {
            let range = maxs[ch] - mins[ch];
            if range > 0 && best.is_none_or(|(_, r)| range > r) {
                best = Some((ch, range));
            }
        }
        best
    }
}

/// Deterministic median-cut quantization of `pixels` into at most
/// `target_boxes` entries, sorted by population descending, then by
/// RGB ascending so ties never depend on iteration order.
pub fn median_cut(pixels: &[[u8; 3]], target_boxes: usize) -> Vec<PaletteEntry> {
    if pixels.is_empty() || target_boxes == 0 {
        return Vec::new();
    }
    let total = pixels.len() as f64;
    let mut sorted: Vec<[u8; 3]> = pixels.to_vec();
    sorted.sort_unstable();
    let mut boxes: Vec<CutBox> = vec![CutBox { pixels: sorted }];
    while boxes.len() < target_boxes {
        // Split the most populous splittable box (first on ties).
        let mut split_idx: Option<usize> = None;
        for (idx, bx) in boxes.iter().enumerate() {
            if bx.pixels.len() < 2 || bx.widest_channel().is_none() {
                continue;
            }
            if split_idx.is_none_or(|cur| bx.pixels.len() > boxes[cur].pixels.len()) {
                split_idx = Some(idx);
            }
        }
        let Some(idx) = split_idx else { break };
        let mut bx = boxes.remove(idx);
        let (channel, _) = bx.widest_channel().expect("splittable box has a channel");
        bx.pixels
            .sort_by(|a, b| a[channel].cmp(&b[channel]).then(a.cmp(b)));
        let mid = bx.pixels.len() / 2;
        let right = bx.pixels.split_off(mid);
        boxes.push(bx);
        boxes.push(CutBox { pixels: right });
    }
    let mut entries: Vec<PaletteEntry> = boxes
        .iter()
        .map(|bx| PaletteEntry {
            share: bx.pixels.len() as f64 / total,
            color: ColorEntry::new(bx.mean_color()),
        })
        .collect();
    entries.sort_by(|a, b| {
        b.share
            .total_cmp(&a.share)
            .then(a.color.rgb.cmp(&b.color.rgb))
    });
    entries
}

// --- image loading (fail-closed, bounded) ---

fn read_art_file(path: &Path) -> Result<Vec<u8>, FactoryError> {
    let meta = std::fs::metadata(path)?;
    if meta.len() as usize > MAX_ART_FILE_BYTES {
        return Err(FactoryError::BadParam(format!(
            "art file {} exceeds size bound",
            path.display()
        )));
    }
    Ok(std::fs::read(path)?)
}

fn check_dimensions(img: &DynamicImage, what: &str) -> Result<(), FactoryError> {
    let (w, h) = img.dimensions();
    if w as u64 * h as u64 > MAX_ART_PIXELS {
        return Err(FactoryError::BadParam(format!(
            "{what} pixel count exceeds bound"
        )));
    }
    Ok(())
}

fn load_portrait(path: &Path) -> Result<RgbaImage, FactoryError> {
    let bytes = read_art_file(path)?;
    let img = image::load_from_memory(&bytes)
        .map_err(|e| FactoryError::BadParam(format!("portrait decode: {e}")))?;
    check_dimensions(&img, "portrait")?;
    let (w, h) = img.dimensions();
    if w < MIN_PORTRAIT_EDGE || h < MIN_PORTRAIT_EDGE {
        return Err(FactoryError::BadParam(format!(
            "portrait {w}x{h} below minimum edge {MIN_PORTRAIT_EDGE}"
        )));
    }
    Ok(img.to_rgba8())
}

fn load_gif_frame0(path: &Path) -> Result<RgbaImage, FactoryError> {
    let bytes = read_art_file(path)?;
    let decoder = image::codecs::gif::GifDecoder::new(std::io::Cursor::new(bytes))
        .map_err(|e| FactoryError::BadParam(format!("sprite gif decode: {e}")))?;
    let mut frames = decoder
        .into_frames()
        .collect_frames()
        .map_err(|e| FactoryError::BadParam(format!("sprite gif frames: {e}")))?;
    if frames.is_empty() {
        return Err(FactoryError::BadParam(
            "sprite gif has no frames".to_string(),
        ));
    }
    let frame = frames.remove(0).into_buffer();
    if frame.width() as u64 * frame.height() as u64 > MAX_ART_PIXELS {
        return Err(FactoryError::BadParam(
            "sprite pixel count exceeds bound".to_string(),
        ));
    }
    if frame.width() < MIN_SPRITE_EDGE || frame.height() < MIN_SPRITE_EDGE {
        return Err(FactoryError::BadParam(format!(
            "sprite {}x{} below minimum edge {MIN_SPRITE_EDGE}",
            frame.width(),
            frame.height()
        )));
    }
    Ok(frame)
}

// --- region sampling ---

fn region_pixels(img: &RgbaImage, region: (f64, f64, f64, f64), stride: u32) -> Vec<[u8; 3]> {
    let (w, h) = (img.width(), img.height());
    let x0 = (region.0 * w as f64) as u32;
    let x1 = ((region.1 * w as f64) as u32).min(w);
    let y0 = (region.2 * h as f64) as u32;
    let y1 = ((region.3 * h as f64) as u32).min(h);
    let mut out = Vec::new();
    let mut y = y0;
    while y < y1 {
        let mut x = x0;
        while x < x1 {
            let px = img.get_pixel(x, y);
            out.push([px[0], px[1], px[2]]);
            x += stride.max(1);
        }
        y += stride.max(1);
    }
    out
}

fn background_estimate(img: &RgbaImage) -> [u8; 3] {
    let (w, h) = (img.width(), img.height());
    let patch = 16u32.min(w).min(h);
    let mut pixels = Vec::new();
    for (cx, cy) in [
        (0, 0),
        (w - patch, 0),
        (0, h - patch),
        (w - patch, h - patch),
    ] {
        for y in cy..cy + patch {
            for x in cx..cx + patch {
                let px = img.get_pixel(x, y);
                pixels.push([px[0], px[1], px[2]]);
            }
        }
    }
    median_color(&pixels).unwrap_or([0, 0, 0])
}

fn derive_hair(portrait: &RgbaImage, background: [u8; 3]) -> Result<[u8; 3], FactoryError> {
    let upper = region_pixels(portrait, (0.0, 1.0, 0.0, 1.0 / 3.0), 3);
    let entries = median_cut(&upper, ANCHOR_BOXES);
    let mut best: Option<(f64, f64, [u8; 3])> = None;
    for entry in &entries {
        if color_dist(entry.color.rgb, background) < COLOR_MATCH_DIST {
            continue;
        }
        let (_, sat, _) = rgb_to_hsv(entry.color.rgb);
        if sat < HAIR_MIN_SATURATION || entry.share < HAIR_MIN_SHARE {
            continue;
        }
        let dominated =
            best.is_none_or(|(bs, bsh, _)| sat > bs || (sat == bs && entry.share > bsh));
        if dominated {
            best = Some((sat, entry.share, entry.color.rgb));
        }
    }
    best.map(|(_, _, rgb)| rgb).ok_or_else(|| {
        FactoryError::BadParam("no qualifying hair color in portrait upper third".to_string())
    })
}

fn derive_skin(portrait: &RgbaImage) -> Result<[u8; 3], FactoryError> {
    let region = region_pixels(portrait, FACE_REGION, 2);
    if region.is_empty() {
        return Err(FactoryError::BadParam("face region is empty".to_string()));
    }
    let skin_like: Vec<[u8; 3]> = region
        .iter()
        .copied()
        .filter(|px| {
            let (_, sat, val) = rgb_to_hsv(*px);
            sat < 0.38 && val > 0.55 && px[0] >= px[1] && (px[0] as i32 - px[2] as i32) < 80
        })
        .collect();
    if skin_like.len() >= SKIN_MIN_PIXELS {
        median_color(&skin_like).ok_or_else(|| FactoryError::BadParam("skin median".into()))
    } else {
        median_color(&region).ok_or_else(|| FactoryError::BadParam("skin median".into()))
    }
}

fn derive_eye(portrait: &RgbaImage, hair: [u8; 3], skin: [u8; 3]) -> Option<[u8; 3]> {
    let region = region_pixels(portrait, EYE_BAND, 1);
    let candidates: Vec<[u8; 3]> = region
        .iter()
        .copied()
        .filter(|px| {
            let (_, sat, val) = rgb_to_hsv(*px);
            sat > 0.30
                && val > 0.10
                && val < 0.90
                && color_dist(*px, hair) >= COLOR_MATCH_DIST
                && color_dist(*px, skin) >= COLOR_MATCH_DIST
        })
        .collect();
    if candidates.len() < 8 {
        return None;
    }
    let entries = median_cut(&candidates, 4);
    entries.first().map(|e| e.color.rgb)
}

fn derive_lips(portrait: &RgbaImage) -> Option<[u8; 3]> {
    let region = region_pixels(portrait, MOUTH_BAND, 1);
    let candidates: Vec<[u8; 3]> = region
        .iter()
        .copied()
        .filter(|px| {
            let (_, sat, _) = rgb_to_hsv(*px);
            sat > 0.22 && px[0] as i32 - px[2] as i32 > 12
        })
        .collect();
    if candidates.len() < 4 {
        return None;
    }
    median_color(&candidates)
}

fn derive_outfit(sprite: &RgbaImage, hair: [u8; 3], skin: [u8; 3]) -> Vec<ColorEntry> {
    // Sprite art is flat-shaded, so outfit primaries come from a
    // coarse bucket histogram (bucket = channel / 32, entry color =
    // bucket mean): median-cut box populations fragment flat color
    // groups across split boundaries, which scrambles their order.
    // The histogram is exact, bounded (<= 512 buckets), and sorted
    // deterministically by population then RGB.
    let mut buckets: BTreeMap<[u8; 3], ([u64; 3], u64)> = BTreeMap::new();
    for px in sprite.pixels() {
        if px[3] < ALPHA_OPAQUE {
            continue;
        }
        let key = [px[0] / 32, px[1] / 32, px[2] / 32];
        let entry = buckets.entry(key).or_insert(([0; 3], 0));
        for ch in 0..3 {
            entry.0[ch] += px[ch] as u64;
        }
        entry.1 += 1;
    }
    let mut ranked: Vec<([u8; 3], u64)> = buckets
        .values()
        .map(|(sums, count)| {
            let n = (*count).max(1);
            (
                [
                    (sums[0] / n) as u8,
                    (sums[1] / n) as u8,
                    (sums[2] / n) as u8,
                ],
                *count,
            )
        })
        .collect();
    ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut out: Vec<ColorEntry> = Vec::new();
    for (rgb, _) in ranked {
        if color_dist(rgb, hair) < COLOR_MATCH_DIST || color_dist(rgb, skin) < COLOR_MATCH_DIST {
            continue;
        }
        if out
            .iter()
            .any(|c: &ColorEntry| color_dist(c.rgb, rgb) < COLOR_MATCH_DIST)
        {
            continue;
        }
        out.push(ColorEntry::new(rgb));
        if out.len() >= 2 {
            break;
        }
    }
    out
}

// --- proportions from the sprite alpha channel ---

/// Measure body proportions from a sprite frame's alpha channel.
///
/// Row profile: per-row count of pixels with alpha >=
/// [`ALPHA_OPAQUE`]. The bounding box of non-empty rows is the total
/// height; the neck and hip rows follow the module-doc definitions.
/// Fails closed on a fully transparent or degenerate sprite.
pub fn measure_proportions(
    sprite: &RgbaImage,
) -> Result<(Proportions, SpriteMeasurements), FactoryError> {
    let (w, h) = (sprite.width(), sprite.height());
    let mut row_counts: Vec<u32> = vec![0; h as usize];
    for (x, y, px) in sprite.enumerate_pixels() {
        let _ = x;
        if px[3] >= ALPHA_OPAQUE {
            row_counts[y as usize] += 1;
        }
    }
    let top = row_counts.iter().position(|&c| c > 0);
    let bottom = row_counts.iter().rposition(|&c| c > 0);
    let (Some(top), Some(bottom)) = (top, bottom) else {
        return Err(FactoryError::BadParam(
            "sprite is fully transparent".to_string(),
        ));
    };
    let total = (bottom - top + 1) as u32;
    if total < MIN_SPRITE_EDGE {
        return Err(FactoryError::BadParam(format!(
            "sprite alpha height {total} is degenerate"
        )));
    }
    let rel = |row: usize| row - top;
    let upper_end = (total as f64 * 0.40) as usize;
    // Head's widest row within the upper 40%.
    let mut head_widest = 0usize;
    for i in 0..upper_end.max(1) {
        if row_counts[top + i] > row_counts[top + head_widest] {
            head_widest = i;
        }
    }
    // Neck: narrowest row from the head's widest row to the 40% line.
    let mut neck = head_widest;
    for i in head_widest..upper_end.max(1) {
        if row_counts[top + i] < row_counts[top + neck] {
            neck = i;
        }
    }
    // Hip: widest row within the 40..60% band.
    let band_lo = (total as f64 * 0.40) as usize;
    let band_hi = ((total as f64 * 0.60) as usize).min(total as usize - 1);
    let mut hip = band_lo;
    for i in band_lo..=band_hi {
        if row_counts[top + i] > row_counts[top + hip] {
            hip = i;
        }
    }
    if neck == 0 || hip <= neck {
        return Err(FactoryError::BadParam(format!(
            "sprite bands degenerate (neck {neck}, hip {hip}, total {total})"
        )));
    }
    let head_px = neck as u32;
    let torso_px = (hip - neck) as u32;
    let leg_px = total - hip as u32;
    let total_f = total as f64;
    let proportions = Proportions {
        head_ratio: head_px as f64 / total_f,
        leg_ratio: leg_px as f64 / total_f,
        torso_ratio: torso_px as f64 / total_f,
    };
    let measurements = SpriteMeasurements {
        hip_row: hip as u32,
        head_height_px: head_px,
        leg_span_px: leg_px,
        neck_row: neck as u32,
        torso_height_px: torso_px,
        total_height_px: total,
        width_px: w,
    };
    let _ = rel;
    Ok((proportions, measurements))
}

// --- ingest entry point ---

fn find_file(dir: &Path, name: &str) -> Option<PathBuf> {
    let candidate = dir.join(name);
    candidate.is_file().then_some(candidate)
}

/// Ingest the canonical art for `companion` from `art_dir` (the
/// companion's art root, containing `companions/` and optionally
/// `expressions/` subdirectories).
pub fn ingest(art_dir: &Path, companion: &str) -> Result<IdentityRecord, FactoryError> {
    if companion.is_empty()
        || !companion
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(FactoryError::BadParam(format!(
            "invalid companion name {companion:?}"
        )));
    }
    if !art_dir.is_dir() {
        return Err(FactoryError::BadParam(format!(
            "art dir {} not found",
            art_dir.display()
        )));
    }
    let companions_dir = art_dir.join("companions");
    let portrait_path = find_file(&companions_dir, &format!("{companion}_portrait.jpg"))
        .ok_or_else(|| {
            FactoryError::BadParam(format!(
                "portrait missing: companions/{companion}_portrait.jpg"
            ))
        })?;
    let sprite_path = find_file(&companions_dir, &format!("{companion}_animated_base.gif"))
        .ok_or_else(|| {
            FactoryError::BadParam(format!(
                "base sprite missing: companions/{companion}_animated_base.gif"
            ))
        })?;
    let mature_path = find_file(&companions_dir, &format!("{companion}_animated_mature.gif"));

    // Hash every source file (role-tagged, sorted by role then path).
    let mut sources: Vec<SourceFile> = Vec::new();
    let mut hash_source = |path: &Path, role: &str| -> Result<(), FactoryError> {
        let bytes = read_art_file(path)?;
        sources.push(SourceFile {
            path: path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
            role: role.to_string(),
            sha256: sha256_hex(&bytes),
        });
        Ok(())
    };
    hash_source(&portrait_path, "portrait")?;
    hash_source(&sprite_path, "sprite_base")?;
    if let Some(mature) = &mature_path {
        hash_source(mature, "sprite_mature")?;
    }
    // Expressions: provenance only; a missing directory is tolerated.
    let expressions_dir = art_dir.join("expressions");
    if expressions_dir.is_dir() {
        let mut names: Vec<PathBuf> = Vec::new();
        for entry in std::fs::read_dir(&expressions_dir)? {
            let entry = entry?;
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if path.is_file()
                && name.starts_with(&format!("{companion}_"))
                && name.ends_with(".png")
            {
                names.push(path);
            }
        }
        names.sort();
        for path in &names {
            hash_source(path, "expression")?;
        }
    }
    sources.sort_by(|a, b| a.role.cmp(&b.role).then(a.path.cmp(&b.path)));

    // Decode the two images that carry identity data.
    let portrait = load_portrait(&portrait_path)?;
    let sprite = load_gif_frame0(&sprite_path)?;

    // Palette anchors.
    let background = background_estimate(&portrait);
    let hair = derive_hair(&portrait, background)?;
    let skin = derive_skin(&portrait)?;
    let eye = derive_eye(&portrait, hair, skin);
    let lips = derive_lips(&portrait);
    let outfit = derive_outfit(&sprite, hair, skin);
    if outfit.is_empty() {
        return Err(FactoryError::BadParam(
            "no outfit colors distinguishable from skin/hair in sprite".to_string(),
        ));
    }

    // Combined dominant palette over portrait (sampled) + sprite.
    let mut samples: Vec<[u8; 3]> =
        region_pixels(&portrait, (0.0, 1.0, 0.0, 1.0), PORTRAIT_SAMPLE_STRIDE);
    for px in sprite.pixels() {
        if px[3] >= ALPHA_OPAQUE {
            samples.push([px[0], px[1], px[2]]);
        }
    }
    if samples.len() > MAX_PALETTE_SAMPLES {
        let stride = samples.len().div_ceil(MAX_PALETTE_SAMPLES);
        samples = samples.iter().step_by(stride).copied().collect::<Vec<_>>();
    }
    let palette_dominant = median_cut(&samples, PALETTE_ENTRIES);

    // Proportions from the sprite alpha channel.
    let (proportions, sprite_measurements) = measure_proportions(&sprite)?;

    let mut record = IdentityRecord {
        companion: companion.to_string(),
        palette: Palette {
            eye: eye.map(ColorEntry::new),
            hair: ColorEntry::new(hair),
            lips: lips.map(ColorEntry::new),
            outfit,
            skin: ColorEntry::new(skin),
        },
        palette_dominant,
        proportions,
        record_hash: String::new(),
        schema: IDENTITY_SCHEMA.to_string(),
        sources,
        sprite_measurements,
        version: IDENTITY_VERSION,
    };
    record.record_hash = record.compute_hash()?;
    Ok(record)
}

/// Load and hash-verify an identity record from a JSON file.
pub fn load_record(path: &Path) -> Result<IdentityRecord, FactoryError> {
    let text = std::fs::read_to_string(path)?;
    let record: IdentityRecord = serde_json::from_str(&text)
        .map_err(|e| FactoryError::BadParam(format!("identity record parse: {e}")))?;
    if record.schema != IDENTITY_SCHEMA || record.version != IDENTITY_VERSION {
        return Err(FactoryError::BadParam(format!(
            "unsupported identity schema {} v{}",
            record.schema, record.version
        )));
    }
    if !record.verify_hash()? {
        return Err(FactoryError::BadParam(
            "identity record hash mismatch".to_string(),
        ));
    }
    Ok(record)
}

/// Collect role -> file digests (helper for tests and CLI summaries).
pub fn source_digests(record: &IdentityRecord) -> BTreeMap<String, Vec<String>> {
    let mut out: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for source in &record.sources {
        out.entry(source.role.clone())
            .or_default()
            .push(source.sha256.clone());
    }
    out
}
