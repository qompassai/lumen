//! Adversarial tests: per-band width/depth scales (GAP-11-4) —
//! out-of-range and non-finite components fail closed at the
//! identity layer, the parameter layer, and the profile builder;
//! unknown fields stay rejected; and an absent frame keeps the
//! canonical serialization (and with it the regression anchors).

use std::path::PathBuf;

use lumen_factory::base::{ScaledBuild, build_base_mesh_parametric};
use lumen_factory::identity::{BandScaleSet, load_record};
use lumen_factory::params::ParameterRecord;

fn asset_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join(name)
}

fn set(v: f64) -> BandScaleSet {
    BandScaleSet {
        head: v,
        leg: v,
        torso: v,
    }
}

#[test]
fn out_of_range_and_nonfinite_scales_fail_closed_everywhere() {
    for bad in [
        f64::NAN,
        f64::INFINITY,
        f64::NEG_INFINITY,
        0.49,
        2.01,
        0.0,
        -1.0,
    ] {
        // Identity layer: derivation and accessors refuse.
        let mut record =
            load_record(&asset_path("seraphine.identity.json")).expect("identity record");
        record.width_scales = Some(set(bad));
        assert!(
            ParameterRecord::from_identity(&record).is_err(),
            "identity width {bad} must fail closed at derivation"
        );
        assert!(
            record.width_scales().is_err(),
            "identity width {bad} must fail closed at the accessor"
        );
        // Parameter layer: validation refuses.
        let mut params = ParameterRecord::from_identity(
            &load_record(&asset_path("seraphine.identity.json")).expect("identity record"),
        )
        .expect("derive params");
        params.proportions.depth_scales = Some(set(bad));
        assert!(
            params.validate().is_err(),
            "params depth {bad} must not validate"
        );
    }
}

#[test]
fn boundary_scales_are_accepted() {
    for ok in [0.5f64, 2.0] {
        let mut record =
            load_record(&asset_path("seraphine.identity.json")).expect("identity record");
        record.width_scales = Some(set(ok));
        let params = ParameterRecord::from_identity(&record)
            .unwrap_or_else(|e| panic!("boundary width {ok} must derive: {e}"));
        params
            .validate()
            .unwrap_or_else(|e| panic!("boundary width {ok} must validate: {e}"));
    }
}

#[test]
fn one_bad_component_poison_the_whole_set() {
    // A single out-of-range band must refuse the set: partial
    // application would silently reshape one body region.
    let mixed = BandScaleSet {
        head: 1.0,
        leg: 2.5,
        torso: 1.0,
    };
    assert!(mixed.validate("width_scales").is_err());
    let mut record = load_record(&asset_path("seraphine.identity.json")).expect("identity record");
    record.depth_scales = Some(mixed);
    assert!(ParameterRecord::from_identity(&record).is_err());
}

#[test]
fn builder_rejects_out_of_range_band_scales() {
    for bad in [0.0f32, -1.0, f32::NAN, f32::INFINITY] {
        assert!(
            build_base_mesh_parametric(&ScaledBuild {
                band_width_scales: [1.0, bad, 1.0],
                ..ScaledBuild::default()
            })
            .is_err(),
            "builder width {bad} must fail closed"
        );
        assert!(
            build_base_mesh_parametric(&ScaledBuild {
                band_depth_scales: [bad, 1.0, 1.0],
                ..ScaledBuild::default()
            })
            .is_err(),
            "builder depth {bad} must fail closed"
        );
    }
}

#[test]
fn absent_frame_keeps_canonical_serializations() {
    let record = load_record(&asset_path("seraphine.identity.json")).expect("identity record");
    let params = ParameterRecord::from_identity(&record).expect("derive params");
    let json = params.canonical_json().expect("canonical json");
    assert!(
        !json.contains("width_scales") && !json.contains("depth_scales"),
        "absent frame fields must not appear in canonical JSON: {json}"
    );
    assert_eq!(params.width_scales(), (1.0, 1.0, 1.0));
    assert_eq!(params.depth_scales(), (1.0, 1.0, 1.0));
    assert_eq!(
        record.width_scales().expect("identity widths"),
        (1.0, 1.0, 1.0)
    );
    assert_eq!(
        record.depth_scales().expect("identity depths"),
        (1.0, 1.0, 1.0)
    );
}

#[test]
fn misspelled_frame_field_is_still_rejected() {
    let record = load_record(&asset_path("seraphine.identity.json")).expect("identity record");
    let params = ParameterRecord::from_identity(&record).expect("derive params");
    let json = params.canonical_json().expect("canonical json");
    let poisoned = json.replacen(
        '{',
        "{\"width_scalez\":{\"head\":1.0,\"leg\":1.0,\"torso\":1.0},",
        1,
    );
    assert!(
        ParameterRecord::from_json(&poisoned).is_err(),
        "deny_unknown_fields must reject the misspelled field"
    );
}
