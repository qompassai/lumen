//! Palette presets, ramps, quantization, and palette inspection.
//!
//! Contract summary:
//! - Colors cross the wire as `#RRGGBB` / `#RRGGBBAA` hex strings.
//! - Layer selection is a 0-based index; `None` means every layer.
//! - Fully transparent pixels (alpha 0) are never recolored and never counted.
//! - Palette mapping uses squared Euclidean RGB distance (ties go to the earlier
//!   entry); the output alpha is always the source pixel's alpha.
//! - Bounds: custom palettes 2..=256 entries; extract 1..=1024 colors; quantize
//!   2..=256 colors; ramps 2..=256 steps; at most `DISTINCT_COLORS_MAX` distinct
//!   colors are tracked per call — more is a `BadParam`, never unbounded growth.
//! - Mutating tools save to `output` (must end in `.lumen.json`) or, when
//!   `output` is `None`, back over the input document.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::ops::Range;
use std::path::Path;

use image::RgbaImage;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::LumenError;
use crate::doc::{DocSaved, SpriteDoc, load_doc, save_doc};

const CUSTOM_PALETTE_MIN: usize = 2;
const CUSTOM_PALETTE_MAX: usize = 256;
/// Longest accepted `palette` parameter: 256 entries of `#RRGGBBAA, ` fit.
const PALETTE_SPEC_BYTES_MAX: usize = 4096;
const EXTRACT_COLORS_MAX: u32 = 1024;
const QUANTIZE_COLORS_MIN: u32 = 2;
const QUANTIZE_COLORS_MAX: u32 = 256;
const RAMP_STEPS_MIN: u32 = 2;
const RAMP_STEPS_MAX: u32 = 256;
/// Distinct-color budget per call (~1M entries, tens of MiB of map memory).
const DISTINCT_COLORS_MAX: usize = 1 << 20;
/// Peak ordered-dither offset is +-(DITHER_SPREAD / 2) per channel, in 8-bit
/// units. Fixed rather than palette-derived to keep results predictable.
const DITHER_SPREAD: i32 = 32;
const BAYER_4X4: [[i32; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

struct Preset {
    name: &'static str,
    rgb: &'static [u32],
}

/// Built-in palettes, authored entry-by-entry as 0xRRGGBB. Provenance notes
/// live with each table; none is claimed to be byte-verified against an
/// upstream source.
const PRESETS: [Preset; 5] = [
    Preset { name: "pico8", rgb: &PICO8 },
    Preset { name: "sweetie16", rgb: &SWEETIE16 },
    Preset { name: "gameboy", rgb: &GAMEBOY },
    Preset { name: "nes", rgb: &NES },
    Preset { name: "endesga32", rgb: &ENDESGA32 },
];

/// PICO-8's 16-color base palette.
const PICO8: [u32; 16] = [
    0x000000, 0x1D2B53, 0x7E2553, 0x008751, 0xAB5236, 0x5F574F, 0xC2C3C7, 0xFFF1E8, 0xFF004D,
    0xFFA300, 0xFFEC27, 0x00E436, 0x29ADFF, 0x83769C, 0xFF77A8, 0xFFCCAA,
];

/// Sweetie 16 (GrafxKid).
const SWEETIE16: [u32; 16] = [
    0x1A1C2C, 0x5D275D, 0xB13E53, 0xEF7D57, 0xFFCD75, 0xA7F070, 0x38B764, 0x257179, 0x29366F,
    0x3B5DC9, 0x41A6F6, 0x73EFF7, 0xF4F4F4, 0x94B0C2, 0x566C86, 0x333C57,
];

/// Four-shade green "DMG" look, darkest first.
const GAMEBOY: [u32; 4] = [0x0F380F, 0x306230, 0x8BAC0F, 0x9BBC0F];

/// NES-style 2C02 rendition: rows $00-$0C, $10-$1C, $21-$2D, $30-$3D, then one
/// black. The duplicate blacks ($xE/$xF, $0D, $1D) collapse into the final
/// black and $20 is dropped as a duplicate of white $30, giving 54 entries.
const NES: [u32; 54] = [
    0x7C7C7C, 0x0000FC, 0x0000BC, 0x4428BC, 0x940084, 0xA80020, 0xA81000, 0x881400, 0x503000,
    0x007800, 0x006800, 0x005800, 0x004058, 0xBCBCBC, 0x0078F8, 0x0058F8, 0x6844FC, 0xD800CC,
    0xE40058, 0xF83800, 0xE45C10, 0xAC7C00, 0x00B800, 0x00A800, 0x00A844, 0x008888, 0x3CBCFC,
    0x6888FC, 0x9878F8, 0xF878F8, 0xF85898, 0xF87858, 0xFCA044, 0xF8B800, 0xB8F818, 0x58D854,
    0x58F898, 0x00E8D8, 0x787878, 0xFCFCFC, 0xA4E4FC, 0xB8B8F8, 0xD8B8F8, 0xF8B8F8, 0xF8A4C0,
    0xF0D0B0, 0xFCE0A8, 0xF8D878, 0xD8F878, 0xB8F8B8, 0xB8F8D8, 0x00FCFC, 0xF8D8F8, 0x000000,
];

/// ENDESGA 32.
const ENDESGA32: [u32; 32] = [
    0xBE4A2F, 0xD77643, 0xEAD4AA, 0xE4A672, 0xB86F50, 0x733E39, 0x3E2731, 0xA22633, 0xE43B44,
    0xF77622, 0xFEAE34, 0xFEE761, 0x63C74D, 0x3E8948, 0x265C42, 0x193C3E, 0x124E89, 0x0099DB,
    0x2CE8F5, 0xFFFFFF, 0xC0CBDC, 0x8B9BB4, 0x5A6988, 0x3A4466, 0x262B44, 0x181425, 0xFF0044,
    0x68386C, 0xB55088, 0xF6757A, 0xE8B796, 0xC28569,
];

// ---------------------------------------------------------------------------
// Requests and results
// ---------------------------------------------------------------------------

/// `palette_presets` takes no parameters.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct PalettePresetsRequest {}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PaletteApplyRequest {
    /// Project-relative input `.lumen.json`.
    pub doc: String,
    /// Output `.lumen.json`; `None` saves over the input.
    pub output: Option<String>,
    /// 0-based layer index; `None` applies to every layer.
    pub layer: Option<usize>,
    /// Preset name (see `palette_presets`) or comma-separated hex list
    /// (2..=256 entries), e.g. `"#000000,#FFFFFF"`.
    pub palette: String,
    /// Ordered (Bayer 4x4) dithering before the nearest-color match.
    pub dither: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PaletteExtractRequest {
    /// Project-relative input `.lumen.json`.
    pub doc: String,
    /// 0-based layer index; `None` counts every layer.
    pub layer: Option<usize>,
    /// Most colors to return, 1..=1024.
    pub max_colors: u32,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PaletteRampRequest {
    /// Start color, `#RRGGBB` or `#RRGGBBAA` (alpha ignored).
    pub from: String,
    /// End color, `#RRGGBB` or `#RRGGBBAA` (alpha ignored).
    pub to: String,
    /// Ramp length including both ends, 2..=256.
    pub steps: u32,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct QuantizeRequest {
    /// Project-relative input `.lumen.json`.
    pub doc: String,
    /// Output `.lumen.json`; `None` saves over the input.
    pub output: Option<String>,
    /// 0-based layer index; `None` quantizes every layer.
    pub layer: Option<usize>,
    /// Target palette size, 2..=256.
    pub max_colors: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct PaletteList {
    pub palettes: Vec<PaletteInfo>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PaletteInfo {
    pub name: String,
    pub entries: usize,
    /// `#RRGGBB` strings.
    pub colors: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ExtractedPalette {
    /// Sorted by count descending, then color ascending.
    pub colors: Vec<CountedColor>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CountedColor {
    /// `#RRGGBBAA`.
    pub color: String,
    pub count: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ColorRamp {
    /// `#RRGGBB` strings, `from` first, `to` last.
    pub colors: Vec<String>,
}

// ---------------------------------------------------------------------------
// Tools
// ---------------------------------------------------------------------------

/// List the built-in palettes. `_root` is unused; kept for a uniform signature.
pub async fn palette_presets(
    _root: &Path,
    _req: PalettePresetsRequest,
) -> Result<PaletteList, LumenError> {
    let palettes = PRESETS
        .iter()
        .map(|p| PaletteInfo {
            name: p.name.to_string(),
            entries: p.rgb.len(),
            colors: p.rgb.iter().map(|&c| hex_rgb(rgba_from_u32(c))).collect(),
        })
        .collect();
    Ok(PaletteList { palettes })
}

/// Map every non-transparent pixel of the selected layer(s) to its nearest
/// palette entry, optionally with ordered dithering. The document's bound
/// palette (`doc.palette`) is left unchanged.
pub async fn palette_apply(
    root: &Path,
    req: PaletteApplyRequest,
) -> Result<DocSaved, LumenError> {
    let target = output_target(&req.doc, req.output.as_deref())?;
    let entries = resolve_palette(&req.palette)?;
    let mut doc = load_doc(root, &req.doc)?;
    let range = layer_range(&doc, req.layer)?;
    for layer in &mut doc.layers[range] {
        map_image(&mut layer.image, &entries, req.dither);
    }
    let path = save_doc(root, &doc, target)?;
    Ok(DocSaved::of(&path, &doc))
}

/// Count the distinct non-transparent RGBA colors of the selected layer(s)
/// (raw layer pixels, not the composite). Read-only.
pub async fn palette_extract(
    root: &Path,
    req: PaletteExtractRequest,
) -> Result<ExtractedPalette, LumenError> {
    if req.max_colors == 0 || req.max_colors > EXTRACT_COLORS_MAX {
        return Err(LumenError::BadParam(format!(
            "max_colors {} outside 1..={EXTRACT_COLORS_MAX}",
            req.max_colors
        )));
    }
    let doc = load_doc(root, &req.doc)?;
    let range = layer_range(&doc, req.layer)?;
    let counts = count_colors(doc.layers[range].iter().map(|l| &l.image))?;
    let mut sorted: Vec<([u8; 4], u64)> = counts.into_iter().collect();
    sorted.sort_unstable_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    sorted.truncate(req.max_colors as usize);
    let colors = sorted
        .into_iter()
        .map(|(c, count)| CountedColor { color: hex_rgba(c), count })
        .collect();
    Ok(ExtractedPalette { colors })
}

/// Linear RGB interpolation from `from` to `to` inclusive. Pure; no doc I/O.
pub async fn palette_ramp(
    _root: &Path,
    req: PaletteRampRequest,
) -> Result<ColorRamp, LumenError> {
    if !(RAMP_STEPS_MIN..=RAMP_STEPS_MAX).contains(&req.steps) {
        return Err(LumenError::BadParam(format!(
            "steps {} outside {RAMP_STEPS_MIN}..={RAMP_STEPS_MAX}",
            req.steps
        )));
    }
    let from = parse_color(&req.from)?;
    let to = parse_color(&req.to)?;
    let last = req.steps - 1;
    let colors = (0..req.steps)
        .map(|i| {
            let mut c = [0u8, 0, 0, 255];
            for ch in 0..3 {
                // Integer lerp with round-half-up; both weights sum to `last`.
                let num = u32::from(from[ch]) * (last - i) + u32::from(to[ch]) * i;
                c[ch] = ((num + last / 2) / last) as u8;
            }
            hex_rgb(c)
        })
        .collect();
    Ok(ColorRamp { colors })
}

/// Median-cut quantization: one palette is built from the RGB colors of all
/// selected layers, then applied to each of them. Alpha is preserved.
pub async fn quantize(root: &Path, req: QuantizeRequest) -> Result<DocSaved, LumenError> {
    let target = output_target(&req.doc, req.output.as_deref())?;
    if !(QUANTIZE_COLORS_MIN..=QUANTIZE_COLORS_MAX).contains(&req.max_colors) {
        return Err(LumenError::BadParam(format!(
            "max_colors {} outside {QUANTIZE_COLORS_MIN}..={QUANTIZE_COLORS_MAX}",
            req.max_colors
        )));
    }
    let mut doc = load_doc(root, &req.doc)?;
    let range = layer_range(&doc, req.layer)?;
    let mut rgb_counts: HashMap<[u8; 3], u64> = HashMap::new();
    for (c, count) in count_colors(doc.layers[range.clone()].iter().map(|l| &l.image))? {
        *rgb_counts.entry([c[0], c[1], c[2]]).or_insert(0) += count;
    }
    let boxes = median_cut(rgb_counts.into_iter().collect(), req.max_colors as usize);
    let mut mapping: HashMap<[u8; 3], [u8; 3]> = HashMap::new();
    for bx in boxes.iter().filter(|b| !b.is_empty()) {
        let rep = box_mean(bx);
        for (c, _) in bx {
            mapping.insert(*c, rep);
        }
    }
    for layer in &mut doc.layers[range] {
        for px in layer.image.pixels_mut() {
            if px[3] == 0 {
                continue;
            }
            if let Some(rep) = mapping.get(&[px[0], px[1], px[2]]) {
                px.0 = [rep[0], rep[1], rep[2], px[3]];
            }
        }
    }
    let path = save_doc(root, &doc, target)?;
    Ok(DocSaved::of(&path, &doc))
}

// ---------------------------------------------------------------------------
// Private machinery
// ---------------------------------------------------------------------------

fn parse_color(s: &str) -> Result<[u8; 4], LumenError> {
    let bad = || LumenError::BadParam(format!("color '{s:.32}' is not #RRGGBB or #RRGGBBAA"));
    let hex = s.strip_prefix('#').ok_or_else(bad)?;
    if !(hex.len() == 6 || hex.len() == 8) || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(bad());
    }
    // All-ASCII was just checked, so byte slicing stays on char boundaries.
    let mut out = [0u8, 0, 0, 255];
    for (i, slot) in out.iter_mut().enumerate().take(hex.len() / 2) {
        *slot = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).map_err(|_| bad())?;
    }
    Ok(out)
}

fn rgba_from_u32(rgb: u32) -> [u8; 4] {
    [(rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8, 255]
}

fn hex_rgb(c: [u8; 4]) -> String {
    format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2])
}

fn hex_rgba(c: [u8; 4]) -> String {
    format!("#{:02X}{:02X}{:02X}{:02X}", c[0], c[1], c[2], c[3])
}

/// `None` keeps the input path; an explicit output must be a sprite document.
fn output_target<'a>(doc: &'a str, output: Option<&'a str>) -> Result<&'a str, LumenError> {
    match output {
        None => Ok(doc),
        Some(o) if o.ends_with(".lumen.json") => Ok(o),
        Some(_) => Err(LumenError::BadParam("output must end in .lumen.json".to_string())),
    }
}

fn layer_range(doc: &SpriteDoc, layer: Option<usize>) -> Result<Range<usize>, LumenError> {
    match layer {
        None => Ok(0..doc.layers.len()),
        Some(idx) if idx < doc.layers.len() => Ok(idx..idx + 1),
        Some(idx) => Err(LumenError::BadParam(format!(
            "layer index {idx} out of range ({} layers)",
            doc.layers.len()
        ))),
    }
}

/// A preset name, or a comma-separated hex list of 2..=256 entries.
fn resolve_palette(spec: &str) -> Result<Vec<[u8; 4]>, LumenError> {
    if spec.len() > PALETTE_SPEC_BYTES_MAX {
        return Err(LumenError::BadParam(format!(
            "palette exceeds {PALETTE_SPEC_BYTES_MAX} bytes"
        )));
    }
    let spec = spec.trim();
    if let Some(preset) = PRESETS.iter().find(|p| p.name == spec) {
        return Ok(preset.rgb.iter().map(|&c| rgba_from_u32(c)).collect());
    }
    if !spec.starts_with('#') {
        let names: Vec<&str> = PRESETS.iter().map(|p| p.name).collect();
        return Err(LumenError::BadParam(format!(
            "unknown palette preset '{spec:.32}'; valid presets: {}",
            names.join(", ")
        )));
    }
    let entries = spec
        .split(',')
        .map(|s| parse_color(s.trim()))
        .collect::<Result<Vec<_>, _>>()?;
    if !(CUSTOM_PALETTE_MIN..=CUSTOM_PALETTE_MAX).contains(&entries.len()) {
        return Err(LumenError::BadParam(format!(
            "custom palette has {} entries; need {CUSTOM_PALETTE_MIN}..={CUSTOM_PALETTE_MAX}",
            entries.len()
        )));
    }
    Ok(entries)
}

fn dist2(a: [u8; 4], b: [u8; 4]) -> u32 {
    (0..3)
        .map(|ch| {
            let d = i32::from(a[ch]) - i32::from(b[ch]);
            (d * d) as u32
        })
        .sum()
}

/// Nearest palette entry by RGB distance; alpha comes from `px`.
fn nearest(entries: &[[u8; 4]], px: [u8; 4]) -> [u8; 4] {
    assert!(!entries.is_empty(), "palette resolution guarantees entries");
    let mut best = entries[0];
    let mut best_dist = u32::MAX;
    for &entry in entries {
        let dist = dist2(entry, px);
        if dist < best_dist {
            best = entry;
            best_dist = dist;
        }
    }
    [best[0], best[1], best[2], px[3]]
}

/// Recolor one image in place. A memo of RGB lookups (capped at
/// `DISTINCT_COLORS_MAX`) keeps the per-pixel palette scan off the hot path
/// for typical few-color sprites.
fn map_image(image: &mut RgbaImage, entries: &[[u8; 4]], dither: bool) {
    let mut memo: HashMap<[u8; 3], [u8; 3]> = HashMap::new();
    for (x, y, px) in image.enumerate_pixels_mut() {
        if px[3] == 0 {
            continue;
        }
        let mut src = px.0;
        if dither {
            let rank = BAYER_4X4[(y % 4) as usize][(x % 4) as usize];
            let offset = (2 * rank + 1 - 16) * DITHER_SPREAD / 32;
            for ch in &mut src[..3] {
                *ch = (i32::from(*ch) + offset).clamp(0, 255) as u8;
            }
        }
        let key = [src[0], src[1], src[2]];
        let rgb = match memo.get(&key) {
            Some(rgb) => *rgb,
            None => {
                let hit = nearest(entries, src);
                let rgb = [hit[0], hit[1], hit[2]];
                if memo.len() < DISTINCT_COLORS_MAX {
                    memo.insert(key, rgb);
                }
                rgb
            }
        };
        px.0 = [rgb[0], rgb[1], rgb[2], px[3]];
    }
}

/// Count non-transparent RGBA colors; fails closed past `DISTINCT_COLORS_MAX`.
fn count_colors<'a>(
    images: impl Iterator<Item = &'a RgbaImage>,
) -> Result<HashMap<[u8; 4], u64>, LumenError> {
    let mut counts: HashMap<[u8; 4], u64> = HashMap::new();
    for image in images {
        for px in image.pixels().filter(|px| px[3] != 0) {
            let distinct = counts.len();
            match counts.entry(px.0) {
                Entry::Occupied(mut e) => *e.get_mut() += 1,
                Entry::Vacant(e) => {
                    if distinct >= DISTINCT_COLORS_MAX {
                        return Err(LumenError::BadParam(format!(
                            "more than {DISTINCT_COLORS_MAX} distinct colors"
                        )));
                    }
                    e.insert(1);
                }
            }
        }
    }
    Ok(counts)
}

type ColorBox = Vec<([u8; 3], u64)>;

/// Median cut. Bounded: each iteration adds exactly one box, so the loop runs
/// at most `boxes_max - 1` times; it stops early once no box can split.
fn median_cut(colors: ColorBox, boxes_max: usize) -> Vec<ColorBox> {
    let mut boxes = vec![colors];
    while boxes.len() < boxes_max {
        let Some((idx, channel)) = widest_box(&boxes) else {
            break;
        };
        let mut lower = boxes.swap_remove(idx);
        lower.sort_unstable_by_key(|(c, _)| (c[channel], *c));
        let total: u64 = lower.iter().map(|(_, n)| n).sum();
        let mut cumulative = 0u64;
        let mut split = lower.len();
        for (i, (_, n)) in lower.iter().enumerate() {
            cumulative += n;
            if cumulative * 2 >= total {
                split = i + 1;
                break;
            }
        }
        // Both halves must be non-empty; widest_box only picks boxes of >= 2.
        let split = split.clamp(1, lower.len() - 1);
        let upper = lower.split_off(split);
        boxes.push(lower);
        boxes.push(upper);
    }
    boxes
}

/// The splittable box (>= 2 distinct colors) with the widest channel range.
fn widest_box(boxes: &[ColorBox]) -> Option<(usize, usize)> {
    let mut best: Option<(usize, usize, u8)> = None;
    for (idx, bx) in boxes.iter().enumerate().filter(|(_, b)| b.len() >= 2) {
        for channel in 0..3 {
            let lo = bx.iter().map(|(c, _)| c[channel]).min().unwrap_or(0);
            let hi = bx.iter().map(|(c, _)| c[channel]).max().unwrap_or(0);
            let range = hi - lo;
            if best.is_none_or(|(_, _, r)| range > r) {
                best = Some((idx, channel, range));
            }
        }
    }
    best.map(|(idx, channel, _)| (idx, channel))
}

/// Count-weighted mean color of a non-empty box, rounded.
fn box_mean(bx: &[([u8; 3], u64)]) -> [u8; 3] {
    let total: u64 = bx.iter().map(|(_, n)| n).sum();
    assert!(total > 0, "boxes hold colors with nonzero counts");
    let mut out = [0u8; 3];
    for (ch, slot) in out.iter_mut().enumerate() {
        let sum: u64 = bx.iter().map(|(c, n)| u64::from(c[ch]) * n).sum();
        *slot = ((sum + total / 2) / total) as u8;
    }
    out
}
