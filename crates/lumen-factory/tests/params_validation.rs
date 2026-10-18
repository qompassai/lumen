//! Validation tests: parameter records — derivation from identity,
//! JSON/TOML round trips, the Séraphine byte-identical regression
//! anchor, override effects, determinism, and the extreme-values
//! parse-back gate.

use std::path::PathBuf;

use lumen_factory::blend::load_templates;
use lumen_factory::gltf::{parse_glb, parse_glb_parts};
use lumen_factory::identity::{IdentityRecord, load_record};
use lumen_factory::params::ParameterRecord;
use lumen_factory::project::{BuildOptions, build_character, build_character_with_params};
use lumen_factory::sha256_hex;

fn asset_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join(name)
}

fn seraphine_record() -> IdentityRecord {
    load_record(&asset_path("seraphine.identity.json")).expect("identity record loads")
}

fn real_templates() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates/emotions.json")
}

fn fixture_templates() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/emotions_fixture.json")
}

#[test]
fn from_identity_reproduces_phase2_scales_and_defaults() {
    let record = seraphine_record();
    let params = ParameterRecord::from_identity(&record).expect("derive params");
    let expected = lumen_factory::project::compute_scales(&record).expect("phase 2 scales");
    let (head, torso, leg) = params.scales();
    assert_eq!(
        (head, torso, leg),
        expected,
        "params scales == compute_scales"
    );
    assert_eq!(params.companion, "seraphine");
    assert_eq!(params.schema, "lumen.factory-params");
    assert_eq!(params.version, 1);
    // Projection v2: hair length is the measured hair mass (mature
    // sprite: hair bottom row 124, neck 45, hip 144 -> 79/99).
    assert_eq!(
        params.hair.length,
        record.hair_length().expect("hair length")
    );
    assert!((params.hair.length - 79.0 / 99.0).abs() < 1e-12);
    assert_eq!(params.face.eye_scale, 1.0, "eye scale default");
    assert_eq!(
        params.appearance.skin.as_deref(),
        Some(record.palette.skin.hex.as_str())
    );
    assert_eq!(
        params.appearance.hair.as_deref(),
        Some(record.palette.hair.hex.as_str())
    );
    assert_eq!(
        params.appearance.eyes.as_deref(),
        record.palette.eye.as_ref().map(|c| c.hex.as_str())
    );
    assert_eq!(
        params.appearance.outfit_primary.as_deref(),
        Some(record.palette.outfit[0].hex.as_str())
    );
    assert_eq!(
        params.appearance.outfit_secondary.as_deref(),
        Some(record.palette.outfit[1].hex.as_str())
    );
    assert!(params.verify_hash().expect("hash verifies"));
    params.validate().expect("derived record validates");
}

#[test]
fn anchor_seraphine_build_with_derived_params_is_byte_identical() {
    let record = seraphine_record();
    let templates = load_templates(&real_templates()).expect("real templates");
    let params = ParameterRecord::from_identity(&record).expect("derive params");
    let expected = std::fs::read(asset_path("seraphine.glb")).expect("anchor glb");
    // Deliberate re-anchors (projection v2, docs/factory/FACTORY-PLAN.md
    // §9): Phase 2/3 cd7abaca... -> fix 1 (color-segmented head)
    // d41e41ac... -> fix 2 (uniform head scale) 5f93fd6e... -> fix 3
    // (shell offsets + hair tail).
    assert_eq!(
        sha256_hex(&expected),
        "5bcaa972879b17a9f7a84b1ef2f00896329f53624d806ddca0c2dde6ab059552",
        "anchor asset is the projection v2 Séraphine build"
    );
    let via_params = build_character_with_params(
        &record,
        &templates,
        &params,
        BuildOptions {
            ramp: false,
            toon: true,
        },
    )
    .expect("build via params");
    assert_eq!(
        via_params, expected,
        "from_identity params build must reproduce the anchor byte-for-byte"
    );
    let via_identity = build_character(
        &record,
        &templates,
        BuildOptions {
            ramp: false,
            toon: true,
        },
    )
    .expect("build via identity");
    assert_eq!(via_identity, expected, "identity build is the same build");
}

#[test]
fn json_round_trip_preserves_canonical_bytes_and_hash() {
    let params = ParameterRecord::from_identity(&seraphine_record()).expect("derive");
    let json = params.canonical_json().expect("canonical json");
    let parsed = ParameterRecord::from_json(&json).expect("parse back");
    assert_eq!(
        parsed.canonical_json().expect("re-canonical"),
        json,
        "canonical bytes survive the round trip"
    );
    assert_eq!(parsed.record_hash, params.record_hash);
    assert_eq!(parsed.companion, params.companion);
    assert_eq!(parsed.appearance, params.appearance);
    // The f64 fields may differ below canonical (12-decimal)
    // precision after a JSON parse; the scales as consumed (f32)
    // are exact.
    assert_eq!(parsed.scales(), params.scales());
}

#[test]
fn toml_round_trip_preserves_the_record_exactly() {
    let params = ParameterRecord::from_identity(&seraphine_record()).expect("derive");
    let toml_text = params.to_toml().expect("to toml");
    let parsed = ParameterRecord::from_toml(&toml_text).expect("from toml");
    assert_eq!(
        parsed, params,
        "JSON -> TOML -> record is an exact round trip"
    );
    // JSON -> TOML -> record via the parsed JSON record as well.
    let json = params.canonical_json().expect("canonical json");
    let from_json = ParameterRecord::from_json(&json).expect("from json");
    let toml_text2 = from_json.to_toml().expect("to toml 2");
    let parsed2 = ParameterRecord::from_toml(&toml_text2).expect("from toml 2");
    assert_eq!(parsed2.record_hash, params.record_hash);
    assert_eq!(parsed2.scales(), params.scales());
    assert_eq!(parsed2.appearance, params.appearance);
}

#[test]
fn toml_round_trip_with_null_appearance_fields() {
    let mut params = ParameterRecord::from_identity(&seraphine_record()).expect("derive");
    params.appearance.eyes = None;
    params.appearance.lips = None;
    params.appearance.outfit_secondary = None;
    params.record_hash = params.compute_hash().expect("re-hash");
    let toml_text = params.to_toml().expect("to toml with nulls");
    let parsed = ParameterRecord::from_toml(&toml_text).expect("from toml with nulls");
    assert_eq!(parsed, params, "null appearance fields survive TOML");
    let json = params.canonical_json().expect("canonical json");
    assert!(json.contains("\"eyes\":null"), "canonical JSON keeps nulls");
}

#[test]
fn load_dispatches_on_extension() {
    let params = ParameterRecord::from_identity(&seraphine_record()).expect("derive");
    let dir = std::env::temp_dir().join(format!("lumen-factory-params-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let json_path = dir.join("record.params.json");
    std::fs::write(&json_path, params.canonical_json().expect("json")).expect("write json");
    let loaded_json = ParameterRecord::load(&json_path).expect("load json");
    assert_eq!(loaded_json.record_hash, params.record_hash);
    let toml_path = dir.join("record.params.toml");
    std::fs::write(&toml_path, params.to_toml().expect("toml")).expect("write toml");
    let loaded_toml = ParameterRecord::load(&toml_path).expect("load toml");
    assert_eq!(loaded_toml, params);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn overrides_change_the_build() {
    let record = seraphine_record();
    let templates = load_templates(&fixture_templates()).expect("fixture templates");
    let base = ParameterRecord::from_identity(&record).expect("derive");
    let base_bytes = build_character_with_params(
        &record,
        &templates,
        &base,
        BuildOptions {
            ramp: false,
            toon: false,
        },
    )
    .expect("base build");

    let mut recolored = base.clone();
    recolored.appearance.hair = Some("#2244cc".to_string());
    recolored.record_hash = recolored.compute_hash().expect("re-hash");
    let recolored_bytes = build_character_with_params(
        &record,
        &templates,
        &recolored,
        BuildOptions {
            ramp: false,
            toon: false,
        },
    )
    .expect("recolored build");
    assert_ne!(
        recolored_bytes, base_bytes,
        "hair color override changes the GLB"
    );

    let mut longer_hair = base.clone();
    longer_hair.hair.length = 1.0;
    longer_hair.record_hash = longer_hair.compute_hash().expect("re-hash");
    let longer_bytes = build_character_with_params(
        &record,
        &templates,
        &longer_hair,
        BuildOptions {
            ramp: false,
            toon: false,
        },
    )
    .expect("long hair build");
    assert_ne!(longer_bytes, base_bytes, "hair length changes the GLB");

    let mut bigger_eyes = base.clone();
    bigger_eyes.face.eye_scale = 1.3;
    bigger_eyes.record_hash = bigger_eyes.compute_hash().expect("re-hash");
    let bigger_bytes = build_character_with_params(
        &record,
        &templates,
        &bigger_eyes,
        BuildOptions {
            ramp: false,
            toon: false,
        },
    )
    .expect("big eyes build");
    assert_ne!(bigger_bytes, base_bytes, "eye scale changes the GLB");
}

#[test]
fn builds_with_params_are_deterministic() {
    let record = seraphine_record();
    let templates = load_templates(&fixture_templates()).expect("fixture templates");
    let params = ParameterRecord::from_identity(&record).expect("derive");
    let a = build_character_with_params(
        &record,
        &templates,
        &params,
        BuildOptions {
            ramp: false,
            toon: true,
        },
    )
    .expect("build a");
    let b = build_character_with_params(
        &record,
        &templates,
        &params,
        BuildOptions {
            ramp: false,
            toon: true,
        },
    )
    .expect("build b");
    assert_eq!(a, b, "same params + templates must be byte-identical");
}

/// Read one accessor's data from a parsed GLB as f32 values
/// (handles the float and unsigned-int component types the writer
/// emits).
fn accessor_values(parts: &lumen_factory::gltf::GlbParts, accessor_idx: usize) -> Vec<f32> {
    let accessor = &parts.json["accessors"][accessor_idx];
    let view_idx = accessor["bufferView"].as_u64().expect("bufferView") as usize;
    let view = &parts.json["bufferViews"][view_idx];
    let offset = view["byteOffset"].as_u64().unwrap_or(0) as usize;
    let length = view["byteLength"].as_u64().expect("byteLength") as usize;
    let slice = &parts.bin[offset..offset + length];
    let component_type = accessor["componentType"].as_u64().expect("componentType");
    let mut out = Vec::new();
    match component_type {
        5126 => {
            for chunk in slice.as_chunks::<4>().0 {
                out.push(f32::from_le_bytes(*chunk));
            }
        }
        5125 => {
            for chunk in slice.as_chunks::<4>().0 {
                out.push(u32::from_le_bytes(*chunk) as f32);
            }
        }
        other => panic!("unexpected componentType {other}"),
    }
    out
}

#[test]
fn extreme_values_build_is_structurally_clean() {
    let record = seraphine_record();
    let templates = load_templates(&fixture_templates()).expect("fixture templates");
    let mut params = ParameterRecord::from_identity(&record).expect("derive");
    params.proportions.head_scale = 2.0;
    params.proportions.torso_scale = 2.0;
    params.proportions.leg_scale = 0.5;
    params.hair.length = 1.0;
    params.face.eye_scale = 1.3;
    params.record_hash = params.compute_hash().expect("re-hash");
    params.validate().expect("bounds values validate");

    let bytes = build_character_with_params(
        &record,
        &templates,
        &params,
        BuildOptions {
            ramp: false,
            toon: false,
        },
    )
    .expect("extreme build succeeds");
    let summary = parse_glb(&bytes).expect("extreme build parses back");
    assert_eq!(summary.joint_count, 65);
    assert!(
        summary.weight_sum_min >= 0.999 && summary.weight_sum_max <= 1.001,
        "weights sum to ~1: [{}, {}]",
        summary.weight_sum_min,
        summary.weight_sum_max
    );

    let parts = parse_glb_parts(&bytes).expect("parts");
    for mesh in parts.json["meshes"].as_array().expect("meshes") {
        for primitive in mesh["primitives"].as_array().expect("primitives") {
            let pos_idx = primitive["attributes"]["POSITION"]
                .as_u64()
                .expect("POSITION") as usize;
            let positions = accessor_values(&parts, pos_idx);
            assert!(
                positions.iter().all(|v| v.is_finite()),
                "all positions finite at extremes"
            );
            let vertex_count = parts.json["accessors"][pos_idx]["count"]
                .as_u64()
                .expect("count") as usize;
            assert_eq!(positions.len(), vertex_count * 3);
            let idx_idx = primitive["indices"].as_u64().expect("indices") as usize;
            let indices = accessor_values(&parts, idx_idx);
            for index in indices {
                assert!(
                    (index as usize) < vertex_count,
                    "index {index} within vertex count {vertex_count}"
                );
            }
        }
    }
}
