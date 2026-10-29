//! GAP-11-2 reconciliation gate: the eye-centre span exists in
//! two spaces and this file pins both to the single source of
//! truth (`base::EYE_CENTER_NX` / `EYE_CENTER_NY`).
//!
//! - Deform/paint space: the lid/brow region centres and the
//!   painted eye centres sit at ±0.36 of the head-ellipsoid
//!   half-width (nx), i.e. a span share of 0.36 of the head width.
//! - The Canon-01/A0 frontal band's ≈0.70 divides the same centre
//!   distance by the colour-segmented SKIN-face width — a narrower
//!   denominator (the face inside the hairline), not a different
//!   eye position. The two numbers are consistent iff
//!   face_width ≈ 0.51 × head_width in the source art; no estate
//!   record measures that ratio, so the band is NOT renumbered
//!   (see the `EYE_CENTER_NX` doc in base.rs).
//!
//! What this gate CAN and does verify end-to-end: the deform
//! regions peak where the constant says, and the painted eyes of
//! the worked character sit at the same normalized centres — so
//! the number that positions the lids is the number that
//! positions the visible eyes.

use std::path::PathBuf;

use lumen_factory::base::{EYE_CENTER_NX, EYE_CENTER_NY, HEAD_CENTER, HEAD_RADII, build_base_mesh};
use lumen_factory::identity::load_record;
use lumen_factory::project::{EYE_NX, EYE_NY, face_point_px, paint_atlas};

fn asset_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join(name)
}

#[test]
fn one_source_of_truth_for_the_eye_centre() {
    assert_eq!(
        EYE_NX, EYE_CENTER_NX,
        "project re-exports the base constant"
    );
    assert_eq!(EYE_NY, EYE_CENTER_NY);
    assert_eq!(EYE_CENTER_NX, 0.36);
    assert_eq!(EYE_CENTER_NY, 0.12);
}

#[test]
fn lid_region_weight_peaks_at_the_eye_centre() {
    // The deform side: the upper-lid field's analytic centre is
    // (EYE_CENTER_NX, EYE_CENTER_NY + 0.10) in head-ellipsoid
    // normalized coordinates. The shipped head is a coarse grid
    // (270 vertices; on the eye row the front vertices sit at
    // nx = ±0.169 / ±0.487 / ±0.747 — none at 0.36), so the gate
    // is stated at grid resolution: the strongest front-right lid
    // weight must sit on the front-right vertex CLOSEST to the
    // analytic centre. A field centred anywhere else fails.
    let mesh = build_base_mesh().expect("base mesh");
    let head = mesh
        .parts
        .iter()
        .find(|p| p.name == "head")
        .expect("head part");
    assert_eq!(head.vertices.len(), mesh.head_masks.len());
    let lid_centre = (EYE_CENTER_NX, EYE_CENTER_NY + 0.10);
    let mut argmax: Option<(f32, f32)> = None; // (weight, dist to centre)
    let mut nearest: Option<f32> = None; // min dist to centre
    for (vertex, mask) in head.vertices.iter().zip(mesh.head_masks.iter()) {
        let nx = (vertex.position[0] - HEAD_CENTER[0]) / HEAD_RADII[0];
        let ny = (vertex.position[1] - HEAD_CENTER[1]) / HEAD_RADII[1];
        let nz = (vertex.position[2] - HEAD_CENTER[2]) / HEAD_RADII[2];
        if nz < 0.5 || nx <= 0.0 {
            continue; // front-right quadrant only
        }
        let dist = ((nx - lid_centre.0).powi(2) + (ny - lid_centre.1).powi(2)).sqrt();
        if nearest.is_none_or(|d| dist < d) {
            nearest = Some(dist);
        }
        if argmax.is_none_or(|(w, _)| mask.upper_lid > w) {
            argmax = Some((mask.upper_lid, dist));
        }
    }
    let (weight, argmax_dist) = argmax.expect("a front-right lid vertex exists");
    let nearest_dist = nearest.expect("a front-right vertex exists");
    // The peak magnitude is whatever the discrete vertex set
    // samples of the analytic field (two gaussians multiplied);
    // 0.35 is the measured maximum over the shipped head.
    assert!(weight > 0.25, "lid peak weight sane: {weight}");
    assert!(
        (argmax_dist - nearest_dist).abs() < 0.01,
        "lid weight must peak on the vertex closest to the centre: \
         peak at dist {argmax_dist}, nearest vertex at {nearest_dist}"
    );
}

#[test]
fn painted_eye_centroids_match_the_deform_centres() {
    // The render side: Séraphine's painted eyes (non-skin pixels
    // along the eye row of her atlas) centroid at the same
    // normalized centres the deform regions use. Span share of the
    // head width = (nx_R − nx_L) / 2 ≈ 0.36.
    let record = load_record(&asset_path("seraphine.identity.json")).expect("identity record");
    let atlas = paint_atlas(&record, false);
    let skin = record.palette.skin.rgb;
    let painted = |nx: f32| {
        let (x, y) = face_point_px(nx, EYE_NY);
        atlas.get_pixel(x, y).0[0..3] != skin
    };
    let centroid = |lo: f32, hi: f32| -> f32 {
        let mut sum = 0.0f32;
        let mut count = 0usize;
        let mut nx = lo;
        while nx <= hi {
            if painted(nx) {
                sum += nx;
                count += 1;
            }
            nx += 0.002;
        }
        assert!(count > 10, "an eye must be painted in [{lo}, {hi}]");
        sum / count as f32
    };
    let right = centroid(0.05, 0.75);
    let left = centroid(-0.75, -0.05);
    assert!(
        (right - EYE_CENTER_NX).abs() < 0.05,
        "right eye centroid {right} vs centre {EYE_CENTER_NX}"
    );
    assert!(
        (left + EYE_CENTER_NX).abs() < 0.05,
        "left eye centroid {left} vs centre -{EYE_CENTER_NX}"
    );
    let span_share = (right - left) / 2.0;
    assert!(
        (span_share - EYE_CENTER_NX).abs() < 0.04,
        "painted span share {span_share} of head width vs {EYE_CENTER_NX}"
    );
}
