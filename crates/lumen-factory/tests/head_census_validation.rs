//! Validation tests for the head-mesh census (T-6) and the
//! texture-step checklist (T-13): a clean head passes every rule,
//! and the permanent-shadow step preflights ready when a UV layer
//! exists.

use lumen_factory::head_census::{
    HeadMeshData, PERMANENT_SHADOW_STEP, TextureStep, census_head_mesh, preflight_texture_step,
    run_texture_step,
};

/// A small clean head: distinct vertices, lower lid behind upper
/// lid, cage [1], one UV layer.
fn clean_head() -> HeadMeshData {
    HeadMeshData {
        lower_lid_loop: vec![5, 6, 7],
        subdivision_levels: vec![1],
        upper_lid_loop: vec![2, 3, 4],
        uv_layers: vec!["UVMap".to_string()],
        vertices: vec![
            [0.0, 1.60, 0.10],   // 0 forehead
            [0.0, 0.80, 0.12],   // 1 chin
            [-0.20, 1.30, 0.16], // 2 upper lid
            [0.0, 1.34, 0.18],   // 3 upper lid
            [0.20, 1.30, 0.16],  // 4 upper lid
            [-0.20, 1.24, 0.10], // 5 lower lid
            [0.0, 1.22, 0.11],   // 6 lower lid
            [0.20, 1.24, 0.10],  // 7 lower lid
            [0.0, 1.15, 0.22],   // 8 nose tip
            [-0.40, 1.00, 0.05], // 9 cheek spare
        ],
    }
}

#[test]
fn clean_head_passes_all_three_rules() {
    let report = census_head_mesh(&clean_head()).expect("clean head censuses");
    assert!(report.pass, "{report:?}");
    assert_eq!(report.checks.len(), 3);
    assert!(report.checks.iter().all(|check| check.pass));
    assert_eq!(report.checks[0].name, "duplicate_vertex_coordinates");
    assert_eq!(report.checks[1].name, "lower_lid_behind_upper_lid");
    assert_eq!(report.checks[2].name, "subdivision_cage_level");
    assert!(report.face_plausibility.duplicate_vertices_clean);
    assert!(report.face_plausibility.lid_depth_order_clean);
    assert!(report.face_plausibility.subdivision_cage_level_one);
    assert!(report.face_plausibility.uv_layer_present);
    assert_eq!(report.head.subdivision_levels, vec![1]);
    assert_eq!(report.uv_layer_count, 1);
}

#[test]
fn census_report_carries_the_blend_census_schema() {
    let report = census_head_mesh(&clean_head()).expect("clean head censuses");
    assert_eq!(report.schema, "lumen.blend-census");
    assert_eq!(report.version, 1);
    let json = serde_json::to_string(&report).expect("report serializes");
    assert!(json.contains("\"schema\":\"lumen.blend-census\""), "{json}");
    assert!(json.contains("\"uv_layer_present\":true"), "{json}");
}

#[test]
fn lid_centroids_measure_the_expected_depths() {
    let report = census_head_mesh(&clean_head()).expect("clean head censuses");
    let lower = report.head.lower_lid_centroid_depth;
    let upper = report.head.upper_lid_centroid_depth;
    assert!((lower - 0.10333).abs() < 0.001, "lower {lower}");
    assert!((upper - 0.16667).abs() < 0.001, "upper {upper}");
}

#[test]
fn permanent_shadow_preflights_ready_with_a_uv_layer() {
    let data = clean_head();
    let preflight =
        preflight_texture_step(&data, TextureStep::PermanentShadow).expect("preflight runs");
    assert!(preflight.ready);
    assert_eq!(preflight.reason, None);
    assert_eq!(preflight.step, PERMANENT_SHADOW_STEP);
    let ran = run_texture_step(&data, TextureStep::PermanentShadow).expect("step gate opens");
    assert!(ran.ready);
}

#[test]
fn negative_zero_is_the_same_coordinate_as_zero() {
    let mut data = clean_head();
    data.vertices[9] = [-0.0, 1.0, 0.05];
    data.vertices.push([0.0, 1.0, 0.05]);
    let report = census_head_mesh(&data).expect("head censuses");
    assert!(!report.checks[0].pass, "-0.0/+0.0 must read as a duplicate");
    assert_eq!(report.head.duplicate_vertex_groups, 1);
    assert_eq!(report.head.duplicate_vertex_excess, 1);
}
