//! INP1 importer tests: a synthetic INP1 built in-test
//! (validation), plus malformed/hostile containers
//! (adversarial).

use lumen_rig_core::inp::{ImportError, import_inp};
use serde_json::{Value, json};

fn payload_json() -> Value {
    json!({
        "meta": {"name": "Test Puppet", "version": "v0.test"},
        "physics": {"pixelsPerMeter": 1000.0, "gravity": 9.8},
        "param": [
            {
                "name": "Head:: Yaw", "uuid": 42, "is_vec2": true,
                "min": [-1.0, -1.0], "max": [1.0, 1.0], "defaults": [0.0, 0.0],
                "axis_points": [[0.0, 0.5, 1.0], [0.0, 1.0]],
                "bindings": [
                    {
                        "node": 7, "param_name": "transform.t.x",
                        "interpolate_mode": "Linear", "isSet": [[true, true], [true, true], [true, true]],
                        "values": [[0.0, 0.0], [10.0, 20.0], [30.0, 40.0]]
                    },
                    {
                        "node": 7, "param_name": "transform.r.z",
                        "interpolate_mode": "Cubic", "isSet": [[true, true], [true, true], [true, true]],
                        "values": [[0.0, 0.0], [0.5, 0.0], [1.0, 0.0]]
                    },
                    {
                        "node": 7, "param_name": "deform",
                        "interpolate_mode": "Linear", "isSet": [[true, true], [true, true], [true, true]],
                        "values": [
                            [[[0.0, 0.0], [0.0, 0.0], [0.0, 0.0]], [[0.0, 0.0], [0.0, 0.0], [0.0, 0.0]]],
                            [[[1.0, 1.0], [1.0, 1.0], [1.0, 1.0]], [[2.0, 2.0], [2.0, 2.0], [2.0, 2.0]]],
                            [[[0.0, 0.0], [0.0, 0.0], [0.0, 0.0]], [[0.0, 0.0], [0.0, 0.0], [0.0, 0.0]]]
                        ]
                    }
                ]
            },
            {
                "name": "Hair:: Physics", "uuid": 43, "is_vec2": false,
                "min": [-1.0, 0.0], "max": [1.0, 1.0], "defaults": [0.0, 0.0],
                "axis_points": [[0.0, 0.5, 1.0], [0.0]],
                "bindings": []
            }
        ],
        "nodes": {
            "name": "root", "uuid": 1, "type": "Node",
            "transform": {"trans": [0.0, 0.0, 0.0], "rot": [0.0, 0.0, 0.0], "scale": [1.0, 1.0]},
            "children": [
                {
                    "name": "Face", "uuid": 7, "type": "Part",
                    "transform": {"trans": [10.0, 20.0, 0.0], "rot": [0.0, 0.0, 0.0], "scale": [1.0, 1.0]},
                    "textures": [0],
                    "blend_mode": "Multiply",
                    "opacity": 0.8,
                    "mesh": {
                        "verts": [-5.0, -5.0, 5.0, 5.0, 0.0, 10.0],
                        "uvs": [0.0, 0.0, 1.0, 1.0, 0.5, 1.0],
                        "indices": [0, 1, 2],
                        "origin": [0.0, 0.0]
                    },
                    "children": []
                },
                {
                    "name": "Hair:: Physics", "uuid": 9, "type": "SimplePhysics",
                    "param": 43, "model_type": "SpringPendulum", "map_mode": "XY",
                    "gravity": 1.0, "length": 120.0, "frequency": 1.5,
                    "angle_damping": 0.5, "length_damping": 0.5,
                    "output_scale": [1.0, 1.0],
                    "transform": {"trans": [0.0, 0.0, 0.0], "rot": [0.0, 0.0, 0.0], "scale": [1.0, 1.0]},
                    "children": []
                }
            ]
        }
    })
}

fn build_inp(payload: &Value, tex_encoding: u8) -> Vec<u8> {
    let payload_bytes = serde_json::to_vec(payload).unwrap();
    let mut out = Vec::new();
    out.extend_from_slice(b"TRNSRTS\0");
    out.extend_from_slice(&(payload_bytes.len() as u32).to_be_bytes());
    out.extend_from_slice(&payload_bytes);
    out.extend_from_slice(b"TEX_SECT");
    out.extend_from_slice(&1u32.to_be_bytes());
    let tex = [0x89u8, 0x50, 0x4e, 0x47];
    out.extend_from_slice(&(tex.len() as u32).to_be_bytes());
    out.push(tex_encoding);
    out.extend_from_slice(&tex);
    out
}

/* ---------------- validation ---------------- */

#[test]
fn synthetic_inp_imports_with_exact_census() {
    let imported = import_inp(&build_inp(&payload_json(), 0), "fallback").unwrap();
    let report = &imported.report;
    assert_eq!(report.nodes, 3);
    assert_eq!(report.group_nodes, 1);
    assert_eq!(report.part_nodes, 1);
    assert_eq!(report.physics_nodes, 1);
    assert_eq!(report.parameters, 2);
    assert_eq!(report.vec2_parameters, 1);
    assert_eq!(report.bindings, 3);
    assert_eq!(report.cubic_bindings, 1);
    assert_eq!(report.textures, 1);
    assert_eq!(imported.model.id, "Test_Puppet");
    // INP1 axis points are normalized across [min, max]; the
    // stage stores them denormalized into value space.
    let head = &imported.stage.parameters[0];
    assert_eq!(head.axis_points[0], [-1.0, 0.0, 1.0]);
    assert_eq!(head.axis_points[1], [-1.0, 1.0]);
    // The imported stage re-parses from its canonical form.
    let text = imported.stage.to_canonical_json();
    let model = &imported.model;
    let again = lumen_rig_core::puppet::parse_puppet(text.as_bytes(), model).unwrap();
    assert_eq!(again, imported.stage);
}

#[test]
fn ids_are_sanitized_and_names_kept_in_provenance() {
    let imported = import_inp(&build_inp(&payload_json(), 0), "fallback").unwrap();
    let names: Vec<&str> = imported
        .stage
        .parameters
        .iter()
        .map(|p| p.id.as_str())
        .collect();
    assert_eq!(names, ["Head_Yaw", "Hair_Physics"]);
    let provenance = imported.stage.provenance.as_ref().unwrap();
    assert!(
        provenance
            .names
            .iter()
            .any(|pair| pair[0] == "Head_Yaw" && pair[1] == "Head:: Yaw")
    );
}

#[test]
fn rotation_bindings_convert_radians_to_degrees() {
    let imported = import_inp(&build_inp(&payload_json(), 0), "fallback").unwrap();
    let binding = imported
        .stage
        .bindings
        .iter()
        .find(|b| b.kind == lumen_rig_core::puppet::PuppetBindingKind::RotationZ)
        .unwrap();
    let lumen_rig_core::puppet::PuppetBindingValues::Scalar(grid) = &binding.values else {
        panic!("rotation binding must be scalar");
    };
    // 1.0 rad at the x=1 corner becomes ~57.2958 degrees.
    assert!((grid[2][0] - 57.29578).abs() < 0.01, "got {}", grid[2][0]);
}

/* ---------------- adversarial ---------------- */

#[test]
fn bad_magic_is_lri_001() {
    let mut bytes = build_inp(&payload_json(), 0);
    bytes[0] = b'X';
    assert_eq!(import_inp(&bytes, "f").unwrap_err().code(), "LRI-001");
}

#[test]
fn truncated_container_is_lri_002() {
    let bytes = build_inp(&payload_json(), 0);
    let cut = &bytes[..bytes.len() - 3];
    assert_eq!(import_inp(cut, "f").unwrap_err().code(), "LRI-002");
}

#[test]
fn bc7_texture_is_lri_005() {
    let bytes = build_inp(&payload_json(), 2);
    assert_eq!(import_inp(&bytes, "f").unwrap_err().code(), "LRI-005");
}

#[test]
fn unknown_blend_mode_is_lri_006() {
    let mut payload = payload_json();
    payload["nodes"]["children"][0]["blend_mode"] = json!("Overlay");
    let err = import_inp(&build_inp(&payload, 0), "f").unwrap_err();
    assert_eq!(err.code(), "LRI-006");
}

#[test]
fn unknown_node_type_is_lri_006() {
    let mut payload = payload_json();
    payload["nodes"]["children"][0]["type"] = json!("Bone");
    let err = import_inp(&build_inp(&payload, 0), "f").unwrap_err();
    assert_eq!(err.code(), "LRI-006");
}

#[test]
fn extra_texture_slot_is_counted_not_fatal() {
    // The reference runtime parses emissive/bump slots but
    // never renders them; the import keeps slot 0 and counts
    // the drop in the report (SPEC.md section 13.12).
    let mut payload = payload_json();
    payload["nodes"]["children"][0]["textures"] = json!([0, 0]);
    let imported = import_inp(&build_inp(&payload, 0), "f").unwrap();
    assert_eq!(imported.report.dropped_texture_slots, 1);
}

#[test]
fn binding_to_unknown_node_is_lri_007() {
    let mut payload = payload_json();
    payload["param"][0]["bindings"][0]["node"] = json!(999);
    let err = import_inp(&build_inp(&payload, 0), "f").unwrap_err();
    assert_eq!(err.code(), "LRI-007");
}

#[test]
fn missing_tex_section_is_lri_004() {
    let payload_bytes = serde_json::to_vec(&payload_json()).unwrap();
    let mut out = Vec::new();
    out.extend_from_slice(b"TRNSRTS\0");
    out.extend_from_slice(&(payload_bytes.len() as u32).to_be_bytes());
    out.extend_from_slice(&payload_bytes);
    out.extend_from_slice(b"NOPE_NOPE");
    let err = import_inp(&out, "f").unwrap_err();
    assert_eq!(err.code(), "LRI-004");
}

#[test]
fn garbage_payload_is_lri_003() {
    let mut out = Vec::new();
    out.extend_from_slice(b"TRNSRTS\0");
    out.extend_from_slice(&5u32.to_be_bytes());
    out.extend_from_slice(b"{oops");
    let err = import_inp(&out, "f").unwrap_err();
    assert_eq!(err.code(), "LRI-003");
}

#[test]
fn error_type_is_the_import_error() {
    let _: fn(&ImportError) -> &'static str = ImportError::code;
}
