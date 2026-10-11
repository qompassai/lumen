//! Oracle intake tests: validation (the rotation lands, the trellis
//! path is untouched, geometry is lossless) and adversarial (corrupt,
//! truncated, mislabeled, or hostile GLBs are refused, fail closed).

#![forbid(unsafe_code)]

use lumen::oracle::{
    INTAKE_SCHEMA, OracleSource, inspect_glb_bytes, intake_glb_bytes, intake_glb_file,
    world_positions,
};

/// One triangle whose vertices are asymmetric under a Y-180 rotation,
/// so a rotation (or a missing one) is unmistakable in the positions.
const TRIANGLE: [[f32; 3]; 3] = [[1.0, 0.5, -2.0], [-3.0, 1.5, 4.0], [0.25, -0.75, 0.5]];

/// Build a minimal valid GLB around `document` (JSON value serialized
/// here) and one binary chunk holding TRIANGLE's positions + indices.
fn fixture_glb(document: &serde_json::Value) -> Vec<u8> {
    let mut bin: Vec<u8> = Vec::new();
    for vertex in TRIANGLE {
        for component in vertex {
            bin.extend_from_slice(&component.to_le_bytes());
        }
    }
    for index in [0u16, 1, 2] {
        bin.extend_from_slice(&index.to_le_bytes());
    }
    while !bin.len().is_multiple_of(4) {
        bin.push(0);
    }
    assemble_glb(document, &bin)
}

fn assemble_glb(document: &serde_json::Value, bin: &[u8]) -> Vec<u8> {
    let mut json_bytes = serde_json::to_vec(document).expect("fixture json serializes");
    while !json_bytes.len().is_multiple_of(4) {
        json_bytes.push(b' ');
    }
    let total = 12 + 8 + json_bytes.len() + 8 + bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&0x4654_6c67u32.to_le_bytes());
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x4e4f_534au32.to_le_bytes());
    out.extend_from_slice(&json_bytes);
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x004e_4942u32.to_le_bytes());
    out.extend_from_slice(bin);
    out
}

/// The canonical minimal document: one mesh, one node, one scene.
fn triangle_document() -> serde_json::Value {
    serde_json::json!({
        "asset": {"version": "2.0"},
        "scene": 0,
        "scenes": [{"nodes": [0]}],
        "nodes": [{"mesh": 0}],
        "meshes": [{
            "primitives": [{
                "attributes": {"POSITION": 0},
                "indices": 1
            }]
        }],
        "accessors": [
            {"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3",
             "min": [-3.0, -0.75, -2.0], "max": [1.0, 1.5, 4.0]},
            {"bufferView": 1, "componentType": 5123, "count": 3, "type": "SCALAR"}
        ],
        "bufferViews": [
            {"buffer": 0, "byteOffset": 0, "byteLength": 36},
            {"buffer": 0, "byteOffset": 36, "byteLength": 6}
        ],
        "buffers": [{"byteLength": 44}]
    })
}

fn triangle_glb() -> Vec<u8> {
    fixture_glb(&triangle_document())
}

/// Extract the binary chunk bytes of a GLB (test-side, spec layout).
fn bin_chunk(glb: &[u8]) -> &[u8] {
    let json_len = u32::from_le_bytes([glb[12], glb[13], glb[14], glb[15]]) as usize;
    let bin_header = 20 + json_len;
    let bin_len = u32::from_le_bytes([
        glb[bin_header],
        glb[bin_header + 1],
        glb[bin_header + 2],
        glb[bin_header + 3],
    ]) as usize;
    &glb[bin_header + 8..bin_header + 8 + bin_len]
}

fn json_chunk(glb: &[u8]) -> serde_json::Value {
    let json_len = u32::from_le_bytes([glb[12], glb[13], glb[14], glb[15]]) as usize;
    serde_json::from_slice(&glb[20..20 + json_len]).expect("output json parses")
}

fn rotated(point: [f32; 3]) -> [f32; 3] {
    [-point[0], point[1], -point[2]]
}

fn assert_points_close(actual: &[[f32; 3]], expected: &[[f32; 3]]) {
    assert_eq!(actual.len(), expected.len(), "point counts differ");
    for (got, want) in actual.iter().zip(expected.iter()) {
        for axis in 0..3 {
            assert!(
                (got[axis] - want[axis]).abs() < 1e-6,
                "point mismatch: got {got:?}, want {want:?}"
            );
        }
    }
}

// ---------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------

#[test]
fn ok_pixal3d_intake_rotates_geometry_180_about_y() {
    let raw = triangle_glb();
    let before = world_positions(&raw).expect("raw positions");
    assert_points_close(&before, &TRIANGLE);
    let (out, report) = intake_glb_bytes(&raw, OracleSource::Pixal3d).expect("intake succeeds");
    assert_eq!(report.rotation_degrees_y, 180);
    assert_eq!(report.source, "pixal3d");
    assert_eq!(report.wrapper_nodes_added, 1);
    let after = world_positions(&out).expect("rotated positions");
    let expected: Vec<[f32; 3]> = TRIANGLE.iter().map(|p| rotated(*p)).collect();
    assert_points_close(&after, &expected);
    // The mechanism is a scene-graph wrapper with the Y-180 quaternion.
    let doc = json_chunk(&out);
    let nodes = doc["nodes"].as_array().expect("nodes array");
    assert_eq!(nodes.len(), 2);
    let wrapper = &nodes[1];
    assert_eq!(wrapper["name"], "lumen_oracle_y180");
    assert_eq!(wrapper["rotation"], serde_json::json!([0.0, 1.0, 0.0, 0.0]));
    assert_eq!(wrapper["children"], serde_json::json!([0]));
    assert_eq!(doc["scenes"][0]["nodes"], serde_json::json!([1]));
}

#[test]
fn ok_trellis_intake_is_byte_identical() {
    let raw = triangle_glb();
    let (out, report) = intake_glb_bytes(&raw, OracleSource::TrellisCpp).expect("intake succeeds");
    assert_eq!(out, raw, "trellis path must not change a single byte");
    assert_eq!(report.rotation_degrees_y, 0);
    assert_eq!(report.wrapper_nodes_added, 0);
    assert_eq!(report.input_fnv1a64, report.output_fnv1a64);
    assert_eq!(report.source, "trellis-cpp");
    let positions = world_positions(&out).expect("positions");
    assert_points_close(&positions, &TRIANGLE);
}

#[test]
fn ok_pixal3d_intake_is_lossless_in_counts_and_binary() {
    let raw = triangle_glb();
    let before = inspect_glb_bytes(&raw).expect("inspect raw");
    let (out, report) = intake_glb_bytes(&raw, OracleSource::Pixal3d).expect("intake succeeds");
    let after = inspect_glb_bytes(&out).expect("inspect output");
    assert_eq!(before.vertices, after.vertices);
    assert_eq!(before.triangles, after.triangles);
    assert_eq!(before.meshes, after.meshes);
    assert_eq!(before.primitives, after.primitives);
    assert_eq!(before.vertices, 3);
    assert_eq!(before.triangles, 1);
    assert!(report.bin_chunk_preserved);
    assert_eq!(
        bin_chunk(&raw),
        bin_chunk(&out),
        "binary chunk must be copied verbatim"
    );
    assert_ne!(
        out, raw,
        "pixal3d intake must actually change the container (JSON chunk)"
    );
    assert_eq!(report.vertices, before.vertices);
    assert_eq!(report.triangles, before.triangles);
}

#[test]
fn ok_source_parse_accepts_documented_tokens() {
    assert_eq!(
        OracleSource::parse("pixal3d").ok(),
        Some(OracleSource::Pixal3d)
    );
    assert_eq!(
        OracleSource::parse("Pixal3D").ok(),
        Some(OracleSource::Pixal3d)
    );
    assert_eq!(
        OracleSource::parse("trellis").ok(),
        Some(OracleSource::TrellisCpp)
    );
    assert_eq!(
        OracleSource::parse("trellis-cpp").ok(),
        Some(OracleSource::TrellisCpp)
    );
    assert_eq!(
        OracleSource::parse("trellis.cpp").ok(),
        Some(OracleSource::TrellisCpp)
    );
    assert_eq!(OracleSource::Pixal3d.rotation_degrees_y(), 180);
    assert_eq!(OracleSource::TrellisCpp.rotation_degrees_y(), 0);
}

#[test]
fn ok_inspect_reports_geometry() {
    let stats = inspect_glb_bytes(&triangle_glb()).expect("inspect succeeds");
    assert_eq!(stats.vertices, 3);
    assert_eq!(stats.triangles, 1);
    assert_eq!(stats.meshes, 1);
    assert_eq!(stats.primitives, 1);
    assert_eq!(stats.nodes, 1);
    assert_eq!(stats.scenes, 1);
    assert_eq!(stats.accessors, 2);
}

#[test]
fn ok_intake_is_deterministic() {
    let raw = triangle_glb();
    let (first, _) = intake_glb_bytes(&raw, OracleSource::Pixal3d).expect("first intake");
    let (second, _) = intake_glb_bytes(&raw, OracleSource::Pixal3d).expect("second intake");
    assert_eq!(first, second);
}

#[test]
fn ok_file_intake_writes_output_and_manifest() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    std::fs::write(root.join("in.glb"), triangle_glb()).expect("write fixture");
    let report = intake_glb_file(
        root,
        "in.glb",
        "out.glb",
        Some("out.intake.json"),
        OracleSource::Pixal3d,
    )
    .expect("file intake succeeds");
    assert_eq!(report.schema, INTAKE_SCHEMA);
    let written = std::fs::read(root.join("out.glb")).expect("output exists");
    let positions = world_positions(&written).expect("output positions");
    let expected: Vec<[f32; 3]> = TRIANGLE.iter().map(|p| rotated(*p)).collect();
    assert_points_close(&positions, &expected);
    let manifest = std::fs::read_to_string(root.join("out.intake.json")).expect("manifest exists");
    let manifest_json: serde_json::Value =
        serde_json::from_str(&manifest).expect("manifest parses");
    assert_eq!(manifest_json["source"], "pixal3d");
    assert_eq!(manifest_json["rotation_degrees_y"], 180);
    assert_eq!(manifest_json["schema"], INTAKE_SCHEMA);
}

// ---------------------------------------------------------------------
// Adversarial
// ---------------------------------------------------------------------

#[test]
fn adv_unknown_source_tokens_are_rejected() {
    for token in [
        "", "pixal", "trellis2", "oracle", " pixal3d", "pixal3d ", "blender",
    ] {
        assert!(
            OracleSource::parse(token).is_err(),
            "token {token:?} must be rejected"
        );
    }
}

#[test]
fn adv_empty_and_truncated_inputs_are_rejected() {
    assert!(inspect_glb_bytes(&[]).is_err());
    assert!(inspect_glb_bytes(&[0x67, 0x6c]).is_err());
    let raw = triangle_glb();
    let half = &raw[..raw.len() / 2];
    assert!(intake_glb_bytes(half, OracleSource::Pixal3d).is_err());
    // Header intact, JSON chunk cut short.
    let cut = &raw[..24];
    assert!(inspect_glb_bytes(cut).is_err());
}

#[test]
fn adv_bad_magic_and_container_version_are_rejected() {
    let mut bad_magic = triangle_glb();
    bad_magic[0] = b'X';
    assert!(inspect_glb_bytes(&bad_magic).is_err());
    let mut bad_version = triangle_glb();
    bad_version[4..8].copy_from_slice(&1u32.to_le_bytes());
    assert!(inspect_glb_bytes(&bad_version).is_err());
}

#[test]
fn adv_corrupt_json_is_rejected() {
    let mut raw = triangle_glb();
    // Overwrite the start of the JSON chunk with same-length garbage.
    raw[20] = b'!';
    assert!(inspect_glb_bytes(&raw).is_err());
}

#[test]
fn adv_trailing_garbage_is_rejected() {
    let mut raw = triangle_glb();
    raw.extend_from_slice(&[0u8; 4]);
    assert!(inspect_glb_bytes(&raw).is_err());
}

#[test]
fn adv_out_of_bounds_references_are_rejected() {
    let mut doc = triangle_document();
    doc["scenes"][0]["nodes"] = serde_json::json!([7]);
    assert!(inspect_glb_bytes(&fixture_glb(&doc)).is_err());
    let mut doc = triangle_document();
    doc["meshes"][0]["primitives"][0]["attributes"]["POSITION"] = serde_json::json!(9);
    assert!(inspect_glb_bytes(&fixture_glb(&doc)).is_err());
    let mut doc = triangle_document();
    doc["nodes"][0]["mesh"] = serde_json::json!(3);
    assert!(inspect_glb_bytes(&fixture_glb(&doc)).is_err());
    let mut doc = triangle_document();
    doc["accessors"][0]["bufferView"] = serde_json::json!(42);
    assert!(inspect_glb_bytes(&fixture_glb(&doc)).is_err());
}

#[test]
fn adv_truncated_buffer_is_rejected() {
    // Declared buffer longer than the binary chunk that carries it.
    let mut doc = triangle_document();
    doc["buffers"][0]["byteLength"] = serde_json::json!(1_000_000);
    assert!(inspect_glb_bytes(&fixture_glb(&doc)).is_err());
    // Accessor claiming more positions than its bufferView holds.
    let mut doc = triangle_document();
    doc["accessors"][0]["count"] = serde_json::json!(300);
    assert!(inspect_glb_bytes(&fixture_glb(&doc)).is_err());
}

#[test]
fn adv_unreviewed_required_extension_is_rejected() {
    let mut doc = triangle_document();
    doc["extensionsRequired"] = serde_json::json!(["KHR_draco_mesh_compression"]);
    assert!(inspect_glb_bytes(&fixture_glb(&doc)).is_err());
    // The reviewed, geometry-neutral extension both generators emit passes.
    let mut doc = triangle_document();
    doc["extensionsRequired"] = serde_json::json!(["EXT_texture_webp"]);
    doc["extensionsUsed"] = serde_json::json!(["EXT_texture_webp"]);
    assert!(inspect_glb_bytes(&fixture_glb(&doc)).is_ok());
}

#[test]
fn adv_external_buffer_uri_is_rejected() {
    let mut doc = triangle_document();
    doc["buffers"][0]["uri"] = serde_json::json!("https://example.invalid/evil.bin");
    assert!(inspect_glb_bytes(&fixture_glb(&doc)).is_err());
}

#[test]
fn adv_file_intake_rejects_bad_paths() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    std::fs::write(root.join("in.png"), triangle_glb()).expect("write misnamed fixture");
    // Wrong extension.
    assert!(intake_glb_file(root, "in.png", "out.glb", None, OracleSource::Pixal3d).is_err());
    // Absolute path.
    assert!(intake_glb_file(root, "/etc/passwd", "out.glb", None, OracleSource::Pixal3d).is_err());
    // Missing file.
    assert!(intake_glb_file(root, "missing.glb", "out.glb", None, OracleSource::Pixal3d).is_err());
    // Output escaping via wrong extension is refused by the write-path rules.
    std::fs::write(root.join("in.glb"), triangle_glb()).expect("write fixture");
    assert!(intake_glb_file(root, "in.glb", "out.png", None, OracleSource::Pixal3d).is_err());
}
