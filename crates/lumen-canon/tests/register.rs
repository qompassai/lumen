//! Register-band tests (D-312): 4 validation, 4 adversarial.
//! The B1 suites (validation.rs / adversarial.rs) are unmodified;
//! this file covers only the register class added at B2.

use canon_record::{gate_all, gate_register, validate_record, GateVerdict, Measurements};
use std::collections::BTreeMap;

fn record_with_register() -> canon_record::CanonRecord {
    let raw = std::fs::read_to_string(format!(
        "{}/fixtures/opal-record.json",
        env!("CARGO_MANIFEST_DIR")
    ))
    .expect("fixture");
    let mut v: serde_json::Value = serde_json::from_str(&raw).expect("value");
    v["style_fingerprint"]["register_bands"] = serde_json::json!([
        {"metric":"ink_colour_distance","target":0.02,"tolerance":0.03,"method":"ink cluster distance"},
        {"metric":"stroke_weight_ratio","target":1.0,"tolerance":0.15,"method":"stroke width median vs anchor"},
        {"metric":"shadow_edge_hardness","target":0.9,"tolerance":0.1,"method":"shadow boundary gradient"},
        {"metric":"saturation_percentile_95","target":0.62,"tolerance":0.08,"method":"hsv percentile"},
        {"metric":"key_light_direction_deg","target":45.0,"tolerance":20.0,"method":"highlight centroid"},
        {"metric":"iris_detail_score","target":0.8,"tolerance":0.15,"method":"iris window spec read"},
        {"metric":"blush_coverage","target":0.12,"tolerance":0.05,"method":"blush region share"}
    ]);
    serde_json::from_value(v).expect("record with register bands parses")
}

fn good_register() -> Measurements {
    let mut m = BTreeMap::new();
    m.insert("ink_colour_distance".into(), 0.03);
    m.insert("stroke_weight_ratio".into(), 1.05);
    m.insert("shadow_edge_hardness".into(), 0.88);
    m.insert("saturation_percentile_95".into(), 0.60);
    m.insert("key_light_direction_deg".into(), 50.0);
    m.insert("iris_detail_score".into(), 0.82);
    m.insert("blush_coverage".into(), 0.13);
    Measurements {
        measurements: m,
        source: None,
    }
}

fn ardent() -> Measurements {
    serde_json::from_str(
        &std::fs::read_to_string(format!(
            "{}/fixtures/measurements-ardent-r2.json",
            env!("CARGO_MANIFEST_DIR")
        ))
        .expect("fixture"),
    )
    .expect("measurements")
}

// Validation

#[test]
fn register_record_validates_clean() {
    assert!(validate_record(&record_with_register()).is_empty());
}

#[test]
fn register_gate_passes_on_register_measurements() {
    assert_eq!(
        gate_register(&record_with_register(), &good_register()),
        GateVerdict::Pass
    );
}

#[test]
fn gate_all_passes_when_proportion_and_register_pass() {
    let r = record_with_register();
    let mut m = ardent();
    m.measurements.extend(good_register().measurements);
    assert_eq!(gate_all(&r, &m), GateVerdict::Pass);
}

#[test]
fn register_floor_band_binds_only_from_below() {
    let raw = std::fs::read_to_string(format!(
        "{}/fixtures/opal-record.json",
        env!("CARGO_MANIFEST_DIR")
    ))
    .expect("fixture");
    let mut v: serde_json::Value = serde_json::from_str(&raw).expect("value");
    v["style_fingerprint"]["register_bands"] = serde_json::json!([
        {"metric":"shadow_edge_hardness","target":0.9,"tolerance":0.1,"method":"m","floor_only":true}
    ]);
    let r: canon_record::CanonRecord = serde_json::from_value(v).expect("record");
    let mut mm = BTreeMap::new();
    mm.insert("shadow_edge_hardness".into(), 0.99);
    assert_eq!(
        gate_register(
            &r,
            &Measurements {
                measurements: mm,
                source: None
            }
        ),
        GateVerdict::Pass
    );
}

// Adversarial

#[test]
fn register_drift_refused_with_deltas() {
    let r = record_with_register();
    let mut m = good_register();
    m.measurements.insert("ink_colour_distance".into(), 0.40);
    match gate_register(&r, &m) {
        GateVerdict::Refused(canon_record::Refusal::DriftDetected { failures }) => {
            assert_eq!(failures.len(), 1);
            assert_eq!(failures[0].metric, "ink_colour_distance");
            assert!(failures[0].excess.expect("excess") > 0.0);
        }
        other => panic!("expected drift refusal, got {other:?}"),
    }
}

#[test]
fn missing_register_measurement_fails() {
    let r = record_with_register();
    let m = Measurements::default();
    match gate_register(&r, &m) {
        GateVerdict::Refused(canon_record::Refusal::DriftDetected { failures }) => {
            assert_eq!(failures.len(), 7);
        }
        other => panic!("expected drift refusal, got {other:?}"),
    }
}

#[test]
fn duplicate_register_metric_invalid() {
    let mut r = record_with_register();
    let dup = r.style_fingerprint.register_bands[0].clone();
    r.style_fingerprint.register_bands.push(dup);
    assert!(!validate_record(&r).is_empty());
}

#[test]
fn gate_all_merges_proportion_and_register_failures_sorted() {
    let r = record_with_register();
    // ardent proportions pass; poison one register metric only.
    let mut m = ardent();
    m.measurements.extend(good_register().measurements);
    m.measurements.insert("blush_coverage".into(), 0.90);
    match gate_all(&r, &m) {
        GateVerdict::Refused(canon_record::Refusal::DriftDetected { failures }) => {
            assert_eq!(failures.len(), 1);
            assert_eq!(failures[0].metric, "blush_coverage");
        }
        other => panic!("expected merged refusal, got {other:?}"),
    }
}
