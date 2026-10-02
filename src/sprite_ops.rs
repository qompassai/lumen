//! Canvas, layer, frame, tag, drawing, transform, and tween operations.
//!
//! Every tool here is stateless: it loads one `.lumen.json` document, applies
//! one validated edit in memory, and saves the result in place (`output:
//! None`) or to `output`. Contract shared by every tool:
//! - Accepted: project-relative `.lumen.json` paths; colors `#RRGGBB` or
//!   `#RRGGBBAA`; indices inside the live document; finite floats in range.
//! - Rejected: anything else, as `LumenError::BadParam` (parameters) or
//!   `LumenError::DocInvalid` (structure). Enum errors list the valid values.
//!   Error text never echoes caller strings, so error size stays bounded.
//! - Ordering: validate → mutate in memory → save. A rejected request never
//!   touches disk; `save_doc` writes atomically (temp file + rename).
//! - Bounds: drawing clips to the canvas and its work is bounded by the
//!   canvas pixel count (or `LINE_COORD_MAX_ABS` for lines); layer, frame,
//!   name, offset, and scale limits come from the `DOC_MAX_*` constants.
//! - Drawing replaces pixels (no alpha blending): the color written is the
//!   color stored.

#![forbid(unsafe_code)]

use std::collections::VecDeque;
use std::ops::Range;
use std::path::Path;

use image::{ImageBuffer, Rgba, RgbaImage, imageops};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::LumenError;
use crate::doc::{
    BlendMode, DOC_MAX_DIMENSION, DOC_MAX_FRAMES, DOC_MAX_LAYERS, DOC_MAX_NAME_CHARS,
    DOC_MAX_OFFSET_PX, DocSaved, Frame, Layer, LayerMod, SpriteDoc, Tag, check_dims,
    identity_mod, load_doc, new_doc, save_doc,
};

/// Frame duration bounds, in milliseconds.
const FRAME_DURATION_MS_MIN: u32 = 1;
const FRAME_DURATION_MS_MAX: u32 = 60_000;
/// Duration of the single frame `new_sprite` creates, in milliseconds.
const NEW_SPRITE_FRAME_MS: u32 = 100;
/// Name of the single layer `new_sprite` creates.
const NEW_SPRITE_LAYER_NAME: &str = "background";
/// Per-frame layer scale bounds (mirrors `doc.rs` validation).
const SCALE_MIN: f32 = 0.0625;
const SCALE_MAX: f32 = 16.0;
/// Most frames one `tween_frames` call may insert.
const TWEEN_STEPS_MAX: u32 = 256;
/// Largest |coordinate| `draw_line` accepts. Bresenham work is
/// `max(|dx|, |dy|) + 1` steps, so this caps one line at ~65k steps while
/// still allowing endpoints far off-canvas.
const LINE_COORD_MAX_ABS: i32 = 8 * DOC_MAX_DIMENSION as i32;

// ---------------------------------------------------------------------------
// Requests
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
pub struct NewSpriteRequest {
    /// Canvas width in pixels, 1..=4096.
    pub width: u32,
    /// Canvas height in pixels, 1..=4096.
    pub height: u32,
    /// Background fill, "#RRGGBB" or "#RRGGBBAA".
    pub background: String,
    /// Project-relative destination ending in .lumen.json (overwritten if present).
    pub output: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AddLayerRequest {
    /// Project-relative input .lumen.json.
    pub doc: String,
    /// Destination .lumen.json; omitted means save in place.
    pub output: Option<String>,
    /// Layer name (truncated to 128 chars; " (2)", " (3)"… appended on collision).
    pub name: String,
    /// Stack index to insert at (0 = bottom); omitted appends on top.
    pub index: Option<usize>,
    /// normal | multiply | screen | add (default normal).
    pub blend: Option<String>,
    /// 0.0..=1.0 (default 1.0).
    pub opacity: Option<f32>,
    /// Optional fill color; omitted means fully transparent.
    pub fill: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DeleteLayerRequest {
    pub doc: String,
    pub output: Option<String>,
    pub index: usize,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ReorderLayerRequest {
    pub doc: String,
    pub output: Option<String>,
    /// Current stack index of the layer.
    pub from: usize,
    /// Final stack index of the layer.
    pub to: usize,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RenameLayerRequest {
    pub doc: String,
    pub output: Option<String>,
    pub index: usize,
    pub name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SetLayerPropsRequest {
    pub doc: String,
    pub output: Option<String>,
    pub index: usize,
    pub visible: Option<bool>,
    pub opacity: Option<f32>,
    pub blend: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AddFrameRequest {
    pub doc: String,
    pub output: Option<String>,
    /// 1..=60000 ms.
    pub duration_ms: u32,
    /// Frame index to insert at; omitted appends.
    pub at: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DeleteFrameRequest {
    pub doc: String,
    pub output: Option<String>,
    pub index: usize,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SetFrameDurationRequest {
    pub doc: String,
    pub output: Option<String>,
    pub index: usize,
    /// 1..=60000 ms.
    pub duration_ms: u32,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SetFrameModRequest {
    pub doc: String,
    pub output: Option<String>,
    pub frame: usize,
    pub layer: usize,
    /// -8192..=8192 px.
    pub offset_x: Option<i32>,
    /// -8192..=8192 px.
    pub offset_y: Option<i32>,
    /// 0.0..=1.0.
    pub opacity_mult: Option<f32>,
    /// 0.0625..=16.0.
    pub scale: Option<f32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AddTagRequest {
    pub doc: String,
    pub output: Option<String>,
    /// Unique tag name, 1..=128 chars.
    pub name: String,
    /// First frame of the inclusive range.
    pub from_frame: u32,
    /// Last frame of the inclusive range.
    pub to_frame: u32,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DeleteTagRequest {
    pub doc: String,
    pub output: Option<String>,
    pub name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SetPixelRequest {
    pub doc: String,
    pub output: Option<String>,
    pub layer: usize,
    pub x: i32,
    pub y: i32,
    pub color: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FillRectRequest {
    pub doc: String,
    pub output: Option<String>,
    pub layer: usize,
    pub x: i32,
    pub y: i32,
    /// Width in pixels, at least 1.
    pub w: u32,
    /// Height in pixels, at least 1.
    pub h: u32,
    pub color: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DrawLineRequest {
    pub doc: String,
    pub output: Option<String>,
    pub layer: usize,
    /// Endpoints accept -32768..=32768; off-canvas pixels are clipped.
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
    pub color: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DrawCircleRequest {
    pub doc: String,
    pub output: Option<String>,
    pub layer: usize,
    pub cx: i32,
    pub cy: i32,
    /// Radius in pixels; 0 draws a single pixel.
    pub r: u32,
    pub color: String,
    /// true = disk, false = 1px outline.
    pub filled: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FloodFillRequest {
    pub doc: String,
    pub output: Option<String>,
    pub layer: usize,
    /// Seed pixel; must lie on the canvas.
    pub x: i32,
    pub y: i32,
    pub color: String,
    /// Max per-channel (RGBA) absolute difference from the seed pixel.
    pub tolerance: u8,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FlipRequest {
    pub doc: String,
    pub output: Option<String>,
    /// Layer index; omitted flips every layer.
    pub layer: Option<usize>,
    /// true = mirror left/right, false = mirror top/bottom.
    pub horizontal: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RotateRequest {
    pub doc: String,
    pub output: Option<String>,
    /// Layer index; omitted rotates every layer.
    pub layer: Option<usize>,
    /// Clockwise degrees: 90, 180, or 270.
    pub degrees: u16,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ResizeCanvasRequest {
    pub doc: String,
    pub output: Option<String>,
    pub width: u32,
    pub height: u32,
    /// center | top-left | top-right | bottom-left | bottom-right.
    pub anchor: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CropRequest {
    pub doc: String,
    pub output: Option<String>,
    pub x: u32,
    pub y: u32,
    /// At least 1; x + w must not exceed the canvas width.
    pub w: u32,
    /// At least 1; y + h must not exceed the canvas height.
    pub h: u32,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TweenFramesRequest {
    pub doc: String,
    pub output: Option<String>,
    pub from_frame: usize,
    pub to_frame: usize,
    /// Frames to insert after from_frame, 1..=256.
    pub steps: u32,
    /// linear | ease_in | ease_out | ease_in_out.
    pub easing: String,
}

// ---------------------------------------------------------------------------
// Structure tools
// ---------------------------------------------------------------------------

/// Create a document: one layer named "background" filled with
/// `background`, one 100ms frame. `output` is required and overwritten if
/// it exists. Fails with `BadParam` on bad dims, color, or output suffix.
pub async fn new_sprite(root: &Path, req: NewSpriteRequest) -> Result<DocSaved, LumenError> {
    check_doc_suffix(&req.output)?;
    let color = parse_color(&req.background)?;
    let mut doc = new_doc(req.width, req.height)?;
    let layer = doc.layers.first_mut().ok_or_else(|| {
        LumenError::DocInvalid("new document has no layer".to_string())
    })?;
    layer.name = NEW_SPRITE_LAYER_NAME.to_string();
    layer.image = ImageBuffer::from_pixel(req.width, req.height, Rgba(color));
    let frame = doc.frames.first_mut().ok_or_else(|| {
        LumenError::DocInvalid("new document has no frame".to_string())
    })?;
    frame.duration_ms = NEW_SPRITE_FRAME_MS;
    commit(root, &req.output, &doc)
}

/// Insert a layer at `index` (default: top). Every frame gets an identity
/// mod at the same index so mods stay aligned with the stack. Refuses past
/// `DOC_MAX_LAYERS` or `index > len`.
pub async fn add_layer(root: &Path, req: AddLayerRequest) -> Result<DocSaved, LumenError> {
    let target = output_target(&req.doc, req.output.as_deref())?;
    let blend = match req.blend.as_deref() {
        Some(s) => parse_blend(s)?,
        None => BlendMode::Normal,
    };
    let opacity = req.opacity.unwrap_or(1.0);
    check_f32_range(opacity, 0.0, 1.0, "opacity")?;
    let fill = match req.fill.as_deref() {
        Some(s) => parse_color(s)?,
        None => [0, 0, 0, 0],
    };
    let base_name = clean_name(&req.name, "layer")?;
    let mut doc = load_doc(root, &req.doc)?;
    if doc.layers.len() >= DOC_MAX_LAYERS {
        return Err(bad(format!("document already has {DOC_MAX_LAYERS} layers (the maximum)")));
    }
    let index = req.index.unwrap_or(doc.layers.len());
    if index > doc.layers.len() {
        return Err(bad(format!(
            "layer index {index} out of range (0..={})",
            doc.layers.len()
        )));
    }
    let name = unique_layer_name(&doc, &base_name, None)?;
    let layer = Layer {
        name,
        visible: true,
        opacity,
        blend,
        image: ImageBuffer::from_pixel(doc.width, doc.height, Rgba(fill)),
    };
    doc.layers.insert(index, layer);
    for frame in &mut doc.frames {
        frame.layer_mods.insert(index, identity_mod());
    }
    commit(root, target, &doc)
}

/// Remove the layer at `index` and its mod in every frame. Refuses to
/// remove the last remaining layer.
pub async fn delete_layer(root: &Path, req: DeleteLayerRequest) -> Result<DocSaved, LumenError> {
    let target = output_target(&req.doc, req.output.as_deref())?;
    let mut doc = load_doc(root, &req.doc)?;
    check_layer_index(&doc, req.index)?;
    if doc.layers.len() == 1 {
        return Err(bad("cannot delete the only layer"));
    }
    doc.layers.remove(req.index);
    for frame in &mut doc.frames {
        frame.layer_mods.remove(req.index);
    }
    commit(root, target, &doc)
}

/// Move the layer at `from` so it ends up at index `to`, carrying its
/// per-frame mods along. `from == to` is a successful no-op.
pub async fn reorder_layer(root: &Path, req: ReorderLayerRequest) -> Result<DocSaved, LumenError> {
    let target = output_target(&req.doc, req.output.as_deref())?;
    let mut doc = load_doc(root, &req.doc)?;
    check_layer_index(&doc, req.from)?;
    check_layer_index(&doc, req.to)?;
    if req.from != req.to {
        let layer = doc.layers.remove(req.from);
        doc.layers.insert(req.to, layer);
        for frame in &mut doc.frames {
            let layer_mod = frame.layer_mods.remove(req.from);
            frame.layer_mods.insert(req.to, layer_mod);
        }
    }
    commit(root, target, &doc)
}

/// Rename a layer. Same name rules as `add_layer`; renaming a layer to its
/// own current name is not a collision.
pub async fn rename_layer(root: &Path, req: RenameLayerRequest) -> Result<DocSaved, LumenError> {
    let target = output_target(&req.doc, req.output.as_deref())?;
    let base_name = clean_name(&req.name, "layer")?;
    let mut doc = load_doc(root, &req.doc)?;
    check_layer_index(&doc, req.index)?;
    let name = unique_layer_name(&doc, &base_name, Some(req.index))?;
    doc.layers[req.index].name = name;
    commit(root, target, &doc)
}

/// Set any of visible / opacity / blend on one layer. At least one must be
/// given.
pub async fn set_layer_props(
    root: &Path,
    req: SetLayerPropsRequest,
) -> Result<DocSaved, LumenError> {
    let target = output_target(&req.doc, req.output.as_deref())?;
    if req.visible.is_none() && req.opacity.is_none() && req.blend.is_none() {
        return Err(bad("give at least one of: visible, opacity, blend"));
    }
    if let Some(opacity) = req.opacity {
        check_f32_range(opacity, 0.0, 1.0, "opacity")?;
    }
    let blend = req.blend.as_deref().map(parse_blend).transpose()?;
    let mut doc = load_doc(root, &req.doc)?;
    check_layer_index(&doc, req.index)?;
    let layer = &mut doc.layers[req.index];
    if let Some(visible) = req.visible {
        layer.visible = visible;
    }
    if let Some(opacity) = req.opacity {
        layer.opacity = opacity;
    }
    if let Some(blend) = blend {
        layer.blend = blend;
    }
    commit(root, target, &doc)
}

/// Insert a frame with identity mods for every layer at `at` (default:
/// end). Tags at or after `at` shift right; a tag spanning `at` grows.
pub async fn add_frame(root: &Path, req: AddFrameRequest) -> Result<DocSaved, LumenError> {
    let target = output_target(&req.doc, req.output.as_deref())?;
    check_duration(req.duration_ms)?;
    let mut doc = load_doc(root, &req.doc)?;
    if doc.frames.len() >= DOC_MAX_FRAMES {
        return Err(bad(format!("document already has {DOC_MAX_FRAMES} frames (the maximum)")));
    }
    let at = req.at.unwrap_or(doc.frames.len());
    if at > doc.frames.len() {
        return Err(bad(format!("frame index {at} out of range (0..={})", doc.frames.len())));
    }
    let frame = Frame {
        duration_ms: req.duration_ms,
        layer_mods: vec![identity_mod(); doc.layers.len()],
    };
    doc.frames.insert(at, frame);
    shift_tags_for_insert(&mut doc.tags, at, 1);
    commit(root, target, &doc)
}

/// Remove one frame. Refuses to remove the last frame, and refuses to remove
/// the only frame of a single-frame tag (delete the tag first) rather than
/// silently dropping the tag. Other tags shift/shrink to stay aligned.
pub async fn delete_frame(root: &Path, req: DeleteFrameRequest) -> Result<DocSaved, LumenError> {
    let target = output_target(&req.doc, req.output.as_deref())?;
    let mut doc = load_doc(root, &req.doc)?;
    check_frame_index(&doc, req.index)?;
    if doc.frames.len() == 1 {
        return Err(bad("cannot delete the only frame"));
    }
    let index = frame_u32(req.index);
    if doc.tags.iter().any(|t| t.from_frame == index && t.to_frame == index) {
        return Err(bad(format!(
            "frame {index} is the only frame of a tag; delete that tag first"
        )));
    }
    doc.frames.remove(req.index);
    for tag in &mut doc.tags {
        if tag.from_frame > index {
            tag.from_frame -= 1;
            tag.to_frame -= 1;
        } else if tag.to_frame >= index {
            // from <= index <= to and from < to (single-frame case refused).
            tag.to_frame -= 1;
        }
    }
    commit(root, target, &doc)
}

/// Set one frame's duration (1..=60000 ms).
pub async fn set_frame_duration(
    root: &Path,
    req: SetFrameDurationRequest,
) -> Result<DocSaved, LumenError> {
    let target = output_target(&req.doc, req.output.as_deref())?;
    check_duration(req.duration_ms)?;
    let mut doc = load_doc(root, &req.doc)?;
    check_frame_index(&doc, req.index)?;
    doc.frames[req.index].duration_ms = req.duration_ms;
    commit(root, target, &doc)
}

/// Set any of offset_x / offset_y / opacity_mult / scale for one layer in
/// one frame. At least one must be given.
pub async fn set_frame_mod(root: &Path, req: SetFrameModRequest) -> Result<DocSaved, LumenError> {
    let target = output_target(&req.doc, req.output.as_deref())?;
    if req.offset_x.is_none()
        && req.offset_y.is_none()
        && req.opacity_mult.is_none()
        && req.scale.is_none()
    {
        return Err(bad("give at least one of: offset_x, offset_y, opacity_mult, scale"));
    }
    for (value, what) in [(req.offset_x, "offset_x"), (req.offset_y, "offset_y")] {
        if let Some(v) = value
            && !(-DOC_MAX_OFFSET_PX..=DOC_MAX_OFFSET_PX).contains(&v)
        {
            return Err(bad(format!(
                "{what} {v} outside -{DOC_MAX_OFFSET_PX}..={DOC_MAX_OFFSET_PX}"
            )));
        }
    }
    if let Some(v) = req.opacity_mult {
        check_f32_range(v, 0.0, 1.0, "opacity_mult")?;
    }
    if let Some(v) = req.scale {
        check_f32_range(v, SCALE_MIN, SCALE_MAX, "scale")?;
    }
    let mut doc = load_doc(root, &req.doc)?;
    check_frame_index(&doc, req.frame)?;
    check_layer_index(&doc, req.layer)?;
    let layer_mod = &mut doc.frames[req.frame].layer_mods[req.layer];
    if let Some(v) = req.offset_x {
        layer_mod.offset_x = v;
    }
    if let Some(v) = req.offset_y {
        layer_mod.offset_y = v;
    }
    if let Some(v) = req.opacity_mult {
        layer_mod.opacity_mult = v;
    }
    if let Some(v) = req.scale {
        layer_mod.scale = v;
    }
    commit(root, target, &doc)
}

/// Add a named inclusive frame range. Names are unique (exact match).
pub async fn add_tag(root: &Path, req: AddTagRequest) -> Result<DocSaved, LumenError> {
    let target = output_target(&req.doc, req.output.as_deref())?;
    if req.name.chars().count() > DOC_MAX_NAME_CHARS {
        return Err(bad(format!("tag name exceeds {DOC_MAX_NAME_CHARS} characters")));
    }
    let name = clean_name(&req.name, "tag")?;
    if req.from_frame > req.to_frame {
        return Err(bad("from_frame must be <= to_frame"));
    }
    let mut doc = load_doc(root, &req.doc)?;
    let frame_count = frame_u32(doc.frames.len());
    if req.to_frame >= frame_count {
        return Err(bad(format!(
            "to_frame {} out of range ({frame_count} frames)",
            req.to_frame
        )));
    }
    if doc.tags.iter().any(|t| t.name == name) {
        return Err(bad("a tag with that name already exists"));
    }
    doc.tags.push(Tag {
        name,
        from_frame: req.from_frame,
        to_frame: req.to_frame,
    });
    commit(root, target, &doc)
}

/// Remove the tag with exactly this name.
pub async fn delete_tag(root: &Path, req: DeleteTagRequest) -> Result<DocSaved, LumenError> {
    let target = output_target(&req.doc, req.output.as_deref())?;
    let mut doc = load_doc(root, &req.doc)?;
    let position = doc
        .tags
        .iter()
        .position(|t| t.name == req.name)
        .ok_or_else(|| bad("no tag with that name"))?;
    doc.tags.remove(position);
    commit(root, target, &doc)
}

// ---------------------------------------------------------------------------
// Drawing tools
// ---------------------------------------------------------------------------

/// Write one pixel. Off-canvas coordinates are clipped (success, no change).
pub async fn set_pixel(root: &Path, req: SetPixelRequest) -> Result<DocSaved, LumenError> {
    let target = output_target(&req.doc, req.output.as_deref())?;
    let color = Rgba(parse_color(&req.color)?);
    let mut doc = load_doc(root, &req.doc)?;
    let image = layer_image_mut(&mut doc, req.layer)?;
    put_clipped(image, i64::from(req.x), i64::from(req.y), color);
    commit(root, target, &doc)
}

/// Fill an axis-aligned rectangle, clipped to the canvas. Work is bounded
/// by the clipped area, never by `w * h`.
pub async fn fill_rect(root: &Path, req: FillRectRequest) -> Result<DocSaved, LumenError> {
    let target = output_target(&req.doc, req.output.as_deref())?;
    if req.w == 0 || req.h == 0 {
        return Err(bad("w and h must be at least 1"));
    }
    let color = Rgba(parse_color(&req.color)?);
    let mut doc = load_doc(root, &req.doc)?;
    let image = layer_image_mut(&mut doc, req.layer)?;
    let x0 = i64::from(req.x);
    let y0 = i64::from(req.y);
    let xs = clip_span(x0, x0 + i64::from(req.w), image.width());
    let ys = clip_span(y0, y0 + i64::from(req.h), image.height());
    for y in ys {
        for x in xs.clone() {
            image.put_pixel(x, y, color);
        }
    }
    commit(root, target, &doc)
}

/// Draw a 1px Bresenham line, clipped to the canvas. Endpoints must lie
/// within ±`LINE_COORD_MAX_ABS` so the step count stays bounded.
pub async fn draw_line(root: &Path, req: DrawLineRequest) -> Result<DocSaved, LumenError> {
    let target = output_target(&req.doc, req.output.as_deref())?;
    for v in [req.x0, req.y0, req.x1, req.y1] {
        if !(-LINE_COORD_MAX_ABS..=LINE_COORD_MAX_ABS).contains(&v) {
            return Err(bad(format!(
                "line coordinates must lie within -{LINE_COORD_MAX_ABS}..={LINE_COORD_MAX_ABS}"
            )));
        }
    }
    let color = Rgba(parse_color(&req.color)?);
    let mut doc = load_doc(root, &req.doc)?;
    let image = layer_image_mut(&mut doc, req.layer)?;
    let (x1, y1) = (i64::from(req.x1), i64::from(req.y1));
    let (mut x, mut y) = (i64::from(req.x0), i64::from(req.y0));
    let dx = (x1 - x).abs();
    let dy = -(y1 - y).abs();
    let step_x = if x < x1 { 1 } else { -1 };
    let step_y = if y < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    // Each iteration advances the major axis by one, so dx.max(-dy) + 1
    // iterations always reach the endpoint.
    let steps_max = dx.max(-dy) + 1;
    for _ in 0..steps_max {
        put_clipped(image, x, y, color);
        if x == x1 && y == y1 {
            break;
        }
        let err_twice = 2 * err;
        if err_twice >= dy {
            err += dy;
            x += step_x;
        }
        if err_twice <= dx {
            err += dx;
            y += step_y;
        }
    }
    commit(root, target, &doc)
}

/// Draw a circle (disk if `filled`, else a 1px 8-connected outline). A pixel
/// is inside when `dx² + dy² <= r² + r`; outline pixels are inside pixels
/// with at least one 4-neighbour outside. Work is bounded by the radius box
/// clipped to the canvas, so huge radii cost at most one canvas scan.
pub async fn draw_circle(root: &Path, req: DrawCircleRequest) -> Result<DocSaved, LumenError> {
    let target = output_target(&req.doc, req.output.as_deref())?;
    let color = Rgba(parse_color(&req.color)?);
    let mut doc = load_doc(root, &req.doc)?;
    let image = layer_image_mut(&mut doc, req.layer)?;
    let (cx, cy, r) = (i64::from(req.cx), i64::from(req.cy), i64::from(req.r));
    // i128: dx and dy can approach 2^32, so their squares overflow i64 sums.
    let limit = i128::from(r) * i128::from(r) + i128::from(r);
    let inside = |x: i64, y: i64| {
        let (dx, dy) = (i128::from(x - cx), i128::from(y - cy));
        dx * dx + dy * dy <= limit
    };
    let xs = clip_span(cx - r, cx + r + 1, image.width());
    let ys = clip_span(cy - r, cy + r + 1, image.height());
    for y in ys {
        for x in xs.clone() {
            let (px, py) = (i64::from(x), i64::from(y));
            if !inside(px, py) {
                continue;
            }
            let edge = !inside(px - 1, py)
                || !inside(px + 1, py)
                || !inside(px, py - 1)
                || !inside(px, py + 1);
            if req.filled || edge {
                image.put_pixel(x, y, color);
            }
        }
    }
    commit(root, target, &doc)
}

/// 4-way flood fill from a seed that must lie on the canvas. A pixel joins
/// the region when every RGBA channel is within `tolerance` of the seed
/// pixel. If the seed already matches `color` within tolerance the call is a
/// successful no-op. Each pixel is queued at most once (visited bitmap), so
/// the queue never exceeds the canvas pixel count.
pub async fn flood_fill(root: &Path, req: FloodFillRequest) -> Result<DocSaved, LumenError> {
    let target = output_target(&req.doc, req.output.as_deref())?;
    let fill = Rgba(parse_color(&req.color)?);
    let mut doc = load_doc(root, &req.doc)?;
    let image = layer_image_mut(&mut doc, req.layer)?;
    let (width, height) = image.dimensions();
    let (seed_x, seed_y) = match (u32::try_from(req.x), u32::try_from(req.y)) {
        (Ok(x), Ok(y)) if x < width && y < height => (x, y),
        _ => return Err(bad(format!("seed is outside the {width}x{height} canvas"))),
    };
    let seed = *image.get_pixel(seed_x, seed_y);
    if !within_tolerance(seed, fill, req.tolerance) {
        flood_region(image, seed_x, seed_y, fill, req.tolerance);
    }
    commit(root, target, &doc)
}

fn flood_region(image: &mut RgbaImage, seed_x: u32, seed_y: u32, fill: Rgba<u8>, tolerance: u8) {
    let (width, height) = image.dimensions();
    // check_dims caps both edges at 4096, so the count fits u32 and usize.
    let pixel_count = width as usize * height as usize;
    let seed = *image.get_pixel(seed_x, seed_y);
    let mut visited = vec![false; pixel_count];
    let mut queue: VecDeque<(u32, u32)> = VecDeque::new();
    visited[(seed_y * width + seed_x) as usize] = true;
    queue.push_back((seed_x, seed_y));
    let mut painted: usize = 0;
    while let Some((x, y)) = queue.pop_front() {
        image.put_pixel(x, y, fill);
        painted += 1;
        assert!(painted <= pixel_count, "flood fill painted more pixels than exist");
        let neighbours = [
            (x.checked_sub(1), Some(y)),
            (x.checked_add(1).filter(|&v| v < width), Some(y)),
            (Some(x), y.checked_sub(1)),
            (Some(x), y.checked_add(1).filter(|&v| v < height)),
        ];
        for (nx, ny) in neighbours {
            let (Some(nx), Some(ny)) = (nx, ny) else {
                continue;
            };
            let slot = (ny * width + nx) as usize;
            if visited[slot] || !within_tolerance(*image.get_pixel(nx, ny), seed, tolerance) {
                continue;
            }
            visited[slot] = true;
            queue.push_back((nx, ny));
        }
    }
}

// ---------------------------------------------------------------------------
// Transform tools
// ---------------------------------------------------------------------------

/// Mirror one layer or every layer. Frame mods are left untouched.
pub async fn flip(root: &Path, req: FlipRequest) -> Result<DocSaved, LumenError> {
    let target = output_target(&req.doc, req.output.as_deref())?;
    let mut doc = load_doc(root, &req.doc)?;
    for idx in layer_range(&doc, req.layer)? {
        let image = &mut doc.layers[idx].image;
        if req.horizontal {
            imageops::flip_horizontal_in_place(image);
        } else {
            imageops::flip_vertical_in_place(image);
        }
    }
    commit(root, target, &doc)
}

/// Rotate clockwise by 90, 180, or 270 degrees. 90/270 swap the canvas
/// width and height, so on a non-square canvas they must rotate every layer
/// (`layer: None`); a single layer is accepted only when the canvas is
/// square. Per-frame `offset_x`/`offset_y` mods are deliberately left
/// un-rotated: they are animation data, not pixels.
pub async fn rotate(root: &Path, req: RotateRequest) -> Result<DocSaved, LumenError> {
    let target = output_target(&req.doc, req.output.as_deref())?;
    if !matches!(req.degrees, 90 | 180 | 270) {
        return Err(bad("degrees must be one of: 90, 180, 270"));
    }
    let mut doc = load_doc(root, &req.doc)?;
    let swaps_axes = req.degrees != 180;
    if swaps_axes && req.layer.is_some() && doc.width != doc.height {
        return Err(bad(
            "rotating one layer by 90/270 needs a square canvas; omit layer to rotate all",
        ));
    }
    for idx in layer_range(&doc, req.layer)? {
        let image = &mut doc.layers[idx].image;
        *image = match req.degrees {
            90 => imageops::rotate90(image),
            180 => imageops::rotate180(image),
            _ => imageops::rotate270(image),
        };
    }
    if swaps_axes {
        std::mem::swap(&mut doc.width, &mut doc.height);
    }
    commit(root, target, &doc)
}

/// Change the canvas size, placing existing content per `anchor`. New area
/// is transparent; content falling outside the new canvas is discarded.
/// `center` rounds toward zero when the size difference is odd.
pub async fn resize_canvas(root: &Path, req: ResizeCanvasRequest) -> Result<DocSaved, LumenError> {
    let target = output_target(&req.doc, req.output.as_deref())?;
    check_dims(req.width, req.height)?;
    let anchor = parse_anchor(&req.anchor)?;
    let mut doc = load_doc(root, &req.doc)?;
    let grow_x = i64::from(req.width) - i64::from(doc.width);
    let grow_y = i64::from(req.height) - i64::from(doc.height);
    let (shift_x, shift_y) = match anchor {
        Anchor::Center => (grow_x / 2, grow_y / 2),
        Anchor::TopLeft => (0, 0),
        Anchor::TopRight => (grow_x, 0),
        Anchor::BottomLeft => (0, grow_y),
        Anchor::BottomRight => (grow_x, grow_y),
    };
    for layer in &mut doc.layers {
        let mut canvas: RgbaImage =
            ImageBuffer::from_pixel(req.width, req.height, Rgba([0, 0, 0, 0]));
        let xs = clip_span(shift_x, shift_x + i64::from(layer.image.width()), req.width);
        let ys = clip_span(shift_y, shift_y + i64::from(layer.image.height()), req.height);
        for y in ys {
            for x in xs.clone() {
                // Source coordinates are in range: x - shift_x lies in
                // 0..old_width by construction of the clipped span.
                let src_x = (i64::from(x) - shift_x) as u32;
                let src_y = (i64::from(y) - shift_y) as u32;
                canvas.put_pixel(x, y, *layer.image.get_pixel(src_x, src_y));
            }
        }
        layer.image = canvas;
    }
    doc.width = req.width;
    doc.height = req.height;
    commit(root, target, &doc)
}

/// Crop every layer to the rectangle (x, y, w, h), which must lie fully
/// inside the canvas with `w`, `h` >= 1.
pub async fn crop(root: &Path, req: CropRequest) -> Result<DocSaved, LumenError> {
    let target = output_target(&req.doc, req.output.as_deref())?;
    if req.w == 0 || req.h == 0 {
        return Err(bad("w and h must be at least 1"));
    }
    let mut doc = load_doc(root, &req.doc)?;
    let fits_x = req.x.checked_add(req.w).is_some_and(|end| end <= doc.width);
    let fits_y = req.y.checked_add(req.h).is_some_and(|end| end <= doc.height);
    if !fits_x || !fits_y {
        return Err(bad(format!(
            "crop rectangle does not fit inside the {}x{} canvas",
            doc.width, doc.height
        )));
    }
    for layer in &mut doc.layers {
        layer.image = imageops::crop_imm(&layer.image, req.x, req.y, req.w, req.h).to_image();
    }
    doc.width = req.w;
    doc.height = req.h;
    commit(root, target, &doc)
}

/// Insert `steps` frames after `from_frame` whose layer mods interpolate
/// from frame A (`from_frame`) to frame B (`to_frame`) at eased
/// t = k / (steps + 1), k = 1..=steps. Offsets interpolate as f32 and round;
/// durations copy from `from_frame`. Tags spanning the insert point grow.
pub async fn tween_frames(root: &Path, req: TweenFramesRequest) -> Result<DocSaved, LumenError> {
    let target = output_target(&req.doc, req.output.as_deref())?;
    if req.steps == 0 || req.steps > TWEEN_STEPS_MAX {
        return Err(bad(format!("steps must be 1..={TWEEN_STEPS_MAX}")));
    }
    let easing = parse_easing(&req.easing)?;
    if req.from_frame == req.to_frame {
        return Err(bad("from_frame and to_frame must differ"));
    }
    let mut doc = load_doc(root, &req.doc)?;
    check_frame_index(&doc, req.from_frame)?;
    check_frame_index(&doc, req.to_frame)?;
    let steps = req.steps as usize;
    if doc.frames.len() + steps > DOC_MAX_FRAMES {
        return Err(bad(format!(
            "tween would exceed {DOC_MAX_FRAMES} frames ({} + {steps})",
            doc.frames.len()
        )));
    }
    let start = &doc.frames[req.from_frame];
    let end = &doc.frames[req.to_frame];
    let mut tween = Vec::with_capacity(steps);
    for k in 1..=req.steps {
        let t = ease(easing, k as f32 / (req.steps + 1) as f32);
        let layer_mods = start
            .layer_mods
            .iter()
            .zip(&end.layer_mods)
            .map(|(a, b)| lerp_mod(a, b, t))
            .collect();
        tween.push(Frame {
            duration_ms: start.duration_ms,
            layer_mods,
        });
    }
    let at = req.from_frame + 1;
    doc.frames.splice(at..at, tween);
    shift_tags_for_insert(&mut doc.tags, at, req.steps);
    commit(root, target, &doc)
}

// ---------------------------------------------------------------------------
// Shared helpers (pub(crate) ones are also used by `text`)
// ---------------------------------------------------------------------------

/// Validate a caller-supplied name: non-empty, no control characters,
/// truncated to `DOC_MAX_NAME_CHARS` characters.
pub(crate) fn clean_name(raw: &str, what: &str) -> Result<String, LumenError> {
    if raw.is_empty() {
        return Err(bad(format!("{what} name is empty")));
    }
    if raw.chars().any(char::is_control) {
        return Err(bad(format!("{what} name contains control characters")));
    }
    Ok(raw.chars().take(DOC_MAX_NAME_CHARS).collect())
}

/// Return `base`, or `base (2)`, `base (3)`… — the first name no other layer
/// (excluding `skip`) uses. The base is shortened to keep the result within
/// `DOC_MAX_NAME_CHARS`. With at most `DOC_MAX_LAYERS` layers, one of the
/// `DOC_MAX_LAYERS + 1` candidates is always free.
pub(crate) fn unique_layer_name(
    doc: &SpriteDoc,
    base: &str,
    skip: Option<usize>,
) -> Result<String, LumenError> {
    let taken = |candidate: &str| {
        doc.layers
            .iter()
            .enumerate()
            .any(|(i, l)| Some(i) != skip && l.name == candidate)
    };
    if !taken(base) {
        return Ok(base.to_string());
    }
    for suffix_number in 2..=DOC_MAX_LAYERS + 1 {
        let suffix = format!(" ({suffix_number})");
        let keep_chars = DOC_MAX_NAME_CHARS - suffix.len();
        let candidate: String = base.chars().take(keep_chars).chain(suffix.chars()).collect();
        if !taken(&candidate) {
            return Ok(candidate);
        }
    }
    Err(LumenError::DocInvalid("no free layer name found".to_string()))
}

#[derive(Debug, Clone, Copy)]
enum Anchor {
    Center,
    TopLeft,
    TopRight,
    BottomLeft,
    BottomRight,
}

#[derive(Debug, Clone, Copy)]
enum Easing {
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
}

fn bad(message: impl Into<String>) -> LumenError {
    LumenError::BadParam(message.into())
}

/// Strict hex color: `#RRGGBB` (alpha 255) or `#RRGGBBAA`. Works on bytes so
/// multi-byte UTF-8 input can never hit a char-boundary panic.
fn parse_color(s: &str) -> Result<[u8; 4], LumenError> {
    let malformed = || bad("color must be #RRGGBB or #RRGGBBAA (hex digits)");
    let digits = match s.as_bytes().split_first() {
        Some((b'#', rest)) if rest.len() == 6 || rest.len() == 8 => rest,
        _ => return Err(malformed()),
    };
    let mut rgba = [0, 0, 0, 255];
    for (slot, [high, low]) in rgba.iter_mut().zip(digits.as_chunks::<2>().0) {
        let high = char::from(*high).to_digit(16).ok_or_else(malformed)?;
        let low = char::from(*low).to_digit(16).ok_or_else(malformed)?;
        // Two hex digits are at most 255.
        *slot = (high * 16 + low) as u8;
    }
    Ok(rgba)
}

fn parse_blend(s: &str) -> Result<BlendMode, LumenError> {
    match s {
        "normal" => Ok(BlendMode::Normal),
        "multiply" => Ok(BlendMode::Multiply),
        "screen" => Ok(BlendMode::Screen),
        "add" => Ok(BlendMode::Add),
        _ => Err(bad("blend must be one of: normal, multiply, screen, add")),
    }
}

fn parse_anchor(s: &str) -> Result<Anchor, LumenError> {
    match s {
        "center" => Ok(Anchor::Center),
        "top-left" => Ok(Anchor::TopLeft),
        "top-right" => Ok(Anchor::TopRight),
        "bottom-left" => Ok(Anchor::BottomLeft),
        "bottom-right" => Ok(Anchor::BottomRight),
        _ => Err(bad(
            "anchor must be one of: center, top-left, top-right, bottom-left, bottom-right",
        )),
    }
}

fn parse_easing(s: &str) -> Result<Easing, LumenError> {
    match s {
        "linear" => Ok(Easing::Linear),
        "ease_in" => Ok(Easing::EaseIn),
        "ease_out" => Ok(Easing::EaseOut),
        "ease_in_out" => Ok(Easing::EaseInOut),
        _ => Err(bad("easing must be one of: linear, ease_in, ease_out, ease_in_out")),
    }
}

/// Map t in 0..=1 through the easing curve; every curve maps 0→0 and 1→1.
fn ease(easing: Easing, t: f32) -> f32 {
    match easing {
        Easing::Linear => t,
        Easing::EaseIn => t * t,
        Easing::EaseOut => 1.0 - (1.0 - t) * (1.0 - t),
        Easing::EaseInOut => 3.0 * t * t - 2.0 * t * t * t,
    }
}

/// Interpolate two valid mods. The eased t stays in 0..=1, so results are
/// convex combinations of in-range values; the clamps only absorb float
/// rounding.
fn lerp_mod(a: &LayerMod, b: &LayerMod, t: f32) -> LayerMod {
    let lerp = |from: f32, to: f32| from + (to - from) * t;
    let offset = |from: i32, to: i32| {
        // |offset| <= 8192, exactly representable in f32.
        (lerp(from as f32, to as f32).round() as i32).clamp(-DOC_MAX_OFFSET_PX, DOC_MAX_OFFSET_PX)
    };
    LayerMod {
        offset_x: offset(a.offset_x, b.offset_x),
        offset_y: offset(a.offset_y, b.offset_y),
        opacity_mult: lerp(a.opacity_mult, b.opacity_mult).clamp(0.0, 1.0),
        scale: lerp(a.scale, b.scale).clamp(SCALE_MIN, SCALE_MAX),
    }
}

fn check_f32_range(value: f32, min: f32, max: f32, what: &str) -> Result<(), LumenError> {
    if !value.is_finite() || value < min || value > max {
        return Err(bad(format!("{what} must be a finite number in {min}..={max}")));
    }
    Ok(())
}

fn check_duration(duration_ms: u32) -> Result<(), LumenError> {
    if !(FRAME_DURATION_MS_MIN..=FRAME_DURATION_MS_MAX).contains(&duration_ms) {
        return Err(bad(format!(
            "duration_ms {duration_ms} outside {FRAME_DURATION_MS_MIN}..={FRAME_DURATION_MS_MAX}"
        )));
    }
    Ok(())
}

fn check_layer_index(doc: &SpriteDoc, index: usize) -> Result<(), LumenError> {
    if index >= doc.layers.len() {
        return Err(bad(format!(
            "layer index {index} out of range ({} layers)",
            doc.layers.len()
        )));
    }
    Ok(())
}

fn check_frame_index(doc: &SpriteDoc, index: usize) -> Result<(), LumenError> {
    if index >= doc.frames.len() {
        return Err(bad(format!(
            "frame index {index} out of range ({} frames)",
            doc.frames.len()
        )));
    }
    Ok(())
}

/// Frame indices/counts are bounded by `DOC_MAX_FRAMES` (1024), so they
/// always fit the `u32` tag fields.
fn frame_u32(value: usize) -> u32 {
    assert!(value <= DOC_MAX_FRAMES, "frame value exceeds DOC_MAX_FRAMES");
    value as u32
}

/// Keep tags aligned after `count` frames are inserted at `at`: tags
/// starting at or after `at` shift right; a tag with from < at <= to grows.
fn shift_tags_for_insert(tags: &mut [Tag], at: usize, count: u32) {
    let at = frame_u32(at);
    for tag in tags {
        if tag.from_frame >= at {
            tag.from_frame += count;
            tag.to_frame += count;
        } else if tag.to_frame >= at {
            tag.to_frame += count;
        }
    }
}

fn layer_range(doc: &SpriteDoc, layer: Option<usize>) -> Result<Range<usize>, LumenError> {
    match layer {
        Some(index) => {
            check_layer_index(doc, index)?;
            Ok(index..index + 1)
        }
        None => Ok(0..doc.layers.len()),
    }
}

fn layer_image_mut(doc: &mut SpriteDoc, layer: usize) -> Result<&mut RgbaImage, LumenError> {
    check_layer_index(doc, layer)?;
    Ok(&mut doc.layers[layer].image)
}

/// Clip the half-open span `start..end` (any i64) to `0..limit`.
fn clip_span(start: i64, end: i64, limit: u32) -> Range<u32> {
    let low = start.clamp(0, i64::from(limit));
    let high = end.clamp(low, i64::from(limit));
    // Both are clamped into 0..=limit, which fits u32.
    low as u32..high as u32
}

fn put_clipped(image: &mut RgbaImage, x: i64, y: i64, color: Rgba<u8>) {
    let on_canvas = (0..i64::from(image.width())).contains(&x)
        && (0..i64::from(image.height())).contains(&y);
    if on_canvas {
        image.put_pixel(x as u32, y as u32, color);
    }
}

fn within_tolerance(a: Rgba<u8>, b: Rgba<u8>, tolerance: u8) -> bool {
    a.0.iter().zip(b.0.iter()).all(|(&p, &q)| p.abs_diff(q) <= tolerance)
}

fn check_doc_suffix(path: &str) -> Result<(), LumenError> {
    if !path.ends_with(".lumen.json") {
        return Err(bad("output must end in .lumen.json"));
    }
    Ok(())
}

/// Where a stateless tool saves: `output` when given (must end in
/// `.lumen.json`), else back over the input document.
pub(crate) fn output_target<'a>(
    doc: &'a str,
    output: Option<&'a str>,
) -> Result<&'a str, LumenError> {
    match output {
        Some(path) => {
            check_doc_suffix(path)?;
            Ok(path)
        }
        None => Ok(doc),
    }
}

/// Save atomically via `save_doc` (which re-validates the whole document and
/// resolves the path inside the project root) and report the result.
pub(crate) fn commit(
    root: &Path,
    target: &str,
    doc: &SpriteDoc,
) -> Result<DocSaved, LumenError> {
    let path = save_doc(root, doc, target)?;
    Ok(DocSaved::of(&path, doc))
}
