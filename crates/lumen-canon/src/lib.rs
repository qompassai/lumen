//! `canon-record/1` reference implementation — CC-PROGRAM B1 (D-290).
//!
//! Implements the shared, lane-neutral machinery of the canon
//! pipeline SPEC (`~/workspace/canon-pipeline-20261028/SPEC.md`):
//!
//! - [`model`] — the record format (SPEC §3), hash-pinned anchors,
//!   the machine / eye-only invariant split, the append-only keeper
//!   chain;
//! - [`validate`] — structural + invariant validation (SPEC §3, §5);
//! - [`handle`] — the `canon://<lane>/<character>@<version>` handle
//!   protocol with SHA-256 verification and the §2.3 lane check;
//! - [`gate`] — the proportion-band gate: PASS, or the typed refusal
//!   `drift_detected` with a per-metric delta table (SPEC §6). A
//!   floor, not a verdict (§6.1);
//! - [`refusal`] — the typed refusal taxonomy (SPEC §6).
//!
//! This crate is the reference implementation absorbed into lumen at
//! program step B2; it holds no state and serves no tools itself.

pub mod gate;
pub mod handle;
pub mod model;
pub mod refusal;
pub mod validate;

pub use gate::{gate_all, gate_bands, gate_register, GateVerdict, Measurements, EDGE_EPSILON};
pub use handle::{check_lane_access, resolve, CanonHandle};
pub use model::{
    is_sha256_hex, sha256_hex, Anchor, AuthorityRef, CanonRecord, ChainEntry, ChainError,
    ChainParams, CheckMethod, ContentLabel, Invariant, Lane, PaletteColour, ProportionBand,
    StyleFingerprint, FORMAT,
};
pub use refusal::{require_lineage, BandFailure, Refusal};
pub use validate::{validate_record, ValidationIssue};
