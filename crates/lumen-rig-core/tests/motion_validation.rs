//! Motion validation tests: well-formed motions parse against
//! their model, defaults resolve as SPEC.md section 4 names
//! them, and canonical emission round-trips identically.

use std::fs;
use std::path::Path;

use lumen_rig_core::{
    Interpolation, Model, Motion, PhaseProvenance, ProvenanceKind, parse_model, parse_motion,
};

fn fixture(relative: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("fixtures")
        .join(relative);
    fs::read(&path).unwrap_or_else(|error| panic!("fixture {relative}: {error}"))
}

fn parse_model_fixture(relative: &str) -> Model {
    parse_model(&fixture(relative))
        .unwrap_or_else(|error| panic!("model fixture {relative} should parse: {error}"))
}

fn parse_motion_fixture(relative: &str, model: &Model) -> Motion {
    parse_motion(&fixture(relative), model)
        .unwrap_or_else(|error| panic!("motion fixture {relative} should parse: {error}"))
}

#[test]
fn minimal_motion_parses_with_spec_defaults() {
    let model = parse_model_fixture("minimal.model.json");
    let motion = parse_motion_fixture("minimal.motion.json", &model);
    assert_eq!(motion.id, "nod");
    assert_eq!(motion.model, "minimal");
    assert_eq!(motion.duration_seconds, 1.0);
    assert!(!motion.looping, "absent loop resolves to false");
    assert_eq!(motion.frames_per_second, 30);
    assert_eq!(motion.curves.len(), 1);
    assert_eq!(motion.curves[0].fade_in_seconds, 0.0);
    assert_eq!(motion.curves[0].fade_out_seconds, 0.0);
    assert!(motion.baked_curves.is_empty());
    assert!(motion.fathom.is_none());
}

#[test]
fn full_motion_parses_all_blocks() {
    let model = parse_model_fixture("full.model.json");
    let motion = parse_motion_fixture("full.motion.json", &model);
    assert_eq!(motion.id, "greeting");
    assert!(motion.looping);
    assert_eq!(motion.curves.len(), 3);

    let cubic = &motion.curves[0];
    assert_eq!(cubic.parameter, "angle_x");
    assert_eq!(cubic.fade_in_seconds, 0.5);
    assert_eq!(cubic.fade_out_seconds, 0.25);
    assert_eq!(cubic.keyframes[0].interpolation, Interpolation::Cubic);
    assert_eq!(cubic.keyframes[0].tangent_out, Some([0.25, 5.0]));
    assert_eq!(cubic.keyframes[0].tangent_in, None);

    // A curve may target a combined parameter (section 4.1).
    assert_eq!(motion.curves[2].parameter, "gaze");

    assert_eq!(motion.baked_curves.len(), 1);
    let baked = &motion.baked_curves[0];
    assert_eq!(baked.parameter, "mouth_open");
    assert_eq!(baked.provenance.kind, ProvenanceKind::Envelope);
    assert_eq!(
        baked.provenance.source_audio_sha256.as_deref(),
        Some("0000000000000000000000000000000000000000000000000000000000000000")
    );
    assert_eq!(baked.provenance.window_seconds, Some(0.02));

    let fathom = motion.fathom.as_ref().expect("fathom block parses");
    assert_eq!(fathom.phases.len(), 2);
    assert_eq!(fathom.phases[0].provenance, PhaseProvenance::Measured);
    assert_eq!(fathom.phases[1].source, None);
}

#[test]
fn full_motion_canonical_round_trip_is_identical() {
    let model = parse_model_fixture("full.model.json");
    let motion = parse_motion_fixture("full.motion.json", &model);
    let emitted = motion.to_canonical_json();
    let reparsed = parse_motion(emitted.as_bytes(), &model)
        .unwrap_or_else(|error| panic!("canonical emission should re-parse: {error}"));
    assert_eq!(reparsed, motion);
    assert_eq!(reparsed.to_canonical_json(), emitted);
}

#[test]
fn minimal_motion_canonical_round_trip_is_identical() {
    let model = parse_model_fixture("minimal.model.json");
    let motion = parse_motion_fixture("minimal.motion.json", &model);
    let emitted = motion.to_canonical_json();
    let reparsed = parse_motion(emitted.as_bytes(), &model)
        .unwrap_or_else(|error| panic!("canonical emission should re-parse: {error}"));
    assert_eq!(reparsed, motion);
    assert_eq!(reparsed.to_canonical_json(), emitted);
}

#[test]
fn parse_is_a_pure_function_of_bytes_and_model() {
    let model = parse_model_fixture("full.model.json");
    let first = parse_motion_fixture("full.motion.json", &model);
    let second = parse_motion_fixture("full.motion.json", &model);
    assert_eq!(first, second);
}
