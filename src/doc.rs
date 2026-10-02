//! Shared sprite document model (`SpriteDoc`) for lumen's tool core.
//!
//! A `SpriteDoc` is the working representation behind the canvas, layer,
//! frame, drawing, palette, export, pipeline, style, and dream tools: a
//! fixed-size canvas, an ordered stack of RGBA layers, a frame list with
//! durations and per-frame layer modifications, named tags, and an optional
//! palette. Documents persist as `.lumen.json` files — JSON metadata with
//! each layer stored as base64 PNG bytes (lossless and bounded).
//!
//! Contract summary:
//! - Accepted: paths resolving inside the project root and ending in
//!   `.lumen.json`; dimensions 1..=4096 px; 1..=64 layers; 1..=1024 frames;
//!   every layer exactly the document dimensions; opacities in 0..=1.
//! - Rejected: anything failing the above, reported as a typed
//!   `LumenError` — never a panic, never a half-written file.
//! - Bounds: no stored file may exceed `MAX_SPRITE_BYTES`; every pixel loop
//!   is bounded by the declared dimensions; layer names are capped.
//! - Trust boundary: documents are project data, never trusted code.
//! - Failure behavior: load validates fully before returning; save encodes
//!   and validates before touching the destination, then writes atomically
//!   (temp file + rename), so a failed save leaves the old file intact.

#![forbid(unsafe_code)]

use std::io::{Cursor, Write};
use std::path::{Path, PathBuf};

use base64::Engine as _;
use image::{ImageBuffer, Rgba, RgbaImage};
use serde::{Deserialize, Serialize};

use crate::{LumenError, MAX_SPRITE_BYTES, resolve_write_path};

/// Document format version. Bump when the stored shape changes.
pub const DOC_VERSION: u32 = 1;
/// Largest canvas edge, in pixels.
pub const DOC_MAX_DIMENSION: u32 = 4096;
/// Largest layer stack.
pub const DOC_MAX_LAYERS: usize = 64;
/// Largest frame list.
pub const DOC_MAX_FRAMES: usize = 1024;
/// Longest accepted layer or tag name, in characters.
pub const DOC_MAX_NAME_CHARS: usize = 128;
/// Widest accepted per-frame layer offset, in pixels (either direction).
pub const DOC_MAX_OFFSET_PX: i32 = 8192;

/// How a layer's RGB combines with the canvas beneath it. Alpha always
/// composites src-over; the mode only affects the color math.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlendMode {
    Normal,
    Multiply,
    Screen,
    Add,
}

/// One RGBA layer, always exactly the document dimensions.
#[derive(Debug, Clone)]
pub struct Layer {
    pub name: String,
    pub visible: bool,
    /// 0.0 (transparent) ..= 1.0 (opaque).
    pub opacity: f32,
    pub blend: BlendMode,
    pub image: RgbaImage,
}

/// Per-frame modification of one layer, index-aligned with the layer stack.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayerMod {
    pub offset_x: i32,
    pub offset_y: i32,
    /// Multiplies the layer opacity. 0.0..=1.0.
    pub opacity_mult: f32,
    /// Scale about the layer center. 1.0 is identity; 0.0625..=16.0.
    pub scale: f32,
}

/// One animation frame: a duration plus one `LayerMod` per layer.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Frame {
    pub duration_ms: u32,
    pub layer_mods: Vec<LayerMod>,
}

/// A named, inclusive frame range (mirrors Aseprite tags).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Tag {
    pub name: String,
    pub from_frame: u32,
    pub to_frame: u32,
}

/// The working sprite document.
#[derive(Debug, Clone)]
pub struct SpriteDoc {
    pub width: u32,
    pub height: u32,
    pub layers: Vec<Layer>,
    pub frames: Vec<Frame>,
    pub tags: Vec<Tag>,
    /// Optional palette as RGBA entries. Empty means "no palette bound".
    pub palette: Vec<[u8; 4]>,
}

// ---------------------------------------------------------------------------
// Stored (serializable) shape
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize)]
struct StoredLayer {
    name: String,
    visible: bool,
    opacity: f32,
    blend: BlendMode,
    png_base64: String,
}

#[derive(Serialize, Deserialize)]
struct StoredDoc {
    version: u32,
    width: u32,
    height: u32,
    layers: Vec<StoredLayer>,
    frames: Vec<Frame>,
    tags: Vec<Tag>,
    palette: Vec<[u8; 4]>,
}

// ---------------------------------------------------------------------------
// Shared result shapes (keep per-tool boilerplate small, stay typed)
// ---------------------------------------------------------------------------

/// What mutating document tools report.
#[derive(Debug, Clone, Serialize)]
pub struct DocSaved {
    pub path: String,
    pub width: u32,
    pub height: u32,
    pub layers: usize,
    pub frames: usize,
}

impl DocSaved {
    pub fn of(path: &Path, doc: &SpriteDoc) -> Self {
        Self {
            path: path.display().to_string(),
            width: doc.width,
            height: doc.height,
            layers: doc.layers.len(),
            frames: doc.frames.len(),
        }
    }
}

/// What PNG-writing tools report.
#[derive(Debug, Clone, Serialize)]
pub struct ImageSaved {
    pub path: String,
    pub width: u32,
    pub height: u32,
}

// ---------------------------------------------------------------------------
// Validation helpers
// ---------------------------------------------------------------------------

/// Check canvas dimensions before any allocation happens.
pub fn check_dims(width: u32, height: u32) -> Result<(), LumenError> {
    if width == 0 || height == 0 {
        return Err(LumenError::BadParam(
            "dimensions must be at least 1x1".to_string(),
        ));
    }
    if width > DOC_MAX_DIMENSION || height > DOC_MAX_DIMENSION {
        return Err(LumenError::BadParam(format!(
            "dimensions {width}x{height} exceed the {DOC_MAX_DIMENSION}px limit"
        )));
    }
    Ok(())
}

fn check_name(what: &str, name: &str) -> Result<(), LumenError> {
    if name.is_empty() {
        return Err(LumenError::BadParam(format!("{what} name is empty")));
    }
    if name.chars().count() > DOC_MAX_NAME_CHARS {
        return Err(LumenError::BadParam(format!(
            "{what} name exceeds {DOC_MAX_NAME_CHARS} characters"
        )));
    }
    Ok(())
}

fn check_opacity(opacity: f32, what: &str) -> Result<(), LumenError> {
    if !opacity.is_finite() || opacity < 0.0 || opacity > 1.0 {
        return Err(LumenError::BadParam(format!(
            "{what} opacity {opacity} is outside 0.0..=1.0"
        )));
    }
    Ok(())
}

fn check_layer_mod(lm: &LayerMod, idx: usize) -> Result<(), LumenError> {
    if lm.offset_x.abs() > DOC_MAX_OFFSET_PX || lm.offset_y.abs() > DOC_MAX_OFFSET_PX {
        return Err(LumenError::BadParam(format!(
            "layer mod {idx}: offset exceeds {DOC_MAX_OFFSET_PX}px"
        )));
    }
    check_opacity(lm.opacity_mult, &format!("layer mod {idx}"))?;
    if !lm.scale.is_finite() || lm.scale < 0.0625 || lm.scale > 16.0 {
        return Err(LumenError::BadParam(format!(
            "layer mod {idx}: scale {} outside 0.0625..=16.0",
            lm.scale
        )));
    }
    Ok(())
}

/// The identity layer modification (no offset, full opacity, scale 1).
pub fn identity_mod() -> LayerMod {
    LayerMod {
        offset_x: 0,
        offset_y: 0,
        opacity_mult: 1.0,
        scale: 1.0,
    }
}

/// Re-align every frame's `layer_mods` with the layer stack after layers are
/// added, removed, or merged. New slots get the identity mod; extras drop.
pub fn sync_frame_mods(doc: &mut SpriteDoc) {
    let n = doc.layers.len();
    for frame in &mut doc.frames {
        frame.layer_mods.resize_with(n, identity_mod);
        frame.layer_mods.truncate(n);
    }
}

/// Immutable layer lookup by name.
pub fn get_layer<'a>(doc: &'a SpriteDoc, name: &str) -> Result<&'a Layer, LumenError> {
    doc.layers.iter().find(|l| l.name == name).ok_or_else(|| {
        LumenError::BadParam(format!("no layer named {name:?}"))
    })
}

/// Mutable layer lookup by name.
pub fn get_layer_mut<'a>(
    doc: &'a mut SpriteDoc,
    name: &str,
) -> Result<&'a mut Layer, LumenError> {
    doc.layers
        .iter_mut()
        .find(|l| l.name == name)
        .ok_or_else(|| LumenError::BadParam(format!("no layer named {name:?}")))
}

// ---------------------------------------------------------------------------
// Paths
// ---------------------------------------------------------------------------

/// Resolve a sprite-document path: project-relative, inside the root, ending
/// in `.lumen.json`. For loads the file must exist; for saves it must not be
/// a directory.
pub fn resolve_doc_path(
    user_path: &str,
    root: &Path,
    must_exist: bool,
) -> Result<PathBuf, LumenError> {
    let path = resolve_write_path(user_path, root, &["json"])?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default();
    if !name.ends_with(".lumen.json") {
        return Err(LumenError::PathRejected(
            "sprite documents must end in .lumen.json".to_string(),
        ));
    }
    if must_exist && !path.is_file() {
        return Err(LumenError::PathRejected(
            "document does not exist inside the project".to_string(),
        ));
    }
    Ok(path)
}

// ---------------------------------------------------------------------------
// Construction, persistence
// ---------------------------------------------------------------------------

/// Create a new document: one transparent layer, one frame, no tags.
pub fn new_doc(width: u32, height: u32) -> Result<SpriteDoc, LumenError> {
    check_dims(width, height)?;
    let layer = Layer {
        name: "Background".to_string(),
        visible: true,
        opacity: 1.0,
        blend: BlendMode::Normal,
        image: ImageBuffer::from_pixel(width, height, Rgba([0, 0, 0, 0])),
    };
    Ok(SpriteDoc {
        width,
        height,
        layers: vec![layer],
        frames: vec![Frame {
            duration_ms: 180,
            layer_mods: vec![identity_mod()],
        }],
        tags: Vec::new(),
        palette: Vec::new(),
    })
}

fn encode_layer_png(image: &RgbaImage) -> Result<Vec<u8>, LumenError> {
    let mut buf = Vec::new();
    {
        let mut cursor = Cursor::new(&mut buf);
        image
            .write_to(&mut cursor, image::ImageFormat::Png)
            .map_err(|e| LumenError::Io(format!("layer png encode failed: {e}")))?;
    }
    Ok(buf)
}

fn decode_layer_png(
    bytes: &[u8],
    width: u32,
    height: u32,
    idx: usize,
) -> Result<RgbaImage, LumenError> {
    let decoded = image::load_from_memory(bytes)
        .map_err(|e| LumenError::DocInvalid(format!("layer {idx}: png decode failed: {e}")))?;
    let rgba = decoded.to_rgba8();
    if rgba.width() != width || rgba.height() != height {
        return Err(LumenError::DocInvalid(format!(
            "layer {idx}: stored {}x{} does not match document {}x{}",
            rgba.width(),
            rgba.height(),
            width,
            height
        )));
    }
    Ok(rgba)
}

/// Save a document atomically: encode and validate first, then temp-file +
/// rename so a failure never leaves a half-written document.
pub fn save_doc(
    root: &Path,
    doc: &SpriteDoc,
    user_path: &str,
) -> Result<PathBuf, LumenError> {
    check_dims(doc.width, doc.height)?;
    if doc.layers.is_empty() || doc.layers.len() > DOC_MAX_LAYERS {
        return Err(LumenError::DocInvalid(format!(
            "layer count {} outside 1..={}",
            doc.layers.len(),
            DOC_MAX_LAYERS
        )));
    }
    if doc.frames.is_empty() || doc.frames.len() > DOC_MAX_FRAMES {
        return Err(LumenError::DocInvalid(format!(
            "frame count {} outside 1..={}",
            doc.frames.len(),
            DOC_MAX_FRAMES
        )));
    }
    let mut stored_layers = Vec::with_capacity(doc.layers.len());
    for (idx, layer) in doc.layers.iter().enumerate() {
        check_name("layer", &layer.name)?;
        check_opacity(layer.opacity, "layer")?;
        if layer.image.width() != doc.width || layer.image.height() != doc.height {
            return Err(LumenError::DocInvalid(format!(
                "layer {idx}: in-memory {}x{} does not match document {}x{}",
                layer.image.width(),
                layer.image.height(),
                doc.width,
                doc.height
            )));
        }
        let png = encode_layer_png(&layer.image)?;
        stored_layers.push(StoredLayer {
            name: layer.name.clone(),
            visible: layer.visible,
            opacity: layer.opacity,
            blend: layer.blend,
            png_base64: base64::engine::general_purpose::STANDARD.encode(&png),
        });
    }
    for (idx, frame) in doc.frames.iter().enumerate() {
        if frame.layer_mods.len() != doc.layers.len() {
            return Err(LumenError::DocInvalid(format!(
                "frame {idx}: {} layer mods for {} layers",
                frame.layer_mods.len(),
                doc.layers.len()
            )));
        }
        for (midx, lm) in frame.layer_mods.iter().enumerate() {
            check_layer_mod(lm, midx)?;
        }
    }
    for (idx, tag) in doc.tags.iter().enumerate() {
        check_name("tag", &tag.name)?;
        if tag.from_frame > tag.to_frame || tag.to_frame >= doc.frames.len() as u32 {
            return Err(LumenError::DocInvalid(format!(
                "tag {idx}: range {}..={} outside 0..{}",
                tag.from_frame,
                tag.to_frame,
                stored_frames_len(doc)
            )));
        }
    }
    let stored = StoredDoc {
        version: DOC_VERSION,
        width: doc.width,
        height: doc.height,
        layers: stored_layers,
        frames: doc.frames.clone(),
        tags: doc.tags.clone(),
        palette: doc.palette.clone(),
    };
    let bytes =
        serde_json::to_vec(&stored).map_err(|e| LumenError::Io(format!("doc encode failed: {e}")))?;
    if bytes.len() as u64 > MAX_SPRITE_BYTES {
        return Err(LumenError::DocInvalid(format!(
            "encoded document exceeds {MAX_SPRITE_BYTES} bytes"
        )));
    }
    let path = resolve_doc_path(user_path, root, false)?;
    let tmp = path.with_extension(format!("json.tmp-{}", std::process::id()));
    if let Err(e) = std::fs::write(&tmp, &bytes)
        .and_then(|()| std::fs::rename(&tmp, &path))
    {
        let _ = std::fs::remove_file(&tmp);
        return Err(LumenError::from(e));
    }
    Ok(path)
}

fn stored_frames_len(doc: &SpriteDoc) -> usize {
    doc.frames.len()
}

/// Load and fully validate a document. Nothing is trusted until every check
/// passes.
pub fn load_doc(root: &Path, user_path: &str) -> Result<SpriteDoc, LumenError> {
    let path = resolve_doc_path(user_path, root, true)?;
    let meta = std::fs::metadata(&path).map_err(LumenError::from)?;
    if meta.len() > MAX_SPRITE_BYTES {
        return Err(LumenError::PathRejected(format!(
            "document larger than {MAX_SPRITE_BYTES} bytes"
        )));
    }
    let bytes = std::fs::read(&path).map_err(LumenError::from)?;
    let stored: StoredDoc = serde_json::from_slice(&bytes)
        .map_err(|e| LumenError::DocInvalid(format!("json parse failed: {e}")))?;
    if stored.version != DOC_VERSION {
        return Err(LumenError::DocInvalid(format!(
            "unsupported document version {}",
            stored.version
        )));
    }
    check_dims(stored.width, stored.height)?;
    if stored.layers.is_empty() || stored.layers.len() > DOC_MAX_LAYERS {
        return Err(LumenError::DocInvalid(format!(
            "layer count {} outside 1..={}",
            stored.layers.len(),
            DOC_MAX_LAYERS
        )));
    }
    if stored.frames.is_empty() || stored.frames.len() > DOC_MAX_FRAMES {
        return Err(LumenError::DocInvalid(format!(
            "frame count {} outside 1..={}",
            stored.frames.len(),
            DOC_MAX_FRAMES
        )));
    }
    let mut layers = Vec::with_capacity(stored.layers.len());
    for (idx, sl) in stored.layers.iter().enumerate() {
        check_name("layer", &sl.name)?;
        check_opacity(sl.opacity, "layer")?;
        if sl.png_base64.len() as u64 > MAX_SPRITE_BYTES {
            return Err(LumenError::DocInvalid(format!(
                "layer {idx}: payload exceeds {MAX_SPRITE_BYTES} bytes"
            )));
        }
        let png = base64::engine::general_purpose::STANDARD
            .decode(sl.png_base64.as_bytes())
            .map_err(|e| LumenError::DocInvalid(format!("layer {idx}: base64 failed: {e}")))?;
        let image = decode_layer_png(&png, stored.width, stored.height, idx)?;
        layers.push(Layer {
            name: sl.name.clone(),
            visible: sl.visible,
            opacity: sl.opacity,
            blend: sl.blend,
            image,
        });
    }
    for (idx, frame) in stored.frames.iter().enumerate() {
        if frame.layer_mods.len() != layers.len() {
            return Err(LumenError::DocInvalid(format!(
                "frame {idx}: {} layer mods for {} layers",
                frame.layer_mods.len(),
                layers.len()
            )));
        }
        for (midx, lm) in frame.layer_mods.iter().enumerate() {
            check_layer_mod(lm, midx)?;
        }
    }
    for (idx, tag) in stored.tags.iter().enumerate() {
        check_name("tag", &tag.name)?;
        if tag.from_frame > tag.to_frame || tag.to_frame >= stored.frames.len() as u32 {
            return Err(LumenError::DocInvalid(format!(
                "tag {idx}: range {}..={} outside 0..{}",
                tag.from_frame,
                tag.to_frame,
                stored.frames.len()
            )));
        }
    }
    Ok(SpriteDoc {
        width: stored.width,
        height: stored.height,
        layers,
        frames: stored.frames,
        tags: stored.tags,
        palette: stored.palette,
    })
}

// ---------------------------------------------------------------------------
// Compositing
// ---------------------------------------------------------------------------

fn blend_src_over(dst: Rgba<u8>, src: Rgba<u8>, mode: BlendMode, opacity: f32) -> Rgba<u8> {
    let src_a = src[3] as f32 / 255.0 * opacity;
    if src_a <= 0.0 {
        return dst;
    }
    let dst_a = dst[3] as f32 / 255.0;
    let (sr, sg, sb) = (src[0] as f32, src[1] as f32, src[2] as f32);
    let (dr, dg, db) = (dst[0] as f32, dst[1] as f32, dst[2] as f32);
    // Blend the RGB channels; alpha always composites src-over.
    let (br, bg, bb) = match mode {
        BlendMode::Normal => (sr, sg, sb),
        BlendMode::Multiply => (sr * dr / 255.0, sg * dg / 255.0, sb * db / 255.0),
        BlendMode::Screen => (
            255.0 - (255.0 - sr) * (255.0 - dr) / 255.0,
            255.0 - (255.0 - sg) * (255.0 - dg) / 255.0,
            255.0 - (255.0 - sb) * (255.0 - db) / 255.0,
        ),
        BlendMode::Add => (
            (sr + dr).min(255.0),
            (sg + dg).min(255.0),
            (sb + db).min(255.0),
        ),
    };
    let out_a = src_a + dst_a * (1.0 - src_a);
    // out_a > 0 whenever src_a > 0, so the division is safe.
    let mix = |b: f32, d: f32| (b * src_a + d * dst_a * (1.0 - src_a)) / out_a;
    Rgba([
        mix(br, dr).round().clamp(0.0, 255.0) as u8,
        mix(bg, dg).round().clamp(0.0, 255.0) as u8,
        mix(bb, db).round().clamp(0.0, 255.0) as u8,
        (out_a * 255.0).round().clamp(0.0, 255.0) as u8,
    ])
}

/// Blit one layer image onto the canvas with offset, blend mode, and
/// effective opacity. Out-of-canvas source pixels are clipped, never panic.
fn blit_layer(
    canvas: &mut RgbaImage,
    src: &RgbaImage,
    offset_x: i32,
    offset_y: i32,
    mode: BlendMode,
    opacity: f32,
) {
    let (cw, ch) = (canvas.width() as i32, canvas.height() as i32);
    let (sw, sh) = (src.width() as i32, src.height() as i32);
    for sy in 0..sh {
        let dy = sy + offset_y;
        if dy < 0 || dy >= ch {
            continue;
        }
        for sx in 0..sw {
            let dx = sx + offset_x;
            if dx < 0 || dx >= cw {
                continue;
            }
            let s = src.get_pixel(sx as u32, sy as u32);
            if s[3] == 0 {
                continue;
            }
            let d = canvas.get_pixel(dx as u32, dy as u32);
            canvas.put_pixel(dx as u32, dy as u32, blend_src_over(*d, *s, mode, opacity));
        }
    }
}

/// Scale a layer image about its center with nearest-neighbor sampling
/// (pixel-art safe). Scale 1.0 returns the image unmodified.
fn scaled_layer_image(doc: &SpriteDoc, layer: &Layer, scale: f32) -> RgbaImage {
    if (scale - 1.0).abs() <= f32::EPSILON {
        return layer.image.clone();
    }
    let sw = (doc.width as f32 * scale)
        .round()
        .clamp(1.0, DOC_MAX_DIMENSION as f32) as u32;
    let sh = (doc.height as f32 * scale)
        .round()
        .clamp(1.0, DOC_MAX_DIMENSION as f32) as u32;
    image::imageops::resize(
        &layer.image,
        sw,
        sh,
        image::imageops::FilterType::Nearest,
    )
}

/// Composite one frame: layers bottom-to-top, honoring visibility, opacity,
/// blend modes, and the frame's layer modifications. Scale is applied about
/// the layer center; the offset then translates the result.
pub fn composite_frame(doc: &SpriteDoc, frame_idx: usize) -> Result<RgbaImage, LumenError> {
    let frame = doc.frames.get(frame_idx).ok_or_else(|| {
        LumenError::BadParam(format!(
            "frame index {frame_idx} out of range ({} frames)",
            doc.frames.len()
        ))
    })?;
    if frame.layer_mods.len() != doc.layers.len() {
        return Err(LumenError::DocInvalid(format!(
            "frame {frame_idx}: {} layer mods for {} layers",
            frame.layer_mods.len(),
            doc.layers.len()
        )));
    }
    let mut canvas: RgbaImage =
        ImageBuffer::from_pixel(doc.width, doc.height, Rgba([0, 0, 0, 0]));
    for (layer, lm) in doc.layers.iter().zip(frame.layer_mods.iter()) {
        if !layer.visible {
            continue;
        }
        let opacity = (layer.opacity * lm.opacity_mult).clamp(0.0, 1.0);
        if opacity <= 0.0 {
            continue;
        }
        let scaled = scaled_layer_image(doc, layer, lm.scale);
        // Identity placement keeps the layer center fixed; the mod offset
        // translates from there.
        let base_x = (doc.width as i32 - scaled.width() as i32) / 2 + lm.offset_x;
        let base_y = (doc.height as i32 - scaled.height() as i32) / 2 + lm.offset_y;
        blit_layer(&mut canvas, &scaled, base_x, base_y, layer.blend, opacity);
    }
    Ok(canvas)
}

// ---------------------------------------------------------------------------
// PNG helpers shared by export/pipeline/dream tools
// ---------------------------------------------------------------------------

/// Load a project PNG as RGBA8, bounded like every other sprite read.
pub fn load_png_rgba(root: &Path, user_path: &str) -> Result<RgbaImage, LumenError> {
    let path = crate::resolve_sprite_path(user_path, root)?;
    let meta = std::fs::metadata(&path).map_err(LumenError::from)?;
    if meta.len() > MAX_SPRITE_BYTES {
        return Err(LumenError::PathRejected(format!(
            "file larger than {MAX_SPRITE_BYTES} bytes"
        )));
    }
    let img = image::ImageReader::open(&path)
        .map_err(|e| LumenError::Io(e.to_string()))?
        .with_guessed_format()
        .map_err(|e| LumenError::BadPng(e.to_string()))?
        .decode()
        .map_err(|e| LumenError::BadPng(e.to_string()))?;
    Ok(img.to_rgba8())
}

/// Write an RGBA image as PNG, atomically (temp file + rename).
pub fn save_png(
    root: &Path,
    image: &RgbaImage,
    user_path: &str,
) -> Result<ImageSaved, LumenError> {
    check_dims(image.width(), image.height())?;
    let path = resolve_write_path(user_path, root, &["png"])?;
    let tmp = path.with_extension(format!("png.tmp-{}", std::process::id()));
    let write_result = (|| -> Result<(), LumenError> {
        let file = std::fs::File::create(&tmp).map_err(LumenError::from)?;
        let mut writer = std::io::BufWriter::new(file);
        image
            .write_to(&mut writer, image::ImageFormat::Png)
            .map_err(|e| LumenError::Io(format!("png encode failed: {e}")))?;
        writer.flush().map_err(LumenError::from)?;
        Ok(())
    })();
    if let Err(e) = write_result {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    if let Err(e) = std::fs::rename(&tmp, &path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(LumenError::from(e));
    }
    Ok(ImageSaved {
        path: path.display().to_string(),
        width: image.width(),
        height: image.height(),
    })
}
