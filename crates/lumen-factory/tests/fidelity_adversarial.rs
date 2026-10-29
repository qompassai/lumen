//! Fidelity metric (G-L2) adversarial tests: hostile or
//! degenerate inputs fail closed, never score.

use std::path::PathBuf;

use image::{ImageBuffer, Rgba};
use lumen_factory::fidelity::measure_fidelity;

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

#[test]
fn garbage_glb_is_refused() {
    let err = measure_fidelity(
        &seraphine_sprite(),
        b"this is not a glb at all",
        "#eaccc8",
        "#a24a86",
        256,
    )
    .unwrap_err();
    assert!(err.to_string().contains("glb"), "{err}");
}

#[test]
fn truncated_glb_is_refused() {
    let glb = seraphine_glb();
    let err =
        measure_fidelity(&seraphine_sprite(), &glb[..64], "#eaccc8", "#a24a86", 256).unwrap_err();
    assert!(err.to_string().contains("glb"), "{err}");
}

#[test]
fn fully_transparent_sprite_is_refused() {
    let img: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::from_pixel(64, 64, Rgba([0, 0, 0, 0]));
    let err = measure_fidelity(&img, &seraphine_glb(), "#eaccc8", "#a24a86", 256).unwrap_err();
    assert!(!err.to_string().is_empty());
}

#[test]
fn unsegmentable_sprite_is_refused() {
    // Opaque block with no skin face: silhouette exists but the
    // identity segmentation cannot read it — fail closed.
    let img: ImageBuffer<Rgba<u8>, Vec<u8>> =
        ImageBuffer::from_pixel(64, 128, Rgba([10, 10, 10, 255]));
    let err = measure_fidelity(&img, &seraphine_glb(), "#eaccc8", "#a24a86", 256).unwrap_err();
    assert!(!err.to_string().is_empty());
}

#[test]
fn bad_palette_hex_is_refused() {
    let err = measure_fidelity(
        &seraphine_sprite(),
        &seraphine_glb(),
        "not-a-color",
        "#a24a86",
        256,
    )
    .unwrap_err();
    assert!(!err.to_string().is_empty());
}

#[test]
fn zero_and_oversized_grids_are_refused() {
    for grid in [0u32, 4096] {
        let err = measure_fidelity(
            &seraphine_sprite(),
            &seraphine_glb(),
            "#eaccc8",
            "#a24a86",
            grid,
        )
        .unwrap_err();
        assert!(err.to_string().contains("grid"), "{err}");
    }
}
