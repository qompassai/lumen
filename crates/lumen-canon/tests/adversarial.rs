//! Adversarial tests: tampering, skew, lane violations, malformed
//! input and out-of-band measurements are refused with typed,
//! evidence-carrying refusals — never silently accepted (SPEC §5,
//! §6, §10 S1 adversarial set).

use canon_record::{
    gate_bands, require_lineage, resolve, sha256_hex, validate_record, CanonHandle, CanonRecord,
    ChainEntry, ChainError, ChainParams, GateVerdict, Lane, Measurements, Refusal, ValidationIssue,
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

fn approx(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

fn drift_failures(verdict: GateVerdict) -> Vec<canon_record::BandFailure> {
    match verdict {
        GateVerdict::Refused(Refusal::DriftDetected { failures }) => failures,
        other => panic!("expected drift_detected refusal, got {other:?}"),
    }
}

#[test]
fn tampered_record_bytes_refused_record_stale() {
    let mut bytes = fixture_bytes("opal-record.json");
    let hash = sha256_hex(&bytes);
    bytes.push(b' ');
    let err = resolve("canon://ntranch/opal@1", &hash, &bytes, Lane::Ntranch)
        .expect_err("tampered bytes must refuse");
    assert!(matches!(err, Refusal::RecordStale { .. }), "got {err:?}");
}

#[test]
fn wrong_pinned_hash_refused_record_stale() {
    let bytes = fixture_bytes("opal-record.json");
    let err = resolve(
        "canon://ntranch/opal@1",
        &"0".repeat(64),
        &bytes,
        Lane::Ntranch,
    )
    .expect_err("wrong pin must refuse");
    assert!(matches!(err, Refusal::RecordStale { .. }), "got {err:?}");
}

#[test]
fn version_skew_refused_record_stale() {
    let bytes = fixture_bytes("opal-record.json");
    let hash = sha256_hex(&bytes);
    let err = resolve("canon://ntranch/opal@2", &hash, &bytes, Lane::Ntranch)
        .expect_err("version skew must refuse");
    assert!(matches!(err, Refusal::RecordStale { .. }), "got {err:?}");
}

#[test]
fn id_mismatch_refused_record_stale() {
    // The handle names a different character than the bytes carry —
    // the "fatter Manon" record cannot be served under Opal's handle.
    let bytes = fixture_bytes("opal-record.json");
    let hash = sha256_hex(&bytes);
    let err = resolve("canon://ntranch/manon@1", &hash, &bytes, Lane::Ntranch)
        .expect_err("id mismatch must refuse");
    assert!(matches!(err, Refusal::RecordStale { .. }), "got {err:?}");
}

#[test]
fn malformed_handles_refused() {
    for bad in [
        "",
        "canon://",
        "canon://ntranch/opal",
        "canon://ntranch/opal@",
        "canon://ntranch/opal@x",
        "canon://ntranch/opal@0",
        "canon://ntranch/opal@-1",
        "canon://ntranch/opal/extra@1",
        "canon://bogus/opal@1",
        "http://ntranch/opal@1",
        "canon://ntranch/@1",
        "canon://ntranch/Opal@1",
        "canon://ntranch/opal@1@2",
    ] {
        let err = CanonHandle::parse(bad).expect_err(&format!("{bad:?} must refuse"));
        assert!(
            matches!(err, Refusal::MalformedHandle { .. }),
            "{bad:?}: got {err:?}"
        );
    }
}

#[test]
fn nsfw_record_refused_to_qompass_caller() {
    let bytes = fixture_bytes("opal-record.json");
    let hash = sha256_hex(&bytes);
    let err = resolve("canon://ntranch/opal@1", &hash, &bytes, Lane::Qompass)
        .expect_err("nsfw record must refuse a qompass caller");
    assert!(matches!(err, Refusal::LaneMismatch { .. }), "got {err:?}");
}

#[test]
fn unclassified_record_defaults_nsfw_and_refuses_qompass() {
    // SPEC §10 S1 adversarial set: an unclassified record must
    // default to nsfw (fail closed) — never leak across the wall.
    let mut value: serde_json::Value =
        serde_json::from_str(&fixture("opal-record.json")).expect("value");
    value
        .as_object_mut()
        .expect("object")
        .remove("content_label");
    let bytes = serde_json::to_vec(&value).expect("serialize");
    let hash = sha256_hex(&bytes);
    let err = resolve("canon://ntranch/opal@1", &hash, &bytes, Lane::Qompass)
        .expect_err("unclassified must refuse a qompass caller");
    assert!(matches!(err, Refusal::LaneMismatch { .. }), "got {err:?}");
    // The home lane can still fetch it.
    assert!(resolve("canon://ntranch/opal@1", &hash, &bytes, Lane::Ntranch).is_ok());
}

#[test]
fn unknown_fields_rejected() {
    // SPEC §3: consumers refuse unknown required fields; within /1
    // extensions are additive-optional only, never surprise fields.
    let mut value: serde_json::Value =
        serde_json::from_str(&fixture("opal-record.json")).expect("value");
    value["surprise"] = serde_json::json!(1);
    assert!(serde_json::from_value::<CanonRecord>(value).is_err());

    let mut value: serde_json::Value =
        serde_json::from_str(&fixture("opal-record.json")).expect("value");
    value["style_fingerprint"]["proportion_bands"][0]["surprise"] = serde_json::json!(1);
    assert!(serde_json::from_value::<CanonRecord>(value).is_err());
}

#[test]
fn anchor_without_adoption_act_fails_validation() {
    let mut record = opal_record();
    record.anchors[0].decision_ref = String::new();
    let issues = validate_record(&record);
    assert!(
        issues.iter().any(|i| matches!(
            i,
            ValidationIssue::AnchorMissingAdoptionAct {
                field: "decision_ref",
                ..
            }
        )),
        "got {issues:?}"
    );
}

#[test]
fn machine_invariant_without_method_fails_validation() {
    let mut record = opal_record();
    if let canon_record::CheckMethod::Machine { method } = &mut record.identity_invariants[0].check
    {
        *method = String::new();
    } else {
        panic!("first fixture invariant is machine-measured");
    }
    let issues = validate_record(&record);
    assert!(
        issues
            .iter()
            .any(|i| matches!(i, ValidationIssue::MachineInvariantWithoutMethod(_))),
        "got {issues:?}"
    );
}

#[test]
fn tampered_band_tolerance_fails_validation() {
    let mut record = opal_record();
    record.style_fingerprint.proportion_bands[0].tolerance = 0.0;
    let issues = validate_record(&record);
    assert!(
        issues
            .iter()
            .any(|i| matches!(i, ValidationIssue::BandToleranceInvalid(_))),
        "got {issues:?}"
    );
}

#[test]
fn fatter_v1_set_refused_with_full_delta_table() {
    // The D-280 counterexample: Stage 1R v1's fatter-face set must
    // refuse on every band, each failure carrying its delta.
    let record = opal_record();
    let failures = drift_failures(gate_bands(
        &record,
        &measurements("measurements-v1-fatter.json"),
    ));
    assert_eq!(failures.len(), 6, "failures: {failures:?}");
    let by_metric = |name: &str| {
        failures
            .iter()
            .find(|f| f.metric == name)
            .unwrap_or_else(|| panic!("missing failure for {name}"))
    };
    let face = by_metric("face_width_height_ratio");
    assert!(approx(face.delta.unwrap(), 0.49) && approx(face.excess.unwrap(), 0.37));
    let jaw = by_metric("jaw_taper_ratio");
    assert!(approx(jaw.delta.unwrap(), 0.18) && approx(jaw.excess.unwrap(), 0.12));
    let eye = by_metric("eye_width_share");
    assert!(approx(eye.delta.unwrap(), -0.08) && approx(eye.excess.unwrap(), 0.06));
    let aperture = by_metric("eye_aperture_ratio");
    assert!(approx(aperture.delta.unwrap(), 0.03) && approx(aperture.excess.unwrap(), 0.02));
    let span = by_metric("eye_centre_span_ratio");
    assert!(approx(span.delta.unwrap(), -0.13) && approx(span.excess.unwrap(), 0.10));
    let bust = by_metric("bust_footprint_share");
    assert!(approx(bust.delta.unwrap(), -0.34) && approx(bust.excess.unwrap(), 0.19));
}

#[test]
fn v2_corrected_set_refused_with_named_residuals() {
    // GATE-V2 named four residuals rather than rounding them into
    // the band; the gate must name exactly those four, and pass the
    // aperture (in band) and the bust (in-costume floor met).
    let record = opal_record();
    let failures = drift_failures(gate_bands(
        &record,
        &measurements("measurements-v2-corrected.json"),
    ));
    let names: Vec<&str> = failures.iter().map(|f| f.metric.as_str()).collect();
    assert_eq!(
        names,
        vec![
            "eye_centre_span_ratio",
            "eye_width_share",
            "face_width_height_ratio",
            "jaw_taper_ratio"
        ]
    );
}

#[test]
fn missing_measurement_refused() {
    let record = opal_record();
    let mut m = measurements("measurements-ardent-r2.json");
    m.measurements.remove("bust_footprint_share");
    let failures = drift_failures(gate_bands(&record, &m));
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].metric, "bust_footprint_share");
    assert_eq!(failures[0].measured, None);
}

#[test]
fn nonfinite_measurement_refused() {
    let record = opal_record();
    let mut m = measurements("measurements-ardent-r2.json");
    m.measurements
        .insert("face_width_height_ratio".to_string(), f64::NAN);
    let failures = drift_failures(gate_bands(&record, &m));
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0].metric, "face_width_height_ratio");
}

#[test]
fn chain_append_version_skew_refused() {
    let mut record = opal_record();
    let err = record
        .append_chain_entry(ChainEntry {
            artifact: "artifact://example/x.png".to_string(),
            sha256: "b".repeat(64),
            backend: "test".to_string(),
            params: ChainParams::default(),
            snapshot_id: None,
            record_version: 2,
            gate_verdict_ref: None,
            adopted: false,
        })
        .expect_err("skewed entry must refuse");
    assert_eq!(
        err,
        ChainError::VersionSkew {
            entry: 2,
            record: 1
        }
    );
}

#[test]
fn chain_append_bad_hash_refused() {
    let mut record = opal_record();
    let err = record
        .append_chain_entry(ChainEntry {
            artifact: "artifact://example/x.png".to_string(),
            sha256: "not-a-hash".to_string(),
            backend: "test".to_string(),
            params: ChainParams::default(),
            snapshot_id: None,
            record_version: 1,
            gate_verdict_ref: None,
            adopted: false,
        })
        .expect_err("bad hash must refuse");
    assert!(matches!(err, ChainError::BadHash(_)), "got {err:?}");
}

#[test]
fn unanchored_lineage_refused() {
    assert_eq!(
        require_lineage(None).expect_err("no lineage must refuse"),
        Refusal::UnanchoredLineage
    );
    assert_eq!(
        require_lineage(Some("  ")).expect_err("blank lineage must refuse"),
        Refusal::UnanchoredLineage
    );
    assert_eq!(
        require_lineage(Some("primary")).expect("lineage ok"),
        "primary"
    );
}
