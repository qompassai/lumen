//! Raster rendering for staged scenes, ported from the
//! evaluator seam's two-body scene driver: each puppet is
//! rendered by the evaluator of record under its solved
//! placement (the scene transform the rig-core delta
//! carries), composited in actor order over a dark ground,
//! then annotated with the ground line, engaged-contact
//! discs, and root discs. The annotation vocabulary is the
//! verdict-pack convention (red = contact hand, white =
//! solved target point, yellow = root).

use image::{Rgb, RgbImage, RgbaImage};
use lumen_engine_core::contact::{endpoint_world, ContactDef, EvaluatedMesh, FrameRead, Placement};
use lumen_rig_core::PuppetEval;

/// Puppet-space scene transform for a solved placement:
/// rotation -theta, translation (T.x - CX, -T.y + CY) for
/// scene centre (CX, CY).
pub fn scene_transform(placement: &Placement, centre: [f64; 2]) -> ([f32; 2], f32) {
    (
        [
            (placement.translation[0] - centre[0]) as f32,
            (-placement.translation[1] + centre[1]) as f32,
        ],
        -placement.rotation_degrees as f32,
    )
}

/// Composite both puppets of a solved frame into one
/// canvas (actor order 0 then 1).
#[allow(clippy::too_many_arguments)]
pub fn composite(
    eval_a: &mut PuppetEval,
    eval_b: &mut PuppetEval,
    read: &FrameRead,
    centre: [f64; 2],
    width: usize,
    height: usize,
    scale: f32,
) -> RgbaImage {
    let (ta, ra) = scene_transform(&read.placements[0], centre);
    eval_a.set_scene_transform(ta, ra);
    let frame_a = eval_a.render(width, height, scale);
    let (tb, rb) = scene_transform(&read.placements[1], centre);
    eval_b.set_scene_transform(tb, rb);
    let frame_b = eval_b.render(width, height, scale);
    eval_a.clear_scene_transform();
    eval_b.clear_scene_transform();
    let mut out =
        RgbaImage::from_pixel(width as u32, height as u32, image::Rgba([24, 24, 28, 255]));
    for (i, px) in frame_a.as_chunks::<4>().0.iter().enumerate() {
        blend_over(&mut out, i, px);
    }
    for (i, px) in frame_b.as_chunks::<4>().0.iter().enumerate() {
        blend_over(&mut out, i, px);
    }
    out
}

fn blend_over(out: &mut RgbaImage, index: usize, src: &[u8; 4]) {
    let w = out.width() as usize;
    let (x, y) = ((index % w) as u32, (index / w) as u32);
    let dst = out.get_pixel(x, y);
    let sa = f32::from(src[3]) / 255.0;
    let da = f32::from(dst[3]) / 255.0;
    let oa = sa + da * (1.0 - sa);
    if oa <= 0.0 {
        return;
    }
    let mut px = [0u8; 4];
    for c in 0..3 {
        let v = (f32::from(src[c]) * sa + f32::from(dst[c]) * da * (1.0 - sa)) / oa;
        px[c] = v.round().clamp(0.0, 255.0) as u8;
    }
    px[3] = (oa * 255.0).round() as u8;
    out.put_pixel(x, y, image::Rgba(px));
}

/// Scene point -> canvas pixel (float), for annotations.
pub fn to_px(p: [f64; 2], centre: [f64; 2], width: usize, height: usize, scale: f32) -> (f64, f64) {
    (
        (p[0] - centre[0]) * f64::from(scale) + f64::from(width as u32) / 2.0,
        f64::from(height as u32) / 2.0 - (p[1] - centre[1]) * f64::from(scale),
    )
}

#[allow(clippy::too_many_arguments)]
fn disc(
    img: &mut RgbImage,
    centre: [f64; 2],
    radius_units: f64,
    color: Rgb<u8>,
    scene_centre: [f64; 2],
    canvas_w: usize,
    canvas_h: usize,
    scale: f32,
) {
    let (cx, cy) = to_px(centre, scene_centre, canvas_w, canvas_h, scale);
    let r = radius_units * f64::from(scale);
    let x0 = (cx - r).floor().max(0.0) as u32;
    let x1 = (cx + r).ceil().min(f64::from(canvas_w as u32) - 1.0) as u32;
    let y0 = (cy - r).floor().max(0.0) as u32;
    let y1 = (cy + r).ceil().min(f64::from(canvas_h as u32) - 1.0) as u32;
    for y in y0..=y1 {
        for x in x0..=x1 {
            let dx = f64::from(x) - cx;
            let dy = f64::from(y) - cy;
            if dx * dx + dy * dy <= r * r {
                img.put_pixel(x, y, color);
            }
        }
    }
}

/// Annotate a composited frame: ground line at scene
/// y = 0, one disc pair per engaged contact (hand point
/// red, solved target point white), one disc per root
/// (yellow).
#[allow(clippy::too_many_arguments)]
pub fn annotate(
    img: &mut RgbImage,
    read: &FrameRead,
    geo_a: &[EvaluatedMesh],
    geo_b: &[EvaluatedMesh],
    contacts: &[ContactDef],
    centre: [f64; 2],
    canvas_w: usize,
    canvas_h: usize,
    scale: f32,
) {
    let (_, gy) = to_px([0.0, 0.0], centre, canvas_w, canvas_h, scale);
    let gy = gy.round() as u32;
    if gy < canvas_h as u32 {
        for x in 0..canvas_w as u32 {
            img.put_pixel(x, gy, Rgb([110, 110, 110]));
        }
    }
    for (index, contact) in contacts.iter().enumerate() {
        if !read.contacts[index].engaged {
            continue;
        }
        let a_point = endpoint_world(geo_a, &read.placements[0], &contact.a);
        disc(
            img,
            a_point,
            26.0,
            Rgb([255, 60, 60]),
            centre,
            canvas_w,
            canvas_h,
            scale,
        );
        let b_local = endpoint_world(geo_b, &Placement::IDENTITY, &contact.b);
        let fixed = read.placements[1].apply([
            b_local[0] + contact.offset[0],
            b_local[1] + contact.offset[1],
        ]);
        disc(
            img,
            fixed,
            17.0,
            Rgb([255, 255, 255]),
            centre,
            canvas_w,
            canvas_h,
            scale,
        );
    }
    for root in &read.roots {
        disc(
            img,
            *root,
            20.0,
            Rgb([255, 220, 60]),
            centre,
            canvas_w,
            canvas_h,
            scale,
        );
    }
}
