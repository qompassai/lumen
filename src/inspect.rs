//! Sprite/layer/tag/palette inspection and pixel reads.
//!
//! Contract summary:
//! - Every tool is read-only: it loads (and so fully validates) a document and
//!   returns a typed report. Nothing is ever written.
//! - Layer indices are 0-based; out-of-range indices are a `BadParam`.
//! - "Non-zero" / "non-transparent" pixels are pixels with alpha != 0. Fully
//!   transparent pixels are skipped by color counts, histograms, and bboxes.
//! - Bounds: every pixel loop is bounded by the document dimensions (load
//!   enforces them); histograms track at most `DISTINCT_COLORS_MAX` colors and
//!   return 1..=4096 entries.

#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::path::Path;

use image::RgbaImage;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::LumenError;
use crate::doc::{BlendMode, Layer, SpriteDoc, composite_frame, load_doc, resolve_doc_path};

const HISTOGRAM_ENTRIES_MAX: u32 = 4096;
/// Distinct-color budget per histogram (~1M entries, tens of MiB of map memory).
const DISTINCT_COLORS_MAX: usize = 1 << 20;

// ---------------------------------------------------------------------------
// Requests and results
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
pub struct InspectSpriteRequest {
    /// Project-relative `.lumen.json`.
    pub doc: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct InspectLayerRequest {
    /// Project-relative `.lumen.json`.
    pub doc: String,
    /// 0-based layer index.
    pub layer: usize,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct HistogramRequest {
    /// Project-relative `.lumen.json`.
    pub doc: String,
    /// 0-based layer index; `None` histograms the composite of frame 0.
    pub layer: Option<usize>,
    /// Most entries to return, 1..=4096.
    pub max_entries: u32,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ColorUsageRequest {
    /// Project-relative `.lumen.json`.
    pub doc: String,
    /// Exact color to find, `#RRGGBB` (alpha 255) or `#RRGGBBAA`.
    pub color: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SpriteReport {
    pub path: String,
    pub width: u32,
    pub height: u32,
    pub layers: Vec<LayerSummary>,
    pub frames: usize,
    pub tags: Vec<TagSummary>,
    pub palette_entries: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct LayerSummary {
    pub index: usize,
    pub name: String,
    pub visible: bool,
    pub opacity: f32,
    /// `normal` | `multiply` | `screen` | `add`.
    pub blend: String,
    pub nonzero_pixels: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct TagSummary {
    pub name: String,
    pub from_frame: u32,
    pub to_frame: u32,
    pub frame_count: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct LayerReport {
    pub index: usize,
    pub name: String,
    /// Distinct RGBA values among non-transparent pixels.
    pub unique_colors: usize,
    pub nonzero_pixels: u64,
    /// Bounding box of non-transparent pixels; `None` if fully transparent.
    pub bbox: Option<BBox>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct BBox {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

#[derive(Debug, Clone, Serialize)]
pub struct Histogram {
    /// Sorted by count descending, then color ascending.
    pub entries: Vec<CountedColor>,
}

#[derive(Debug, Clone, Serialize)]
pub struct CountedColor {
    /// `#RRGGBBAA`.
    pub color: String,
    pub count: u64,
}

#[derive(Debug, Clone, Serialize)]
pub struct ColorUsage {
    /// The queried color, normalized to `#RRGGBBAA`.
    pub color: String,
    /// Only layers with at least one exact match.
    pub hits: Vec<LayerHit>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LayerHit {
    pub layer: usize,
    pub name: String,
    pub count: u64,
}

// ---------------------------------------------------------------------------
// Tools
// ---------------------------------------------------------------------------

/// Summarize a document: canvas, layers, frames, tags, palette size.
pub async fn inspect_sprite(
    root: &Path,
    req: InspectSpriteRequest,
) -> Result<SpriteReport, LumenError> {
    let path = resolve_doc_path(&req.doc, root, true)?;
    let doc = load_doc(root, &req.doc)?;
    let layers = doc
        .layers
        .iter()
        .enumerate()
        .map(|(index, l)| LayerSummary {
            index,
            name: l.name.clone(),
            visible: l.visible,
            opacity: l.opacity,
            blend: blend_name(l.blend).to_string(),
            nonzero_pixels: nonzero_pixels(&l.image),
        })
        .collect();
    let tags = doc
        .tags
        .iter()
        .map(|t| TagSummary {
            name: t.name.clone(),
            from_frame: t.from_frame,
            to_frame: t.to_frame,
            // load_doc guarantees from <= to; saturate rather than trust it.
            frame_count: t.to_frame.saturating_sub(t.from_frame).saturating_add(1),
        })
        .collect();
    Ok(SpriteReport {
        path: path.display().to_string(),
        width: doc.width,
        height: doc.height,
        layers,
        frames: doc.frames.len(),
        tags,
        palette_entries: doc.palette.len(),
    })
}

/// Report one layer's distinct colors, coverage, and content bounding box.
pub async fn inspect_layer(
    root: &Path,
    req: InspectLayerRequest,
) -> Result<LayerReport, LumenError> {
    let doc = load_doc(root, &req.doc)?;
    let layer = layer_at(&doc, req.layer)?;
    // Sort + dedup of packed pixels: exact, and memory is bounded by the
    // layer's pixel count (<= 4096 * 4096 * 4 bytes).
    let mut packed: Vec<u32> = layer
        .image
        .pixels()
        .filter(|px| px[3] != 0)
        .map(|px| u32::from_be_bytes(px.0))
        .collect();
    let nonzero = packed.len() as u64;
    packed.sort_unstable();
    packed.dedup();
    Ok(LayerReport {
        index: req.layer,
        name: layer.name.clone(),
        unique_colors: packed.len(),
        nonzero_pixels: nonzero,
        bbox: content_bbox(&layer.image),
    })
}

/// Color histogram of one layer, or of the frame-0 composite when `layer` is
/// `None`. Fully transparent pixels are skipped.
pub async fn histogram(root: &Path, req: HistogramRequest) -> Result<Histogram, LumenError> {
    if req.max_entries == 0 || req.max_entries > HISTOGRAM_ENTRIES_MAX {
        return Err(LumenError::BadParam(format!(
            "max_entries {} outside 1..={HISTOGRAM_ENTRIES_MAX}",
            req.max_entries
        )));
    }
    let doc = load_doc(root, &req.doc)?;
    let composite;
    let image = match req.layer {
        Some(idx) => &layer_at(&doc, idx)?.image,
        None => {
            composite = composite_frame(&doc, 0)?;
            &composite
        }
    };
    let mut counts: HashMap<[u8; 4], u64> = HashMap::new();
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
    let mut sorted: Vec<([u8; 4], u64)> = counts.into_iter().collect();
    sorted.sort_unstable_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    sorted.truncate(req.max_entries as usize);
    let entries = sorted
        .into_iter()
        .map(|(c, count)| CountedColor { color: hex_rgba(c), count })
        .collect();
    Ok(Histogram { entries })
}

/// Which layers contain an exact RGBA match for `color`, and how often.
pub async fn color_usage(
    root: &Path,
    req: ColorUsageRequest,
) -> Result<ColorUsage, LumenError> {
    let color = parse_color(&req.color)?;
    let doc = load_doc(root, &req.doc)?;
    let hits = doc
        .layers
        .iter()
        .enumerate()
        .filter_map(|(layer, l)| {
            let count = l.image.pixels().filter(|px| px.0 == color).count() as u64;
            (count > 0).then(|| LayerHit { layer, name: l.name.clone(), count })
        })
        .collect();
    Ok(ColorUsage { color: hex_rgba(color), hits })
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

fn hex_rgba(c: [u8; 4]) -> String {
    format!("#{:02X}{:02X}{:02X}{:02X}", c[0], c[1], c[2], c[3])
}

fn blend_name(mode: BlendMode) -> &'static str {
    match mode {
        BlendMode::Normal => "normal",
        BlendMode::Multiply => "multiply",
        BlendMode::Screen => "screen",
        BlendMode::Add => "add",
    }
}

fn layer_at(doc: &SpriteDoc, idx: usize) -> Result<&Layer, LumenError> {
    doc.layers.get(idx).ok_or_else(|| {
        LumenError::BadParam(format!(
            "layer index {idx} out of range ({} layers)",
            doc.layers.len()
        ))
    })
}

fn nonzero_pixels(image: &RgbaImage) -> u64 {
    image.pixels().filter(|px| px[3] != 0).count() as u64
}

fn content_bbox(image: &RgbaImage) -> Option<BBox> {
    let mut bounds: Option<(u32, u32, u32, u32)> = None;
    for (x, y, px) in image.enumerate_pixels() {
        if px[3] == 0 {
            continue;
        }
        bounds = Some(match bounds {
            None => (x, y, x, y),
            Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
        });
    }
    bounds.map(|(x0, y0, x1, y1)| BBox { x: x0, y: y0, w: x1 - x0 + 1, h: y1 - y0 + 1 })
}
