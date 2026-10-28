//! Adversarial tests: every malformed fixture fails with
//! the exact typed error SPEC.md section 9 names for it.
//! No input in this file may parse successfully or panic.

use std::fs;
use std::path::Path;

use lumen_rig_core::limits::INPUT_BYTES_MAX;
use lumen_rig_core::{ParseError, parse_model};

fn fixture(relative: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("fixtures")
        .join("adversarial")
        .join(relative);
    fs::read(&path).unwrap_or_else(|error| panic!("fixture {relative}: {error}"))
}

fn error_of(relative: &str) -> ParseError {
    parse_model(&fixture(relative))
        .err()
        .unwrap_or_else(|| panic!("fixture {relative} must fail"))
}

#[test]
fn oversized_input_is_lrf_101() {
    let bytes = vec![b' '; INPUT_BYTES_MAX + 1];
    let error = parse_model(&bytes).expect_err("oversized input must fail");
    assert!(matches!(error, ParseError::InputTooLarge { .. }));
    assert_eq!(error.code(), "LRF-101");
}

#[test]
fn too_many_parameters_is_lrf_107() {
    let mut text = String::from(
        "{\"format\":\"lumen-rig-model\",\"format_version\":1,\"id\":\"x\",\"parameters\":[",
    );
    for index in 0..513 {
        if index > 0 {
            text.push(',');
        }
        text.push_str(&format!(
            "{{\"kind\":\"scalar\",\"id\":\"p{index}\",\"min\":0.0,\"max\":1.0,\"default\":0.0}}"
        ));
    }
    text.push_str("],\"parts\":[],\"textures\":[],\"deformers\":[],\"meshes\":[],\"physics\":[]}");
    let error = parse_model(text.as_bytes()).expect_err("513 parameters must fail");
    assert!(matches!(
        error,
        ParseError::CountExceeded { count: 513, .. }
    ));
    assert_eq!(error.code(), "LRF-107");
}

#[test]
fn truncated_json_is_lrf_102() {
    let error = error_of("truncated.json");
    assert!(matches!(error, ParseError::Syntax { .. }));
    assert_eq!(error.code(), "LRF-102");
}

#[test]
fn unknown_top_level_field_is_lrf_104() {
    let error = error_of("unknown-top-field.json");
    assert!(matches!(error, ParseError::UnknownField { .. }));
    assert_eq!(error.code(), "LRF-104");
}

#[test]
fn unknown_mesh_field_is_lrf_104() {
    let error = error_of("unknown-mesh-field.json");
    assert!(matches!(error, ParseError::UnknownField { .. }));
    assert_eq!(error.code(), "LRF-104");
}

#[test]
fn unknown_parameter_field_is_lrf_104() {
    let error = error_of("unknown-param-field.json");
    assert!(matches!(error, ParseError::UnknownField { .. }));
    assert_eq!(error.code(), "LRF-104");
}

#[test]
fn wrong_format_string_is_lrf_105() {
    let error = error_of("bad-format.json");
    assert!(matches!(error, ParseError::UnsupportedFormat { .. }));
    assert_eq!(error.code(), "LRF-105");
}

#[test]
fn wrong_version_is_lrf_106() {
    let error = error_of("bad-version.json");
    assert!(matches!(error, ParseError::UnsupportedVersion { found: 2 }));
    assert_eq!(error.code(), "LRF-106");
}

#[test]
fn invalid_id_charset_is_lrf_201() {
    let error = error_of("bad-id.json");
    assert!(matches!(error, ParseError::InvalidId { .. }));
    assert_eq!(error.code(), "LRF-201");
}

#[test]
fn duplicate_parameter_id_is_lrf_202() {
    let error = error_of("duplicate-param-id.json");
    assert!(matches!(
        error,
        ParseError::DuplicateId {
            collection: "parameters",
            ..
        }
    ));
    assert_eq!(error.code(), "LRF-202");
}

#[test]
fn dangling_texture_reference_is_lrf_203() {
    let error = error_of("dangling-texture.json");
    assert!(matches!(error, ParseError::DanglingReference { .. }));
    assert_eq!(error.code(), "LRF-203");
}

#[test]
fn part_parent_cycle_is_lrf_204() {
    let error = error_of("part-cycle.json");
    assert!(matches!(
        error,
        ParseError::ReferenceCycle {
            collection: "parts",
            ..
        }
    ));
    assert_eq!(error.code(), "LRF-204");
}

#[test]
fn mesh_clipping_itself_is_lrf_204() {
    let error = error_of("mesh-self-mask.json");
    assert!(matches!(
        error,
        ParseError::ReferenceCycle {
            collection: "meshes",
            ..
        }
    ));
    assert_eq!(error.code(), "LRF-204");
}

#[test]
fn combined_cycle_is_lrf_205() {
    let error = error_of("combined-cycle.json");
    assert!(matches!(error, ParseError::CombinedCycle { .. }));
    assert_eq!(error.code(), "LRF-205");
}

#[test]
fn escaping_texture_path_is_lrf_206() {
    let error = error_of("texture-escape.json");
    assert!(matches!(error, ParseError::InvalidTexturePath { .. }));
    assert_eq!(error.code(), "LRF-206");
}

#[test]
fn combined_lattice_axis_is_lrf_207() {
    let error = error_of("lattice-combined-axis.json");
    assert!(matches!(error, ParseError::WrongKind { .. }));
    assert_eq!(error.code(), "LRF-207");
}

#[test]
fn overflowing_coordinate_is_lrf_301() {
    let error = error_of("coord-overflow.json");
    assert!(matches!(error, ParseError::NonFinite { .. }));
    assert_eq!(error.code(), "LRF-301");
}

#[test]
fn inverted_scalar_range_is_lrf_302() {
    let error = error_of("scalar-min-max.json");
    assert!(matches!(error, ParseError::ParameterOrder { .. }));
    assert_eq!(error.code(), "LRF-302");
}

#[test]
fn default_out_of_range_is_lrf_303() {
    let error = error_of("default-out-of-range.json");
    assert!(matches!(error, ParseError::OutOfRange { .. }));
    assert_eq!(error.code(), "LRF-303");
}

#[test]
fn combined_range_escape_is_lrf_304() {
    let error = error_of("combined-range.json");
    assert!(matches!(
        error,
        ParseError::CombinedRangeNotContained { .. }
    ));
    assert_eq!(error.code(), "LRF-304");
}

#[test]
fn index_out_of_bounds_is_lrf_401() {
    let error = error_of("index-oob.json");
    assert!(matches!(
        error,
        ParseError::IndexOutOfBounds { index: 7, .. }
    ));
    assert_eq!(error.code(), "LRF-401");
}

#[test]
fn index_count_not_triangle_multiple_is_lrf_402() {
    let error = error_of("index-count.json");
    assert!(matches!(
        error,
        ParseError::IndexCountInvalid { count: 4, .. }
    ));
    assert_eq!(error.code(), "LRF-402");
}

#[test]
fn uv_length_mismatch_is_lrf_403() {
    let error = error_of("uv-mismatch.json");
    assert!(matches!(error, ParseError::LengthMismatch { .. }));
    assert_eq!(error.code(), "LRF-403");
}

#[test]
fn incomplete_lattice_is_lrf_404() {
    let error = error_of("lattice-incomplete.json");
    assert!(matches!(error, ParseError::LatticeInvalid { .. }));
    assert_eq!(error.code(), "LRF-404");
}

#[test]
fn noncanonical_lattice_order_is_lrf_404() {
    let error = error_of("lattice-order.json");
    assert!(matches!(error, ParseError::LatticeInvalid { .. }));
    assert_eq!(error.code(), "LRF-404");
}

#[test]
fn nonincreasing_lattice_keys_are_lrf_404() {
    let error = error_of("lattice-keys-order.json");
    assert!(matches!(error, ParseError::LatticeInvalid { .. }));
    assert_eq!(error.code(), "LRF-404");
}

#[test]
fn unknown_blend_mode_is_lrf_405() {
    let error = error_of("bad-blend.json");
    assert!(matches!(error, ParseError::UnsupportedBlendMode { .. }));
    assert_eq!(error.code(), "LRF-405");
}

#[test]
fn two_strands_one_output_is_lrf_601() {
    let error = error_of("physics-conflict.json");
    assert!(matches!(error, ParseError::PhysicsOutputConflict { .. }));
    assert_eq!(error.code(), "LRF-601");
}
