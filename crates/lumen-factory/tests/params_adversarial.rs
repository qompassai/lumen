//! Adversarial tests: parameter records fail closed — unknown keys,
//! out-of-range scales, malformed colors, NaN, hash tampering, and
//! companion mismatches are all typed errors, never clamps.

use std::path::PathBuf;

use lumen_factory::FactoryError;
use lumen_factory::blend::load_templates;
use lumen_factory::identity::load_record;
use lumen_factory::params::ParameterRecord;
use lumen_factory::project::{BuildOptions, build_character_with_params};

fn seraphine_params() -> ParameterRecord {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join("seraphine.identity.json");
    let record = load_record(&path).expect("identity record loads");
    ParameterRecord::from_identity(&record).expect("derive params")
}

#[test]
fn unknown_json_key_is_rejected() {
    let params = seraphine_params();
    let mut value: serde_json::Value =
        serde_json::from_str(&params.canonical_json().expect("json")).expect("value");
    value
        .as_object_mut()
        .expect("object")
        .insert("bogus_key".to_string(), serde_json::json!(1));
    let text = serde_json::to_string(&value).expect("render");
    let err = ParameterRecord::from_json(&text).expect_err("unknown key must fail");
    assert!(matches!(err, FactoryError::BadParam(_)), "got {err:?}");
}

#[test]
fn unknown_toml_key_is_rejected() {
    let params = seraphine_params();
    let mut text = params.to_toml().expect("toml");
    text.push_str("\nbogus_key = 1\n");
    assert!(
        ParameterRecord::from_toml(&text).is_err(),
        "unknown TOML key must fail"
    );
}

#[test]
fn scale_above_max_is_rejected_not_clamped() {
    let mut params = seraphine_params();
    params.proportions.head_scale = 2.5;
    let err = params.validate().expect_err("scale 2.5 must fail");
    assert!(matches!(err, FactoryError::BadParam(_)), "got {err:?}");
    params.proportions.head_scale = 0.49;
    assert!(params.validate().is_err(), "scale 0.49 must fail");
    // The JSON path rejects it too (validation precedes hash check).
    let text = params.canonical_json().expect("json");
    assert!(ParameterRecord::from_json(&text).is_err());
}

#[test]
fn bad_hex_colors_are_rejected() {
    for bad in [
        "red", "#12345", "#1234567", "123456", "#gg0044", "#12 456", "",
    ] {
        let mut params = seraphine_params();
        params.appearance.hair = Some(bad.to_string());
        assert!(
            params.validate().is_err(),
            "hair color {bad:?} must be rejected"
        );
    }
    // Well-formed colors in either case are accepted.
    let mut params = seraphine_params();
    params.appearance.hair = Some("#A0b1C2".to_string());
    assert!(params.validate().is_ok(), "mixed-case hex is well-formed");
}

#[test]
fn nan_and_infinite_values_are_rejected() {
    let mut params = seraphine_params();
    params.proportions.torso_scale = f64::NAN;
    assert!(params.validate().is_err(), "NaN scale must fail");
    params.proportions.torso_scale = f64::INFINITY;
    assert!(params.validate().is_err(), "infinite scale must fail");
    params.proportions.torso_scale = 1.0;
    params.hair.length = f64::NAN;
    assert!(params.validate().is_err(), "NaN hair length must fail");
    // A raw NaN literal is not valid JSON, so the parse itself fails.
    let text = "{\"companion\":\"x\",\"face\":{\"eye_scale\":NaN}}";
    assert!(ParameterRecord::from_json(text).is_err());
}

#[test]
fn hair_and_eye_ranges_are_enforced() {
    let mut params = seraphine_params();
    params.hair.length = -0.1;
    assert!(params.validate().is_err(), "hair length -0.1 must fail");
    params.hair.length = 1.5;
    assert!(params.validate().is_err(), "hair length 1.5 must fail");
    params.hair.length = 0.65;
    params.face.eye_scale = 0.79;
    assert!(params.validate().is_err(), "eye scale 0.79 must fail");
    params.face.eye_scale = 1.31;
    assert!(params.validate().is_err(), "eye scale 1.31 must fail");
    params.face.eye_scale = 0.8;
    assert!(params.validate().is_ok(), "eye scale 0.8 is the bound");
    params.face.eye_scale = 1.3;
    assert!(params.validate().is_ok(), "eye scale 1.3 is the bound");
}

#[test]
fn wrong_schema_and_tampered_hash_are_rejected() {
    let mut params = seraphine_params();
    params.schema = "lumen.something-else".to_string();
    assert!(params.validate().is_err(), "wrong schema must fail");
    params.schema = "lumen.factory-params".to_string();
    params.version = 99;
    assert!(params.validate().is_err(), "wrong version must fail");

    // Tamper with contents but keep the original hash: the JSON load
    // must reject on hash mismatch even though validation passes.
    let mut tampered = seraphine_params();
    tampered.appearance.skin = Some("#010203".to_string());
    let text = tampered.canonical_json().expect("json");
    let err = ParameterRecord::from_json(&text).expect_err("tampered record must fail");
    assert!(matches!(err, FactoryError::BadParam(_)), "got {err:?}");
}

#[test]
fn companion_mismatch_fails_the_build() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join("seraphine.identity.json");
    let record = load_record(&path).expect("identity record loads");
    let templates_path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/emotions_fixture.json");
    let templates = load_templates(&templates_path).expect("templates");
    let mut params = ParameterRecord::from_identity(&record).expect("derive");
    params.companion = "someone-else".to_string();
    let err = build_character_with_params(&record, &templates, &params, BuildOptions::default())
        .expect_err("companion mismatch must fail");
    assert!(matches!(err, FactoryError::BadParam(_)), "got {err:?}");
}

#[test]
fn load_rejects_unknown_extension() {
    let dir = std::env::temp_dir().join(format!("lumen-factory-params-adv-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let path = dir.join("record.params.yaml");
    std::fs::write(&path, "companion: x\n").expect("write");
    assert!(
        ParameterRecord::load(&path).is_err(),
        "unknown extension must fail"
    );
    let _ = std::fs::remove_dir_all(&dir);
}
