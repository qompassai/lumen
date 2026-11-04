//! termcap — the terminal capability / image layer.
//!
//! D-902 posture: this module is separable from day
//! one. It depends on nothing else in the engine: given
//! an image it produces terminal renderings, and given
//! an environment it reports what the terminal can do.
//! Extraction into a shared estate crate happens when a
//! second consumer lands, not before.
//!
//! Three renderings of one image, in descending
//! fidelity (SPEC section 5.2):
//!
//! - **kitty graphics** — the image as PNG, base64'd
//!   into the terminal graphics protocol's chunked
//!   transmit form;
//! - **halfblocks** — one character cell per two
//!   vertical pixels (`▀` with truecolour foreground =
//!   top pixel, background = bottom pixel), as a pure
//!   cell grid the ANSI writer and the ratatui view both
//!   consume;
//! - the picker readout itself, so a surface can say
//!   which protocol it would use before it renders.
//!
//! Honest limit (SPEC section 5.2): in-terminal previews
//! are working instruments for staging and inspection.
//! They are not verdict surfaces; verdicts consume
//! rendered frames as files (D-221/D-629).

/// The graphics protocol a terminal presentation would
/// use, best first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Protocol {
    /// Kitty graphics protocol.
    Kitty,
    /// Sixel.
    Sixel,
    /// iTerm2 inline images.
    Iterm2,
    /// Half-block truecolour cells (always available).
    Halfblocks,
}

impl Protocol {
    /// Stable lowercase name for readouts.
    pub fn name(self) -> &'static str {
        match self {
            Protocol::Kitty => "kitty",
            Protocol::Sixel => "sixel",
            Protocol::Iterm2 => "iterm2",
            Protocol::Halfblocks => "halfblocks",
        }
    }
}

/// What the current terminal stack offers, and why the
/// layer believes it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Capabilities {
    /// The protocol a presentation would pick.
    pub protocol: Protocol,
    /// The evidence the pick rests on (env values read).
    pub evidence: String,
}

/// Detect capabilities from environment values. Pure:
/// the caller passes the values it read, so detection
/// is testable without a terminal.
pub fn detect(term: Option<&str>, term_program: Option<&str>) -> Capabilities {
    let term = term.unwrap_or("");
    let program = term_program.unwrap_or("");
    let evidence = format!("TERM={term} TERM_PROGRAM={program}");
    // Kitty protocol: the kitty terminal itself, and
    // ghostty, which speaks the same graphics protocol.
    let protocol = if term.contains("kitty")
        || program.eq_ignore_ascii_case("kitty")
        || program.eq_ignore_ascii_case("ghostty")
        || term.contains("ghostty")
    {
        Protocol::Kitty
    } else if term.starts_with("foot") || program.eq_ignore_ascii_case("foot") {
        Protocol::Sixel
    } else if program.eq_ignore_ascii_case("iterm.app") {
        Protocol::Iterm2
    } else {
        Protocol::Halfblocks
    };
    Capabilities { protocol, evidence }
}

/// One halfblock cell: the glyph is always `▀`;
/// `fg` colours the top pixel, `bg` the bottom.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cell {
    /// Top pixel RGB.
    pub fg: [u8; 3],
    /// Bottom pixel RGB.
    pub bg: [u8; 3],
}

/// A halfblock rendering: `rows` of `cols` cells.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CellGrid {
    /// Cell columns.
    pub cols: usize,
    /// Cell rows.
    pub rows: usize,
    /// Row-major cells.
    pub cells: Vec<Cell>,
}

/// Render an RGBA image to a halfblock cell grid,
/// nearest-neighbour scaled to at most `max_cols`
/// columns (aspect preserved through the 2:1 cell
/// geometry: one cell covers a 1x2 pixel block, so the
/// grid's visual aspect matches the source's).
pub fn halfblocks(img: &image::RgbaImage, max_cols: usize) -> CellGrid {
    let (w, h) = (img.width() as usize, img.height() as usize);
    if w == 0 || h == 0 || max_cols == 0 {
        return CellGrid {
            cols: 0,
            rows: 0,
            cells: Vec::new(),
        };
    }
    let cols = max_cols.min(w);
    // Source pixels per cell horizontally; vertically a
    // cell spans two pixel rows of the *scaled* image.
    // Scaled height in pixels = h * cols / w; rows =
    // ceil(scaled_height / 2).
    let scaled_h = ((h * cols) / w).max(1);
    let rows = scaled_h.div_ceil(2);
    let sample = |sx: usize, sy: usize| -> [u8; 3] {
        let px = img.get_pixel(sx.min(w - 1) as u32, sy.min(h - 1) as u32);
        composite_over_dark(px.0)
    };
    let mut cells = Vec::with_capacity(cols * rows);
    for row in 0..rows {
        for col in 0..cols {
            let sx = col * w / cols;
            let top_sy = (2 * row) * h / scaled_h;
            let bot_sy = ((2 * row + 1) * h / scaled_h).min(h - 1);
            cells.push(Cell {
                fg: sample(sx, top_sy),
                bg: sample(sx, bot_sy),
            });
        }
    }
    CellGrid { cols, rows, cells }
}

/// The evaluator's frames are RGBA over transparency;
/// a terminal cell has no alpha, so preview pixels are
/// composited over the staging ground (24, 24, 28) —
/// the same ground the host compositor uses.
fn composite_over_dark(px: [u8; 4]) -> [u8; 3] {
    let a = f32::from(px[3]) / 255.0;
    let ground = [24.0f32, 24.0, 28.0];
    let mut out = [0u8; 3];
    for i in 0..3 {
        out[i] = (f32::from(px[i]) * a + ground[i] * (1.0 - a)).round() as u8;
    }
    out
}

/// The cell grid as an ANSI truecolour string (one
/// text line per cell row), for terminals and files.
pub fn grid_to_ansi(grid: &CellGrid) -> String {
    let mut out = String::new();
    for row in 0..grid.rows {
        let mut last: Option<Cell> = None;
        for col in 0..grid.cols {
            let cell = grid.cells[row * grid.cols + col];
            if last != Some(cell) {
                out.push_str(&format!(
                    "\x1b[38;2;{};{};{};48;2;{};{};{}m",
                    cell.fg[0], cell.fg[1], cell.fg[2], cell.bg[0], cell.bg[1], cell.bg[2]
                ));
                last = Some(cell);
            }
            out.push('▀');
        }
        out.push_str("\x1b[0m\n");
    }
    out
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 (with padding), hand-rolled so the
/// layer carries no codec dependency of its own.
pub fn base64(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(B64[(n >> 18) as usize & 63] as char);
        out.push(B64[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 {
            B64[(n >> 6) as usize & 63] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            B64[n as usize & 63] as char
        } else {
            '='
        });
    }
    out
}

/// Encode PNG bytes as a kitty graphics transmit
/// payload (`a=T`, format 100 = PNG, direct data in
/// 4096-byte base64 chunks, `m=1` while more chunks
/// follow). The payload displays the image at the
/// terminal's current position when written raw.
pub fn kitty_payload(png: &[u8]) -> String {
    let encoded = base64(png);
    let mut out = String::new();
    let mut chunks = encoded.as_bytes().chunks(4096).peekable();
    let mut first = true;
    while let Some(chunk) = chunks.next() {
        let more = chunks.peek().is_some();
        if first {
            out.push_str(&format!(
                "\x1b_Ga=T,f=100,m={};{}\x1b\\",
                u8::from(more),
                String::from_utf8_lossy(chunk)
            ));
            first = false;
        } else {
            out.push_str(&format!(
                "\x1b_Gm={};{}\x1b\\",
                u8::from(more),
                String::from_utf8_lossy(chunk)
            ));
        }
    }
    if first {
        // Empty image: still a well-formed single chunk.
        out.push_str("\x1b_Ga=T,f=100,m=0;\x1b\\");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_known_vectors() {
        assert_eq!(base64(b""), "");
        assert_eq!(base64(b"f"), "Zg==");
        assert_eq!(base64(b"fo"), "Zm8=");
        assert_eq!(base64(b"foo"), "Zm9v");
        assert_eq!(base64(b"foob"), "Zm9vYg==");
        assert_eq!(base64(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn detect_prefers_kitty_then_sixel_then_halfblocks() {
        assert_eq!(detect(Some("xterm-kitty"), None).protocol, Protocol::Kitty);
        assert_eq!(detect(Some("foot"), None).protocol, Protocol::Sixel);
        assert_eq!(
            detect(Some("xterm-256color"), Some("iTerm.app")).protocol,
            Protocol::Iterm2
        );
        assert_eq!(detect(Some("dumb"), None).protocol, Protocol::Halfblocks);
        assert_eq!(detect(None, None).protocol, Protocol::Halfblocks);
    }

    #[test]
    fn halfblocks_grid_geometry() {
        let img = image::RgbaImage::from_pixel(4, 4, image::Rgba([200, 10, 10, 255]));
        let grid = halfblocks(&img, 100);
        assert_eq!(grid.cols, 4);
        assert_eq!(grid.rows, 2);
        assert_eq!(grid.cells.len(), 8);
        assert_eq!(grid.cells[0].fg, [200, 10, 10]);
        // Column cap is honoured.
        let capped = halfblocks(&img, 2);
        assert_eq!(capped.cols, 2);
        assert_eq!(capped.rows, 1);
    }

    #[test]
    fn transparency_composites_over_the_staging_ground() {
        let img = image::RgbaImage::from_pixel(1, 2, image::Rgba([0, 0, 0, 0]));
        let grid = halfblocks(&img, 10);
        assert_eq!(grid.cells[0].fg, [24, 24, 28]);
    }

    #[test]
    fn kitty_payload_is_chunked_and_framed() {
        let payload = kitty_payload(&vec![1u8; 5000]);
        assert!(payload.starts_with("\x1b_Ga=T,f=100,m=1;"));
        assert!(payload.contains("\x1b_Gm=0;"));
        assert!(payload.ends_with("\x1b\\"));
    }

    #[test]
    fn ansi_grid_has_one_line_per_row() {
        let img = image::RgbaImage::from_pixel(2, 4, image::Rgba([1, 2, 3, 255]));
        let text = grid_to_ansi(&halfblocks(&img, 10));
        assert_eq!(text.lines().count(), 2);
        assert!(text.contains('▀'));
    }
}
