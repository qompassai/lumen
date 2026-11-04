//! Gate tests for the contact-pair probe spec
//! (ActLab Tool 2, D-943): grammar conformance,
//! validator-semantics legality, aggregation, and
//! report determinism at the data level. The live
//! calibration against the real rig records is the
//! bevy host's `probe_calibration` gate; everything
//! here is hermetic.

use lumen_engine_core::probe::{
    band_box, box_distance, PairAccumulator, ProbePair, ProbeSpec, RigParameterTable,
};

const CALIBRATION_SPEC: &str = include_str!("../../../scenes/probe-lift-calibration.probe.ron");

/// Parameter tables shaped like the real rigs' (names
/// and ranges from the records' own parameter data).
fn tables() -> Vec<RigParameterTable> {
    let articulation = [
        ("ParamArmL", -1.0, 1.0),
        ("ParamArmR", -1.0, 1.0),
        ("ParamElbowL", 0.0, 1.0),
        ("ParamElbowR", 0.0, 1.0),
        ("ParamCrouch", 0.0, 1.0),
        ("ParamKneeL", 0.0, 1.0),
        ("ParamKneeR", 0.0, 1.0),
    ];
    ["prima", "alma"]
        .iter()
        .map(|id| RigParameterTable {
            rig_id: (*id).to_string(),
            parameters: articulation
                .iter()
                .map(|(n, lo, hi)| ((*n).to_string(), *lo, *hi))
                .collect(),
        })
        .collect()
}

/* ------------------------- validation ------------------------- */

#[test]
fn calibration_spec_parses_and_validates() {
    let spec = ProbeSpec::load_ron(CALIBRATION_SPEC).expect("calibration spec parses");
    assert_eq!(spec.id, "lift-calibration");
    assert_eq!(spec.steps, 720);
    assert_eq!(spec.rigs.len(), 2);
    assert_eq!(spec.pairs.len(), 2);
    spec.validate_timeline(&tables())
        .expect("calibration timeline is legal against the rig tables");
}

#[test]
fn band_box_matches_the_d859_convention() {
    // AABB top at y = 100, height 50: the band is
    // [x0, 100 - 0.78*50, x1, 100 - 0.62*50].
    let band = band_box(([-10.0, 50.0], [30.0, 100.0]), 0.62, 0.78);
    assert_eq!(band, [-10.0, 61.0, 30.0, 69.0]);
    assert_eq!(
        box_distance([0.0, 65.0], [band[0], band[1]], [band[2], band[3]]),
        0.0
    );
    assert_eq!(
        box_distance([0.0, 59.0], [band[0], band[1]], [band[2], band[3]]),
        2.0
    );
}

#[test]
fn accumulator_reports_planted_series_exactly() {
    let spec = ProbeSpec::load_ron(CALIBRATION_SPEC).expect("spec parses");
    let pair = &spec.pairs[0];
    let mut acc = PairAccumulator::new(pair, spec.rate, Some([0.0, 0.0, 1.0, 1.0]));
    // Steps 0..720 at 60/s; the pair engages 2..10 s
    // (steps 120..600). Plant distance = 10.0 exactly
    // at every step.
    for step in 0..720u64 {
        acc.push(step, [0.0, 0.0], Some(10.0), Some(0.0), Some(-1.5));
    }
    let report = acc.finish(true);
    assert_eq!(report.engaged_steps, 480);
    let distance = report.distance.expect("distance aggregate");
    assert_eq!(distance.max, 10.0);
    assert_eq!(distance.mean, 10.0);
    let band = report.band_distance.expect("band aggregate");
    assert_eq!(band.max, 0.0);
    let penetration = report.penetration.expect("penetration aggregate");
    assert_eq!(penetration.min, -1.5);
    assert!(report.has_band_distance && report.has_penetration);
}

#[test]
fn report_serialization_is_deterministic() {
    let spec = ProbeSpec::load_ron(CALIBRATION_SPEC).expect("spec parses");
    let build = || {
        let mut acc = PairAccumulator::new(&spec.pairs[0], spec.rate, None);
        for step in 0..720u64 {
            acc.push(step, [step as f64, 1.0], Some(0.25), None, None);
        }
        acc.finish(true)
    };
    let first = ron::ser::to_string(&build()).expect("serializes");
    let second = ron::ser::to_string(&build()).expect("serializes");
    assert_eq!(first, second);
}

#[test]
fn boundary_values_at_declared_range_edges_are_legal() {
    let mut spec = ProbeSpec::load_ron(CALIBRATION_SPEC).expect("spec parses");
    // ParamArmL spans [-1, 1] and ParamCrouch [0, 1] in
    // the tables: the exact edges must validate.
    spec.rigs[0].pose_track[1]
        .params
        .insert("ParamArmL".to_string(), -1.0);
    spec.rigs[0].pose_track[2]
        .params
        .insert("ParamCrouch".to_string(), 1.0);
    spec.validate_timeline(&tables())
        .expect("edge values are legal");
}

#[test]
fn accumulator_aggregates_varying_series_exactly() {
    let spec = ProbeSpec::load_ron(CALIBRATION_SPEC).expect("spec parses");
    let pair = &spec.pairs[0];
    let mut acc = PairAccumulator::new(pair, spec.rate, None);
    for step in 0..720u64 {
        // Engaged steps are 120..600: distance ramps
        // 0, 1, 2, ..., 479 there; 99.0 outside (must
        // not leak into the aggregates).
        let distance = if (120..600).contains(&step) {
            (step - 120) as f64
        } else {
            99.0
        };
        acc.push(step, [0.0, 0.0], Some(distance), None, None);
    }
    let report = acc.finish(true);
    let distance = report.distance.expect("distance aggregate");
    assert_eq!(distance.max, 479.0);
    assert_eq!(distance.min, 0.0);
    assert_eq!(distance.mean, 239.5);
    assert_eq!(report.engaged_steps, 480);
    assert!(report.band_distance.is_none());
    assert!(report.penetration.is_none());
    // The pair declares band + penetration, so the
    // only capability note is the universal one: no
    // mesh-overlap volume for point anchors.
    assert_eq!(report.capability_notes.len(), 1);
    assert!(report.capability_notes[0].contains("mesh-overlap volume"));
}

#[test]
fn sha256_form_accepts_the_real_digests() {
    assert!(lumen_engine_core::probe::is_sha256_hex(
        "ce0f95de7c1ea300b6af1051c396482b7c1c38fdb496c6ec67c3cb650504c228"
    ));
    assert!(!lumen_engine_core::probe::is_sha256_hex("ce0f95de"));
    assert!(!lumen_engine_core::probe::is_sha256_hex(
        "ge0f95de7c1ea300b6af1051c396482b7c1c38fdb496c6ec67c3cb650504c228"
    ));
}

/* ------------------------- adversarial ------------------------ */

#[test]
fn unknown_field_is_refused() {
    let bad = CALIBRATION_SPEC.replacen(
        "id: \"lift-calibration\"",
        "id: \"lift-calibration\", bogus: 1",
        1,
    );
    let err = ProbeSpec::load_ron(&bad).expect_err("unknown field refuses");
    assert_eq!(err.code(), "LE-110");
}

#[test]
fn bad_hash_is_refused() {
    let bad = CALIBRATION_SPEC.replacen(
        "ce0f95de7c1ea300b6af1051c396482b7c1c38fdb496c6ec67c3cb650504c228",
        "CE0F95DE7C1EA300B6AF1051C396482B7C1C38FDB496C6EC67C3CB650504C228",
        1,
    );
    let err = ProbeSpec::load_ron(&bad).expect_err("uppercase hash refuses");
    assert_eq!(err.code(), "LE-110");
    assert!(err.to_string().contains("sha256"), "{err}");
}

#[test]
fn unknown_timeline_parameter_is_refused_by_name() {
    let mut spec = ProbeSpec::load_ron(CALIBRATION_SPEC).expect("spec parses");
    spec.rigs[0].pose_track[1]
        .params
        .insert("ParamNope".to_string(), 0.5);
    let err = spec
        .validate_timeline(&tables())
        .expect_err("unknown parameter refuses");
    assert_eq!(err.code(), "LE-110");
    assert!(err.to_string().contains("ParamNope"), "{err}");
}

#[test]
fn out_of_range_timeline_value_is_refused() {
    let mut spec = ProbeSpec::load_ron(CALIBRATION_SPEC).expect("spec parses");
    spec.rigs[0].pose_track[1]
        .params
        .insert("ParamArmL".to_string(), 1.5);
    let err = spec
        .validate_timeline(&tables())
        .expect_err("out-of-range value refuses");
    assert_eq!(err.code(), "LE-110");
    let text = err.to_string();
    assert!(
        text.contains("ParamArmL") && text.contains("outside"),
        "{text}"
    );
}

#[test]
fn duplicate_pair_id_is_refused() {
    let mut spec = ProbeSpec::load_ron(CALIBRATION_SPEC).expect("spec parses");
    spec.pairs[1].id = spec.pairs[0].id.clone();
    let err = spec.validate_shape().expect_err("duplicate pair refuses");
    assert_eq!(err.code(), "LE-110");
}

#[test]
fn unsorted_pose_keys_are_refused() {
    let mut spec = ProbeSpec::load_ron(CALIBRATION_SPEC).expect("spec parses");
    spec.rigs[0].pose_track.swap(1, 2);
    let err = spec.validate_shape().expect_err("unsorted keys refuse");
    assert_eq!(err.code(), "LE-110");
}

#[test]
fn pair_bound_to_undeclared_contact_is_refused() {
    let mut spec = ProbeSpec::load_ron(CALIBRATION_SPEC).expect("spec parses");
    spec.pairs[0].contact = Some("c_nope".to_string());
    let err = spec.validate_shape().expect_err("dangling contact refuses");
    assert_eq!(err.code(), "LE-110");
}

#[test]
fn wrong_version_is_refused() {
    let bad = CALIBRATION_SPEC.replacen("version: 1", "version: 2", 1);
    let err = ProbeSpec::load_ron(&bad).expect_err("version refuses");
    assert_eq!(err.code(), "LE-102");
}

// Keep the pair type reachable for fixture construction
// in future gates.
#[allow(dead_code)]
fn _pair_type_anchor(pair: &ProbePair) -> &str {
    &pair.id
}
