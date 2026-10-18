//! Validation tests for blendshape composition (Phase 1b).

use std::collections::BTreeSet;
use std::path::PathBuf;

use lumen_factory::base::build_base_mesh;
use lumen_factory::blend::{compose_targets, load_templates, parse_templates};
use lumen_factory::gltf::{MorphTarget, parse_glb, write_glb};

fn fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/emotions_fixture.json")
}

fn real_templates_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../templates/emotions.json")
}

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

fn expected_emotion_keys() -> BTreeSet<String> {
    [
        "angry",
        "annoyed",
        "determined",
        "embarrassed",
        "fun",
        "happy",
        "neutral",
        "sad",
        "smug",
        "surprised",
        "worried",
    ]
    .iter()
    .map(|key| key.to_string())
    .collect()
}

#[test]
fn fixture_parses_with_all_11_emotion_keys() {
    let templates = load_templates(&fixture_path()).expect("fixture parses");
    let keys: BTreeSet<String> = templates
        .emotions
        .iter()
        .map(|entry| entry.key.clone())
        .collect();
    assert_eq!(keys, expected_emotion_keys());
    assert_eq!(templates.emotions.len(), 11);
}

#[test]
fn real_repo_templates_parse_with_all_11_keys() {
    let templates = load_templates(&real_templates_path()).expect("real templates parse");
    let keys: BTreeSet<String> = templates
        .emotions
        .iter()
        .map(|entry| entry.key.clone())
        .collect();
    assert_eq!(keys, expected_emotion_keys());
    // The ratified recipes reference exactly these 30 ARKit channels.
    let referenced = templates.referenced_channels();
    assert_eq!(referenced.len(), 30, "{referenced:?}");
    for channel in [
        "browInnerUp",
        "eyeBlinkRight",
        "jawOpen",
        "mouthSmileLeft",
        "noseSneerRight",
    ] {
        assert!(referenced.contains(channel), "missing {channel}");
    }
    assert!(templates.procedural_groups.contains_key("blink"));
    assert!(templates.procedural_groups.contains_key("gaze"));
    assert!(templates.procedural_groups.contains_key("visemes"));
}

#[test]
fn compose_produces_28_sorted_targets() {
    let templates = load_templates(&real_templates_path()).expect("real templates parse");
    let (positions, masks) = head_inputs();
    let targets = compose_targets(&templates, &positions, &masks).expect("compose");
    assert_eq!(
        targets.len(),
        28,
        "{:?}",
        targets.iter().map(|t| &t.0).collect::<Vec<_>>()
    );
    let names: Vec<String> = targets.iter().map(|target| target.0.clone()).collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted, "targets must be alphabetical");
    for expected in [
        "aa",
        "alarmed",
        "blink",
        "blinkLeft",
        "blinkRight",
        "blush",
        "celebrate",
        "ee",
        "happy",
        "ih",
        "lookDown",
        "lookLeft",
        "lookRight",
        "lookUp",
        "neutral",
        "oh",
        "ou",
        "pout",
        "wink",
    ] {
        assert!(
            names.iter().any(|name| name == expected),
            "missing {expected}"
        );
    }
    assert!(
        !names.iter().any(|name| name == "idle"),
        "idle must be skipped"
    );
    for (_, deltas) in &targets {
        assert_eq!(deltas.len(), positions.len());
    }
}

#[test]
fn compose_is_deterministic() {
    let templates = load_templates(&fixture_path()).expect("fixture parses");
    let (positions, masks) = head_inputs();
    let first = compose_targets(&templates, &positions, &masks).expect("compose 1");
    let second = compose_targets(&templates, &positions, &masks).expect("compose 2");
    assert_eq!(first, second);
    // Byte-level determinism of the deltas.
    let bytes = |targets: &[(String, Vec<[f32; 3]>)]| -> Vec<u8> {
        let mut out = Vec::new();
        for (name, deltas) in targets {
            out.extend_from_slice(name.as_bytes());
            for delta in deltas {
                for value in delta {
                    out.extend_from_slice(&value.to_le_bytes());
                }
            }
        }
        out
    };
    assert_eq!(bytes(&first), bytes(&second));
}

#[test]
fn neutral_target_is_all_zero_and_happy_is_nonzero() {
    let templates = load_templates(&real_templates_path()).expect("real templates parse");
    let (positions, masks) = head_inputs();
    let targets = compose_targets(&templates, &positions, &masks).expect("compose");
    let neutral = &targets
        .iter()
        .find(|target| target.0 == "neutral")
        .expect("neutral target")
        .1;
    assert!(neutral.iter().all(|delta| *delta == [0.0, 0.0, 0.0]));
    let happy = &targets
        .iter()
        .find(|target| target.0 == "happy")
        .expect("happy target")
        .1;
    let max_delta = happy
        .iter()
        .flat_map(|delta| delta.iter())
        .fold(0.0f32, |acc, value| acc.max(value.abs()));
    assert!(max_delta > 0.001, "happy max delta {max_delta}");
}

#[test]
fn sheet_and_procedural_compositions_follow_the_mapping() {
    let templates = load_templates(&real_templates_path()).expect("real templates parse");
    let (positions, masks) = head_inputs();
    let targets = compose_targets(&templates, &positions, &masks).expect("compose");
    let get = |name: &str| {
        targets
            .iter()
            .find(|target| target.0 == name)
            .unwrap_or_else(|| panic!("target {name}"))
            .1
            .clone()
    };
    // celebrate == happy at full weight.
    assert_eq!(get("celebrate"), get("happy"));
    // wink == happy plus a right blink: different, and its max delta
    // is at least happy's (the blink only adds lid motion).
    assert_ne!(get("wink"), get("happy"));
    // alarmed differs from surprised (brow reinforcement).
    assert_ne!(get("alarmed"), get("surprised"));
    // pout differs from annoyed (mouthShrugLower emphasis).
    assert_ne!(get("pout"), get("annoyed"));
    // blush is the cheek-only component: nonzero but smaller than the
    // full embarrassed target.
    let max_of = |deltas: &[[f32; 3]]| {
        deltas
            .iter()
            .flat_map(|delta| delta.iter())
            .fold(0.0f32, |acc, value| acc.max(value.abs()))
    };
    assert!(max_of(&get("blush")) > 0.0);
    assert!(max_of(&get("blush")) < max_of(&get("embarrassed")));
    // Procedural targets are nonzero where they should be.
    for name in ["blink", "blinkLeft", "lookUp", "lookLeft", "aa", "ee", "oh"] {
        assert!(max_of(&get(name)) > 0.0, "{name} must be nonzero");
    }
}

#[test]
fn blended_glb_round_trips_target_names() {
    let templates = load_templates(&real_templates_path()).expect("real templates parse");
    let mesh = build_base_mesh().expect("base builds");
    let head = mesh
        .parts
        .iter()
        .find(|part| part.name == "head")
        .expect("head part");
    let positions: Vec<[f32; 3]> = head.vertices.iter().map(|v| v.position).collect();
    let composed = compose_targets(&templates, &positions, &mesh.head_masks).expect("compose");
    let morphs: Vec<MorphTarget> = composed
        .iter()
        .map(|(name, deltas)| MorphTarget {
            deltas: deltas.clone(),
            name: name.clone(),
        })
        .collect();
    let bytes = write_glb(&mesh, &morphs).expect("write blended glb");
    let summary = parse_glb(&bytes).expect("parse blended glb");
    let expected_names: Vec<String> = composed.iter().map(|target| target.0.clone()).collect();
    assert_eq!(summary.target_names, expected_names);
    assert_eq!(summary.joint_count, 65);
}

#[test]
fn fixture_text_parses_via_parse_templates() {
    let text = std::fs::read_to_string(fixture_path()).expect("fixture readable");
    let templates = parse_templates(&text).expect("parse_templates");
    assert_eq!(templates.emotions.len(), 11);
    let happy = templates.emotion("happy").expect("happy entry");
    assert_eq!(happy.display, "Happy");
    assert!(happy.arkit.contains_key("mouthSmileLeft"));
}
