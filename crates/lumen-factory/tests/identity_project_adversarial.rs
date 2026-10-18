//! Adversarial tests: identity ingest + projection v1 fail closed on
//! hostile or degenerate inputs, and tolerate only what the contract
//! says is tolerable (a missing expressions directory).

use std::path::PathBuf;

use image::{DynamicImage, Rgba, RgbaImage};
use lumen_factory::blend::load_templates;
use lumen_factory::identity::{ingest, load_record, measure_proportions};
use lumen_factory::project::{BuildOptions, build_character, compute_scales};

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lumen-factory-adv-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("companions")).expect("mkdir");
    dir
}

fn solid_image(w: u32, h: u32, color: [u8; 3]) -> RgbaImage {
    RgbaImage::from_pixel(w, h, Rgba([color[0], color[1], color[2], 255]))
}

fn write_valid_pair(dir: &std::path::Path) {
    // Portrait: dark background, magenta top, pale center.
    let mut portrait = solid_image(64, 96, [10, 12, 50]);
    for y in 0..32 {
        for x in 16..48 {
            portrait.put_pixel(x, y, Rgba([230, 70, 170, 255]));
        }
    }
    for y in 28..44 {
        for x in 24..40 {
            portrait.put_pixel(x, y, Rgba([242, 214, 196, 255]));
        }
    }
    DynamicImage::ImageRgba8(portrait)
        .save(dir.join("companions/adv_portrait.jpg"))
        .expect("portrait");
    // Sprite: simple banded silhouette.
    let mut sprite = RgbaImage::new(40, 80);
    for p in sprite.pixels_mut() {
        *p = Rgba([0, 0, 0, 0]);
    }
    for y in 0..80u32 {
        let (x0, x1, c): (u32, u32, [u8; 3]) = match y {
            0..=19 => (10, 30, [230, 70, 170]),
            20..=23 => (17, 23, [242, 214, 196]),
            24..=47 => (8, 32, [60, 80, 200]),
            _ => (12, 28, [40, 44, 60]),
        };
        for x in x0..x1 {
            sprite.put_pixel(x, y, Rgba([c[0], c[1], c[2], 255]));
        }
    }
    DynamicImage::ImageRgba8(sprite)
        .save(dir.join("companions/adv_animated_base.gif"))
        .expect("sprite");
}

fn fixture_templates() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/emotions_fixture.json")
}

#[test]
fn missing_art_dir_errors() {
    let dir = std::env::temp_dir().join("lumen-factory-adv-does-not-exist-xyz");
    assert!(ingest(&dir, "adv").is_err());
}

#[test]
fn missing_portrait_errors() {
    let dir = temp_dir("noportrait");
    assert!(ingest(&dir, "adv").is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn corrupt_portrait_bytes_error() {
    let dir = temp_dir("corruptportrait");
    std::fs::write(
        dir.join("companions/adv_portrait.jpg"),
        b"this is not a jpeg at all",
    )
    .expect("write");
    DynamicImage::ImageRgba8(solid_image(40, 80, [60, 80, 200]))
        .save(dir.join("companions/adv_animated_base.gif"))
        .expect("sprite");
    assert!(ingest(&dir, "adv").is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn corrupt_sprite_bytes_error() {
    let dir = temp_dir("corruptsprite");
    DynamicImage::ImageRgba8(solid_image(64, 96, [230, 70, 170]))
        .save(dir.join("companions/adv_portrait.jpg"))
        .expect("portrait");
    std::fs::write(
        dir.join("companions/adv_animated_base.gif"),
        b"GIF89a garbage garbage garbage",
    )
    .expect("write");
    assert!(ingest(&dir, "adv").is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn fully_transparent_sprite_errors() {
    let img = RgbaImage::new(40, 80);
    assert!(measure_proportions(&img).is_err());
    let dir = temp_dir("transparent");
    write_valid_pair(&dir);
    // Replace the sprite with a fully transparent one.
    DynamicImage::ImageRgba8(RgbaImage::new(40, 80))
        .save(dir.join("companions/adv_animated_base.gif"))
        .expect("sprite");
    assert!(ingest(&dir, "adv").is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn portrait_1x1_errors() {
    let dir = temp_dir("tiny");
    DynamicImage::ImageRgba8(solid_image(1, 1, [230, 70, 170]))
        .save(dir.join("companions/adv_portrait.jpg"))
        .expect("portrait");
    DynamicImage::ImageRgba8(solid_image(40, 80, [60, 80, 200]))
        .save(dir.join("companions/adv_animated_base.gif"))
        .expect("sprite");
    assert!(ingest(&dir, "adv").is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn degenerate_single_row_sprite_errors() {
    let mut img = RgbaImage::new(40, 80);
    for p in img.pixels_mut() {
        *p = Rgba([0, 0, 0, 0]);
    }
    for x in 10..30 {
        img.put_pixel(x, 40, Rgba([200, 100, 100, 255]));
    }
    assert!(measure_proportions(&img).is_err());
}

#[test]
fn missing_expressions_dir_is_tolerated() {
    let dir = temp_dir("noexpr");
    write_valid_pair(&dir);
    assert!(!dir.join("expressions").exists());
    let record = ingest(&dir, "adv").expect("expressions are provenance only");
    assert!(record.sources.iter().all(|s| s.role != "expression"));
    assert!(record.sources.iter().any(|s| s.role == "portrait"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn proportions_beyond_scale_clamp_error() {
    let dir = temp_dir("clamp");
    write_valid_pair(&dir);
    let mut record = ingest(&dir, "adv").expect("ingest");
    // Head ratio 0.60 against a base of ~0.157 is a scale of ~3.8,
    // far beyond SCALE_MAX: must fail closed, never clamp.
    record.proportions.head_ratio = 0.60;
    record.proportions.torso_ratio = 0.20;
    record.proportions.leg_ratio = 0.20;
    assert!(compute_scales(&record).is_err());
    let templates = load_templates(&fixture_templates()).expect("templates");
    assert!(build_character(&record, &templates, BuildOptions::default()).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn tampered_record_fails_hash_verification() {
    let dir = temp_dir("tamper");
    write_valid_pair(&dir);
    let mut record = ingest(&dir, "adv").expect("ingest");
    record.palette.hair = lumen_factory::identity::ColorEntry::new([1, 2, 3]);
    assert!(!record.verify_hash().expect("verify runs"));
    let path = dir.join("record.json");
    std::fs::write(&path, record.canonical_json().expect("json")).expect("write");
    assert!(load_record(&path).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn invalid_companion_name_errors() {
    let dir = temp_dir("badname");
    assert!(ingest(&dir, "../escape").is_err());
    assert!(ingest(&dir, "").is_err());
    let _ = std::fs::remove_dir_all(&dir);
}
