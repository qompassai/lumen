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

const SKIN: [u8; 3] = [242, 214, 196];
const HAIR: [u8; 3] = [230, 70, 170];

#[test]
fn fully_transparent_sprite_errors() {
    let img = RgbaImage::new(40, 80);
    assert!(measure_proportions(&img, SKIN, HAIR).is_err());
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
    assert!(measure_proportions(&img, SKIN, HAIR).is_err());
}

#[test]
fn sprite_without_skin_colored_face_fails_closed() {
    // A clean banded silhouette whose "face" is outfit-blue: the face
    // column cannot be located, so the head is never guessed.
    let mut img = RgbaImage::from_pixel(40, 80, Rgba([0, 0, 0, 0]));
    for y in 0..80u32 {
        let (x0, x1) = if (20..24).contains(&y) {
            (17, 23)
        } else {
            (8, 32)
        };
        for x in x0..x1 {
            img.put_pixel(x, y, Rgba([60, 80, 200, 255]));
        }
    }
    let err = measure_proportions(&img, SKIN, HAIR).expect_err("no skin face");
    assert!(err.to_string().contains("skin-colored"), "error: {err}");
}

#[test]
fn sprite_without_jaw_narrowing_fails_closed() {
    // Hair hanging on both sides keeps the face-column run as wide as
    // the head all the way down (the chibi long-hair case): the head
    // cannot be segmented, and the measurement must not fall back to
    // counting the hair mass as head.
    let mut img = RgbaImage::from_pixel(40, 80, Rgba([0, 0, 0, 0]));
    for y in 0..80u32 {
        for x in 8..32 {
            let skin_face = (10..30).contains(&y) && (14..26).contains(&x);
            let color = if skin_face { SKIN } else { HAIR };
            img.put_pixel(x, y, Rgba([color[0], color[1], color[2], 255]));
        }
    }
    let err = measure_proportions(&img, SKIN, HAIR).expect_err("no jaw narrowing");
    assert!(err.to_string().contains("jaw narrowing"), "error: {err}");
}

#[test]
fn version_1_identity_record_is_rejected() {
    // A v1 record's head ratio measured the alpha band (hair mass
    // included); v2 changed the meaning, so v1 fails closed even with
    // a valid hash.
    let dir = temp_dir("v1record");
    write_valid_pair(&dir);
    let mut record = ingest(&dir, "adv").expect("ingest");
    record.version = 1;
    record.record_hash = record.compute_hash().expect("re-hash");
    let path = dir.join("record.json");
    std::fs::write(&path, record.canonical_json().expect("json")).expect("write");
    let err = load_record(&path).expect_err("v1 rejected");
    assert!(
        err.to_string().contains("unsupported identity schema"),
        "error: {err}"
    );
    let _ = std::fs::remove_dir_all(&dir);
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

/// Head primitive `(width, height)` from its POSITION accessor bounds.
fn head_size(bytes: &[u8]) -> (f32, f32) {
    let parts = lumen_factory::gltf::parse_glb_parts(bytes).expect("glb parts");
    let json = &parts.json;
    for primitive in json["meshes"][0]["primitives"]
        .as_array()
        .expect("primitives")
    {
        let material = primitive["material"].as_u64().expect("material") as usize;
        if json["materials"][material]["name"] != "head" {
            continue;
        }
        let accessor = &json["accessors"][primitive["attributes"]["POSITION"]
            .as_u64()
            .expect("POSITION") as usize];
        let span = |axis: usize| {
            (accessor["max"][axis].as_f64().unwrap() - accessor["min"][axis].as_f64().unwrap())
                as f32
        };
        return (span(0), span(1));
    }
    panic!("no head primitive");
}

#[test]
fn extreme_head_scales_never_stretch_one_axis() {
    // At both ends of the accepted range the head must scale as a
    // whole: v1 scaled only its height, so a 2.0 band scale doubled
    // the aspect.
    let record = lumen_factory::identity::load_record(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/seraphine.identity.json"),
    )
    .expect("record");
    let templates = load_templates(&fixture_templates()).expect("templates");
    let build_at = |head_scale: f64| {
        let mut params = lumen_factory::params::ParameterRecord::from_identity(&record).unwrap();
        params.proportions.head_scale = head_scale;
        params.record_hash = params.compute_hash().unwrap();
        let bytes = lumen_factory::project::build_character_with_params(
            &record,
            &templates,
            &params,
            BuildOptions::default(),
        )
        .expect("bounds-valid head scale builds");
        head_size(&bytes)
    };
    let (unit_w, unit_h) = build_at(1.0);
    for scale in [0.5f32, 2.0] {
        let (w, h) = build_at(f64::from(scale));
        assert!(
            ((h / w) / (unit_h / unit_w) - 1.0).abs() < 1e-3,
            "aspect drifts at {scale}"
        );
        assert!(
            (w / unit_w / scale - 1.0).abs() < 1e-3,
            "width ignores scale {scale}"
        );
        assert!(
            (h / unit_h / scale - 1.0).abs() < 1e-3,
            "height ignores scale {scale}"
        );
    }
}

fn seraphine_record() -> lumen_factory::identity::IdentityRecord {
    lumen_factory::identity::load_record(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/seraphine.identity.json"),
    )
    .expect("record")
}

#[test]
fn hair_past_the_hips_or_degenerate_bands_fail_closed() {
    let mut record = seraphine_record();
    let m = record.sprite_measurements;
    // Hair below the hip row is outside the modeled range: an error,
    // never a clamp to 1.0.
    record.sprite_measurements.hair_bottom_row = Some(m.hip_row + 5);
    let err = record.hair_length().expect_err("past the hips");
    assert!(err.to_string().contains("not clamped"), "error: {err}");
    assert!(lumen_factory::params::ParameterRecord::from_identity(&record).is_err());
    // Exactly at the hips is the top of the range.
    record.sprite_measurements.hair_bottom_row = Some(m.hip_row);
    assert_eq!(record.hair_length().expect("at hips"), 1.0);
    // Hip at or above the neck: degenerate bands, no division.
    record.sprite_measurements.hip_row = m.neck_row;
    assert!(record.hair_length().is_err());
}

#[test]
fn hair_length_bounds_build_no_tail_and_a_hip_length_tail() {
    let record = seraphine_record();
    let templates = load_templates(&fixture_templates()).expect("templates");
    let hair_min_y = |length: f64| {
        let mut params = lumen_factory::params::ParameterRecord::from_identity(&record).unwrap();
        params.hair.length = length;
        params.record_hash = params.compute_hash().unwrap();
        let bytes = lumen_factory::project::build_character_with_params(
            &record,
            &templates,
            &params,
            BuildOptions::default(),
        )
        .expect("bounds-valid hair length builds");
        let parts = lumen_factory::gltf::parse_glb_parts(&bytes).expect("parts");
        let json = &parts.json;
        let hair = json["meshes"][0]["primitives"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| json["materials"][p["material"].as_u64().unwrap() as usize]["name"] == "hair")
            .expect("hair primitive");
        let accessor =
            &json["accessors"][hair["attributes"]["POSITION"].as_u64().unwrap() as usize];
        (accessor["min"][1].as_f64().unwrap() as f32, params.scales())
    };
    let (no_tail_y, scales) = hair_min_y(0.0);
    let joints = lumen_factory::base::scaled_joint_table(scales.0, scales.1, scales.2);
    let y_of = |name: &str| joints.iter().find(|j| j.name == name).unwrap().position[1];
    // Length 0: only the cap remains, its back panel ending above the
    // hips by far (the tail is the only geometry that reaches down).
    assert!(
        no_tail_y > y_of("Hips") + 0.3,
        "no-tail hair min y {no_tail_y}"
    );
    let (hip_tail_y, _) = hair_min_y(1.0);
    assert!(
        (hip_tail_y - y_of("Hips")).abs() < 1e-3,
        "hip-length tail {hip_tail_y}"
    );
}

#[test]
fn invalid_companion_name_errors() {
    let dir = temp_dir("badname");
    assert!(ingest(&dir, "../escape").is_err());
    assert!(ingest(&dir, "").is_err());
    let _ = std::fs::remove_dir_all(&dir);
}
