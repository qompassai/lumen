//! Adversarial tests for the Phase 4 validator: corrupted files must
//! fail by the *named* rule they break, and truncated files must
//! produce a failing report, never a panic.

use lumen_factory::base::build_base_mesh;
use lumen_factory::gltf::{MorphTarget, build_glb, parse_glb_parts, write_glb};
use lumen_factory::validate::validate_bytes;

fn base_bytes() -> Vec<u8> {
    write_glb(&build_base_mesh().unwrap(), &[]).unwrap()
}

fn bin_start(bytes: &[u8]) -> usize {
    let json_len = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
    20 + json_len + 8
}

/// File offset of accessor `index`'s data.
fn accessor_file_offset(bytes: &[u8], accessor_index: usize) -> usize {
    let parts = parse_glb_parts(bytes).unwrap();
    let accessor = &parts.json["accessors"][accessor_index];
    let view_index = accessor["bufferView"].as_u64().unwrap() as usize;
    let view = &parts.json["bufferViews"][view_index];
    let view_offset = view["byteOffset"].as_u64().unwrap_or(0) as usize;
    let accessor_offset = accessor["byteOffset"].as_u64().unwrap_or(0) as usize;
    bin_start(bytes) + view_offset + accessor_offset
}

fn primitive_accessor_index(bytes: &[u8], attribute: &str) -> usize {
    let parts = parse_glb_parts(bytes).unwrap();
    for mesh in parts.json["meshes"].as_array().unwrap() {
        for primitive in mesh["primitives"].as_array().unwrap() {
            if let Some(idx) = primitive["attributes"].get(attribute) {
                return idx.as_u64().unwrap() as usize;
            }
        }
    }
    panic!("attribute {attribute} not found");
}

fn check<'a>(
    report: &'a lumen_factory::validate::ValidationReport,
    name: &str,
) -> &'a lumen_factory::validate::ValidationCheck {
    report
        .checks
        .iter()
        .find(|c| c.name == name)
        .unwrap_or_else(|| panic!("check {name} missing"))
}

#[test]
fn corrupted_weight_fails_weight_sum_by_name() {
    let mut bytes = base_bytes();
    let accessor = primitive_accessor_index(&bytes, "WEIGHTS_0");
    let start = accessor_file_offset(&bytes, accessor);
    // Find a substantial weight and flip its sign bit (top byte of the
    // little-endian f32): the vertex sum moves far from 1.0.
    let mut flipped = false;
    for vertex in 0..1024 {
        for component in 0..4 {
            let offset = start + (vertex * 4 + component) * 4;
            let value = f32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap());
            if (0.2..0.9).contains(&value) {
                bytes[offset + 3] ^= 0x80;
                flipped = true;
                break;
            }
        }
        if flipped {
            break;
        }
    }
    assert!(flipped, "a substantial weight must exist");
    let report = validate_bytes(&bytes, "corrupt-weight.glb");
    assert!(!report.pass);
    assert!(
        !check(&report, "skin_weight_sums").pass,
        "detail: {}",
        check(&report, "skin_weight_sums").detail
    );
}

#[test]
fn tampered_position_min_fails_min_max_by_name() {
    let bytes = base_bytes();
    let parts = parse_glb_parts(&bytes).unwrap();
    let mut json = parts.json.clone();
    let pos = primitive_accessor_index(&bytes, "POSITION");
    let original = json["accessors"][pos]["min"][0].as_f64().unwrap();
    json["accessors"][pos]["min"][0] = serde_json::json!(original + 1.0);
    let tampered = build_glb(&json, &parts.bin).unwrap();
    let report = validate_bytes(&tampered, "tampered-min.glb");
    assert!(!report.pass);
    assert!(!check(&report, "position_min_max_exact").pass);
}

#[test]
fn nan_in_position_fails_no_nan_by_name() {
    let mut bytes = base_bytes();
    let accessor = primitive_accessor_index(&bytes, "POSITION");
    let start = accessor_file_offset(&bytes, accessor);
    bytes[start..start + 4].copy_from_slice(&f32::NAN.to_le_bytes());
    let report = validate_bytes(&bytes, "nan.glb");
    assert!(!report.pass);
    assert!(!check(&report, "no_nan_float_accessors").pass);
}

#[test]
fn joint_index_out_of_range_fails_by_name() {
    let mut bytes = base_bytes();
    let accessor = primitive_accessor_index(&bytes, "JOINTS_0");
    let start = accessor_file_offset(&bytes, accessor);
    bytes[start..start + 2].copy_from_slice(&999u16.to_le_bytes());
    let report = validate_bytes(&bytes, "bad-joint.glb");
    assert!(!report.pass);
    assert!(!check(&report, "skin_joint_indices_in_range").pass);
}

#[test]
fn truncated_file_is_a_failing_report_not_a_panic() {
    let bytes = base_bytes();
    for cut in [10usize, bytes.len() / 2, bytes.len() - 1] {
        let report = validate_bytes(&bytes[..cut], "truncated.glb");
        assert!(!report.pass, "cut {cut} must fail");
        assert!(
            report.checks.iter().any(|c| !c.pass),
            "cut {cut} must name a failing rule"
        );
    }
    let empty = validate_bytes(&[], "empty.glb");
    assert!(!empty.pass);
    assert!(!check(&empty, "container_magic").pass);
}

#[test]
fn data_uri_buffer_fails_single_buffer_by_name() {
    let bytes = base_bytes();
    let parts = parse_glb_parts(&bytes).unwrap();
    let mut json = parts.json.clone();
    json["buffers"][0]["uri"] = serde_json::json!("data:application/octet-stream;base64,AAAA");
    let tampered = build_glb(&json, &parts.bin).unwrap();
    let report = validate_bytes(&tampered, "data-uri.glb");
    assert!(!report.pass);
    assert!(!check(&report, "container_single_buffer_no_uri").pass);
}

#[test]
fn morph_name_count_mismatch_fails_by_name() {
    let mesh = build_base_mesh().unwrap();
    let head_len = mesh.head_masks.len();
    let target = MorphTarget {
        deltas: vec![[0.0, 0.001, 0.0]; head_len],
        name: "test_smile".to_string(),
    };
    let bytes = write_glb(&mesh, &[target]).unwrap();
    let parts = parse_glb_parts(&bytes).unwrap();
    let mut json = parts.json.clone();
    json["meshes"][0]["extras"]["targetNames"] = serde_json::json!(["test_smile", "extra_name"]);
    let tampered = build_glb(&json, &parts.bin).unwrap();
    let report = validate_bytes(&tampered, "morph-mismatch.glb");
    assert!(!report.pass);
    assert!(!check(&report, "morph_target_names_match").pass);
}

#[test]
fn bad_magic_fails_container_by_name() {
    let mut bytes = base_bytes();
    bytes[0] = b'X';
    let report = validate_bytes(&bytes, "bad-magic.glb");
    assert!(!report.pass);
    assert!(!check(&report, "container_magic").pass);
}
