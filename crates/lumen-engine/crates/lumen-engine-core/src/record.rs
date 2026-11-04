//! The engine's record family (SPEC section 4.1): text
//! records, strictly parsed, that fully declare a scene —
//! its actors and rig references, pose and placement
//! tracks, contacts, the grasp-tune contract, and the
//! dialogue / effect / audio material the runtime steps
//! alongside. Records are data: they name rig import
//! directories relative to a host-supplied base and carry
//! no code, no paths outside that base, and no behaviour
//! beyond what these shapes declare.
//!
//! Every struct refuses unknown fields, so a record that
//! drifts from the format fails at load, not mid-run.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::EngineError;
use crate::expr::ValueExpr;

/// The only scene-record version this build reads.
pub const SCENE_RECORD_VERSION: u32 = 1;

/// A full scene declaration.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SceneRecord {
    /// Record format version; must equal
    /// [`SCENE_RECORD_VERSION`].
    pub version: u32,
    /// Scene identifier; state records bind to it.
    pub id: String,
    /// Fixed steps the scene runs.
    pub steps: u64,
    /// Fixed-step rate (steps per second).
    pub rate: u32,
    /// Settle steps each rig's evaluator runs before step 0
    /// (spring-state warm-up, the staging discipline the
    /// evaluator seam's own scenes use).
    pub settle_steps: u32,
    /// Actors in rig order (contact and root indices refer
    /// to this order).
    pub actors: Vec<ActorRecord>,
    /// Declared contacts between actor parts.
    pub contacts: Vec<ContactRecord>,
    /// Per-actor root part names (the contact seam's root
    /// reads), parallel to `actors`.
    pub roots: Vec<String>,
    /// Raster canvas for hosts that render.
    pub canvas: CanvasRecord,
    /// Steps the host renders as key frames.
    pub key_frames: Vec<KeyFrameRecord>,
    /// The grasp-tune contract, when the scene tunes its
    /// contacts against a measured band.
    pub tune: Option<TuneRecord>,
    /// Dialogue stepped by the runtime alongside the scene.
    pub dialogue: Option<DialogueRecord>,
    /// Effect timelines stepped by the runtime.
    pub effects: Vec<EffectRecord>,
    /// Audio bindings evaluated by the runtime (cue events
    /// only; the core never plays audio).
    pub audio: Vec<AudioBindingRecord>,
}

impl SceneRecord {
    /// Parse a scene record from RON text, then validate
    /// its shape. Unknown fields, wrong versions, and
    /// malformed windows are typed refusals.
    pub fn load_ron(text: &str) -> Result<Self, EngineError> {
        let record: SceneRecord =
            ron::de::from_str(text).map_err(|e| EngineError::RecordParse(e.to_string()))?;
        record.validate()?;
        Ok(record)
    }

    /// Shape checks that parsing alone does not prove:
    /// version, non-empty actors, roots parallel to actors,
    /// contact indices in range, sorted track keys, sane
    /// windows.
    pub fn validate(&self) -> Result<(), EngineError> {
        if self.version != SCENE_RECORD_VERSION {
            return Err(EngineError::RecordVersion {
                kind: "scene",
                found: self.version,
            });
        }
        if self.actors.is_empty() {
            return Err(EngineError::TrackShape("scene has no actors".into()));
        }
        if self.roots.len() != self.actors.len() {
            return Err(EngineError::TrackShape(format!(
                "roots ({}) must parallel actors ({})",
                self.roots.len(),
                self.actors.len()
            )));
        }
        if self.rate == 0 || self.steps == 0 {
            return Err(EngineError::TrackShape(
                "scene rate and steps must be positive".into(),
            ));
        }
        for actor in &self.actors {
            actor.validate()?;
        }
        for contact in &self.contacts {
            for (label, index) in [
                ("a_actor", contact.a_actor),
                ("b_actor", contact.b_actor),
                ("driven_actor", contact.driven_actor),
            ] {
                if index >= self.actors.len() {
                    return Err(EngineError::TrackShape(format!(
                        "contact {}: {label} {index} out of range",
                        contact.id
                    )));
                }
            }
            if contact.active_to <= contact.active_from {
                return Err(EngineError::TrackShape(format!(
                    "contact {}: active window is empty or inverted",
                    contact.id
                )));
            }
        }
        for frame in &self.key_frames {
            if frame.step >= self.steps {
                return Err(EngineError::TrackShape(format!(
                    "key frame '{}' at step {} is outside the scene",
                    frame.label, frame.step
                )));
            }
        }
        if let Some(tune) = &self.tune {
            tune.validate()?;
        }
        Ok(())
    }
}

/// One actor: a rig import (directory name relative to the
/// host's imports base), its pose track, and its authored
/// placement track.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ActorRecord {
    /// Actor identifier (record-local).
    pub id: String,
    /// Display name used in reports.
    pub display: String,
    /// Rig import directory, relative to the host's base.
    pub rig: String,
    /// Pose keys over scene time (seconds), sorted by `t`.
    pub pose_track: Vec<PoseKeyRecord>,
    /// Authored placement over scene time.
    pub placement: PlacementTrack,
}

impl ActorRecord {
    fn validate(&self) -> Result<(), EngineError> {
        if self.pose_track.is_empty() {
            return Err(EngineError::TrackShape(format!(
                "actor {}: pose track is empty",
                self.id
            )));
        }
        for pair in self.pose_track.windows(2) {
            if pair[1].t < pair[0].t {
                return Err(EngineError::TrackShape(format!(
                    "actor {}: pose keys are not sorted by time",
                    self.id
                )));
            }
        }
        if let PlacementTrack::PivotTurn { keys, .. } = &self.placement {
            if keys.len() < 2 {
                return Err(EngineError::TrackShape(format!(
                    "actor {}: pivot-turn needs at least two keys",
                    self.id
                )));
            }
            for pair in keys.windows(2) {
                if pair[1].0 < pair[0].0 {
                    return Err(EngineError::TrackShape(format!(
                        "actor {}: pivot-turn keys are not sorted by time",
                        self.id
                    )));
                }
            }
        }
        if let PlacementTrack::StepInOut {
            step_in_window,
            step_back_window,
            ..
        } = &self.placement
        {
            for (label, window) in [("step-in", step_in_window), ("step-back", step_back_window)] {
                if window.1 <= window.0 {
                    return Err(EngineError::TrackShape(format!(
                        "actor {}: {label} window is empty or inverted",
                        self.id
                    )));
                }
            }
        }
        Ok(())
    }
}

/// One pose key: parameter values at time `t` (seconds).
/// Parameters a key does not list hold their neighbouring
/// keys' interpolated value, or zero before the first key
/// that names them — the pose convention of the evaluator
/// seam's scenes.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PoseKeyRecord {
    /// Key time in seconds.
    pub t: f64,
    /// Parameter name -> value form.
    pub params: BTreeMap<String, ValueExpr>,
}

/// An authored placement track. Pivot and ground references
/// are NOT record data: the host measures the actor's foot
/// centre and ground lift from the loaded rig (the staging
/// discipline of the seam's scenes), and the track supplies
/// only the motion over them.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub enum PlacementTrack {
    /// A fixed translation, no rotation.
    Fixed {
        /// Scene-space x.
        x: f64,
        /// Scene-space y (ground-relative).
        y: f64,
    },
    /// Rotation keys (time seconds, degrees) interpolated
    /// linearly, pivoting about the measured foot centre,
    /// with a compensation lift applied pro-rata with
    /// |sin| of the current rotation against the track's
    /// peak rotation.
    PivotTurn {
        /// (time, degrees) keys, sorted by time.
        keys: Vec<(f64, f64)>,
        /// Peak compensation lift (a value form; the tune's
        /// `turn_lift` output for tuned scenes).
        compensation: ValueExpr,
    },
    /// A step-in / hold / step-back approach: the actor
    /// starts `step_in` units beyond `base_x`, eases to
    /// `base_x` over the step-in window (smoothstep), holds,
    /// then eases `step_back` units further over the
    /// step-back window.
    StepInOut {
        /// Held x while contacts are live (a value form;
        /// the tune's `b_x` output for tuned scenes).
        base_x: ValueExpr,
        /// Approach distance at time zero.
        step_in: f64,
        /// (start, end) seconds of the approach ease.
        step_in_window: (f64, f64),
        /// Release distance added after the hold.
        step_back: f64,
        /// (start, end) seconds of the release ease.
        step_back_window: (f64, f64),
    },
}

/// One declared contact between two actor parts, active
/// over a time window. Its world offset is tune-derived
/// (see [`TuneRecord`]) or zero for untuned scenes: the
/// record declares the attachment, the tune anchors it.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ContactRecord {
    /// Contact identifier (stable across runs).
    pub id: String,
    /// Actor index of the `a` endpoint.
    pub a_actor: usize,
    /// Part name of the `a` endpoint (centroid-bound).
    pub a_part: String,
    /// Actor index of the `b` endpoint.
    pub b_actor: usize,
    /// Part name of the `b` endpoint (centroid-bound).
    pub b_part: String,
    /// The rig the solver moves to honour the contact.
    pub driven_actor: usize,
    /// Window start (seconds, inclusive).
    pub active_from: f64,
    /// Window end (seconds, exclusive).
    pub active_to: f64,
}

/// The raster canvas a host renders into.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CanvasRecord {
    /// Canvas width in pixels.
    pub width: u32,
    /// Canvas height in pixels.
    pub height: u32,
    /// Puppet units per pixel scale factor.
    pub scale: f32,
}

/// A step the host renders as a labelled key frame.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct KeyFrameRecord {
    /// Step index.
    pub step: u64,
    /// Frame label (file naming and reports).
    pub label: String,
}

/// The grasp-tune contract: scan a measured band on the
/// driven actor's band part for hand targets minimizing the
/// rigid residual, subject to the declared bars, then set
/// the crouch depth under its drop cap. `expected` pins
/// the tune's outputs as gate values: a host whose tune
/// diverges from them reports the divergence as a gate
/// failure rather than silently staging a different scene
/// (the numbers are the D-875 reads of record for the
/// standing two-body scene).
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TuneRecord {
    /// Actor index whose band part is scanned (the driven
    /// actor).
    pub band_actor: usize,
    /// Part carrying the band (its rest AABB is scanned).
    pub band_part: String,
    /// Actor index whose hand pair is tuned (the lifter).
    pub hand_actor: usize,
    /// Hand part names, in contact order.
    pub hand_parts: (String, String),
    /// Band edge nearest the part's top, as a fraction of
    /// the part's height measured down from its top.
    pub band_near: f64,
    /// Band edge farthest from the part's top (same
    /// convention); rows scan from the far edge to the
    /// near edge in 1-unit steps.
    pub band_far: f64,
    /// Grasp parameter candidates, scanned in order.
    pub g_candidates: Vec<f64>,
    /// Feasibility bar: predicted hold clearance must
    /// reach this many units for a candidate to be
    /// feasible (infeasible scans fall back to the
    /// minimum-residual candidate overall).
    pub clearance_bar: f64,
    /// Feasibility bar: the grasp must engage at or above
    /// this height relative to ground level.
    pub engage_min: f64,
    /// Crouch hand-drop cap: the crouch depth shrinks
    /// until the crouch-window dip stays inside it.
    pub crouch_drop_cap: f64,
    /// Crouch depth scanned first (and the depth the drop
    /// is measured at).
    pub default_crouch: f64,
    /// The pose family the tune parameterizes (grasp /
    /// crouch / lift / setdown maps over the tune names
    /// `g` and `cd`).
    pub family: PoseFamilyRecord,
    /// The tune outputs of record, as gate values.
    pub expected: TuneExpected,
}

impl TuneRecord {
    fn validate(&self) -> Result<(), EngineError> {
        if self.g_candidates.is_empty() {
            return Err(EngineError::TrackShape("tune: no grasp candidates".into()));
        }
        if !(0.0..=1.0).contains(&self.band_near) || !(0.0..=1.0).contains(&self.band_far) {
            return Err(EngineError::TrackShape(
                "tune: band fractions must lie in [0, 1]".into(),
            ));
        }
        Ok(())
    }
}

/// The tune's pose family and the pose-track key times at
/// which each family pose sits for the hand actor. The host
/// verifies the actor's track keys at those times resolve
/// to exactly these maps, so the record carries one source
/// of truth for the poses.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PoseFamilyRecord {
    /// Grasp pose (names: `g`).
    pub grasp: BTreeMap<String, ValueExpr>,
    /// Crouch pose (names: `g`, `cd`).
    pub crouch: BTreeMap<String, ValueExpr>,
    /// Lift pose (names: `g`, `cd`).
    pub lift: BTreeMap<String, ValueExpr>,
    /// Setdown pose (names: `g`, `cd`).
    pub setdown: BTreeMap<String, ValueExpr>,
    /// Pose-track key time of the grasp pose.
    pub grasp_at: f64,
    /// Pose-track key time of the crouch pose.
    pub crouch_at: f64,
    /// Pose-track key time of the lift pose.
    pub lift_at: f64,
    /// Pose-track key time of the setdown pose.
    pub setdown_at: f64,
}

/// Tune outputs pinned as gate values.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TuneExpected {
    /// The driven actor's held x.
    pub b_x: f64,
    /// The chosen band row (driven-actor local y).
    pub target_row_y: f64,
    /// Hand target for the first contact (local).
    pub target_left: [f64; 2],
    /// Hand target for the second contact (local).
    pub target_right: [f64; 2],
    /// Predicted rigid residual of the chosen candidate.
    pub predicted_slip: f64,
    /// Per-contact pre-solve slip at the grasp.
    pub pre_solve_slip: [f64; 2],
    /// Chosen grasp parameter value.
    pub grasp_arm_l: f64,
    /// Chosen crouch depth.
    pub crouch_depth: f64,
    /// Measured turn compensation lift.
    pub turn_lift: f64,
    /// Measured crouch hand drop.
    pub crouch_drop: f64,
    /// Measured hand rise from grasp to lift.
    pub hand_rise: f64,
    /// Predicted hold clearance of the chosen candidate.
    pub predicted_clearance: f64,
    /// The band box (driven-actor local):
    /// [min_x, min_y, max_x, max_y].
    pub waist_band: [f64; 4],
    /// Per-contact world offsets the tune produced.
    pub contact_offsets: [[f64; 2]; 2],
}

/// A dialogue sequence stepped by the runtime: lines type
/// out at a fixed character pace (the kinetic-UI
/// discipline: text timing is part of the scene, not the
/// renderer).
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DialogueRecord {
    /// Step at which the first line starts.
    pub start_step: u64,
    /// Characters revealed per step while a line types.
    pub chars_per_step: u32,
    /// Steps a completed line holds before the next starts.
    pub hold_steps: u64,
    /// The lines, in order.
    pub lines: Vec<DialogueLineRecord>,
}

/// One dialogue line.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct DialogueLineRecord {
    /// Speaker name.
    pub speaker: String,
    /// Line text (English, per the production rule).
    pub text: String,
}

/// One effect timeline: keys on the scene clock, each
/// spawning a deterministic particle instance. Kinds are
/// the effect-timeline/1 vocabulary.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EffectRecord {
    /// Timeline identifier.
    pub id: String,
    /// Step at which the timeline's local clock starts.
    pub start_step: u64,
    /// Steps a spawned instance lives.
    pub life_steps: u64,
    /// Spawn keys (local steps from `start_step`).
    pub keys: Vec<EffectKeyRecord>,
}

/// One effect spawn key.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct EffectKeyRecord {
    /// Local step at which the key fires.
    pub at_step: u64,
    /// Particle pattern.
    pub kind: EffectKind,
    /// Emitter x (scene units).
    pub x: f64,
    /// Emitter y (scene units).
    pub y: f64,
    /// Particles in the instance.
    pub count: u32,
    /// Instance seed (counter-based randomness; the
    /// instance's particles are a pure function of this
    /// seed, the particle index, and the instance age).
    pub seed: u64,
    /// Whether this key is a strobe (full-frame flash)
    /// event for the flash law.
    pub strobe: bool,
}

/// Effect particle patterns (effect-timeline/1 kinds).
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub enum EffectKind {
    /// Radial burst.
    Burst,
    /// Expanding ring.
    Ring,
    /// Spiral emission.
    Spiral,
}

/// One audio binding: a cue the runtime fires as an event
/// at a step, or repeatedly inside a window. Evaluation
/// only — the core emits cue events; hosts own playback
/// (and the estate's audio-silence discipline governs
/// testing).
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AudioBindingRecord {
    /// Binding identifier.
    pub id: String,
    /// Cue name fired.
    pub cue: String,
    /// Bus the cue belongs to ("music" | "sfx" | "voice").
    pub bus: String,
    /// Evaluated gain in dB carried on the event.
    pub gain_db: f64,
    /// One-shot fire step, if any.
    pub at_step: Option<u64>,
    /// Repeating window (first step, last step inclusive,
    /// period in steps), if any.
    pub window: Option<(u64, u64, u64)>,
}
