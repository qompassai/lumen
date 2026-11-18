//! Regional conditioning for img2img (L7, `lumen_img2img_regional`).
//!
//! A unify pass over a composite (a body plus pasted elements) normally
//! re-draws every pixel under one global prompt, so composited elements
//! merge or smear into the body. The regional template keeps each element
//! in its own region in two ways, both core ComfyUI:
//!
//! - **Regional prompts.** Each region's prompt is encoded and applied only
//!   inside its mask (`ConditioningSetMask`), combined with the global
//!   prompt, so the sampler is told what belongs in that region.
//! - **Hold.** A fractional noise mask (`SetLatentNoiseMask`) denoises the
//!   image at the global strength except inside regions, where the
//!   strength is scaled by `1 - hold`: an element is integrated (shading,
//!   line weight) without losing its silhouette.
//!
//! Callers give boxes in source pixels; this module validates them and
//! rasterises the masks deterministically (integer geometry, an inward
//! linear feather, no randomness) into the two RGB PNGs the template loads:
//! `regions` R/G/B = regions 1-3 and `aux` R = noise mask, G = region 4.

use image::{ImageEncoder, RgbImage, codecs::png::PngEncoder};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::generation::admission::sha256_bytes;
use crate::generation::error::{AtelierError, ErrorCode, Result};
use crate::generation::template::REGION_SLOTS;

/// Feather ceiling, output pixels.
pub const FEATHER_MAX: u32 = 128;
/// Region conditioning strength range (inclusive).
pub const STRENGTH_MIN: f64 = 0.0;
pub const STRENGTH_MAX: f64 = 2.0;

/// One region of a regional img2img call.
#[derive(Debug, Clone, Deserialize, Serialize, JsonSchema, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RegionSpec {
    /// `[x0, y0, x1, y1]` in SOURCE pixels, half-open, inside the source.
    #[serde(rename = "box")]
    pub bbox: [u32; 4],
    /// What belongs in this region (applied only inside it). Nonempty.
    pub prompt: String,
    /// Share of global denoise withheld in the region, [0, 1]: 0 = re-drawn like the rest, 1 = left as composited. Default 0.6.
    pub hold: Option<f64>,
    /// Region conditioning strength in [0, 2]. Default 1.0.
    pub strength: Option<f64>,
    /// Inward feather of both masks, output pixels, <= 128. Default 8.
    pub feather_px: Option<u32>,
}

/// A validated region with its geometry in output pixels.
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Region {
    pub box_source: [u32; 4],
    pub box_output: [u32; 4],
    pub prompt: String,
    pub hold: f64,
    pub strength: f64,
    pub feather_px: u32,
}

/// The two rasterised mask images and their identities.
#[derive(Debug, Clone)]
pub struct MaskImages {
    pub regions_png: Vec<u8>,
    pub aux_png: Vec<u8>,
}

fn bad(message: impl Into<String>) -> AtelierError {
    AtelierError::new(ErrorCode::ArgumentInvalid, message)
}

/// Validate `specs` against the source and output geometry. The output
/// must keep the source aspect ratio (the template scales with a centre
/// crop, which would misalign the masks otherwise).
pub fn validate(
    specs: &[RegionSpec],
    source: (u32, u32),
    output: (u32, u32),
    prompt_bytes_max: usize,
) -> Result<Vec<Region>> {
    let (sw, sh) = source;
    let (ow, oh) = output;
    if specs.is_empty() || specs.len() > REGION_SLOTS {
        return Err(bad(format!("regions must hold 1..={REGION_SLOTS} entries")));
    }
    assert!(
        sw > 0 && sh > 0 && ow > 0 && oh > 0,
        "geometry is validated upstream"
    );
    // aspect: |ow/oh - sw/sh| within one output pixel of rounding
    let lhs = u64::from(ow) * u64::from(sh);
    let rhs = u64::from(oh) * u64::from(sw);
    if lhs.abs_diff(rhs) > u64::from(sh.max(sw)) {
        return Err(bad(format!(
            "output {ow}x{oh} does not keep the source aspect {sw}x{sh}; regions would misalign"
        )));
    }
    let mut out = Vec::with_capacity(specs.len());
    for (index, spec) in specs.iter().enumerate() {
        let n = index + 1;
        let [x0, y0, x1, y1] = spec.bbox;
        if x0 >= x1 || y0 >= y1 || x1 > sw || y1 > sh {
            return Err(bad(format!(
                "region {n} box {:?} must satisfy x0<x1<={sw}, y0<y1<={sh}",
                spec.bbox
            )));
        }
        if spec.prompt.trim().is_empty() || spec.prompt.len() > prompt_bytes_max {
            return Err(bad(format!(
                "region {n} prompt must be nonempty and <= {prompt_bytes_max} bytes"
            )));
        }
        let hold = spec.hold.unwrap_or(0.6);
        if !hold.is_finite() || !(0.0..=1.0).contains(&hold) {
            return Err(bad(format!("region {n} hold must be finite in [0, 1]")));
        }
        let strength = spec.strength.unwrap_or(1.0);
        if !strength.is_finite() || !(STRENGTH_MIN..=STRENGTH_MAX).contains(&strength) {
            return Err(bad(format!(
                "region {n} strength must be finite in [{STRENGTH_MIN}, {STRENGTH_MAX}]"
            )));
        }
        let feather_px = spec.feather_px.unwrap_or(8);
        if feather_px > FEATHER_MAX {
            return Err(bad(format!(
                "region {n} feather_px must be <= {FEATHER_MAX}"
            )));
        }
        let scale = |v: u32, num: u32, den: u32| -> u32 {
            u32::try_from(u64::from(v) * u64::from(num) / u64::from(den)).unwrap_or(u32::MAX)
        };
        let box_output = [
            scale(x0, ow, sw),
            scale(y0, oh, sh),
            scale(x1, ow, sw).max(scale(x0, ow, sw) + 1).min(ow),
            scale(y1, oh, sh).max(scale(y0, oh, sh) + 1).min(oh),
        ];
        out.push(Region {
            box_source: spec.bbox,
            box_output,
            prompt: spec.prompt.clone(),
            hold,
            strength,
            feather_px,
        });
    }
    Ok(out)
}

/// Mask value of `region` at output pixel centre (x, y), in [0, 1]: 1 deep
/// inside the box, a linear ramp over `feather_px` inward from its edges,
/// 0 outside.
pub fn coverage(region: &Region, x: u32, y: u32) -> f64 {
    let [x0, y0, x1, y1] = region.box_output;
    if x < x0 || x >= x1 || y < y0 || y >= y1 {
        return 0.0;
    }
    if region.feather_px == 0 {
        return 1.0;
    }
    let inner = [x - x0, x1 - 1 - x, y - y0, y1 - 1 - y]
        .into_iter()
        .min()
        .unwrap_or(0);
    ((f64::from(inner) + 0.5) / f64::from(region.feather_px)).min(1.0)
}

fn to_u8(value: f64) -> u8 {
    // value is in [0, 1] by construction
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn encode(image: &RgbImage) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    PngEncoder::new(&mut bytes)
        .write_image(
            image.as_raw(),
            image.width(),
            image.height(),
            image::ExtendedColorType::Rgb8,
        )
        .map_err(|cause| AtelierError::new(ErrorCode::OutputWriteFailed, cause.to_string()))?;
    Ok(bytes)
}

/// Rasterise the two mask images at the output size.
pub fn rasterise(regions: &[Region], output: (u32, u32)) -> Result<MaskImages> {
    assert!(!regions.is_empty() && regions.len() <= REGION_SLOTS);
    let (ow, oh) = output;
    let mut reg = RgbImage::new(ow, oh);
    let mut aux = RgbImage::new(ow, oh);
    for y in 0..oh {
        for x in 0..ow {
            let cov: Vec<f64> = regions.iter().map(|r| coverage(r, x, y)).collect();
            let get = |i: usize| cov.get(i).copied().unwrap_or(0.0);
            // noise mask: 1 - the strongest hold present at this pixel
            let held = regions
                .iter()
                .zip(&cov)
                .map(|(r, c)| r.hold * c)
                .fold(0.0_f64, f64::max);
            reg.put_pixel(
                x,
                y,
                image::Rgb([to_u8(get(0)), to_u8(get(1)), to_u8(get(2))]),
            );
            aux.put_pixel(x, y, image::Rgb([to_u8(1.0 - held), to_u8(get(3)), 0]));
        }
    }
    Ok(MaskImages {
        regions_png: encode(&reg)?,
        aux_png: encode(&aux)?,
    })
}

/// What a regional job records in its result and manifest.
#[derive(Debug, Clone, Serialize)]
pub struct RegionsRecord {
    pub regions: Vec<Region>,
    pub regions_image_sha256: String,
    pub aux_image_sha256: String,
    pub regions_image_upload: crate::generation::backend::UploadedImage,
    pub aux_image_upload: crate::generation::backend::UploadedImage,
    pub mask_rule: &'static str,
}

pub const MASK_RULE: &str = "regions_image R/G/B = regions 1-3, aux_image G = region 4: box coverage with an inward linear feather; aux_image R = noise mask = 1 - max(hold x coverage) (1 = full global denoise)";

/// sha256 helper re-exported for the service.
pub fn digest(bytes: &[u8]) -> String {
    sha256_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(bbox: [u32; 4]) -> RegionSpec {
        RegionSpec {
            bbox,
            prompt: "a pink wand".into(),
            hold: Some(0.5),
            strength: None,
            feather_px: Some(4),
        }
    }

    // ---- validation ------------------------------------------------------------------------------
    #[test]
    fn valid_region_scales_to_output() {
        let r = validate(
            &[spec([10, 20, 110, 220])],
            (100 * 2, 300),
            (100 * 2, 300),
            4096,
        )
        .unwrap();
        assert_eq!(r[0].box_output, [10, 20, 110, 220]);
        let r = validate(&[spec([10, 20, 110, 220])], (200, 300), (400, 600), 4096).unwrap();
        assert_eq!(r[0].box_output, [20, 40, 220, 440]);
        assert_eq!(r[0].strength, 1.0);
        assert_eq!(r[0].hold, 0.5);
    }

    #[test]
    fn four_regions_accepted() {
        let specs: Vec<RegionSpec> = (0..4).map(|i| spec([i * 10, 0, i * 10 + 5, 5])).collect();
        assert_eq!(
            validate(&specs, (100, 100), (100, 100), 4096)
                .unwrap()
                .len(),
            4
        );
    }

    #[test]
    fn coverage_is_one_inside_ramps_at_edge_zero_outside() {
        let r = &validate(&[spec([10, 10, 50, 50])], (100, 100), (100, 100), 4096).unwrap()[0];
        assert_eq!(coverage(r, 30, 30), 1.0);
        assert!(coverage(r, 10, 30) > 0.0 && coverage(r, 10, 30) < 0.5);
        assert_eq!(coverage(r, 9, 30), 0.0);
        assert_eq!(coverage(r, 50, 30), 0.0);
    }

    #[test]
    fn rasterise_is_deterministic_and_encodes_hold() {
        let r = validate(&[spec([10, 10, 50, 50])], (64, 64), (64, 64), 4096).unwrap();
        let a = rasterise(&r, (64, 64)).unwrap();
        let b = rasterise(&r, (64, 64)).unwrap();
        assert_eq!(a.regions_png, b.regions_png);
        assert_eq!(a.aux_png, b.aux_png);
        let aux = image::load_from_memory(&a.aux_png).unwrap().to_rgb8();
        assert_eq!(aux.get_pixel(0, 0)[0], 255); // outside: full denoise
        assert_eq!(aux.get_pixel(30, 30)[0], 128); // inside, hold 0.5
        let reg = image::load_from_memory(&a.regions_png).unwrap().to_rgb8();
        assert_eq!(reg.get_pixel(30, 30)[0], 255);
        assert_eq!(reg.get_pixel(30, 30)[1], 0); // slot 2 unused
    }

    // ---- adversarial -----------------------------------------------------------------------------
    #[test]
    fn empty_and_too_many_regions_refused() {
        assert!(validate(&[], (64, 64), (64, 64), 4096).is_err());
        let specs: Vec<RegionSpec> = (0..5).map(|i| spec([i, 0, i + 1, 1])).collect();
        assert!(validate(&specs, (64, 64), (64, 64), 4096).is_err());
    }

    #[test]
    fn degenerate_or_outside_box_refused() {
        assert!(validate(&[spec([10, 10, 10, 20])], (64, 64), (64, 64), 4096).is_err());
        assert!(validate(&[spec([20, 10, 10, 20])], (64, 64), (64, 64), 4096).is_err());
        assert!(validate(&[spec([0, 0, 65, 10])], (64, 64), (64, 64), 4096).is_err());
    }

    #[test]
    fn bad_hold_strength_feather_prompt_refused() {
        let mut s = spec([0, 0, 8, 8]);
        s.hold = Some(1.5);
        assert!(validate(&[s.clone()], (64, 64), (64, 64), 4096).is_err());
        s.hold = Some(f64::NAN);
        assert!(validate(&[s.clone()], (64, 64), (64, 64), 4096).is_err());
        let mut s = spec([0, 0, 8, 8]);
        s.strength = Some(-0.1);
        assert!(validate(&[s], (64, 64), (64, 64), 4096).is_err());
        let mut s = spec([0, 0, 8, 8]);
        s.feather_px = Some(FEATHER_MAX + 1);
        assert!(validate(&[s], (64, 64), (64, 64), 4096).is_err());
        let mut s = spec([0, 0, 8, 8]);
        s.prompt = "   ".into();
        assert!(validate(&[s.clone()], (64, 64), (64, 64), 4096).is_err());
        s.prompt = "x".repeat(10);
        assert!(validate(&[s], (64, 64), (64, 64), 5).is_err());
    }

    #[test]
    fn aspect_change_refused() {
        assert!(validate(&[spec([0, 0, 8, 8])], (64, 64), (64, 128), 4096).is_err());
    }

    #[test]
    fn unknown_field_refused() {
        let r: std::result::Result<RegionSpec, _> = serde_json::from_value(
            serde_json::json!({"box": [0, 0, 1, 1], "prompt": "x", "mask": "m.png"}),
        );
        assert!(r.is_err());
    }
}
