//! Adversarial tests: the bust delta parameter — out-of-range and
//! non-finite rejection at both layers, serialization invariance
//! for records that predate the parameter, unknown-field
//! discipline, and proof that shaping touches only the torso it
//! is supposed to.
//!
//! Ported from the factory Stage-3 line (bust-param branch) with
//! the port onto the landed factory, plus identity-layer cases.

use std::path::PathBuf;

use lumen_factory::base::{
    ScaledBuild, apply_bust_delta, build_base_mesh_parametric, default_body_profile,
};
use lumen_factory::identity::load_record;
use lumen_factory::params::ParameterRecord;

fn asset_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join(name)
}

fn seraphine_params() -> ParameterRecord {
    let record = load_record(&asset_path("seraphine.identity.json")).expect("identity record");
    ParameterRecord::from_identity(&record).expect("derive params")
}

#[test]
fn shaping_rejects_out_of_range_and_nonfinite_deltas() {
    let profile = default_body_profile();
    for bad in [
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
        0.0,
        4.99,
        35.01,
        -10.0,
    ] {
        assert!(
            apply_bust_delta(&profile, bad).is_err(),
            "delta {bad} must fail closed"
        );
    }
    for ok in [5.0f32, 35.0] {
        assert!(
            apply_bust_delta(&profile, ok).is_ok(),
            "boundary delta {ok} must be accepted"
        );
    }
}

#[test]
fn record_validation_rejects_bad_deltas() {
    for bad in [f64::NAN, f64::INFINITY, 4.99, 35.01, -1.0] {
        let mut params = seraphine_params();
        params.proportions.bust_delta_cm = Some(bad);
        assert!(params.validate().is_err(), "delta {bad} must not validate");
    }
    let mut params = seraphine_params();
    params.proportions.bust_delta_cm = Some(35.0);
    assert!(params.validate().is_ok(), "boundary delta must validate");
}

#[test]
fn identity_derivation_rejects_bad_deltas() {
    for bad in [f64::NAN, f64::INFINITY, 4.99, 35.01, -1.0] {
        let mut record =
            load_record(&asset_path("seraphine.identity.json")).expect("identity record");
        record.bust_delta_cm = Some(bad);
        assert!(
            ParameterRecord::from_identity(&record).is_err(),
            "identity delta {bad} must fail closed at derivation"
        );
        assert!(
            record.bust_delta_cm().is_err(),
            "identity delta {bad} must fail closed at the accessor"
        );
    }
}

#[test]
fn absent_field_keeps_canonical_serialization() {
    let params = seraphine_params();
    let json = params.canonical_json().expect("canonical json");
    assert!(
        !json.contains("bust_delta_cm"),
        "absent parameter must not appear in canonical JSON: {json}"
    );
    let parsed = ParameterRecord::from_json(&json).expect("round trip parses");
    assert_eq!(parsed.proportions.bust_delta_cm, None);
    // The canonical form is fixed-precision by design, so losslessness
    // is judged on the canonical bytes and the hash, not on f64 bits.
    assert_eq!(
        parsed.canonical_json().expect("parsed canonical json"),
        json,
        "canonical round trip must be byte-stable"
    );
    assert!(params.verify_hash().expect("hash verifies"));
}

#[test]
fn absent_identity_fields_keep_the_shipped_record_bytes() {
    // The shipped Séraphine identity record predates the authored
    // frame fields; loading and re-deriving must not change its
    // canonical bytes or its hash (the regression anchor's base).
    let record = load_record(&asset_path("seraphine.identity.json")).expect("identity record");
    let json = record.canonical_json().expect("canonical json");
    assert!(
        !json.contains("bust_delta_cm"),
        "absent bust field must not appear: {json}"
    );
    assert!(
        !json.contains("width_scales") && !json.contains("depth_scales"),
        "absent frame fields must not appear: {json}"
    );
    assert!(record.verify_hash().expect("hash verifies"));
    let on_disk =
        std::fs::read_to_string(asset_path("seraphine.identity.json")).expect("record file reads");
    let parsed: serde_json::Value = serde_json::from_str(&on_disk).expect("record json parses");
    assert_eq!(
        parsed["record_hash"].as_str().expect("hash field"),
        record.record_hash,
        "the shipped record's hash is unchanged by the new fields"
    );
}

#[test]
fn misspelled_field_is_still_rejected() {
    let params = seraphine_params();
    let json = params.canonical_json().expect("canonical json");
    let poisoned = json.replacen('{', "{\"bust_delta\":12.5,", 1);
    assert!(
        ParameterRecord::from_json(&poisoned).is_err(),
        "deny_unknown_fields must reject the misspelled field"
    );
}

#[test]
fn shaping_touches_only_the_torso() {
    // On the landed factory the invariant is narrower than on the
    // Stage-3 line (where the hair ignored the profile): the hair
    // tail's drop line clears the profile's deepest ring, so a bust
    // delta translates the tail rigidly in Z — and nothing else
    // outside the torso may move at all.
    let plain = build_base_mesh_parametric(&ScaledBuild::default()).expect("plain build");
    let shaped = build_base_mesh_parametric(&ScaledBuild {
        bust_delta_cm: Some(25.0),
        ..ScaledBuild::default()
    })
    .expect("shaped build");
    for name in ["head", "arm_left", "arm_right", "leg_left", "leg_right"] {
        let a = plain.parts.iter().find(|p| p.name == name).expect("part");
        let b = shaped.parts.iter().find(|p| p.name == name).expect("part");
        assert_eq!(a.vertices.len(), b.vertices.len(), "part {name}");
        for (va, vb) in a.vertices.iter().zip(b.vertices.iter()) {
            assert_eq!(va.position, vb.position, "part {name} must not move");
        }
    }
    // The hair: the head shell never moves. The tail's drop rings
    // translate rigidly in Z by one constant (the drop line
    // backing off the deeper bust ring); the tail's root ring,
    // whose tube frame tilts with the drop line's new direction,
    // moves by at most 2 mm and never in X. (Measured on this
    // build: drop rings dz = -0.0088 exactly, root ring |d| <=
    // 0.0016, shell 0.)
    let hair_a = plain.parts.iter().find(|p| p.name == "hair").expect("hair");
    let hair_b = shaped
        .parts
        .iter()
        .find(|p| p.name == "hair")
        .expect("hair");
    assert_eq!(hair_a.vertices.len(), hair_b.vertices.len());
    let dz_full = hair_a
        .vertices
        .iter()
        .zip(hair_b.vertices.iter())
        .map(|(va, vb)| vb.position[2] - va.position[2])
        .fold(0.0f32, f32::min);
    assert!(
        dz_full < -0.005,
        "the tail must back off the deeper ring: dz {dz_full}"
    );
    for (va, vb) in hair_a.vertices.iter().zip(hair_b.vertices.iter()) {
        let (dx, dy, dz) = (
            vb.position[0] - va.position[0],
            vb.position[1] - va.position[1],
            vb.position[2] - va.position[2],
        );
        assert_eq!(dx, 0.0, "hair X must never move");
        assert!(
            dy.abs() <= 0.002,
            "hair Y movement is root-ring tilt only: {dy}"
        );
        assert!(
            dz <= 0.002 && dz >= dz_full - 1e-6,
            "hair Z stays inside the tilt/back-off envelope: {dz}"
        );
        if va.position[1] <= lumen_factory::base::HEAD_CENTER[1] + 0.001 {
            assert_eq!(dy, 0.0, "drop-ring Y must not move");
            assert!(
                (dz - dz_full).abs() < 1e-5 || dz == 0.0,
                "drop rings translate rigidly: dz {dz} vs {dz_full}"
            );
        }
    }
    let body_a = plain.parts.iter().find(|p| p.name == "body").expect("body");
    let body_b = shaped
        .parts
        .iter()
        .find(|p| p.name == "body")
        .expect("body");
    assert!(
        body_a
            .vertices
            .iter()
            .zip(body_b.vertices.iter())
            .any(|(va, vb)| va.position != vb.position),
        "the body must actually change"
    );
}

#[test]
fn degenerate_profiles_fail_closed() {
    assert!(
        apply_bust_delta(&[(0.9, 0.2, 0.2)], 15.0).is_err(),
        "one ring"
    );
    assert!(
        apply_bust_delta(&[(0.9, 0.2, 0.2), (1.0, 0.1, 0.1)], 15.0).is_err(),
        "widest ring first leaves no underbust ring"
    );
    assert!(
        apply_bust_delta(&[(0.9, 0.0, 0.2), (1.0, 0.1, 0.1), (1.1, 0.2, 0.2)], 15.0).is_err(),
        "non-positive radius"
    );
}
