//! Light Show pipeline: fullbody sheet, mature variant, contact sheet.
//!
//! These are the lumen-native counterparts of the Aseprite-driven Light
//! Show scripts: same grid ideas, but driven straight from a `.lumen.json`
//! document through `doc::composite_frame`.
//!
//! Contract summary:
//! - Accepted: project-relative paths with the suffix each tool documents.
//! - Bounds: every sheet is sized with checked arithmetic and rejected
//!   before compositing if either edge exceeds `DOC_MAX_DIMENSION`;
//!   `generate_mature_variant` does exactly one pass over each layer's
//!   pixels (at most `DOC_MAX_LAYERS` x `DOC_MAX_DIMENSION`²).
//! - Failure behavior: parameters are validated before any write; images
//!   and documents are written atomically by `doc`; a sidecar is written
//!   only after its PNG succeeded. Existing outputs are overwritten.

#![forbid(unsafe_code)]

use std::path::Path;

use image::{Rgba, RgbaImage};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::LumenError;
use crate::doc::{DocSaved, ImageSaved, load_doc, save_doc, save_png};
use crate::export::{Grid, render_grid, require_suffix, write_json_sidecar};

/// Smallest accepted mature-variant head scale.
pub const HEAD_SCALE_MIN: f32 = 0.3;
/// Largest accepted mature-variant head scale (identity).
pub const HEAD_SCALE_MAX: f32 = 1.0;
/// Default head scale: the ratio chosen in the Light Show mature rework.
pub const HEAD_SCALE_DEFAULT: f32 = 0.55;
/// Contact-sheet border and gap between cells, in pixels.
pub const CONTACT_PADDING_PX: u32 = 8;

// ---------------------------------------------------------------------------
// Requests and results
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
pub struct BuildFullbodySheetRequest {
    /// Project-relative `.lumen.json` document to render.
    pub doc: String,
    /// Project-relative `.png` sheet output path (required).
    pub output: String,
    /// Project-relative `.json` metadata sidecar path (required).
    pub meta_output: String,
    /// Grid columns, 1..=64. Clamped down to the frame count.
    pub columns: u32,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GenerateMatureVariantRequest {
    /// Project-relative `.lumen.json` source document.
    pub doc: String,
    /// Output `.lumen.json` path. `None` saves over `doc` in place.
    pub output: Option<String>,
    /// Uniform scale about the canvas center, 0.3..=1.0. Defaults to 0.55.
    #[serde(default = "head_scale_default")]
    pub head_scale: f32,
}

fn head_scale_default() -> f32 {
    HEAD_SCALE_DEFAULT
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ContactSheetRequest {
    /// Project-relative `.lumen.json` document to render.
    pub doc: String,
    /// Project-relative `.png` output path (required).
    pub output: String,
    /// Grid columns, 1..=64. Clamped down to the frame count.
    pub columns: u32,
    /// Stamp each cell's 0-based frame index in its top-left corner.
    pub label: bool,
}

/// JSON sidecar written by `build_fullbody_sheet`.
#[derive(Debug, Clone, Serialize)]
pub struct FullbodyMeta {
    /// The sheet PNG path exactly as requested (project-relative).
    pub image: String,
    pub cell_w: u32,
    pub cell_h: u32,
    /// Effective column count (after clamping to the frame count).
    pub columns: u32,
    pub rows: u32,
    pub frames: Vec<FullbodyCell>,
}

/// One frame's placement in a fullbody sheet.
#[derive(Debug, Clone, Serialize)]
pub struct FullbodyCell {
    pub frame: usize,
    /// Every tag whose range covers this frame, in document order.
    pub tags: Vec<String>,
    pub x: u32,
    pub y: u32,
    pub duration_ms: u32,
}

// ---------------------------------------------------------------------------
// Tools
// ---------------------------------------------------------------------------

/// Render every frame, row-major, into a gapless grid PNG plus a JSON
/// sidecar listing each cell's position, duration, and covering tags.
///
/// Rejects: columns outside 1..=64, wrong output suffixes, sheets larger
/// than 4096 px on either edge.
pub async fn build_fullbody_sheet(
    root: &Path,
    req: BuildFullbodySheetRequest,
) -> Result<ImageSaved, LumenError> {
    require_suffix("output", &req.output, ".png")?;
    require_suffix("meta_output", &req.meta_output, ".json")?;
    let doc = load_doc(root, &req.doc)?;
    let frames: Vec<usize> = (0..doc.frames.len()).collect();
    let grid = Grid::plan(frames.len(), doc.width, doc.height, req.columns, 0, 0)?;
    let meta_path = crate::resolve_write_path(&req.meta_output, root, &["json"])?;
    let mut cells = Vec::with_capacity(frames.len());
    for (slot, &frame) in (0u32..).zip(frames.iter()) {
        let (x, y) = grid.origin(slot);
        let tags = doc
            .tags
            .iter()
            .filter(|t| t.from_frame as usize <= frame && frame <= t.to_frame as usize)
            .map(|t| t.name.clone())
            .collect();
        cells.push(FullbodyCell {
            frame,
            tags,
            x,
            y,
            duration_ms: doc.frames[frame].duration_ms,
        });
    }
    let sheet = render_grid(&doc, &grid, &frames)?;
    let saved = save_png(root, &sheet, &req.output)?;
    let meta = FullbodyMeta {
        image: req.output,
        cell_w: grid.cell_w,
        cell_h: grid.cell_h,
        columns: grid.columns,
        rows: grid.rows,
        frames: cells,
    };
    write_json_sidecar(&meta_path, &meta)?;
    Ok(saved)
}

/// Produce a variant document with every layer's pixels uniformly scaled
/// by `head_scale` about the canvas center.
///
/// This is a GEOMETRIC transform only. It does NOT replicate the Aseprite
/// Lua mature pipeline's expression rework (no feature redraw, no
/// proportion changes between body parts): the whole layer shrinks
/// uniformly. Frame layer mods (offsets, opacity, scale) are copied
/// unchanged.
///
/// Resampling is nearest-neighbor on straight (non-premultiplied) alpha:
/// each output pixel copies exactly one source pixel, so no colors or
/// alphas are ever mixed and no premultiplication fringe can appear. This
/// keeps pixel art crisp, matching `composite_frame`'s own scaling.
///
/// Rejects: `head_scale` NaN, infinite, or outside 0.3..=1.0; an output
/// not ending in `.lumen.json`.
pub async fn generate_mature_variant(
    root: &Path,
    req: GenerateMatureVariantRequest,
) -> Result<DocSaved, LumenError> {
    let scale = req.head_scale;
    if !scale.is_finite() || !(HEAD_SCALE_MIN..=HEAD_SCALE_MAX).contains(&scale) {
        return Err(LumenError::BadParam(format!(
            "head_scale {scale} outside {HEAD_SCALE_MIN}..={HEAD_SCALE_MAX}"
        )));
    }
    let out = req.output.as_deref().unwrap_or(&req.doc);
    require_suffix("output", out, ".lumen.json")?;
    let mut doc = load_doc(root, &req.doc)?;
    for layer in &mut doc.layers {
        layer.image = scale_about_center(&layer.image, scale);
    }
    let path = save_doc(root, &doc, out)?;
    Ok(DocSaved::of(&path, &doc))
}

/// Render every frame into a grid with `CONTACT_PADDING_PX` around and
/// between cells. With `label`, each cell gets its 0-based frame index in
/// white 3x5 digits, 1 px in from the cell's top-left corner, clipped to
/// the cell.
///
/// Rejects: columns outside 1..=64, non-`.png` output, sheets larger than
/// 4096 px. Columns above the frame count are clamped, not rejected.
pub async fn contact_sheet(
    root: &Path,
    req: ContactSheetRequest,
) -> Result<ImageSaved, LumenError> {
    require_suffix("output", &req.output, ".png")?;
    let doc = load_doc(root, &req.doc)?;
    let frames: Vec<usize> = (0..doc.frames.len()).collect();
    let grid = Grid::plan(
        frames.len(),
        doc.width,
        doc.height,
        req.columns,
        CONTACT_PADDING_PX,
        CONTACT_PADDING_PX,
    )?;
    let mut sheet = render_grid(&doc, &grid, &frames)?;
    if req.label {
        for (slot, &frame) in (0u32..).zip(frames.iter()) {
            let (x, y) = grid.origin(slot);
            draw_number(&mut sheet, frame, x, y, grid.cell_w, grid.cell_h);
        }
    }
    save_png(root, &sheet, &req.output)
}

// ---------------------------------------------------------------------------
// Private machinery
// ---------------------------------------------------------------------------

/// Nearest-neighbor scale of `src` about its center into a same-size
/// transparent canvas. `scale` is in (0, 1]: every output pixel maps back
/// to at most one source pixel; pixels mapping outside the source stay
/// transparent.
fn scale_about_center(src: &RgbaImage, scale: f32) -> RgbaImage {
    assert!(scale > 0.0 && scale <= 1.0, "scale validated by caller");
    let (w, h) = (src.width(), src.height());
    let (cx, cy) = (f64::from(w) / 2.0, f64::from(h) / 2.0);
    let inv = 1.0 / f64::from(scale);
    let mut out = RgbaImage::from_pixel(w, h, Rgba([0, 0, 0, 0]));
    for y in 0..h {
        let sy = ((f64::from(y) + 0.5 - cy) * inv + cy).floor();
        if sy < 0.0 || sy >= f64::from(h) {
            continue;
        }
        for x in 0..w {
            let sx = ((f64::from(x) + 0.5 - cx) * inv + cx).floor();
            if sx < 0.0 || sx >= f64::from(w) {
                continue;
            }
            // Range-checked above, so the casts are exact.
            out.put_pixel(x, y, *src.get_pixel(sx as u32, sy as u32));
        }
    }
    out
}

/// Private minimal 3x5 digit font, one `u8` row per line, low 3 bits used
/// (bit 2 = left column). Deliberately duplicated here instead of using the
/// `text` module: that module is built in parallel by another worker and
/// this file must not depend on it. Consolidate once both have landed.
const DIGIT_GLYPHS_3X5: [[u8; 5]; 10] = [
    [0b111, 0b101, 0b101, 0b101, 0b111], // 0
    [0b010, 0b110, 0b010, 0b010, 0b111], // 1
    [0b111, 0b001, 0b111, 0b100, 0b111], // 2
    [0b111, 0b001, 0b111, 0b001, 0b111], // 3
    [0b101, 0b101, 0b111, 0b001, 0b001], // 4
    [0b111, 0b100, 0b111, 0b001, 0b111], // 5
    [0b111, 0b100, 0b111, 0b101, 0b111], // 6
    [0b111, 0b001, 0b001, 0b001, 0b001], // 7
    [0b111, 0b101, 0b111, 0b101, 0b111], // 8
    [0b111, 0b101, 0b111, 0b001, 0b111], // 9
];
const GLYPH_W_PX: u32 = 3;
const GLYPH_ADVANCE_PX: u32 = 4;
const LABEL_INSET_PX: u32 = 1;
const LABEL_COLOR: Rgba<u8> = Rgba([255, 255, 255, 255]);

/// Stamp `number` in decimal at the cell's top-left (inset 1 px). Pixels
/// that would fall outside the cell are skipped, so small cells clip the
/// label instead of bleeding into neighbors.
fn draw_number(sheet: &mut RgbaImage, number: usize, cell_x: u32, cell_y: u32, w: u32, h: u32) {
    let text = number.to_string();
    for (i, ch) in (0u32..).zip(text.bytes()) {
        let glyph = &DIGIT_GLYPHS_3X5[usize::from(ch - b'0')];
        for (gy, bits) in (0u32..).zip(glyph.iter()) {
            for gx in 0..GLYPH_W_PX {
                if bits & (0b100 >> gx) == 0 {
                    continue;
                }
                let lx = LABEL_INSET_PX + i * GLYPH_ADVANCE_PX + gx;
                let ly = LABEL_INSET_PX + gy;
                if lx < w && ly < h {
                    sheet.put_pixel(cell_x + lx, cell_y + ly, LABEL_COLOR);
                }
            }
        }
    }
}
