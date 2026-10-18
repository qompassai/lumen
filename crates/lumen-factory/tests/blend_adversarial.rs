//! Adversarial tests for blendshape composition (Phase 1b):
//! hostile template inputs must fail closed, never panic, and never
//! silently drop a channel or clamp a weight.

use std::collections::BTreeMap;
use std::path::PathBuf;

use lumen_factory::base::build_base_mesh;
use lumen_factory::blend::{
    EmotionTemplate, Templates, compose_targets, load_templates, parse_templates,
};

fn head_inputs() -> (Vec<[f32; 3]>, Vec<lumen_factory::base::HeadRegionWeights>) {
    let mesh = build_base_mesh().expect("base builds");
    let head = mesh
        .parts
        .iter()
        .find(|part| part.name == "head")
        .expect("head part");
    let positions: Vec<[f32; 3]> = head.vertices.iter().map(|v| v.position).collect();
    (positions, mesh.head_masks.clone())
}

fn minimal_json(arkit: &str) -> String {
    format!(
        r#"{{
  "emotions": [
    {{"key": "happy", "display": "Happy", "arkit": {arkit}}},
    {{"key": "neutral", "display": "Neutral", "arkit": {{}}}},
    {{"key": "surprised", "display": "Surprised", "arkit": {{}}}},
    {{"key": "embarrassed", "display": "Embarrassed", "arkit": {{"cheekSquintLeft": 0.3, "cheekSquintRight": 0.3}}}},
    {{"key": "annoyed", "display": "Annoyed", "arkit": {{}}}}
  ],
  "procedural_groups": {{"blink": ["blink"], "gaze": ["lookUp"], "visemes": ["aa"]}},
  "sheet_mood_mapping": {{"alarmed": "x", "blush": "x", "celebrate": "x", "idle": "neutral", "pout": "x", "wink": "x"}}
}}"#
    )
}

fn manual_templates(arkit: BTreeMap<String, f32>) -> Templates {
    let entry = |key: &str, arkit: BTreeMap<String, f32>| EmotionTemplate {
        arkit,
        display: key.to_string(),
        key: key.to_string(),
    };
    Templates {
        emotions: vec![
            entry("happy", arkit),
            entry("neutral", BTreeMap::new()),
            entry("surprised", BTreeMap::new()),
            entry(
                "embarrassed",
                BTreeMap::from([
                    ("cheekSquintLeft".to_string(), 0.3),
                    ("cheekSquintRight".to_string(), 0.3),
                ]),
            ),
            entry("annoyed", BTreeMap::new()),
        ],
        procedural_groups: BTreeMap::from([
            ("blink".to_string(), vec!["blink".to_string()]),
            ("gaze".to_string(), vec!["lookUp".to_string()]),
            ("visemes".to_string(), vec!["aa".to_string()]),
        ]),
        sheet_mood_mapping: BTreeMap::from([
            ("alarmed".to_string(), "x".to_string()),
            ("blush".to_string(), "x".to_string()),
            ("celebrate".to_string(), "x".to_string()),
            ("pout".to_string(), "x".to_string()),
            ("wink".to_string(), "x".to_string()),
        ]),
    }
}

#[test]
fn malformed_json_is_rejected() {
    assert!(parse_templates("{not json").is_err());
    assert!(parse_templates("").is_err());
    assert!(parse_templates("[1, 2, 3]").is_err());
    assert!(parse_templates("42").is_err());
}

#[test]
fn missing_emotions_list_is_rejected() {
    assert!(parse_templates(r#"{"procedural_groups": {}}"#).is_err());
    assert!(parse_templates(r#"{"emotions": {}}"#).is_err());
    assert!(parse_templates(r#"{"emotions": []}"#).is_err());
}

#[test]
fn missing_file_is_rejected() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/does_not_exist.json");
    assert!(load_templates(&path).is_err());
}

#[test]
fn unknown_channel_in_recipe_fails_closed_naming_it() {
    let templates = parse_templates(&minimal_json(
        r#"{"mouthSmileLeft": 0.5, "channelThatDoesNotExist": 0.5}"#,
    ))
    .expect("load does not know fields; compose must reject");
    let (positions, masks) = head_inputs();
    let error = compose_targets(&templates, &positions, &masks)
        .expect_err("unknown channel must fail compose");
    assert!(
        error.to_string().contains("channelThatDoesNotExist"),
        "error must name the channel: {error}"
    );
}

#[test]
fn weight_above_one_is_rejected_not_clamped() {
    let result = parse_templates(&minimal_json(r#"{"mouthSmileLeft": 1.5}"#));
    assert!(result.is_err(), "weight 1.5 must be an error: {result:?}");
    // Also at compose time for manually built templates.
    let templates = manual_templates(BTreeMap::from([("mouthSmileLeft".to_string(), 1.5)]));
    let (positions, masks) = head_inputs();
    assert!(compose_targets(&templates, &positions, &masks).is_err());
}

#[test]
fn negative_weight_is_rejected() {
    assert!(parse_templates(&minimal_json(r#"{"mouthSmileLeft": -0.25}"#)).is_err());
}

#[test]
fn nan_weight_is_rejected() {
    // A NaN literal is not valid JSON, so parsing fails closed...
    assert!(parse_templates(&minimal_json(r#"{"mouthSmileLeft": NaN}"#)).is_err());
    // ...and a NaN weight in a manually built template fails compose.
    let templates = manual_templates(BTreeMap::from([("mouthSmileLeft".to_string(), f32::NAN)]));
    let (positions, masks) = head_inputs();
    let error = compose_targets(&templates, &positions, &masks).expect_err("NaN must fail");
    assert!(error.to_string().contains("mouthSmileLeft"), "{error}");
    // Infinity likewise.
    let templates_inf = manual_templates(BTreeMap::from([(
        "mouthSmileLeft".to_string(),
        f32::INFINITY,
    )]));
    assert!(compose_targets(&templates_inf, &positions, &masks).is_err());
}

#[test]
fn duplicate_emotion_keys_are_rejected() {
    let json = r#"{
  "emotions": [
    {"key": "happy", "display": "Happy", "arkit": {}},
    {"key": "happy", "display": "Happy Again", "arkit": {}}
  ],
  "procedural_groups": {"blink": ["blink"], "gaze": [], "visemes": []},
  "sheet_mood_mapping": {"alarmed": "x", "blush": "x", "celebrate": "x", "pout": "x", "wink": "x"}
}"#;
    let error = parse_templates(json).expect_err("duplicate keys must fail");
    assert!(error.to_string().contains("happy"), "{error}");
}

#[test]
fn empty_head_is_rejected() {
    let templates = manual_templates(BTreeMap::new());
    assert!(compose_targets(&templates, &[], &[]).is_err());
    // Mismatched positions/masks length is also an error.
    let (positions, masks) = head_inputs();
    assert!(compose_targets(&templates, &positions, &masks[..masks.len() - 1]).is_err());
}

#[test]
fn unknown_procedural_target_fails_closed_naming_it() {
    let mut templates = manual_templates(BTreeMap::new());
    templates
        .procedural_groups
        .insert("blink".to_string(), vec!["blinkBogus".to_string()]);
    let (positions, masks) = head_inputs();
    let error = compose_targets(&templates, &positions, &masks)
        .expect_err("unknown procedural target must fail");
    assert!(error.to_string().contains("blinkBogus"), "{error}");
}
