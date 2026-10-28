//! Motion adversarial tests: every loader-side refusal class
//! in the R2 handoff — LRF-202/203/303/501/502/602 — plus the
//! shared header, schema, and count law, proven by name. Each
//! test mutates exactly one thing from the valid full motion.

use std::fs;
use std::path::Path;

use lumen_rig_core::{Model, ParseError, parse_model, parse_motion};
use serde_json::{Value, json};

fn fixture(relative: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("fixtures")
        .join(relative);
    fs::read(&path).unwrap_or_else(|error| panic!("fixture {relative}: {error}"))
}

fn full_model() -> Model {
    parse_model(&fixture("full.model.json"))
        .unwrap_or_else(|error| panic!("full model should parse: {error}"))
}

fn valid_doc() -> Value {
    serde_json::from_slice(&fixture("full.motion.json")).expect("fixture is valid JSON")
}

fn error_code(doc: &Value) -> &'static str {
    let bytes = serde_json::to_vec(doc).expect("serialize mutated doc");
    match parse_motion(&bytes, &full_model()) {
        Ok(_) => panic!("mutated doc should be refused"),
        Err(error) => error.code(),
    }
}

fn expect_code(doc: &Value, code: &str) {
    assert_eq!(error_code(doc), code, "doc: {doc}");
}

#[test]
fn motion_for_another_model_is_refused_lrf_203() {
    let mut doc = valid_doc();
    doc["model"] = json!("minimal");
    expect_code(&doc, "LRF-203");
}

#[test]
fn curve_for_a_missing_parameter_is_refused_lrf_203() {
    let mut doc = valid_doc();
    doc["curves"][0]["parameter"] = json!("missing");
    expect_code(&doc, "LRF-203");
}

#[test]
fn curve_on_a_physics_output_is_refused_lrf_602() {
    // `sway` is the full model's physics strand output.
    let mut doc = valid_doc();
    doc["curves"][0]["parameter"] = json!("sway");
    expect_code(&doc, "LRF-602");
}

#[test]
fn duplicate_curve_parameter_is_refused_lrf_202() {
    let mut doc = valid_doc();
    let second = doc["curves"][0].clone();
    doc["curves"]
        .as_array_mut()
        .expect("curves array")
        .push(second);
    expect_code(&doc, "LRF-202");
}

#[test]
fn baked_set_duplicating_a_curve_is_refused_lrf_202() {
    let mut doc = valid_doc();
    doc["audio"]["baked"][0]["parameter"] = json!("angle_x");
    expect_code(&doc, "LRF-202");
}

#[test]
fn keyframe_value_out_of_range_is_refused_lrf_303() {
    let mut doc = valid_doc();
    doc["curves"][0]["keyframes"][1]["value"] = json!(31.0);
    expect_code(&doc, "LRF-303");
}

#[test]
fn negative_fade_is_refused_lrf_303() {
    let mut doc = valid_doc();
    doc["curves"][0]["fade_in_seconds"] = json!(-0.5);
    expect_code(&doc, "LRF-303");
}

#[test]
fn non_increasing_keyframe_times_are_refused_lrf_501() {
    let mut doc = valid_doc();
    doc["curves"][1]["keyframes"][1]["time"] = json!(0.0);
    expect_code(&doc, "LRF-501");
}

#[test]
fn keyframe_past_duration_is_refused_lrf_501() {
    let mut doc = valid_doc();
    doc["curves"][1]["keyframes"][2]["time"] = json!(2.5);
    expect_code(&doc, "LRF-501");
}

#[test]
fn tangent_on_a_linear_key_is_refused_lrf_502() {
    let mut doc = valid_doc();
    doc["curves"][1]["keyframes"][0]["tangent_out"] = json!([0.1, 0.0]);
    expect_code(&doc, "LRF-502");
}

#[test]
fn tangent_reaching_past_the_next_key_is_refused_lrf_502() {
    let mut doc = valid_doc();
    doc["curves"][0]["keyframes"][0]["tangent_out"] = json!([1.5, 5.0]);
    expect_code(&doc, "LRF-502");
}

#[test]
fn negative_tangent_dt_is_refused_lrf_502() {
    let mut doc = valid_doc();
    doc["curves"][0]["keyframes"][1]["tangent_in"] = json!([-0.25, -5.0]);
    expect_code(&doc, "LRF-502");
}

#[test]
fn tangent_in_on_the_first_key_is_refused_lrf_502() {
    // No segment precedes the first key, so nothing can read
    // its tangent_in (section 4.2 reads it per segment).
    let mut doc = valid_doc();
    doc["curves"][0]["keyframes"][0]["tangent_in"] = json!([0.1, 0.0]);
    expect_code(&doc, "LRF-502");
}

#[test]
fn tangent_out_on_the_last_key_is_refused_lrf_502() {
    let mut doc = valid_doc();
    doc["curves"][0]["keyframes"][2]["tangent_out"] = json!([0.1, 0.0]);
    expect_code(&doc, "LRF-502");
}

#[test]
fn unknown_interpolation_is_refused_lrf_103() {
    let mut doc = valid_doc();
    doc["curves"][1]["keyframes"][0]["interpolation"] = json!("smooth");
    expect_code(&doc, "LRF-103");
}

#[test]
fn unknown_top_level_field_is_refused_lrf_104() {
    let mut doc = valid_doc();
    doc["surprise"] = json!(true);
    expect_code(&doc, "LRF-104");
}

#[test]
fn model_document_is_refused_lrf_105() {
    let mut doc = valid_doc();
    doc["format"] = json!("lumen-rig-model");
    expect_code(&doc, "LRF-105");
}

#[test]
fn version_two_is_refused_lrf_106() {
    let mut doc = valid_doc();
    doc["format_version"] = json!(2);
    expect_code(&doc, "LRF-106");
}

#[test]
fn version_error_precedes_unknown_field() {
    let mut doc = valid_doc();
    doc["format_version"] = json!(2);
    doc["surprise"] = json!(true);
    expect_code(&doc, "LRF-106");
}

#[test]
fn zero_duration_is_refused_lrf_305() {
    let mut doc = valid_doc();
    doc["duration_seconds"] = json!(0.0);
    expect_code(&doc, "LRF-305");
}

#[test]
fn frames_per_second_out_of_range_is_refused_lrf_305() {
    let mut doc = valid_doc();
    doc["frames_per_second"] = json!(0);
    expect_code(&doc, "LRF-305");
    let mut doc = valid_doc();
    doc["frames_per_second"] = json!(241);
    expect_code(&doc, "LRF-305");
}

#[test]
fn malformed_source_hash_is_refused_lrf_103() {
    let mut doc = valid_doc();
    doc["audio"]["baked"][0]["provenance"]["source_audio_sha256"] = json!("ABCD");
    expect_code(&doc, "LRF-103");
}

#[test]
fn manual_provenance_with_analysis_fields_is_refused_lrf_103() {
    let mut doc = valid_doc();
    doc["audio"]["baked"][0]["provenance"]["kind"] = json!("manual");
    expect_code(&doc, "LRF-103");
}

#[test]
fn analyzed_provenance_missing_hop_is_refused_lrf_103() {
    let mut doc = valid_doc();
    doc["audio"]["baked"][0]["provenance"]
        .as_object_mut()
        .expect("provenance object")
        .remove("hop_seconds");
    expect_code(&doc, "LRF-103");
}

#[test]
fn wrong_fathom_schema_is_refused_lrf_103() {
    let mut doc = valid_doc();
    doc["fathom"]["schema"] = json!("fathom-motion-record/2");
    expect_code(&doc, "LRF-103");
}

#[test]
fn fathom_phase_starting_after_it_ends_is_refused_lrf_305() {
    let mut doc = valid_doc();
    doc["fathom"]["phases"][0]["start_frame"] = json!(90);
    expect_code(&doc, "LRF-305");
}

#[test]
fn unknown_arc_kind_is_refused_lrf_103() {
    let mut doc = valid_doc();
    doc["fathom"]["arc"]["kind"] = json!("spiral");
    expect_code(&doc, "LRF-103");
}

#[test]
fn curve_without_keyframes_is_refused_lrf_103() {
    let mut doc = valid_doc();
    doc["curves"][1]["keyframes"] = json!([]);
    expect_code(&doc, "LRF-103");
}

#[test]
fn too_many_curves_is_refused_lrf_107() {
    let mut doc = valid_doc();
    let curve = doc["curves"][1].clone();
    let curves = doc["curves"].as_array_mut().expect("curves array");
    curves.clear();
    for index in 0..513 {
        let mut entry = curve.clone();
        entry["parameter"] = json!(format!("angle_x_{index}"));
        curves.push(entry);
    }
    expect_code(&doc, "LRF-107");
}

#[test]
fn oversized_input_is_refused_lrf_101() {
    let bytes = vec![b' '; lumen_rig_core::limits::INPUT_BYTES_MAX + 1];
    match parse_motion(&bytes, &full_model()) {
        Err(error) => assert_eq!(error.code(), "LRF-101"),
        Ok(_) => panic!("oversized input should be refused"),
    }
}

#[test]
fn duplicate_json_key_is_refused_lrf_103() {
    let text = fixture("minimal.motion.json");
    let mut raw = String::from_utf8(text).expect("fixture is UTF-8");
    raw = raw.replacen("\"id\":", "\"id\": \"nod\", \"id\":", 1);
    match parse_motion(
        raw.as_bytes(),
        &parse_model(&fixture("minimal.model.json")).unwrap(),
    ) {
        Err(error) => assert_eq!(error.code(), "LRF-103", "error: {error}"),
        Ok(_) => panic!("duplicate key should be refused"),
    }
}

#[test]
fn refusal_is_a_typed_error_with_context() {
    let mut doc = valid_doc();
    doc["curves"][0]["parameter"] = json!("sway");
    let bytes = serde_json::to_vec(&doc).expect("serialize");
    let error: ParseError = parse_motion(&bytes, &full_model()).unwrap_err();
    let text = error.to_string();
    assert!(text.contains("LRF-602"), "display carries the code: {text}");
    assert!(text.contains("sway"), "display names the parameter: {text}");
}
