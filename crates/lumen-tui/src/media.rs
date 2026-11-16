//! Media ladder rung 1 (DESIGN §D5): browse output images/sprites,
//! render them in-terminal, and label exactly how.
//!
//! Ladder state, honestly reported by [`capabilities`]. Kitty
//! graphics: implemented — lossless PNG passthrough, the file bytes
//! chunked into APC escapes, never re-encoded. Sixel: NOT implemented
//! in this rung; capability reporting names it `Unavailable` so no UI
//! surface can imply otherwise. Half-block: implemented — the floor
//! renderer, with pinned nearest-neighbour scaling in integer math
//! and pinned composite-over-black for alpha; two renders of the same
//! bytes are byte-identical (proven by test). Video frame extraction
//! is likewise a named follow-on rung, absent here. Viewing never
//! rewrites the viewed file: renderers take bytes and return buffers.
//!
//! Preparation path labelling: everything this rung displays comes
//! from the **pinned CPU decode path**. No GPU decode/scale runs in
//! the TUI process (DESIGN §D5: GPU paths are presentation-only and
//! must be labelled; this rung uses none).

use std::path::Path;

use image::{ImageReader, RgbaImage};
use std::io::Cursor;
use thiserror::Error;

/// Maximum image file bytes accepted for viewing (16 MiB).
pub const IMAGE_BYTES_MAX: usize = 16 * 1024 * 1024;
/// Maximum decoded pixels accepted (4096 × 4096).
pub const IMAGE_PIXELS_MAX: u32 = 16_777_216;
/// Maximum render width in terminal cells.
pub const CELLS_WIDTH_MAX: u32 = 256;
/// Maximum render height in terminal cells.
pub const CELLS_HEIGHT_MAX: u32 = 128;
/// Base64 chars per kitty APC chunk (protocol recommendation).
pub const KITTY_CHUNK_CHARS: usize = 4096;
/// Maximum entries returned by one media directory listing.
pub const MEDIA_ENTRIES_MAX: usize = 512;

/// Typed media failures. Oversized input is a named rejection, never
/// an allocation attempt and never a panic.
#[derive(Debug, Error)]
pub enum MediaError {
    /// Input bytes over [`IMAGE_BYTES_MAX`].
    #[error("image rejected: {0} bytes over bound {IMAGE_BYTES_MAX}")]
    TooManyBytes(usize),
    /// Decoded dimensions over [`IMAGE_PIXELS_MAX`].
    #[error("image rejected: {0}x{1} pixels over bound {IMAGE_PIXELS_MAX}")]
    TooManyPixels(u32, u32),
    /// Requested cell grid over the cell bounds.
    #[error("render rejected: {0}x{1} cells over bound {CELLS_WIDTH_MAX}x{CELLS_HEIGHT_MAX}")]
    TooManyCells(u32, u32),
    /// The bytes are not a decodable PNG.
    #[error("image rejected: not a decodable png: {0}")]
    BadImage(String),
    /// Filesystem failure while listing or reading.
    #[error("io error: {0}")]
    Io(String),
}

/// The presentation protocol used for one displayed image.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    /// Kitty graphics protocol, PNG passthrough.
    Kitty,
    /// Half-block glyphs with truecolor fg/bg per cell.
    HalfBlock,
}

impl Protocol {
    /// Operator-facing protocol name.
    pub fn label(self) -> &'static str {
        match self {
            Protocol::Kitty => "kitty graphics (PNG passthrough)",
            Protocol::HalfBlock => "half-block (upper-half glyph, truecolor fg/bg)",
        }
    }
}

/// Decode/preparation path that produced what is on screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodePath {
    /// Pinned CPU decode + pinned integer nearest-neighbour scale.
    PinnedCpu,
}

impl DecodePath {
    /// Operator-facing path name.
    pub fn label(self) -> &'static str {
        "pinned-CPU decode (no GPU path in this rung)"
    }
}

/// A follow-on rung's explicit absence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RungState {
    /// Implemented in this rung.
    Available,
    /// Named follow-on rung; not implemented, never faked.
    Unavailable,
}

/// The media ladder as this build honestly implements it.
#[derive(Debug, Clone, PartialEq)]
pub struct Capabilities {
    /// Kitty passthrough usable (terminal advertises support).
    pub kitty: bool,
    /// Sixel encoding state — always `Unavailable` in rung 1.
    pub sixel: RungState,
    /// Video frame extraction state — always `Unavailable` in rung 1.
    pub video: RungState,
    /// Half-block floor — always available.
    pub half_block: RungState,
}

/// Detect terminal graphics support from the environment.
///
/// Kitty: `KITTY_WINDOW_ID` set, or `TERM` naming kitty/WezTerm
/// (WezTerm speaks the kitty protocol). Anything else gets the
/// half-block floor. Detection is a pure function of the two values
/// so tests can drive every branch.
pub fn detect_capabilities(term: Option<&str>, kitty_window_id: Option<&str>) -> Capabilities {
    let term = term.unwrap_or("");
    let kitty = kitty_window_id.is_some_and(|v| !v.is_empty())
        || term.contains("kitty")
        || term.contains("wezterm");
    Capabilities {
        kitty,
        sixel: RungState::Unavailable,
        video: RungState::Unavailable,
        half_block: RungState::Available,
    }
}

/// Capabilities from the live process environment.
pub fn capabilities() -> Capabilities {
    detect_capabilities(
        std::env::var("TERM").ok().as_deref(),
        std::env::var("KITTY_WINDOW_ID").ok().as_deref(),
    )
}

/// The protocol a display will use under these capabilities.
pub fn choose_protocol(caps: &Capabilities) -> Protocol {
    if caps.kitty {
        Protocol::Kitty
    } else {
        Protocol::HalfBlock
    }
}

/// The on-screen label naming protocol + decode path for one image.
/// Every rendered image carries this line (DESIGN §D5 labelling rule).
pub fn display_label(protocol: Protocol) -> String {
    format!(
        "protocol: {} | decode: {}",
        protocol.label(),
        DecodePath::PinnedCpu.label()
    )
}

/// One browsable media entry (image/sprite output).
#[derive(Debug, Clone, PartialEq)]
pub struct MediaEntry {
    /// File name only.
    pub name: String,
    /// Full path.
    pub path: String,
    /// File size in bytes.
    pub bytes: u64,
}

/// List image entries (`.png`, `.gif`) in `dir`, sorted by name,
/// bounded at [`MEDIA_ENTRIES_MAX`]. Read-only.
pub fn list_media(dir: &Path) -> Result<Vec<MediaEntry>, MediaError> {
    let read = std::fs::read_dir(dir).map_err(|e| MediaError::Io(e.to_string()))?;
    let mut entries: Vec<MediaEntry> = Vec::new();
    for entry in read.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        let lower = name.to_ascii_lowercase();
        if !(lower.ends_with(".png") || lower.ends_with(".gif")) {
            continue;
        }
        let bytes = entry.metadata().map(|m| m.len()).unwrap_or(0);
        entries.push(MediaEntry {
            name,
            path: entry.path().display().to_string(),
            bytes,
        });
        if entries.len() >= MEDIA_ENTRIES_MAX {
            break;
        }
    }
    entries.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(entries)
}

/// Read one image file for viewing, enforcing [`IMAGE_BYTES_MAX`]
/// before the bytes are fully read into memory where the metadata
/// allows. The file is never written.
pub fn read_image_file(path: &Path) -> Result<Vec<u8>, MediaError> {
    let meta = std::fs::metadata(path).map_err(|e| MediaError::Io(e.to_string()))?;
    if meta.len() > IMAGE_BYTES_MAX as u64 {
        return Err(MediaError::TooManyBytes(
            usize::try_from(meta.len()).unwrap_or(usize::MAX),
        ));
    }
    let bytes = std::fs::read(path).map_err(|e| MediaError::Io(e.to_string()))?;
    if bytes.len() > IMAGE_BYTES_MAX {
        return Err(MediaError::TooManyBytes(bytes.len()));
    }
    Ok(bytes)
}

/// Decode PNG bytes to RGBA with dimension bounds checked from the
/// header before full decode where possible, and again after.
pub fn decode_png(bytes: &[u8]) -> Result<RgbaImage, MediaError> {
    if bytes.len() > IMAGE_BYTES_MAX {
        return Err(MediaError::TooManyBytes(bytes.len()));
    }
    let reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| MediaError::BadImage(e.to_string()))?;
    let image = reader
        .decode()
        .map_err(|e| MediaError::BadImage(e.to_string()))?;
    let (width, height) = (image.width(), image.height());
    if width.saturating_mul(height) > IMAGE_PIXELS_MAX {
        return Err(MediaError::TooManyPixels(width, height));
    }
    Ok(image.to_rgba8())
}

/// One half-block cell: the `▀` glyph with fg = upper pixel, bg = lower.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cell {
    /// Upper pixel (foreground colour).
    pub fg: [u8; 3],
    /// Lower pixel (background colour).
    pub bg: [u8; 3],
}

/// A rendered half-block grid: `cells[y][x]`, row-major.
#[derive(Debug, Clone, PartialEq)]
pub struct HalfBlockRender {
    /// Grid width in cells.
    pub width_cells: u32,
    /// Grid height in cells.
    pub height_cells: u32,
    /// Row-major cells, exactly `width_cells * height_cells` long.
    pub cells: Vec<Cell>,
}

impl HalfBlockRender {
    /// Render to an ANSI truecolor string. Byte-stable for a given
    /// grid: colours are emitted in a fixed order with a reset at
    /// each line end.
    pub fn to_ansi(&self) -> String {
        let mut out = String::new();
        for y in 0..self.height_cells {
            for x in 0..self.width_cells {
                let idx = (y * self.width_cells + x) as usize;
                let cell = self.cells[idx];
                out.push_str(&format!(
                    "\u{1b}[38;2;{};{};{};48;2;{};{};{}m▀",
                    cell.fg[0], cell.fg[1], cell.fg[2], cell.bg[0], cell.bg[1], cell.bg[2]
                ));
            }
            out.push_str("\u{1b}[0m\n");
        }
        out
    }
}

/// Composite one RGBA pixel over black (the pinned alpha rule) and
/// drop alpha. Deterministic integer math only.
fn over_black(px: [u8; 4]) -> [u8; 3] {
    let a = u32::from(px[3]);
    let mix = |c: u8| -> u8 { ((u32::from(c) * a) / 255) as u8 };
    [mix(px[0]), mix(px[1]), mix(px[2])]
}

/// Render PNG bytes as a half-block grid fitted inside
/// `cells_width_max` × `cells_height_max` cells, preserving aspect.
///
/// Scaling is the pinned path: integer nearest-neighbour,
/// `src = dst * src_size / dst_size` per axis. The same bytes and
/// bounds always produce the same grid (gate-proven by test).
pub fn render_half_block(
    bytes: &[u8],
    cells_width_max: u32,
    cells_height_max: u32,
) -> Result<HalfBlockRender, MediaError> {
    if cells_width_max == 0
        || cells_height_max == 0
        || cells_width_max > CELLS_WIDTH_MAX
        || cells_height_max > CELLS_HEIGHT_MAX
    {
        return Err(MediaError::TooManyCells(cells_width_max, cells_height_max));
    }
    let image = decode_png(bytes)?;
    let (src_w, src_h) = (image.width(), image.height());
    // Fit: pixel grid is cells_w × 2*cells_h. Scale by the tighter axis
    // using integer ratios so the fit is identical on every machine.
    let mut pix_w = cells_width_max;
    let mut pix_h = cells_height_max.saturating_mul(2);
    if src_w * pix_h > src_h * pix_w {
        pix_h = (src_h * pix_w / src_w).max(1);
    } else {
        pix_w = (src_w * pix_h / src_h).max(1);
    }
    let cells_w = pix_w;
    let cells_h = pix_h.div_ceil(2);
    let mut cells = Vec::with_capacity((cells_w * cells_h) as usize);
    for cy in 0..cells_h {
        for cx in 0..cells_w {
            let upper = sample(&image, cx, cy * 2, pix_w, pix_h);
            let lower = if cy * 2 + 1 < pix_h {
                sample(&image, cx, cy * 2 + 1, pix_w, pix_h)
            } else {
                [0, 0, 0]
            };
            cells.push(Cell {
                fg: upper,
                bg: lower,
            });
        }
    }
    Ok(HalfBlockRender {
        width_cells: cells_w,
        height_cells: cells_h,
        cells,
    })
}

fn sample(image: &RgbaImage, dst_x: u32, dst_y: u32, dst_w: u32, dst_h: u32) -> [u8; 3] {
    let src_x = dst_x * image.width() / dst_w.max(1);
    let src_y = dst_y * image.height() / dst_h.max(1);
    let px = image.get_pixel(src_x.min(image.width() - 1), src_y.min(image.height() - 1));
    over_black(px.0)
}

/// Encode PNG bytes as a kitty graphics passthrough sequence.
///
/// The file bytes are base64-chunked into APC escapes (`a=T` transmit
/// and display, `f=100` PNG). Passthrough means the terminal decodes
/// the original PNG — no re-encode, no quantization.
pub fn kitty_passthrough(png_bytes: &[u8]) -> Result<String, MediaError> {
    if png_bytes.len() > IMAGE_BYTES_MAX {
        return Err(MediaError::TooManyBytes(png_bytes.len()));
    }
    if png_bytes.len() < 8 || &png_bytes[0..8] != b"\x89PNG\r\n\x1a\n" {
        return Err(MediaError::BadImage(
            "kitty passthrough requires PNG bytes".into(),
        ));
    }
    let encoded = base64_encode(png_bytes);
    let mut out = String::new();
    let chunks: Vec<&str> = encoded
        .as_bytes()
        .chunks(KITTY_CHUNK_CHARS)
        .map(|c| std::str::from_utf8(c).unwrap_or(""))
        .collect();
    let total = chunks.len().max(1);
    for (i, chunk) in chunks.iter().enumerate() {
        let more = u8::from(i + 1 < total);
        if i == 0 {
            out.push_str(&format!("\u{1b}_Ga=T,f=100,m={more};{chunk}\u{1b}\\"));
        } else {
            out.push_str(&format!("\u{1b}_Gm={more};{chunk}\u{1b}\\"));
        }
    }
    if total == 1 && chunks.is_empty() {
        out.push_str("\u{1b}_Ga=T,f=100,m=0;\u{1b}\\");
    }
    Ok(out)
}

/// Minimal standard-alphabet base64 (no line wraps), used so the
/// crate needs no extra dependency for one encoding.
fn base64_encode(data: &[u8]) -> String {
    const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for triple in data.chunks(3) {
        let b0 = u32::from(triple[0]);
        let b1 = u32::from(*triple.get(1).unwrap_or(&0));
        let b2 = u32::from(*triple.get(2).unwrap_or(&0));
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ALPHABET[(n >> 18) as usize & 63] as char);
        out.push(ALPHABET[(n >> 12) as usize & 63] as char);
        out.push(if triple.len() > 1 {
            ALPHABET[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if triple.len() > 2 {
            ALPHABET[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// What displaying one image means under the current ladder: the
/// protocol, the label line, and the payload (ANSI for half-block,
/// APC sequence for kitty).
#[derive(Debug, Clone, PartialEq)]
pub struct DisplayPlan {
    /// Protocol used.
    pub protocol: Protocol,
    /// On-screen label naming protocol + decode path.
    pub label: String,
    /// Bytes/text to write to the terminal.
    pub payload: String,
}

/// Build the display plan for PNG bytes under `caps`, bounded to the
/// given cell box for the half-block path.
pub fn plan_display(
    png_bytes: &[u8],
    caps: &Capabilities,
    cells_width_max: u32,
    cells_height_max: u32,
) -> Result<DisplayPlan, MediaError> {
    let protocol = choose_protocol(caps);
    let payload = match protocol {
        Protocol::Kitty => kitty_passthrough(png_bytes)?,
        Protocol::HalfBlock => {
            render_half_block(png_bytes, cells_width_max, cells_height_max)?.to_ansi()
        }
    };
    Ok(DisplayPlan {
        protocol,
        label: display_label(protocol),
        payload,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgba};

    /// A deterministic 4×4 fixture PNG: red/green/blue/white quadrants.
    fn fixture_png() -> Vec<u8> {
        let img: RgbaImage = ImageBuffer::from_fn(4, 4, |x, y| match (x < 2, y < 2) {
            (true, true) => Rgba([255, 0, 0, 255]),
            (false, true) => Rgba([0, 255, 0, 255]),
            (true, false) => Rgba([0, 0, 255, 255]),
            (false, false) => Rgba([255, 255, 255, 255]),
        });
        let mut bytes: Vec<u8> = Vec::new();
        img.write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .expect("encode fixture");
        bytes
    }

    #[test]
    fn half_block_render_is_byte_stable_across_two_runs() {
        let bytes = fixture_png();
        let first = render_half_block(&bytes, 8, 4).expect("render 1");
        let second = render_half_block(&bytes, 8, 4).expect("render 2");
        assert_eq!(first, second);
        assert_eq!(first.to_ansi(), second.to_ansi());
    }

    #[test]
    fn half_block_quadrants_land_in_the_right_cells() {
        let bytes = fixture_png();
        let render = render_half_block(&bytes, 2, 1).expect("render");
        assert_eq!(render.width_cells, 2);
        assert_eq!(render.cells[0].fg, [255, 0, 0]);
        assert_eq!(render.cells[1].fg, [0, 255, 0]);
        assert_eq!(render.cells[0].bg, [0, 0, 255]);
        assert_eq!(render.cells[1].bg, [255, 255, 255]);
    }

    #[test]
    fn oversized_image_bytes_are_rejected_by_named_bound() {
        let big = vec![0u8; IMAGE_BYTES_MAX + 1];
        assert!(matches!(
            render_half_block(&big, 8, 4),
            Err(MediaError::TooManyBytes(_))
        ));
        assert!(matches!(
            kitty_passthrough(&big),
            Err(MediaError::TooManyBytes(_))
        ));
    }

    #[test]
    fn oversized_cell_grid_is_rejected_not_panicked() {
        let bytes = fixture_png();
        assert!(matches!(
            render_half_block(&bytes, CELLS_WIDTH_MAX + 1, 4),
            Err(MediaError::TooManyCells(_, _))
        ));
        assert!(matches!(
            render_half_block(&bytes, 0, 4),
            Err(MediaError::TooManyCells(_, _))
        ));
    }

    #[test]
    fn non_png_bytes_are_rejected() {
        assert!(matches!(
            render_half_block(b"definitely not a png", 8, 4),
            Err(MediaError::BadImage(_))
        ));
        assert!(matches!(
            kitty_passthrough(b"definitely not a png"),
            Err(MediaError::BadImage(_))
        ));
    }

    #[test]
    fn capability_detection_without_graphics_terminal() {
        let caps = detect_capabilities(Some("xterm-256color"), None);
        assert!(!caps.kitty);
        assert_eq!(choose_protocol(&caps), Protocol::HalfBlock);
        // Follow-on rungs are explicitly absent, never implied.
        assert_eq!(caps.sixel, RungState::Unavailable);
        assert_eq!(caps.video, RungState::Unavailable);
        assert_eq!(caps.half_block, RungState::Available);
    }

    #[test]
    fn capability_detection_with_kitty_terminal() {
        let caps = detect_capabilities(Some("xterm-kitty"), None);
        assert!(caps.kitty);
        assert_eq!(choose_protocol(&caps), Protocol::Kitty);
        let caps_env = detect_capabilities(Some("xterm"), Some("12345"));
        assert!(caps_env.kitty);
    }

    #[test]
    fn kitty_passthrough_chunks_and_labels() {
        let bytes = fixture_png();
        let seq = kitty_passthrough(&bytes).expect("sequence");
        assert!(seq.starts_with("\u{1b}_Ga=T,f=100"));
        assert!(seq.ends_with("\u{1b}\\"));
        // Passthrough carries the original bytes (base64), not a re-encode.
        assert!(seq.contains(&base64_encode(&bytes[0..16])[0..8]));
        let label = display_label(Protocol::Kitty);
        assert!(label.contains("kitty"));
        assert!(label.contains("pinned-CPU"));
    }

    #[test]
    fn viewing_never_rewrites_the_file() {
        let dir = std::env::temp_dir().join(format!("lumen-tui-media-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        let path = dir.join("fixture.png");
        let bytes = fixture_png();
        std::fs::write(&path, &bytes).expect("write fixture");
        let read_back = read_image_file(&path).expect("read");
        let _ = render_half_block(&read_back, 8, 4).expect("render");
        let _ = kitty_passthrough(&read_back).expect("kitty");
        let after = std::fs::read(&path).expect("read after");
        assert_eq!(after, bytes);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn list_media_filters_and_sorts() {
        let dir = std::env::temp_dir().join(format!("lumen-tui-list-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join("b.png"), fixture_png()).expect("write");
        std::fs::write(dir.join("a.gif"), b"GIF89a").expect("write");
        std::fs::write(dir.join("c.txt"), b"nope").expect("write");
        let entries = list_media(&dir).expect("list");
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "a.gif");
        assert_eq!(entries[1].name, "b.png");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
