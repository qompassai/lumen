//! The typed refusal taxonomy (SPEC §6). Payloads are byte-stable:
//! variants carry only sorted, owned data and serialize
//! deterministically, so the same refusal always renders identically.
//!
//! Five variants are the SPEC's own: `drift_detected`,
//! `invariant_violated`, `unanchored_lineage`, `lane_mismatch`,
//! `record_stale`. `malformed_handle` and `malformed_record` are
//! client-error additions of this reference implementation (README,
//! ambiguity A4): the SPEC names no refusal for an unparseable handle
//! or an unparseable record body, and silently succeeding is not an
//! option.

use serde::Serialize;
use std::fmt;

/// One band's measured failure inside a `drift_detected` refusal.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BandFailure {
    pub metric: String,
    pub target: f64,
    pub tolerance: f64,
    /// The measured value, or `None` when the artifact supplied no
    /// measurement for a banded metric (a missing measurement cannot
    /// pass a floor gate).
    pub measured: Option<f64>,
    /// `measured - target`, when a measurement exists.
    pub delta: Option<f64>,
    /// How far beyond the band edge the measurement fell (>= 0).
    /// `None` when there is no measurement to place.
    pub excess: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "refusal", rename_all = "snake_case")]
pub enum Refusal {
    /// The artifact measured outside one or more proportion bands.
    /// Carries the full delta table (SPEC §6), failures sorted by
    /// metric name so payloads are byte-stable.
    DriftDetected { failures: Vec<BandFailure> },
    /// A machine-measured identity invariant was violated. Emitted by
    /// the measurement stack in lumen (SPEC §10 S1); the band gate in
    /// this crate does not measure invariants (README, scope note).
    InvariantViolated { invariant: String, detail: String },
    /// The artifact declares no anchor lineage (SPEC §6).
    UnanchoredLineage,
    /// Lane law (SPEC §2.3): an nsfw (or unclassified) record was
    /// requested by a qompass-lane caller.
    LaneMismatch {
        record_id: String,
        caller_lane: String,
    },
    /// Hash mismatch or version skew against the handle (SPEC §5):
    /// the bytes are not the record the handle names.
    RecordStale { detail: String },
    /// Broker refusal (SPEC §6.1, D-312): a generation job whose
    /// inputs do not include keeper hashes the record requires
    /// (the selected anchor and the style-authority refs). Carries
    /// the missing hashes, sorted.
    MissingKeeperAnchor { missing: Vec<String> },
    /// Broker refusal (SPEC §6.1, PIPELINE §11 rule 10): a job
    /// input presented as an anchor is outside the Owner-adopted
    /// keeper set (record anchors + adopted keeper-chain entries).
    /// Generated outputs never anchor.
    AnchorNotAdopted { sha256: String },
    /// A persisted generation receipt failed hash verification
    /// (ComfyUI intake item 5 receipts are tamper-evident).
    ReceiptTampered { detail: String },
    /// The handle string does not parse (client error; not a SPEC
    /// refusal kind — see module docs).
    MalformedHandle { detail: String },
    /// The record bytes do not parse or do not validate (client
    /// error; not a SPEC refusal kind — see module docs).
    MalformedRecord { detail: String },
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Refusal::DriftDetected { failures } => {
                write!(f, "drift_detected: {} band failure(s)", failures.len())
            }
            Refusal::InvariantViolated { invariant, detail } => {
                write!(f, "invariant_violated: {invariant}: {detail}")
            }
            Refusal::UnanchoredLineage => write!(f, "unanchored_lineage"),
            Refusal::LaneMismatch {
                record_id,
                caller_lane,
            } => write!(f, "lane_mismatch: {caller_lane} may not fetch {record_id}"),
            Refusal::RecordStale { detail } => write!(f, "record_stale: {detail}"),
            Refusal::MissingKeeperAnchor { missing } => {
                write!(
                    f,
                    "missing_keeper_anchor: {} hash(es) absent from job inputs",
                    missing.len()
                )
            }
            Refusal::AnchorNotAdopted { sha256 } => {
                write!(
                    f,
                    "anchor_not_adopted: {sha256} is outside the adopted keeper set"
                )
            }
            Refusal::ReceiptTampered { detail } => write!(f, "receipt_tampered: {detail}"),
            Refusal::MalformedHandle { detail } => write!(f, "malformed_handle: {detail}"),
            Refusal::MalformedRecord { detail } => write!(f, "malformed_record: {detail}"),
        }
    }
}

impl std::error::Error for Refusal {}

/// The lineage check of SPEC §6: an artifact that declares no anchor
/// lineage is refused `unanchored_lineage`. There is no unanchored
/// mode; the absence is the enforcement.
pub fn require_lineage(anchor_role: Option<&str>) -> Result<String, Refusal> {
    match anchor_role {
        Some(role) if !role.trim().is_empty() => Ok(role.to_string()),
        _ => Err(Refusal::UnanchoredLineage),
    }
}
