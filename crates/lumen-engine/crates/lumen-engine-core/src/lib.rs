#![forbid(unsafe_code)]
//! lumen-engine-core: the deterministic runtime core of
//! the lumen engine experiment (D-893 spec, D-895 adopted
//! position).
//!
//! The core is the part of the engine that is a pure
//! function of its records: a fixed-step clock, the
//! scene/state/dialogue/effect/audio record family,
//! keyframe and placement track sampling, the two-body
//! contact seam, and the sub-runtimes that step dialogue,
//! effect timelines, and audio bindings in lockstep. It
//! renders nothing and touches no I/O beyond parsing
//! record text a host hands it. Hosts (lumen-engine-bevy)
//! bind it to the evaluator of record (lumen-rig-core,
//! consumed exactly as the games do today — DP-8) and
//! to a renderer.
//!
//! Determinism contract: no wall clock, no map iteration
//! over unordered maps, no platform intrinsics beyond
//! IEEE arithmetic in a fixed order. A recorded step
//! sequence replays byte-identically across processes.

pub mod audio;
pub mod clock;
pub mod contact;
pub mod dialogue;
pub mod effect;
pub mod error;
pub mod expr;
pub mod record;
pub mod runtime;
pub mod state;
pub mod track;

pub use audio::{AudioBindings, AudioEvent, BUSES};
pub use clock::FixedClock;
pub use contact::{
    endpoint_world, evaluate_frame, evaluate_frame_with_geometry, evaluate_model,
    resolve_parameters, ContactDef, ContactEndpoint, ContactRead, EvaluatedMesh, FrameInput,
    FrameRead, Placement, ProbeDef, Scene, TwoBodyError, CONTACTS_MAX, PROBES_MAX, RIGS_MAX,
};
pub use dialogue::{DialogueEvent, DialogueRuntime};
pub use effect::{particle_position, splitmix64, EffectEvent, EffectPlayer, FLASH_MAX_PER_SECOND};
pub use error::EngineError;
pub use expr::{NameSource, ValueExpr};
pub use record::{
    ActorRecord, AudioBindingRecord, CanvasRecord, ContactRecord, DialogueLineRecord,
    DialogueRecord, EffectKeyRecord, EffectKind, EffectRecord, KeyFrameRecord, PlacementTrack,
    PoseFamilyRecord, PoseKeyRecord, SceneRecord, TuneExpected, TuneRecord, SCENE_RECORD_VERSION,
};
pub use runtime::{Runtime, StepEvents};
pub use state::{fnv1a64, StateRecord, STATE_RECORD_VERSION};
pub use track::{
    resolve_pose_track, sample_pivot_turn, sample_placement, sample_pose_track, sample_step_in_out,
    smoothstep_f32, smoothstep_f64, ResolvedPoseKey,
};
