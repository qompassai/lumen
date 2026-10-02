//! Built-in 5x7 pixel font: text rasterization and measurement.
//!
//! Contract:
//! - Accepted text: 1..=512 characters, each printable ASCII (32..=126) or
//!   `\n` (starts a new line). Anything else — tabs, `\r`, accented letters,
//!   emoji — is `BadParam` naming the offending character.
//! - Metrics at scale 1: glyph 5x7 px, advance 6 px (1 px gap), line height
//!   8 px. Every metric multiplies by `scale` (1..=16).
//! - Measured size is the inked bounding box: width = longest line in
//!   characters * 6 - 1, height = lines * 8 - 1 (an empty line has width 0).
//! - Rasterizing writes `fg` into glyph pixels only, clipped to the canvas;
//!   other pixels of the target layer are untouched (a new layer starts fully
//!   transparent). Work is bounded by 512 glyphs * 35 cells * scale², and
//!   glyphs entirely off-canvas are skipped.

#![forbid(unsafe_code)]

use std::path::Path;

use image::{ImageBuffer, Rgba, RgbaImage};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::LumenError;
use crate::doc::{BlendMode, DOC_MAX_LAYERS, DocSaved, Layer, load_doc, sync_frame_mods};
use crate::sprite_ops::{commit, output_target, unique_layer_name};

/// Longest accepted text, in characters (including `\n`).
const TEXT_MAX_CHARS: usize = 512;
/// Largest accepted integer scale factor.
const SCALE_MAX: u32 = 16;
const GLYPH_WIDTH_PX: u32 = 5;
const GLYPH_HEIGHT_PX: u32 = 7;
const ADVANCE_PX: u32 = 6;
const LINE_HEIGHT_PX: u32 = 8;
const FIRST_GLYPH: char = ' ';
const LAST_GLYPH: char = '~';
/// Name given to the layer created when `layer` is omitted.
const TEXT_LAYER_NAME: &str = "text";

/// Glyphs for ASCII 32..=126, seven rows top to bottom, five bits per row
/// (bit 4 = leftmost column).
#[rustfmt::skip]
const FONT: [[u8; 7]; 95] = [
    [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000], // ' '
    [0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00000, 0b00100], // '!'
    [0b01010, 0b01010, 0b01010, 0b00000, 0b00000, 0b00000, 0b00000], // '"'
    [0b01010, 0b01010, 0b11111, 0b01010, 0b11111, 0b01010, 0b01010], // '#'
    [0b00100, 0b01111, 0b10100, 0b01110, 0b00101, 0b11110, 0b00100], // '$'
    [0b11000, 0b11001, 0b00010, 0b00100, 0b01000, 0b10011, 0b00011], // '%'
    [0b01100, 0b10010, 0b10100, 0b01000, 0b10101, 0b10010, 0b01101], // '&'
    [0b00100, 0b00100, 0b01000, 0b00000, 0b00000, 0b00000, 0b00000], // '\''
    [0b00010, 0b00100, 0b01000, 0b01000, 0b01000, 0b00100, 0b00010], // '('
    [0b01000, 0b00100, 0b00010, 0b00010, 0b00010, 0b00100, 0b01000], // ')'
    [0b00000, 0b00100, 0b10101, 0b01110, 0b10101, 0b00100, 0b00000], // '*'
    [0b00000, 0b00100, 0b00100, 0b11111, 0b00100, 0b00100, 0b00000], // '+'
    [0b00000, 0b00000, 0b00000, 0b00000, 0b01100, 0b00100, 0b01000], // ','
    [0b00000, 0b00000, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000], // '-'
    [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b01100, 0b01100], // '.'
    [0b00000, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b00000], // '/'
    [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110], // '0'
    [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110], // '1'
    [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111], // '2'
    [0b11111, 0b00010, 0b00100, 0b00010, 0b00001, 0b10001, 0b01110], // '3'
    [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010], // '4'
    [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110], // '5'
    [0b00110, 0b01000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110], // '6'
    [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000], // '7'
    [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110], // '8'
    [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00010, 0b01100], // '9'
    [0b00000, 0b01100, 0b01100, 0b00000, 0b01100, 0b01100, 0b00000], // ':'
    [0b00000, 0b01100, 0b01100, 0b00000, 0b01100, 0b00100, 0b01000], // ';'
    [0b00010, 0b00100, 0b01000, 0b10000, 0b01000, 0b00100, 0b00010], // '<'
    [0b00000, 0b00000, 0b11111, 0b00000, 0b11111, 0b00000, 0b00000], // '='
    [0b01000, 0b00100, 0b00010, 0b00001, 0b00010, 0b00100, 0b01000], // '>'
    [0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b00000, 0b00100], // '?'
    [0b01110, 0b10001, 0b00001, 0b01101, 0b10101, 0b10101, 0b01110], // '@'
    [0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001], // 'A'
    [0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110], // 'B'
    [0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110], // 'C'
    [0b11100, 0b10010, 0b10001, 0b10001, 0b10001, 0b10010, 0b11100], // 'D'
    [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111], // 'E'
    [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000], // 'F'
    [0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01111], // 'G'
    [0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001], // 'H'
    [0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110], // 'I'
    [0b00111, 0b00010, 0b00010, 0b00010, 0b00010, 0b10010, 0b01100], // 'J'
    [0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001], // 'K'
    [0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111], // 'L'
    [0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001], // 'M'
    [0b10001, 0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001], // 'N'
    [0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110], // 'O'
    [0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000], // 'P'
    [0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101], // 'Q'
    [0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001], // 'R'
    [0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110], // 'S'
    [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100], // 'T'
    [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110], // 'U'
    [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100], // 'V'
    [0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b10101, 0b01010], // 'W'
    [0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001], // 'X'
    [0b10001, 0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100], // 'Y'
    [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111], // 'Z'
    [0b01110, 0b01000, 0b01000, 0b01000, 0b01000, 0b01000, 0b01110], // '['
    [0b00000, 0b10000, 0b01000, 0b00100, 0b00010, 0b00001, 0b00000], // '\\'
    [0b01110, 0b00010, 0b00010, 0b00010, 0b00010, 0b00010, 0b01110], // ']'
    [0b00100, 0b01010, 0b10001, 0b00000, 0b00000, 0b00000, 0b00000], // '^'
    [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b11111], // '_'
    [0b01000, 0b00100, 0b00010, 0b00000, 0b00000, 0b00000, 0b00000], // '`'
    [0b00000, 0b00000, 0b01110, 0b00001, 0b01111, 0b10001, 0b01111], // 'a'
    [0b10000, 0b10000, 0b10110, 0b11001, 0b10001, 0b10001, 0b11110], // 'b'
    [0b00000, 0b00000, 0b01110, 0b10000, 0b10000, 0b10001, 0b01110], // 'c'
    [0b00001, 0b00001, 0b01101, 0b10011, 0b10001, 0b10001, 0b01111], // 'd'
    [0b00000, 0b00000, 0b01110, 0b10001, 0b11111, 0b10000, 0b01110], // 'e'
    [0b00110, 0b01001, 0b01000, 0b11100, 0b01000, 0b01000, 0b01000], // 'f'
    [0b00000, 0b01111, 0b10001, 0b10001, 0b01111, 0b00001, 0b01110], // 'g'
    [0b10000, 0b10000, 0b10110, 0b11001, 0b10001, 0b10001, 0b10001], // 'h'
    [0b00100, 0b00000, 0b01100, 0b00100, 0b00100, 0b00100, 0b01110], // 'i'
    [0b00010, 0b00000, 0b00110, 0b00010, 0b00010, 0b10010, 0b01100], // 'j'
    [0b10000, 0b10000, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010], // 'k'
    [0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110], // 'l'
    [0b00000, 0b00000, 0b11010, 0b10101, 0b10101, 0b10001, 0b10001], // 'm'
    [0b00000, 0b00000, 0b10110, 0b11001, 0b10001, 0b10001, 0b10001], // 'n'
    [0b00000, 0b00000, 0b01110, 0b10001, 0b10001, 0b10001, 0b01110], // 'o'
    [0b00000, 0b00000, 0b11110, 0b10001, 0b11110, 0b10000, 0b10000], // 'p'
    [0b00000, 0b00000, 0b01101, 0b10011, 0b01111, 0b00001, 0b00001], // 'q'
    [0b00000, 0b00000, 0b10110, 0b11001, 0b10000, 0b10000, 0b10000], // 'r'
    [0b00000, 0b00000, 0b01110, 0b10000, 0b01110, 0b00001, 0b11110], // 's'
    [0b01000, 0b01000, 0b11100, 0b01000, 0b01000, 0b01001, 0b00110], // 't'
    [0b00000, 0b00000, 0b10001, 0b10001, 0b10001, 0b10011, 0b01101], // 'u'
    [0b00000, 0b00000, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100], // 'v'
    [0b00000, 0b00000, 0b10001, 0b10001, 0b10101, 0b10101, 0b01010], // 'w'
    [0b00000, 0b00000, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001], // 'x'
    [0b00000, 0b00000, 0b10001, 0b10001, 0b01111, 0b00001, 0b01110], // 'y'
    [0b00000, 0b00000, 0b11111, 0b00010, 0b00100, 0b01000, 0b11111], // 'z'
    [0b00010, 0b00100, 0b00100, 0b01000, 0b00100, 0b00100, 0b00010], // '{'
    [0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100], // '|'
    [0b01000, 0b00100, 0b00100, 0b00010, 0b00100, 0b00100, 0b01000], // '}'
    [0b00000, 0b00000, 0b01000, 0b10101, 0b00010, 0b00000, 0b00000], // '~'
];

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TextRasterizeRequest {
    /// Project-relative input .lumen.json.
    pub doc: String,
    /// Destination .lumen.json; omitted means save in place.
    pub output: Option<String>,
    /// Target layer index; omitted creates a new top layer named "text".
    pub layer: Option<usize>,
    /// 1..=512 chars of printable ASCII; "\n" starts a new line.
    pub text: String,
    /// Glyph color, "#RRGGBB" or "#RRGGBBAA".
    pub fg: String,
    /// Integer pixel scale, 1..=16.
    pub scale: u32,
    /// Top-left of the first glyph; may be off-canvas (clipped).
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TextMeasureRequest {
    /// 1..=512 chars of printable ASCII; "\n" starts a new line.
    pub text: String,
    /// Integer pixel scale, 1..=16.
    pub scale: u32,
}

/// Inked bounding box of a text block, in pixels.
#[derive(Debug, Clone, Serialize)]
pub struct TextSize {
    pub width: u32,
    pub height: u32,
}

/// Draw `text` into a layer (or a new "text" layer on top) with its first
/// glyph's top-left at (x, y). Glyph pixels are replaced with `fg`;
/// off-canvas pixels are clipped.
pub async fn text_rasterize(
    root: &Path,
    req: TextRasterizeRequest,
) -> Result<DocSaved, LumenError> {
    let target = output_target(&req.doc, req.output.as_deref())?;
    check_text(&req.text)?;
    check_scale(req.scale)?;
    let fg = Rgba(parse_color(&req.fg)?);
    let mut doc = load_doc(root, &req.doc)?;
    let layer_index = match req.layer {
        Some(index) if index < doc.layers.len() => index,
        Some(index) => {
            return Err(LumenError::BadParam(format!(
                "layer index {index} out of range ({} layers)",
                doc.layers.len()
            )));
        }
        None => {
            if doc.layers.len() >= DOC_MAX_LAYERS {
                return Err(LumenError::BadParam(format!(
                    "document already has {DOC_MAX_LAYERS} layers (the maximum)"
                )));
            }
            let name = unique_layer_name(&doc, TEXT_LAYER_NAME, None)?;
            doc.layers.push(Layer {
                name,
                visible: true,
                opacity: 1.0,
                blend: BlendMode::Normal,
                image: ImageBuffer::from_pixel(doc.width, doc.height, Rgba([0, 0, 0, 0])),
            });
            // Appending on top: sync_frame_mods adds the identity mod at the end.
            sync_frame_mods(&mut doc);
            doc.layers.len() - 1
        }
    };
    draw_text(&mut doc.layers[layer_index].image, &req.text, fg, req.scale, req.x, req.y);
    commit(root, target, &doc)
}

/// Measure the inked bounding box of `text` at `scale`. Pure: no I/O.
pub async fn text_measure(req: TextMeasureRequest) -> Result<TextSize, LumenError> {
    check_text(&req.text)?;
    check_scale(req.scale)?;
    let mut line_count: u32 = 0;
    let mut widest_chars: u32 = 0;
    for line in req.text.split('\n') {
        line_count += 1;
        // check_text bounds the whole text to 512 chars, so this fits u32.
        widest_chars = widest_chars.max(line.len() as u32);
    }
    // Max: 512 * 6 * 16 = 49152, far from u32 overflow.
    let width = (widest_chars * ADVANCE_PX).saturating_sub(1) * req.scale;
    let height = (line_count * LINE_HEIGHT_PX - 1) * req.scale;
    Ok(TextSize { width, height })
}

fn check_text(text: &str) -> Result<(), LumenError> {
    if text.is_empty() {
        return Err(LumenError::BadParam("text is empty".to_string()));
    }
    // Bounded scan: stop counting one past the limit.
    if text.chars().take(TEXT_MAX_CHARS + 1).count() > TEXT_MAX_CHARS {
        return Err(LumenError::BadParam(format!(
            "text exceeds {TEXT_MAX_CHARS} characters"
        )));
    }
    if let Some(c) = text
        .chars()
        .find(|&c| c != '\n' && !(FIRST_GLYPH..=LAST_GLYPH).contains(&c))
    {
        return Err(LumenError::BadParam(format!(
            "character {c:?} (U+{:04X}) is not printable ASCII (32..=126) or \\n",
            u32::from(c)
        )));
    }
    Ok(())
}

fn check_scale(scale: u32) -> Result<(), LumenError> {
    if !(1..=SCALE_MAX).contains(&scale) {
        return Err(LumenError::BadParam(format!("scale must be 1..={SCALE_MAX}")));
    }
    Ok(())
}

/// Strict hex color: `#RRGGBB` (alpha 255) or `#RRGGBBAA`. Byte-based so
/// multi-byte UTF-8 input cannot hit a char-boundary panic.
fn parse_color(s: &str) -> Result<[u8; 4], LumenError> {
    let malformed = || LumenError::BadParam("color must be #RRGGBB or #RRGGBBAA".to_string());
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

/// Rasterize pre-validated text. All coordinate math is i64 so a caller
/// origin near i32::MAX plus 512 * 6 * 16 px cannot overflow.
fn draw_text(image: &mut RgbaImage, text: &str, fg: Rgba<u8>, scale: u32, x: i32, y: i32) {
    let scale = i64::from(scale);
    let advance = i64::from(ADVANCE_PX) * scale;
    let line_height = i64::from(LINE_HEIGHT_PX) * scale;
    let (mut column, mut row): (i64, i64) = (0, 0);
    for c in text.chars() {
        if c == '\n' {
            row += 1;
            column = 0;
            continue;
        }
        // check_text admitted only FIRST_GLYPH..=LAST_GLYPH here.
        let glyph = &FONT[(u32::from(c) - u32::from(FIRST_GLYPH)) as usize];
        let origin_x = i64::from(x) + column * advance;
        let origin_y = i64::from(y) + row * line_height;
        draw_glyph(image, glyph, origin_x, origin_y, scale, fg);
        column += 1;
    }
}

fn draw_glyph(
    image: &mut RgbaImage,
    glyph: &[u8; 7],
    origin_x: i64,
    origin_y: i64,
    scale: i64,
    fg: Rgba<u8>,
) {
    let (width, height) = (i64::from(image.width()), i64::from(image.height()));
    let glyph_w = i64::from(GLYPH_WIDTH_PX) * scale;
    let glyph_h = i64::from(GLYPH_HEIGHT_PX) * scale;
    if origin_x >= width || origin_y >= height || origin_x + glyph_w <= 0 || origin_y + glyph_h <= 0
    {
        return;
    }
    for (glyph_row, bits) in (0_i64..).zip(glyph.iter()) {
        for glyph_col in 0..i64::from(GLYPH_WIDTH_PX) {
            if bits & (0b10000 >> glyph_col) == 0 {
                continue;
            }
            let cell_x = origin_x + glyph_col * scale;
            let cell_y = origin_y + glyph_row * scale;
            for py in cell_y.max(0)..(cell_y + scale).min(height) {
                for px in cell_x.max(0)..(cell_x + scale).min(width) {
                    // Clamped into 0..width/height, which fit u32.
                    image.put_pixel(px as u32, py as u32, fg);
                }
            }
        }
    }
}
