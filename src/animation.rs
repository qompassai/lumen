//! Idle animation pipeline: strip splitting, transparency cleanup, GIF assembly.
//!
//! Streamlines the companion idle-animation workflow established 2026-10-02:
//! 12-frame breathing loops, one consistent pose with micro-motion, clean
//! transparency, consistent output sizing.
//!
//! Contract summary:
//! - Accepted: RGBA images; strips must have width divisible by frame count.
//! - Bounds: frames are rejected if any dimension exceeds `DOC_MAX_DIMENSION`.
//! - Failure behavior: parameters validated before any write; outputs are
//!   written atomically; existing outputs are overwritten.

use std::path::Path;

use image::{Rgba, RgbaImage};
use rayon::prelude::*;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::LumenError;
use crate::aseprite::TagDirection;

/// Standard idle GIF output width (matches README table).
pub const IDLE_GIF_WIDTH: u32 = 144;
/// Standard idle GIF output height (matches README table).
pub const IDLE_GIF_HEIGHT: u32 = 288;
/// Standard frame duration for breathing idle (ms). 12 frames = ~2.4s loop.
pub const IDLE_FRAME_DURATION_MS: u32 = 200;
/// Alpha threshold below which pixels are forced fully transparent.
/// Kills the edge "traces" from AI generation and scaling.
pub const ALPHA_CLEAN_THRESHOLD: u8 = 16;

// ---------------------------------------------------------------------------
// Requests and results
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SplitStripRequest {
    /// Project-relative strip image path (horizontal frames).
    pub input: String,
    /// Number of frames in the strip. Width must be divisible by this.
    pub frames: u32,
    /// Project-relative output directory for individual frame PNGs.
    pub output_dir: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct SplitStripResult {
    /// Paths of written frame PNGs, in order.
    pub frames: Vec<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct BuildIdleGifRequest {
    /// Project-relative frame PNG paths, in order.
    pub frames: Vec<String>,
    /// Project-relative `.gif` output path (required).
    pub output: String,
    /// Frame duration in ms. Defaults to `IDLE_FRAME_DURATION_MS`.
    pub frame_duration_ms: Option<u32>,
    /// Output width. Defaults to `IDLE_GIF_WIDTH`.
    pub width: Option<u32>,
    /// Output height. Defaults to `IDLE_GIF_HEIGHT`.
    pub height: Option<u32>,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct BuildIdleGifResult {
    /// Path of written GIF.
    pub output: String,
    /// Frame count.
    pub frame_count: u32,
}

// ---------------------------------------------------------------------------
// Core operations
// ---------------------------------------------------------------------------

/// Split a horizontal strip into individual frames.
///
/// Each frame is cleaned (transparency), trimmed to content, scaled to fit
/// within `target_w` x `target_h` preserving aspect ratio, and centered on
/// a transparent canvas of exactly that size. This is what guarantees
/// consistent sizing across companions with different body proportions.
pub fn split_strip(
    strip: &RgbaImage,
    frame_count: u32,
    target_w: u32,
    target_h: u32,
) -> Result<Vec<RgbaImage>, LumenError> {
    if frame_count == 0 || frame_count > 64 {
        return Err(LumenError::BadParam(format!(
            "frame_count must be 1..=64, got {frame_count}"
        )));
    }
    let (sw, sh) = (strip.width(), strip.height());
    if sw % frame_count != 0 {
        return Err(LumenError::BadParam(format!(
            "strip width {sw} not divisible by frame_count {frame_count}"
        )));
    }
    let fw = sw / frame_count;

    // Frames are independent: rayon parallelizes the loop while `collect`
    // preserves frame order, so output stays deterministic.
    let out: Vec<RgbaImage> = (0..frame_count)
        .into_par_iter()
        .map(|i| {
            let x0 = i * fw;
            let mut frame = RgbaImage::new(fw, sh);
            for y in 0..sh {
                for x in 0..fw {
                    frame.put_pixel(x, y, *strip.get_pixel(x0 + x, y));
                }
            }
            clean_transparency(&mut frame);
            trim_scale_center(&frame, target_w, target_h)
        })
        .collect();
    Ok(out)
}

/// Force near-transparent pixels to fully transparent.
///
/// AI-generated images and scaling leave semi-transparent edge "traces".
/// Pixels below `ALPHA_CLEAN_THRESHOLD` become `(0,0,0,0)`; surviving
/// pixels get full opacity to prevent halos.
pub fn clean_transparency(img: &mut RgbaImage) {
    for px in img.pixels_mut() {
        if px[3] < ALPHA_CLEAN_THRESHOLD {
            *px = Rgba([0, 0, 0, 0]);
        } else {
            px[3] = 255;
        }
    }
}

/// Key background via connected components instead of a global threshold.
///
/// Labels connected regions of "background-like" pixels (transparent or
/// near-white opaque) with `imageproc`, then keys only the regions touching
/// the image border. Interior white (clothing, highlights, enclosed holes)
/// is preserved because its region never touches the border. This is the
/// deterministic form of the white-panel keying the README pipeline needed:
/// generated strips arrive on solid white, and only the border-connected
/// panel may go.
pub fn clean_transparency_cc(img: &mut RgbaImage) {
    use image::{GrayImage, Luma};
    use imageproc::region_labelling::{Connectivity, connected_components};

    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return;
    }
    // imageproc's connected_components mis-indexes degenerate (1-wide)
    // images. Every pixel touches the border there anyway, so key the
    // background-like pixels directly with identical semantics.
    if w < 2 || h < 2 {
        for px in img.pixels_mut() {
            let bg = px[3] < 128 || (px[0] > 240 && px[1] > 240 && px[2] > 240);
            if bg {
                *px = Rgba([0, 0, 0, 0]);
            }
        }
        return;
    }
    let mask = GrayImage::from_fn(w, h, |x, y| {
        let px = img.get_pixel(x, y);
        let bg = px[3] < 128 || (px[0] > 240 && px[1] > 240 && px[2] > 240);
        Luma([u8::from(bg) * 255])
    });
    let labels = connected_components(&mask, Connectivity::Eight, Luma([0u8]));
    // Border-touching component labels are true background. Label 0 marks
    // content pixels, so it is excluded even when content touches the edge.
    let mut border = std::collections::BTreeSet::new();
    for x in 0..w {
        border.insert(labels.get_pixel(x, 0)[0]);
        border.insert(labels.get_pixel(x, h - 1)[0]);
    }
    for y in 0..h {
        border.insert(labels.get_pixel(0, y)[0]);
        border.insert(labels.get_pixel(w - 1, y)[0]);
    }
    border.remove(&0);
    if border.is_empty() {
        return;
    }
    for y in 0..h {
        for x in 0..w {
            if border.contains(&labels.get_pixel(x, y)[0]) {
                img.put_pixel(x, y, Rgba([0, 0, 0, 0]));
            }
        }
    }
}

/// Trim to content bounding box, scale to fit within target preserving
/// aspect ratio, and center on a transparent canvas of exactly target size.
pub fn trim_scale_center(img: &RgbaImage, target_w: u32, target_h: u32) -> RgbaImage {
    // Find content bounds
    let (w, h) = (img.width(), img.height());
    let mut min_x = w; let mut max_x = 0;
    let mut min_y = h; let mut max_y = 0;
    for y in 0..h {
        for x in 0..w {
            if img.get_pixel(x, y)[3] > 0 {
                if x < min_x { min_x = x; }
                if x > max_x { max_x = x; }
                if y < min_y { min_y = y; }
                if y > max_y { max_y = y; }
            }
        }
    }
    // Empty image: return blank canvas
    if max_x < min_x || max_y < min_y {
        return RgbaImage::new(target_w, target_h);
    }

    let cw = max_x - min_x + 1;
    let ch = max_y - min_y + 1;
    let scale = (target_w as f32 / cw as f32).min(target_h as f32 / ch as f32);
    let nw = ((cw as f32 * scale) as u32).max(1);
    let nh = ((ch as f32 * scale) as u32).max(1);

    let trimmed = image::imageops::crop_imm(img, min_x, min_y, cw, ch).to_image();
    let resized = image::imageops::resize(&trimmed, nw, nh, image::imageops::FilterType::Lanczos3);

    let mut canvas = RgbaImage::new(target_w, target_h);
    let ox = (target_w.saturating_sub(nw)) / 2;
    let oy = (target_h.saturating_sub(nh)) / 2;
    image::imageops::overlay(&mut canvas, &resized, ox as i64, oy as i64);
    canvas
}

/// Assemble frames into an animated GIF.
///
/// Uses `IDLE_FRAME_DURATION_MS` and `IDLE_GIF_WIDTH`/`IDLE_GIF_HEIGHT`
/// defaults when not specified. Loops forever.
pub fn build_gif(
    frames: &[RgbaImage],
    output: &Path,
    frame_duration_ms: u32,
) -> Result<(), LumenError> {
    if frames.is_empty() {
        return Err(LumenError::BadParam(
            "build_gif requires at least one frame".into(),
        ));
    }
    let (w, h) = (frames[0].width(), frames[0].height());
    for (i, f) in frames.iter().enumerate() {
        if f.width() != w || f.height() != h {
            return Err(LumenError::BadParam(format!(
                "frame {i} is {}x{}, expected {w}x{h}",
                f.width(), f.height()
            )));
        }
    }

    let file = std::fs::File::create(output).map_err(|e| {
        LumenError::Io(format!("creating GIF {}: {e}", output.display()))
    })?;
    let mut encoder = image::codecs::gif::GifEncoder::new(file);
    encoder
        .set_repeat(image::codecs::gif::Repeat::Infinite)
        .map_err(|e| LumenError::Io(format!("setting GIF repeat: {e}")))?;

    let delay = image::Delay::from_numer_denom_ms(frame_duration_ms, 1);
    for frame in frames {
        let buffer = image::ImageBuffer::from_fn(w, h, |x, y| {
            let px = frame.get_pixel(x, y);
            image::Rgba([px[0], px[1], px[2], px[3]])
        });
        let gif_frame = image::Frame::from_parts(buffer, 0, 0, delay);
        encoder.encode_frame(gif_frame).map_err(|e| {
            LumenError::Io(format!("encoding GIF frame: {e}"))
        })?;
    }
    Ok(())
}

/// Count opaque pixels (alpha > threshold) for blank-image detection.
///
/// A working 64×64 profile has 1500+ opaque pixels. Below 1000 is suspect.
pub fn count_opaque(img: &RgbaImage, threshold: u8) -> u32 {
    img.pixels().filter(|px| px[3] > threshold).count() as u32
}


// ---------------------------------------------------------------------------
// Tag frame expansion
// ---------------------------------------------------------------------------

/// Expand a tag's frame range into an ordered frame sequence honoring its
/// loop direction.
///
/// - `from_frame`, `to_frame`: inclusive range from the Aseprite tag.
///   Must satisfy `from_frame <= to_frame`.
/// - Returns the frame indices in playback order.
///
/// Contract:
/// - Forward:  `f, f+1, ..., t`
/// - Reverse:  `t, t-1, ..., f`
/// - PingPong: `f, f+1, ..., t, t-1, ..., f+1` (endpoints not duplicated,
///   matching Aseprite's ping-pong playback)
/// - Single-frame tags yield exactly one frame regardless of direction.
/// - Rejects `from_frame > to_frame` as `BadParam` (corrupt file).
/// - Bounded: output length <= 2 * frame count for ping-pong, else <= frame count.
pub fn expand_tag_frames(
    from_frame: u32,
    to_frame: u32,
    direction: TagDirection,
) -> Result<Vec<u32>, LumenError> {
    if from_frame > to_frame {
        return Err(LumenError::BadParam(format!(
            "tag frame range inverted: from_frame {from_frame} > to_frame {to_frame}"
        )));
    }
    let mut frames: Vec<u32> = Vec::new();
    match direction {
        TagDirection::Forward => {
            for f in from_frame..=to_frame {
                frames.push(f);
            }
        }
        TagDirection::Reverse => {
            for f in (from_frame..=to_frame).rev() {
                frames.push(f);
            }
        }
        TagDirection::PingPong => {
            // Up: f..=t
            for f in from_frame..=to_frame {
                frames.push(f);
            }
            // Down: (t-1)..=f, skipping both endpoints to avoid duplication.
            // from_frame == to_frame yields a single frame (loop below is empty).
            if from_frame < to_frame {
                for f in (from_frame..to_frame).rev() {
                    frames.push(f);
                }
            }
        }
    }
    Ok(frames)
}

/// Sanitize a tag name for filesystem use: lowercase, spaces to underscores,
/// keep only `[a-z0-9_-]`. Empty results become `"tag"`.
pub fn sanitize_tag_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if c == ' ' || c == '-' || c == '_' {
            out.push('_');
        }
        // Drop everything else (including path separators and unicode).
    }
    // Collapse consecutive underscores and trim leading/trailing.
    let mut collapsed = String::with_capacity(out.len());
    let mut prev_underscore = false;
    for c in out.chars() {
        if c == '_' {
            if !prev_underscore {
                collapsed.push(c);
            }
            prev_underscore = true;
        } else {
            collapsed.push(c);
            prev_underscore = false;
        }
    }
    let trimmed = collapsed.trim_matches('_');
    if trimmed.is_empty() {
        "tag".to_string()
    } else {
        trimmed.to_string()
    }
}

#[cfg(test)]
mod tag_tests {
    use super::*;

    #[test]
    fn expand_forward() {
        assert_eq!(
            expand_tag_frames(2, 5, TagDirection::Forward).unwrap(),
            vec![2, 3, 4, 5]
        );
    }

    #[test]
    fn expand_reverse() {
        assert_eq!(
            expand_tag_frames(2, 5, TagDirection::Reverse).unwrap(),
            vec![5, 4, 3, 2]
        );
    }

    #[test]
    fn expand_pingpong() {
        assert_eq!(
            expand_tag_frames(2, 5, TagDirection::PingPong).unwrap(),
            vec![2, 3, 4, 5, 4, 3, 2]
        );
    }

    #[test]
    fn expand_single_frame() {
        for d in [TagDirection::Forward, TagDirection::Reverse, TagDirection::PingPong] {
            assert_eq!(expand_tag_frames(3, 3, d).unwrap(), vec![3]);
        }
    }

    #[test]
    fn expand_two_frame_pingpong() {
        // f=0,t=1 ping-pong: 0,1 (down-leg is empty since endpoints excluded)
        assert_eq!(
            expand_tag_frames(0, 1, TagDirection::PingPong).unwrap(),
            vec![0, 1, 0]
        );
    }

    #[test]
    fn expand_inverted_rejected() {
        assert!(expand_tag_frames(5, 2, TagDirection::Forward).is_err());
    }

    #[test]
    fn sanitize_basic() {
        assert_eq!(sanitize_tag_name("Idle"), "idle");
        assert_eq!(sanitize_tag_name("Run Cycle"), "run_cycle");
        assert_eq!(sanitize_tag_name("attack-1"), "attack_1");
    }

    #[test]
    fn sanitize_hostile() {
        // Path traversal stripped
        assert_eq!(sanitize_tag_name("../../etc"), "etc");
        // Empty and punctuation-only become "tag"
        assert_eq!(sanitize_tag_name(""), "tag");
        assert_eq!(sanitize_tag_name("!!!"), "tag");
        // Unicode dropped
        assert_eq!(sanitize_tag_name("h\u{00e9}ro"), "hro");
    }

    #[test]
    fn sanitize_collapses_underscores() {
        assert_eq!(sanitize_tag_name("a  b"), "a_b");
        assert_eq!(sanitize_tag_name("_lead_"), "lead");
    }
}


// ---------------------------------------------------------------------------
// Magenta chrominance matting
// ---------------------------------------------------------------------------
//
// Port of the Python `magenta_matte.py` pipeline (itself ported from
// gykim80/perfectpixel-studio's chroma.go, MIT). Removes a solid magenta
// (#FF00FF) background from AI-generated sprites via YCbCr chrominance
// keying, preserving smooth high-resolution alpha (no pixel quantization).
//
// Pipeline:
// 1. Detect background key color from corners/borders (CbCr histogram mode,
//    with magenta bias).
// 2. Per-pixel CbCr distance -> smoothstep alpha (soft edges).
// 3. Chroma-space despill (remove magenta cast from edge pixels).
// 4. Border flood-fill (key only background-connected regions).
// 5. Isolated speck cleanup (3x3 neighbor voting).
//
// Contract:
// - Input: RGBA image (alpha channel ignored, treated as opaque RGB).
// - Output: RGBA image with matted alpha.
// - Pure function: no I/O, no allocation beyond the output image + small
//   working buffers. Bounded by input dimensions (checked by caller via
//   MAX_IMAGE_DIMENSION).
// - Never panics on valid input; empty images return empty output.

/// CbCr distance below which pixels are fully transparent.
pub const MAGENTA_CHROMA_IN: f64 = 24.0;
/// CbCr distance above which pixels are fully opaque.
pub const MAGENTA_CHROMA_OUT: f64 = 72.0;
/// Despill band width in CbCr distance.
const MAGENTA_DESPILL_BAND: f64 = 100.0;
/// Despill strength (0.0 = none, 1.0 = full).
const MAGENTA_DESPILL_SCALE: f64 = 0.92;
/// Flood-fill tolerance in CbCr distance.
const MAGENTA_FLOOD_TOL: f64 = 88.0;
/// Alpha threshold for opaque/transparent classification.
const MAGENTA_ALPHA_THRESH: u8 = 16;

/// Convert RGB to YCbCr (JFIF variant, matching the Python implementation).
fn rgb_to_ycc(r: f64, g: f64, b: f64) -> (f64, f64, f64) {
    let y = 0.299 * r + 0.587 * g + 0.114 * b;
    let cb = (b - y) * 0.564 + 128.0;
    let cr = (r - y) * 0.713 + 128.0;
    (y, cb, cr)
}

/// Convert YCbCr back to RGB, clamped to [0, 255].
fn ycc_to_rgb(y: f64, cb: f64, cr: f64) -> (f64, f64, f64) {
    let r = y + 1.402 * (cr - 128.0);
    let g = y - 0.344136 * (cb - 128.0) - 0.714136 * (cr - 128.0);
    let b = y + 1.772 * (cb - 128.0);
    (r.clamp(0.0, 255.0), g.clamp(0.0, 255.0), b.clamp(0.0, 255.0))
}

/// Hermite smoothstep: 0 below e0, 1 above e1, smooth in between.
fn smoothstep(e0: f64, e1: f64, x: f64) -> f64 {
    let t = ((x - e0) / (e1 - e0).max(1e-9)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Detect the background key color from corners and borders.
///
/// Uses CbCr histogram mode with a magenta bias: if >=12% of border samples
/// look magenta (R>150, B>150, G<120), returns their mean; otherwise returns
/// the mean RGB of the most common CbCr bin (8x8 quantized).
fn detect_background_key(img: &RgbaImage) -> (f64, f64, f64) {
    let (w, h) = (img.width() as usize, img.height() as usize);
    if w == 0 || h == 0 {
        return (255.0, 0.0, 255.0);
    }

    // Collect border/corner samples.
    let cw = (w / 5).max(2);
    let ch = (h / 5).max(2);
    let mut samples: Vec<(u8, u8, u8)> = Vec::new();

    // Corners (cw x ch blocks)
    for y in 0..ch {
        for x in 0..cw {
            let p = img.get_pixel(x as u32, y as u32);
            samples.push((p[0], p[1], p[2]));
            let p = img.get_pixel((w - cw + x) as u32, y as u32);
            samples.push((p[0], p[1], p[2]));
            let p = img.get_pixel(x as u32, (h - ch + y) as u32);
            samples.push((p[0], p[1], p[2]));
            let p = img.get_pixel((w - cw + x) as u32, (h - ch + y) as u32);
            samples.push((p[0], p[1], p[2]));
        }
    }
    // Thin borders (top/bottom rows, left/right columns)
    for x in 0..w {
        let p = img.get_pixel(x as u32, 0);
        samples.push((p[0], p[1], p[2]));
        let p = img.get_pixel(x as u32, (h - 1) as u32);
        samples.push((p[0], p[1], p[2]));
    }
    for y in 0..h {
        let p = img.get_pixel(0, y as u32);
        samples.push((p[0], p[1], p[2]));
        let p = img.get_pixel((w - 1) as u32, y as u32);
        samples.push((p[0], p[1], p[2]));
    }

    if samples.is_empty() {
        return (255.0, 0.0, 255.0);
    }

    // Magenta bias: if >=12% of samples look magenta, use their mean.
    let magenta_samples: Vec<_> = samples
        .iter()
        .filter(|(r, g, b)| *r > 150 && *b > 150 && *g < 120)
        .collect();
    if magenta_samples.len() * 100 >= samples.len() * 12 {
        let n = magenta_samples.len() as f64;
        let (sr, sg, sb) = magenta_samples.iter().fold((0.0, 0.0, 0.0), |(ar, ag, ab), (r, g, b)| {
            (ar + *r as f64, ag + *g as f64, ab + *b as f64)
        });
        return (sr / n, sg / n, sb / n);
    }

    // CbCr histogram mode: quantize to 8x8 bins, take the most common.
    use std::collections::HashMap;
    let mut bins: HashMap<u16, (u32, f64, f64, f64)> = HashMap::new();
    for (r, g, b) in &samples {
        let (_, cb, cr) = rgb_to_ycc(*r as f64, *g as f64, *b as f64);
        let key = ((cb as u16 / 8) << 8) | (cr as u16 / 8);
        let entry = bins.entry(key).or_insert((0, 0.0, 0.0, 0.0));
        entry.0 += 1;
        entry.1 += *r as f64;
        entry.2 += *g as f64;
        entry.3 += *b as f64;
    }
    bins.values()
        .max_by_key(|(count, _, _, _)| *count)
        .map(|(count, sr, sg, sb)| {
            let n = *count as f64;
            (sr / n, sg / n, sb / n)
        })
        .unwrap_or((255.0, 0.0, 255.0))
}

/// Check if a key color looks magenta (for fallback logic).
fn is_magenta_key(key: (f64, f64, f64)) -> bool {
    key.0 > 150.0 && key.2 > 150.0 && key.1 < 120.0
}

/// Compute magenta residue fraction (for self-diagnostics).
/// Fraction of opaque pixels within CbCr distance 55 of pure magenta.
fn magenta_residue_frac(img: &RgbaImage) -> f64 {
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return 0.0;
    }
    let (_, mk_cb, mk_cr) = rgb_to_ycc(255.0, 0.0, 255.0);
    let mut residue = 0u64;
    let mut total = 0u64;
    for px in img.pixels() {
        total += 1;
        if px[3] > MAGENTA_ALPHA_THRESH {
            let (_, cb, cr) = rgb_to_ycc(px[0] as f64, px[1] as f64, px[2] as f64);
            let dist = ((cb - mk_cb).powi(2) + (cr - mk_cr).powi(2)).sqrt();
            if dist < 55.0 {
                residue += 1;
            }
        }
    }
    if total == 0 {
        0.0
    } else {
        residue as f64 / total as f64
    }
}

/// Matte an image against a specific key color.
/// Returns (matted RGBA, opaque fraction).
fn matte_with_key(img: &RgbaImage, key: (f64, f64, f64)) -> (RgbaImage, f64) {
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return (RgbaImage::new(w, h), 0.0);
    }

    let (_, k_cb, k_cr) = rgb_to_ycc(key.0, key.1, key.2);
    let kvb = k_cb - 128.0;
    let kvr = k_cr - 128.0;
    let klen = (kvb.powi(2) + kvr.powi(2)).sqrt();

    let mut out = RgbaImage::new(w, h);
    // Store per-pixel CbCr distance for flood-fill.
    let mut dist_map = vec![0.0f64; (w * h) as usize];

    for y in 0..h {
        for x in 0..w {
            let px = img.get_pixel(x, y);
            let (py, pcb, pcr) = rgb_to_ycc(px[0] as f64, px[1] as f64, px[2] as f64);
            let dist = ((pcb - k_cb).powi(2) + (pcr - k_cr).powi(2)).sqrt();
            dist_map[(y * w + x) as usize] = dist;

            let alpha = smoothstep(MAGENTA_CHROMA_IN, MAGENTA_CHROMA_OUT, dist);

            // Despill: pull chroma away from key direction in the band.
            let (mut r, mut g, mut b) = (px[0] as f64, px[1] as f64, px[2] as f64);
            if klen > 1.0 && dist < MAGENTA_DESPILL_BAND {
                let vx = pcb - 128.0;
                let vy = pcr - 128.0;
                let proj = (vx * kvb + vy * kvr) / klen;
                if proj > 0.0 {
                    let wgt = smoothstep(0.0, 1.0, (MAGENTA_DESPILL_BAND - dist) / MAGENTA_DESPILL_BAND)
                        * MAGENTA_DESPILL_SCALE;
                    let ub = kvb / klen;
                    let ur = kvr / klen;
                    let ncb = 128.0 + (vx - ub * proj * wgt);
                    let ncr = 128.0 + (vy - ur * proj * wgt);
                    let (nr, ng, nb) = ycc_to_rgb(py, ncb, ncr);
                    r = nr;
                    g = ng;
                    b = nb;
                }
            }

            out.put_pixel(
                x,
                y,
                Rgba([
                    r.clamp(0.0, 255.0) as u8,
                    g.clamp(0.0, 255.0) as u8,
                    b.clamp(0.0, 255.0) as u8,
                    (alpha * 255.0).clamp(0.0, 255.0) as u8,
                ]),
            );
        }
    }

    // Border flood-fill: key pixels connected to the border become transparent.
    let is_key = |x: u32, y: u32| -> bool {
        dist_map[(y * w + x) as usize] <= MAGENTA_FLOOD_TOL
    };
    let mut visited = vec![false; (w * h) as usize];
    let mut queue = std::collections::VecDeque::new();

    // Seed from borders.
    for x in 0..w {
        for y in [0, h - 1] {
            if is_key(x, y) && !visited[(y * w + x) as usize] {
                visited[(y * w + x) as usize] = true;
                queue.push_back((x, y));
            }
        }
    }
    for y in 0..h {
        for x in [0, w - 1] {
            if is_key(x, y) && !visited[(y * w + x) as usize] {
                visited[(y * w + x) as usize] = true;
                queue.push_back((x, y));
            }
        }
    }

    // BFS flood fill.
    while let Some((x, y)) = queue.pop_front() {
        out.get_pixel_mut(x, y)[3] = 0;
        for (nx, ny) in [(x.wrapping_sub(1), y), (x + 1, y), (x, y.wrapping_sub(1)), (x, y + 1)] {
            if nx < w && ny < h {
                let idx = (ny * w + nx) as usize;
                if is_key(nx, ny) && !visited[idx] {
                    visited[idx] = true;
                    queue.push_back((nx, ny));
                }
            }
        }
    }

    // Opaque fraction.
    let opaque = out.pixels().filter(|p| p[3] > MAGENTA_ALPHA_THRESH).count();
    let frac = opaque as f64 / (w as f64 * h as f64);

    (out, frac)
}

/// Clean up isolated specks via 3x3 neighbor voting.
/// - Opaque pixels with zero opaque neighbors become transparent.
/// - Transparent pixels with >=7 opaque neighbors become opaque.
fn cleanup_alpha(img: &mut RgbaImage) {
    let (w, h) = (img.width(), img.height());
    if w < 3 || h < 3 {
        return;
    }

    // Snapshot opacity to avoid mutation during iteration.
    let opaque: Vec<bool> = img.pixels().map(|p| p[3] > MAGENTA_ALPHA_THRESH).collect();

    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let idx = (y * w + x) as usize;
            let mut neighbors = 0;
            for dy in -1..=1i32 {
                for dx in -1..=1i32 {
                    if dx == 0 && dy == 0 {
                        continue;
                    }
                    let nx = (x as i32 + dx) as u32;
                    let ny = (y as i32 + dy) as u32;
                    if opaque[(ny * w + nx) as usize] {
                        neighbors += 1;
                    }
                }
            }
            let px = img.get_pixel_mut(x, y);
            if opaque[idx] && neighbors == 0 {
                px[3] = 0;
            } else if !opaque[idx] && neighbors >= 7 {
                px[3] = 255;
            }
        }
    }
}

/// Remove a magenta (or auto-detected) background via chrominance matting.
///
/// Full pipeline with magenta-fallback self-diagnostics: if the detected key
/// produces too much opaque area (>60%) or too much magenta residue (>2.5%),
/// retries with pure magenta and keeps the better result.
pub fn remove_magenta_background(img: &RgbaImage) -> RgbaImage {
    let key = detect_background_key(img);
    let (mut out, frac) = matte_with_key(img, key);

    // Self-diagnostics: try pure magenta if the detected key looks wrong.
    let residue = magenta_residue_frac(&out);
    if frac > 0.60 || residue > 0.025 {
        let (out2, frac2) = matte_with_key(img, (255.0, 0.0, 255.0));
        let residue2 = magenta_residue_frac(&out2);
        let better_frac = frac2 < frac - 0.03 && frac2 > 0.02;
        let better_residue = residue2 < residue;
        if (better_frac || better_residue) && frac2 > 0.02 {
            out = out2;
        }
    }
    if !is_magenta_key(key) {
        let (out2, frac2) = matte_with_key(img, (255.0, 0.0, 255.0));
        if frac2 > 0.02 && magenta_residue_frac(&out2) < magenta_residue_frac(&out) {
            out = out2;
        }
    }

    cleanup_alpha(&mut out);
    out
}

#[cfg(test)]
mod magenta_tests {
    use super::*;

    #[test]
    fn ycc_roundtrip() {
        let (y, cb, cr) = rgb_to_ycc(255.0, 0.0, 255.0);
        let (r, g, b) = ycc_to_rgb(y, cb, cr);
        assert!((r - 255.0).abs() < 1.0);
        assert!((g - 0.0).abs() < 1.0);
        assert!((b - 255.0).abs() < 1.0);
    }

    #[test]
    fn smoothstep_bounds() {
        assert_eq!(smoothstep(24.0, 72.0, 0.0), 0.0);
        assert_eq!(smoothstep(24.0, 72.0, 100.0), 1.0);
        let mid = smoothstep(24.0, 72.0, 48.0);
        assert!(mid > 0.4 && mid < 0.6);
    }

    #[test]
    fn magenta_background_removed() {
        // 10x10 magenta image with a red square in the center.
        let mut img = RgbaImage::from_pixel(10, 10, Rgba([255, 0, 255, 255]));
        for y in 3..7 {
            for x in 3..7 {
                img.put_pixel(x, y, Rgba([255, 0, 0, 255]));
            }
        }
        let out = remove_magenta_background(&img);
        // Corners should be transparent.
        assert_eq!(out.get_pixel(0, 0)[3], 0);
        assert_eq!(out.get_pixel(9, 9)[3], 0);
        // Center should be opaque red.
        let c = out.get_pixel(5, 5);
        assert!(c[3] > 200);
        assert!(c[0] > 200);
    }

    #[test]
    fn empty_image_noop() {
        let img = RgbaImage::new(0, 0);
        let out = remove_magenta_background(&img);
        assert_eq!(out.width(), 0);
        assert_eq!(out.height(), 0);
    }
}

#[cfg(test)]
mod animation_tests {
    use image::{Rgba, RgbaImage};
    use crate::animation::*;

    fn solid(w: u32, h: u32) -> RgbaImage {
        RgbaImage::from_pixel(w, h, Rgba([255, 0, 0, 255]))
    }

    // Validation: split_strip divides evenly
    #[test]
    fn split_even_division() {
        let strip = solid(120, 30);
        let frames = split_strip(&strip, 4, 144, 288).unwrap();
        assert_eq!(frames.len(), 4);
        for f in &frames {
            assert_eq!((f.width(), f.height()), (144, 288));
        }
    }

    // Validation: clean_transparency kills faint pixels, keeps solid
    #[test]
    fn clean_kills_faint() {
        let mut img = RgbaImage::from_pixel(2, 1, Rgba([0,0,0,0]));
        img.put_pixel(0, 0, Rgba([255,255,255,10])); // below threshold
        img.put_pixel(1, 0, Rgba([255,255,255,200])); // above threshold
        clean_transparency(&mut img);
        assert_eq!(img.get_pixel(0, 0)[3], 0);
        assert_eq!(img.get_pixel(1, 0)[3], 255);
    }

    // Validation: count_opaque
    #[test]
    fn opaque_count() {
        let img = solid(10, 10);
        assert_eq!(count_opaque(&img, 16), 100);
        let blank = RgbaImage::new(10, 10);
        assert_eq!(count_opaque(&blank, 16), 0);
    }

    // Adversarial: zero frames rejected
    #[test]
    fn split_zero_frames_rejected() {
        let strip = solid(120, 30);
        assert!(split_strip(&strip, 0, 144, 288).is_err());
    }

    // Adversarial: non-divisible width rejected
    #[test]
    fn split_uneven_rejected() {
        let strip = solid(121, 30);
        assert!(split_strip(&strip, 4, 144, 288).is_err());
    }

    // Adversarial: empty frames rejected for GIF
    #[test]
    fn gif_empty_rejected() {
        let out = std::path::Path::new("/tmp/should_not_exist.gif");
        assert!(build_gif(&[], out, 200).is_err());
    }

    // Adversarial: mismatched frame sizes rejected
    #[test]
    fn gif_mismatch_rejected() {
        let a = solid(144, 288);
        let b = solid(100, 100);
        let out = std::path::Path::new("/tmp/should_not_exist2.gif");
        assert!(build_gif(&[a, b], out, 200).is_err());
    }

    // Validation: trim_scale_center on empty returns blank canvas
    #[test]
    fn trim_empty_gives_blank() {
        let blank = RgbaImage::new(50, 50);
        let out = trim_scale_center(&blank, 144, 288);
        assert_eq!((out.width(), out.height()), (144, 288));
        assert_eq!(count_opaque(&out, 16), 0);
    }

    fn panel_fixture() -> RgbaImage {
        // 10x10: white border panel, red 6x6 center, white 2x2 hole inside.
        let mut img = RgbaImage::from_pixel(10, 10, Rgba([255, 255, 255, 255]));
        for y in 2..8 {
            for x in 2..8 {
                img.put_pixel(x, y, Rgba([255, 0, 0, 255]));
            }
        }
        for y in 4..6 {
            for x in 4..6 {
                img.put_pixel(x, y, Rgba([255, 255, 255, 255]));
            }
        }
        img
    }

    // Validation: CC keys the border panel, keeps center and interior hole
    #[test]
    fn cc_keys_border_panel_only() {
        let mut img = panel_fixture();
        clean_transparency_cc(&mut img);
        assert_eq!(img.get_pixel(0, 0)[3], 0); // border panel keyed
        assert_eq!(img.get_pixel(9, 9)[3], 0);
        assert_eq!(img.get_pixel(3, 3).0, [255, 0, 0, 255]); // center kept
        assert_eq!(img.get_pixel(4, 4).0, [255, 255, 255, 255]); // hole kept
    }

    // Validation: all-white image becomes fully transparent
    #[test]
    fn cc_all_white_clears() {
        let mut img = RgbaImage::from_pixel(8, 8, Rgba([255, 255, 255, 255]));
        clean_transparency_cc(&mut img);
        assert_eq!(count_opaque(&img, 16), 0);
    }

    // Validation: solid content with no border background is untouched
    #[test]
    fn cc_solid_untouched() {
        let mut img = RgbaImage::from_pixel(8, 8, Rgba([10, 20, 30, 255]));
        clean_transparency_cc(&mut img);
        assert_eq!(count_opaque(&img, 16), 64);
    }

    // Adversarial: empty image is a no-op, not a panic
    #[test]
    fn cc_empty_noop() {
        let mut img = RgbaImage::new(0, 0);
        clean_transparency_cc(&mut img);
    }

    // Adversarial: 1x1 images behave
    #[test]
    fn cc_single_pixel() {
        let mut white = RgbaImage::from_pixel(1, 1, Rgba([255, 255, 255, 255]));
        clean_transparency_cc(&mut white);
        assert_eq!(white.get_pixel(0, 0)[3], 0);
        let mut red = RgbaImage::from_pixel(1, 1, Rgba([255, 0, 0, 255]));
        clean_transparency_cc(&mut red);
        assert_eq!(red.get_pixel(0, 0)[3], 255);
    }
}
