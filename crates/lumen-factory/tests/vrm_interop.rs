//! Interop tests for the VRoid integration update: the factory
//! reader and the landed lumen VRoid wiring (`src/vrm.rs` in lumen
//! main: `vrm_inspect` / `vrm_grade`) against each other's files.
//!
//! `assets/seraphine_graded.vrm` is a real output of lumen's landed
//! `vrm_grade` (stdio MCP surface, preset `demo_toon_portrait`,
//! strength 1.0) applied to `assets/seraphine.vrm` — the factory's
//! own wrap. Grading rewrites only the embedded texture (appended
//! bufferView, repackaged container), so the factory's structural
//! extraction must be identical for both files, and the graded
//! file's proportions must drive a build through the VRM bridge in
//! `params.rs`.

use std::path::PathBuf;

use lumen_factory::blend::load_templates;
use lumen_factory::gltf::parse_glb;
use lumen_factory::identity::load_record;
use lumen_factory::params::ParameterRecord;
use lumen_factory::project::{BuildOptions, build_character_with_params};
use lumen_factory::validate::validate_bytes;
use lumen_factory::vrm::{VrmProportions, inspect_vrm};

fn manifest_path(parts: &[&str]) -> PathBuf {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    for part in parts {
        path.push(part);
    }
    path
}

fn asset_bytes(name: &str) -> Vec<u8> {
    let path = manifest_path(&["assets", name]);
    std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn close(a: f32, b: f32) -> bool {
    (a - b).abs() < 1e-4
}

#[test]
fn lumen_graded_vrm_inspects_identically_to_source() {
    let source = asset_bytes("seraphine.vrm");
    let graded = asset_bytes("seraphine_graded.vrm");
    assert_ne!(source, graded, "grading must change the file bytes");
    let a = inspect_vrm(&source).expect("inspect source");
    let b = inspect_vrm(&graded).expect("inspect lumen-graded file");
    assert_eq!(a.flavor, b.flavor);
    assert_eq!(a.meta, b.meta);
    assert_eq!(a.humanoid_bones, b.humanoid_bones);
    assert_eq!(a.preset_expressions, b.preset_expressions);
    assert_eq!(a.custom_expressions, b.custom_expressions);
    let pa = a.proportions.expect("source proportions");
    let pb = b.proportions.expect("graded proportions");
    assert!(close(pa.head_ratio, pb.head_ratio), "head ratio");
    assert!(close(pa.torso_ratio, pb.torso_ratio), "torso ratio");
    assert!(close(pa.leg_ratio, pb.leg_ratio), "leg ratio");
    assert!(close(pa.total_height, pb.total_height), "total height");
    // The Phase 4 validator accepts the lumen-repackaged container.
    let report = validate_bytes(&graded, "seraphine_graded.vrm");
    assert!(report.pass, "checks: {:?}", report.checks);
}

#[test]
fn vrm_proportions_drive_a_build() {
    let record = load_record(&manifest_path(&["assets", "seraphine.identity.json"])).unwrap();
    let templates =
        load_templates(&manifest_path(&["..", "..", "templates", "emotions.json"])).unwrap();
    let graded = asset_bytes("seraphine_graded.vrm");
    let summary = inspect_vrm(&graded).expect("inspect graded");
    let proportions = summary.proportions.expect("proportions");
    let via_vrm = ParameterRecord::from_identity_with_vrm_proportions(&record, &proportions)
        .expect("bridge derives a record");
    via_vrm.validate().expect("bridge record validates");
    // The VRM was wrapped from the identity-built GLB, so the two
    // proportion sources agree: scales match the identity derivation
    // to float noise.
    let via_identity = ParameterRecord::from_identity(&record).expect("identity record");
    for (vrm_scale, id_scale) in [
        (
            via_vrm.proportions.head_scale,
            via_identity.proportions.head_scale,
        ),
        (
            via_vrm.proportions.torso_scale,
            via_identity.proportions.torso_scale,
        ),
        (
            via_vrm.proportions.leg_scale,
            via_identity.proportions.leg_scale,
        ),
    ] {
        assert!(
            (vrm_scale - id_scale).abs() < 0.005,
            "vrm scale {vrm_scale} vs identity scale {id_scale}"
        );
    }
    // The bridged record builds a structurally clean character.
    let bytes = build_character_with_params(
        &record,
        &templates,
        &via_vrm,
        BuildOptions {
            ramp: false,
            toon: true,
        },
    )
    .expect("build from VRM proportions");
    let summary = parse_glb(&bytes).expect("built glb parses");
    assert_eq!(summary.joint_count, 65);
    let report = validate_bytes(&bytes, "vrm_build.glb");
    assert!(report.pass, "checks: {:?}", report.checks);
}

#[test]
fn bridge_rejects_degenerate_vrm_proportions() {
    let record = load_record(&manifest_path(&["assets", "seraphine.identity.json"])).unwrap();
    let base = VrmProportions {
        head_ratio: 0.30,
        head_span: 0.53,
        leg_ratio: 0.41,
        leg_span: 0.73,
        torso_ratio: 0.29,
        torso_span: 0.52,
        total_height: 1.78,
    };
    // Ratios that do not sum to ~1.
    let bad_sum = VrmProportions {
        head_ratio: 0.9,
        leg_ratio: 0.9,
        torso_ratio: 0.9,
        ..base
    };
    assert!(
        ParameterRecord::from_identity_with_vrm_proportions(&record, &bad_sum).is_err(),
        "sum-violating ratios must fail closed"
    );
    // Sum is 1 but the head scale lands far outside [0.5, 2.0].
    let bad_scale = VrmProportions {
        head_ratio: 0.98,
        leg_ratio: 0.01,
        torso_ratio: 0.01,
        ..base
    };
    assert!(
        ParameterRecord::from_identity_with_vrm_proportions(&record, &bad_scale).is_err(),
        "out-of-range scale must fail closed, not clamp"
    );
    // Non-finite ratio.
    let bad_nan = VrmProportions {
        head_ratio: f32::NAN,
        ..base
    };
    assert!(
        ParameterRecord::from_identity_with_vrm_proportions(&record, &bad_nan).is_err(),
        "NaN ratio must fail closed"
    );
}
