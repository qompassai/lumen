//! Canon B2 tests (lumen surface): 7 validation, 9 adversarial.
//!
//! Validation proves resolve/gate/generate behave on honest input,
//! including the B1 fixtures behaving identically through the
//! absorbed gate (Opal: ardent PASS, v1 set refuses all 6
//! proportion bands, v2 set refuses exactly the 4 GATE-V2
//! residuals). Adversarial proves the typed refusals: unanchored
//! job, fabricated anchor hash, nsfw handle to a qompass caller,
//! register drift with deltas, receipt tampering, and friends.
//! No character generation happens here (D-292): the broker is
//! exercised with fixtures and receipts only.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use canon_record::{GateVerdict, Measurements, Refusal};
use lumen::canon::{
    GateRequest, GenerateRequest, InputKind, JobInput, JobOutput, ResolveRequest, canon_gate,
    canon_generate, canon_resolve, verify_receipt,
};

const ANCHOR_SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const AUTHORITY_SHA: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const TURNAROUND_SHA: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const PLATE_SHA: &str = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
const OUTPUT_SHA: &str = "eeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeeee";

fn fixture(name: &str) -> String {
    let path = format!(
        "{}/crates/lumen-canon/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"))
}

fn measurements(name: &str) -> Measurements {
    serde_json::from_str(&fixture(name)).expect("fixture measurements")
}

/// A small SFW record with proportion + register bands, written
/// into the store of a temp project root. Returns its handle pin.
fn sfw_store(root: &Path) -> String {
    let record = serde_json::json!({
        "format": "canon-record/1",
        "id": "qompass/testchar",
        "version": 1,
        "origin_lane": "qompass",
        "content_label": "sfw",
        "identity_invariants": [
            {"name": "hair", "fact": "black bob", "check": {"kind": "eye-only"}}
        ],
        "anchors": [{
            "role": "primary", "handle": "artifact://test/anchor.png",
            "sha256": ANCHOR_SHA, "adopted_by": "Owner",
            "adopted_at": "2026-10-28", "decision_ref": "D-TEST"
        }],
        "style_fingerprint": {
            "proportion_bands": [
                {"metric": "face_width_height_ratio", "target": 0.86, "tolerance": 0.12, "method": "landmarks"}
            ],
            "register_bands": [
                {"metric": "ink_colour_distance", "target": 0.02, "tolerance": 0.03, "method": "ink cluster"},
                {"metric": "shadow_edge_hardness", "target": 0.9, "tolerance": 0.1, "method": "gradient"},
                {"metric": "blush_coverage", "target": 0.12, "tolerance": 0.05, "method": "blush share"}
            ],
            "rendering_markers": ["flat cel"],
            "palette": [{"role": "ink", "hex": "#160804", "tolerance": 0.05}]
        },
        "authority_refs": [{
            "handle": "artifact://test/authority.png", "sha256": AUTHORITY_SHA
        }],
        "keeper_chain": []
    });
    let dir = root.join("canon").join("qompass");
    std::fs::create_dir_all(&dir).expect("store dir");
    let bytes = serde_json::to_vec(&record).expect("serialize");
    std::fs::write(dir.join("testchar.json"), &bytes).expect("write record");
    canon_record::sha256_hex(&bytes)
}

fn opal_store(root: &Path) -> String {
    let bytes = fixture("opal-record.json").into_bytes();
    let dir = root.join("canon").join("ntranch");
    std::fs::create_dir_all(&dir).expect("store dir");
    std::fs::write(dir.join("opal.json"), &bytes).expect("write record");
    canon_record::sha256_hex(&bytes)
}

fn good_measurements() -> Measurements {
    let mut m = BTreeMap::new();
    m.insert("face_width_height_ratio".into(), 0.86);
    m.insert("ink_colour_distance".into(), 0.03);
    m.insert("shadow_edge_hardness".into(), 0.88);
    m.insert("blush_coverage".into(), 0.13);
    Measurements {
        measurements: m,
        source: None,
    }
}

fn good_inputs() -> Vec<JobInput> {
    vec![
        JobInput {
            kind: InputKind::Anchor,
            handle: "artifact://test/anchor.png".into(),
            sha256: ANCHOR_SHA.into(),
        },
        JobInput {
            kind: InputKind::StyleAuthority,
            handle: "artifact://test/authority.png".into(),
            sha256: AUTHORITY_SHA.into(),
        },
        JobInput {
            kind: InputKind::Turnaround,
            handle: "artifact://test/turnaround.png".into(),
            sha256: TURNAROUND_SHA.into(),
        },
        JobInput {
            kind: InputKind::Plate,
            handle: "artifact://test/plate.png".into(),
            sha256: PLATE_SHA.into(),
        },
    ]
}

fn good_job() -> GenerateRequest {
    GenerateRequest {
        handle: "canon://qompass/testchar@1".into(),
        record_sha256: String::new(), // filled by caller
        caller_lane: "qompass".into(),
        anchor_role: "primary".into(),
        backend: "fixture-backend".into(),
        params: canon_record::ChainParams {
            seed: Some(7),
            denoise: Some(0.5),
            steps: Some(20),
            cfg: Some(7.0),
            prompt_hash: None,
        },
        inputs: good_inputs(),
        outputs: vec![],
    }
}

fn resolve_req(sha: &str) -> ResolveRequest {
    ResolveRequest {
        handle: "canon://qompass/testchar@1".into(),
        record_sha256: sha.into(),
        caller_lane: "qompass".into(),
    }
}

// -------------------------------------------------------------------
// Validation
// -------------------------------------------------------------------

#[test]
fn resolve_sfw_record_summary() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sha = sfw_store(dir.path());
    let s = canon_resolve(dir.path(), &resolve_req(&sha)).expect("resolves");
    assert_eq!(s.id, "qompass/testchar");
    assert_eq!(s.content_label, "sfw");
    assert_eq!(s.proportion_band_count, 1);
    assert_eq!(s.register_band_count, 3);
    assert_eq!(s.adopted_anchor_hashes, vec![ANCHOR_SHA.to_string()]);
}

#[test]
fn gate_passes_good_measurements() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sha = sfw_store(dir.path());
    let v = canon_gate(
        dir.path(),
        &GateRequest {
            handle: "canon://qompass/testchar@1".into(),
            record_sha256: sha,
            caller_lane: "qompass".into(),
            anchor_sha256: Some(ANCHOR_SHA.into()),
            measurements: good_measurements(),
        },
    )
    .expect("gate runs");
    assert_eq!(v, GateVerdict::Pass);
}

#[test]
fn opal_ardent_passes_through_absorbed_gate() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sha = opal_store(dir.path());
    // Opal's record predates register bands, so the broker gate
    // (which requires them) is not used here; the absorbed B1 band
    // gate is the surface under test, via the crate directly and
    // via a record resolved through lumen's store.
    let record: canon_record::CanonRecord =
        serde_json::from_str(&fixture("opal-record.json")).expect("record");
    assert_eq!(
        canon_record::gate_bands(&record, &measurements("measurements-ardent-r2.json")),
        GateVerdict::Pass
    );
    let s = canon_resolve(
        dir.path(),
        &ResolveRequest {
            handle: "canon://ntranch/opal@1".into(),
            record_sha256: sha,
            caller_lane: "ntranch".into(),
        },
    )
    .expect("ntranch caller resolves opal");
    assert_eq!(s.proportion_band_count, 6);
    assert_eq!(
        s.chain_head_sha256.as_deref(),
        Some("1b6454c958a0bea3b44a1a9b76c259e6fbaed6d7405a94d77ca65ea30f5406be")
    );
}

#[test]
fn generate_admits_anchored_job_and_receipt_verifies() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sha = sfw_store(dir.path());
    let mut job = good_job();
    job.record_sha256 = sha;
    let receipt = canon_generate(dir.path(), &job).expect("admitted");
    assert_eq!(receipt.status, "admitted");
    assert_eq!(receipt.anchor_sha256, ANCHOR_SHA);
    assert!(receipt.workflow["nodes"].as_array().expect("nodes").len() == 4);
    let path: PathBuf = dir
        .path()
        .join("canon-receipts")
        .join(format!("{}.json", receipt.job_id));
    assert!(path.exists(), "receipt persisted");
    let verified = verify_receipt(dir.path(), &receipt.job_id).expect("verifies");
    assert_eq!(verified.receipt_sha256, receipt.receipt_sha256);
}

#[test]
fn generate_is_deterministic_x3() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sha = sfw_store(dir.path());
    let mut ids = Vec::new();
    for _ in 0..3 {
        let mut job = good_job();
        job.record_sha256 = sha.clone();
        ids.push(canon_generate(dir.path(), &job).expect("admitted").job_id);
    }
    assert!(
        ids.windows(2).all(|w| w[0] == w[1]),
        "job ids differ: {ids:?}"
    );
}

#[test]
fn generate_with_passing_output_admits_and_records_gate() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sha = sfw_store(dir.path());
    let mut job = good_job();
    job.record_sha256 = sha;
    job.outputs = vec![JobOutput {
        handle: "artifact://test/out.png".into(),
        sha256: OUTPUT_SHA.into(),
        measurements: good_measurements(),
    }];
    let receipt = canon_generate(dir.path(), &job).expect("admitted");
    assert_eq!(receipt.outputs.len(), 1);
    assert_eq!(receipt.outputs[0].gate, "pass");
}

#[test]
fn receipt_carries_reference_conditioning_inputs_by_hash() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sha = sfw_store(dir.path());
    let mut job = good_job();
    job.record_sha256 = sha;
    let receipt = canon_generate(dir.path(), &job).expect("admitted");
    let kinds: Vec<String> = receipt
        .inputs
        .iter()
        .map(|i| format!("{:?}", i.kind))
        .collect();
    assert!(kinds.contains(&"Turnaround".to_string()));
    assert!(kinds.contains(&"Plate".to_string()));
    assert!(receipt.inputs.iter().any(|i| i.sha256 == TURNAROUND_SHA));
    assert!(receipt.inputs.iter().any(|i| i.sha256 == PLATE_SHA));
}

// -------------------------------------------------------------------
// Adversarial
// -------------------------------------------------------------------

#[test]
fn adv_unanchored_job_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sha = sfw_store(dir.path());
    let mut job = good_job();
    job.record_sha256 = sha;
    job.anchor_role = String::new();
    assert!(matches!(
        canon_generate(dir.path(), &job),
        Err(Refusal::UnanchoredLineage)
    ));
}

#[test]
fn adv_job_missing_keeper_hashes_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sha = sfw_store(dir.path());
    let mut job = good_job();
    job.record_sha256 = sha;
    job.inputs.retain(|i| i.kind != InputKind::StyleAuthority);
    match canon_generate(dir.path(), &job) {
        Err(Refusal::MissingKeeperAnchor { missing }) => {
            assert_eq!(missing, vec![AUTHORITY_SHA.to_string()]);
        }
        other => panic!("expected missing_keeper_anchor, got {other:?}"),
    }
}

#[test]
fn adv_fabricated_anchor_hash_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sha = sfw_store(dir.path());
    let mut job = good_job();
    job.record_sha256 = sha;
    job.inputs.push(JobInput {
        kind: InputKind::Anchor,
        handle: "artifact://evil/fake.png".into(),
        sha256: "ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff".into(),
    });
    match canon_generate(dir.path(), &job) {
        Err(Refusal::AnchorNotAdopted { sha256 }) => {
            assert!(sha256.starts_with("ffff"));
        }
        other => panic!("expected anchor_not_adopted, got {other:?}"),
    }
}

#[test]
fn adv_nsfw_handle_refused_to_qompass_caller() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sha = opal_store(dir.path());
    let err = canon_resolve(
        dir.path(),
        &ResolveRequest {
            handle: "canon://ntranch/opal@1".into(),
            record_sha256: sha.clone(),
            caller_lane: "qompass".into(),
        },
    )
    .expect_err("nsfw to qompass must refuse");
    assert!(matches!(err, Refusal::LaneMismatch { .. }), "got {err:?}");
    // And through the broker as well.
    let mut job = good_job();
    job.handle = "canon://ntranch/opal@1".into();
    job.record_sha256 = sha;
    job.caller_lane = "qompass".into();
    assert!(matches!(
        canon_generate(dir.path(), &job),
        Err(Refusal::LaneMismatch { .. })
    ));
}

#[test]
fn adv_register_drift_refused_with_deltas() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sha = sfw_store(dir.path());
    let mut m = good_measurements();
    m.measurements.insert("ink_colour_distance".into(), 0.40);
    let v = canon_gate(
        dir.path(),
        &GateRequest {
            handle: "canon://qompass/testchar@1".into(),
            record_sha256: sha,
            caller_lane: "qompass".into(),
            anchor_sha256: Some(ANCHOR_SHA.into()),
            measurements: m,
        },
    )
    .expect("gate runs");
    match v {
        GateVerdict::Refused(Refusal::DriftDetected { failures }) => {
            assert_eq!(failures.len(), 1);
            assert_eq!(failures[0].metric, "ink_colour_distance");
            assert!(failures[0].delta.expect("delta") > 0.3);
            assert!(failures[0].excess.expect("excess") > 0.0);
        }
        other => panic!("expected drift refusal, got {other:?}"),
    }
}

#[test]
fn adv_receipt_tampering_detected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sha = sfw_store(dir.path());
    let mut job = good_job();
    job.record_sha256 = sha;
    let receipt = canon_generate(dir.path(), &job).expect("admitted");
    let path = dir
        .path()
        .join("canon-receipts")
        .join(format!("{}.json", receipt.job_id));
    let text = std::fs::read_to_string(&path).expect("read receipt");
    std::fs::write(&path, text.replace("fixture-backend", "tampered-backend")).expect("tamper");
    match verify_receipt(dir.path(), &receipt.job_id) {
        Err(Refusal::ReceiptTampered { .. }) => {}
        other => panic!("expected receipt_tampered, got {other:?}"),
    }
}

#[test]
fn adv_opal_v1_fatter_refuses_all_six_bands() {
    let record: canon_record::CanonRecord =
        serde_json::from_str(&fixture("opal-record.json")).expect("record");
    match canon_record::gate_bands(&record, &measurements("measurements-v1-fatter.json")) {
        GateVerdict::Refused(Refusal::DriftDetected { failures }) => {
            assert_eq!(failures.len(), 6, "failures: {failures:?}");
        }
        other => panic!("expected 6-band refusal, got {other:?}"),
    }
}

#[test]
fn adv_opal_v2_refuses_exactly_the_four_residuals() {
    let record: canon_record::CanonRecord =
        serde_json::from_str(&fixture("opal-record.json")).expect("record");
    match canon_record::gate_bands(&record, &measurements("measurements-v2-corrected.json")) {
        GateVerdict::Refused(Refusal::DriftDetected { failures }) => {
            assert_eq!(failures.len(), 4, "failures: {failures:?}");
        }
        other => panic!("expected 4-band refusal, got {other:?}"),
    }
}

#[test]
fn adv_drifted_output_refused_and_receipt_persisted() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sha = sfw_store(dir.path());
    let mut job = good_job();
    job.record_sha256 = sha;
    let mut m = good_measurements();
    m.measurements
        .insert("face_width_height_ratio".into(), 1.40);
    job.outputs = vec![JobOutput {
        handle: "artifact://test/out.png".into(),
        sha256: OUTPUT_SHA.into(),
        measurements: m,
    }];
    match canon_generate(dir.path(), &job) {
        Err(Refusal::DriftDetected { failures }) => {
            assert!(
                failures
                    .iter()
                    .any(|f| f.metric == "face_width_height_ratio")
            );
        }
        other => panic!("expected drift refusal, got {other:?}"),
    }
    // The refused job still left its receipt as evidence.
    let receipts = std::fs::read_dir(dir.path().join("canon-receipts")).expect("receipts dir");
    let files: Vec<_> = receipts.collect();
    assert_eq!(files.len(), 1);
    let text = std::fs::read_to_string(files[0].as_ref().expect("entry").path()).expect("read");
    assert!(text.contains("refused_drift"), "receipt status: {text}");
}

#[test]
fn adv_gate_without_anchor_lineage_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let sha = sfw_store(dir.path());
    let err = canon_gate(
        dir.path(),
        &GateRequest {
            handle: "canon://qompass/testchar@1".into(),
            record_sha256: sha,
            caller_lane: "qompass".into(),
            anchor_sha256: None,
            measurements: good_measurements(),
        },
    )
    .expect_err("unanchored artifact must refuse");
    assert_eq!(err, Refusal::UnanchoredLineage);
}

#[test]
fn adv_tampered_record_hash_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let _ = sfw_store(dir.path());
    let err = canon_resolve(dir.path(), &resolve_req(ANCHOR_SHA)).expect_err("bad pin must refuse");
    assert!(matches!(err, Refusal::RecordStale { .. }), "got {err:?}");
}
