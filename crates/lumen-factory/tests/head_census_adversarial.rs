//! Adversarial tests for the head-mesh census (T-6) and the
//! permanent-shadow precheck (T-13): each planted violation is
//! flagged by exactly its own rule, and the no-UV branch refuses
//! by name.

use lumen_factory::head_census::{
    HeadMeshData, PERMANENT_SHADOW_UV_REASON, TextureStep, census_head_mesh,
    preflight_texture_step, run_texture_step,
};

/// The same clean head as the validation suite's fixture:
/// distinct vertices, lower lid behind upper lid, cage [1],
/// one UV layer. Each test below plants exactly one violation.
fn clean_head() -> HeadMeshData {
    HeadMeshData {
        lower_lid_loop: vec![5, 6, 7],
        subdivision_levels: vec![1],
        upper_lid_loop: vec![2, 3, 4],
        uv_layers: vec!["UVMap".to_string()],
        vertices: vec![
            [0.0, 1.60, 0.10],
            [0.0, 0.80, 0.12],
            [-0.20, 1.30, 0.16],
            [0.0, 1.34, 0.18],
            [0.20, 1.30, 0.16],
            [-0.20, 1.24, 0.10],
            [0.0, 1.22, 0.11],
            [0.20, 1.24, 0.10],
            [0.0, 1.15, 0.22],
            [-0.40, 1.00, 0.05],
        ],
    }
}

fn check_passes(report: &lumen_factory::head_census::BlendCensusReport) -> [bool; 3] {
    [
        report.checks[0].pass,
        report.checks[1].pass,
        report.checks[2].pass,
    ]
}

#[test]
fn planted_duplicate_vertex_fails_only_rule_a() {
    let mut data = clean_head();
    let spare = data.vertices[9];
    data.vertices.push(spare);
    let report = census_head_mesh(&data).expect("head censuses");
    assert!(!report.pass);
    assert_eq!(check_passes(&report), [false, true, true]);
    assert_eq!(report.head.duplicate_vertex_groups, 1);
    assert_eq!(report.head.duplicate_vertex_excess, 1);
    assert!(!report.face_plausibility.duplicate_vertices_clean);
    assert!(report.face_plausibility.lid_depth_order_clean);
    assert!(report.face_plausibility.subdivision_cage_level_one);
}

#[test]
fn planted_lid_depth_swap_fails_only_rule_b() {
    let mut data = clean_head();
    // Exchange the loops' depths: the lower lid now sits in front.
    for index in [5, 6, 7] {
        data.vertices[index][2] += 0.07;
    }
    for index in [2, 3, 4] {
        data.vertices[index][2] -= 0.07;
    }
    let report = census_head_mesh(&data).expect("head censuses");
    assert!(!report.pass);
    assert_eq!(check_passes(&report), [true, false, true]);
    assert!(report.head.lower_lid_centroid_depth > report.head.upper_lid_centroid_depth);
    assert!(!report.face_plausibility.lid_depth_order_clean);
}

#[test]
fn planted_cage_level_two_fails_only_rule_c() {
    let mut data = clean_head();
    data.subdivision_levels = vec![2];
    let report = census_head_mesh(&data).expect("head censuses");
    assert!(!report.pass);
    assert_eq!(check_passes(&report), [true, true, false]);
    assert!(!report.face_plausibility.subdivision_cage_level_one);
}

#[test]
fn missing_modifier_is_recorded_and_fails_rule_c() {
    let mut data = clean_head();
    data.subdivision_levels = Vec::new();
    let report = census_head_mesh(&data).expect("head censuses");
    assert_eq!(check_passes(&report), [true, true, false]);
    assert!(report.head.subdivision_levels.is_empty());
    assert!(
        report.checks[2].detail.contains("[]"),
        "{}",
        report.checks[2].detail
    );
}

#[test]
fn permanent_shadow_refuses_by_name_without_a_uv_layer() {
    let mut data = clean_head();
    data.uv_layers = Vec::new();
    // The census itself is unaffected: UV presence is a checklist
    // precondition and a gate input, not a fourth hygiene rule.
    let report = census_head_mesh(&data).expect("head censuses");
    assert!(report.pass);
    assert!(!report.face_plausibility.uv_layer_present);

    let preflight =
        preflight_texture_step(&data, TextureStep::PermanentShadow).expect("preflight runs");
    assert!(!preflight.ready);
    assert_eq!(
        preflight.reason.as_deref(),
        Some(PERMANENT_SHADOW_UV_REASON)
    );
    assert!(
        PERMANENT_SHADOW_UV_REASON.starts_with("permanent_shadow_requires_uv_layer"),
        "{PERMANENT_SHADOW_UV_REASON}"
    );

    let error =
        run_texture_step(&data, TextureStep::PermanentShadow).expect_err("step must refuse");
    assert!(
        error
            .to_string()
            .contains("permanent_shadow_requires_uv_layer"),
        "{error}"
    );
}

#[test]
fn structurally_invalid_inputs_fail_closed() {
    let mut out_of_range = clean_head();
    out_of_range.lower_lid_loop = vec![5, 6, 99];
    assert!(census_head_mesh(&out_of_range).is_err());

    let mut empty_loop = clean_head();
    empty_loop.upper_lid_loop = Vec::new();
    assert!(census_head_mesh(&empty_loop).is_err());

    let mut repeated = clean_head();
    repeated.upper_lid_loop = vec![2, 3, 3];
    assert!(census_head_mesh(&repeated).is_err());

    let mut non_finite = clean_head();
    non_finite.vertices[0][1] = f32::NAN;
    assert!(census_head_mesh(&non_finite).is_err());

    let mut absurd_level = clean_head();
    absurd_level.subdivision_levels = vec![12];
    assert!(census_head_mesh(&absurd_level).is_err());

    let mut empty_name = clean_head();
    empty_name.uv_layers = vec![String::new()];
    assert!(census_head_mesh(&empty_name).is_err());
}
