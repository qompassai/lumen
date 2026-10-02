//! Fidelity presets and art style presets.
//!
//! Two kinds of whole-document look changes, both pixel rewrites of every
//! layer:
//!
//! - **Fidelity presets** (`fidelity_set`) evoke a hardware era's VISUAL
//!   look: a capped palette size (median-cut quantization) plus pixel
//!   chunkiness (block snapping). They do NOT change the file's bit depth —
//!   layers stay 8-bit-per-channel RGBA throughout, and the saved PNGs are
//!   RGBA. "8bit" names a look, not a file format.
//! - **Style presets** (`style_list`, `style_apply`) are per-pixel HSL color
//!   grades blended toward the original by a strength factor.
//!
//! Contract summary:
//! - Accepted: an existing `.lumen.json` document inside the project root;
//!   an optional `.lumen.json` output path (`None` = save in place).
//! - Rejected: unknown preset/style names (the error lists valid values),
//!   non-finite or out-of-range strength, bad paths — all `LumenError`,
//!   validated before any pixel is touched.
//! - Bounds: every pixel loop is bounded by the document dimensions
//!   (<= 4096x4096) and layer count (<= 64); median-cut performs at most
//!   `max_colors` (<= 4096) splits per layer.
//! - Alpha: style grading never modifies alpha; quantization never modifies
//!   alpha; only fidelity block snapping resamples alpha spatially.

#![forbid(unsafe_code)]

use std::path::Path;

use image::{Rgba, RgbaImage};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::LumenError;
use crate::doc::{DocSaved, SpriteDoc, load_doc, save_doc};

// ---------------------------------------------------------------------------
// Public contract
// ---------------------------------------------------------------------------

/// Request for `fidelity_set`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct FidelitySetReq {
    /// Project-relative input `.lumen.json` document.
    pub doc: String,
    /// Project-relative output `.lumen.json`; omit to overwrite the input.
    pub output: Option<String>,
    /// One of: 8bit, 16bit, 32bit, 64bit, studio, max. A visual-era look
    /// (palette size + pixel chunkiness); the file stays RGBA.
    pub preset: String,
}

/// Request for `style_list` (takes no parameters).
#[derive(Debug, Deserialize, JsonSchema)]
pub struct StyleListReq {}

/// Request for `style_apply`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct StyleApplyReq {
    /// Project-relative input `.lumen.json` document.
    pub doc: String,
    /// Project-relative output `.lumen.json`; omit to overwrite the input.
    pub output: Option<String>,
    /// Style preset name (see `style_list`).
    pub style: String,
    /// 0.0 (no change) ..= 1.0 (full grade).
    pub strength: f32,
}

/// What `style_list` reports.
#[derive(Debug, Clone, Serialize)]
pub struct StyleList {
    pub styles: Vec<StyleInfo>,
}

/// One style preset's name and one-line description.
#[derive(Debug, Clone, Serialize)]
pub struct StyleInfo {
    pub name: String,
    pub description: String,
}

/// Apply a fidelity preset to every layer of a document.
///
/// Fidelity presets evoke a hardware era's VISUAL look — palette size plus
/// pixel chunkiness. They do NOT change the file's bit depth: layers stay
/// RGBA, 8 bits per channel, before and after.
///
/// | preset | max_colors | pixel_block |
/// |--------|-----------:|------------:|
/// | 8bit   | 32         | 4           |
/// | 16bit  | 256        | 2           |
/// | 32bit  | 4096       | 1           |
/// | 64bit  | unlimited  | 1           |
/// | studio | unlimited  | 1 (identity; documents intent) |
/// | max    | unlimited  | 1 (identity; documents intent) |
///
/// Per layer: when `pixel_block > 1`, every pixel takes the value of its
/// block's top-left pixel (identical to a nearest downscale + nearest
/// upscale when the dimensions divide evenly; well-defined at ragged
/// edges). When `max_colors` is set, the RGB of non-transparent pixels is
/// median-cut quantized to at most that many colors; alpha is untouched.
///
/// Errors: unknown preset → `BadParam` listing all six; bad paths →
/// `PathRejected`; save failures leave the destination untouched.
pub async fn fidelity_set(root: &Path, req: FidelitySetReq) -> Result<DocSaved, LumenError> {
    let preset = find_fidelity(&req.preset)?;
    let target = output_target(&req.doc, req.output.as_deref())?;
    let mut doc = load_doc(root, &req.doc)?;
    for layer in &mut doc.layers {
        if preset.pixel_block > 1 {
            layer.image = block_snap(&layer.image, preset.pixel_block);
        }
        if let Some(max_colors) = preset.max_colors {
            quantize_median_cut(&mut layer.image, max_colors);
        }
    }
    save_to(root, target, &doc)
}

/// List the 13 style presets. `ref_01`..`ref_09` are placeholders awaiting
/// Matt's reference-image naming (each is currently a distinct hue family).
pub async fn style_list(_root: &Path, _req: StyleListReq) -> Result<StyleList, LumenError> {
    let styles = STYLE_PRESETS
        .iter()
        .map(|p| StyleInfo {
            name: p.name.to_string(),
            description: p.description.to_string(),
        })
        .collect();
    Ok(StyleList { styles })
}

/// Grade every layer of a document with a style preset.
///
/// Per non-transparent pixel: RGB → HSL; hue += `hue_shift_deg` (wrapped);
/// saturation *= `sat_mult`; lightness *= `light_mult`, then contrast about
/// 0.5, then optional posterize to N lightness levels; HSL → RGB; finally
/// the result is blended toward the original by `strength`
/// (`out = orig + (graded - orig) * strength`). Alpha is NEVER modified.
/// `strength == 0.0` is a successful no-op (pixels identical; doc saved).
///
/// Errors: unknown style → `BadParam` listing all 13; strength NaN,
/// infinite, or outside 0.0..=1.0 → `BadParam`.
pub async fn style_apply(root: &Path, req: StyleApplyReq) -> Result<DocSaved, LumenError> {
    let preset = find_style(&req.style)?;
    if !req.strength.is_finite() || !(0.0..=1.0).contains(&req.strength) {
        return Err(LumenError::BadParam(format!(
            "strength {} is outside 0.0..=1.0",
            req.strength
        )));
    }
    let target = output_target(&req.doc, req.output.as_deref())?;
    let mut doc = load_doc(root, &req.doc)?;
    if req.strength > 0.0 {
        for layer in &mut doc.layers {
            for pixel in layer.image.pixels_mut() {
                if pixel[3] != 0 {
                    *pixel = grade_pixel(*pixel, preset, req.strength);
                }
            }
        }
    }
    save_to(root, target, &doc)
}

// ---------------------------------------------------------------------------
// Preset tables
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
struct FidelityPreset {
    name: &'static str,
    max_colors: Option<usize>,
    pixel_block: u32,
}

const FIDELITY_PRESETS: [FidelityPreset; 6] = [
    FidelityPreset { name: "8bit", max_colors: Some(32), pixel_block: 4 },
    FidelityPreset { name: "16bit", max_colors: Some(256), pixel_block: 2 },
    FidelityPreset { name: "32bit", max_colors: Some(4096), pixel_block: 1 },
    FidelityPreset { name: "64bit", max_colors: None, pixel_block: 1 },
    FidelityPreset { name: "studio", max_colors: None, pixel_block: 1 },
    FidelityPreset { name: "max", max_colors: None, pixel_block: 1 },
];

#[derive(Debug, Clone, Copy)]
struct StylePreset {
    name: &'static str,
    description: &'static str,
    hue_shift_deg: f32,
    sat_mult: f32,
    light_mult: f32,
    contrast: f32,
    /// Lightness levels, 2..=16 when set.
    posterize: Option<u8>,
}

const REF_DESCRIPTION: &str =
    "Placeholder — awaiting Matt's reference-image naming; currently a distinct hue family.";

const fn ref_preset(name: &'static str, index: u8) -> StylePreset {
    StylePreset {
        name,
        description: REF_DESCRIPTION,
        hue_shift_deg: (index - 1) as f32 * 40.0,
        sat_mult: 1.1,
        light_mult: 1.0,
        contrast: 1.0,
        posterize: None,
    }
}

const STYLE_PRESETS: [StylePreset; 13] = [
    ref_preset("ref_01", 1),
    ref_preset("ref_02", 2),
    ref_preset("ref_03", 3),
    ref_preset("ref_04", 4),
    ref_preset("ref_05", 5),
    ref_preset("ref_06", 6),
    ref_preset("ref_07", 7),
    ref_preset("ref_08", 8),
    ref_preset("ref_09", 9),
    StylePreset {
        name: "dark_fantasy",
        description: "Muted, shadowed, high-contrast grade with a cold crimson lean.",
        hue_shift_deg: -10.0,
        sat_mult: 0.7,
        light_mult: 0.75,
        contrast: 1.2,
        posterize: None,
    },
    StylePreset {
        name: "solarpunk",
        description: "Bright, lush, sun-warmed grade with lifted light and rich greens.",
        hue_shift_deg: 15.0,
        sat_mult: 1.3,
        light_mult: 1.1,
        contrast: 1.0,
        posterize: None,
    },
    StylePreset {
        name: "synthwave",
        description: "Punchy neon grade: heavy saturation, boosted contrast, hot hue shift.",
        hue_shift_deg: 20.0,
        sat_mult: 1.5,
        light_mult: 1.0,
        contrast: 1.15,
        posterize: None,
    },
    StylePreset {
        name: "ukiyo_e",
        description: "Softened color in flat posterized tones evoking woodblock prints.",
        hue_shift_deg: 0.0,
        sat_mult: 0.85,
        light_mult: 1.05,
        contrast: 0.95,
        posterize: Some(6),
    },
];

fn find_fidelity(name: &str) -> Result<FidelityPreset, LumenError> {
    FIDELITY_PRESETS.iter().find(|p| p.name == name).copied().ok_or_else(|| {
        let valid: Vec<&str> = FIDELITY_PRESETS.iter().map(|p| p.name).collect();
        LumenError::BadParam(format!(
            "unknown fidelity preset {name:?}; valid: {}",
            valid.join(", ")
        ))
    })
}

fn find_style(name: &str) -> Result<StylePreset, LumenError> {
    STYLE_PRESETS.iter().find(|p| p.name == name).copied().ok_or_else(|| {
        let valid: Vec<&str> = STYLE_PRESETS.iter().map(|p| p.name).collect();
        LumenError::BadParam(format!("unknown style {name:?}; valid: {}", valid.join(", ")))
    })
}

// ---------------------------------------------------------------------------
// Save plumbing (duplicated per module by design; a later pass dedups)
// ---------------------------------------------------------------------------

/// Pick the save target before any work: the input itself, or an output
/// that must end in `.lumen.json`.
fn output_target<'a>(input: &'a str, output: Option<&'a str>) -> Result<&'a str, LumenError> {
    match output {
        None => Ok(input),
        Some(o) if o.ends_with(".lumen.json") => Ok(o),
        Some(_) => Err(LumenError::BadParam("output must end in .lumen.json".to_string())),
    }
}

fn save_to(root: &Path, target: &str, doc: &SpriteDoc) -> Result<DocSaved, LumenError> {
    let path = save_doc(root, doc, target)?;
    Ok(DocSaved::of(&path, doc))
}

// ---------------------------------------------------------------------------
// Fidelity machinery
// ---------------------------------------------------------------------------

/// Every pixel takes the value of the top-left pixel of its `block`x`block`
/// cell. Bounded by the image dimensions.
fn block_snap(src: &RgbaImage, block: u32) -> RgbaImage {
    assert!(block >= 1, "pixel_block comes from the preset table");
    RgbaImage::from_fn(src.width(), src.height(), |x, y| {
        *src.get_pixel(x - x % block, y - y % block)
    })
}

fn pack_rgb(p: &Rgba<u8>) -> u32 {
    (u32::from(p[0]) << 16) | (u32::from(p[1]) << 8) | u32::from(p[2])
}

/// Channel 0 = R, 1 = G, 2 = B.
fn channel(packed: u32, ch: usize) -> u8 {
    ((packed >> (16 - 8 * ch)) & 0xFF) as u8
}

/// Distinct RGB colors of non-transparent pixels with their counts, sorted
/// by packed value. At most width*height entries (<= 16.7M).
fn color_histogram(image: &RgbaImage) -> Vec<(u32, u32)> {
    let mut packed: Vec<u32> = image.pixels().filter(|p| p[3] != 0).map(pack_rgb).collect();
    packed.sort_unstable();
    let mut hist: Vec<(u32, u32)> = Vec::new();
    for color in packed {
        match hist.last_mut() {
            Some((last, count)) if *last == color => *count += 1,
            _ => hist.push((color, 1)),
        }
    }
    hist
}

/// A median-cut box: a range of the histogram plus its widest channel.
#[derive(Debug, Clone, Copy)]
struct ColorBox {
    start: usize,
    end: usize,
    widest_channel: usize,
    extent: u8,
}

fn make_box(hist: &[(u32, u32)], start: usize, end: usize) -> ColorBox {
    let mut lo = [u8::MAX; 3];
    let mut hi = [0u8; 3];
    for &(color, _) in &hist[start..end] {
        for ch in 0..3 {
            lo[ch] = lo[ch].min(channel(color, ch));
            hi[ch] = hi[ch].max(channel(color, ch));
        }
    }
    let mut widest_channel = 0;
    for ch in 1..3 {
        if hi[ch] - lo[ch] > hi[widest_channel] - lo[widest_channel] {
            widest_channel = ch;
        }
    }
    let extent = hi[widest_channel] - lo[widest_channel];
    ColorBox { start, end, widest_channel, extent }
}

/// Split a box at the count-weighted median of its widest channel.
fn split_box(hist: &mut [(u32, u32)], b: ColorBox) -> (ColorBox, ColorBox) {
    assert!(b.end - b.start >= 2, "only boxes with >= 2 colors are split");
    let slice = &mut hist[b.start..b.end];
    // Key is unique per color (packed tiebreak), so the order is deterministic.
    slice.sort_unstable_by_key(|&(c, _)| (channel(c, b.widest_channel), c));
    let total: u64 = slice.iter().map(|&(_, n)| u64::from(n)).sum();
    let mut cumulative = 0u64;
    let mut cut = 1;
    for (i, &(_, n)) in slice.iter().enumerate() {
        cumulative += u64::from(n);
        if cumulative * 2 >= total {
            cut = i + 1;
            break;
        }
    }
    let cut = cut.clamp(1, slice.len() - 1);
    let mid = b.start + cut;
    (make_box(hist, b.start, mid), make_box(hist, mid, b.end))
}

/// Median-cut quantize the RGB of non-transparent pixels to at most
/// `max_colors` colors. Alpha is untouched. Performs at most `max_colors`
/// splits; a layer already within budget is left unchanged.
fn quantize_median_cut(image: &mut RgbaImage, max_colors: usize) {
    assert!(max_colors >= 1, "max_colors comes from the preset table");
    let mut hist = color_histogram(image);
    if hist.len() <= max_colors {
        return;
    }
    let mut boxes = vec![make_box(&hist, 0, hist.len())];
    while boxes.len() < max_colors {
        let pick = boxes
            .iter()
            .enumerate()
            .filter(|(_, b)| b.end - b.start >= 2 && b.extent > 0)
            .max_by_key(|&(i, b)| (b.extent, std::cmp::Reverse(i)))
            .map(|(i, _)| i);
        let Some(i) = pick else { break };
        let (left, right) = split_box(&mut hist, boxes[i]);
        boxes[i] = left;
        boxes.push(right);
    }
    let mut lookup: Vec<(u32, [u8; 3])> = Vec::with_capacity(hist.len());
    for b in &boxes {
        let mean = box_mean(&hist[b.start..b.end]);
        lookup.extend(hist[b.start..b.end].iter().map(|&(c, _)| (c, mean)));
    }
    lookup.sort_unstable_by_key(|&(c, _)| c);
    for pixel in image.pixels_mut() {
        if pixel[3] == 0 {
            continue;
        }
        let found = lookup.binary_search_by_key(&pack_rgb(pixel), |&(c, _)| c);
        debug_assert!(found.is_ok(), "every opaque color is in the histogram");
        if let Ok(i) = found {
            let [r, g, b] = lookup[i].1;
            *pixel = Rgba([r, g, b, pixel[3]]);
        }
    }
}

/// Count-weighted mean color of a non-empty histogram slice.
fn box_mean(colors: &[(u32, u32)]) -> [u8; 3] {
    let mut sums = [0u64; 3];
    let mut total = 0u64;
    for &(color, n) in colors {
        for (ch, sum) in sums.iter_mut().enumerate() {
            *sum += u64::from(channel(color, ch)) * u64::from(n);
        }
        total += u64::from(n);
    }
    assert!(total > 0, "boxes are never empty");
    sums.map(|s| ((s + total / 2) / total) as u8)
}

// ---------------------------------------------------------------------------
// Style machinery
// ---------------------------------------------------------------------------

fn grade_pixel(p: Rgba<u8>, preset: StylePreset, strength: f32) -> Rgba<u8> {
    let (r, g, b) = (p[0] as f32 / 255.0, p[1] as f32 / 255.0, p[2] as f32 / 255.0);
    let (h, s, l) = rgb_to_hsl(r, g, b);
    let h = (h + preset.hue_shift_deg).rem_euclid(360.0);
    let s = (s * preset.sat_mult).clamp(0.0, 1.0);
    let l = (l * preset.light_mult).clamp(0.0, 1.0);
    let mut l = ((l - 0.5) * preset.contrast + 0.5).clamp(0.0, 1.0);
    if let Some(levels) = preset.posterize {
        assert!((2..=16).contains(&levels), "posterize levels come from the table");
        let steps = f32::from(levels - 1);
        l = (l * steps).round() / steps;
    }
    let (gr, gg, gb) = hsl_to_rgb(h, s, l);
    let blend = |orig: u8, graded: f32| {
        let o = orig as f32;
        (o + (graded * 255.0 - o) * strength).round().clamp(0.0, 255.0) as u8
    };
    Rgba([blend(p[0], gr), blend(p[1], gg), blend(p[2], gb), p[3]])
}

/// RGB in 0..=1 → (hue degrees 0..360, saturation 0..=1, lightness 0..=1).
fn rgb_to_hsl(r: f32, g: f32, b: f32) -> (f32, f32, f32) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let l = (max + min) / 2.0;
    let delta = max - min;
    if delta <= f32::EPSILON {
        return (0.0, 0.0, l);
    }
    let s = delta / (1.0 - (2.0 * l - 1.0).abs());
    let h = if max == r {
        60.0 * ((g - b) / delta).rem_euclid(6.0)
    } else if max == g {
        60.0 * ((b - r) / delta + 2.0)
    } else {
        60.0 * ((r - g) / delta + 4.0)
    };
    (h, s.clamp(0.0, 1.0), l)
}

/// (hue degrees, saturation, lightness) → RGB in 0..=1.
fn hsl_to_rgb(h: f32, s: f32, l: f32) -> (f32, f32, f32) {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let hp = h.rem_euclid(360.0) / 60.0;
    let x = c * (1.0 - (hp.rem_euclid(2.0) - 1.0).abs());
    let (r1, g1, b1) = match hp as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = l - c / 2.0;
    (r1 + m, g1 + m, b1 + m)
}
