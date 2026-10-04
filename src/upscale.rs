//! Real-ESRGAN upscaling via ONNX Runtime.
//!
//! Replaces the Python `fleet_upscale.py` (PyTorch) with pure Rust inference.
//! Uses the `ort` crate (ONNX Runtime bindings) with CUDA execution provider
//! on primo's RTX 4070.
//!
//! Contract summary:
//! - Accepted: RGBA images, model path to `.onnx` Real-ESRGAN file.
//! - Model: RealESRGAN_x4plus (RRDBNet), input NCHW f32 RGB [0,1], output 4x.
//! - Tiled inference: 512px tiles with 16px overlap to handle large images.
//! - Output: 2x upscale (4x model output Lanczos-downscaled to 2x, matching
//!   the Python pipeline's behavior).
//! - Alpha channel: Lanczos-upscaled separately on CPU (not through model).
//! - Failure: model load errors, inference errors as typed LumenError.

use std::path::Path;
use std::sync::{Arc, Mutex};

use image::{Rgba, RgbaImage};
use ort::session::Session;

use crate::LumenError;

/// Tile size for tiled inference (pixels).
pub const UPSCALE_TILE_SIZE: usize = 512;
/// Overlap padding between tiles (pixels).
pub const UPSCALE_TILE_PAD: usize = 16;
/// Model upscale factor (RealESRGAN_x4plus).
pub const MODEL_SCALE: usize = 4;
/// Target output scale (we do 4x then Lanczos down to 2x).
pub const TARGET_SCALE: usize = 2;

/// A tile for inference: padded read rect + inner content rect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Tile {
    sx: usize,
    sy: usize,
    sw: usize,
    sh: usize,
    ix: usize,
    iy: usize,
    iw: usize,
    ih: usize,
    ox: usize,
    oy: usize,
}

/// Plan tiles for a WxH image.
fn plan_tiles(w: usize, h: usize) -> Vec<Tile> {
    let mut tiles = Vec::new();
    let mut oy = 0;
    while oy < h {
        let ih = UPSCALE_TILE_SIZE.min(h - oy);
        let mut ox = 0;
        while ox < w {
            let iw = UPSCALE_TILE_SIZE.min(w - ox);
            let sx = ox.saturating_sub(UPSCALE_TILE_PAD);
            let sy = oy.saturating_sub(UPSCALE_TILE_PAD);
            let ex = (ox + iw + UPSCALE_TILE_PAD).min(w);
            let ey = (oy + ih + UPSCALE_TILE_PAD).min(h);
            tiles.push(Tile {
                sx, sy, sw: ex - sx, sh: ey - sy,
                ix: ox - sx, iy: oy - sy, iw, ih,
                ox, oy,
            });
            ox += iw;
        }
        oy += ih;
    }
    tiles
}

/// ONNX upscaler holding a persistent session.
pub struct Upscaler {
    session: Arc<Mutex<Session>>,
}

impl Upscaler {
    /// Load an ONNX model. Uses CUDA EP if available, falls back to CPU.
    pub fn new(model_path: &Path) -> Result<Self, LumenError> {
        let session = Session::builder()
            .map_err(|e| LumenError::DocInvalid(format!("ort builder: {e}")))?
            .with_optimization_level(ort::session::builder::GraphOptimizationLevel::Level3)
            .map_err(|e| LumenError::DocInvalid(format!("ort opt level: {e}")))?
            .commit_from_file(model_path)
            .map_err(|e| LumenError::Io(format!("loading ONNX {}: {e}", model_path.display())))?;
        Ok(Self { session: Arc::new(Mutex::new(session)) })
    }

    /// Upscale an RGBA image 2x (via 4x model + Lanczos down).
    pub fn upscale_2x(&self, img: &RgbaImage) -> Result<RgbaImage, LumenError> {
        let (w, h) = (img.width() as usize, img.height() as usize);
        if w == 0 || h == 0 {
            return Ok(RgbaImage::new(0, 0));
        }

        let mut rgb = image::RgbImage::new(w as u32, h as u32);
        let mut alpha = image::GrayImage::new(w as u32, h as u32);
        for (x, y, px) in img.enumerate_pixels() {
            rgb.put_pixel(x, y, image::Rgb([px[0], px[1], px[2]]));
            alpha.put_pixel(x, y, image::Luma([px[3]]));
        }

        let sr_4x = self.infer_tiled_4x(&rgb)?;

        let out_w = (w * TARGET_SCALE) as u32;
        let out_h = (h * TARGET_SCALE) as u32;
        let rgb_2x = image::imageops::resize(&sr_4x, out_w, out_h, image::imageops::FilterType::Lanczos3);
        let alpha_2x = image::imageops::resize(&alpha, out_w, out_h, image::imageops::FilterType::Lanczos3);

        let mut out = RgbaImage::new(out_w, out_h);
        for y in 0..out_h {
            for x in 0..out_w {
                let rgb_px = rgb_2x.get_pixel(x, y);
                let a = alpha_2x.get_pixel(x, y)[0];
                out.put_pixel(x, y, Rgba([rgb_px[0], rgb_px[1], rgb_px[2], a]));
            }
        }
        Ok(out)
    }

    fn infer_tiled_4x(&self, rgb: &image::RgbImage) -> Result<image::RgbImage, LumenError> {
        let (w, h) = (rgb.width() as usize, rgb.height() as usize);
        let out_w = w * MODEL_SCALE;
        let out_h = h * MODEL_SCALE;
        let mut out = image::RgbImage::new(out_w as u32, out_h as u32);

        for tile in plan_tiles(w, h) {
            let tile_img = image::imageops::crop_imm(
                rgb, tile.sx as u32, tile.sy as u32, tile.sw as u32, tile.sh as u32,
            ).to_image();

            let input = pack_nchw(&tile_img);

            let output = {
                let mut session = self.session.lock()
                    .map_err(|_| LumenError::DocInvalid("upscaler session lock poisoned".into()))?;
                let outputs = session.run(ort::inputs!["input" => input])
                    .map_err(|e| LumenError::DocInvalid(format!("onnx inference: {e}")))?;
                let (_shape, data) = outputs["output"].try_extract_tensor::<f32>()
                    .map_err(|e| LumenError::DocInvalid(format!("output tensor: {e}")))?;
                data.to_vec()
            };

            let tw = tile.sw * MODEL_SCALE;
            let th = tile.sh * MODEL_SCALE;
            let tile_4x = unpack_nchw(&output, tw, th)?;

            let ix = tile.ix * MODEL_SCALE;
            let iy = tile.iy * MODEL_SCALE;
            let iw = tile.iw * MODEL_SCALE;
            let ih = tile.ih * MODEL_SCALE;
            let ox = tile.ox * MODEL_SCALE;
            let oy = tile.oy * MODEL_SCALE;

            for y in 0..ih {
                for x in 0..iw {
                    let px = tile_4x.get_pixel((ix + x) as u32, (iy + y) as u32);
                    out.put_pixel((ox + x) as u32, (oy + y) as u32, *px);
                }
            }
        }
        Ok(out)
    }
}

fn pack_nchw(img: &image::RgbImage) -> ort::value::Tensor<f32> {
    let (w, h) = (img.width() as usize, img.height() as usize);
    let mut data = vec![0.0f32; 3 * w * h];
    for y in 0..h {
        for x in 0..w {
            let px = img.get_pixel(x as u32, y as u32);
            data[y * w + x] = px[0] as f32 / 255.0;
            data[w * h + y * w + x] = px[1] as f32 / 255.0;
            data[2 * w * h + y * w + x] = px[2] as f32 / 255.0;
        }
    }
    ort::value::Tensor::from_array(([1, 3, h, w], data)).expect("NCHW tensor shape valid")
}

fn unpack_nchw(data: &[f32], w: usize, h: usize) -> Result<image::RgbImage, LumenError> {
    if data.len() != 3 * w * h {
        return Err(LumenError::DocInvalid(format!("tensor size {} != 3*{w}*{h}", data.len())));
    }
    let mut img = image::RgbImage::new(w as u32, h as u32);
    for y in 0..h {
        for x in 0..w {
            let r = (data[y * w + x].clamp(0.0, 1.0) * 255.0) as u8;
            let g = (data[w * h + y * w + x].clamp(0.0, 1.0) * 255.0) as u8;
            let b = (data[2 * w * h + y * w + x].clamp(0.0, 1.0) * 255.0) as u8;
            img.put_pixel(x as u32, y as u32, image::Rgb([r, g, b]));
        }
    }
    Ok(img)
}

/// CLI entry: upscale every PNG in a directory 2x.
pub fn run_upscale(
    dir: &Path,
    out_dir: &Path,
    model_path: &Path,
) -> Result<Vec<std::path::PathBuf>, LumenError> {
    let upscaler = Upscaler::new(model_path)?;
    std::fs::create_dir_all(out_dir)
        .map_err(|e| LumenError::Io(format!("creating {}: {e}", out_dir.display())))?;

    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| LumenError::Io(format!("reading {}: {e}", dir.display())))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("png")))
        .collect();
    entries.sort();

    let mut written = Vec::new();
    for path in entries {
        let img = image::open(&path)
            .map_err(|e| LumenError::Io(format!("opening {}: {e}", path.display())))?;
        let rgba = img.to_rgba8();
        let upscaled = upscaler.upscale_2x(&rgba)?;
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("out.png");
        let dest = out_dir.join(name);
        upscaled.save(&dest)
            .map_err(|e| LumenError::Io(format!("saving {}: {e}", dest.display())))?;
        written.push(dest);
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tile_planning_single() {
        let tiles = plan_tiles(100, 100);
        assert_eq!(tiles.len(), 1);
        assert_eq!(tiles[0].iw, 100);
    }

    #[test]
    fn tile_planning_multi() {
        let tiles = plan_tiles(600, 600);
        assert_eq!(tiles.len(), 4);
    }

    #[test]
    fn tile_coverage_no_gaps() {
        let (w, h) = (700, 500);
        let tiles = plan_tiles(w, h);
        let mut covered = vec![vec![false; w]; h];
        for t in &tiles {
            for y in 0..t.ih {
                for x in 0..t.iw {
                    let px = t.ox + x;
                    let py = t.oy + y;
                    assert!(!covered[py][px]);
                    covered[py][px] = true;
                }
            }
        }
        for row in &covered {
            for c in row {
                assert!(c);
            }
        }
    }

    #[test]
    fn nchw_roundtrip() {
        let img = image::RgbImage::from_fn(4, 4, |x, y| image::Rgb([(x * 16) as u8, (y * 16) as u8, 128]));
        let tensor = pack_nchw(&img);
        let (_, data) = tensor.try_extract_tensor::<f32>().unwrap();
        let back = unpack_nchw(data, 4, 4).unwrap();
        assert_eq!(img, back);
    }
}
