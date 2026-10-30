//! lumen-rig-core — parser, validator, motion evaluator,
//! pose/physics runtime, and CPU rasteriser for the Lumen
//! native rig format.
//!
//! The format is specified in `SPEC.md` beside this crate.
//! This crate is the R2 stage of the rig runtime: it parses a
//! model asset (SPEC sections 1–3), validates it completely,
//! and emits it in canonical JSON form (the R1 surface); it
//! parses a motion asset against its model (section 4, 6.1)
//! with the loader-side checks the format demands, and it
//! evaluates motions exactly as sections 4.2 and 4.3 specify
//! — the iteration counts in the cubic solver are part of the
//! format, so evaluated frames are byte-stable across repeat
//! runs and across conforming machines.
//!
//! On top of the R1/R2 surface, the runtime stages land
//! here: pose evaluation (lattice interpolation, warp and
//! rotation deformer application, BIND-1 part translations —
//! [`pose`]), the section 5 Verlet physics simulation
//! ([`physics`]), the [`runtime::Rig`] that keeps parameters,
//! physics, and geometry in step, and RENDER-1, the CPU
//! software rasteriser ([`render`]) that turns a posed model
//! into a PNG frame with no GPU dependency. Expression
//! assets (section 4.5) remain a later stage — absent,
//! not stubbed.
//!
//! Contract summary (SPEC.md sections 7–9):
//! - Parsing is a pure function of the input bytes.
//! - Every invalid or unsupported condition fails with a
//!   typed [`ParseError`] carrying a stable `LRF-` code.
//!   Nothing is silently degraded, defaulted beyond the
//!   spec's resolved defaults, or clamped, except the two
//!   evaluation-time clamps named in SPEC section 9.3 (which
//!   live in the evaluator, not here).
//! - All work is bounded by the limits in [`limits`]; inputs
//!   beyond them fail before or during parsing.
//! - A validated [`Model`] re-emits to canonical JSON and
//!   re-parses to an identical value (round-trip law).
//!
//! The crate is content-free and lane-neutral: it knows the
//! format's identifiers and nothing about any artwork,
//! character, or consumer.

#![forbid(unsafe_code)]

pub mod error;
pub mod evaluate;
pub mod limits;
pub mod model;
pub mod motion;
pub mod physics;
pub mod pose;
pub mod render;
pub mod runtime;

mod raw;
mod validate;
mod validate_motion;

pub use error::ParseError;
pub use evaluate::{EvaluatedContribution, MotionPlayer, curve_value_at};
pub use model::{
    Anchor, AnchorKind, BlendMode, CombinedParameter, CombinedTerm, Deformer, FORMAT_VERSION,
    Lattice, Mesh, MeshForm, Model, Parameter, Part, Particle, PhysicsGroup, PhysicsInput,
    PhysicsOutput, RotationDeformer, RotationForm, ScalarParameter, Strand, Texture,
    TranslationForm, WarpDeformer, WarpForm,
};
pub use motion::{
    ArcKind, BakedCurveSet, BakedProvenance, Contribution, Curve, FathomArc, FathomBinding,
    FathomPhase, Interpolation, Keyframe, Motion, PhaseProvenance, ProvenanceKind,
};
pub use physics::PhysicsSim;
pub use pose::{EvaluatedMesh, ParameterValues, evaluate_pose};
pub use render::{FrameMapping, RenderError, RgbaImage, decode_png, encode_png, render_frame};
pub use runtime::Rig;
pub use validate::parse_model;
pub use validate_motion::parse_motion;
