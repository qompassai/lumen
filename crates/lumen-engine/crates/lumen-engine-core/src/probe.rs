//! The contact-pair probe (ActLab Tool 2, D-943): a data
//! record that declares two rigs, anchor pairs between
//! them, and a pose timeline, plus the pure machinery the
//! host runner builds on — strict parsing, validator-style
//! parameter legality against the rigs' own parameter
//! tables, band-box geometry, and report aggregation.
//!
//! Grammar discipline (the Tool 1 act-record grammar,
//! carried at the data level — no code is shared with the
//! torogao tree): one root named-field struct, named
//! fields only, exact field sets (unknown fields refused
//! at parse), parameter sets by name with plain numeric
//! values, and legality of every named parameter judged
//! against the target rig's own parameter data (unknown
//! name and out-of-range value are typed refusals issued
//! before any evaluation runs).
//!
//! Anchor addressing: an anchor is `(rig, selector)` where
//! the selector names evaluated geometry the rig records
//! expose — a part's vertex centroid, or one vertex of one
//! mesh by index. The selectors mirror the contact seam's
//! own endpoint forms ([`crate::contact::ContactEndpoint`])
//! so a pair and a contact can name the same point with the
//! same words.
//!
//! Measures are labelled per pair by capability, never
//! implied: `distance` (endpoint separation, or the seam
//! slip of the bound contact), `band_distance` (distance
//! from the A point to a declared band box on B's part;
//! zero while the point is inside the box — the D-859
//! acceptance measure), and `penetration` (signed margin
//! of the A point against B's named part triangles;
//! negative means the point is inside the part's
//! silhouette). A pair reports only the measures it
//! declares; a point anchor has no volume, so no pair in
//! this build claims a mesh-overlap volume — that is the
//! future mesh-interpenetration audit's ground.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::error::EngineError;

/// The only probe-spec version this build reads.
pub const PROBE_SPEC_VERSION: u32 = 1;
/// Maximum rigs in one probe spec.
pub const PROBE_RIGS_MAX: usize = 8;
/// Maximum declared pairs in one probe spec.
pub const PROBE_PAIRS_MAX: usize = 64;
/// Maximum contacts in one probe spec.
pub const PROBE_CONTACTS_MAX: usize = 64;
/// Maximum steps in one probe run.
pub const PROBE_STEPS_MAX: u64 = 1_000_000;
/// Maximum pose keys per rig track.
pub const PROBE_KEYS_MAX: usize = 4096;

/// A probe failure at the spec or validation layer. The
/// `LE-110` code is carried through [`EngineError::Probe`];
/// this struct keeps the structured detail (which rig,
/// which parameter) for hosts that render their own
/// diagnostics.
#[derive(Clone, Debug, PartialEq)]
pub struct ProbeRefusal {
    /// Human-readable detail naming the refusing field.
    pub message: String,
}

impl From<ProbeRefusal> for EngineError {
    fn from(refusal: ProbeRefusal) -> Self {
        EngineError::Probe(refusal.message)
    }
}

fn refuse<T>(message: impl Into<String>) -> Result<T, EngineError> {
    Err(EngineError::Probe(message.into()))
}

/// One rig's parameter table, as read from the rig record
/// itself (the lumen-side analogue of Tool 1's rig
/// snapshot): the legality source for timeline parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct RigParameterTable {
    /// Rig identifier (matches `ProbeRig::id`).
    pub rig_id: String,
    /// `(name, min, max)` per scalar parameter, in record
    /// order.
    pub parameters: Vec<(String, f32, f32)>,
}

impl RigParameterTable {
    /// Look up one parameter's declared range.
    pub fn range_of(&self, name: &str) -> Option<(f32, f32)> {
        self.parameters
            .iter()
            .find(|(n, _, _)| n == name)
            .map(|(_, lo, hi)| (*lo, *hi))
    }
}

/// A point selector over a rig's evaluated geometry.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub enum AnchorSelector {
    /// The centroid of the evaluated vertices of every
    /// mesh declared on this part.
    PartCentroid {
        /// Part name, as the rig record declares it.
        part: String,
    },
    /// One vertex of one mesh, by declaration index.
    Vertex {
        /// Mesh name, as the rig record declares it.
        mesh: String,
        /// Vertex index inside the mesh.
        index: u32,
    },
}

/// A named point on one rig of the spec.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeAnchor {
    /// Rig index into `ProbeSpec::rigs`.
    pub rig: usize,
    /// The point selector on that rig.
    pub selector: AnchorSelector,
}

/// One pose key: parameter values (x channel) at time
/// `t` seconds. Parameters a key does not name hold the
/// previous key's interpolated value path exactly as the
/// scene track sampler defines it (absent = 0 at the
/// ends, smoothstep interpolation between keys).
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProbePoseKey {
    /// Key time in seconds.
    pub t: f64,
    /// Parameter name -> value (plain numbers; the
    /// validator grammar has no expression forms).
    pub params: BTreeMap<String, f64>,
}

/// An authored placement track for one rig, in the same
/// three shapes the scene record declares.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub enum ProbePlacement {
    /// A fixed scene-space translation (no ground lift is
    /// added; the values are the placement).
    Fixed {
        /// Scene x.
        x: f64,
        /// Scene y.
        y: f64,
    },
    /// Rotation keys about the rig's foot pivot, with the
    /// measured turn compensation (the seam's pivot fold).
    PivotTurn {
        /// `(time, rotation degrees)` keys, sorted.
        keys: Vec<(f64, f64)>,
        /// Turn-lift compensation in rig units.
        compensation: f64,
    },
    /// Step in over one window, hold, step back over
    /// another (the seam's step-in/out).
    StepInOut {
        /// Held scene x while contacts are live.
        base_x: f64,
        /// Step-in distance.
        step_in: f64,
        /// Step-in window `(from, to)` in seconds.
        step_in_window: (f64, f64),
        /// Step-back distance.
        step_back: f64,
        /// Step-back window `(from, to)` in seconds.
        step_back_window: (f64, f64),
    },
}

/// One rig the probe evaluates.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeRig {
    /// Rig identifier (report and error label).
    pub id: String,
    /// The `.inx` record path, relative to the host's
    /// rig-file base.
    pub inx: String,
    /// Expected SHA-256 of the `.inx` bytes (64 lowercase
    /// hex); the host refuses a record that does not hash
    /// to this before importing it.
    pub sha256: String,
    /// Pose keys over scene time, sorted by `t`.
    pub pose_track: Vec<ProbePoseKey>,
    /// Authored placement over scene time.
    pub placement: ProbePlacement,
}

/// One attachment contact, solved by the seam while its
/// window is active. The seam offset is derived by the
/// host: `target` minus the B endpoint's rest point, both
/// in B-local scene coordinates.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeContact {
    /// Contact identifier (unique per spec).
    pub id: String,
    /// The driving side's anchor.
    pub a: ProbeAnchor,
    /// The driven side's anchor.
    pub b: ProbeAnchor,
    /// Rig whose placement the solver adjusts (one of the
    /// two anchor rigs).
    pub driven_rig: usize,
    /// The fixed point on B the A anchor is driven to, in
    /// B-local scene coordinates.
    pub target: [f64; 2],
    /// Window start (seconds, inclusive).
    pub active_from: f64,
    /// Window end (seconds, exclusive).
    pub active_to: f64,
}

/// A band region on one part of one rig: the horizontal
/// slice of the part's rest AABB between the `near` and
/// `far` fractions of its height, measured from the top
/// (the D-859 waist-band convention: the band is
/// `[aabb_min_x, top - far*h, aabb_max_x, top - near*h]`
/// in scene coordinates).
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BandSpec {
    /// Rig the band part lives on.
    pub rig: usize,
    /// Part whose rest AABB defines the band.
    pub part: String,
    /// Near fraction of the part height (smaller).
    pub near: f64,
    /// Far fraction of the part height (larger).
    pub far: f64,
}

/// One measured pair: two anchors, the measures the pair
/// supports, and the window its aggregates cover.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProbePair {
    /// Pair identifier (unique per spec).
    pub id: String,
    /// The measured point.
    pub a: ProbeAnchor,
    /// Measurement offset applied to the A point before
    /// every measure of this pair, in scene units. The
    /// calibration specs write `(0.0, 0.0)`; the
    /// adversarial spec writes the planted offset here.
    pub a_offset: [f64; 2],
    /// The reference point (distance measure's other end,
    /// when no contact is bound).
    pub b: ProbeAnchor,
    /// The seam contact this pair is bound to, if any:
    /// its slip is the pair's distance measure, and its
    /// window is expected to match the pair's.
    pub contact: Option<String>,
    /// Band measure declaration, if the pair supports it.
    pub band: Option<BandSpec>,
    /// Penetration measure declaration: the part on B's
    /// rig whose triangles the A point is tested against,
    /// if the pair supports it.
    pub penetration_part: Option<String>,
    /// Aggregate window start (seconds, inclusive).
    pub engaged_from: f64,
    /// Aggregate window end (seconds, exclusive).
    pub engaged_to: f64,
}

/// A named step whose reads are pinned in the report.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeCheckpoint {
    /// Step index.
    pub step: u64,
    /// Checkpoint label.
    pub label: String,
}

/// The probe spec: the whole declaration.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProbeSpec {
    /// Record format version; must equal
    /// [`PROBE_SPEC_VERSION`].
    pub version: u32,
    /// Spec identifier.
    pub id: String,
    /// Fixed steps the probe runs.
    pub steps: u64,
    /// Fixed-step rate (steps per second).
    pub rate: u32,
    /// Settle steps each rig's evaluator runs before
    /// step 0.
    pub settle_steps: u32,
    /// Rigs in anchor/contact index order.
    pub rigs: Vec<ProbeRig>,
    /// Root part per rig (the seam's root reads),
    /// parallel to `rigs`.
    pub roots: Vec<String>,
    /// Declared contacts (solved while active).
    pub contacts: Vec<ProbeContact>,
    /// Declared measured pairs.
    pub pairs: Vec<ProbePair>,
    /// Rig whose foot clearance is reported (its minimum
    /// leg-vertex scene y per step), if any.
    pub clearance_rig: Option<usize>,
    /// Steps pinned in the report.
    pub checkpoints: Vec<ProbeCheckpoint>,
}

impl ProbeSpec {
    /// Parse a probe spec from RON text, then validate
    /// its shape. Unknown fields, wrong versions, bad
    /// hashes, and malformed windows are typed refusals
    /// (`LE-110`).
    pub fn load_ron(text: &str) -> Result<Self, EngineError> {
        let spec: ProbeSpec =
            ron::de::from_str(text).map_err(|e| EngineError::Probe(e.to_string()))?;
        spec.validate_shape()?;
        Ok(spec)
    }

    /// Shape checks parsing alone does not prove.
    pub fn validate_shape(&self) -> Result<(), EngineError> {
        if self.version != PROBE_SPEC_VERSION {
            return Err(EngineError::RecordVersion {
                kind: "probe",
                found: self.version,
            });
        }
        if self.id.is_empty() {
            return refuse("probe spec id is empty");
        }
        if self.rigs.len() < 2 || self.rigs.len() > PROBE_RIGS_MAX {
            return refuse(format!(
                "probe needs 2..={PROBE_RIGS_MAX} rigs, declares {}",
                self.rigs.len()
            ));
        }
        if self.roots.len() != self.rigs.len() {
            return refuse(format!(
                "roots ({}) must parallel rigs ({})",
                self.roots.len(),
                self.rigs.len()
            ));
        }
        if self.rate == 0 || self.steps == 0 || self.steps > PROBE_STEPS_MAX {
            return refuse(format!(
                "probe rate/steps out of bounds: rate {}, steps {} (max {PROBE_STEPS_MAX})",
                self.rate, self.steps
            ));
        }
        let mut rig_ids: Vec<&str> = Vec::new();
        for rig in &self.rigs {
            if rig_ids.contains(&rig.id.as_str()) {
                return refuse(format!("duplicate rig id '{}'", rig.id));
            }
            rig_ids.push(rig.id.as_str());
            if !is_sha256_hex(&rig.sha256) {
                return refuse(format!(
                    "rig {}: sha256 is not 64 lowercase hex characters",
                    rig.id
                ));
            }
            if rig.pose_track.is_empty() || rig.pose_track.len() > PROBE_KEYS_MAX {
                return refuse(format!(
                    "rig {}: pose track has {} keys (1..={PROBE_KEYS_MAX})",
                    rig.id,
                    rig.pose_track.len()
                ));
            }
            for pair in rig.pose_track.windows(2) {
                if pair[1].t < pair[0].t {
                    return refuse(format!("rig {}: pose keys are not sorted by time", rig.id));
                }
            }
            for key in &rig.pose_track {
                if !key.t.is_finite() {
                    return refuse(format!("rig {}: pose key time is not finite", rig.id));
                }
                for (name, value) in &key.params {
                    if !value.is_finite() {
                        return refuse(format!(
                            "rig {}: pose value for {name} is not finite",
                            rig.id
                        ));
                    }
                }
            }
            match &rig.placement {
                ProbePlacement::PivotTurn { keys, .. } => {
                    if keys.len() < 2 {
                        return refuse(format!(
                            "rig {}: pivot-turn needs at least two keys",
                            rig.id
                        ));
                    }
                    for pair in keys.windows(2) {
                        if pair[1].0 < pair[0].0 {
                            return refuse(format!(
                                "rig {}: pivot-turn keys are not sorted by time",
                                rig.id
                            ));
                        }
                    }
                }
                ProbePlacement::StepInOut {
                    step_in_window,
                    step_back_window,
                    ..
                } => {
                    for (label, window) in
                        [("step-in", step_in_window), ("step-back", step_back_window)]
                    {
                        if window.1 <= window.0 {
                            return refuse(format!(
                                "rig {}: {label} window is empty or inverted",
                                rig.id
                            ));
                        }
                    }
                }
                ProbePlacement::Fixed { .. } => {}
            }
        }
        if self.contacts.len() > PROBE_CONTACTS_MAX {
            return refuse(format!(
                "probe declares {} contacts (max {PROBE_CONTACTS_MAX})",
                self.contacts.len()
            ));
        }
        let mut contact_ids: Vec<&str> = Vec::new();
        for contact in &self.contacts {
            if contact_ids.contains(&contact.id.as_str()) {
                return refuse(format!("duplicate contact id '{}'", contact.id));
            }
            contact_ids.push(contact.id.as_str());
            self.check_anchor("contact", &contact.id, &contact.a)?;
            self.check_anchor("contact", &contact.id, &contact.b)?;
            if contact.driven_rig != contact.a.rig && contact.driven_rig != contact.b.rig {
                return refuse(format!(
                    "contact {}: driven_rig {} is not one of its anchor rigs",
                    contact.id, contact.driven_rig
                ));
            }
            if contact.active_to <= contact.active_from {
                return refuse(format!(
                    "contact {}: active window is empty or inverted",
                    contact.id
                ));
            }
            if !contact.target.iter().all(|v| v.is_finite()) {
                return refuse(format!("contact {}: target is not finite", contact.id));
            }
        }
        if self.pairs.is_empty() || self.pairs.len() > PROBE_PAIRS_MAX {
            return refuse(format!(
                "probe needs 1..={PROBE_PAIRS_MAX} pairs, declares {}",
                self.pairs.len()
            ));
        }
        let mut pair_ids: Vec<&str> = Vec::new();
        for pair in &self.pairs {
            if pair_ids.contains(&pair.id.as_str()) {
                return refuse(format!("duplicate pair id '{}'", pair.id));
            }
            pair_ids.push(pair.id.as_str());
            self.check_anchor("pair", &pair.id, &pair.a)?;
            self.check_anchor("pair", &pair.id, &pair.b)?;
            if !pair.a_offset.iter().all(|v| v.is_finite()) {
                return refuse(format!("pair {}: a_offset is not finite", pair.id));
            }
            if let Some(contact_id) = &pair.contact {
                if !contact_ids.contains(&contact_id.as_str()) {
                    return refuse(format!(
                        "pair {}: bound contact '{contact_id}' is not declared",
                        pair.id
                    ));
                }
            }
            if let Some(band) = &pair.band {
                if band.rig >= self.rigs.len() {
                    return refuse(format!(
                        "pair {}: band rig {} out of range",
                        pair.id, band.rig
                    ));
                }
                if !(0.0 <= band.near && band.near < band.far && band.far <= 1.0) {
                    return refuse(format!(
                        "pair {}: band fractions must satisfy 0 <= near < far <= 1",
                        pair.id
                    ));
                }
            }
            if pair.engaged_to <= pair.engaged_from {
                return refuse(format!(
                    "pair {}: engaged window is empty or inverted",
                    pair.id
                ));
            }
        }
        if let Some(rig) = self.clearance_rig {
            if rig >= self.rigs.len() {
                return refuse(format!("clearance rig {rig} out of range"));
            }
        }
        for checkpoint in &self.checkpoints {
            if checkpoint.step >= self.steps {
                return refuse(format!(
                    "checkpoint '{}' at step {} is outside the probe",
                    checkpoint.label, checkpoint.step
                ));
            }
        }
        Ok(())
    }

    fn check_anchor(&self, owner: &str, id: &str, anchor: &ProbeAnchor) -> Result<(), EngineError> {
        if anchor.rig >= self.rigs.len() {
            return refuse(format!(
                "{owner} {id}: anchor rig {} out of range",
                anchor.rig
            ));
        }
        Ok(())
    }

    /// Validator-semantics legality of the whole timeline
    /// against the rigs' own parameter tables: every
    /// parameter a key names must exist on that rig, and
    /// every value must lie inside the rig's declared
    /// range for it. Refusal happens here, before any
    /// evaluation. Key legality implies step legality:
    /// sampling interpolates linearly in the smoothstep
    /// fraction between adjacent key values, so a sampled
    /// value never leaves the bracket its keys declare.
    pub fn validate_timeline(&self, tables: &[RigParameterTable]) -> Result<(), EngineError> {
        if tables.len() != self.rigs.len() {
            return refuse(format!(
                "parameter tables ({}) must parallel rigs ({})",
                tables.len(),
                self.rigs.len()
            ));
        }
        for (rig, table) in self.rigs.iter().zip(tables.iter()) {
            if table.rig_id != rig.id {
                return refuse(format!(
                    "parameter table for '{}' does not match rig '{}'",
                    table.rig_id, rig.id
                ));
            }
            for key in &rig.pose_track {
                for (name, value) in &key.params {
                    let Some((min, max)) = table.range_of(name) else {
                        return refuse(format!(
                            "rig {}: timeline names unknown parameter '{name}'",
                            rig.id
                        ));
                    };
                    let value32 = *value as f32;
                    if value32 < min || value32 > max {
                        return refuse(format!(
                            "rig {}: value {value} for parameter '{name}' is outside \
                             its declared range [{min}, {max}]",
                            rig.id
                        ));
                    }
                }
            }
        }
        Ok(())
    }
}

/// A SHA-256 hex digest in the grammar's form: exactly 64
/// lowercase hex characters (Tool 1's `ACTLAB-BAD-HASH`
/// rule, carried as data discipline).
pub fn is_sha256_hex(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// The band box of a part: `[min_x, min_y, max_x, max_y]`
/// from the part's rest AABB under the band fractions
/// (see [`BandSpec`]).
pub fn band_box(aabb: ([f64; 2], [f64; 2]), near: f64, far: f64) -> [f64; 4] {
    let (lo, hi) = aabb;
    let height = hi[1] - lo[1];
    [lo[0], hi[1] - far * height, hi[0], hi[1] - near * height]
}

/// Distance from a point to an axis-aligned box (0 while
/// the point is inside) — the band measure.
pub fn box_distance(point: [f64; 2], lo: [f64; 2], hi: [f64; 2]) -> f64 {
    let dx = (lo[0] - point[0]).max(point[0] - hi[0]).max(0.0);
    let dy = (lo[1] - point[1]).max(point[1] - hi[1]).max(0.0);
    dx.hypot(dy)
}

/* ---------------------------------------------------------------- */
/* Report                                                          */
/* ---------------------------------------------------------------- */

/// Per-rig provenance in the report: what was actually
/// loaded and verified.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ProbeRigReport {
    /// Rig identifier.
    pub id: String,
    /// The `.inx` path the host read.
    pub inx: String,
    /// SHA-256 of the bytes read (verified against the
    /// spec before import).
    pub sha256: String,
    /// Scalar parameters the record declares.
    pub parameters: usize,
    /// Part nodes the record declares.
    pub parts: usize,
}

/// Aggregates for one measure over a pair's engaged
/// steps. Absent when the pair does not declare the
/// measure or no engaged step produced it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct MeasureAggregate {
    /// Worst (largest) value over engaged steps.
    pub max: f64,
    /// Mean over engaged steps.
    pub mean: f64,
    /// For signed measures (penetration): the smallest
    /// value over engaged steps.
    pub min: f64,
}

/// One pair's full report: capability labels, engaged
/// aggregates, and the per-step series the aggregates
/// came from.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ProbePairReport {
    /// Pair identifier.
    pub id: String,
    /// Bound contact id, if any.
    pub contact: Option<String>,
    /// Capability: the pair reports endpoint/contact
    /// distance.
    pub has_distance: bool,
    /// Capability: the pair reports band distance.
    pub has_band_distance: bool,
    /// Capability: the pair reports signed penetration
    /// (point vs the named part's triangles).
    pub has_penetration: bool,
    /// Why a measure is absent, for absent measures:
    /// honest capability labelling in the report itself.
    pub capability_notes: Vec<String>,
    /// Engaged steps that produced measures.
    pub engaged_steps: u64,
    /// Distance aggregate (seam slip when bound to a
    /// contact, else endpoint separation).
    pub distance: Option<MeasureAggregate>,
    /// Band-distance aggregate.
    pub band_distance: Option<MeasureAggregate>,
    /// Penetration aggregate (signed; `min` is the
    /// deepest reading).
    pub penetration: Option<MeasureAggregate>,
    /// Per-step A anchor point in scene space (after the
    /// pair's measurement offset), every step.
    pub a_world: Vec<[f64; 2]>,
    /// Per-step distance, `None` where the measure does
    /// not apply (unbound pair distance always applies;
    /// contact slip applies at every step the seam reads,
    /// engaged or not — aggregates use engaged only).
    pub distance_series: Vec<Option<f64>>,
    /// Per-step band distance where declared.
    pub band_series: Vec<Option<f64>>,
    /// Per-step signed penetration where declared.
    pub penetration_series: Vec<Option<f64>>,
    /// The band box in B-local scene coordinates, where
    /// declared.
    pub band_box_local: Option<[f64; 4]>,
}

/// One checkpoint's pinned reads.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ProbeCheckpointRead {
    /// Step index.
    pub step: u64,
    /// Checkpoint label.
    pub label: String,
    /// Clearance of the clearance rig at this step.
    pub clearance: Option<f64>,
    /// Per-pair reads at this step (pair id -> values).
    pub pairs: BTreeMap<String, ProbeStepRead>,
}

/// One pair's values at one step.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ProbeStepRead {
    /// Distance at the step, if measured.
    pub distance: Option<f64>,
    /// Band distance at the step, if measured.
    pub band_distance: Option<f64>,
    /// Signed penetration at the step, if measured.
    pub penetration: Option<f64>,
}

/// The probe's machine-readable report. Field order is
/// declaration order everywhere and maps are `BTreeMap`s,
/// so two runs over one spec serialize byte-identically.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ProbeReport {
    /// Report format version (mirrors the spec version).
    pub version: u32,
    /// Spec identifier.
    pub spec_id: String,
    /// Steps evaluated.
    pub steps: u64,
    /// Per-rig provenance.
    pub rigs: Vec<ProbeRigReport>,
    /// Per-pair reports, in spec order.
    pub pairs: Vec<ProbePairReport>,
    /// Clearance rig index, if declared.
    pub clearance_rig: Option<usize>,
    /// Minimum clearance over engaged steps of every
    /// pair window union (steps where any pair is
    /// engaged), if a clearance rig is declared.
    pub clearance_min_engaged: Option<f64>,
    /// Per-step clearance series (minimum leg-vertex
    /// scene y of the clearance rig), if declared.
    pub clearance_series: Option<Vec<f64>>,
    /// Checkpoint reads, in spec order.
    pub checkpoints: Vec<ProbeCheckpointRead>,
}

/// Accumulates one pair's per-step measures and finishes
/// into a [`ProbePairReport`]. Pure: the host feeds it
/// measured values, it owns ordering and aggregation.
#[derive(Clone, Debug)]
pub struct PairAccumulator {
    id: String,
    contact: Option<String>,
    has_band: bool,
    has_penetration: bool,
    band_box_local: Option<[f64; 4]>,
    engaged_from: f64,
    engaged_to: f64,
    rate: u32,
    a_world: Vec<[f64; 2]>,
    distance_series: Vec<Option<f64>>,
    band_series: Vec<Option<f64>>,
    penetration_series: Vec<Option<f64>>,
    engaged_steps: u64,
    distance_sum: f64,
    distance_max: f64,
    distance_min: f64,
    band_sum: f64,
    band_max: f64,
    band_min: f64,
    penetration_sum: f64,
    penetration_max: f64,
    penetration_min: f64,
}

impl PairAccumulator {
    /// Start accumulating for one declared pair.
    pub fn new(pair: &ProbePair, rate: u32, band_box_local: Option<[f64; 4]>) -> Self {
        Self {
            id: pair.id.clone(),
            contact: pair.contact.clone(),
            has_band: pair.band.is_some(),
            has_penetration: pair.penetration_part.is_some(),
            band_box_local,
            engaged_from: pair.engaged_from,
            engaged_to: pair.engaged_to,
            rate,
            a_world: Vec::new(),
            distance_series: Vec::new(),
            band_series: Vec::new(),
            penetration_series: Vec::new(),
            engaged_steps: 0,
            distance_sum: 0.0,
            distance_max: f64::NEG_INFINITY,
            distance_min: f64::INFINITY,
            band_sum: 0.0,
            band_max: f64::NEG_INFINITY,
            band_min: f64::INFINITY,
            penetration_sum: 0.0,
            penetration_max: f64::NEG_INFINITY,
            penetration_min: f64::INFINITY,
        }
    }

    /// The band box this accumulator was built with
    /// (B-local scene coordinates), if the pair declares
    /// a band.
    pub fn band_box_local(&self) -> Option<[f64; 4]> {
        self.band_box_local
    }

    /// Whether `t` (seconds) is inside this pair's
    /// engaged window.
    pub fn engaged_at(&self, t: f64) -> bool {
        (self.engaged_from..self.engaged_to).contains(&t)
    }

    /// Feed one step's measured values. `step` must be
    /// fed in order, one call per step.
    pub fn push(
        &mut self,
        step: u64,
        a_world: [f64; 2],
        distance: Option<f64>,
        band: Option<f64>,
        penetration: Option<f64>,
    ) {
        debug_assert_eq!(step as usize, self.a_world.len(), "steps fed in order");
        let t = step as f64 / f64::from(self.rate);
        let engaged = self.engaged_at(t);
        self.a_world.push(a_world);
        self.distance_series.push(distance);
        self.band_series.push(band);
        self.penetration_series.push(penetration);
        if engaged {
            let mut counted = false;
            if let Some(value) = distance {
                counted = true;
                self.distance_sum += value;
                self.distance_max = self.distance_max.max(value);
                self.distance_min = self.distance_min.min(value);
            }
            if let Some(value) = band {
                counted = true;
                self.band_sum += value;
                self.band_max = self.band_max.max(value);
                self.band_min = self.band_min.min(value);
            }
            if let Some(value) = penetration {
                counted = true;
                self.penetration_sum += value;
                self.penetration_max = self.penetration_max.max(value);
                self.penetration_min = self.penetration_min.min(value);
            }
            if counted {
                self.engaged_steps += 1;
            }
        }
    }

    fn aggregate(sum: f64, max: f64, min: f64, count: u64) -> Option<MeasureAggregate> {
        if count == 0 || max == f64::NEG_INFINITY {
            return None;
        }
        Some(MeasureAggregate {
            max,
            mean: sum / count as f64,
            min,
        })
    }

    /// Finish into the report row. `steps` is the run's
    /// total step count (series length check).
    pub fn finish(self, has_distance: bool) -> ProbePairReport {
        let count = self.engaged_steps;
        let mut capability_notes = Vec::new();
        if !self.has_band {
            capability_notes
                .push("band_distance absent: the pair declares no band region on B".to_string());
        }
        if !self.has_penetration {
            capability_notes.push(
                "penetration absent: the pair declares no B part for a signed \
                 point-to-mesh margin"
                    .to_string(),
            );
        }
        capability_notes.push(
            "no mesh-overlap volume is reported for any pair: point anchors have \
             no extent; volume interpenetration needs the mesh audit"
                .to_string(),
        );
        ProbePairReport {
            id: self.id,
            contact: self.contact,
            has_distance,
            has_band_distance: self.has_band,
            has_penetration: self.has_penetration,
            capability_notes,
            engaged_steps: count,
            distance: Self::aggregate(
                self.distance_sum,
                self.distance_max,
                self.distance_min,
                count,
            ),
            band_distance: Self::aggregate(self.band_sum, self.band_max, self.band_min, count),
            penetration: Self::aggregate(
                self.penetration_sum,
                self.penetration_max,
                self.penetration_min,
                count,
            ),
            a_world: self.a_world,
            distance_series: self.distance_series,
            band_series: self.band_series,
            penetration_series: self.penetration_series,
            band_box_local: self.band_box_local,
        }
    }
}
