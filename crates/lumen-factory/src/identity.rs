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
//!   derived from documented regions and predicates with named
//!   threshold constants; every derivation is deterministic.
//! - Proportions come from frame 0 of the mature sprite when one is
//!   present (the mature set is the default target, factory plan §1),
//!   else from the base sprite; `proportions_source` records which.
//! - The record also carries three OPTIONAL authored fields —
//!   `bust_delta_cm` and per-band `width_scales` / `depth_scales`
//!   ([`BandScaleSet`]) — for the per-character frame the sprite
//!   cannot measure (a silhouette carries no depth; GAP-11-3 /
//!   GAP-11-4). Ingest always leaves them `None`; they enter a
//!   record only as authored data, are omitted from serialization
//!   when `None` (pre-existing records keep identical bytes and
//!   hashes), and are validated fail-closed at every use.
//!   The head is segmented by color as well as alpha (see "Head
//!   segmentation" below), so the head band measures the head, not
//!   the hair mass around it. A fully transparent sprite, a sprite
//!   with no skin-colored face, or one whose head cannot be segmented
//!   fails closed.
//! - The record serializes as canonical JSON (sorted keys) and carries
//!   `record_hash`: the SHA-256 of the canonical JSON with the hash
//!   field itself empty. Re-ingesting the same art yields the same
//!   hash (the Phase 2 determinism gate).
//!
//! Portrait anchor derivations (no alpha is assumed: the canonical
//! portraits are opaque busts on gradient backdrops):
//! - Background estimate: median of the four 16x16 corner patches.
//!   On a gradient it matches no actual pixel; it is used only as an
//!   exclusion anchor for the eye/lips/hair candidate populations.
//! - Skin: the per-channel median of skin-like pixels over the whole
//!   portrait. "Skin-like" adds a warmth floor (`8 <= R-B < 80`) to
//!   the saturation/value/channel-order predicate: the backdrops are
//!   neutral to cool (`R-B <= 0`), and flat white otherwise satisfies
//!   the predicate and pollutes the median. There is no fallback; a
//!   portrait with too few skin-like pixels fails closed.
//! - Hair: median-cut palette of the portrait's upper area over a
//!   population pre-filtered to saturated, non-background pixels (a
//!   saturation-first ranking lets near-black line art, saturation
//!   0.44, beat the hair mass, 0.40); the winner is the most populous
//!   remaining entry whose share clears [`HAIR_MIN_SHARE`].
//! - Face segmentation: a seeded skin component. The densest window
//!   of strict skin matches in the portrait's upper half seeds a
//!   flood over a loose skin predicate (shadowed skin sits ~96 RGB
//!   units from the anchor, beyond [`COLOR_MATCH_DIST`], so the
//!   strict predicate alone cannot span a lit face). The component's
//!   row-span profile gives the box: top row, widest span, and the
//!   chin — the onset of the terminal narrowing, the first
//!   post-plateau row under [`FACE_CHIN_FRACTION`] of the widest span
//!   from which the span never recovers (mid-face dips from
//!   three-quarter hair occlusion recover and never read as chin).
//! - Eye band: face-relative rows/cols inside the segmented face box
//!   ([`FACE_EYE_ROWS`], [`FACE_EYE_COLS`]); the legacy fixed
//!   [`EYE_BAND`]/[`MOUTH_BAND`] fractions remain as the documented
//!   outer envelopes. Eye candidates are saturated, mid-bright
//!   (value > 0.25: lash/liner sits ~0.15 and the iris's shadow rim
//!   below 0.25 otherwise wins the cut), and match neither skin,
//!   hair, nor background; the winner is the dominant median-cut
//!   entry. Best-effort (`None` when nothing qualifies).
//! - Mouth band: face-relative ([`FACE_MOUTH_ROWS`],
//!   [`FACE_MOUTH_COLS`]). Lips are the dominant median-cut entry of
//!   reddish saturated pixels matching neither skin, hair, nor
//!   background; best-effort.
//! - Outfit primaries: the two most populous sprite colors (median-cut)
//!   that match neither skin nor hair.
//!
//! Head segmentation (sprite, v2). Pixels "match" an anchor when
//! opaque and within [`COLOR_MATCH_DIST`] of the portrait-derived
//! skin or hair color. The upper band is the top 40% of the alpha
//! bounding box.
//! - Face column: the median x of skin-matching pixels in the upper
//!   band (at least [`SPRITE_FACE_MIN_SKIN_PIXELS`]).
//! - Center run: per row, the width of the contiguous opaque run that
//!   contains the face column (0 when that pixel is transparent).
//!   Hair masses that hang beside the body, and a ponytail tip that
//!   rises off-center, are not part of it.
//! - Scalp: the first row with a non-zero center run. Rows above it
//!   (an off-center ponytail tip) are excluded from the figure.
//! - Chin: from the first center run that contains skin, the first
//!   stretch of rows narrower than [`JAW_NARROW_FRACTION`] of the
//!   widest run so far is the jaw/neck narrowing; the chin is its
//!   narrowest row (first on ties).
//! - Neck: the chin plus the canonical base's neck allowance
//!   ([`crate::base::base_neck_per_skull`] skull heights), so the
//!   head band (scalp to neck) has the same definition as the base's
//!   (`Neck` joint to the top) and the scales law is unchanged.
//! - Hip: the widest alpha row inside the 40..60% band of the figure.
//! - Hair bottom: tracing down from the scalp, the last row holding a
//!   hair-matching pixel before a run of more than
//!   [`HAIR_MAX_GAP_ROWS`] hair-free rows (`None` when the trace finds
//!   no hair).
//!
//! All rows in [`SpriteMeasurements`] are relative to the scalp.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use image::{AnimationDecoder, DynamicImage, GenericImageView, RgbaImage};
use serde::{Deserialize, Serialize};

use crate::base::base_proportions;
use crate::{FactoryError, sha256_hex};

/// Schema identifier emitted in every record.
pub const IDENTITY_SCHEMA: &str = "lumen.factory-identity";
/// Schema version emitted in every record. Version 2 (projection v2)
/// changed what the head band measures (color-segmented head instead
/// of the alpha band that counted hair mass); v1 records fail closed.
pub const IDENTITY_VERSION: u32 = 2;

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
/// Minimum skin-matching pixels in the sprite's upper band before the
/// face column is trusted (see the head segmentation module docs).
pub const SPRITE_FACE_MIN_SKIN_PIXELS: usize = 8;
/// A center run narrower than this fraction of the widest run seen
/// from the scalp belongs to the jaw/neck narrowing.
pub const JAW_NARROW_FRACTION: f64 = 0.6;
/// Hair-free rows tolerated inside the hair mass when tracing it down
/// from the scalp (pixel-art strands break up near the tips).
pub const HAIR_MAX_GAP_ROWS: u32 = 3;
/// Face-seed window edge (pixels): the densest window of this size
/// in the portrait's upper half seeds the skin component (see
/// [`face_seed`]).
pub const FACE_SEED_WINDOW: u32 = 64;
/// Step between candidate face-seed windows.
pub const FACE_SEED_STRIDE: u32 = 16;
/// A skin-component row span at this fraction of the widest span
/// counts as the face plateau (the cheeks) for chin detection.
pub const FACE_PLATEAU_FRACTION: f64 = 0.9;
/// A skin-component row span under this fraction of the widest span
/// counts as narrowed (jaw/neck) for chin detection.
pub const FACE_CHIN_FRACTION: f64 = 0.75;

/// Eye band: `(x0, x1, y0, y1)` fractions of the portrait. Legacy
/// fixed window, retained as the documented fallback envelope only:
/// eye sampling is derived from the segmented face box (see
/// [`FACE_EYE_ROWS`] / [`FACE_EYE_COLS`]) whenever segmentation
/// succeeds, because a fixed frame window misses any face that is
/// not centered exactly where the window assumes.
pub const EYE_BAND: (f64, f64, f64, f64) = (0.36, 0.64, 0.33, 0.40);
/// Mouth band: `(x0, x1, y0, y1)` fractions of the portrait. Legacy
/// fixed window; see [`EYE_BAND`] for why sampling is face-derived.
pub const MOUTH_BAND: (f64, f64, f64, f64) = (0.44, 0.56, 0.42, 0.47);
/// Eye sampling rows as fractions of the segmented face box
/// (`scalp..chin`): `(top, bottom)`. Generous by design — the eye
/// filter (saturation + hair/skin exclusion) does the precise work;
/// the band only has to contain both irises and exclude the mouth.
/// Measured against the Vivienne BEGIN portrait (1024x1920): irises
/// span 0.13..0.35 of the skin bounding box. The bottom is 0.60, not
/// 0.50, because the synthetic validation portrait's irises sit at
/// 0.41..0.56 of its face-only skin box (the toy has almost no
/// forehead); the band covers both calibrations.
pub const FACE_EYE_ROWS: (f64, f64) = (0.05, 0.60);
/// Eye sampling columns as fractions of the segmented face width,
/// centered on the face column: `(left, right)` extents. Measured
/// against the Vivienne BEGIN portrait: irises span 0.16..0.86 of
/// the skin bounding box width.
pub const FACE_EYE_COLS: (f64, f64) = (0.70, 0.70);
/// Mouth sampling rows as fractions of the segmented face box:
/// `(top, bottom)`. Measured against the Vivienne BEGIN portrait:
/// lips span 0.67..0.74 of the skin bounding box. The band starts at
/// 0.60 and runs to 0.98 because the synthetic validation portrait's
/// lips sit at 0.82..0.92 of its face-only skin box.
pub const FACE_MOUTH_ROWS: (f64, f64) = (0.60, 0.98);
/// Mouth sampling columns as fractions of the segmented face width,
/// centered on the face column: `(left, right)` extents. Measured
/// against the Vivienne BEGIN portrait: lips span 0.19..0.34 of the
/// skin bounding box width (left of center); the extents below are
/// deliberately asymmetric-safe: centered and wide enough to hold a
/// centered mouth while the red-dominance filter rejects the rest.
pub const FACE_MOUTH_COLS: (f64, f64) = (0.45, 0.45);

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

/// Raw sprite measurements behind the ratios (pixels). Rows are
/// relative to the scalp row (the figure top; see the head
/// segmentation module docs).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpriteMeasurements {
    /// Chin row: the narrowest row of the jaw/neck narrowing.
    pub chin_row: u32,
    /// Face column (x) the center run is measured through.
    pub face_center_x: u32,
    /// Last row of the hair mass traced down from the scalp, `None`
    /// when no hair-matching pixel was found.
    pub hair_bottom_row: Option<u32>,
    /// Widest alpha row inside the 40..60% band of the figure.
    pub hip_row: u32,
    /// Head band height (scalp to the neck row).
    pub head_height_px: u32,
    /// Leg span (hip row to the bottom of the figure).
    pub leg_span_px: u32,
    /// Neck row: the chin plus the base's neck allowance.
    pub neck_row: u32,
    /// Rows between the alpha bounding-box top and the scalp (an
    /// off-center ponytail tip, excluded from the figure).
    pub scalp_offset_px: u32,
    /// Skull height (scalp to chin), the color-segmented head.
    pub skull_height_px: u32,
    /// Torso span (neck row to hip row).
    pub torso_height_px: u32,
    /// Figure height (scalp to the bottom alpha row, inclusive).
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

/// Per-band scales for one horizontal axis (width or depth), in
/// the band order the scales law uses everywhere: head, torso,
/// leg. Each component is a scale against the canonical base,
/// finite in [`SCALE_MIN`]..=[`SCALE_MAX`] — the same fail-closed
/// range discipline as the Y band scales. A record that carries no
/// set at all resolves to exactly 1.0 per band (the locked base).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BandScaleSet {
    /// Head band scale.
    pub head: f64,
    /// Leg band scale.
    pub leg: f64,
    /// Torso band scale.
    pub torso: f64,
}

impl BandScaleSet {
    /// The scales as `(head, torso, leg)` f32, the order
    /// `base::ScaledBuild`'s band arrays use.
    pub fn resolved(&self) -> (f32, f32, f32) {
        (self.head as f32, self.torso as f32, self.leg as f32)
    }

    /// Fail closed when any component is non-finite or outside
    /// [`SCALE_MIN`]..=[`SCALE_MAX`] (never clamped).
    pub fn validate(&self, field: &str) -> Result<(), FactoryError> {
        let range = f64::from(SCALE_MIN)..=f64::from(SCALE_MAX);
        for (name, scale) in [
            ("head", self.head),
            ("torso", self.torso),
            ("leg", self.leg),
        ] {
            if !scale.is_finite() || !range.contains(&scale) {
                return Err(FactoryError::BadParam(format!(
                    "{field} {name} scale {scale} outside [{SCALE_MIN}, {SCALE_MAX}] (fail closed, not clamped)"
                )));
            }
        }
        Ok(())
    }
}

/// The canonical identity record (schema `lumen.factory-identity` v1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct IdentityRecord {
    /// Authored bust delta (bust − underbust circumference) in
    /// centimetres, finite in the range `base.rs` owns
    /// ([`crate::base::BUST_DELTA_MIN_CM`]..=
    /// [`crate::base::BUST_DELTA_MAX_CM`]) when present. This is
    /// AUTHORED per-character data: ingest leaves it `None`,
    /// because a sprite is a 2D silhouette and carries no depth —
    /// the Canon-11 finding is that the 2D bust footprint does not
    /// transfer to volume, so no depth-bearing value is ever
    /// derived from art. `None` reproduces the canonical base
    /// torso exactly; the field is omitted from serialization when
    /// `None`, so records written before this field existed keep
    /// identical bytes and hashes (GAP-11-3).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bust_delta_cm: Option<f64>,
    /// Companion name (the ingest key).
    pub companion: String,
    /// Authored per-band depth (Z) scales, or `None` for the
    /// locked base depths (every band exactly 1.0). Authored data,
    /// like [`Self::bust_delta_cm`]: ingest leaves it `None`, and
    /// the field is omitted from serialization when `None`
    /// (GAP-11-4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth_scales: Option<BandScaleSet>,
    /// Derived palette anchors plus the combined dominant palette.
    pub palette: Palette,
    /// Combined dominant colors (portrait + sprite), population order.
    pub palette_dominant: Vec<PaletteEntry>,
    /// Measured body proportions.
    pub proportions: Proportions,
    /// Role tag of the sprite the proportions were measured from
    /// (`sprite_mature` when present, else `sprite_base`).
    pub proportions_source: String,
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
    /// Authored per-band width (X) scales, or `None` for the
    /// locked base widths (every band exactly 1.0). See
    /// [`Self::depth_scales`] for the authored-data posture
    /// (GAP-11-4).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width_scales: Option<BandScaleSet>,
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

    /// The authored bust delta, validated against the range
    /// `base.rs` owns. `None` (the ingest posture, and every record
    /// written before the field existed) stays `None`. Fails closed
    /// on a non-finite or out-of-range value — never clamped.
    pub fn bust_delta_cm(&self) -> Result<Option<f64>, FactoryError> {
        match self.bust_delta_cm {
            None => Ok(None),
            Some(delta) => {
                let range = f64::from(crate::base::BUST_DELTA_MIN_CM)
                    ..=f64::from(crate::base::BUST_DELTA_MAX_CM);
                if !delta.is_finite() || !range.contains(&delta) {
                    return Err(FactoryError::BadParam(format!(
                        "bust_delta_cm {delta} outside [{}, {}] (fail closed, not clamped)",
                        crate::base::BUST_DELTA_MIN_CM,
                        crate::base::BUST_DELTA_MAX_CM
                    )));
                }
                Ok(Some(delta))
            }
        }
    }

    /// The per-band width scales `(head, torso, leg)` this record
    /// applies: the authored set, validated, or exactly
    /// `(1.0, 1.0, 1.0)` when the record carries no set (the locked
    /// base widths). Fails closed on an invalid set.
    pub fn width_scales(&self) -> Result<(f32, f32, f32), FactoryError> {
        match &self.width_scales {
            None => Ok((1.0, 1.0, 1.0)),
            Some(set) => {
                set.validate("width_scales")?;
                Ok(set.resolved())
            }
        }
    }

    /// The per-band depth scales `(head, torso, leg)`; the depth
    /// counterpart of [`Self::width_scales`].
    pub fn depth_scales(&self) -> Result<(f32, f32, f32), FactoryError> {
        match &self.depth_scales {
            None => Ok((1.0, 1.0, 1.0)),
            Some(set) => {
                set.validate("depth_scales")?;
                Ok(set.resolved())
            }
        }
    }

    /// The hair length this record measures, in the parameter
    /// record's `hair.length` units: how far the sprite's hair mass
    /// reaches below the neck row, as a fraction of the torso band
    /// (neck row to hip row) — the same band the build maps it onto
    /// (`base.rs`, the hair tail). Hair ending at or above the neck
    /// has no length below it (0.0), as does a record with no hair
    /// mass. Fails closed when the bands are degenerate or the hair
    /// reaches past the hips (> 1.0 is outside the modeled range;
    /// never clamped).
    pub fn hair_length(&self) -> Result<f64, FactoryError> {
        let m = &self.sprite_measurements;
        let Some(hair_bottom) = m.hair_bottom_row else {
            return Ok(0.0);
        };
        if m.hip_row <= m.neck_row {
            return Err(FactoryError::BadParam(format!(
                "sprite bands degenerate (neck {}, hip {})",
                m.neck_row, m.hip_row
            )));
        }
        let below_neck = f64::from(hair_bottom.saturating_sub(m.neck_row));
        let length = below_neck / f64::from(m.hip_row - m.neck_row);
        if length > 1.0 {
            return Err(FactoryError::BadParam(format!(
                "hair reaches {length:.3} of the torso band (past the hips; fail closed, not clamped)"
            )));
        }
        Ok(length)
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

/// Pixels inside `region` (frame fractions), strided, as RGB triples.
/// Transparent pixels never count: region sampling is figure sampling,
/// and the RGB stored under a transparent pixel is encoder residue, not
/// color evidence. On alpha art (the sprites) that makes region
/// sampling figure-only; on opaque portraits every pixel counts,
/// which is why the palette derivations filter by color instead.
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
            if px[3] >= ALPHA_OPAQUE {
                out.push([px[0], px[1], px[2]]);
            }
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
    // The quantizer sees saturated, non-background pixels only.
    // Feeding it the raw upper third let the dominant background eat
    // most of the boxes and fragment the figure's colors across the
    // rest — on the Vivienne BEGIN portrait the top qualifying box
    // came out near-black. Both exclusions therefore happen BEFORE
    // the cut: saturation (these portraits' backdrops are unsaturated
    // — white, gray, mauve-gray all sit below HAIR_MIN_SATURATION)
    // and background distance (a saturated backdrop, like the
    // synthetic fixtures' blue, is removed by the estimate instead).
    let upper: Vec<[u8; 3]> = region_pixels(portrait, (0.0, 1.0, 0.0, 1.0 / 3.0), 3)
        .into_iter()
        .filter(|px| {
            rgb_to_hsv(*px).1 >= HAIR_MIN_SATURATION
                && color_dist(*px, background) >= COLOR_MATCH_DIST
        })
        .collect();
    let entries = median_cut(&upper, ANCHOR_BOXES);
    // Rank by population share first, saturation second. The previous
    // saturation-first ranking picked lineart over hair: outline black
    // (13, 9, 16) has HSV saturation 0.44, above hair (79, 57, 95) at
    // 0.40, so the largest saturated mass in the upper third lost to
    // a thin outline. Hair is the DOMINANT saturated mass; the
    // saturation and share floors below are the qualifiers, share is
    // the ranking.
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
            best.is_none_or(|(bsh, bs, _)| entry.share > bsh || (entry.share == bsh && sat > bs));
        if dominated {
            best = Some((entry.share, sat, entry.color.rgb));
        }
    }
    best.map(|(_, _, rgb)| rgb).ok_or_else(|| {
        FactoryError::BadParam("no qualifying hair color in portrait upper third".to_string())
    })
}

/// Skin anchor: the median of the portrait's skin-like pixels over
/// the WHOLE portrait, not a frame window. The previous version
/// sampled FACE_REGION (a fixed x 0.34..0.66, y 0.24..0.46 window)
/// and fell back to the unfiltered region median when too few
/// skin-like pixels appeared there — so an off-center face yielded
/// the background color as its "skin" (the window held no face at
/// all), and a skinless portrait ingested successfully with an
/// invented anchor. Both are closed here, and the predicate carries
/// a warmth floor (R − B >= 8): the first whole-portrait version
/// admitted the portrait's own cool background (white has R − B = 0,
/// the BEGIN backdrop's mauve-gray has R − B = −8, and both passed
/// every other clause), landing the "skin" on the backdrop. Human
/// skin is warm by a wide margin (the BEGIN ground truth sits at
/// R − B = 32); with the floor, the whole-portrait median reproduces
/// the measured skin anchor exactly. Fewer than SKIN_MIN_PIXELS
/// skin-like pixels is an error, not a fallback. The median stays
/// robust when a warm outfit contributes a minority of skin-like
/// pixels; a portrait whose skin-like mass IS the outfit would
/// still mis-anchor, which the acceptance run on real art guards
/// against.
fn derive_skin(portrait: &RgbaImage) -> Result<[u8; 3], FactoryError> {
    let region = region_pixels(portrait, (0.0, 1.0, 0.0, 1.0), 2);
    if region.is_empty() {
        return Err(FactoryError::BadParam(
            "portrait has no opaque pixels".to_string(),
        ));
    }
    let skin_like: Vec<[u8; 3]> = region
        .iter()
        .copied()
        .filter(|px| {
            let (_, sat, val) = rgb_to_hsv(*px);
            sat < 0.38
                && val > 0.55
                && px[0] >= px[1]
                && (8..80).contains(&(px[0] as i32 - px[2] as i32))
        })
        .collect();
    if skin_like.len() >= SKIN_MIN_PIXELS {
        median_color(&skin_like).ok_or_else(|| FactoryError::BadParam("skin median".into()))
    } else {
        Err(FactoryError::BadParam(format!(
            "portrait has {} skin-like pixels (< {SKIN_MIN_PIXELS}); cannot derive a skin anchor",
            skin_like.len()
        )))
    }
}

fn derive_eye(
    portrait: &RgbaImage,
    background: [u8; 3],
    hair: [u8; 3],
    skin: [u8; 3],
    face: Option<&FaceBox>,
) -> Option<[u8; 3]> {
    let face = face?;
    let region = region_pixels(
        portrait,
        face_region(portrait, face, FACE_EYE_ROWS, FACE_EYE_COLS),
        1,
    );
    // Candidates exclude hair, skin, AND the background estimate by
    // anchor distance: the sampling rectangle always extends past
    // the head silhouette, and a saturated backdrop (the fixtures'
    // blue) passes the saturation/value predicate outright — without
    // the background exclusion its pixels outnumber the irises and
    // the median-cut lands on backdrop, not eye. The value floor
    // (0.25) separates iris from lash, liner, and the iris's own
    // shadow rim: on the BEGIN portrait the lash cluster sits at
    // value ~0.15 and the rim/shadow mass below 0.25 outvotes the
    // lit iris core, while both measured irises sit above 0.65 — a
    // floor at 0.10 let the lashes win the cut, and at 0.20 the rim
    // still did.
    let candidates: Vec<[u8; 3]> = region
        .iter()
        .copied()
        .filter(|px| {
            let (_, sat, val) = rgb_to_hsv(*px);
            sat > 0.30
                && val > 0.25
                && val < 0.90
                && color_dist(*px, hair) >= COLOR_MATCH_DIST
                && color_dist(*px, skin) >= COLOR_MATCH_DIST
                && color_dist(*px, background) >= COLOR_MATCH_DIST
        })
        .collect();
    if candidates.len() < 8 {
        return None;
    }
    let entries = median_cut(&candidates, 4);
    entries.first().map(|e| e.color.rgb)
}

/// The segmented face box on a portrait: the geometry eye and mouth
/// sampling is derived from, instead of a fixed frame window (see
/// [`EYE_BAND`]).
struct FaceBox {
    /// Face column x (median skin-component x over `top..=chin`).
    center_x: u32,
    /// Chin row: onset of the face component's terminal narrowing
    /// (the neck), or its last row when it never narrows.
    chin: u32,
    /// Widest skin-component row span across the face.
    width: u32,
    /// First row of the skin component (the sampling box top).
    top: u32,
}

/// Strict skin predicate for face seeding: the pixel must match the
/// derived skin anchor AND carry skin's warmth signature (the
/// [`derive_skin`] predicate). Anchor distance alone admits the
/// white backdrop (40.0 < [`COLOR_MATCH_DIST`] from the BEGIN skin
/// anchor); warmth alone admits any warm tone in the frame.
/// Together they name lit skin and very little else.
fn strict_skin(px: [u8; 3], skin: [u8; 3]) -> bool {
    let (_, sat, val) = rgb_to_hsv(px);
    color_dist(px, skin) < COLOR_MATCH_DIST
        && sat < 0.38
        && val > 0.55
        && px[0] >= px[1]
        && (8..80).contains(&(i32::from(px[0]) - i32::from(px[2])))
}

/// Loose skin predicate for the face-component flood. Shadowed skin
/// falls outside the anchor's [`COLOR_MATCH_DIST`] (the BEGIN
/// portrait's shadow cheek measures ~96 away), so the flood admits
/// the wider warmth family, fenced by saturation/value bounds, a
/// hair exclusion, and a generous anchor-distance cap. Flooding
/// from a strict seed is what keeps the backdrop out: its warm
/// tones pass this predicate too, but they are not connected to the
/// face through skin — hair and lineart break the path. Background
/// keying was measured and rejected on the real art three ways
/// (estimate keying matches nothing on a gradient; fixed-distance
/// keying eats the skin; flood fills leak through JPEG-softened
/// edges into 93% of the hair), which is why segmentation is a
/// seeded component and not a keyed mask.
fn loose_skin(px: [u8; 3], skin: [u8; 3], hair: [u8; 3]) -> bool {
    let (_, sat, val) = rgb_to_hsv(px);
    sat < 0.42
        && val > 0.42
        && i32::from(px[0]) + 10 >= i32::from(px[1])
        && (6..110).contains(&(i32::from(px[0]) - i32::from(px[2])))
        && color_dist(px, hair) >= 40.0
        && color_dist(px, skin) < 130.0
}

/// Seed pixel for the face component: the first strict-skin pixel
/// of the densest [`FACE_SEED_WINDOW`]-sized window in the
/// portrait's upper half (windows stepped by [`FACE_SEED_STRIDE`],
/// density sampled at stride 2). The face is the densest skin mass
/// in the upper half of any portrait this crate ingests; a backdrop
/// can match individual pixels, but not at skin's density in a
/// window that size.
fn face_seed(img: &RgbaImage, skin: [u8; 3]) -> Option<(u32, u32)> {
    let (w, h) = (img.width(), img.height());
    let win = FACE_SEED_WINDOW.min(w).min(h);
    if win == 0 {
        return None;
    }
    let strict_at = |x: u32, y: u32| {
        let px = img.get_pixel(x, y);
        px[3] >= ALPHA_OPAQUE && strict_skin([px[0], px[1], px[2]], skin)
    };
    let mut best: Option<(usize, u32, u32)> = None;
    let mut y0 = 0;
    loop {
        let mut x0 = 0;
        loop {
            let mut count = 0usize;
            let mut sy = y0;
            while sy < y0 + win {
                let mut sx = x0;
                while sx < x0 + win {
                    if strict_at(sx, sy) {
                        count += 1;
                    }
                    sx += 2;
                }
                sy += 2;
            }
            if count > 0 && best.is_none_or(|(b, _, _)| count > b) {
                // First strict pixel of the window, in scan order.
                'scan: for sy in y0..y0 + win {
                    for sx in x0..x0 + win {
                        if strict_at(sx, sy) {
                            best = Some((count, sx, sy));
                            break 'scan;
                        }
                    }
                }
            }
            if x0 + win >= w {
                break;
            }
            x0 += FACE_SEED_STRIDE.min(w - win - x0);
        }
        if y0 + win >= h || y0 >= h / 2 {
            break;
        }
        y0 += FACE_SEED_STRIDE.min(h / 2 - y0);
    }
    best.map(|(_, x, y)| (x, y))
}

/// The face skin component: every loose-skin pixel connected
/// (4-neighborhood) to the seed, as per-row sorted x lists.
fn skin_component(
    img: &RgbaImage,
    skin: [u8; 3],
    hair: [u8; 3],
    seed: (u32, u32),
) -> std::collections::BTreeMap<u32, Vec<u32>> {
    let (w, h) = (img.width(), img.height());
    let mut visited = vec![false; (w * h) as usize];
    let mut rows: std::collections::BTreeMap<u32, Vec<u32>> = std::collections::BTreeMap::new();
    let mut queue = std::collections::VecDeque::from([seed]);
    visited[(seed.1 * w + seed.0) as usize] = true;
    while let Some((x, y)) = queue.pop_front() {
        rows.entry(y).or_default().push(x);
        for (nx, ny) in [
            (x.wrapping_sub(1), y),
            (x + 1, y),
            (x, y.wrapping_sub(1)),
            (x, y + 1),
        ] {
            if nx >= w || ny >= h || visited[(ny * w + nx) as usize] {
                continue;
            }
            visited[(ny * w + nx) as usize] = true;
            let px = img.get_pixel(nx, ny);
            if px[3] >= ALPHA_OPAQUE && loose_skin([px[0], px[1], px[2]], skin, hair) {
                queue.push_back((nx, ny));
            }
        }
    }
    for xs in rows.values_mut() {
        xs.sort_unstable();
    }
    rows
}

/// A row's robust span: the 1st-to-99th percentile width of its
/// component pixels, so a stray connected pixel cannot stretch the
/// row. Rows under 10 pixels report 0 (no reliable span).
fn row_span(xs: &[u32]) -> u32 {
    if xs.len() < 10 {
        return 0;
    }
    xs[(0.99 * xs.len() as f64) as usize] - xs[(0.01 * xs.len() as f64) as usize]
}

/// Segment the face on a portrait: flood the skin component from
/// its densest strict seed (see [`face_seed`] / [`skin_component`]),
/// then read the box off the component's row-span profile. The
/// span grows as the face emerges from the hairline, plateaus
/// across the cheeks, and falls for good at the jaw — the chin is
/// the onset of that terminal narrowing: the first post-plateau row
/// under [`FACE_CHIN_FRACTION`] of the widest span from which the
/// span never recovers. A component that never narrows (a face
/// cropped at the jaw — the synthetic fixtures) ends at its last
/// row instead. Mid-face span dips (a three-quarter hair occlusion
/// narrows the VISIBLE skin for tens of rows) never read as the
/// chin, because the span recovers to the plateau behind them.
/// Returns `None` when no face component exists; callers then leave
/// eye/lips anchors absent rather than inventing them from a fixed
/// frame window, which is what poisoned the old anchors on any face
/// that was not centered exactly where the window assumed.
fn segment_face(img: &RgbaImage, skin: [u8; 3], hair: [u8; 3]) -> Option<FaceBox> {
    if img.width() == 0 || img.height() == 0 {
        return None;
    }
    let seed = face_seed(img, skin)?;
    let rows = skin_component(img, skin, hair, seed);
    let total: usize = rows.values().map(Vec::len).sum();
    if total < SPRITE_FACE_MIN_SKIN_PIXELS {
        return None;
    }
    let top = *rows.keys().next()?;
    let last = *rows.keys().next_back()?;
    let spans: Vec<(u32, u32)> = rows.iter().map(|(y, xs)| (*y, row_span(xs))).collect();
    let width = spans.iter().map(|(_, s)| *s).max()?;
    if width == 0 {
        return None;
    }
    let plateau = spans
        .iter()
        .find(|(_, s)| f64::from(*s) >= FACE_PLATEAU_FRACTION * f64::from(width))
        .map(|(y, _)| *y)?;
    // Terminal narrowing: the first post-plateau row under the chin
    // fraction whose every successor is also under it.
    let mut chin = last;
    let mut suffix_all_narrow = true;
    for (y, s) in spans.iter().rev() {
        if *y < plateau {
            break;
        }
        if f64::from(*s) >= FACE_CHIN_FRACTION * f64::from(width) {
            suffix_all_narrow = false;
        } else if suffix_all_narrow {
            chin = *y;
        }
    }
    if chin <= top {
        return None;
    }
    let mut xs: Vec<u32> = Vec::new();
    for (y, row) in &rows {
        if *y >= top && *y <= chin {
            xs.extend(row.iter().copied());
        }
    }
    xs.sort_unstable();
    let center_x = xs[xs.len() / 2];
    Some(FaceBox {
        center_x,
        chin,
        width,
        top,
    })
}

/// A sampling region (frame fractions) from a face box: `rows` are
/// fractions of `top..chin`, `cols` are left/right extents from the
/// face column as multiples of the face width, clamped to the frame.
fn face_region(
    img: &RgbaImage,
    face: &FaceBox,
    rows: (f64, f64),
    cols: (f64, f64),
) -> (f64, f64, f64, f64) {
    let (w, h) = (img.width() as f64, img.height() as f64);
    let height = (face.chin - face.top) as f64;
    let y0 = (face.top as f64 + rows.0 * height) / h;
    let y1 = (face.top as f64 + rows.1 * height) / h;
    let x0 = (face.center_x as f64 - cols.0 * face.width as f64) / w;
    let x1 = (face.center_x as f64 + cols.1 * face.width as f64) / w;
    (x0.max(0.0), x1.min(1.0), y0.max(0.0), y1.min(1.0))
}

fn derive_lips(
    portrait: &RgbaImage,
    background: [u8; 3],
    hair: [u8; 3],
    skin: [u8; 3],
    face: Option<&FaceBox>,
) -> Option<[u8; 3]> {
    let face = face?;
    let region = region_pixels(
        portrait,
        face_region(portrait, face, FACE_MOUTH_ROWS, FACE_MOUTH_COLS),
        1,
    );
    // Hair, skin, and background are excluded by anchor distance, as
    // in derive_eye: the saturation/red-dominance predicate alone
    // also admits saturated hair (magenta hair passes both tests),
    // and the mouth band always contains the hair sides framing the
    // face — without the exclusion the median lands on hair whenever
    // the sides outnumber the lips, which the off-center fixture
    // proved.
    let candidates: Vec<[u8; 3]> = region
        .iter()
        .copied()
        .filter(|px| {
            let (_, sat, _) = rgb_to_hsv(*px);
            sat > 0.22
                && px[0] as i32 - px[2] as i32 > 12
                && color_dist(*px, hair) >= COLOR_MATCH_DIST
                && color_dist(*px, skin) >= COLOR_MATCH_DIST
                && color_dist(*px, background) >= COLOR_MATCH_DIST
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

// --- proportions from the sprite (color-aware head segmentation) ---

fn pixel_matches(sprite: &RgbaImage, x: u32, y: u32, anchor: [u8; 3]) -> bool {
    let px = sprite.get_pixel(x, y);
    px[3] >= ALPHA_OPAQUE && color_dist([px[0], px[1], px[2]], anchor) < COLOR_MATCH_DIST
}

fn row_matches(sprite: &RgbaImage, y: u32, anchor: [u8; 3]) -> bool {
    (0..sprite.width()).any(|x| pixel_matches(sprite, x, y, anchor))
}

/// The contiguous opaque run of row `y` containing column `x`, as
/// `(width, left, right)`; width 0 when that pixel is transparent.
fn center_run(sprite: &RgbaImage, x: u32, y: u32) -> (u32, u32, u32) {
    let opaque = |cx: u32| sprite.get_pixel(cx, y)[3] >= ALPHA_OPAQUE;
    if !opaque(x) {
        return (0, x, x);
    }
    let mut left = x;
    while left > 0 && opaque(left - 1) {
        left -= 1;
    }
    let mut right = x;
    while right + 1 < sprite.width() && opaque(right + 1) {
        right += 1;
    }
    (right - left + 1, left, right)
}

/// Median x of skin-matching pixels in rows `top..upper_end`.
fn face_column(
    sprite: &RgbaImage,
    skin: [u8; 3],
    top: u32,
    upper_end: u32,
) -> Result<u32, FactoryError> {
    let mut xs: Vec<u32> = Vec::new();
    for y in top..upper_end {
        for x in 0..sprite.width() {
            if pixel_matches(sprite, x, y, skin) {
                xs.push(x);
            }
        }
    }
    if xs.len() < SPRITE_FACE_MIN_SKIN_PIXELS {
        return Err(FactoryError::BadParam(format!(
            "sprite upper band has {} skin-colored pixels (< {SPRITE_FACE_MIN_SKIN_PIXELS}); \
             cannot locate the face",
            xs.len()
        )));
    }
    xs.sort_unstable();
    Ok(xs[xs.len() / 2])
}

/// The chin row: narrowest row of the first jaw/neck narrowing below
/// the face (module docs), or `None` when no narrowing is found.
fn find_chin(sprite: &RgbaImage, face_x: u32, skin: [u8; 3], scalp: u32, end: u32) -> Option<u32> {
    let mut head_width = 0u32;
    let mut in_face = false;
    let mut best: Option<(u32, u32)> = None;
    for y in scalp..end {
        let (width, left, right) = center_run(sprite, face_x, y);
        if !in_face {
            in_face = width > 0 && (left..=right).any(|x| pixel_matches(sprite, x, y, skin));
        }
        let narrow = f64::from(width) < JAW_NARROW_FRACTION * f64::from(head_width);
        match best {
            None if in_face && narrow => best = Some((width, y)),
            None => head_width = head_width.max(width),
            Some(_) if !narrow => break,
            Some((best_width, _)) if width < best_width => best = Some((width, y)),
            Some(_) => {}
        }
    }
    best.map(|(_, y)| y)
}

/// Last row of the hair mass traced down from `scalp` (module docs).
fn hair_bottom(sprite: &RgbaImage, hair: [u8; 3], scalp: u32, bottom: u32) -> Option<u32> {
    let mut last: Option<u32> = None;
    let mut gap = 0u32;
    for y in scalp..=bottom {
        if row_matches(sprite, y, hair) {
            last = Some(y);
            gap = 0;
        } else {
            gap += 1;
            if gap > HAIR_MAX_GAP_ROWS {
                break;
            }
        }
    }
    last
}

/// Alpha bounding rows `(top, bottom)` of a sprite; fails closed when
/// fully transparent or shorter than [`MIN_SPRITE_EDGE`].
fn alpha_rows(sprite: &RgbaImage) -> Result<(u32, u32), FactoryError> {
    let opaque_row =
        |y: u32| (0..sprite.width()).any(|x| sprite.get_pixel(x, y)[3] >= ALPHA_OPAQUE);
    let top = (0..sprite.height()).find(|&y| opaque_row(y));
    let bottom = (0..sprite.height()).rev().find(|&y| opaque_row(y));
    let (Some(top), Some(bottom)) = (top, bottom) else {
        return Err(FactoryError::BadParam(
            "sprite is fully transparent".to_string(),
        ));
    };
    let total = bottom - top + 1;
    if total < MIN_SPRITE_EDGE {
        return Err(FactoryError::BadParam(format!(
            "sprite alpha height {total} is degenerate"
        )));
    }
    Ok((top, bottom))
}

/// Widest alpha row in rows `lo..=hi` (first on ties).
fn widest_row(sprite: &RgbaImage, lo: u32, hi: u32) -> u32 {
    let count = |y: u32| {
        (0..sprite.width())
            .filter(|&x| sprite.get_pixel(x, y)[3] >= ALPHA_OPAQUE)
            .count()
    };
    let mut widest = lo;
    for y in lo..=hi {
        if count(y) > count(widest) {
            widest = y;
        }
    }
    widest
}

/// Measure body proportions from a sprite frame, segmenting the head
/// by color (`skin` / `hair` anchors from the portrait) as well as
/// alpha; see the module docs for every definition. Fails closed on a
/// transparent or degenerate sprite, a sprite with no skin-colored
/// face, no jaw narrowing below the face, or degenerate bands.
pub fn measure_proportions(
    sprite: &RgbaImage,
    skin: [u8; 3],
    hair: [u8; 3],
) -> Result<(Proportions, SpriteMeasurements), FactoryError> {
    let (alpha_top, bottom) = alpha_rows(sprite)?;
    let upper_end = alpha_top + ((bottom - alpha_top + 1) as f64 * 0.40) as u32;
    let face_x = face_column(sprite, skin, alpha_top, upper_end)?;
    let scalp = (alpha_top..upper_end)
        .find(|&y| center_run(sprite, face_x, y).0 > 0)
        .ok_or_else(|| FactoryError::BadParam("sprite face column has no head".to_string()))?;
    let chin = find_chin(sprite, face_x, skin, scalp, upper_end).ok_or_else(|| {
        FactoryError::BadParam("no jaw narrowing below the face; cannot segment the head".into())
    })?;
    let total = bottom - scalp + 1;
    let skull = chin - scalp;
    let neck = skull + (skull as f32 * crate::base::base_neck_per_skull()).round() as u32;
    let band_lo = scalp + (total as f64 * 0.40) as u32;
    let band_hi = (scalp + (total as f64 * 0.60) as u32).min(bottom);
    let hip = widest_row(sprite, band_lo, band_hi) - scalp;
    if skull == 0 || hip <= neck {
        return Err(FactoryError::BadParam(format!(
            "sprite bands degenerate (skull {skull}, neck {neck}, hip {hip}, total {total})"
        )));
    }
    let total_f = f64::from(total);
    let proportions = Proportions {
        head_ratio: f64::from(neck) / total_f,
        leg_ratio: f64::from(total - hip) / total_f,
        torso_ratio: f64::from(hip - neck) / total_f,
    };
    let measurements = SpriteMeasurements {
        chin_row: skull,
        face_center_x: face_x,
        hair_bottom_row: hair_bottom(sprite, hair, scalp, bottom).map(|y| y - scalp),
        hip_row: hip,
        head_height_px: neck,
        leg_span_px: total - hip,
        neck_row: neck,
        scalp_offset_px: scalp - alpha_top,
        skull_height_px: skull,
        torso_height_px: hip - neck,
        total_height_px: total,
        width_px: sprite.width(),
    };
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

    // Decode the two images that carry identity data. No background
    // keying anywhere: it was measured and rejected on the real art
    // (see loose_skin). Anchors come from color statistics over
    // the portrait as decoded; segmentation runs on the anchor mask.
    let portrait = load_portrait(&portrait_path)?;
    let sprite = load_gif_frame0(&sprite_path)?;

    // Palette anchors.
    let background = background_estimate(&portrait);
    let hair = derive_hair(&portrait, background)?;
    let skin = derive_skin(&portrait)?;
    let face = segment_face(&portrait, skin, hair);
    let eye = derive_eye(&portrait, background, hair, skin, face.as_ref());
    let lips = derive_lips(&portrait, background, hair, skin, face.as_ref());
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

    // Proportions: mature sprite when present (the default target),
    // segmented by the portrait's skin and hair anchors.
    let mature = mature_path.as_deref().map(load_gif_frame0).transpose()?;
    let (measured, proportions_source) = match &mature {
        Some(frame) => (frame, "sprite_mature"),
        None => (&sprite, "sprite_base"),
    };
    let (proportions, sprite_measurements) = measure_proportions(measured, skin, hair)?;

    let mut record = IdentityRecord {
        // Ingest measures only what art can carry: the authored
        // frame fields (bust delta, width/depth scales) stay None —
        // a sprite silhouette has no depth to derive them from.
        bust_delta_cm: None,
        companion: companion.to_string(),
        depth_scales: None,
        palette: Palette {
            eye: eye.map(ColorEntry::new),
            hair: ColorEntry::new(hair),
            lips: lips.map(ColorEntry::new),
            outfit,
            skin: ColorEntry::new(skin),
        },
        palette_dominant,
        proportions,
        proportions_source: proportions_source.to_string(),
        record_hash: String::new(),
        schema: IDENTITY_SCHEMA.to_string(),
        sources,
        sprite_measurements,
        version: IDENTITY_VERSION,
        width_scales: None,
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
