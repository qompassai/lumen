//! Shared, versioned scene values. No model, filesystem, renderer or style authority.
//! Time is integral frames; curves are piecewise linear (no overshooting splines).
#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

pub const SCHEMA_VERSION: u32 = 1;
pub const MAX_JSON_BYTES: usize = 1_048_576;
pub const MAX_FRAMES: u32 = 216_000;
pub const MAX_ITEMS: usize = 1024;

/// Labels travel with records from birth (Bloch). The vocabulary is
/// lane-neutral: it must be able to state the truth in both lanes, and
/// no label is ever rewritten or stripped in transit. Admission policy
/// is the consuming lane's own: the SFW runner refuses Nsfw-labelled
/// records; the NSFW instance requires them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContentLabel {
    Sfw,
    Nsfw,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    pub origin_lane: String,
    pub content_label: ContentLabel,
    pub evidence: Vec<String>,
}

impl Provenance {
    pub fn validate(&self) -> Result<(), String> {
        if self.origin_lane.is_empty()
            || self.evidence.is_empty()
            || self.evidence.iter().any(|s| s.is_empty())
        {
            return Err("provenance: origin_lane and evidence required".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Brief {
    pub provenance: Provenance,
    pub text: String,
    pub style_category: String,
}

/// Measurement only: no media paths, bytes, URLs or intake directory capability.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub start_ms: u32,
    pub end_ms: u32,
    pub shot_length_ms: u32,
    pub camera_class: String,
    pub motion_beats: String,
    pub palette: String,
    pub staging: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Analysis {
    pub provenance: Provenance,
    /// Each shot's measurements cite the same explicit source time range.
    pub shots: Vec<Observation>,
}

/// Registry entries are supplied by the instance, never minted by a proposer.
/// Evidence identifies the authoritative canon record; this is not a canonizer.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CharacterRecord {
    pub provenance: Provenance,
    pub canon_id: String,
    pub canon_record_sha256: String,
}

/// Cross-medium plan 35262cd §2.7. Voice is reserved by the shared schema,
/// but refused by Stage 1 authoring: sound is effects/music cues only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PerformanceChannel {
    Eyes,
    Mouth,
    Blush,
    Chest,
    FullBody,
    Staging,
    Voice,
}

/// Named, bounded controls from the instance's performance contract.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelRange {
    pub family: PerformanceChannel,
    pub min: f64,
    pub max: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Registry {
    pub provenance: Provenance,
    pub characters: Vec<CharacterRecord>,
    pub channels: BTreeMap<String, ChannelRange>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Key {
    pub frame: u32,
    pub value: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Curve {
    pub character: String,
    pub channel: String,
    pub keys: Vec<Key>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CameraKey {
    pub frame: u32,
    pub position: [f64; 3],
    pub target: [f64; 3],
    pub focal_length_mm: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Shot {
    pub duration_frames: u32,
    /// Shot-local frame offsets; linear interpolation.
    pub camera: Vec<CameraKey>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CueKind {
    Effect,
    Music,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SoundCue {
    pub kind: CueKind,
    /// Logical asset id only; no file access occurs in stages 0–3.
    pub asset_id: String,
    pub start_frame: u32,
    pub duration_frames: u32,
    pub gain: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneScript {
    pub schema_version: u32,
    pub provenance: Provenance,
    pub style_category: String,
    pub fps: u32,
    pub length_frames: u32,
    pub cast: Vec<String>,
    pub shots: Vec<Shot>,
    /// Scene-local frame offsets.
    pub performance: Vec<Curve>,
    pub sound: Vec<SoundCue>,
}

/// Project canonical JSON v1: UTF-8, recursively sorted object keys, compact,
/// serde_json numeric encoding, array order preserved, no trailing newline.
/// This is a versioned project encoding, not a claim of RFC 8785 compliance.
pub fn canonical<T: Serialize>(value: &T) -> Result<Vec<u8>, String> {
    fn sorted(v: serde_json::Value) -> serde_json::Value {
        match v {
            serde_json::Value::Object(m) => {
                let ordered: BTreeMap<_, _> = m.into_iter().map(|(k, v)| (k, sorted(v))).collect();
                serde_json::Value::Object(ordered.into_iter().collect())
            }
            serde_json::Value::Array(a) => {
                serde_json::Value::Array(a.into_iter().map(sorted).collect())
            }
            other => other,
        }
    }
    // Stop serialization at the cap before constructing a JSON value tree.
    struct BoundedBytes(Vec<u8>);
    impl std::io::Write for BoundedBytes {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if bytes.len() > MAX_JSON_BYTES - self.0.len() {
                return Err(std::io::Error::other("canonical byte cap"));
            }
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut bounded = BoundedBytes(Vec::new());
    serde_json::to_writer(&mut bounded, value).map_err(|e| e.to_string())?;
    let parsed = serde_json::from_slice(&bounded.0).map_err(|e| e.to_string())?;
    serde_json::to_vec(&sorted(parsed)).map_err(|e| e.to_string())
}

pub fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Frozen bytes are opaque: only authoring validation may construct them.
#[derive(Debug, Clone, Serialize)]
pub struct Frozen {
    canonical_bytes: Vec<u8>,
    sha256: String,
}
impl Frozen {
    pub fn bytes(&self) -> &[u8] {
        &self.canonical_bytes
    }
    pub fn hash(&self) -> &str {
        &self.sha256
    }
}

pub fn validate(script: &SceneScript, registry: &Registry) -> Result<(), String> {
    // Cap the entire public typed input too, not only JSON ingress.
    canonical(script)?;
    canonical(registry)?;
    script.provenance.validate()?;
    registry.provenance.validate()?;
    if script.schema_version != SCHEMA_VERSION
        || !(1..=240).contains(&script.fps)
        || !(1..=MAX_FRAMES).contains(&script.length_frames)
    {
        return Err("schema_version, fps or length_frames".into());
    }
    if registry.characters.is_empty()
        || registry.characters.len() > MAX_ITEMS
        || registry.channels.is_empty()
        || registry.channels.len() > MAX_ITEMS
    {
        return Err("registry bounds".into());
    }
    let mut ids = std::collections::BTreeSet::new();
    for character in &registry.characters {
        character.provenance.validate()?;
        if character.canon_id.is_empty()
            || !ids.insert(&character.canon_id)
            || character.canon_record_sha256.len() != 64
            || !character
                .canon_record_sha256
                .bytes()
                .all(|b| b.is_ascii_hexdigit())
        {
            return Err("invalid or duplicate canon record".into());
        }
    }
    for (name, range) in &registry.channels {
        if range.family == PerformanceChannel::Voice
            || name.is_empty()
            || !range.min.is_finite()
            || !range.max.is_finite()
            || range.min > range.max
        {
            return Err("invalid channel range".into());
        }
    }
    if script.cast.is_empty()
        || script.cast.len() > MAX_ITEMS
        || script.shots.is_empty()
        || script.shots.len() > MAX_ITEMS
        || script.performance.len() > MAX_ITEMS
        || script.sound.len() > MAX_ITEMS
    {
        return Err("scene collection bounds".into());
    }
    let mut cast = std::collections::BTreeSet::new();
    for id in &script.cast {
        if !ids.contains(id) || !cast.insert(id) {
            return Err(format!("unknown or duplicate character: {id}"));
        }
    }
    let mut duration = 0u32;
    for shot in &script.shots {
        duration = duration
            .checked_add(shot.duration_frames)
            .ok_or("duration overflow")?;
        if shot.duration_frames == 0 || shot.camera.is_empty() || shot.camera.len() > MAX_ITEMS {
            return Err("shot bounds".into());
        }
        let mut previous = None;
        for key in &shot.camera {
            if key.frame >= shot.duration_frames
                || previous.is_some_and(|f| f >= key.frame)
                || !key
                    .position
                    .iter()
                    .chain(&key.target)
                    .all(|v| v.is_finite())
                || !key.focal_length_mm.is_finite()
                || key.focal_length_mm <= 0.0
            {
                return Err("camera key bounds".into());
            }
            previous = Some(key.frame);
        }
    }
    if duration != script.length_frames {
        return Err("shot duration sum mismatch".into());
    }
    let mut controls = std::collections::BTreeSet::new();
    for curve in &script.performance {
        if !cast.contains(&curve.character) {
            return Err(format!("unknown character: {}", curve.character));
        }
        let range = registry
            .channels
            .get(&curve.channel)
            .ok_or_else(|| format!("unknown channel: {}", curve.channel))?;
        if !controls.insert((&curve.character, &curve.channel))
            || curve.keys.is_empty()
            || curve.keys.len() > MAX_ITEMS
        {
            return Err("duplicate or empty curve".into());
        }
        let mut previous = None;
        for key in &curve.keys {
            if key.frame >= script.length_frames
                || previous.is_some_and(|f| f >= key.frame)
                || !key.value.is_finite()
                || key.value < range.min
                || key.value > range.max
            {
                return Err(format!("curve out of range: {}", curve.channel));
            }
            previous = Some(key.frame);
        }
    }
    for cue in &script.sound {
        if cue.asset_id.is_empty()
            || cue.duration_frames == 0
            || cue
                .start_frame
                .checked_add(cue.duration_frames)
                .is_none_or(|end| end > script.length_frames)
            || !cue.gain.is_finite()
            || !(0.0..=1.0).contains(&cue.gain)
        {
            return Err("sound cue bounds".into());
        }
    }
    Ok(())
}

pub fn freeze(script: &SceneScript, registry: &Registry) -> Result<Frozen, String> {
    validate(script, registry)?;
    // Authoring resolves ids against this exact registry. Seal that context too,
    // so changing a canon-record revision or control range changes the artifact.
    #[derive(Serialize)]
    struct SceneData<'a> {
        scene_script: &'a SceneScript,
        canon_context: &'a Registry,
    }
    let authored = SceneData {
        scene_script: script,
        canon_context: registry,
    };
    let bytes = canonical(&authored)?;
    let twice = canonical(&authored)?;
    if bytes != twice {
        return Err("unstable canonical encoding".into());
    }
    Ok(Frozen {
        sha256: sha256(&bytes),
        canonical_bytes: bytes,
    })
}
