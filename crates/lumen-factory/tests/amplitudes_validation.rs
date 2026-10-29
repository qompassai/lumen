//! Validation tests: blendshape displacement amplitudes
//! (GAP-11-1) — the checked-in spec table for the worked character
//! (assets/seraphine.amplitudes.json) parses, covers every morph
//! target of seraphine.glb and every one of the 52 blend channels,
//! and re-measures clean against the live artifact and fields.

use std::path::PathBuf;

use lumen_factory::amplitudes::{
    AMPLITUDES_SCHEMA, AMPLITUDES_VERSION, AmplitudeTable, channel_amplitudes,
    measure_target_amplitudes,
};
use lumen_factory::blend::KNOWN_CHANNELS;

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

#[test]
fn checked_in_table_covers_the_worked_character() {
    let table = checked_in_table();
    assert_eq!(table.schema, AMPLITUDES_SCHEMA);
    assert_eq!(table.version, AMPLITUDES_VERSION);
    assert_eq!(table.source_glb, "seraphine.glb");
    // 28, not the 29 the Canon-03 Part B prose (P-34) claims: the
    // GLB's own targetNames and Canon-11's metrics-3d.json both
    // list exactly these 28. The prose count is a canon-record
    // erratum (flagged in the GAP-11 report); the artifact governs.
    assert_eq!(
        table.targets.len(),
        28,
        "seraphine.glb ships 28 morph targets; the table must cover each one"
    );
    assert_eq!(
        table.channels.len(),
        KNOWN_CHANNELS.len(),
        "the table must cover every blend channel"
    );
    for target in &table.targets {
        if target.name == "neutral" {
            // The neutral key IS the rest pose: zero displacement
            // by construction, and the table must say so exactly.
            assert_eq!(
                target.max_displacement_m, 0.0,
                "the neutral target is the rest pose and must not displace"
            );
        } else {
            assert!(
                target.max_displacement_m > 0.0,
                "target {} must displace something at weight 1.0",
                target.name
            );
        }
        assert!(target.vertices > 0, "target {} vertex count", target.name);
    }
}

#[test]
fn checked_in_table_remeasures_clean() {
    // The drift gate itself: re-measure the artifact and the
    // channel fields; anything beyond the recorded tolerance fails.
    let table = checked_in_table();
    let glb = std::fs::read(asset_path("seraphine.glb")).expect("anchor glb");
    table
        .check(&glb)
        .expect("the shipped artifact must match its amplitude spec");
}

#[test]
fn fresh_measurement_matches_the_table_within_tolerance() {
    let table = checked_in_table();
    let glb = std::fs::read(asset_path("seraphine.glb")).expect("anchor glb");
    let measured = measure_target_amplitudes(&glb).expect("measure targets");
    assert_eq!(measured.len(), table.targets.len());
    for recorded in &table.targets {
        let found = measured
            .iter()
            .find(|m| m.name == recorded.name)
            .unwrap_or_else(|| panic!("target {} missing from measurement", recorded.name));
        assert!(
            (found.max_displacement_m - recorded.max_displacement_m).abs() <= table.tolerance_m,
            "target {} drifted: recorded {} vs measured {}",
            recorded.name,
            recorded.max_displacement_m,
            found.max_displacement_m
        );
        assert_eq!(found.vertices, recorded.vertices);
    }
    let channels = channel_amplitudes().expect("measure channels");
    assert_eq!(channels.len(), KNOWN_CHANNELS.len());
    for name in KNOWN_CHANNELS {
        assert!(
            channels.iter().any(|c| c.channel == *name),
            "channel {name} missing from the amplitude measurement"
        );
    }
    let jaw = channels
        .iter()
        .find(|c| c.channel == "jawOpen")
        .expect("jawOpen channel");
    assert!(
        jaw.max_displacement_m > 0.005,
        "jawOpen must travel visibly at weight 1.0 (measured 0.00736 m), got {} m",
        jaw.max_displacement_m
    );
}

#[test]
fn table_json_round_trips() {
    let table = checked_in_table();
    let rendered = table.to_json().expect("render");
    let parsed = AmplitudeTable::from_json(&rendered).expect("re-parse");
    assert_eq!(parsed.source_glb_sha256, table.source_glb_sha256);
    assert_eq!(parsed.targets.len(), table.targets.len());
    assert_eq!(parsed.channels.len(), table.channels.len());
}
