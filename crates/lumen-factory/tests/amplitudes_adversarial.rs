//! Adversarial tests: blendshape displacement amplitudes
//! (GAP-11-1) — the drift gate fails closed: a tampered amplitude,
//! a missing entry, the wrong artifact, a foreign schema, and
//! unreadable GLB bytes all refuse, naming what drifted; a
//! sub-tolerance wobble passes, proving the tolerance is real and
//! not a rubber stamp in either direction.

use std::path::PathBuf;

use lumen_factory::amplitudes::{AmplitudeTable, measure_target_amplitudes};

fn asset_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join(name)
}

fn checked_in_table() -> AmplitudeTable {
    let text = std::fs::read_to_string(asset_path("seraphine.amplitudes.json"))
        .expect("checked-in amplitude table reads");
    AmplitudeTable::from_json(&text).expect("checked-in amplitude table parses")
}

fn glb_bytes() -> Vec<u8> {
    std::fs::read(asset_path("seraphine.glb")).expect("anchor glb")
}

#[test]
fn tampered_target_amplitude_fails_and_names_the_target() {
    let mut table = checked_in_table();
    let victim = table.targets[0].name.clone();
    table.targets[0].max_displacement_m += 0.001; // 1 mm >> 1e-5 m tolerance
    let err = table
        .check(&glb_bytes())
        .expect_err("a 1 mm amplitude drift must fail the gate");
    assert!(
        err.to_string().contains(&victim),
        "the failure must name the drifted target {victim}: {err}"
    );
}

#[test]
fn tampered_channel_amplitude_fails_and_names_the_channel() {
    let mut table = checked_in_table();
    let victim = table.channels[3].channel.clone();
    table.channels[3].max_displacement_m += 0.001;
    let err = table
        .check(&glb_bytes())
        .expect_err("a 1 mm channel drift must fail the gate");
    assert!(
        err.to_string().contains(&victim),
        "the failure must name the drifted channel {victim}: {err}"
    );
}

#[test]
fn missing_target_entry_fails() {
    let mut table = checked_in_table();
    table.targets.pop();
    assert!(
        table.check(&glb_bytes()).is_err(),
        "a dropped target must fail the set comparison"
    );
}

#[test]
fn wrong_artifact_fails_on_the_hash() {
    let table = checked_in_table();
    let base = std::fs::read(asset_path("base.glb")).expect("base glb");
    let err = table
        .check(&base)
        .expect_err("base.glb is not the table's recorded source");
    assert!(
        err.to_string().contains("hash"),
        "the failure must be the source-hash check: {err}"
    );
}

#[test]
fn sub_tolerance_wobble_passes() {
    let mut table = checked_in_table();
    table.targets[0].max_displacement_m += table.tolerance_m * 0.5;
    table
        .check(&glb_bytes())
        .expect("half a tolerance of wobble is inside the gate");
}

#[test]
fn unreadable_glb_bytes_fail_closed() {
    assert!(measure_target_amplitudes(&[]).is_err(), "empty input");
    assert!(
        measure_target_amplitudes(b"not a glb at all").is_err(),
        "garbage input"
    );
    let glb = glb_bytes();
    assert!(
        measure_target_amplitudes(&glb[..glb.len() / 2]).is_err(),
        "a truncated GLB must fail, not measure half an artifact"
    );
    let table = checked_in_table();
    assert!(
        table.check(&glb[..100]).is_err(),
        "check against truncated bytes must fail"
    );
}

#[test]
fn foreign_schema_fails_closed() {
    let text = std::fs::read_to_string(asset_path("seraphine.amplitudes.json"))
        .expect("checked-in amplitude table reads");
    let poisoned = text.replacen("lumen.factory-amplitudes", "lumen.other", 1);
    assert_ne!(
        poisoned, text,
        "the schema string must be present to poison"
    );
    assert!(
        AmplitudeTable::from_json(&poisoned).is_err(),
        "a foreign schema must fail closed at parse time"
    );
}
