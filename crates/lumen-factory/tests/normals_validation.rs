//! Validation tests for `normals-from-proxy/1` (T-12): the golden
//! transfer against hand-computed references, and the stage-1
//! read-back census of the converter's GLB output (T-4 clause).

use std::collections::BTreeMap;

use lumen_factory::base::{BaseMesh, MeshPart, Vertex};
use lumen_factory::convert::contract::{ExportCensus, verify_read_back};
use lumen_factory::convert::normals::{NormalsMode, contract, run_normals_from_proxy};
use lumen_factory::gltf::{parse_glb, write_glb};

const EPSILON: f32 = 1e-5;

fn vertex(position: [f32; 3], normal: [f32; 3]) -> Vertex {
    Vertex {
        joints: [0; 4],
        normal,
        position,
        uv: [0.0; 2],
        weights: [0.0; 4],
    }
}

/// The golden proxy: one triangle in the z=0 plane whose vertex
/// normals are the three axis directions, so barycentric
/// interpolation is hand-computable exactly.
fn golden_proxy() -> BaseMesh {
    BaseMesh {
        head_masks: Vec::new(),
        joints: Vec::new(),
        parts: vec![MeshPart {
            indices: vec![0, 1, 2],
            name: "head".to_string(),
            vertices: vec![
                vertex([0.0, 0.0, 0.0], [0.0, 0.0, 1.0]),
                vertex([2.0, 0.0, 0.0], [1.0, 0.0, 0.0]),
                vertex([0.0, 2.0, 0.0], [0.0, 1.0, 0.0]),
            ],
        }],
    }
}

/// Target vertices hovering 0.25 above the proxy plane, all with
/// original normal +Z:
/// A (0.5, 0.5) → barycentric (1/2, 1/4, 1/4) on the proxy.
/// B (1.0, 0.0) → barycentric (1/2, 1/2, 0).
/// C (0.0, 1.0) → barycentric (1/2, 0, 1/2), scope 0 (untouched).
/// D (0.5, 0.5) → same transfer as A, scope 1/2 (blend).
fn golden_target() -> BaseMesh {
    BaseMesh {
        head_masks: Vec::new(),
        joints: Vec::new(),
        parts: vec![MeshPart {
            indices: vec![0, 1, 2],
            name: "head".to_string(),
            vertices: vec![
                vertex([0.5, 0.5, 0.25], [0.0, 0.0, 1.0]),
                vertex([1.0, 0.0, 0.25], [0.0, 0.0, 1.0]),
                vertex([0.0, 1.0, 0.25], [0.0, 0.0, 1.0]),
                vertex([0.5, 0.5, 0.25], [0.0, 0.0, 1.0]),
            ],
        }],
    }
}

fn golden_scopes() -> BTreeMap<String, Vec<f32>> {
    BTreeMap::from([("head".to_string(), vec![1.0, 1.0, 0.0, 0.5])])
}

fn assert_normal(actual: [f32; 3], expected: [f32; 3]) {
    for (a, e) in actual.iter().zip(expected.iter()) {
        assert!(
            (a - e).abs() < EPSILON,
            "normal {actual:?} != expected {expected:?}"
        );
    }
}

#[test]
fn golden_transfer_matches_hand_computed_normals() {
    let (output, report) = run_normals_from_proxy(
        &golden_target(),
        &golden_proxy(),
        &golden_scopes(),
        NormalsMode::Direct,
    )
    .expect("golden transfer runs");
    let vertices = &output.parts[0].vertices;
    // A: interpolated (0.25, 0.25, 0.5), normalized.
    assert_normal(vertices[0].normal, [0.408248, 0.408248, 0.816497]);
    // B: interpolated (0.5, 0, 0.5), normalized: 1/sqrt(2) exactly.
    assert_normal(
        vertices[1].normal,
        [
            std::f32::consts::FRAC_1_SQRT_2,
            0.0,
            std::f32::consts::FRAC_1_SQRT_2,
        ],
    );
    // C: scope 0 — the original normal, exactly.
    assert_eq!(vertices[2].normal, [0.0, 0.0, 1.0]);
    // D: halfway between (0,0,1) and A's transfer, normalized:
    // (0.204124, 0.204124, 0.908248) / 0.953020.
    assert_normal(vertices[3].normal, [0.214183, 0.214183, 0.953020]);
    assert_eq!(report.parts_transferred, 1);
    assert_eq!(report.vertices_considered, 4);
    assert_eq!(report.vertices_blended, 3);
    assert_eq!(report.vertices_unchanged, 1);
    assert!(report.skin_preserved);
}

#[test]
fn converter_contract_names_itself_and_requires_read_back() {
    let record = contract();
    assert_eq!(record.converter_id, "normals-from-proxy/1");
    record.validate().expect("converter contract validates");
}

#[test]
fn output_glb_passes_stage1_read_back_and_truncation_fails() {
    let (output, _) = run_normals_from_proxy(
        &golden_target(),
        &golden_proxy(),
        &golden_scopes(),
        NormalsMode::Direct,
    )
    .expect("golden transfer runs");
    let bytes = write_glb(&output, &[]).expect("output writes as glb");
    let summary = parse_glb(&bytes).expect("output re-reads");
    let expected = ExportCensus::from(&summary);
    let report = verify_read_back(&bytes, &expected).expect("read-back verifies the output");
    assert!(report.pass);
    // The T-4 clause has teeth on this converter's output too.
    let truncated = &bytes[..bytes.len() / 2];
    assert!(verify_read_back(truncated, &expected).is_err());
}
