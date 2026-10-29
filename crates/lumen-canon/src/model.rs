//! Data model for `canon-record/1` (SPEC §3).
//!
//! Every struct denies unknown fields: per SPEC §3 the /1 -> /2 boundary
//! follows the arousal-format precedent — consumers refuse unknown
//! required fields and extensions within /1 are additive-optional only.
//! With `deny_unknown_fields`, an unknown field is a deserialization
//! error rather than something silently dropped.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::str::FromStr;

/// The only format tag this implementation reads or writes.
pub const FORMAT: &str = "canon-record/1";

/// A lane of the estate. Lane strings are the first segment of a record
/// id and of a canon handle (SPEC §3, §5).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Lane {
    Ntranch,
    Qompass,
}

impl Lane {
    pub fn as_str(&self) -> &'static str {
        match self {
            Lane::Ntranch => "ntranch",
            Lane::Qompass => "qompass",
        }
    }
}

impl fmt::Display for Lane {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Lane {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "ntranch" => Ok(Lane::Ntranch),
            "qompass" => Ok(Lane::Qompass),
            other => Err(format!("unknown lane: {other}")),
        }
    }
}

/// Content label carried from birth (SPEC §2.3, bloch §9 algebra).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ContentLabel {
    Sfw,
    Nsfw,
}

/// One canon record per character (SPEC §3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CanonRecord {
    /// Format tag; must equal [`FORMAT`]. (Encoding choice: the SPEC
    /// names the format `canon-record/1` but does not name the field;
    /// see README, ambiguity A1.)
    pub format: String,
    /// Lane-scoped character id, e.g. `ntranch/opal`.
    pub id: String,
    /// Record revision, monotone, >= 1.
    pub version: u64,
    pub origin_lane: Lane,
    /// Absent means *unclassified*, which defaults to nsfw everywhere
    /// it matters (fail closed, SPEC §2.3). Never serialize a guess.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content_label: Option<ContentLabel>,
    pub identity_invariants: Vec<Invariant>,
    pub anchors: Vec<Anchor>,
    pub style_fingerprint: StyleFingerprint,
    pub authority_refs: Vec<AuthorityRef>,
    pub keeper_chain: Vec<ChainEntry>,
}

impl CanonRecord {
    /// The label the lane law acts on: the stored label, or nsfw when
    /// the record is unclassified (SPEC §2.3, fail closed).
    pub fn effective_label(&self) -> ContentLabel {
        self.content_label.unwrap_or(ContentLabel::Nsfw)
    }

    /// The character segment of the id (`opal` in `ntranch/opal`), if
    /// the id is well-formed.
    pub fn character(&self) -> Option<&str> {
        let (lane, character) = self.id.split_once('/')?;
        if lane.is_empty() || character.is_empty() || character.contains('/') {
            return None;
        }
        Some(character)
    }

    /// The lane segment of the id, if the id is well-formed.
    pub fn id_lane(&self) -> Option<Lane> {
        let (lane, _) = self.id.split_once('/')?;
        lane.parse().ok()
    }
}

/// A named identity fact with its check method (SPEC §3): either
/// machine-measured (the method names the measurement) or `eye-only`
/// (the record states the fact; no gate pretends to measure it).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Invariant {
    pub name: String,
    pub fact: String,
    pub check: CheckMethod,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case", deny_unknown_fields)]
pub enum CheckMethod {
    Machine { method: String },
    EyeOnly,
}

/// A governing anchor artifact (SPEC §3). An anchor enters the record
/// only by an adoption act — `adopted_by`, `adopted_at` and
/// `decision_ref` are how the act is recorded, and the validator
/// requires all three.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Anchor {
    /// At minimum one of `primary`, `three-quarter`, `figure`,
    /// `expression-basis` (SPEC §3). Kept as a string so /1 can add
    /// roles additively; the validator requires a `primary`.
    pub role: String,
    pub handle: String,
    pub sha256: String,
    pub adopted_by: String,
    pub adopted_at: String,
    pub decision_ref: String,
}

/// The measured style fingerprint (SPEC §3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StyleFingerprint {
    pub proportion_bands: Vec<ProportionBand>,
    /// Register bands (D-312, PIPELINE §11 rule 7): the register
    /// fingerprint beside the proportion bands. Metrics are data, not
    /// a closed enum — v1 names: ink_colour_distance,
    /// stroke_weight_ratio, shadow_edge_hardness,
    /// saturation_percentile_50, saturation_percentile_95,
    /// contrast_percentile_50, contrast_percentile_95,
    /// key_light_direction_deg, iris_detail_score, blush_coverage.
    /// Additive-optional within /1: absent means no register bands
    /// were ratified for this record yet (never a silent pass in the
    /// lumen broker, which requires them — see lumen::canon).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub register_bands: Vec<ProportionBand>,
    /// Register facts NOT machine-measured in v1 (SPEC §3): stated in
    /// the record, checked by the D-221 rubric/eye path.
    pub rendering_markers: Vec<String>,
    pub palette: Vec<PaletteColour>,
}

/// One proportion band: `{ metric, target, tolerance, method }`
/// (SPEC §3), plus two additive-optional fields the SPEC's own text
/// requires but its field list omits (README, ambiguity A2):
/// `cap` states an in-costume cap where costume caps the metric, and
/// `floor_only` marks a band that binds only from below (the bust
/// footprint band is a floor, not a window).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProportionBand {
    pub metric: String,
    pub target: f64,
    pub tolerance: f64,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cap: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub floor_only: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// One governing palette colour with a per-colour tolerance (SPEC §3).
/// Tolerance is a fraction in [0, 1] applied per channel by the
/// consuming grade machinery (lumen); this crate validates the shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PaletteColour {
    pub role: String,
    /// `#RRGGBB`.
    pub hex: String,
    pub tolerance: f64,
}

/// A hash-pinned member of the style-authority set the fingerprint
/// was authored from (SPEC §3).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AuthorityRef {
    pub handle: String,
    pub sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// One append-only keeper-chain entry (SPEC §3). `adopted` flips only
/// by the adoption act, never by a gate (SPEC §6.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChainEntry {
    pub artifact: String,
    pub sha256: String,
    pub backend: String,
    pub params: ChainParams,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot_id: Option<String>,
    pub record_version: u64,
    /// Optional because chain heads at record creation are landed
    /// keepers that predate the gate (SPEC §9); see README, A3.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gate_verdict_ref: Option<String>,
    pub adopted: bool,
}

/// Generation parameters recorded per chain entry (SPEC §3, §12):
/// hosted backends disclose what they provide; every field is
/// optional, never silently mixed.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChainParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub denoise: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steps: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cfg: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_hash: Option<String>,
}

/// Why a chain append was refused (append-only discipline, SPEC §3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChainError {
    /// Entries are written against the record's current version;
    /// anything else is version skew.
    VersionSkew { entry: u64, record: u64 },
    /// The entry's artifact hash is not a SHA-256 hex digest.
    BadHash(String),
}

impl fmt::Display for ChainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ChainError::VersionSkew { entry, record } => {
                write!(
                    f,
                    "chain entry record_version {entry} != record version {record}"
                )
            }
            ChainError::BadHash(h) => write!(f, "chain entry sha256 is not a hex digest: {h}"),
        }
    }
}

impl std::error::Error for ChainError {}

impl CanonRecord {
    /// Append a keeper-chain entry. The chain is append-only: this is
    /// the only mutation the reference implementation offers, and it
    /// refuses version skew and malformed hashes.
    pub fn append_chain_entry(&mut self, entry: ChainEntry) -> Result<(), ChainError> {
        if entry.record_version != self.version {
            return Err(ChainError::VersionSkew {
                entry: entry.record_version,
                record: self.version,
            });
        }
        if !is_sha256_hex(&entry.sha256) {
            return Err(ChainError::BadHash(entry.sha256.clone()));
        }
        self.keeper_chain.push(entry);
        Ok(())
    }
}

/// True when `s` is a 64-character lowercase hex SHA-256 digest.
pub fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// SHA-256 of `bytes`, lowercase hex. The handle protocol pins
/// records by this digest (SPEC §5).
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    let digest = sha2::Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}
