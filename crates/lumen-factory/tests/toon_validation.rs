//! Validation tests for Phase 5: the toon/ramp configuration, its
//! GLB extras round-trip, and the project build integration. The
//! no-ramp build must remain byte-identical to the Phase 2 anchor.

use std::path::PathBuf;

use lumen_factory::base::build_base_mesh;
use lumen_factory::blend::load_templates;
use lumen_factory::gltf::{parse_glb, parse_glb_parts, write_glb, write_glb_with_toon};
use lumen_factory::identity::load_record;
use lumen_factory::params::ParameterRecord;
use lumen_factory::project::{BuildOptions, build_character_with_params};
use lumen_factory::sha256_hex;
use lumen_factory::toon::{TOON_EXTRAS_KEY, ToonConfig};
use lumen_factory::validate::validate_bytes;

/// SHA-256 anchor of the Séraphine build (committed asset). Deliberate
/// re-anchors (projection v2, docs/factory/FACTORY-PLAN.md §9):
/// Phase 2/3 cd7abaca... -> fix 1 (color-segmented head) d41e41ac...
/// -> fix 2 (uniform head scale).
const SERAPHINE_ANCHOR: &str = "5f93fd6ea87c5f0e232f49eb914992e85b97c116535854b6101547f92773f37c";

fn manifest_path(parts: &[&str]) -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for part in parts {
        path.push(part);
    }
    path
}

fn lumen_toon(bytes: &[u8]) -> Option<serde_json::Value> {
    let parts = parse_glb_parts(bytes).unwrap();
    parts.json["meshes"][0]["extras"]
        .get(TOON_EXTRAS_KEY)
        .cloned()
}

#[test]
fn default_config_matches_plan_t14() {
    let config = ToonConfig::default();
    assert_eq!(config.ramp_thresholds, [0.35, 0.70]);
    assert_eq!(config.rim_strength, 0.35);
    assert_eq!(config.shadow_tint, "#2a2438");
    assert_eq!(config.highlight_tint, "#ffffff");
    config.validate().expect("default config validates");
}

#[test]
fn config_json_round_trip() {
    let config = ToonConfig::default();
    let json = config.to_json();
    let back = ToonConfig::from_json(&json).expect("round trip parses");
    assert_eq!(back, config);
    assert_eq!(
        back.shadow_rgb().unwrap(),
        [42.0 / 255.0, 36.0 / 255.0, 56.0 / 255.0]
    );
    assert_eq!(back.highlight_rgb().unwrap(), [1.0, 1.0, 1.0]);
}

#[test]
fn writer_with_toon_writes_mesh_extras() {
    let mesh = build_base_mesh().unwrap();
    let config = ToonConfig::default();
    let bytes = write_glb_with_toon(&mesh, &[], None, Some(&config)).unwrap();
    let found = lumen_toon(&bytes).expect("lumen_toon extras present");
    assert_eq!(ToonConfig::from_json(&found).unwrap(), config);
    // Morph names and the toon config coexist in the same extras object.
    assert!(validate_bytes(&bytes, "toon.glb").pass);
    let summary = parse_glb(&bytes).unwrap();
    assert_eq!(summary.joint_count, 65);
}

#[test]
fn writer_without_toon_has_no_key_and_is_byte_identical() {
    let mesh = build_base_mesh().unwrap();
    let plain = write_glb(&mesh, &[]).unwrap();
    let via_new = write_glb_with_toon(&mesh, &[], None, None).unwrap();
    assert_eq!(plain, via_new);
    assert!(lumen_toon(&plain).is_none());
}

#[test]
fn build_with_ramp_carries_config_and_validates() {
    let record = load_record(&manifest_path(&["assets", "seraphine.identity.json"])).unwrap();
    let templates =
        load_templates(&manifest_path(&["..", "..", "templates", "emotions.json"])).unwrap();
    let params =
        ParameterRecord::load(&manifest_path(&["assets", "seraphine.params.json"])).unwrap();
    let bytes = build_character_with_params(
        &record,
        &templates,
        &params,
        BuildOptions {
            ramp: true,
            toon: true,
        },
    )
    .unwrap();
    let found = lumen_toon(&bytes).expect("lumen_toon extras present");
    assert_eq!(
        ToonConfig::from_json(&found).unwrap(),
        ToonConfig::default()
    );
    let report = validate_bytes(&bytes, "seraphine_ramp.glb");
    assert!(report.pass, "checks: {:?}", report.checks);
}

#[test]
fn build_without_ramp_reproduces_seraphine_anchor() {
    let record = load_record(&manifest_path(&["assets", "seraphine.identity.json"])).unwrap();
    let templates =
        load_templates(&manifest_path(&["..", "..", "templates", "emotions.json"])).unwrap();
    let params =
        ParameterRecord::load(&manifest_path(&["assets", "seraphine.params.json"])).unwrap();
    let bytes = build_character_with_params(
        &record,
        &templates,
        &params,
        BuildOptions {
            ramp: false,
            toon: true,
        },
    )
    .unwrap();
    assert!(lumen_toon(&bytes).is_none());
    assert_eq!(sha256_hex(&bytes), SERAPHINE_ANCHOR);
}
