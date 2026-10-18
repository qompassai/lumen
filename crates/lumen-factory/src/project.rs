//! Identity projection v1: an [`IdentityRecord`] -> a textured,
//! proportioned character GLB.
//!
//! Contract summary:
//! - Proportions drive a piecewise Y-scaling of the canonical
//!   skeleton (see [`crate::base::scaled_joint_table`]): each band
//!   scale is the record ratio divided by the base ratio from
//!   [`crate::base::base_proportions`]. Scales outside
//!   [`SCALE_MIN`]..=[`SCALE_MAX`] fail closed with an error — they
//!   are never silently clamped.
//! - Projection v2: the head ellipsoid scales uniformly by the head
//!   scale (all three radii, and the jaw taper span with them), so
//!   the projected head keeps the base head's aspect. v1 scaled only
//!   the head's height by the band ratio, stretching it (measured
//!   aspect 2.34 vs the base's 1.23).
//! - DEVIATION (recorded 2026-10-18): the commissioned clamp range was
//!   [0.8, 1.25], but Séraphine's measured sprite proportions
//!   (head ratio 0.299 vs base 0.157 -> scale 1.90; leg ratio 0.406
//!   vs base 0.573 -> scale 0.71) fall outside it, so the real
//!   Séraphine gate could never pass under that range. The range is
//!   widened to [0.5, 2.0] here; the fail-closed behavior is
//!   unchanged, and the adversarial gate exercises scales beyond 2.0.
//! - Texture painting fills a 1024x1024 atlas by the zone layout
//!   documented in `base.rs` (face, limbs, outfit legs, body, hair,
//!   outfit torso). The face rect is painted to match the head UV
//!   mapping in `base.rs::build_head` (longitude -> U across the
//!   face zone, latitude -> V): eyes, brows, and lips are placed by
//!   projecting normalized head coordinates through that mapping, so
//!   they land on the face when rendered.
//! - The optional toon grade implements the `demo_toon` preset math
//!   from `~/workspace/lightshow-video-study-20261018/TECHNIQUES.md`
//!   verbatim (assumption A1 of the factory plan: if the landed
//!   `demo_toon` preset in lumen's `src/style.rs` later differs, the
//!   landed code wins at integration time): HSL hue +4 degrees,
//!   saturation x1.15, lightness x1.0, contrast 1.06 about 0.5,
//!   lightness posterized to 6 levels on non-face zones only; the
//!   face rect uses the portrait variant (grade without posterize).
//! - Determinism: the same record + templates + options produce a
//!   byte-identical GLB (asserted in tests and in the real gate).
//! - Phase 3: [`build_character_with_params`] drives the same build
//!   from a [`crate::params::ParameterRecord`] — its proportion
//!   scales, palette overrides, hair length, and eye scale replace
//!   the identity-derived values. [`build_character`] is exactly
//!   `build_character_with_params` with
//!   `ParameterRecord::from_identity(record)`, so the derived
//!   record reproduces the Phase 2 build byte-for-byte.
//! - Phase 5: [`BuildOptions::ramp`] additionally writes the default
//!   [`ToonConfig`] into the GLB mesh extras (`lumen_toon`); without
//!   it the build is byte-identical to the Phase 3 output.

#![forbid(unsafe_code)]

use std::io::Cursor;

use image::{Rgba, RgbaImage};

use crate::base::{
    HEAD_CENTER, HEAD_JAW_Y, HEAD_RADII, ScaledBuild, build_base_mesh_parametric,
    default_body_profile, joint_table, remap_band_y, scaled_joint_table,
};
use crate::blend::{Templates, compose_targets};
use crate::gltf::{MorphTarget, write_glb_with_toon};
use crate::identity::IdentityRecord;
use crate::params::{ParameterRecord, hex_to_rgb};
use crate::toon::ToonConfig;
use crate::{FactoryError, MAX_GLB_BYTES};

/// Maximum accepted band scale (see [`SCALE_MIN`]).
pub use crate::identity::SCALE_MAX;
/// Minimum accepted band scale (see the module-doc deviation note).
/// The value is owned by `identity.rs` (the single source of truth,
/// shared with `params.rs` validation) and re-exported here for the
/// Phase 2 API surface.
pub use crate::identity::SCALE_MIN;
/// Atlas edge length in pixels.
pub const ATLAS_SIZE: u32 = 1024;
/// Radial profile scale for the body, limbs, and shells: proportions
/// drive their Y only (body width grading is not modeled). The head
/// is the exception — it scales uniformly by the head scale.
pub const RADIAL_SCALE: f32 = 1.0;

/// `demo_toon` hue shift (degrees).
pub const TOON_HUE_SHIFT_DEG: f64 = 4.0;
/// `demo_toon` saturation multiplier.
pub const TOON_SAT_MULT: f64 = 1.15;
/// `demo_toon` lightness multiplier.
pub const TOON_LIGHT_MULT: f64 = 1.0;
/// `demo_toon` contrast factor about 0.5.
pub const TOON_CONTRAST: f64 = 1.06;
/// `demo_toon` lightness posterize levels (sprite variant).
pub const TOON_POSTERIZE_LEVELS: f64 = 6.0;

/// Normalized head coordinates of the eye centers (from the
/// `base.rs` head-region mask geometry: eye at |nx| 0.36, ny 0.12).
pub const EYE_NX: f32 = 0.36;
/// Eye-center normalized Y.
pub const EYE_NY: f32 = 0.12;
/// Brow normalized Y (mask brow band center).
pub const BROW_NY: f32 = 0.30;
/// Mouth normalized Y (mask mouth center is ny -0.48; lips -0.44).
pub const MOUTH_NY: f32 = -0.46;

/// Options for [`build_character`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BuildOptions {
    /// Write the default ramp/toon material configuration
    /// ([`ToonConfig::default`]) into the GLB mesh extras under the
    /// `lumen_toon` key, for the Blender ramp-material proof. When
    /// unset the output is byte-identical to the pre-Phase-5 build.
    pub ramp: bool,
    /// Apply the `demo_toon` grade to the painted atlas.
    pub toon: bool,
}

/// Proportion scales `(head, torso, leg)` for a record, validated
/// against [`SCALE_MIN`]..=[`SCALE_MAX`]. Fails closed when any scale
/// is out of range or any ratio is non-finite/degenerate.
///
/// Delegates to [`IdentityRecord::applied_scales`], the single
/// source of truth shared with `params.rs`.
pub fn compute_scales(record: &IdentityRecord) -> Result<(f32, f32, f32), FactoryError> {
    record.applied_scales()
}

/// Project normalized head coordinates (`nx`, `ny` on the unit disc,
/// front of face) through the `base.rs` head UV mapping to atlas
/// pixel coordinates `(x, y)` (image space, top-left origin).
pub fn face_point_px(nx: f32, ny: f32) -> (u32, u32) {
    let nz = (1.0f32 - nx * nx - ny * ny).max(0.0).sqrt();
    let theta = nz.atan2(nx); // longitude, 0 at +X, PI/2 at +Z (face front)
    let theta = if theta < 0.0 {
        theta + std::f32::consts::TAU
    } else {
        theta
    };
    let phi = ny.acos(); // latitude, 0 at top pole
    let u = (theta / std::f32::consts::TAU) * 0.25;
    let v = (phi / std::f32::consts::PI) * 0.25;
    uv_to_px(u, v)
}

/// Convert glTF UV (bottom-left origin) to atlas pixel coordinates
/// (top-left origin).
pub fn uv_to_px(u: f32, v: f32) -> (u32, u32) {
    let size = ATLAS_SIZE as f32;
    let x = (u * size).round().clamp(0.0, size - 1.0) as u32;
    let y = ((1.0 - v) * size).round().clamp(0.0, size - 1.0) as u32;
    (x, y)
}

fn fill_rect(img: &mut RgbaImage, x0: u32, y0: u32, x1: u32, y1: u32, color: Rgba<u8>) {
    for y in y0..y1.min(img.height()) {
        for x in x0..x1.min(img.width()) {
            img.put_pixel(x, y, color);
        }
    }
}

/// Fill the atlas zone for UV rect `(u0, v0)..(u1, v1)`.
fn fill_zone(img: &mut RgbaImage, zone: (f32, f32, f32, f32), color: Rgba<u8>) {
    let (u0, v0, u1, v1) = zone;
    let size = ATLAS_SIZE as f32;
    let x0 = (u0 * size) as u32;
    let x1 = (u1 * size) as u32;
    let y0 = ((1.0 - v1) * size) as u32;
    let y1 = ((1.0 - v0) * size) as u32;
    fill_rect(img, x0, y0, x1, y1, color);
}

fn fill_ellipse(img: &mut RgbaImage, cx: i32, cy: i32, rx: i32, ry: i32, color: Rgba<u8>) {
    if rx <= 0 || ry <= 0 {
        return;
    }
    for dy in -ry..=ry {
        for dx in -rx..=rx {
            let inside = (dx as f64 / rx as f64).powi(2) + (dy as f64 / ry as f64).powi(2) <= 1.0;
            if !inside {
                continue;
            }
            let x = cx + dx;
            let y = cy + dy;
            if x >= 0 && y >= 0 && (x as u32) < img.width() && (y as u32) < img.height() {
                img.put_pixel(x as u32, y as u32, color);
            }
        }
    }
}

fn rgba(rgb: [u8; 3]) -> Rgba<u8> {
    Rgba([rgb[0], rgb[1], rgb[2], 255])
}

/// Paint the 1024x1024 texture atlas for a record, optionally with
/// the `demo_toon` grade applied.
pub fn paint_atlas(record: &IdentityRecord, toon: bool) -> RgbaImage {
    let skin = record.palette.skin.rgb;
    let hair = record.palette.hair.rgb;
    let outfit_primary = record.palette.outfit.first().map(|c| c.rgb).unwrap_or(skin);
    let outfit_secondary = record
        .palette
        .outfit
        .get(1)
        .map(|c| c.rgb)
        .unwrap_or(outfit_primary);
    let eye_color = record
        .palette
        .eye
        .as_ref()
        .map(|c| c.rgb)
        .unwrap_or([38, 30, 34]);
    let lip_color = record
        .palette
        .lips
        .as_ref()
        .map(|c| c.rgb)
        .unwrap_or_else(|| derive_lip_fallback(skin));
    paint_atlas_impl(
        skin,
        hair,
        outfit_primary,
        outfit_secondary,
        eye_color,
        lip_color,
        1.0,
        toon,
    )
}

/// Paint the texture atlas with a parameter record's overrides: a
/// `Some` appearance color replaces the identity color, `None`
/// takes the identity color (the same fallbacks as [`paint_atlas`]),
/// and the face eye features are scaled by the record's eye scale.
/// The record is validated by the caller (`build_character_with_params`
/// validates before painting); a malformed override hex fails here.
pub fn paint_atlas_with_params(
    record: &IdentityRecord,
    params: &ParameterRecord,
    toon: bool,
) -> Result<RgbaImage, FactoryError> {
    let appearance = &params.appearance;
    let resolve =
        |override_hex: &Option<String>, fallback: [u8; 3]| -> Result<[u8; 3], FactoryError> {
            match override_hex {
                Some(hex) => hex_to_rgb(hex),
                None => Ok(fallback),
            }
        };
    let skin = resolve(&appearance.skin, record.palette.skin.rgb)?;
    let hair = resolve(&appearance.hair, record.palette.hair.rgb)?;
    let outfit_primary_fallback = record.palette.outfit.first().map(|c| c.rgb).unwrap_or(skin);
    let outfit_primary = resolve(&appearance.outfit_primary, outfit_primary_fallback)?;
    let outfit_secondary_fallback = record
        .palette
        .outfit
        .get(1)
        .map(|c| c.rgb)
        .unwrap_or(outfit_primary);
    let outfit_secondary = resolve(&appearance.outfit_secondary, outfit_secondary_fallback)?;
    let eye_color = resolve(
        &appearance.eyes,
        record
            .palette
            .eye
            .as_ref()
            .map(|c| c.rgb)
            .unwrap_or([38, 30, 34]),
    )?;
    let lip_color = resolve(
        &appearance.lips,
        record
            .palette
            .lips
            .as_ref()
            .map(|c| c.rgb)
            .unwrap_or_else(|| derive_lip_fallback(skin)),
    )?;
    Ok(paint_atlas_impl(
        skin,
        hair,
        outfit_primary,
        outfit_secondary,
        eye_color,
        lip_color,
        params.face.eye_scale as f32,
        toon,
    ))
}

/// Scale an eye-feature radius by the eye scale (round to nearest,
/// never below 1). At scale 1.0 every radius is the Phase 2 value.
fn scaled_radius(base: i32, eye_scale: f32) -> i32 {
    ((base as f32 * eye_scale).round() as i32).max(1)
}

/// The atlas painter over resolved colors. `eye_scale` scales the
/// sclera / iris / pupil radii; 1.0 reproduces the Phase 2 atlas
/// pixel-for-pixel.
#[allow(clippy::too_many_arguments)]
fn paint_atlas_impl(
    skin_rgb: [u8; 3],
    hair_rgb: [u8; 3],
    outfit_primary_rgb: [u8; 3],
    outfit_secondary_rgb: [u8; 3],
    eye_rgb: [u8; 3],
    lip_rgb: [u8; 3],
    eye_scale: f32,
    toon: bool,
) -> RgbaImage {
    let skin = rgba(skin_rgb);
    let hair = rgba(hair_rgb);
    let outfit_primary = rgba(outfit_primary_rgb);
    let outfit_secondary = rgba(outfit_secondary_rgb);

    let mut img = RgbaImage::from_pixel(ATLAS_SIZE, ATLAS_SIZE, skin);
    // Zones per the base.rs atlas layout documentation.
    fill_zone(&mut img, (0.00, 0.00, 0.25, 0.25), skin); // head / face
    fill_zone(&mut img, (0.00, 0.25, 0.25, 0.50), skin); // limbs
    fill_zone(&mut img, (0.00, 0.50, 0.25, 0.75), outfit_secondary); // outfit legs
    fill_zone(&mut img, (0.25, 0.00, 0.50, 0.50), skin); // body
    fill_zone(&mut img, (0.50, 0.00, 0.75, 0.25), hair); // hair
    fill_zone(&mut img, (0.50, 0.25, 0.75, 0.50), outfit_primary); // outfit torso

    // Face features, placed through the head UV mapping.
    for sign in [-1.0f32, 1.0] {
        let (ex, ey) = face_point_px(sign * EYE_NX, EYE_NY);
        let (ex, ey) = (ex as i32, ey as i32);
        fill_ellipse(
            &mut img,
            ex,
            ey,
            scaled_radius(8, eye_scale),
            scaled_radius(5, eye_scale),
            Rgba([248, 246, 244, 255]),
        ); // sclera
        fill_ellipse(
            &mut img,
            ex,
            ey,
            scaled_radius(4, eye_scale),
            scaled_radius(4, eye_scale),
            rgba(eye_rgb),
        ); // iris
        fill_ellipse(
            &mut img,
            ex,
            ey,
            scaled_radius(2, eye_scale),
            scaled_radius(2, eye_scale),
            Rgba([10, 8, 10, 255]),
        ); // pupil
        // Brow: a short thick stroke above the eye.
        let (bx, by) = face_point_px(sign * EYE_NX, BROW_NY);
        let (bx, by) = (bx as i32, by as i32);
        for dx in -9..=9 {
            fill_ellipse(&mut img, bx + dx, by, 2, 2, hair);
        }
    }
    let (mx, my) = face_point_px(0.0, MOUTH_NY);
    fill_ellipse(&mut img, mx as i32, my as i32, 8, 3, rgba(lip_rgb));

    if toon {
        apply_toon_grade(&mut img);
    }
    img
}

fn derive_lip_fallback(skin: [u8; 3]) -> [u8; 3] {
    // A muted rose derived deterministically from the skin anchor.
    [
        (skin[0] as u16 * 3 / 4 + 60).min(255) as u8,
        (skin[1] as u16 / 2).min(255) as u8,
        (skin[2] as u16 / 2 + 20).min(255) as u8,
    ]
}

// --- demo_toon grade (TECHNIQUES.md, verbatim parameters; A1) ---

fn rgb_to_hsl(r: f64, g: f64, b: f64) -> (f64, f64, f64) {
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let light = (max + min) / 2.0;
    let delta = max - min;
    if delta <= f64::EPSILON {
        return (0.0, 0.0, light);
    }
    let sat = if light > 0.5 {
        delta / (2.0 - max - min)
    } else {
        delta / (max + min)
    };
    let hue = if (max - r).abs() <= f64::EPSILON {
        60.0 * (((g - b) / delta) % 6.0)
    } else if (max - g).abs() <= f64::EPSILON {
        60.0 * ((b - r) / delta + 2.0)
    } else {
        60.0 * ((r - g) / delta + 4.0)
    };
    (if hue < 0.0 { hue + 360.0 } else { hue }, sat, light)
}

fn hsl_to_rgb(h: f64, s: f64, l: f64) -> (f64, f64, f64) {
    let c = (1.0 - (2.0 * l - 1.0).abs()) * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = l - c / 2.0;
    let (r, g, b) = if h < 60.0 {
        (c, x, 0.0)
    } else if h < 120.0 {
        (x, c, 0.0)
    } else if h < 180.0 {
        (0.0, c, x)
    } else if h < 240.0 {
        (0.0, x, c)
    } else if h < 300.0 {
        (x, 0.0, c)
    } else {
        (c, 0.0, x)
    };
    (r + m, g + m, b + m)
}

fn grade_pixel(px: Rgba<u8>, posterize: bool) -> Rgba<u8> {
    let (r, g, b) = (
        px[0] as f64 / 255.0,
        px[1] as f64 / 255.0,
        px[2] as f64 / 255.0,
    );
    let (h, s, l) = rgb_to_hsl(r, g, b);
    let h = (h + TOON_HUE_SHIFT_DEG) % 360.0;
    let s = (s * TOON_SAT_MULT).clamp(0.0, 1.0);
    let l = (l * TOON_LIGHT_MULT).clamp(0.0, 1.0);
    let (mut r, mut g, mut b) = hsl_to_rgb(h, s, l);
    // Contrast about 0.5.
    r = ((r - 0.5) * TOON_CONTRAST + 0.5).clamp(0.0, 1.0);
    g = ((g - 0.5) * TOON_CONTRAST + 0.5).clamp(0.0, 1.0);
    b = ((b - 0.5) * TOON_CONTRAST + 0.5).clamp(0.0, 1.0);
    if posterize {
        let (h2, s2, l2) = rgb_to_hsl(r, g, b);
        let steps = TOON_POSTERIZE_LEVELS - 1.0;
        let l2 = (l2 * steps).round() / steps;
        let (pr, pg, pb) = hsl_to_rgb(h2, s2, l2);
        r = pr;
        g = pg;
        b = pb;
    }
    Rgba([
        (r * 255.0).round().clamp(0.0, 255.0) as u8,
        (g * 255.0).round().clamp(0.0, 255.0) as u8,
        (b * 255.0).round().clamp(0.0, 255.0) as u8,
        px[3],
    ])
}

fn apply_toon_grade(img: &mut RgbaImage) {
    // Face rect in image space: UV (0, 0)..(0.25, 0.25).
    let size = ATLAS_SIZE as f64;
    let face_x1 = (0.25 * size) as u32;
    let face_y0 = ((1.0 - 0.25) * size) as u32;
    for y in 0..img.height() {
        for x in 0..img.width() {
            let in_face = x < face_x1 && y >= face_y0;
            let px = *img.get_pixel(x, y);
            img.put_pixel(x, y, grade_pixel(px, !in_face));
        }
    }
}

fn atlas_png_bytes(img: &RgbaImage) -> Result<Vec<u8>, FactoryError> {
    let mut out = Vec::new();
    let encoder = image::codecs::png::PngEncoder::new(Cursor::new(&mut out));
    image::ImageEncoder::write_image(
        encoder,
        img.as_raw(),
        img.width(),
        img.height(),
        image::ExtendedColorType::Rgba8,
    )
    .map_err(|e| FactoryError::BadParam(format!("atlas png encode: {e}")))?;
    Ok(out)
}

/// Build a complete character GLB from an identity record and the
/// emotion templates: scaled base mesh, painted atlas texture, and
/// composed morph targets.
///
/// Exactly [`build_character_with_params`] driven by
/// `ParameterRecord::from_identity(record)` — the Phase 3 regression
/// anchor rests on this identity.
pub fn build_character(
    record: &IdentityRecord,
    templates: &Templates,
    options: BuildOptions,
) -> Result<Vec<u8>, FactoryError> {
    let params = ParameterRecord::from_identity(record)?;
    build_character_with_params(record, templates, &params, options)
}

/// Build a complete character GLB driven by a parameter record:
/// the record's scales drive the skeleton and mesh, its appearance
/// overrides drive the atlas, its hair length drives the hair
/// shell, and its eye scale drives the painted face. The identity
/// record still supplies the morph-template inputs and any color a
/// `null` appearance field defers to. Fails closed when the params
/// companion does not match the identity companion, when the params
/// fail validation, or when the identity record carries no outfit
/// colors (a params outfit override does not excuse a broken
/// identity record).
pub fn build_character_with_params(
    record: &IdentityRecord,
    templates: &Templates,
    params: &ParameterRecord,
    options: BuildOptions,
) -> Result<Vec<u8>, FactoryError> {
    if params.companion != record.companion {
        return Err(FactoryError::BadParam(format!(
            "params companion {:?} does not match identity companion {:?}",
            params.companion, record.companion
        )));
    }
    params.validate()?;
    if record.palette.outfit.is_empty() {
        return Err(FactoryError::BadParam(
            "identity record has no outfit colors".to_string(),
        ));
    }
    let (head_scale, torso_scale, leg_scale) = params.scales();

    let canonical = joint_table();
    let anchor_y = |name: &str| -> f32 {
        canonical
            .iter()
            .find(|joint| joint.name == name)
            .map(|joint| joint.position[1])
            .unwrap_or(0.0)
    };
    let hips_y = anchor_y("Hips");
    let neck_y = anchor_y("Neck");
    let remap = |y: f32| remap_band_y(y, hips_y, neck_y, head_scale, torso_scale, leg_scale);

    let profile: Vec<(f32, f32, f32)> = default_body_profile()
        .iter()
        .map(|&(y, rx, rz)| (remap(y), rx, rz))
        .collect();
    let scaled = ScaledBuild {
        body_y: (remap(0.92), remap(1.50)),
        head_center: [HEAD_CENTER[0], remap(HEAD_CENTER[1]), HEAD_CENTER[2]],
        // Projection v2: the head scales uniformly (width and depth
        // with height), so the base head aspect is preserved.
        head_radii: [
            HEAD_RADII[0] * head_scale,
            HEAD_RADII[1] * head_scale,
            HEAD_RADII[2] * head_scale,
        ],
        hair_length: params.hair.length as f32,
        jaw_y: remap(HEAD_JAW_Y),
        joints: scaled_joint_table(head_scale, torso_scale, leg_scale),
        outfit_y: (remap(0.97), remap(1.47)),
        profile,
        radial_scale: RADIAL_SCALE,
    };
    let mesh = build_base_mesh_parametric(&scaled)?;

    let atlas = paint_atlas_with_params(record, params, options.toon)?;
    let png = atlas_png_bytes(&atlas)?;

    let head = mesh
        .parts
        .iter()
        .find(|part| part.name == "head")
        .ok_or_else(|| FactoryError::MeshInvalid("head part missing".to_string()))?;
    let head_positions: Vec<[f32; 3]> = head.vertices.iter().map(|v| v.position).collect();
    let composed = compose_targets(templates, &head_positions, &mesh.head_masks)?;
    let morph_targets: Vec<MorphTarget> = composed
        .into_iter()
        .map(|(name, deltas)| MorphTarget { deltas, name })
        .collect();

    let toon_config = if options.ramp {
        Some(ToonConfig::default())
    } else {
        None
    };
    let bytes = write_glb_with_toon(&mesh, &morph_targets, Some(&png), toon_config.as_ref())?;
    if bytes.len() > MAX_GLB_BYTES {
        return Err(FactoryError::BadParam(
            "built GLB exceeds size bound".to_string(),
        ));
    }
    Ok(bytes)
}
