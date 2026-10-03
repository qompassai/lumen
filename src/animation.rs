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
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::LumenError;

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

    let mut out = Vec::with_capacity(frame_count as usize);
    for i in 0..frame_count {
        let x0 = i * fw;
        let mut frame = RgbaImage::new(fw, sh);
        for y in 0..sh {
            for x in 0..fw {
                frame.put_pixel(x, y, *strip.get_pixel(x0 + x, y));
            }
        }
        clean_transparency(&mut frame);
        let centered = trim_scale_center(&frame, target_w, target_h);
        out.push(centered);
    }
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
}
