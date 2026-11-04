//! Calibration gates for the contact-pair probe
//! (ActLab Tool 2, D-943) against the real rig records.
//! These tests need the D-942 landed `.inx` records;
//! set `LUMEN_PROBE_INX_BASE` to the directory holding
//! `prima-full.inx` / `alma-full.inx` to run them. On a
//! machine without the records they return early (the
//! hermetic grammar gates are the core's
//! `probe_contracts`).
//!
//! The run of record these gates reproduce is the
//! D-875/D-942 lumen scene (evaluator of record):
//! `evidence/d942-scene-summary.json` of the full-rigs
//! programme — hand-to-waist band engaged max
//! [0.0, 0.0], Alma foot clearance at hold step 300 =
//! 73.91440643310546 rig units, on records
//! prima ce0f95de..., alma e0f6aceb... (template v1.2).

use std::path::PathBuf;

use lumen_engine_bevy::probe::run_probe;
use lumen_engine_core::probe::{ProbeReport, ProbeSpec};

const CALIBRATION_SPEC: &str = include_str!("../../../scenes/probe-lift-calibration.probe.ron");
const OFFSET_SPEC: &str = include_str!("../../../scenes/probe-lift-offset10.probe.ron");

/// The D-942 run of record: Alma foot clearance at
/// hold step 300.
const HOLD_CLEARANCE_STEP_300: f64 = 73.91440643310546;
/// The D-942 run of record: engaged hand-to-target
/// slip max (the plateau the longer arm travel costs).
const ENGAGED_SLIP_MAX: f64 = 13.199996948242239;

fn inx_base() -> Option<PathBuf> {
    std::env::var_os("LUMEN_PROBE_INX_BASE").map(PathBuf::from)
}

fn run_calibration() -> Option<ProbeReport> {
    let base = inx_base()?;
    let spec = ProbeSpec::load_ron(CALIBRATION_SPEC).expect("calibration spec parses");
    Some(run_probe(&spec, &base).expect("calibration probe runs"))
}

#[test]
fn calibration_reproduces_the_run_of_record() {
    let Some(report) = run_calibration() else {
        return;
    };
    assert_eq!(report.steps, 720);
    assert_eq!(
        report.rigs[0].sha256,
        "ce0f95de7c1ea300b6af1051c396482b7c1c38fdb496c6ec67c3cb650504c228"
    );
    assert_eq!(
        report.rigs[1].sha256,
        "e0f6aceb4f6423dd5bf92f49ddd2781a9236e2bf1f69b635125369e68cd062c2"
    );
    // Hand-to-waist band: engaged max exactly [0.0, 0.0]
    // in the run of record; the probe must land on the
    // same zero, not near it.
    for pair in &report.pairs {
        let band = pair.band_distance.expect("band aggregate");
        assert_eq!(band.max, 0.0, "pair {} band engaged max", pair.id);
        assert_eq!(band.mean, 0.0, "pair {} band engaged mean", pair.id);
        assert_eq!(pair.engaged_steps, 480, "pair {} engaged steps", pair.id);
        // Distance (seam slip): the D-942 plateau, to
        // the f64 the seam produces.
        let distance = pair.distance.expect("distance aggregate");
        assert!(
            (distance.max - ENGAGED_SLIP_MAX).abs() <= 1e-6,
            "pair {} slip max {} vs run of record {ENGAGED_SLIP_MAX}",
            pair.id,
            distance.max
        );
        // Penetration is reported (signed), never
        // silently absent on a declared pair.
        assert!(
            pair.has_penetration,
            "pair {} penetration capability",
            pair.id
        );
        assert!(
            pair.penetration.is_some(),
            "pair {} penetration aggregate",
            pair.id
        );
    }
    // Foot clearance at the hold checkpoint (step 300).
    let hold = report
        .checkpoints
        .iter()
        .find(|c| c.step == 300)
        .expect("hold checkpoint");
    let clearance = hold.clearance.expect("hold clearance");
    assert!(
        (clearance - HOLD_CLEARANCE_STEP_300).abs() <= 1e-6,
        "hold clearance {clearance} vs run of record {HOLD_CLEARANCE_STEP_300}"
    );
    // Crouch checkpoint (step 180): the run of record
    // reads 26.898079745044214; the probe reproduces it
    // inside the evaluator's f32 envelope (measured
    // deviation 1.2e-6, one f32 ulp at this magnitude).
    let crouch = report
        .checkpoints
        .iter()
        .find(|c| c.step == 180)
        .expect("crouch checkpoint");
    let crouch_clearance = crouch.clearance.expect("crouch clearance");
    assert!(
        (crouch_clearance - 26.898079745044214).abs() <= 2e-6,
        "crouch clearance {crouch_clearance} vs run of record 26.898079745044214"
    );
}

#[test]
fn adversarial_offset_reports_the_planted_error() {
    let Some(base) = inx_base() else {
        return;
    };
    let spec = ProbeSpec::load_ron(OFFSET_SPEC).expect("offset spec parses");
    let report = run_probe(&spec, &base).expect("offset probe runs");
    // The planted pair: 10.0 units down on the A anchor.
    // Its band-distance engaged max is exactly the
    // planted amount (the hand rides at the band's
    // bottom edge plane through the hold); the mean is
    // the filed 7.136205 (the hand rides inside the
    // band at other engaged steps).
    let planted = &report.pairs[0];
    let band = planted.band_distance.expect("planted band aggregate");
    assert!(
        (band.max - 10.0).abs() <= 1e-6,
        "planted band max {} vs planted 10.0",
        band.max
    );
    assert!(
        (band.mean - 7.136204856487469).abs() <= 1e-6,
        "planted band mean {} vs filed 7.136204856487469",
        band.mean
    );
    // The un-offset pair still reproduces the record.
    let clean = &report.pairs[1];
    let clean_band = clean.band_distance.expect("clean band aggregate");
    assert_eq!(clean_band.max, 0.0);
    assert_eq!(clean_band.mean, 0.0);
}

#[test]
fn double_run_reports_are_byte_identical() {
    let Some(first) = run_calibration() else {
        return;
    };
    let Some(second) = run_calibration() else {
        return;
    };
    let first_json = serde_json::to_string(&first).expect("serializes");
    let second_json = serde_json::to_string(&second).expect("serializes");
    assert_eq!(first_json, second_json);
    use sha2::{Digest, Sha256};
    let first_hash = format!("{:x}", Sha256::digest(first_json.as_bytes()));
    let second_hash = format!("{:x}", Sha256::digest(second_json.as_bytes()));
    assert_eq!(first_hash, second_hash);
}

#[test]
fn illegal_timeline_is_refused_before_evaluation() {
    let Some(base) = inx_base() else {
        return;
    };
    let mut spec = ProbeSpec::load_ron(CALIBRATION_SPEC).expect("spec parses");
    spec.rigs[0].pose_track[1]
        .params
        .insert("ParamArmL".to_string(), 1.5);
    let err = run_probe(&spec, &base).expect_err("out-of-range timeline refuses");
    assert!(err.contains("LE-110"), "{err}");
    assert!(err.contains("ParamArmL"), "{err}");

    let mut spec = ProbeSpec::load_ron(CALIBRATION_SPEC).expect("spec parses");
    spec.rigs[1].pose_track[4]
        .params
        .insert("ParamNope".to_string(), 0.5);
    let err = run_probe(&spec, &base).expect_err("unknown parameter refuses");
    assert!(err.contains("LE-110"), "{err}");
    assert!(err.contains("ParamNope"), "{err}");
}

#[test]
fn short_spec_runs_end_to_end_with_free_pair() {
    let Some(base) = inx_base() else {
        return;
    };
    let mut spec = ProbeSpec::load_ron(CALIBRATION_SPEC).expect("spec parses");
    spec.id = "lift-short".to_string();
    spec.steps = 120;
    spec.checkpoints = Vec::new();
    spec.clearance_rig = None;
    // A free (unbound) pair over the same anchors: its
    // distance is plain endpoint separation, measured
    // at every step.
    spec.pairs[0].contact = None;
    spec.pairs[0].band = None;
    spec.pairs[0].penetration_part = None;
    spec.pairs.truncate(1);
    let report = run_probe(&spec, &base).expect("short probe runs");
    assert_eq!(report.steps, 120);
    let pair = &report.pairs[0];
    assert_eq!(pair.engaged_steps, 0); // window opens at 2 s = step 120
    assert_eq!(pair.distance_series.len(), 120);
    assert!(pair.distance_series.iter().all(|d| d.is_some()));
    assert!(report.clearance_series.is_none());
}

#[test]
fn hash_mismatch_is_refused_before_import() {
    // No rig records needed: point the spec at this
    // crate's own manifest with a wrong expected hash.
    let mut spec = ProbeSpec::load_ron(CALIBRATION_SPEC).expect("spec parses");
    spec.rigs[0].inx = "Cargo.toml".to_string();
    spec.rigs[0].sha256 = "0".repeat(64);
    let base = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let err = run_probe(&spec, &base).expect_err("hash mismatch refuses");
    assert!(err.contains("PROBE-HASH-MISMATCH"), "{err}");
}
