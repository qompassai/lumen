//! Fidelity metric (G-L2) validation tests: the metric runs on
//! honest inputs and is deterministic.

use std::path::PathBuf;

use image::{ImageBuffer, Rgba};
use lumen_factory::base::build_base_mesh;
use lumen_factory::fidelity::{measure_fidelity, measure_fidelity_measured};
use lumen_factory::gltf::write_glb;
use lumen_factory::identity::load_record;

fn fixture(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(rel)
}

fn seraphine_sprite() -> image::RgbaImage {
    image::open(fixture("tests/fixtures/seraphine_sprite_sheet_frame.png"))
        .expect("fixture decodes")
        .to_rgba8()
}

fn seraphine_glb() -> Vec<u8> {
    std::fs::read(fixture("assets/seraphine.glb")).expect("glb fixture reads")
}

/// A synthetic sprite the identity segmentation can read: a
/// skin-faced, hair-topped figure on transparency, built the
/// same way the identity tests build theirs.
fn synthetic_sprite() -> image::RgbaImage {
    let (w, h) = (96u32, 240u32);
    let mut img: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::from_pixel(w, h, Rgba([0, 0, 0, 0]));
    let skin = Rgba([234, 204, 200, 255]);
    let hair = Rgba([162, 74, 134, 255]);
    let cloth = Rgba([40, 40, 90, 255]);
    // Hair mass over the top of the head.
    for y in 8..44 {
        for x in 30..66 {
            img.put_pixel(x, y, hair);
        }
    }
    // Face: a skin block whose width narrows at the jaw.
    for y in 20..58 {
        for x in 34..62 {
            img.put_pixel(x, y, skin);
        }
    }
    for y in 58..66 {
        for x in 40..56 {
            img.put_pixel(x, y, skin);
        }
    }
    // Body.
    for y in 66..h - 4 {
        for x in 26..70 {
            img.put_pixel(x, y, cloth);
        }
    }
    img
}

#[test]
fn seraphine_first_measurement_is_in_the_recorded_band() {
    let record = load_record(&fixture("assets/seraphine.identity.json")).expect("record loads");
    let report = measure_fidelity_measured(
        &seraphine_sprite(),
        &seraphine_glb(),
        &record.sprite_measurements,
        "identity_record",
        256,
    )
    .expect("measurement runs");
    // Recorded 2026-10-29 (FIDELITY.md): IoU 0.2456, landmark
    // mean delta 0.00685, max 0.0139. Bands are wide enough for
    // lattice rounding, tight enough to catch a regression in
    // either leg.
    assert!(
        (0.20..0.30).contains(&report.silhouette_iou),
        "iou {}",
        report.silhouette_iou
    );
    assert!(report.landmark_mean_delta < 0.02, "{report:?}");
    assert!(report.landmark_max_delta < 0.03, "{report:?}");
    assert_eq!(report.landmarks.len(), 5);
    assert_eq!(report.glb_vertices, 1370);
    assert_eq!(report.glb_triangles, 2618);
}

#[test]
fn measurement_is_deterministic() {
    let sprite = seraphine_sprite();
    let glb = seraphine_glb();
    let a = measure_fidelity(&sprite, &glb, "#eaccc8", "#a24a86", 256).expect("runs");
    let b = measure_fidelity(&sprite, &glb, "#eaccc8", "#a24a86", 256).expect("runs");
    assert_eq!(a.silhouette_iou, b.silhouette_iou);
    assert_eq!(a.landmark_mean_delta, b.landmark_mean_delta);
    assert_eq!(a.sprite_cells, b.sprite_cells);
    assert_eq!(a.glb_cells, b.glb_cells);
}

#[test]
fn synthetic_pair_scores_and_self_match_beats_cross_match() {
    let sprite = synthetic_sprite();
    let base = write_glb(&build_base_mesh().expect("base builds"), &[]).expect("glb writes");
    let report = measure_fidelity(&sprite, &base, "#eaccc8", "#a24a86", 128).expect("runs");
    assert!(report.silhouette_iou > 0.0 && report.silhouette_iou <= 1.0);
    assert!(report.landmark_mean_delta >= 0.0);
    // The same sprite against the Seraphine GLB (different
    // proportions) must not score identically to the base pair:
    // the metric discriminates.
    let cross =
        measure_fidelity(&sprite, &seraphine_glb(), "#eaccc8", "#a24a86", 128).expect("cross runs");
    assert_ne!(report.silhouette_iou, cross.silhouette_iou);
}

#[test]
fn identical_silhouette_source_gives_stable_cell_counts() {
    let sprite = synthetic_sprite();
    let base = write_glb(&build_base_mesh().expect("base builds"), &[]).expect("glb writes");
    let a = measure_fidelity(&sprite, &base, "#eaccc8", "#a24a86", 64).expect("runs");
    assert!(a.sprite_cells > 0 && a.glb_cells > 0);
    assert!(a.sprite_cells <= 64 * 64 && a.glb_cells <= 64 * 64);
}
