//! Validation tests: well-formed records, handles, resolutions and
//! measurement sets behave exactly as SPEC §3/§5/§6 say they should.
//! Fixtures carry real measured values from the D-280 record (see
//! fixtures/*.json `source` fields).

use canon_record::{
    gate_bands, resolve, sha256_hex, validate_record, CanonHandle, CanonRecord, ChainEntry,
    ChainParams, ContentLabel, GateVerdict, Lane, Measurements,
};

fn fixture(name: &str) -> String {
    let path = format!("{}/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

fn fixture_bytes(name: &str) -> Vec<u8> {
    let path = format!("{}/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

fn opal_record() -> CanonRecord {
    serde_json::from_str(&fixture("opal-record.json")).expect("fixture record parses")
}

fn measurements(name: &str) -> Measurements {
    serde_json::from_str(&fixture(name)).expect("fixture measurements parse")
}

#[test]
fn fixture_record_parses_and_validates_clean() {
    let record = opal_record();
    assert_eq!(record.id, "ntranch/opal");
    assert_eq!(record.effective_label(), ContentLabel::Nsfw);
    let issues = validate_record(&record);
    assert!(issues.is_empty(), "unexpected issues: {issues:?}");
}

#[test]
fn round_trip_is_stable() {
    let record = opal_record();
    let text = serde_json::to_string(&record).expect("serialize");
    let again: CanonRecord = serde_json::from_str(&text).expect("re-parse");
    assert_eq!(record, again);
}

#[test]
fn handle_parse_display_round_trip() {
    let handle = CanonHandle::parse("canon://ntranch/opal@1").expect("parse");
    assert_eq!(handle.lane, Lane::Ntranch);
    assert_eq!(handle.character, "opal");
    assert_eq!(handle.version, 1);
    assert_eq!(handle.to_string(), "canon://ntranch/opal@1");
    assert_eq!(handle.record_id(), "ntranch/opal");
}

#[test]
fn resolve_happy_path() {
    let bytes = fixture_bytes("opal-record.json");
    let hash = sha256_hex(&bytes);
    let record = resolve("canon://ntranch/opal@1", &hash, &bytes, Lane::Ntranch)
        .expect("resolution succeeds");
    assert_eq!(record, opal_record());
}

#[test]
fn sfw_record_fetchable_by_any_caller() {
    let mut value: serde_json::Value =
        serde_json::from_str(&fixture("opal-record.json")).expect("value");
    value["id"] = serde_json::json!("qompass/opal");
    value["origin_lane"] = serde_json::json!("qompass");
    value["content_label"] = serde_json::json!("sfw");
    let bytes = serde_json::to_vec(&value).expect("serialize");
    let hash = sha256_hex(&bytes);
    for caller in [Lane::Ntranch, Lane::Qompass] {
        let record = resolve("canon://qompass/opal@1", &hash, &bytes, caller)
            .expect("sfw record fetchable by any caller");
        assert_eq!(record.effective_label(), ContentLabel::Sfw);
    }
}

#[test]
fn unclassified_label_defaults_to_nsfw() {
    let mut record = opal_record();
    record.content_label = None;
    assert_eq!(record.effective_label(), ContentLabel::Nsfw);
}

#[test]
fn ardent_measurements_pass_gate() {
    let record = opal_record();
    let verdict = gate_bands(&record, &measurements("measurements-ardent-r2.json"));
    assert_eq!(verdict, GateVerdict::Pass);
}

#[test]
fn band_edges_are_inclusive() {
    let record = opal_record();
    // Aperture lower edge 0.16 and upper edge 0.18 both pass; bust
    // floor exactly 0.68 passes; face upper edge exactly 0.98 passes.
    for (metric, edge) in [
        ("eye_aperture_ratio", 0.16),
        ("eye_aperture_ratio", 0.18),
        ("bust_footprint_share", 0.68),
        ("face_width_height_ratio", 0.98),
    ] {
        let mut m = measurements("measurements-ardent-r2.json");
        m.measurements.insert(metric.to_string(), edge);
        assert_eq!(
            gate_bands(&record, &m),
            GateVerdict::Pass,
            "edge {metric}={edge} must pass"
        );
    }
}

#[test]
fn floor_band_binds_only_from_below() {
    let record = opal_record();
    let mut m = measurements("measurements-ardent-r2.json");
    // Above the 0.83 target is not a violation for a floor band.
    m.measurements
        .insert("bust_footprint_share".to_string(), 0.95);
    assert_eq!(gate_bands(&record, &m), GateVerdict::Pass);
}

#[test]
fn eye_only_invariants_do_not_block_the_gate() {
    // The gate measures bands only; eye-only invariants ride the
    // D-221 eye path (SPEC §6.1) and never gate mechanically.
    let record = opal_record();
    assert!(record
        .identity_invariants
        .iter()
        .any(|i| matches!(i.check, canon_record::CheckMethod::EyeOnly)));
    assert_eq!(
        gate_bands(&record, &measurements("measurements-ardent-r2.json")),
        GateVerdict::Pass
    );
}

#[test]
fn gate_is_deterministic_across_repeated_runs() {
    // SPEC §10 S1: same artifact + same record -> identical verdict, x3.
    let record = opal_record();
    let m = measurements("measurements-v1-fatter.json");
    let first = gate_bands(&record, &m);
    for _ in 0..2 {
        assert_eq!(gate_bands(&record, &m), first);
    }
    let rendered = serde_json::to_string(&first).expect("verdict serializes");
    assert_eq!(
        rendered,
        serde_json::to_string(&gate_bands(&record, &m)).expect("verdict serializes")
    );
}

#[test]
fn chain_append_accepts_current_version_entry() {
    let mut record = opal_record();
    let before = record.keeper_chain.len();
    record
        .append_chain_entry(ChainEntry {
            artifact: "artifact://example/opal-02-smiling.png".to_string(),
            sha256: "a".repeat(64),
            backend: "P-LOCAL img2img".to_string(),
            params: ChainParams {
                seed: Some(733_303),
                denoise: Some(0.50),
                steps: Some(28),
                cfg: Some(5.5),
                prompt_hash: None,
            },
            snapshot_id: None,
            record_version: 1,
            gate_verdict_ref: Some("gate://run/1".to_string()),
            adopted: false,
        })
        .expect("append succeeds");
    assert_eq!(record.keeper_chain.len(), before + 1);
    assert!(validate_record(&record).is_empty());
}

#[test]
fn pinned_anchor_hash_matches_the_real_artifact() {
    // The fixture record pins the R2 ardent anchor by the SHA-256 of
    // the real file on disk; if the artifact changes, this fails.
    // B2 absorption note: the artifact lives in the Hands-On tree,
    // outside this repo — and as an adult-lane anchor image it must
    // never be vendored into lumen (lane law). Candidate locations
    // cover the original B1 layout and the lumen staging layout;
    // on a machine without the artifact (e.g. the primo mirror)
    // the test falls back to the pin's internal consistency
    // (chain head == primary anchor, asserted below either way).
    let candidates = [
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../hands-on-opal-stage2-20261028/evidence/r2-anchor-face-2x.png"
        ),
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../../../../hands-on-opal-stage2-20261028/evidence/r2-anchor-face-2x.png"
        ),
    ];
    let bytes = candidates.iter().find_map(|path| std::fs::read(path).ok());
    let record = opal_record();
    let primary = record
        .anchors
        .iter()
        .find(|a| a.role == "primary")
        .expect("primary anchor");
    if let Some(bytes) = bytes {
        assert_eq!(sha256_hex(&bytes), primary.sha256);
    }
    assert_eq!(record.keeper_chain[0].sha256, primary.sha256);
}
