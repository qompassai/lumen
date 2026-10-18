//! Validation tests for the Phase 4 structural export validator:
//! committed assets and freshly built GLBs pass every named rule,
//! and the report serializes to the v1 schema.

use std::path::PathBuf;

use lumen_factory::base::build_base_mesh;
use lumen_factory::gltf::write_glb;
use lumen_factory::validate::{VALIDATION_SCHEMA, validate_bytes};

fn asset_bytes(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn check<'a>(report: &'a lumen_factory::validate::ValidationReport, name: &str) -> &'a lumen_factory::validate::ValidationCheck {
    report
        .checks
        .iter()
        .find(|c| c.name == name)
        .unwrap_or_else(|| panic!("check {name} missing from report"))
}

#[test]
fn built_base_glb_passes_validation() {
    let bytes = write_glb(&build_base_mesh().unwrap(), &[]).unwrap();
    let report = validate_bytes(&bytes, "in-test-base.glb");
    assert!(report.pass, "checks: {:?}", report.checks);
    assert_eq!(report.counts.joints, 65);
    assert_eq!(report.counts.nodes, 67);
    assert_eq!(report.counts.meshes, 1);
    assert_eq!(report.counts.primitives, 9);
    let bbox = report.bbox.expect("bbox present");
    assert!((1.0..=2.5).contains(&bbox.height), "height {}", bbox.height);
}

#[test]
fn committed_assets_all_pass() {
    for name in ["base.glb", "base_blended.glb", "seraphine.glb", "seraphine.vrm"] {
        let bytes = asset_bytes(name);
        let report = validate_bytes(&bytes, name);
        assert_eq!(report.size, bytes.len());
        assert!(
            report.pass,
            "{name} failed checks: {:?}",
            report.checks.iter().filter(|c| !c.pass).collect::<Vec<_>>()
        );
    }
}

#[test]
fn seraphine_counts_and_targets() {
    let report = validate_bytes(&asset_bytes("seraphine.glb"), "seraphine.glb");
    assert!(report.pass);
    assert_eq!(report.counts.joints, 65);
    assert_eq!(report.counts.primitives, 9);
    assert_eq!(report.counts.targets, 28);
    assert_eq!(report.counts.images, 1);
    assert!(report.vrm.is_none());
    assert!(check(&report, "texture_png_mime_and_dims").pass);
    assert!(check(&report, "texture_png_mime_and_dims").detail.contains("1024x1024"));
}

#[test]
fn seraphine_vrm_reports_flavor_and_bones() {
    let report = validate_bytes(&asset_bytes("seraphine.vrm"), "seraphine.vrm");
    assert!(report.pass);
    let vrm = report.vrm.as_ref().expect("vrm info present");
    assert_eq!(vrm.flavor, "vrm-1.0");
    assert_eq!(vrm.bones, 18);
    assert!(check(&report, "vrm_extension").detail.contains("vrm-1.0"));
}

#[test]
fn report_serializes_to_v1_schema() {
    let bytes = write_glb(&build_base_mesh().unwrap(), &[]).unwrap();
    let report = validate_bytes(&bytes, "schema.glb");
    let json = serde_json::to_value(&report).unwrap();
    assert_eq!(json["schema"], VALIDATION_SCHEMA);
    assert_eq!(json["version"], 1);
    assert_eq!(json["file"], "schema.glb");
    assert_eq!(json["pass"], true);
    let checks = json["checks"].as_array().unwrap();
    assert!(checks.len() >= 15, "named rules: {}", checks.len());
    for entry in checks {
        assert!(entry["name"].is_string());
        assert!(entry["pass"].is_boolean());
        assert!(entry["detail"].is_string());
    }
}

#[test]
fn every_named_rule_from_the_spec_is_present() {
    let bytes = write_glb(&build_base_mesh().unwrap(), &[]).unwrap();
    let report = validate_bytes(&bytes, "rules.glb");
    for name in [
        "container_magic",
        "container_version",
        "container_chunk_layout",
        "container_json_parses",
        "container_single_buffer_no_uri",
        "buffer_views_in_bounds",
        "accessors_component_types",
        "accessors_counts_consistent",
        "position_min_max_exact",
        "skin_ibm_count_matches_joints",
        "skin_weight_sums",
        "skin_joint_indices_in_range",
        "morph_target_names_match",
        "morph_target_position_counts",
        "scene_bbox_height",
        "no_nan_float_accessors",
        "texture_png_mime_and_dims",
        "vrm_extension",
    ] {
        check(&report, name);
    }
}
