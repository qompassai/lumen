//! Validation tests: per-band width/depth scales (GAP-11-4) —
//! widths are identity variables end-to-end (identity record →
//! parameter record → skeleton spans, profile rings, head radii,
//! limb tubes), measured on the built artifacts Canon-11-style
//! (vertex slices and part extents), and exactly neutral when
//! unspecified or specified at 1.0.

use std::path::PathBuf;

use lumen_factory::base::{ScaledBuild, build_base_mesh_parametric};
use lumen_factory::blend::load_templates;
use lumen_factory::identity::{BandScaleSet, load_record};
use lumen_factory::params::ParameterRecord;
use lumen_factory::project::{BuildOptions, build_character_with_params};

fn asset_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join(name)
}

fn templates() -> lumen_factory::blend::Templates {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/emotions_fixture.json");
    load_templates(&path).expect("fixture templates")
}

fn part_span(mesh: &lumen_factory::base::BaseMesh, name: &str, axis: usize) -> f32 {
    let part = mesh.parts.iter().find(|p| p.name == name).expect("part");
    let mut lo = f32::INFINITY;
    let mut hi = f32::NEG_INFINITY;
    for v in &part.vertices {
        lo = lo.min(v.position[axis]);
        hi = hi.max(v.position[axis]);
    }
    hi - lo
}

/// Width (X span) of the body part's vertex slice at height `y`.
fn slice_width(mesh: &lumen_factory::base::BaseMesh, y: f32) -> f32 {
    let part = mesh
        .parts
        .iter()
        .find(|p| p.name == "body")
        .expect("body part");
    let mut lo = f32::INFINITY;
    let mut hi = f32::NEG_INFINITY;
    for v in &part.vertices {
        if (v.position[1] - y).abs() < 0.005 {
            lo = lo.min(v.position[0]);
            hi = hi.max(v.position[0]);
        }
    }
    hi - lo
}

/// Depth (Z span) of the body part's vertex slice at height `y`.
fn slice_depth(mesh: &lumen_factory::base::BaseMesh, y: f32) -> f32 {
    let part = mesh
        .parts
        .iter()
        .find(|p| p.name == "body")
        .expect("body part");
    let mut lo = f32::INFINITY;
    let mut hi = f32::NEG_INFINITY;
    for v in &part.vertices {
        if (v.position[1] - y).abs() < 0.005 {
            lo = lo.min(v.position[2]);
            hi = hi.max(v.position[2]);
        }
    }
    hi - lo
}

fn joint_x(mesh: &lumen_factory::base::BaseMesh, name: &str) -> f32 {
    mesh.joints
        .iter()
        .find(|j| j.name == name)
        .map(|j| j.position[0])
        .expect("joint")
}

#[test]
fn torso_width_scale_widens_the_waist_and_bust_rings() {
    let plain = build_base_mesh_parametric(&ScaledBuild::default()).expect("plain build");
    let wide = build_base_mesh_parametric(&ScaledBuild {
        band_width_scales: [1.0, 1.25, 1.0],
        ..ScaledBuild::default()
    })
    .expect("wide build");
    // Waist and bust rings, at the lathe's actual ring heights
    // (the body lathe samples 0.92..=1.50 in 18 steps; the profile
    // waist sits at ring 1.1456, the bust at ring 1.3067).
    for y in [1.1456f32, 1.3067] {
        let (a, b) = (slice_width(&plain, y), slice_width(&wide, y));
        assert!(
            (b / a - 1.25).abs() < 0.01,
            "ring at {y}: width {a} -> {b}, expected x1.25"
        );
    }
    // Hip ring (y = 0.92, leg band) is untouched by a torso scale.
    let (a, b) = (slice_width(&plain, 0.92), slice_width(&wide, 0.92));
    assert!((b - a).abs() < 1e-6, "hip ring must not move: {a} -> {b}");
}

#[test]
fn torso_depth_scale_deepens_the_rings() {
    let plain = build_base_mesh_parametric(&ScaledBuild::default()).expect("plain build");
    let deep = build_base_mesh_parametric(&ScaledBuild {
        band_depth_scales: [1.0, 1.3, 1.0],
        ..ScaledBuild::default()
    })
    .expect("deep build");
    let (a, b) = (slice_depth(&plain, 1.31), slice_depth(&deep, 1.31));
    assert!(
        (b / a - 1.3).abs() < 0.01,
        "bust ring depth {a} -> {b}, expected x1.3"
    );
    // And the width is untouched by a depth scale.
    let (a, b) = (slice_width(&plain, 1.31), slice_width(&deep, 1.31));
    assert!((b - a).abs() < 1e-6, "width must not move: {a} -> {b}");
}

#[test]
fn head_width_and_depth_scales_reshape_the_head() {
    let plain = build_base_mesh_parametric(&ScaledBuild::default()).expect("plain build");
    let framed = build_base_mesh_parametric(&ScaledBuild {
        band_depth_scales: [1.2, 1.0, 1.0],
        band_width_scales: [1.2, 1.0, 1.0],
        ..ScaledBuild::default()
    })
    .expect("framed build");
    let (a, b) = (part_span(&plain, "head", 0), part_span(&framed, "head", 0));
    assert!(
        (b / a - 1.2).abs() < 0.01,
        "head width {a} -> {b}, expected x1.2"
    );
    let (a, b) = (part_span(&plain, "head", 2), part_span(&framed, "head", 2));
    assert!(
        (b / a - 1.2).abs() < 0.02,
        "head depth {a} -> {b}, expected x1.2 (the face-flatten step trims the front)"
    );
}

#[test]
fn width_scales_move_the_joint_spans() {
    let plain = build_base_mesh_parametric(&ScaledBuild::default()).expect("plain build");
    let framed = build_base_mesh_parametric(&ScaledBuild {
        joints: lumen_factory::base::scaled_joint_table_with_frame(1.0, 1.0, 1.0, (1.0, 1.25, 1.1)),
        band_width_scales: [1.0, 1.25, 1.1],
        ..ScaledBuild::default()
    })
    .expect("framed build");
    // Shoulder span (Arm joints, torso band): x1.25.
    let span =
        |m: &lumen_factory::base::BaseMesh| (joint_x(m, "RightArm") - joint_x(m, "LeftArm")).abs();
    assert!(
        (span(&framed) / span(&plain) - 1.25).abs() < 0.001,
        "shoulder span must take the torso width scale"
    );
    // Hip span (UpLeg joints, leg band): x1.1.
    let hips = |m: &lumen_factory::base::BaseMesh| {
        (joint_x(m, "RightUpLeg") - joint_x(m, "LeftUpLeg")).abs()
    };
    assert!(
        (hips(&framed) / hips(&plain) - 1.1).abs() < 0.001,
        "hip span must take the leg width scale"
    );
}

#[test]
fn unit_frame_scales_are_bit_identical_to_no_frame() {
    // Some(1.0 everywhere) must be the same build as None: every
    // scale enters as a multiplication by exactly 1.0.
    let record = load_record(&asset_path("seraphine.identity.json")).expect("identity record");
    let plain = ParameterRecord::from_identity(&record).expect("derive plain");
    let mut framed = plain.clone();
    framed.proportions.width_scales = Some(BandScaleSet {
        head: 1.0,
        leg: 1.0,
        torso: 1.0,
    });
    framed.proportions.depth_scales = Some(BandScaleSet {
        head: 1.0,
        leg: 1.0,
        torso: 1.0,
    });
    framed.record_hash = framed.compute_hash().expect("recompute hash");
    framed.validate().expect("framed params validate");
    // The shipped anchor was built with the toon grade on and the
    // ramp extras off (the params_validation anchor's options).
    let options = BuildOptions {
        ramp: false,
        toon: true,
    };
    let a =
        build_character_with_params(&record, &templates(), &plain, options).expect("plain build");
    let b =
        build_character_with_params(&record, &templates(), &framed, options).expect("framed build");
    assert_eq!(a, b, "a unit frame must rebuild the anchor byte-for-byte");
    let shipped = std::fs::read(asset_path("seraphine.glb")).expect("anchor glb");
    assert_eq!(a, shipped, "and both must equal the shipped anchor");
}

#[test]
fn identity_frame_fields_drive_the_build_end_to_end() {
    let mut record = load_record(&asset_path("seraphine.identity.json")).expect("identity record");
    record.width_scales = Some(BandScaleSet {
        head: 1.0,
        leg: 1.0,
        torso: 1.3,
    });
    record.record_hash = record.compute_hash().expect("recompute hash");
    let params = ParameterRecord::from_identity(&record).expect("derive params");
    assert_eq!(
        params.proportions.width_scales,
        Some(BandScaleSet {
            head: 1.0,
            leg: 1.0,
            torso: 1.3,
        }),
        "the identity's authored frame must ride through derivation"
    );
    assert_eq!(params.width_scales(), (1.0, 1.3, 1.0));
    let built =
        build_character_with_params(&record, &templates(), &params, BuildOptions::default())
            .expect("framed build");
    let shipped = std::fs::read(asset_path("seraphine.glb")).expect("anchor glb");
    assert_ne!(built, shipped, "a 1.3 torso frame must change the build");
}

#[test]
fn scaled_joint_table_with_unit_widths_matches_the_y_only_table() {
    let a = lumen_factory::base::scaled_joint_table(1.1, 0.9, 1.05);
    let b = lumen_factory::base::scaled_joint_table_with_frame(1.1, 0.9, 1.05, (1.0, 1.0, 1.0));
    assert_eq!(a.len(), b.len());
    for (ja, jb) in a.iter().zip(b.iter()) {
        assert_eq!(ja.position, jb.position, "joint {}", ja.name);
    }
}
