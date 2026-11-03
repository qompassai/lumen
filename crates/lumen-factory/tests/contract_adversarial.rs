//! Adversarial tests for the converter contract clauses (T-2/T-4):
//! unnamed/wrong solvers refused, truncated and mismatched exports
//! fail with the census diff, broken meshes census as broken.

use lumen_factory::FactoryError;
use lumen_factory::base::build_base_mesh;
use lumen_factory::convert::contract::{
    BooleanSolver, ConverterContract, ExportCensus, ExportFormat, ReadBackRequirement,
    TriangleMesh, UnionContract, census_read_back, edge_manifoldness, edge_manifoldness_glb,
    verify_read_back,
};
use lumen_factory::gltf::{parse_glb, write_glb};

fn contract_with_union(union: UnionContract) -> ConverterContract {
    ConverterContract {
        converter_id: "fixture-union/1".to_string(),
        export_format: ExportFormat::Glb,
        read_back: ReadBackRequirement::Required,
        union: Some(union),
    }
}

#[test]
fn watertightness_critical_union_refuses_the_exact_solver() {
    let contract = contract_with_union(UnionContract::new(BooleanSolver::Exact, true));
    let error = contract
        .validate()
        .expect_err("critical exact union must refuse");
    assert!(error.to_string().contains("manifold"), "{error}");
}

#[test]
fn converter_id_must_be_a_name_version_token() {
    for bad_id in ["", "no-version", "white space/1"] {
        let mut contract = contract_with_union(UnionContract::watertight());
        contract.converter_id = bad_id.to_string();
        assert!(contract.validate().is_err(), "id {bad_id:?} must refuse");
    }
}

#[test]
fn golden_fixture_exact_union_is_not_watertight() {
    // The other half of D-730's proving fixture: under Blender's
    // exact solver the same fusion evaluates to an EMPTY mesh —
    // a union of non-empty solids cannot legitimately be empty,
    // so the census verdict is non-watertight.
    let bytes = include_bytes!("fixtures/boolean-exact.glb");
    let census = edge_manifoldness_glb(bytes).expect("exact fixture censuses");
    assert!(!census.watertight, "{census:?}");
    assert_eq!(census.vertex_count, 0);
    assert_eq!(census.triangle_count, 0);
}

#[test]
fn truncated_export_fails_read_back_with_the_diff() {
    // D-732's proving test: the harness truncates an export
    // mid-write; verification must fail, never succeed.
    let mesh = build_base_mesh().expect("base mesh builds");
    let bytes = write_glb(&mesh, &[]).expect("glb writes");
    let summary = parse_glb(&bytes).expect("export re-reads");
    let expected = ExportCensus::from(&summary);
    let truncated = &bytes[..bytes.len() / 2];

    let report = census_read_back(truncated, &expected);
    assert!(!report.pass);
    assert!(report.actual.is_none());
    assert_eq!(report.differences.len(), 1);
    assert_eq!(report.differences[0].field, "read_back");

    let error = verify_read_back(truncated, &expected).expect_err("truncated export must fail");
    match error {
        FactoryError::ReadBackMismatch(diff) => {
            assert!(diff.contains("read_back"), "{diff}");
        }
        other => panic!("expected ReadBackMismatch, got {other}"),
    }
}

#[test]
fn mismatched_expectation_fails_with_field_level_diff() {
    let mesh = build_base_mesh().expect("base mesh builds");
    let bytes = write_glb(&mesh, &[]).expect("glb writes");
    let summary = parse_glb(&bytes).expect("export re-reads");
    let mut expected = ExportCensus::from(&summary);
    expected.vertex_count += 1;
    let report = census_read_back(&bytes, &expected);
    assert!(!report.pass);
    assert_eq!(report.differences.len(), 1);
    assert_eq!(report.differences[0].field, "vertex_count");
    assert!(verify_read_back(&bytes, &expected).is_err());
}

#[test]
fn open_box_has_exactly_four_boundary_edges() {
    // Cube minus its two top triangles: the top rim is open.
    let mesh = TriangleMesh {
        indices: vec![
            0, 2, 1, 0, 3, 2, // bottom
            0, 1, 5, 0, 5, 4, // front
            3, 7, 6, 3, 6, 2, // back
            0, 4, 7, 0, 7, 3, // left
            1, 2, 6, 1, 6, 5, // right
        ],
        vertex_count: 8,
    };
    let census = edge_manifoldness(&mesh).expect("open box censuses");
    assert!(!census.watertight);
    assert_eq!(census.boundary_edge_count, 4);
    assert_eq!(census.nonmanifold_edge_count, 0);
}

#[test]
fn triple_shared_edge_is_nonmanifold() {
    // Three triangles fanning on edge (0,1).
    let mesh = TriangleMesh {
        indices: vec![0, 1, 2, 0, 1, 3, 0, 1, 4],
        vertex_count: 5,
    };
    let census = edge_manifoldness(&mesh).expect("fan censuses");
    assert!(!census.watertight);
    assert_eq!(census.nonmanifold_edge_count, 1);
}

#[test]
fn degenerate_triangles_are_counted_and_break_watertightness() {
    let mesh = TriangleMesh {
        indices: vec![0, 1, 1],
        vertex_count: 3,
    };
    let census = edge_manifoldness(&mesh).expect("degenerate censuses");
    assert_eq!(census.degenerate_triangle_count, 1);
    assert!(!census.watertight);
}

#[test]
fn empty_mesh_is_not_watertight() {
    let mesh = TriangleMesh {
        indices: Vec::new(),
        vertex_count: 0,
    };
    let census = edge_manifoldness(&mesh).expect("empty mesh censuses");
    assert!(!census.watertight);
}

#[test]
fn edge_census_refuses_out_of_range_and_ragged_indices() {
    let out_of_range = TriangleMesh {
        indices: vec![0, 1, 9],
        vertex_count: 3,
    };
    assert!(edge_manifoldness(&out_of_range).is_err());
    let ragged = TriangleMesh {
        indices: vec![0, 1],
        vertex_count: 3,
    };
    assert!(edge_manifoldness(&ragged).is_err());
}
