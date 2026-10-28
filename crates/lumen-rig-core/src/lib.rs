//! lumen-rig-core — parser, validator, and motion evaluator
//! for the Lumen native rig format.
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
//! runs and across conforming machines. Physics evaluation
//! (section 5), expression assets (section 4.5), and rendering
//! are later stages and are not present here — stubbed
//! nowhere, simply absent.
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

mod raw;
mod validate;
mod validate_motion;

pub use error::ParseError;
pub use evaluate::{EvaluatedContribution, MotionPlayer, curve_value_at};
pub use model::{
    Anchor, AnchorKind, BlendMode, CombinedParameter, CombinedTerm, Deformer, FORMAT_VERSION,
    Lattice, Mesh, MeshForm, Model, Parameter, Part, Particle, PhysicsGroup, PhysicsInput,
    PhysicsOutput, RotationDeformer, RotationForm, ScalarParameter, Strand, Texture, WarpDeformer,
    WarpForm,
};
pub use motion::{
    ArcKind, BakedCurveSet, BakedProvenance, Contribution, Curve, FathomArc, FathomBinding,
    FathomPhase, Interpolation, Keyframe, Motion, PhaseProvenance, ProvenanceKind,
};
pub use validate::parse_model;
pub use validate_motion::parse_motion;
