//! The validated motion: the only shape the evaluator consumes.
//!
//! A `Motion` value is proof of validation — every invariant in
//! SPEC.md section 4 has been checked against the target model
//! by `crate::validate_motion`. Fields the format makes
//! optional-with-default are stored resolved (for example
//! `fade_in_seconds` is always present here), so evaluation
//! never re-applies defaults.

use serde::Serialize;

use crate::model::FORMAT_VERSION;

/// A validated lumen rig motion (format `lumen-rig-motion`, v1).
#[derive(Debug, Clone, PartialEq)]
pub struct Motion {
    pub id: String,
    /// The id of the model this motion targets. Binding is
    /// enforced at load (`LRF-203`): a motion only ever loads
    /// against the model it names.
    pub model: String,
    pub duration_seconds: f32,
    pub looping: bool,
    pub frames_per_second: u32,
    pub curves: Vec<Curve>,
    /// Baked audio curve sets (SPEC.md section 6.1). Evaluation
    /// treats each set exactly as a curve; the provenance rides
    /// along as data and is never interpreted by the evaluator.
    pub baked_curves: Vec<BakedCurveSet>,
    pub fathom: Option<FathomBinding>,
}

/// One parameter curve (SPEC.md section 4.1).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Curve {
    pub parameter: String,
    pub fade_in_seconds: f32,
    pub fade_out_seconds: f32,
    pub keyframes: Vec<Keyframe>,
}

/// A baked audio curve set: an ordinary curve plus the
/// provenance of the analysis that produced it (section 6.1).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BakedCurveSet {
    pub parameter: String,
    pub fade_in_seconds: f32,
    pub fade_out_seconds: f32,
    pub keyframes: Vec<Keyframe>,
    pub provenance: BakedProvenance,
}

/// Provenance of a baked curve set. For `Manual` sets the three
/// analysis fields are absent (stored `None`); for `Envelope`
/// and `Onset` sets all three are present and
/// `source_audio_sha256` names the analyzed audio by hash. The
/// format carries the hash, never the audio.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BakedProvenance {
    pub kind: ProvenanceKind,
    pub tool: String,
    pub tool_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_audio_sha256: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_seconds: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hop_seconds: Option<f32>,
}

/// The kind of analysis that produced a baked curve set.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProvenanceKind {
    Envelope,
    Onset,
    Manual,
}

impl ProvenanceKind {
    /// Map a format string to a provenance kind. Returns `None`
    /// for anything outside the section-6.1 set — the validator
    /// turns that into a schema error, never a substitution.
    pub fn from_format_str(text: &str) -> Option<ProvenanceKind> {
        let kind = match text {
            "envelope" => ProvenanceKind::Envelope,
            "onset" => ProvenanceKind::Onset,
            "manual" => ProvenanceKind::Manual,
            _ => return None,
        };
        Some(kind)
    }
}

/// One keyframe of a curve (SPEC.md section 4.1). Tangents are
/// present only on cubic keys; the first key carries no
/// `tangent_in` and the last no `tangent_out` by convention —
/// the evaluator reads `tangent_out` of key `i` and
/// `tangent_in` of key `i + 1` per segment (section 4.2).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Keyframe {
    pub time: f32,
    pub value: f32,
    pub interpolation: Interpolation,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tangent_in: Option<[f32; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tangent_out: Option<[f32; 2]>,
}

/// The interpolation mode of the segment STARTING at a key
/// (SPEC.md section 4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Interpolation {
    Linear,
    Stepped,
    Cubic,
}

impl Interpolation {
    /// Map a format string to an interpolation mode. Returns
    /// `None` for anything outside the section-4.1 set — the
    /// validator turns that into a schema error, never a
    /// substitution (the inox2d "unknown interpolate mode"
    /// failure is the anti-spec here).
    pub fn from_format_str(text: &str) -> Option<Interpolation> {
        let mode = match text {
            "linear" => Interpolation::Linear,
            "stepped" => Interpolation::Stepped,
            "cubic" => Interpolation::Cubic,
            _ => return None,
        };
        Some(mode)
    }
}

/// The fathom alignment block (SPEC.md section 4.4). The phase
/// labels and arc vocabulary are fathom's
/// (`fathom-motion-record/3`), carried verbatim; this format
/// adds no phase semantics of its own.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FathomBinding {
    pub schema: String,
    pub phases: Vec<FathomPhase>,
    pub arc: FathomArc,
}

/// One fathom phase: frame `f` covers the time interval
/// `[f / fps, (f + 1) / fps)` at the motion's
/// `frames_per_second`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FathomPhase {
    pub label: String,
    pub start_frame: u32,
    pub end_frame: u32,
    pub provenance: PhaseProvenance,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// Where a phase's label comes from (fathom vocabulary).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum PhaseProvenance {
    Measured,
    AssertedFromScenario,
    AssertedByOwnerReview,
}

impl PhaseProvenance {
    /// Map a format string to a phase provenance. Returns
    /// `None` for anything outside the section-4.4 set.
    pub fn from_format_str(text: &str) -> Option<PhaseProvenance> {
        let provenance = match text {
            "measured" => PhaseProvenance::Measured,
            "asserted-from-scenario" => PhaseProvenance::AssertedFromScenario,
            "asserted-by-owner-review" => PhaseProvenance::AssertedByOwnerReview,
            _ => return None,
        };
        Some(provenance)
    }
}

/// The fathom arc descriptor.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FathomArc {
    pub kind: ArcKind,
}

/// The shape of a fathom arc (fathom vocabulary).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ArcKind {
    Progression,
    Cycle,
    SegmentLibrary,
    Excerpt,
    SingleFrame,
    None,
}

impl ArcKind {
    /// Map a format string to an arc kind. Returns `None` for
    /// anything outside the section-4.4 set.
    pub fn from_format_str(text: &str) -> Option<ArcKind> {
        let kind = match text {
            "progression" => ArcKind::Progression,
            "cycle" => ArcKind::Cycle,
            "segment_library" => ArcKind::SegmentLibrary,
            "excerpt" => ArcKind::Excerpt,
            "single_frame" => ArcKind::SingleFrame,
            "none" => ArcKind::None,
            _ => return None,
        };
        Some(kind)
    }
}

/// Curves view: the plain curves followed by the baked sets,
/// in document order. Evaluation order is part of determinism
/// (SPEC.md section 7.3), so this order is contractual.
impl Motion {
    /// Every contribution the motion makes, in evaluation
    /// order: plain curves first, then baked audio sets.
    pub fn contributions(&self) -> Vec<Contribution<'_>> {
        let mut out: Vec<Contribution<'_>> =
            Vec::with_capacity(self.curves.len() + self.baked_curves.len());
        for curve in &self.curves {
            out.push(Contribution {
                parameter: &curve.parameter,
                fade_in_seconds: curve.fade_in_seconds,
                fade_out_seconds: curve.fade_out_seconds,
                keyframes: &curve.keyframes,
            });
        }
        for baked in &self.baked_curves {
            out.push(Contribution {
                parameter: &baked.parameter,
                fade_in_seconds: baked.fade_in_seconds,
                fade_out_seconds: baked.fade_out_seconds,
                keyframes: &baked.keyframes,
            });
        }
        out
    }

    /// Serialize to the canonical text form (SPEC.md
    /// section 1.2): spec field order, 2-space indent,
    /// shortest-round-trip floats, one trailing newline. A pure
    /// function of the motion: `motion_to_json` output re-parses
    /// (against the same model) to an identical motion and
    /// re-emits byte-identically.
    pub fn to_canonical_json(&self) -> String {
        let audio = if self.baked_curves.is_empty() {
            None
        } else {
            Some(AudioOut {
                baked: &self.baked_curves,
            })
        };
        let out = MotionOut {
            format: "lumen-rig-motion",
            format_version: FORMAT_VERSION,
            id: &self.id,
            model: &self.model,
            duration_seconds: self.duration_seconds,
            loop_field: self.looping,
            frames_per_second: self.frames_per_second,
            curves: &self.curves,
            audio,
            fathom: self.fathom.as_ref(),
        };
        // Serializing this plain-data view cannot fail: every
        // type is a struct/enum of strings, numbers, bools, and
        // vecs with derived Serialize and no custom
        // implementations.
        let mut text = serde_json::to_string_pretty(&out)
            .expect("serializing a validated motion is infallible");
        text.push('\n');
        text
    }
}

/// A borrowed view of one curve-shaped contribution (a plain
/// curve or a baked audio set) — the evaluator's unit of work.
#[derive(Debug, Clone, Copy)]
pub struct Contribution<'a> {
    pub parameter: &'a str,
    pub fade_in_seconds: f32,
    pub fade_out_seconds: f32,
    pub keyframes: &'a [Keyframe],
}

/// Serialization view for the motion's `audio` object.
#[derive(Serialize)]
struct AudioOut<'a> {
    baked: &'a [BakedCurveSet],
}

/// Serialization view emitting the motion with the format
/// header fields first, in canonical order (section 1.2).
#[derive(Serialize)]
struct MotionOut<'a> {
    format: &'static str,
    format_version: u32,
    id: &'a str,
    model: &'a str,
    duration_seconds: f32,
    #[serde(rename = "loop")]
    loop_field: bool,
    frames_per_second: u32,
    curves: &'a [Curve],
    #[serde(skip_serializing_if = "Option::is_none")]
    audio: Option<AudioOut<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    fathom: Option<&'a FathomBinding>,
}
