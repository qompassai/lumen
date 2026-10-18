//! Blendshape composition from ratified emotion templates.
//!
//! Contract summary:
//! - Templates are loaded from a caller-supplied path (production:
//!   repo-root `templates/emotions.json`). The path is never hardcoded
//!   in library code; tests resolve it from `CARGO_MANIFEST_DIR`.
//! - Top level is an object; `emotions` is a LIST of entries with
//!   `key` / `display` / `arkit{channel: weight}`; `procedural_groups`
//!   (`blink`, `gaze`, `visemes`) and `sheet_mood_mapping` are required.
//! - Weights are validated finite and in `0.0..=1.0` at load time and
//!   again at compose time. Out-of-range weights are an error; nothing
//!   is ever clamped or silently dropped.
//! - Any recipe, procedural definition, or sheet composition that
//!   references a channel with no defined displacement field fails
//!   closed with a [`FactoryError`] naming the channel.
//! - Composition is deterministic: channels are applied in sorted
//!   (alphabetical) channel-name order and targets are returned sorted
//!   alphabetically by target name.
//!
//! # Targets produced (28 for the ratified templates)
//!
//! - All emotion keys from the file (11 for the ratified file,
//!   including `neutral`, whose recipe is empty and whose target is
//!   therefore all-zero by construction).
//! - Procedural targets: `blink`, `blinkLeft`, `blinkRight`,
//!   `lookUp`, `lookDown`, `lookLeft`, `lookRight`, and the visemes
//!   `aa`, `ee`, `ih`, `oh`, `ou`.
//! - Sheet targets from `sheet_mood_mapping`: `alarmed` (surprised plus
//!   browInnerUp reinforced by +0.30), `blush` (the cheek component of
//!   embarrassed only), `celebrate` (happy at full weight, i.e. the
//!   happy recipe unchanged), `pout` (annoyed plus `mouthShrugLower`
//!   at 0.90 = a 0.60 base emphasis multiplied by 1.5), and `wink`
//!   (happy plus `eyeBlinkRight` at 1.0). `idle` maps to neutral and
//!   is skipped, per the mapping.
//!
//! # Displacement field table (per-head-vertex offset at weight 1.0)
//!
//! Fields are built from the `base.rs` head masks (brow, upper_lid,
//! lower_lid, eye_corner, mouth, lips, cheek, jaw) and head-local
//! geometry (left/right side factors from the sign of X relative to
//! the head centre — character left is -X in `base.rs`; inner/outer
//! and mouth-corner factors from |X|; a nose weight derived from the
//! head-local position, since there is no nose mask). Amplitudes are
//! in meters at facial scale (roughly 2–10 mm).
//!
//! | Channel group | Field |
//! |---|---|
//! | browDown{Left,Right} | brow * side, -Y 0.005 |
//! | browInnerUp | brow * inner, +Y 0.006 |
//! | browOuterUp{Left,Right} | brow * side * outer, +Y 0.005 |
//! | cheekPuff | cheek, per-vertex outward X 0.003 + Z 0.004 |
//! | cheekSquint{Left,Right} | cheek * side, +Y 0.004 +Z 0.001 |
//! | eyeBlink{Left,Right} | upper_lid * side -Y 0.008, lower_lid * side +Y 0.002 |
//! | eyeLook{Down,Up}{Left,Right} | eye region (upper+lower+corner) * side, -/+Y 0.002 |
//! | eyeLook{In,Out}{Left,Right} | eye region * side, toward/away from nose X 0.002 |
//! | eyeSquint{Left,Right} | upper_lid * side -Y 0.004, lower_lid * side +Y 0.003 |
//! | eyeWide{Left,Right} | upper_lid * side +Y 0.005, lower_lid * side -Y 0.002 |
//! | jawForward | jaw, +Z 0.006 |
//! | jaw{Left,Right} | jaw, -/+X 0.005 |
//! | jawOpen | jaw, -Y 0.010 -Z 0.002 |
//! | mouthClose | lips, -Z 0.002 |
//! | mouthDimple{Left,Right} | mouth * side * corner, outward X 0.003 +Y 0.001 -Z 0.002 |
//! | mouthFrown{Left,Right} | mouth * side * (0.3 + 0.7*corner), -Y 0.005 |
//! | mouthFunnel | lips +Z 0.006, mouth +Z 0.003 |
//! | mouth{Left,Right} | mouth, -/+X 0.005 |
//! | mouthLowerDown{Left,Right} | lips * side, -Y 0.004 +Z 0.001 |
//! | mouthPress{Left,Right} | lips * side, -Z 0.003 |
//! | mouthPucker | lips, +Z 0.007 |
//! | mouthRollLower | lips, -Y 0.002 -Z 0.003 |
//! | mouthRollUpper | lips, +Y 0.002 -Z 0.003 |
//! | mouthShrugLower | lips, +Y 0.003 +Z 0.004 (pushed lower lip) |
//! | mouthShrugUpper | lips, +Y 0.003 +Z 0.002 |
//! | mouthSmile{Left,Right} | mouth * side, outward X 0.006 +Y 0.004 |
//! | mouthStretch{Left,Right} | mouth * side, outward X 0.007 -Y 0.001 |
//! | mouthUpperUp{Left,Right} | lips * side, +Y 0.004 |
//! | noseSneer{Left,Right} | nose weight * side, +Y 0.003 +Z 0.001 |
//! | tongueOut | jaw * 0.5 jawOpen direction + lips +Z 0.006 (approximation) |
//!
//! `eyeLook*` fields move the lid / eye-corner region only: v1 uses
//! painted eyes on the face texture, so a gaze shift is approximated
//! by shifting that region in the look direction (documented v1
//! approximation, not separate eye geometry).

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde_json::Value;

use crate::FactoryError;
use crate::base::HeadRegionWeights;

/// One emotion entry from the templates file.
#[derive(Clone, Debug, PartialEq)]
pub struct EmotionTemplate {
    /// ARKit channel weights (sorted by channel name).
    pub arkit: BTreeMap<String, f32>,
    /// Human-readable display name.
    pub display: String,
    /// Canonical emotion key (e.g. `happy`).
    pub key: String,
}

/// One composed morph target: name plus per-head-vertex deltas.
pub type ComposedTarget = (String, Vec<[f32; 3]>);

/// All composed morph targets, sorted alphabetically by name.
pub type ComposedTargets = Vec<ComposedTarget>;

/// Parsed emotion templates file.
#[derive(Clone, Debug, PartialEq)]
pub struct Templates {
    /// Emotion entries in file order.
    pub emotions: Vec<EmotionTemplate>,
    /// Procedural groups: group name (`blink`/`gaze`/`visemes`) to the
    /// target names in that group, in file order.
    pub procedural_groups: BTreeMap<String, Vec<String>>,
    /// Sheet mood mapping entries (mood name to description text).
    pub sheet_mood_mapping: BTreeMap<String, String>,
}

impl Templates {
    /// Look up one emotion by key.
    pub fn emotion(&self, key: &str) -> Option<&EmotionTemplate> {
        self.emotions.iter().find(|entry| entry.key == key)
    }

    /// ARKit channels referenced by the emotion recipes in the file,
    /// sorted. For the ratified file this is the 30-channel set
    /// enumerated in the Phase 1b report.
    pub fn referenced_channels(&self) -> BTreeSet<String> {
        let mut out = BTreeSet::new();
        for entry in &self.emotions {
            for channel in entry.arkit.keys() {
                out.insert(channel.clone());
            }
        }
        out
    }
}

/// All ARKit channel names with a defined displacement field (52),
/// sorted alphabetically.
pub const KNOWN_CHANNELS: &[&str] = &[
    "browDownLeft",
    "browDownRight",
    "browInnerUp",
    "browOuterUpLeft",
    "browOuterUpRight",
    "cheekPuff",
    "cheekSquintLeft",
    "cheekSquintRight",
    "eyeBlinkLeft",
    "eyeBlinkRight",
    "eyeLookDownLeft",
    "eyeLookDownRight",
    "eyeLookInLeft",
    "eyeLookInRight",
    "eyeLookOutLeft",
    "eyeLookOutRight",
    "eyeLookUpLeft",
    "eyeLookUpRight",
    "eyeSquintLeft",
    "eyeSquintRight",
    "eyeWideLeft",
    "eyeWideRight",
    "jawForward",
    "jawLeft",
    "jawOpen",
    "jawRight",
    "mouthClose",
    "mouthDimpleLeft",
    "mouthDimpleRight",
    "mouthFrownLeft",
    "mouthFrownRight",
    "mouthFunnel",
    "mouthLeft",
    "mouthLowerDownLeft",
    "mouthLowerDownRight",
    "mouthPressLeft",
    "mouthPressRight",
    "mouthPucker",
    "mouthRight",
    "mouthRollLower",
    "mouthRollUpper",
    "mouthShrugLower",
    "mouthShrugUpper",
    "mouthSmileLeft",
    "mouthSmileRight",
    "mouthStretchLeft",
    "mouthStretchRight",
    "mouthUpperUpLeft",
    "mouthUpperUpRight",
    "noseSneerLeft",
    "noseSneerRight",
    "tongueOut",
];

fn validate_weight(context: &str, channel: &str, weight: f32) -> Result<(), FactoryError> {
    if !weight.is_finite() {
        return Err(FactoryError::BadParam(format!(
            "{context}: channel {channel} has non-finite weight {weight}"
        )));
    }
    if !(0.0..=1.0).contains(&weight) {
        return Err(FactoryError::BadParam(format!(
            "{context}: channel {channel} weight {weight} out of range 0.0..=1.0"
        )));
    }
    Ok(())
}

fn validate_recipe(context: &str, recipe: &BTreeMap<String, f32>) -> Result<(), FactoryError> {
    for (channel, weight) in recipe {
        validate_weight(context, channel, *weight)?;
    }
    Ok(())
}

/// Load templates from a caller-supplied path. Fails closed on I/O
/// failure, malformed JSON, a missing `emotions` list, duplicate
/// emotion keys, or any out-of-range / non-finite weight.
pub fn load_templates(path: &Path) -> Result<Templates, FactoryError> {
    let text = std::fs::read_to_string(path)?;
    parse_templates(&text)
}

/// Parse templates from JSON text. See [`load_templates`].
pub fn parse_templates(text: &str) -> Result<Templates, FactoryError> {
    let root: Value = serde_json::from_str(text)
        .map_err(|e| FactoryError::BadParam(format!("malformed templates json: {e}")))?;
    let object = root.as_object().ok_or_else(|| {
        FactoryError::BadParam("templates top level is not an object".to_string())
    })?;
    let emotions_value = object
        .get("emotions")
        .ok_or_else(|| FactoryError::BadParam("templates missing emotions list".to_string()))?;
    let emotions_list = emotions_value
        .as_array()
        .ok_or_else(|| FactoryError::BadParam("templates emotions is not a list".to_string()))?;
    if emotions_list.is_empty() {
        return Err(FactoryError::BadParam(
            "templates emotions list is empty".to_string(),
        ));
    }
    let mut emotions: Vec<EmotionTemplate> = Vec::with_capacity(emotions_list.len());
    let mut seen_keys: BTreeSet<String> = BTreeSet::new();
    for entry in emotions_list {
        let entry_object = entry
            .as_object()
            .ok_or_else(|| FactoryError::BadParam("emotion entry is not an object".to_string()))?;
        let key = entry_object
            .get("key")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| FactoryError::BadParam("emotion entry missing key".to_string()))?
            .to_string();
        if !seen_keys.insert(key.clone()) {
            return Err(FactoryError::BadParam(format!(
                "duplicate emotion key: {key}"
            )));
        }
        let display = entry_object
            .get("display")
            .and_then(Value::as_str)
            .unwrap_or(&key)
            .to_string();
        let arkit_object = entry_object
            .get("arkit")
            .and_then(Value::as_object)
            .ok_or_else(|| FactoryError::BadParam(format!("emotion {key} missing arkit object")))?;
        let mut arkit: BTreeMap<String, f32> = BTreeMap::new();
        for (channel, value) in arkit_object {
            let number = value.as_f64().ok_or_else(|| {
                FactoryError::BadParam(format!(
                    "emotion {key} channel {channel} weight is not a number"
                ))
            })?;
            let weight = number as f32;
            // as_f64 -> as f32 can overflow to infinity for huge
            // literals; the finite check in validate_weight rejects it.
            if (number - f64::from(weight)).abs() > 1e-6 && number.is_finite() {
                return Err(FactoryError::BadParam(format!(
                    "emotion {key} channel {channel} weight {number} not representable"
                )));
            }
            validate_weight(&format!("emotion {key}"), channel, weight)?;
            arkit.insert(channel.clone(), weight);
        }
        emotions.push(EmotionTemplate {
            arkit,
            display,
            key,
        });
    }

    let groups_object = object
        .get("procedural_groups")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            FactoryError::BadParam("templates missing procedural_groups object".to_string())
        })?;
    let mut procedural_groups: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for group_name in ["blink", "gaze", "visemes"] {
        let names = groups_object
            .get(group_name)
            .and_then(Value::as_array)
            .ok_or_else(|| {
                FactoryError::BadParam(format!("procedural_groups missing {group_name} list"))
            })?;
        let mut list: Vec<String> = Vec::with_capacity(names.len());
        for name in names {
            let text = name.as_str().ok_or_else(|| {
                FactoryError::BadParam(format!(
                    "procedural_groups {group_name} entry is not a string"
                ))
            })?;
            list.push(text.to_string());
        }
        procedural_groups.insert(group_name.to_string(), list);
    }

    let sheet_object = object
        .get("sheet_mood_mapping")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            FactoryError::BadParam("templates missing sheet_mood_mapping object".to_string())
        })?;
    let mut sheet_mood_mapping: BTreeMap<String, String> = BTreeMap::new();
    for (mood, value) in sheet_object {
        if let Some(text) = value.as_str() {
            sheet_mood_mapping.insert(mood.clone(), text.to_string());
        }
    }
    for mood in ["alarmed", "blush", "celebrate", "pout", "wink"] {
        if !sheet_mood_mapping.contains_key(mood) {
            return Err(FactoryError::BadParam(format!(
                "sheet_mood_mapping missing {mood}"
            )));
        }
    }

    Ok(Templates {
        emotions,
        procedural_groups,
        sheet_mood_mapping,
    })
}

/// Procedural target definitions, keyed by target name. Each recipe
/// uses only channels with defined fields. Gaze left/right use the
/// ARKit in/out pairing (the left eye looks out, the right eye looks
/// in, for a leftward gaze; mirrored for rightward).
pub fn procedural_definitions() -> BTreeMap<String, BTreeMap<String, f32>> {
    let mut definitions: BTreeMap<String, BTreeMap<String, f32>> = BTreeMap::new();
    let mut add = |name: &str, channels: &[(&str, f32)]| {
        let recipe: BTreeMap<String, f32> = channels
            .iter()
            .map(|(channel, weight)| (channel.to_string(), *weight))
            .collect();
        definitions.insert(name.to_string(), recipe);
    };
    add(
        "aa",
        &[
            ("jawOpen", 0.85),
            ("mouthStretchLeft", 0.25),
            ("mouthStretchRight", 0.25),
        ],
    );
    add("blink", &[("eyeBlinkLeft", 1.0), ("eyeBlinkRight", 1.0)]);
    add("blinkLeft", &[("eyeBlinkLeft", 1.0)]);
    add("blinkRight", &[("eyeBlinkRight", 1.0)]);
    add(
        "ee",
        &[
            ("jawOpen", 0.15),
            ("mouthSmileLeft", 0.25),
            ("mouthSmileRight", 0.25),
            ("mouthStretchLeft", 0.65),
            ("mouthStretchRight", 0.65),
        ],
    );
    add(
        "ih",
        &[
            ("jawOpen", 0.35),
            ("mouthStretchLeft", 0.4),
            ("mouthStretchRight", 0.4),
        ],
    );
    add(
        "lookDown",
        &[("eyeLookDownLeft", 1.0), ("eyeLookDownRight", 1.0)],
    );
    add(
        "lookLeft",
        &[("eyeLookInRight", 1.0), ("eyeLookOutLeft", 1.0)],
    );
    add(
        "lookRight",
        &[("eyeLookInLeft", 1.0), ("eyeLookOutRight", 1.0)],
    );
    add("lookUp", &[("eyeLookUpLeft", 1.0), ("eyeLookUpRight", 1.0)]);
    add("oh", &[("jawOpen", 0.5), ("mouthFunnel", 0.85)]);
    add(
        "ou",
        &[
            ("jawOpen", 0.25),
            ("mouthFunnel", 0.55),
            ("mouthPucker", 0.45),
        ],
    );
    definitions
}

/// Sheet target recipes, derived from the emotion recipes in
/// `templates` following the `sheet_mood_mapping` text. See the
/// module header for the composition rules.
pub fn sheet_definitions(
    templates: &Templates,
) -> Result<BTreeMap<String, BTreeMap<String, f32>>, FactoryError> {
    let recipe_of = |key: &str| -> Result<BTreeMap<String, f32>, FactoryError> {
        templates
            .emotion(key)
            .map(|entry| entry.arkit.clone())
            .ok_or_else(|| {
                FactoryError::BadParam(format!("sheet composition needs missing emotion {key}"))
            })
    };
    let mut out: BTreeMap<String, BTreeMap<String, f32>> = BTreeMap::new();

    // alarmed = surprised + browInnerUp reinforcement (+0.30).
    let mut alarmed = recipe_of("surprised")?;
    let reinforced = alarmed.get("browInnerUp").copied().unwrap_or(0.0) + 0.30;
    validate_weight("sheet alarmed", "browInnerUp", reinforced)?;
    alarmed.insert("browInnerUp".to_string(), reinforced);
    out.insert("alarmed".to_string(), alarmed);

    // blush = the cheek component of embarrassed only.
    let embarrassed = recipe_of("embarrassed")?;
    let mut blush: BTreeMap<String, f32> = BTreeMap::new();
    for channel in ["cheekSquintLeft", "cheekSquintRight"] {
        if let Some(weight) = embarrassed.get(channel) {
            blush.insert(channel.to_string(), *weight);
        }
    }
    if blush.is_empty() {
        return Err(FactoryError::BadParam(
            "sheet blush: embarrassed has no cheek component".to_string(),
        ));
    }
    out.insert("blush".to_string(), blush);

    // celebrate = happy at full weight (the happy recipe unchanged).
    out.insert("celebrate".to_string(), recipe_of("happy")?);

    // pout = annoyed + mouthShrugLower emphasized x1.5 (0.60 * 1.5).
    let mut pout = recipe_of("annoyed")?;
    pout.insert("mouthShrugLower".to_string(), 0.60 * 1.5);
    out.insert("pout".to_string(), pout);

    // wink = happy composed + blinkRight at 1.0.
    let mut wink = recipe_of("happy")?;
    wink.insert("eyeBlinkRight".to_string(), 1.0);
    out.insert("wink".to_string(), wink);

    Ok(out)
}

fn scale(weight: f32, direction: [f32; 3]) -> [f32; 3] {
    [
        weight * direction[0],
        weight * direction[1],
        weight * direction[2],
    ]
}

fn add_offset(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

/// Displacement field for one channel, or `None` when the channel has
/// no defined field (the fail-closed signal for callers).
pub fn channel_field(
    channel: &str,
    head_positions: &[[f32; 3]],
    masks: &[HeadRegionWeights],
    center: [f32; 3],
) -> Option<Vec<[f32; 3]>> {
    if !KNOWN_CHANNELS.contains(&channel) {
        return None;
    }
    let mut field: Vec<[f32; 3]> = Vec::with_capacity(head_positions.len());
    for (position, mask) in head_positions.iter().zip(masks.iter()) {
        let x = position[0];
        let left = (0.5 + (center[0] - x) / 0.03).clamp(0.0, 1.0);
        let right = (0.5 + (x - center[0]) / 0.03).clamp(0.0, 1.0);
        let inner = (1.0 - (x - center[0]).abs() / 0.045).clamp(0.0, 1.0);
        let outer = ((x - center[0]).abs() / 0.06).clamp(0.0, 1.0);
        let corner = ((x - center[0]).abs() / 0.045).clamp(0.0, 1.0);
        let eye_region = (mask.upper_lid + mask.lower_lid + mask.eye_corner).clamp(0.0, 1.0);
        // Nose weight from head-local geometry (no nose mask exists).
        let nose_dx = (x - center[0]) / 0.028;
        let nose_dy = (position[1] - (center[1] - 0.02)) / 0.028;
        let nose_base = (1.0 - (nose_dx * nose_dx + nose_dy * nose_dy)).clamp(0.0, 1.0);
        let nose_front = ((position[2] - center[2]) / 0.06).clamp(0.0, 1.0);
        let nose = nose_base * nose_front;
        let outward_x = (x - center[0]).signum();
        let offset = match channel {
            "browDownLeft" => scale(mask.brow * left, [0.0, -0.005, 0.0]),
            "browDownRight" => scale(mask.brow * right, [0.0, -0.005, 0.0]),
            "browInnerUp" => scale(mask.brow * inner, [0.0, 0.006, 0.0]),
            "browOuterUpLeft" => scale(mask.brow * left * outer, [0.0, 0.005, 0.0]),
            "browOuterUpRight" => scale(mask.brow * right * outer, [0.0, 0.005, 0.0]),
            "cheekPuff" => scale(mask.cheek, [outward_x * 0.003, 0.0, 0.004]),
            "cheekSquintLeft" => scale(mask.cheek * left, [0.0, 0.004, 0.001]),
            "cheekSquintRight" => scale(mask.cheek * right, [0.0, 0.004, 0.001]),
            "eyeBlinkLeft" => add_offset(
                scale(mask.upper_lid * left, [0.0, -0.008, 0.0]),
                scale(mask.lower_lid * left, [0.0, 0.002, 0.0]),
            ),
            "eyeBlinkRight" => add_offset(
                scale(mask.upper_lid * right, [0.0, -0.008, 0.0]),
                scale(mask.lower_lid * right, [0.0, 0.002, 0.0]),
            ),
            "eyeLookDownLeft" => scale(eye_region * left, [0.0, -0.002, 0.0]),
            "eyeLookDownRight" => scale(eye_region * right, [0.0, -0.002, 0.0]),
            "eyeLookInLeft" => scale(eye_region * left, [0.002, 0.0, 0.0]),
            "eyeLookInRight" => scale(eye_region * right, [-0.002, 0.0, 0.0]),
            "eyeLookOutLeft" => scale(eye_region * left, [-0.002, 0.0, 0.0]),
            "eyeLookOutRight" => scale(eye_region * right, [0.002, 0.0, 0.0]),
            "eyeLookUpLeft" => scale(eye_region * left, [0.0, 0.002, 0.0]),
            "eyeLookUpRight" => scale(eye_region * right, [0.0, 0.002, 0.0]),
            "eyeSquintLeft" => add_offset(
                scale(mask.upper_lid * left, [0.0, -0.004, 0.0]),
                scale(mask.lower_lid * left, [0.0, 0.003, 0.0]),
            ),
            "eyeSquintRight" => add_offset(
                scale(mask.upper_lid * right, [0.0, -0.004, 0.0]),
                scale(mask.lower_lid * right, [0.0, 0.003, 0.0]),
            ),
            "eyeWideLeft" => add_offset(
                scale(mask.upper_lid * left, [0.0, 0.005, 0.0]),
                scale(mask.lower_lid * left, [0.0, -0.002, 0.0]),
            ),
            "eyeWideRight" => add_offset(
                scale(mask.upper_lid * right, [0.0, 0.005, 0.0]),
                scale(mask.lower_lid * right, [0.0, -0.002, 0.0]),
            ),
            "jawForward" => scale(mask.jaw, [0.0, 0.0, 0.006]),
            "jawLeft" => scale(mask.jaw, [-0.005, 0.0, 0.0]),
            "jawOpen" => scale(mask.jaw, [0.0, -0.010, -0.002]),
            "jawRight" => scale(mask.jaw, [0.005, 0.0, 0.0]),
            "mouthClose" => scale(mask.lips, [0.0, 0.0, -0.002]),
            "mouthDimpleLeft" => scale(mask.mouth * left * corner, [-0.003, 0.001, -0.002]),
            "mouthDimpleRight" => scale(mask.mouth * right * corner, [0.003, 0.001, -0.002]),
            "mouthFrownLeft" => scale(mask.mouth * left * (0.3 + 0.7 * corner), [0.0, -0.005, 0.0]),
            "mouthFrownRight" => scale(
                mask.mouth * right * (0.3 + 0.7 * corner),
                [0.0, -0.005, 0.0],
            ),
            "mouthFunnel" => add_offset(
                scale(mask.lips, [0.0, 0.0, 0.006]),
                scale(mask.mouth, [0.0, 0.0, 0.003]),
            ),
            "mouthLeft" => scale(mask.mouth, [-0.005, 0.0, 0.0]),
            "mouthLowerDownLeft" => scale(mask.lips * left, [0.0, -0.004, 0.001]),
            "mouthLowerDownRight" => scale(mask.lips * right, [0.0, -0.004, 0.001]),
            "mouthPressLeft" => scale(mask.lips * left, [0.0, 0.0, -0.003]),
            "mouthPressRight" => scale(mask.lips * right, [0.0, 0.0, -0.003]),
            "mouthPucker" => scale(mask.lips, [0.0, 0.0, 0.007]),
            "mouthRight" => scale(mask.mouth, [0.005, 0.0, 0.0]),
            "mouthRollLower" => scale(mask.lips, [0.0, -0.002, -0.003]),
            "mouthRollUpper" => scale(mask.lips, [0.0, 0.002, -0.003]),
            "mouthShrugLower" => scale(mask.lips, [0.0, 0.003, 0.004]),
            "mouthShrugUpper" => scale(mask.lips, [0.0, 0.003, 0.002]),
            "mouthSmileLeft" => scale(mask.mouth * left, [-0.006, 0.004, 0.0]),
            "mouthSmileRight" => scale(mask.mouth * right, [0.006, 0.004, 0.0]),
            "mouthStretchLeft" => scale(mask.mouth * left, [-0.007, -0.001, 0.0]),
            "mouthStretchRight" => scale(mask.mouth * right, [0.007, -0.001, 0.0]),
            "mouthUpperUpLeft" => scale(mask.lips * left, [0.0, 0.004, 0.0]),
            "mouthUpperUpRight" => scale(mask.lips * right, [0.0, 0.004, 0.0]),
            "noseSneerLeft" => scale(nose * left, [0.0, 0.003, 0.001]),
            "noseSneerRight" => scale(nose * right, [0.0, 0.003, 0.001]),
            "tongueOut" => add_offset(
                scale(mask.jaw, [0.0, -0.005, -0.001]),
                scale(mask.lips, [0.0, 0.0, 0.006]),
            ),
            _ => return None,
        };
        field.push(offset);
    }
    Some(field)
}

fn head_center(positions: &[[f32; 3]]) -> [f32; 3] {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for position in positions {
        for axis in 0..3 {
            min[axis] = min[axis].min(position[axis]);
            max[axis] = max[axis].max(position[axis]);
        }
    }
    [
        (min[0] + max[0]) * 0.5,
        (min[1] + max[1]) * 0.5,
        (min[2] + max[2]) * 0.5,
    ]
}

/// Compose all morph targets for a head.
///
/// `head_positions` and `masks` must be aligned 1:1 (the head part's
/// vertices and the base mesh's head masks) and non-empty; a mismatch
/// or an empty head is a fail-closed error. The returned targets are
/// sorted alphabetically by target name, and each target's deltas are
/// aligned 1:1 with `head_positions`.
pub fn compose_targets(
    templates: &Templates,
    head_positions: &[[f32; 3]],
    masks: &[HeadRegionWeights],
) -> Result<ComposedTargets, FactoryError> {
    if head_positions.is_empty() || masks.is_empty() {
        return Err(FactoryError::MeshInvalid(
            "empty head: no vertices to compose targets for".to_string(),
        ));
    }
    if head_positions.len() != masks.len() {
        return Err(FactoryError::MeshInvalid(format!(
            "head positions {} != masks {}",
            head_positions.len(),
            masks.len()
        )));
    }
    if !head_positions
        .iter()
        .all(|position| position.iter().all(|value| value.is_finite()))
    {
        return Err(FactoryError::MeshInvalid(
            "head position contains non-finite value".to_string(),
        ));
    }

    // Assemble target recipes, failing closed on collisions.
    let mut recipes: BTreeMap<String, BTreeMap<String, f32>> = BTreeMap::new();
    let mut seen_emotion_keys: BTreeSet<String> = BTreeSet::new();
    for entry in &templates.emotions {
        if !seen_emotion_keys.insert(entry.key.clone()) {
            return Err(FactoryError::BadParam(format!(
                "duplicate emotion key: {}",
                entry.key
            )));
        }
        validate_recipe(&format!("emotion {}", entry.key), &entry.arkit)?;
        if recipes
            .insert(entry.key.clone(), entry.arkit.clone())
            .is_some()
        {
            return Err(FactoryError::BadParam(format!(
                "duplicate target name: {}",
                entry.key
            )));
        }
    }

    // Procedural targets: every name listed in the file's groups must
    // have a definition; an unknown listed name fails closed.
    let procedural = procedural_definitions();
    let mut listed: BTreeSet<String> = BTreeSet::new();
    for names in templates.procedural_groups.values() {
        for name in names {
            listed.insert(name.clone());
        }
    }
    for name in &listed {
        let recipe = procedural.get(name).ok_or_else(|| {
            FactoryError::BadParam(format!("unknown procedural target (no definition): {name}"))
        })?;
        validate_recipe(&format!("procedural {name}"), recipe)?;
        if recipes.insert(name.clone(), recipe.clone()).is_some() {
            return Err(FactoryError::BadParam(format!(
                "duplicate target name: {name}"
            )));
        }
    }

    // Sheet targets.
    for (name, recipe) in sheet_definitions(templates)? {
        validate_recipe(&format!("sheet {name}"), &recipe)?;
        if recipes.insert(name.clone(), recipe).is_some() {
            return Err(FactoryError::BadParam(format!(
                "duplicate target name: {name}"
            )));
        }
    }

    // Build every referenced field, in sorted channel order. A
    // channel with no field fails closed, naming the channel.
    let mut referenced: BTreeSet<String> = BTreeSet::new();
    for recipe in recipes.values() {
        for channel in recipe.keys() {
            referenced.insert(channel.clone());
        }
    }
    let center = head_center(head_positions);
    let mut fields: BTreeMap<String, Vec<[f32; 3]>> = BTreeMap::new();
    for channel in &referenced {
        let field = channel_field(channel, head_positions, masks, center).ok_or_else(|| {
            FactoryError::BadParam(format!(
                "unknown channel (no displacement field defined): {channel}"
            ))
        })?;
        fields.insert(channel.clone(), field);
    }

    // Compose: target = sum over channels (sorted) of weight * field.
    let mut targets: ComposedTargets = Vec::with_capacity(recipes.len());
    for (name, recipe) in &recipes {
        let mut deltas = vec![[0.0f32; 3]; head_positions.len()];
        for (channel, weight) in recipe {
            let field = &fields[channel];
            for (delta, offset) in deltas.iter_mut().zip(field.iter()) {
                delta[0] += weight * offset[0];
                delta[1] += weight * offset[1];
                delta[2] += weight * offset[2];
            }
        }
        if !deltas
            .iter()
            .all(|delta| delta.iter().all(|value| value.is_finite()))
        {
            return Err(FactoryError::MeshInvalid(format!(
                "target {name} contains non-finite deltas"
            )));
        }
        targets.push((name.clone(), deltas));
    }
    targets.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(targets)
}
