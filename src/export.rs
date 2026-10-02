//! Document exports (sprite/frame/sheet/tag), console backends (TIC-80
//! cart, WASM-4 sprite source), and layer import.
//!
//! Contract summary:
//! - Every render goes through `doc::composite_frame`, so exports honor
//!   visibility, blend modes, opacity, and per-frame layer mods exactly as
//!   the document defines them.
//! - Accepted: project-relative paths; PNG outputs ending in `.png`, sidecar
//!   metadata ending in `.json`, document outputs ending in `.lumen.json`.
//! - Bounds: every output image is sized with checked arithmetic and
//!   rejected (`BadParam`) before any compositing if it would exceed
//!   `DOC_MAX_DIMENSION` on either edge, so work and memory are capped at
//!   one `DOC_MAX_DIMENSION`² RGBA canvas plus one frame.
//! - Failure behavior: all parameters and output paths are validated before
//!   the first byte is written. PNGs and documents are written atomically by
//!   `doc`. A sidecar is written only after its PNG succeeded.
//! - Overwrites: existing files at an output path are replaced.

#![forbid(unsafe_code)]

use std::path::Path;

use image::{ImageBuffer, Rgba, RgbaImage};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::LumenError;
use crate::doc::{
    BlendMode, DOC_MAX_LAYERS, DOC_MAX_NAME_CHARS, DocSaved, ImageSaved, Layer, SpriteDoc, Tag,
    check_dims, composite_frame, load_doc, load_png_rgba, save_doc, save_png, sync_frame_mods,
};

/// Widest accepted sheet grid, in columns.
pub const SHEET_COLUMNS_MAX: u32 = 64;
/// Largest accepted gap between sheet cells, in pixels.
pub const SHEET_PADDING_MAX_PX: u32 = 64;
/// Most tag names quoted back in an "unknown tag" error.
const TAG_NAMES_LISTED_MAX: usize = 16;
/// Layer name used by `import_layer` when none is given.
const IMPORT_LAYER_DEFAULT_NAME: &str = "imported";

/// TIC-80 sprite memory is 512 8x8 tiles, 16 tiles wide: ids 0..256 are the
/// TILES chunk, 256..512 the SPRITES chunk, together one 128x256 px sheet.
pub const TIC80_SHEET_TILES_W: u32 = 16;
/// Height of the combined TIC-80 sheet, in tiles.
pub const TIC80_SHEET_TILES_H: u32 = 32;
/// TIC-80 palette slots (4 bits per pixel).
pub const TIC80_PALETTE_SLOTS: usize = 16;
const TIC80_TILE_PX: u32 = 8;
const TIC80_TILE_BYTES: usize = 32;
const TIC80_BANK_BYTES: usize = 256 * TIC80_TILE_BYTES;
const TIC80_SCREEN_W_PX: u32 = 240;
const TIC80_SCREEN_H_PX: u32 = 136;
const TIC80_CHUNK_TILES: u8 = 1;
const TIC80_CHUNK_SPRITES: u8 = 2;
const TIC80_CHUNK_CODE: u8 = 5;
const TIC80_CHUNK_PALETTE: u8 = 12;

/// WASM-4 screen edge; a single frame may not exceed it.
pub const WASM4_SCREEN_PX: u32 = 160;
/// Largest emitted WASM-4 sprite blob (half of the console's 64 KiB RAM).
pub const WASM4_SPRITE_BYTES_MAX: u64 = 32 * 1024;
/// WASM-4 `BLIT_2BPP` flag; `BLIT_1BPP` is 0.
const WASM4_BLIT_2BPP: u32 = 1;
const WASM4_NAME_DEFAULT: &str = "SPRITE";
const WASM4_NAME_CHARS_MAX: usize = 32;
const WASM4_BYTES_PER_LINE: usize = 16;

// ---------------------------------------------------------------------------
// Requests and results
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ExportSpriteRequest {
    /// Project-relative `.lumen.json` document to render.
    pub doc: String,
    /// Project-relative `.png` output path (required).
    pub output: String,
    /// Frame index to render. Defaults to 0.
    pub frame: Option<usize>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ExportSheetRequest {
    /// Project-relative `.lumen.json` document to render.
    pub doc: String,
    /// Project-relative `.png` sheet output path (required).
    pub output: String,
    /// Project-relative `.json` metadata sidecar path (required).
    pub meta_output: String,
    /// Grid columns, 1..=64. Clamped down to the exported frame count.
    pub columns: u32,
    /// Export only this tag's frame range. `None` exports every frame.
    pub tag: Option<String>,
    /// Transparent gap between cells, 0..=64 px. No outer border.
    pub padding: u32,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ExportTagRequest {
    /// Project-relative `.lumen.json` document to render.
    pub doc: String,
    /// Tag whose frame range becomes the filmstrip.
    pub tag: String,
    /// Project-relative `.png` output path (required).
    pub output: String,
    /// `true` lays frames out in one row; `false` in one column.
    pub horizontal: bool,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ImportLayerRequest {
    /// Project-relative `.lumen.json` document to extend.
    pub doc: String,
    /// Output `.lumen.json` path. `None` saves over `doc` in place.
    pub output: Option<String>,
    /// Project-relative `.png` to import; must match the document size.
    pub png: String,
    /// New layer name, 1..=128 chars, unique. Defaults to "imported".
    pub name: Option<String>,
}

/// JSON sidecar written by `export_sheet`.
#[derive(Debug, Clone, Serialize)]
pub struct SheetMeta {
    /// The sheet PNG path exactly as requested (project-relative).
    pub image: String,
    pub cell_w: u32,
    pub cell_h: u32,
    /// Effective column count (after clamping to the frame count).
    pub columns: u32,
    pub frames: Vec<SheetCell>,
}

/// One frame's placement in an exported sheet.
#[derive(Debug, Clone, Serialize)]
pub struct SheetCell {
    /// Document frame index.
    pub frame: usize,
    /// The requested tag, or else the first document tag covering the frame.
    pub tag: Option<String>,
    pub x: u32,
    pub y: u32,
    pub duration_ms: u32,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ExportTic80Request {
    /// Project-relative `.lumen.json` document to render.
    pub doc: String,
    /// Project-relative `.tic` cart output path (required).
    pub output: String,
    /// Project-relative `.json` metadata sidecar path (required).
    pub meta_output: String,
    /// Export only this tag's frame range. `None` exports every frame.
    pub tag: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ExportWasm4Request {
    /// Project-relative `.lumen.json` document to render.
    pub doc: String,
    /// Project-relative `.rs` sprite-source output path (required).
    pub output: String,
    /// Project-relative `.json` metadata sidecar path (required).
    pub meta_output: String,
    /// Export only this tag's frame range. `None` exports every frame.
    pub tag: Option<String>,
    /// Bits per pixel: 1 (2 palette slots) or 2 (4 palette slots).
    pub bpp: u8,
    /// Rust constant name, `[A-Z][A-Z0-9_]*`, at most 32 chars. Defaults to
    /// "SPRITE".
    pub name: Option<String>,
}

/// JSON sidecar written by `export_tic80`.
#[derive(Debug, Clone, Serialize)]
pub struct Tic80Meta {
    /// The cart path exactly as requested (project-relative).
    pub cart: String,
    /// `#RRGGBB` per used palette slot, slot 0 first.
    pub palette: Vec<String>,
    /// Slot holding transparent pixels (pass as `spr` colorkey), if any.
    pub transparent_index: Option<u8>,
    /// Frame size in 8x8 tiles (the `w`/`h` arguments of `spr`).
    pub tiles_w: u32,
    pub tiles_h: u32,
    pub frames: Vec<Tic80Cell>,
}

/// One frame's placement in the TIC-80 sprite sheet.
#[derive(Debug, Clone, Serialize)]
pub struct Tic80Cell {
    pub frame: usize,
    /// Top-left sprite id (0..512) to pass to `spr`.
    pub sprite_id: u32,
    pub duration_ms: u32,
}

/// JSON sidecar written by `export_wasm4`.
#[derive(Debug, Clone, Serialize)]
pub struct Wasm4Meta {
    /// The source path exactly as requested (project-relative).
    pub source: String,
    /// Rust constant name used in the source.
    pub name: String,
    pub bpp: u8,
    /// `blit` flags (`BLIT_1BPP` = 0, `BLIT_2BPP` = 1).
    pub flags: u32,
    /// One frame's size; frames stack vertically in one blob.
    pub width: u32,
    pub height: u32,
    pub bytes: usize,
    /// The four `PALETTE` registers as `#RRGGBB`; unused slots are black.
    pub palette: Vec<String>,
    /// `DRAW_COLORS` value mapping sprite values to palette slots.
    pub draw_colors: u16,
    /// Sprite value drawn as transparent (`DRAW_COLORS` nibble 0), if any.
    pub transparent_index: Option<u8>,
    pub frames: Vec<Wasm4Cell>,
}

/// One frame's placement in the WASM-4 blob (`blit_sub` source row).
#[derive(Debug, Clone, Serialize)]
pub struct Wasm4Cell {
    pub frame: usize,
    pub src_y: u32,
    pub duration_ms: u32,
}

// ---------------------------------------------------------------------------
// Shared machinery (also used by `pipeline`)
// ---------------------------------------------------------------------------

/// A validated row-major grid of equally sized cells. Construction proves
/// the full sheet fits `check_dims`, so every cell origin fits in `u32`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Grid {
    pub columns: u32,
    pub rows: u32,
    pub cell_w: u32,
    pub cell_h: u32,
    pub gap: u32,
    pub margin: u32,
    pub width: u32,
    pub height: u32,
}

impl Grid {
    /// Plan a grid for `cell_count` cells. `columns_requested` must be
    /// 1..=`SHEET_COLUMNS_MAX` and is clamped down to `cell_count`. `gap`
    /// separates cells; `margin` borders the whole sheet.
    pub(crate) fn plan(
        cell_count: usize,
        cell_w: u32,
        cell_h: u32,
        columns_requested: u32,
        gap: u32,
        margin: u32,
    ) -> Result<Self, LumenError> {
        if columns_requested == 0 || columns_requested > SHEET_COLUMNS_MAX {
            return Err(LumenError::BadParam(format!(
                "columns {columns_requested} outside 1..={SHEET_COLUMNS_MAX}"
            )));
        }
        let cells = u32::try_from(cell_count)
            .ok()
            .filter(|c| *c >= 1)
            .ok_or_else(|| LumenError::BadParam(format!("cell count {cell_count} invalid")))?;
        let columns = columns_requested.min(cells);
        let rows = cells.div_ceil(columns);
        let width = grid_extent(columns, cell_w, gap, margin);
        let height = grid_extent(rows, cell_h, gap, margin);
        let too_big = || {
            LumenError::BadParam(format!(
                "sheet would be {width}x{height} px ({columns} columns x {rows} rows); \
                 lower the cell count, columns, or padding"
            ))
        };
        let width32 = u32::try_from(width).map_err(|_| too_big())?;
        let height32 = u32::try_from(height).map_err(|_| too_big())?;
        check_dims(width32, height32).map_err(|_| too_big())?;
        Ok(Self {
            columns,
            rows,
            cell_w,
            cell_h,
            gap,
            margin,
            width: width32,
            height: height32,
        })
    }

    /// Top-left pixel of cell `slot` (row-major). `slot` must be inside the
    /// planned grid; the sheet size check makes the arithmetic overflow-free.
    pub(crate) fn origin(&self, slot: u32) -> (u32, u32) {
        assert!(
            slot < self.columns * self.rows,
            "grid slot {slot} out of range"
        );
        let (col, row) = (slot % self.columns, slot / self.columns);
        (
            self.margin + col * (self.cell_w + self.gap),
            self.margin + row * (self.cell_h + self.gap),
        )
    }

    /// A fresh fully transparent sheet of the planned size.
    pub(crate) fn blank(&self) -> RgbaImage {
        ImageBuffer::from_pixel(self.width, self.height, Rgba([0, 0, 0, 0]))
    }
}

/// `2*margin + count*cell + (count-1)*gap`, in u64 so it cannot overflow
/// for any u32 inputs (each term < 2^39).
fn grid_extent(count: u32, cell: u32, gap: u32, margin: u32) -> u64 {
    let count = u64::from(count);
    2 * u64::from(margin) + count * u64::from(cell) + count.saturating_sub(1) * u64::from(gap)
}

/// Composite each listed frame into its grid slot, in list order.
pub(crate) fn render_grid(
    doc: &SpriteDoc,
    grid: &Grid,
    frames: &[usize],
) -> Result<RgbaImage, LumenError> {
    let mut sheet = grid.blank();
    for (slot, &frame_idx) in (0u32..).zip(frames.iter()) {
        let image = composite_frame(doc, frame_idx)?;
        let (x, y) = grid.origin(slot);
        image::imageops::replace(&mut sheet, &image, i64::from(x), i64::from(y));
    }
    Ok(sheet)
}

/// Reject an output path that does not end with `suffix` (case-insensitive).
pub(crate) fn require_suffix(what: &str, path: &str, suffix: &str) -> Result<(), LumenError> {
    if path.to_ascii_lowercase().ends_with(suffix) {
        return Ok(());
    }
    Err(LumenError::BadParam(format!(
        "{what} {path:?} must end in {suffix}"
    )))
}

/// Find a tag by name (first match wins on duplicates). Unknown names list
/// the valid ones, capped at `TAG_NAMES_LISTED_MAX`.
pub(crate) fn find_tag<'a>(doc: &'a SpriteDoc, name: &str) -> Result<&'a Tag, LumenError> {
    if let Some(tag) = doc.tags.iter().find(|t| t.name == name) {
        return Ok(tag);
    }
    let valid: Vec<&str> = doc
        .tags
        .iter()
        .take(TAG_NAMES_LISTED_MAX)
        .map(|t| t.name.as_str())
        .collect();
    Err(LumenError::BadParam(format!(
        "no tag named {name:?}; valid tags: {valid:?}{}",
        if doc.tags.len() > TAG_NAMES_LISTED_MAX {
            " (truncated)"
        } else {
            ""
        }
    )))
}

/// Frame indices of a tag's inclusive range. `load_doc` already proved the
/// range lies inside the frame list.
pub(crate) fn tag_frames(tag: &Tag) -> Vec<usize> {
    (tag.from_frame..=tag.to_frame)
        .map(|f| f as usize)
        .collect()
}

/// Write a JSON sidecar to an already-resolved path, atomically.
pub(crate) fn write_json_sidecar<T: Serialize>(path: &Path, meta: &T) -> Result<(), LumenError> {
    let text = serde_json::to_string_pretty(meta)
        .map_err(|e| LumenError::Io(format!("metadata encode failed: {e}")))?;
    write_atomic(path, text.as_bytes(), "json")
}

/// Write bytes to an already-resolved path via temp file + rename. `ext` is
/// the path's own extension, kept in the temp name for readability.
fn write_atomic(path: &Path, bytes: &[u8], ext: &str) -> Result<(), LumenError> {
    let tmp = path.with_extension(format!("{ext}.tmp-{}", std::process::id()));
    if let Err(e) = std::fs::write(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, path)) {
        // Best effort: the temp file may not exist if the write itself failed.
        let _ = std::fs::remove_file(&tmp);
        return Err(LumenError::from(e));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tools
// ---------------------------------------------------------------------------

/// Render one frame of a document to a PNG.
///
/// Rejects: a non-`.png` output, an out-of-range frame (`BadParam`).
pub async fn export_sprite(
    root: &Path,
    req: ExportSpriteRequest,
) -> Result<ImageSaved, LumenError> {
    require_suffix("output", &req.output, ".png")?;
    let doc = load_doc(root, &req.doc)?;
    let frame = req.frame.unwrap_or(0);
    if frame >= doc.frames.len() {
        return Err(LumenError::BadParam(format!(
            "frame index {frame} out of range ({} frames)",
            doc.frames.len()
        )));
    }
    let image = composite_frame(&doc, frame)?;
    save_png(root, &image, &req.output)
}

/// Render frames into a grid sheet PNG plus a JSON metadata sidecar.
///
/// `tag: None` exports every frame; `Some(name)` exports that tag's range.
/// Rejects: columns outside 1..=64, padding outside 0..=64, unknown tag,
/// wrong output suffixes, and sheets wider or taller than 4096 px. The PNG
/// is written first; the sidecar only after the PNG succeeded.
pub async fn export_sheet(root: &Path, req: ExportSheetRequest) -> Result<ImageSaved, LumenError> {
    require_suffix("output", &req.output, ".png")?;
    require_suffix("meta_output", &req.meta_output, ".json")?;
    if req.padding > SHEET_PADDING_MAX_PX {
        return Err(LumenError::BadParam(format!(
            "padding {} outside 0..={SHEET_PADDING_MAX_PX}",
            req.padding
        )));
    }
    let doc = load_doc(root, &req.doc)?;
    let frames: Vec<usize> = match &req.tag {
        Some(name) => tag_frames(find_tag(&doc, name)?),
        None => (0..doc.frames.len()).collect(),
    };
    let grid = Grid::plan(
        frames.len(),
        doc.width,
        doc.height,
        req.columns,
        req.padding,
        0,
    )?;
    let meta_path = crate::resolve_write_path(&req.meta_output, root, &["json"])?;
    let mut cells = Vec::with_capacity(frames.len());
    for (slot, &frame) in (0u32..).zip(frames.iter()) {
        let (x, y) = grid.origin(slot);
        let tag = match &req.tag {
            Some(name) => Some(name.clone()),
            None => doc
                .tags
                .iter()
                .find(|t| t.from_frame as usize <= frame && frame <= t.to_frame as usize)
                .map(|t| t.name.clone()),
        };
        cells.push(SheetCell {
            frame,
            tag,
            x,
            y,
            duration_ms: doc.frames[frame].duration_ms,
        });
    }
    let sheet = render_grid(&doc, &grid, &frames)?;
    let saved = save_png(root, &sheet, &req.output)?;
    let meta = SheetMeta {
        image: req.output,
        cell_w: grid.cell_w,
        cell_h: grid.cell_h,
        columns: grid.columns,
        frames: cells,
    };
    write_json_sidecar(&meta_path, &meta)?;
    Ok(saved)
}

/// Render a tag's frames as a single-row (`horizontal`) or single-column
/// filmstrip PNG with no gaps.
///
/// Rejects: unknown tag, non-`.png` output, a strip longer than 4096 px.
pub async fn export_tag(root: &Path, req: ExportTagRequest) -> Result<ImageSaved, LumenError> {
    require_suffix("output", &req.output, ".png")?;
    let doc = load_doc(root, &req.doc)?;
    let frames = tag_frames(find_tag(&doc, &req.tag)?);
    // A vertical strip is one column; a horizontal strip is one row, which
    // `Grid::plan` gets by clamping SHEET_COLUMNS_MAX down to the count.
    // A longer tag would wrap into a second row, so it is rejected below.
    let columns = if req.horizontal { SHEET_COLUMNS_MAX } else { 1 };
    let grid = Grid::plan(frames.len(), doc.width, doc.height, columns, 0, 0)?;
    if req.horizontal && grid.rows != 1 {
        return Err(LumenError::BadParam(format!(
            "tag {:?} has {} frames; a horizontal strip holds at most {SHEET_COLUMNS_MAX}",
            req.tag,
            frames.len()
        )));
    }
    let strip = render_grid(&doc, &grid, &frames)?;
    save_png(root, &strip, &req.output)
}

/// Append a project PNG to the document as a new top layer.
///
/// The new layer is visible, opaque, `normal` blend, with an identity mod
/// on every frame. Rejects: undecodable PNGs (`BadPng`), size mismatch,
/// a full layer stack, empty/overlong/duplicate names (`BadParam`).
pub async fn import_layer(root: &Path, req: ImportLayerRequest) -> Result<DocSaved, LumenError> {
    let out = req.output.as_deref().unwrap_or(&req.doc);
    require_suffix("output", out, ".lumen.json")?;
    let name = req.name.as_deref().unwrap_or(IMPORT_LAYER_DEFAULT_NAME);
    if name.is_empty() || name.chars().count() > DOC_MAX_NAME_CHARS {
        return Err(LumenError::BadParam(format!(
            "layer name must be 1..={DOC_MAX_NAME_CHARS} characters"
        )));
    }
    let mut doc = load_doc(root, &req.doc)?;
    if doc.layers.iter().any(|l| l.name == name) {
        return Err(LumenError::BadParam(format!(
            "a layer named {name:?} already exists; pass a unique name"
        )));
    }
    if doc.layers.len() >= DOC_MAX_LAYERS {
        return Err(LumenError::BadParam(format!(
            "document already has the maximum {DOC_MAX_LAYERS} layers"
        )));
    }
    let image = load_png_rgba(root, &req.png)?;
    if image.width() != doc.width || image.height() != doc.height {
        return Err(LumenError::BadParam(format!(
            "png is {}x{} but the document is {}x{}",
            image.width(),
            image.height(),
            doc.width,
            doc.height
        )));
    }
    doc.layers.push(Layer {
        name: name.to_string(),
        visible: true,
        opacity: 1.0,
        blend: BlendMode::Normal,
        image,
    });
    sync_frame_mods(&mut doc);
    let path = save_doc(root, &doc, out)?;
    Ok(DocSaved::of(&path, &doc))
}

/// Export frames as a TIC-80 `.tic` cart plus a JSON sidecar.
///
/// The cart holds a TILES and a SPRITES chunk (4bpp 8x8 tiles, 16 tiles
/// wide), a 16-color PALETTE chunk, and a small Lua CODE chunk that loops
/// the frames with their durations. Each frame occupies a block of
/// `width/8` x `height/8` tiles; blocks pack row-major into the 128x256
/// sheet. Colors are never computed here: pixels must already fit 16 slots
/// (see `index_frames`). Returns the cart path and the frame size.
///
/// Rejects (`BadParam`, before any write): frame edges not a multiple of 8,
/// wider than 128 or taller than 256 px, more frames than fit the sheet,
/// palette overflow, partial alpha, all-transparent frames, wrong suffixes.
pub async fn export_tic80(root: &Path, req: ExportTic80Request) -> Result<ImageSaved, LumenError> {
    require_suffix("output", &req.output, ".tic")?;
    require_suffix("meta_output", &req.meta_output, ".json")?;
    let doc = load_doc(root, &req.doc)?;
    let frames = select_frames(&doc, req.tag.as_deref())?;
    let layout = Tic80Layout::plan(doc.width, doc.height, frames.len())?;
    let cart_path = crate::resolve_write_path(&req.output, root, &["tic"])?;
    let meta_path = crate::resolve_write_path(&req.meta_output, root, &["json"])?;
    let indexed = index_frames(&doc, &frames, TIC80_PALETTE_SLOTS, "TIC-80")?;
    let cells: Vec<Tic80Cell> = (0u32..)
        .zip(frames.iter())
        .map(|(slot, &frame)| Tic80Cell {
            frame,
            sprite_id: layout.sprite_id(slot),
            duration_ms: doc.frames[frame].duration_ms,
        })
        .collect();
    let sheet = encode_tic80_sheet(&indexed, &layout, doc.width);
    let code = tic80_lua(&cells, &layout, indexed.transparent, doc.width, doc.height);
    let mut cart = Vec::with_capacity(2 * TIC80_BANK_BYTES + code.len() + 64);
    tic80_chunk(&mut cart, TIC80_CHUNK_TILES, &sheet[..TIC80_BANK_BYTES]);
    tic80_chunk(&mut cart, TIC80_CHUNK_SPRITES, &sheet[TIC80_BANK_BYTES..]);
    tic80_chunk(
        &mut cart,
        TIC80_CHUNK_PALETTE,
        &tic80_palette(&indexed.palette),
    );
    tic80_chunk(&mut cart, TIC80_CHUNK_CODE, code.as_bytes());
    write_atomic(&cart_path, &cart, "tic")?;
    let meta = Tic80Meta {
        cart: req.output,
        palette: indexed.palette.iter().map(|&c| hex_rgb(c)).collect(),
        transparent_index: indexed.transparent.then_some(0),
        tiles_w: layout.tiles_w,
        tiles_h: layout.tiles_h,
        frames: cells,
    };
    write_json_sidecar(&meta_path, &meta)?;
    Ok(ImageSaved {
        path: cart_path.display().to_string(),
        width: doc.width,
        height: doc.height,
    })
}

/// Export frames as a WASM-4 sprite in Rust source form plus a JSON sidecar.
///
/// This is an ASSET SOURCE file, not a bootable cart: it defines
/// `NAME: [u8; N]` (frames stacked vertically, bit-packed MSB-first with no
/// row padding, as `blit`/`blit_sub` read them) and `NAME_WIDTH`,
/// `NAME_HEIGHT` (one frame), `NAME_FRAMES`, `NAME_FLAGS`, `NAME_PALETTE`,
/// `NAME_DRAW_COLORS` for a game to include. Colors are never computed
/// here: pixels must already fit `2^bpp` slots (see `index_frames`).
///
/// Rejects (`BadParam`, before any write): `bpp` not 1 or 2, an invalid
/// name, frame edges over 160 px, a blob over 32 KiB, palette overflow,
/// partial alpha, all-transparent frames, wrong suffixes.
pub async fn export_wasm4(root: &Path, req: ExportWasm4Request) -> Result<ImageSaved, LumenError> {
    require_suffix("output", &req.output, ".rs")?;
    require_suffix("meta_output", &req.meta_output, ".json")?;
    if req.bpp != 1 && req.bpp != 2 {
        return Err(LumenError::BadParam(format!(
            "bpp {} unsupported; WASM-4 sprites are 1 or 2 bpp",
            req.bpp
        )));
    }
    let name = req.name.as_deref().unwrap_or(WASM4_NAME_DEFAULT);
    check_rust_const_name(name)?;
    let doc = load_doc(root, &req.doc)?;
    let frames = select_frames(&doc, req.tag.as_deref())?;
    check_wasm4_size(doc.width, doc.height, frames.len(), req.bpp)?;
    let src_path = crate::resolve_write_path(&req.output, root, &["rs"])?;
    let meta_path = crate::resolve_write_path(&req.meta_output, root, &["json"])?;
    let indexed = index_frames(&doc, &frames, 1usize << req.bpp, "WASM-4")?;
    let blob = encode_wasm4(&indexed, req.bpp);
    let mut palette = [[0u8; 3]; 4];
    palette[..indexed.palette.len()].copy_from_slice(&indexed.palette);
    let meta = Wasm4Meta {
        source: req.output,
        name: name.to_string(),
        bpp: req.bpp,
        flags: if req.bpp == 2 { WASM4_BLIT_2BPP } else { 0 },
        width: doc.width,
        height: doc.height,
        bytes: blob.len(),
        palette: palette.iter().map(|&c| hex_rgb(c)).collect(),
        draw_colors: wasm4_draw_colors(req.bpp, indexed.transparent),
        transparent_index: indexed.transparent.then_some(0),
        frames: (0u32..)
            .zip(frames.iter())
            .map(|(slot, &frame)| Wasm4Cell {
                frame,
                src_y: slot * doc.height,
                duration_ms: doc.frames[frame].duration_ms,
            })
            .collect(),
    };
    let source = wasm4_rust_source(&meta, &palette, &blob);
    write_atomic(&src_path, source.as_bytes(), "rs")?;
    write_json_sidecar(&meta_path, &meta)?;
    Ok(ImageSaved {
        path: src_path.display().to_string(),
        width: doc.width,
        height: doc.height,
    })
}

// ---------------------------------------------------------------------------
// Console backend machinery
// ---------------------------------------------------------------------------

/// Every frame (`None`) or one tag's range.
fn select_frames(doc: &SpriteDoc, tag: Option<&str>) -> Result<Vec<usize>, LumenError> {
    match tag {
        Some(name) => Ok(tag_frames(find_tag(doc, name)?)),
        None => Ok((0..doc.frames.len()).collect()),
    }
}

/// Composited frames reduced to console palette indices.
struct IndexedFrames {
    /// RGB per palette slot. Slot 0 is the (black) transparent slot when
    /// `transparent`; color slots follow.
    palette: Vec<[u8; 3]>,
    transparent: bool,
    /// One row-major `width*height` index buffer per frame.
    frames: Vec<Vec<u8>>,
}

/// Composite `frames` and map every pixel to a palette slot, by exact RGB
/// match only. No color math happens here: reducing colors is the job of
/// the `quantize` / `palette_apply` / `fidelity_set` tools, so every
/// backend inherits one quantizer instead of growing its own.
///
/// Palette: the document's bound palette (deduplicated, in order) when set,
/// else the distinct opaque colors in first-seen row-major order. Alpha 0
/// maps to slot 0, which is then reserved. Rejects partial alpha (neither
/// console blends), colors outside a bound palette, more than `slots_max`
/// slots, and output with no opaque pixel.
fn index_frames(
    doc: &SpriteDoc,
    frames: &[usize],
    slots_max: usize,
    console: &str,
) -> Result<IndexedFrames, LumenError> {
    assert!(
        (2..=TIC80_PALETTE_SLOTS).contains(&slots_max),
        "slot count is a console constant"
    );
    let mut bound: Vec<[u8; 3]> = Vec::new();
    for c in &doc.palette {
        if !bound.contains(&[c[0], c[1], c[2]]) {
            bound.push([c[0], c[1], c[2]]);
        }
    }
    let images = frames
        .iter()
        .map(|&f| composite_frame(doc, f))
        .collect::<Result<Vec<_>, _>>()?;
    let (used, transparent) = collect_colors(&images, frames, &bound, slots_max, console)?;
    if used.is_empty() {
        return Err(LumenError::BadParam(
            "every exported frame is fully transparent; nothing to export".to_string(),
        ));
    }
    let colors = if bound.is_empty() { used } else { bound };
    let reserved = usize::from(transparent);
    if colors.len() + reserved > slots_max {
        return Err(LumenError::BadParam(format!(
            "{console} has {slots_max} palette slots but the export needs {} colors{}; \
             reduce colors with quantize or palette_apply first",
            colors.len(),
            if transparent {
                " + 1 transparent slot"
            } else {
                ""
            }
        )));
    }
    let mut palette = Vec::with_capacity(colors.len() + reserved);
    if transparent {
        palette.push([0, 0, 0]);
    }
    palette.extend_from_slice(&colors);
    let indexed = images
        .iter()
        .map(|img| {
            img.pixels()
                .map(|px| {
                    if px[3] == 0 {
                        return 0;
                    }
                    let slot = colors.iter().position(|c| *c == [px[0], px[1], px[2]]);
                    assert!(slot.is_some(), "collect_colors admitted every opaque color");
                    // Bounded by `slots_max` <= 16, so the cast is exact.
                    (slot.unwrap_or(0) + reserved) as u8
                })
                .collect()
        })
        .collect();
    Ok(IndexedFrames {
        palette,
        transparent,
        frames: indexed,
    })
}

/// First pass of `index_frames`: distinct opaque colors (bounded by
/// `slots_max`, failing fast past it) and whether any pixel is transparent.
fn collect_colors(
    images: &[RgbaImage],
    frames: &[usize],
    bound: &[[u8; 3]],
    slots_max: usize,
    console: &str,
) -> Result<(Vec<[u8; 3]>, bool), LumenError> {
    let mut used: Vec<[u8; 3]> = Vec::new();
    let mut transparent = false;
    for (img, &frame) in images.iter().zip(frames) {
        for (x, y, px) in img.enumerate_pixels() {
            match px[3] {
                0 => transparent = true,
                255 => {}
                alpha => {
                    return Err(LumenError::BadParam(format!(
                        "frame {frame} pixel ({x},{y}) has alpha {alpha}; {console} has only \
                         on/off transparency, so alpha must be 0 or 255"
                    )));
                }
            }
            let rgb = [px[0], px[1], px[2]];
            if px[3] == 0 || used.contains(&rgb) {
                continue;
            }
            if !bound.is_empty() && !bound.contains(&rgb) {
                return Err(LumenError::BadParam(format!(
                    "frame {frame} pixel ({x},{y}) color {} is not in the document's bound \
                     palette; run palette_apply with that palette first",
                    hex_rgb(rgb)
                )));
            }
            used.push(rgb);
            if used.len() > slots_max {
                return Err(LumenError::BadParam(format!(
                    "{console} has {slots_max} palette slots but the frames use more than \
                     {slots_max} colors; run quantize (max_colors <= {}) first",
                    slots_max - 1
                )));
            }
        }
    }
    Ok((used, transparent))
}

fn hex_rgb(c: [u8; 3]) -> String {
    format!("#{:02X}{:02X}{:02X}", c[0], c[1], c[2])
}

/// Where frames go in the TIC-80 sheet: blocks of `tiles_w` x `tiles_h`
/// tiles, `per_row` blocks across. Construction proves every block fits.
#[derive(Debug, Clone, Copy)]
struct Tic80Layout {
    tiles_w: u32,
    tiles_h: u32,
    per_row: u32,
}

impl Tic80Layout {
    fn plan(width: u32, height: u32, frame_count: usize) -> Result<Self, LumenError> {
        let max_w = TIC80_SHEET_TILES_W * TIC80_TILE_PX;
        let max_h = TIC80_SHEET_TILES_H * TIC80_TILE_PX;
        if !width.is_multiple_of(TIC80_TILE_PX) || !height.is_multiple_of(TIC80_TILE_PX) {
            return Err(LumenError::BadParam(format!(
                "frame is {width}x{height} px; TIC-80 tiles need both edges to be multiples of 8"
            )));
        }
        if width > max_w || height > max_h {
            return Err(LumenError::BadParam(format!(
                "frame is {width}x{height} px; the TIC-80 sprite sheet is {max_w}x{max_h}"
            )));
        }
        let (tiles_w, tiles_h) = (width / TIC80_TILE_PX, height / TIC80_TILE_PX);
        let per_row = TIC80_SHEET_TILES_W / tiles_w;
        let capacity = per_row * (TIC80_SHEET_TILES_H / tiles_h);
        if frame_count == 0 || frame_count > capacity as usize {
            return Err(LumenError::BadParam(format!(
                "{frame_count} frames of {width}x{height} px; the TIC-80 sheet holds 1..={capacity}"
            )));
        }
        Ok(Self {
            tiles_w,
            tiles_h,
            per_row,
        })
    }

    /// Top-left sprite id of frame block `slot` (row-major, 16 ids per row).
    fn sprite_id(&self, slot: u32) -> u32 {
        let (col, row) = (slot % self.per_row, slot / self.per_row);
        let id = row * self.tiles_h * TIC80_SHEET_TILES_W + col * self.tiles_w;
        assert!(
            id < TIC80_SHEET_TILES_W * TIC80_SHEET_TILES_H,
            "plan bounds the slot"
        );
        id
    }
}

/// Pack indexed frames into the 512-tile sheet: 32 bytes per tile, rows
/// top to bottom, two pixels per byte with the LEFT pixel in the LOW nibble.
fn encode_tic80_sheet(indexed: &IndexedFrames, layout: &Tic80Layout, width: u32) -> Vec<u8> {
    let mut sheet = vec![0u8; 2 * TIC80_BANK_BYTES];
    for (slot, pixels) in (0u32..).zip(&indexed.frames) {
        let base = layout.sprite_id(slot);
        for (i, &idx) in (0u32..).zip(pixels) {
            let (x, y) = (i % width, i / width);
            let tile = base + (y / TIC80_TILE_PX) * TIC80_SHEET_TILES_W + x / TIC80_TILE_PX;
            let in_tile = (y % TIC80_TILE_PX) * TIC80_TILE_PX + x % TIC80_TILE_PX;
            let byte = tile as usize * TIC80_TILE_BYTES + (in_tile / 2) as usize;
            sheet[byte] |= if x % 2 == 0 { idx } else { idx << 4 };
        }
    }
    sheet
}

/// 16 RGB triplets (48 bytes); unused slots stay black.
fn tic80_palette(palette: &[[u8; 3]]) -> Vec<u8> {
    assert!(
        palette.len() <= TIC80_PALETTE_SLOTS,
        "index_frames bounds the palette"
    );
    let mut slots = [[0u8; 3]; TIC80_PALETTE_SLOTS];
    slots[..palette.len()].copy_from_slice(palette);
    slots.as_flattened().to_vec()
}

/// Append one chunk: byte 0 = type (low 5 bits) | bank 0 (high 3 bits),
/// bytes 1..3 = data size (u16 little-endian), byte 3 reserved. A size of 0
/// means 64 KiB to the TIC-80 loader, so empty chunks are never written.
fn tic80_chunk(cart: &mut Vec<u8>, chunk_type: u8, data: &[u8]) {
    assert!(chunk_type < 32, "chunk type fits 5 bits");
    assert!(
        !data.is_empty() && data.len() <= usize::from(u16::MAX),
        "chunk {chunk_type} size {} is bounded by its caller",
        data.len()
    );
    let size = data.len() as u16;
    cart.push(chunk_type);
    cart.extend_from_slice(&size.to_le_bytes());
    cart.push(0);
    cart.extend_from_slice(data);
}

/// A minimal Lua program that loops the frames at their durations
/// (TIC() runs at 60 Hz). Only numbers are interpolated, never user text.
fn tic80_lua(
    cells: &[Tic80Cell],
    layout: &Tic80Layout,
    transparent: bool,
    w: u32,
    h: u32,
) -> String {
    let ids: Vec<String> = cells.iter().map(|c| c.sprite_id.to_string()).collect();
    let durations: Vec<String> = cells.iter().map(|c| c.duration_ms.to_string()).collect();
    let colorkey = if transparent { 0 } else { -1 };
    let x = TIC80_SCREEN_W_PX.saturating_sub(w) / 2;
    let y = TIC80_SCREEN_H_PX.saturating_sub(h) / 2;
    format!(
        "-- title:  lumen export\n-- script: lua\n\
         -- {count} frames of {w}x{h} px; sprite ids in F, durations (ms) in D.\n\
         local F={{{ids}}}\nlocal D={{{durs}}}\nlocal i,t=1,0\n\
         function TIC()\n cls(0)\n spr(F[i],{x},{y},{colorkey},1,0,0,{tw},{th})\n \
         t=t+1000/60\n if t>=D[i] then t=t-D[i] i=i%#F+1 end\nend\n",
        count = cells.len(),
        ids = ids.join(","),
        durs = durations.join(","),
        tw = layout.tiles_w,
        th = layout.tiles_h,
    )
}

/// Reject frames larger than the screen or a blob over the byte budget,
/// before compositing. All arithmetic is u64 on bounded inputs.
fn check_wasm4_size(
    width: u32,
    height: u32,
    frame_count: usize,
    bpp: u8,
) -> Result<(), LumenError> {
    if width > WASM4_SCREEN_PX || height > WASM4_SCREEN_PX {
        return Err(LumenError::BadParam(format!(
            "frame is {width}x{height} px; WASM-4 frames must fit the \
             {WASM4_SCREEN_PX}x{WASM4_SCREEN_PX} screen"
        )));
    }
    let bits = u64::from(width) * u64::from(height) * frame_count as u64 * u64::from(bpp);
    let bytes = bits.div_ceil(8);
    if frame_count == 0 || bytes > WASM4_SPRITE_BYTES_MAX {
        return Err(LumenError::BadParam(format!(
            "{frame_count} frames would be {bytes} bytes; WASM-4 export holds \
             1..={WASM4_SPRITE_BYTES_MAX} bytes"
        )));
    }
    Ok(())
}

/// `[A-Z][A-Z0-9_]*`, 1..=32 chars: the name is pasted into Rust source,
/// so anything else (quotes, newlines, `;`) is rejected outright.
fn check_rust_const_name(name: &str) -> Result<(), LumenError> {
    let mut chars = name.chars();
    let valid = name.len() <= WASM4_NAME_CHARS_MAX
        && chars.next().is_some_and(|c| c.is_ascii_uppercase())
        && chars.all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
    if valid {
        return Ok(());
    }
    Err(LumenError::BadParam(format!(
        "name {:?} must match [A-Z][A-Z0-9_]* and be at most {WASM4_NAME_CHARS_MAX} chars",
        name.chars().take(WASM4_NAME_CHARS_MAX).collect::<String>()
    )))
}

/// Bit-pack every frame back to back, `bpp` bits per pixel, the first pixel
/// in the most significant bits of each byte, no row padding.
fn encode_wasm4(indexed: &IndexedFrames, bpp: u8) -> Vec<u8> {
    let bpp = usize::from(bpp);
    let pixel_count: usize = indexed.frames.iter().map(Vec::len).sum();
    let mut blob = vec![0u8; (pixel_count * bpp).div_ceil(8)];
    for (i, &idx) in indexed.frames.iter().flatten().enumerate() {
        let bit = i * bpp;
        assert!(
            usize::from(idx) < (1 << bpp),
            "index_frames bounds the index"
        );
        blob[bit / 8] |= idx << (8 - bpp - bit % 8);
    }
    blob
}

/// `DRAW_COLORS` nibble `v` (bits 4v..4v+4) picks the palette color for
/// sprite value `v`: 0 = transparent, `v + 1` = `PALETTE[v]`.
fn wasm4_draw_colors(bpp: u8, transparent: bool) -> u16 {
    (0..(1u16 << bpp))
        .map(|v| {
            if transparent && v == 0 {
                0
            } else {
                (v + 1) << (4 * v)
            }
        })
        .sum()
}

fn wasm4_rust_source(meta: &Wasm4Meta, palette: &[[u8; 3]; 4], blob: &[u8]) -> String {
    let name = &meta.name;
    let palette: Vec<String> = palette
        .iter()
        .map(|c| format!("0x{:02x}{:02x}{:02x}", c[0], c[1], c[2]))
        .collect();
    let mut src = format!(
        "// Generated by lumen: WASM-4 sprite ASSET SOURCE (not a cart).\n\
         // {frames} frames of {w}x{h} px stacked vertically; draw frame i with\n\
         // blit_sub(&{name}, x, y, {name}_WIDTH, {name}_HEIGHT, 0, i * {name}_HEIGHT,\n\
         //          {name}_WIDTH, {name}_FLAGS) after setting PALETTE and DRAW_COLORS.\n\
         pub const {name}_WIDTH: u32 = {w};\n\
         pub const {name}_HEIGHT: u32 = {h};\n\
         pub const {name}_FRAMES: u32 = {frames};\n\
         pub const {name}_FLAGS: u32 = {flags}; // BLIT_{bpp}BPP\n\
         pub const {name}_PALETTE: [u32; 4] = [{pal}];\n\
         pub const {name}_DRAW_COLORS: u16 = 0x{dc:04x};\n\
         pub const {name}: [u8; {len}] = [\n",
        frames = meta.frames.len(),
        w = meta.width,
        h = meta.height,
        flags = meta.flags,
        bpp = meta.bpp,
        pal = palette.join(", "),
        dc = meta.draw_colors,
        len = blob.len(),
    );
    for line in blob.chunks(WASM4_BYTES_PER_LINE) {
        let bytes: Vec<String> = line.iter().map(|b| format!("0x{b:02x}")).collect();
        src.push_str(&format!("    {},\n", bytes.join(", ")));
    }
    src.push_str("];\n");
    src
}
/// Request for `export_sheet_paperzd`: grid sheet PNG plus TexturePacker-style
/// JSON metadata for PaperZD / Paper 2D import.
///
/// Same grid layout as `export_sheet`; only the sidecar format differs.
/// Frame names are `{tag}_{index}` when a tag covers the frame, else
/// `frame_{index}`. Rejects the same inputs as `export_sheet`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ExportSheetPaperZdRequest {
    /// Project-relative `.lumen.json` document to render.
    pub doc: String,
    /// Project-relative `.png` sheet output path (required).
    pub output: String,
    /// Project-relative `.json` TexturePacker metadata path (required).
    pub meta_output: String,
    /// Grid columns, 1..=64. Clamped down to the exported frame count.
    pub columns: u32,
    /// Export only this tag's frame range. `None` exports every frame.
    pub tag: Option<String>,
    /// Transparent gap between cells, 0..=64 px. No outer border.
    pub padding: u32,
}

/// One frame entry in TexturePacker JSON format, as PaperZD expects.
#[derive(Debug, Clone, Serialize)]
pub struct PaperZdFrame {
    pub frame: PaperZdRect,
    pub rotated: bool,
    pub trimmed: bool,
    #[serde(rename = "spriteSourceSize")]
    pub sprite_source_size: PaperZdRect,
    #[serde(rename = "sourceSize")]
    pub source_size: PaperZdSize,
}

/// Rectangle `{x, y, w, h}` in TexturePacker JSON.
#[derive(Debug, Clone, Serialize)]
pub struct PaperZdRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// Size `{w, h}` in TexturePacker JSON.
#[derive(Debug, Clone, Serialize)]
pub struct PaperZdSize {
    pub w: u32,
    pub h: u32,
}

/// Top-level TexturePacker JSON document for PaperZD import.
///
/// `frames` is a `BTreeMap` so serialization is deterministic (sorted by
/// name); PaperZD reads the `frame` rect and `meta.image`/`meta.size`.
#[derive(Debug, Clone, Serialize)]
pub struct PaperZdSheetMeta {
    pub frames: std::collections::BTreeMap<String, PaperZdFrame>,
    pub meta: PaperZdMetaInner,
}

/// The `meta` section of TexturePacker JSON.
#[derive(Debug, Clone, Serialize)]
pub struct PaperZdMetaInner {
    pub app: String,
    pub version: String,
    pub image: String,
    pub format: String,
    pub size: PaperZdSize,
    pub scale: String,
}

/// Render frames into a grid sheet PNG plus a TexturePacker-style JSON
/// sidecar for PaperZD / Paper 2D import.
///
/// Contract: identical grid layout and validation to `export_sheet`; the
/// sidecar uses TexturePacker field names (`frame`, `rotated`, `trimmed`,
/// `spriteSourceSize`, `sourceSize`, `meta.image`, `meta.size`) so Unreal's
/// PaperZD importer reads frame rects without manual slicing. Frames are
/// never rotated or trimmed; pivots default to center (PaperZD convention).
/// The PNG is written first; the sidecar only after the PNG succeeded.
pub async fn export_sheet_paperzd(
    root: &Path,
    req: ExportSheetPaperZdRequest,
) -> Result<ImageSaved, LumenError> {
    require_suffix("output", &req.output, ".png")?;
    require_suffix("meta_output", &req.meta_output, ".json")?;
    if req.padding > SHEET_PADDING_MAX_PX {
        return Err(LumenError::BadParam(format!(
            "padding {} outside 0..={SHEET_PADDING_MAX_PX}",
            req.padding
        )));
    }
    let doc = load_doc(root, &req.doc)?;
    let frames: Vec<usize> = match &req.tag {
        Some(name) => tag_frames(find_tag(&doc, name)?),
        None => (0..doc.frames.len()).collect(),
    };
    let grid = Grid::plan(
        frames.len(),
        doc.width,
        doc.height,
        req.columns,
        req.padding,
        0,
    )?;
    let meta_path = crate::resolve_write_path(&req.meta_output, root, &["json"])?;

    let mut paper_frames = std::collections::BTreeMap::new();
    for (slot, &frame) in (0u32..).zip(frames.iter()) {
        let (x, y) = grid.origin(slot);
        let tag_name = match &req.tag {
            Some(name) => Some(name.clone()),
            None => doc
                .tags
                .iter()
                .find(|t| t.from_frame as usize <= frame && frame <= t.to_frame as usize)
                .map(|t| t.name.clone()),
        };
        let frame_name = match tag_name {
            Some(tag) => format!("{tag}_{slot}"),
            None => format!("frame_{slot}"),
        };
        let rect = PaperZdRect {
            x,
            y,
            w: doc.width,
            h: doc.height,
        };
        paper_frames.insert(
            frame_name,
            PaperZdFrame {
                frame: PaperZdRect { x, y, w: doc.width, h: doc.height },
                rotated: false,
                trimmed: false,
                sprite_source_size: rect,
                source_size: PaperZdSize {
                    w: doc.width,
                    h: doc.height,
                },
            },
        );
    }

    let sheet = render_grid(&doc, &grid, &frames)?;
    let saved = save_png(root, &sheet, &req.output)?;
    let image_filename = Path::new(&req.output)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or(&req.output)
        .to_string();
    let meta = PaperZdSheetMeta {
        frames: paper_frames,
        meta: PaperZdMetaInner {
            app: "lumen".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            image: image_filename,
            format: "RGBA8888".to_string(),
            size: PaperZdSize {
                w: sheet.width(),
                h: sheet.height(),
            },
            scale: "1".to_string(),
        },
    };
    write_json_sidecar(&meta_path, &meta)?;
    Ok(saved)
}
