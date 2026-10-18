//! Validation tests: identity ingest + projection v1 happy paths, on
//! synthetic art with known colors and known sprite bands.

use std::path::PathBuf;

use image::{DynamicImage, Rgba, RgbaImage};
use lumen_factory::base::{ScaledBuild, build_base_mesh, build_base_mesh_parametric, joint_table};
use lumen_factory::blend::load_templates;
use lumen_factory::gltf::{parse_glb, write_glb};
use lumen_factory::identity::{ingest, measure_proportions, median_cut};
use lumen_factory::project::{BuildOptions, build_character, face_point_px, paint_atlas};

const HAIR: [u8; 3] = [232, 72, 168];
const SKIN: [u8; 3] = [243, 214, 196];
const EYE: [u8; 3] = [120, 40, 160];
const LIPS: [u8; 3] = [196, 64, 84];
const OUTFIT_A: [u8; 3] = [60, 80, 200];
const OUTFIT_B: [u8; 3] = [40, 44, 60];
const BG: [u8; 3] = [12, 18, 60];

fn px(c: [u8; 3]) -> Rgba<u8> {
    Rgba([c[0], c[1], c[2], 255])
}

fn fill(img: &mut RgbaImage, x0: u32, y0: u32, x1: u32, y1: u32, color: Rgba<u8>) {
    for y in y0..y1.min(img.height()) {
        for x in x0..x1.min(img.width()) {
            img.put_pixel(x, y, color);
        }
    }
}

/// Synthetic portrait 128x192 with known regions (see identity.rs
/// module docs for the region fractions this targets).
fn synthetic_portrait() -> RgbaImage {
    let mut img = RgbaImage::from_pixel(128, 192, px(BG));
    // Hair mass across the upper third.
    fill(&mut img, 24, 0, 104, 64, px(HAIR));
    // Hair sides framing the face.
    fill(&mut img, 24, 64, 40, 110, px(HAIR));
    fill(&mut img, 88, 64, 104, 110, px(HAIR));
    // Face.
    fill(&mut img, 48, 50, 80, 88, px(SKIN));
    // Eyes.
    fill(&mut img, 54, 66, 60, 72, px(EYE));
    fill(&mut img, 68, 66, 74, 72, px(EYE));
    // Lips.
    fill(&mut img, 58, 82, 70, 86, px(LIPS));
    img
}

/// Synthetic sprite 100x200 with known alpha bands:
/// hair rows 0..50 (w 70), neck 50..60 (w 24), torso 60..120 (w 66),
/// hips at 100..110 (w 84), legs 120..200 (two columns, total w 44).
fn synthetic_sprite() -> RgbaImage {
    let mut img = RgbaImage::new(100, 200);
    for p in img.pixels_mut() {
        *p = Rgba([0, 0, 0, 0]);
    }
    fill(&mut img, 15, 0, 85, 50, px(HAIR));
    fill(&mut img, 38, 50, 62, 60, px(SKIN));
    fill(&mut img, 17, 60, 83, 120, px(OUTFIT_A));
    fill(&mut img, 8, 100, 92, 110, px(OUTFIT_A));
    fill(&mut img, 20, 120, 42, 200, px(OUTFIT_B));
    fill(&mut img, 58, 120, 80, 200, px(OUTFIT_B));
    img
}

fn temp_art_dir(tag: &str, with_expressions: bool) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lumen-factory-val-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let companions = dir.join("companions");
    std::fs::create_dir_all(&companions).expect("mkdir companions");
    DynamicImage::ImageRgba8(synthetic_portrait())
        .save(companions.join("testina_portrait.jpg"))
        .expect("write portrait jpg");
    DynamicImage::ImageRgba8(synthetic_sprite())
        .save(companions.join("testina_animated_base.gif"))
        .expect("write sprite gif");
    if with_expressions {
        let expressions = dir.join("expressions");
        std::fs::create_dir_all(&expressions).expect("mkdir expressions");
        DynamicImage::ImageRgba8(synthetic_portrait())
            .save(expressions.join("testina_happy.png"))
            .expect("write expression png");
    }
    dir
}

fn fixture_templates() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/emotions_fixture.json")
}

#[test]
fn median_cut_finds_known_dominant_colors_deterministically() {
    let mut pixels: Vec<[u8; 3]> = Vec::new();
    pixels.extend(std::iter::repeat_n(HAIR, 700));
    pixels.extend(std::iter::repeat_n(BG, 300));
    let first = median_cut(&pixels, 4);
    let second = median_cut(&pixels, 4);
    assert_eq!(first, second, "quantizer must be deterministic");
    assert!(!first.is_empty());
    let top = &first[0];
    // Median-cut splits boxes at the population median, so a 70%
    // color group yields a pure top box of share >= 0.5, not 0.7.
    assert!(top.share >= 0.5, "top share {}", top.share);
    assert!(
        (top.color.rgb[0] as i32 - HAIR[0] as i32).abs() <= 2
            && (top.color.rgb[1] as i32 - HAIR[1] as i32).abs() <= 2
            && (top.color.rgb[2] as i32 - HAIR[2] as i32).abs() <= 2,
        "top color {:?} should be the magenta block",
        top.color.rgb
    );
}

#[test]
fn proportions_on_known_sprite_bands() {
    let (props, m) = measure_proportions(&synthetic_sprite()).expect("measures");
    assert_eq!(m.total_height_px, 200);
    assert_eq!(m.neck_row, 50);
    assert_eq!(m.hip_row, 100);
    assert_eq!(m.head_height_px, 50);
    assert_eq!(m.torso_height_px, 50);
    assert_eq!(m.leg_span_px, 100);
    assert!((props.head_ratio - 0.25).abs() < 1e-9);
    assert!((props.torso_ratio - 0.25).abs() < 1e-9);
    assert!((props.leg_ratio - 0.50).abs() < 1e-9);
}

#[test]
fn synthetic_ingest_finds_magenta_hair_and_skin() {
    let dir = temp_art_dir("anchors", true);
    let record = ingest(&dir, "testina").expect("ingest succeeds");
    let hair = record.palette.hair.rgb;
    assert!(
        hair[0] > 150 && hair[2] > 80 && hair[0] > hair[1] + 40,
        "hair {hair:?} must be in the magenta family"
    );
    let skin = record.palette.skin.rgb;
    assert!(
        skin[0] > 200 && skin[1] > 160 && skin[2] > 130 && skin[0] >= skin[1] && skin[1] >= skin[2],
        "skin {skin:?} must be the pale warm block"
    );
    assert!(record.palette.eye.is_some(), "eye anchor should be found");
    assert!(record.palette.lips.is_some(), "lip anchor should be found");
    assert_eq!(record.palette.outfit.len(), 2, "two outfit primaries");
    let primary = record.palette.outfit[0].rgb;
    assert!(
        primary[2] > primary[0] + 40,
        "outfit primary {primary:?} should be the blue block"
    );
    assert_eq!(record.schema, "lumen.factory-identity");
    assert_eq!(record.version, 1);
    assert!(record.sources.iter().any(|s| s.role == "portrait"));
    assert!(record.sources.iter().any(|s| s.role == "sprite_base"));
    assert!(record.sources.iter().any(|s| s.role == "expression"));
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn record_round_trip_and_hash_stability() {
    let dir = temp_art_dir("roundtrip", false);
    let first = ingest(&dir, "testina").expect("first ingest");
    let second = ingest(&dir, "testina").expect("second ingest");
    assert_eq!(first.record_hash, second.record_hash);
    assert!(first.verify_hash().expect("hash verifies"));
    let json = first.canonical_json().expect("canonical json");
    let parsed: lumen_factory::identity::IdentityRecord =
        serde_json::from_str(&json).expect("round trip parses");
    // Canonical bytes are the identity contract: floats are stored
    // at fixed 12-decimal precision, so a parsed record may differ
    // from the in-memory one below that precision while remaining
    // byte-identical in canonical form (and hash-identical).
    assert_eq!(parsed.canonical_json().expect("re-canonical"), json);
    assert_eq!(parsed.record_hash, first.record_hash);
    assert!(parsed.verify_hash().expect("parsed hash verifies"));
    assert_eq!(parsed.companion, first.companion);
    assert_eq!(parsed.sources, first.sources);
    assert_eq!(parsed.palette, first.palette);
    assert!(
        (parsed.proportions.head_ratio - first.proportions.head_ratio).abs() < 1e-9,
        "ratios survive the round trip to canonical precision"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn parametric_default_matches_phase1a_base_exactly() {
    let default = build_base_mesh().expect("default base");
    let parametric = build_base_mesh_parametric(&ScaledBuild::default()).expect("parametric base");
    assert_eq!(default.joints, parametric.joints);
    assert_eq!(default.parts, parametric.parts);
    let a = write_glb(&default, &[]).expect("glb a");
    let b = write_glb(&parametric, &[]).expect("glb b");
    assert_eq!(a, b, "default parametric build must be byte-identical");
    assert_eq!(joint_table().len(), 65);
}

#[test]
fn build_is_deterministic_and_textured() {
    let dir = temp_art_dir("build", false);
    let record = ingest(&dir, "testina").expect("ingest");
    let templates = load_templates(&fixture_templates()).expect("templates");
    let a = build_character(
        &record,
        &templates,
        BuildOptions {
            ramp: false,
            toon: true,
        },
    )
    .expect("build a");
    let b = build_character(
        &record,
        &templates,
        BuildOptions {
            ramp: false,
            toon: true,
        },
    )
    .expect("build b");
    assert_eq!(a, b, "same record + templates must be byte-identical");
    let summary = parse_glb(&a).expect("built glb parses");
    assert_eq!(summary.joint_count, 65);
    assert!(
        summary.target_names.len() >= 20,
        "targets: {}",
        summary.target_names.len()
    );
    let text = String::from_utf8_lossy(&a);
    assert!(
        text.contains("baseColorTexture"),
        "atlas must be embedded as baseColor texture"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn face_paint_lands_on_uv_expected_pixels() {
    let dir = temp_art_dir("face", false);
    let record = ingest(&dir, "testina").expect("ingest");
    let atlas = paint_atlas(&record, false);
    // Iris at the eye center (sampled 3px off-center: the exact
    // center pixel is the pupil, by construction).
    let eye_rgb = record.palette.eye.expect("eye").rgb;
    let (ex, ey) = face_point_px(0.36, 0.12);
    assert_eq!(
        atlas.get_pixel(ex + 3, ey).0[0..3],
        eye_rgb,
        "iris at eye UV"
    );
    assert_eq!(
        atlas.get_pixel(ex, ey).0[0..3],
        [10, 8, 10],
        "pupil at eye center"
    );
    // Brow stroke carries the hair color.
    let (bx, by) = face_point_px(-0.36, 0.30);
    assert_eq!(
        atlas.get_pixel(bx, by).0[0..3],
        record.palette.hair.rgb,
        "brow at brow UV"
    );
    // Lips at the mouth point.
    let lip_rgb = record.palette.lips.expect("lips").rgb;
    let (mx, my) = face_point_px(0.0, -0.46);
    assert_eq!(atlas.get_pixel(mx, my).0[0..3], lip_rgb, "lips at mouth UV");
    // Zone centers carry their zone colors.
    assert_eq!(
        atlas.get_pixel(640, 896).0[0..3],
        record.palette.hair.rgb,
        "hair zone"
    );
    assert_eq!(
        atlas.get_pixel(640, 640).0[0..3],
        record.palette.outfit[0].rgb,
        "outfit torso zone"
    );
    assert_eq!(
        atlas.get_pixel(128, 384).0[0..3],
        record.palette.outfit[1].rgb,
        "outfit legs zone"
    );
    assert_eq!(
        atlas.get_pixel(384, 768).0[0..3],
        record.palette.skin.rgb,
        "body zone is skin"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn toon_grade_is_deterministic_and_changes_non_face_pixels() {
    let dir = temp_art_dir("toon", false);
    let record = ingest(&dir, "testina").expect("ingest");
    let plain = paint_atlas(&record, false);
    let graded_a = paint_atlas(&record, true);
    let graded_b = paint_atlas(&record, true);
    assert_eq!(graded_a, graded_b, "grade must be deterministic");
    assert_ne!(plain, graded_a, "grade must change pixels");
    let _ = std::fs::remove_dir_all(&dir);
}
