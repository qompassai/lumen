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

/// Synthetic sprite 100x200 with known color-segmentation landmarks:
/// an off-center ponytail tip (rows 0..6, x 64..70), the hair cap
/// (rows 6..18, w 40), the face (rows 18..34: skin x 34..66 framed by
/// hair), a hair tail hanging off the head's right side (x 70..76,
/// rows 18..66), the jaw/neck narrowing (rows 34..38, skin w 12), the
/// torso (rows 38..106, w 60) with hips (rows 94..102, w 80), and two
/// leg columns (rows 106..200).
fn synthetic_sprite() -> RgbaImage {
    let mut img = RgbaImage::new(100, 200);
    for p in img.pixels_mut() {
        *p = Rgba([0, 0, 0, 0]);
    }
    fill(&mut img, 64, 0, 70, 6, px(HAIR));
    fill(&mut img, 30, 6, 70, 34, px(HAIR));
    fill(&mut img, 34, 18, 66, 34, px(SKIN));
    fill(&mut img, 44, 34, 56, 38, px(SKIN));
    fill(&mut img, 20, 38, 80, 106, px(OUTFIT_A));
    fill(&mut img, 10, 94, 90, 102, px(OUTFIT_A));
    fill(&mut img, 70, 18, 76, 66, px(HAIR));
    fill(&mut img, 26, 106, 46, 200, px(OUTFIT_B));
    fill(&mut img, 54, 106, 74, 200, px(OUTFIT_B));
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
    let (props, m) = measure_proportions(&synthetic_sprite(), SKIN, HAIR).expect("measures");
    // Face column: median x of the skin pixels in the upper band.
    assert_eq!(m.face_center_x, 50);
    // The ponytail tip (rows 0..6) is off the face column: the scalp
    // is row 6 and the figure is rows 6..200.
    assert_eq!(m.scalp_offset_px, 6);
    assert_eq!(m.total_height_px, 194);
    // Chin at the jaw narrowing (absolute row 34), not at the hair
    // tail that keeps the head band wide down to row 66.
    assert_eq!(m.chin_row, 28);
    assert_eq!(m.skull_height_px, 28);
    // Neck = chin + round(28 * base_neck_per_skull) = 28 + 8.
    assert_eq!(m.neck_row, 36);
    assert_eq!(m.head_height_px, 36);
    assert_eq!(m.hip_row, 88);
    assert_eq!(m.torso_height_px, 52);
    assert_eq!(m.leg_span_px, 106);
    // The tail is the hair mass's lowest extent (absolute row 65).
    assert_eq!(m.hair_bottom_row, Some(59));
    assert!((props.head_ratio - 36.0 / 194.0).abs() < 1e-12);
    assert!((props.torso_ratio - 52.0 / 194.0).abs() < 1e-12);
    assert!((props.leg_ratio - 106.0 / 194.0).abs() < 1e-12);
}

#[test]
fn base_neck_allowance_comes_from_the_base_head() {
    // Chin 1.665 - 0.106 = 1.559 m, Neck joint 1.50 m, skull 0.212 m.
    let ratio = lumen_factory::base::base_neck_per_skull();
    assert!((ratio - 0.059 / 0.212).abs() < 1e-5, "ratio {ratio}");
}

#[test]
fn ingest_measures_the_mature_sprite_when_present() {
    let dir = temp_art_dir("mature", false);
    // Base sprite: a tall-headed chibi-like silhouette the mature
    // sprite must override.
    let mut chibi = RgbaImage::from_pixel(100, 200, Rgba([0, 0, 0, 0]));
    fill(&mut chibi, 20, 0, 80, 70, px(HAIR));
    fill(&mut chibi, 30, 20, 70, 70, px(SKIN));
    fill(&mut chibi, 44, 70, 56, 74, px(SKIN));
    fill(&mut chibi, 20, 74, 80, 200, px(OUTFIT_A));
    fill(&mut chibi, 10, 110, 90, 116, px(OUTFIT_A));
    let companions = dir.join("companions");
    DynamicImage::ImageRgba8(chibi.clone())
        .save(companions.join("testina_animated_base.gif"))
        .expect("write base sprite");
    let without = ingest(&dir, "testina").expect("ingest without mature");
    assert_eq!(without.proportions_source, "sprite_base");
    let (chibi_props, _) = measure_proportions(&chibi, SKIN, HAIR).expect("chibi measures");
    assert_eq!(without.proportions, chibi_props);

    DynamicImage::ImageRgba8(synthetic_sprite())
        .save(companions.join("testina_animated_mature.gif"))
        .expect("write mature sprite");
    let with = ingest(&dir, "testina").expect("ingest with mature");
    assert_eq!(with.proportions_source, "sprite_mature");
    let (mature_props, mature_m) =
        measure_proportions(&synthetic_sprite(), SKIN, HAIR).expect("mature measures");
    assert_eq!(with.proportions, mature_props);
    assert_eq!(with.sprite_measurements, mature_m);
    assert!(with.sources.iter().any(|s| s.role == "sprite_mature"));
    let _ = std::fs::remove_dir_all(&dir);
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
    assert_eq!(record.version, 2);
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

fn asset_bytes(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// POSITION accessor bounds `(min, max)` of the primitive whose
/// material is named `part`, or of every primitive when `part` is
/// `None`.
fn extent(bytes: &[u8], part: Option<&str>) -> ([f32; 3], [f32; 3]) {
    let parts = lumen_factory::gltf::parse_glb_parts(bytes).expect("glb parts");
    let json = &parts.json;
    let mut lo = [f32::INFINITY; 3];
    let mut hi = [f32::NEG_INFINITY; 3];
    for primitive in json["meshes"][0]["primitives"]
        .as_array()
        .expect("primitives")
    {
        let material = primitive["material"].as_u64().expect("material") as usize;
        let name = json["materials"][material]["name"].as_str().expect("name");
        if part.is_some_and(|wanted| wanted != name) {
            continue;
        }
        let accessor = &json["accessors"][primitive["attributes"]["POSITION"]
            .as_u64()
            .expect("POSITION") as usize];
        for axis in 0..3 {
            lo[axis] = lo[axis].min(accessor["min"][axis].as_f64().expect("min") as f32);
            hi[axis] = hi[axis].max(accessor["max"][axis].as_f64().expect("max") as f32);
        }
    }
    (lo, hi)
}

#[test]
fn seraphine_head_keeps_base_aspect_and_measured_fraction() {
    let aspect = |bytes: &[u8]| {
        let (lo, hi) = extent(bytes, Some("head"));
        (hi[1] - lo[1]) / (hi[0] - lo[0])
    };
    let base_aspect = aspect(&asset_bytes("base.glb"));
    let glb = asset_bytes("seraphine.glb");
    let built_aspect = aspect(&glb);
    // The commission bound is +-10%; a uniform scale holds it exactly
    // (to f32 accessor noise).
    assert!(
        (built_aspect / base_aspect - 1.0).abs() < 1e-3,
        "head aspect {built_aspect} vs base {base_aspect}"
    );
    // The head's share of the figure matches the color-segmented
    // skull fraction from the identity record.
    let record = lumen_factory::identity::load_record(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("assets/seraphine.identity.json"),
    )
    .expect("record");
    let m = record.sprite_measurements;
    let skull_fraction = f64::from(m.skull_height_px) / f64::from(m.total_height_px);
    let (head_lo, head_hi) = extent(&glb, Some("head"));
    let (all_lo, all_hi) = extent(&glb, None);
    let head_fraction = f64::from((head_hi[1] - head_lo[1]) / (all_hi[1] - all_lo[1]));
    assert!(
        (head_fraction / skull_fraction - 1.0).abs() < 0.05,
        "head fraction {head_fraction} vs color-segmented skull {skull_fraction}"
    );
}
