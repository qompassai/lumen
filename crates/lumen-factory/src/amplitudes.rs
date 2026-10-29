//! Blendshape displacement amplitudes: how far the mesh actually
//! moves, per morph target and per ARKit channel (GAP-11-1).
//!
//! Contract summary:
//! - Channel weights and channel membership were always gated
//!   (`blend.rs`: 52 known channels, weights in 0.0..=1.0, fail
//!   closed). What no record specified was the *travel*: the
//!   displacement fields are procedural, so a channel's amplitude
//!   existed only as code. This module makes amplitudes a measured,
//!   checked-in spec.
//! - [`measure_target_amplitudes`] reads a built GLB's morph
//!   targets (the head primitive's `extras.targetNames` set) and
//!   reports each target's maximum vertex displacement at weight
//!   1.0 — the shipped truth, measured off the artifact the way
//!   Canon-11 asked (29 targets in `assets/seraphine.glb`).
//! - [`channel_amplitudes`] computes every known channel's maximum
//!   displacement at weight 1.0 over the canonical base head,
//!   straight from `blend.rs`'s displacement fields.
//! - An [`AmplitudeTable`] (schema `lumen.factory-amplitudes` v1)
//!   is the checked-in record: source GLB name + SHA-256, the
//!   tolerance, and both amplitude sets. [`AmplitudeTable::check`]
//!   re-measures and fails closed, naming the first drifted entry,
//!   when any amplitude moved by more than
//!   [`AMPLITUDE_TOLERANCE_M`] — so a silent change to a
//!   displacement field or a rebuilt GLB trips the gate in tests.
//! - Everything fails closed: a GLB with no named targets, a
//!   non-f32 / strided / truncated accessor, or a table whose
//!   target set differs from the artifact's is an error.

#![forbid(unsafe_code)]

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::base::build_base_mesh;
use crate::blend::{KNOWN_CHANNELS, channel_field};
use crate::gltf::parse_glb_parts;
use crate::{FactoryError, MAX_PART_VERTICES, sha256_hex};

/// Schema identifier emitted in every amplitude table.
pub const AMPLITUDES_SCHEMA: &str = "lumen.factory-amplitudes";
/// Schema version emitted in every amplitude table.
pub const AMPLITUDES_VERSION: u32 = 1;

/// Drift tolerance for the amplitude gate, in metres. Amplitudes
/// are millimetre-scale (the largest channel travel is ~10 mm);
/// 0.01 mm sits far above float round-trip noise (measurements are
/// f32-exact reads and deterministic field evaluations) and far
/// below any intentional field change.
pub const AMPLITUDE_TOLERANCE_M: f64 = 1e-5;

/// One morph target's measured amplitude at weight 1.0.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TargetAmplitude {
    /// Maximum Euclidean vertex displacement, metres.
    pub max_displacement_m: f64,
    /// Target name (from the mesh's `extras.targetNames`).
    pub name: String,
    /// Vertex count of the target's delta accessor.
    pub vertices: usize,
}

/// One ARKit channel's measured amplitude at weight 1.0 over the
/// canonical base head.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChannelAmplitude {
    /// Channel name (a member of `blend::KNOWN_CHANNELS`).
    pub channel: String,
    /// Maximum Euclidean vertex displacement, metres.
    pub max_displacement_m: f64,
}

/// The checked-in amplitude spec for one worked character.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AmplitudeTable {
    /// Per-channel amplitudes over the canonical base head.
    pub channels: Vec<ChannelAmplitude>,
    /// Schema identifier ([`AMPLITUDES_SCHEMA`]).
    pub schema: String,
    /// Source GLB file name the target amplitudes were measured
    /// from (name only, so the record stays portable).
    pub source_glb: String,
    /// SHA-256 of the source GLB bytes at measurement time.
    pub source_glb_sha256: String,
    /// Per-target amplitudes measured from the source GLB.
    pub targets: Vec<TargetAmplitude>,
    /// The drift tolerance this table is gated at, metres.
    pub tolerance_m: f64,
    /// Schema version ([`AMPLITUDES_VERSION`]).
    pub version: u32,
}

fn norm(d: [f32; 3]) -> f64 {
    let (x, y, z) = (f64::from(d[0]), f64::from(d[1]), f64::from(d[2]));
    (x * x + y * y + z * z).sqrt()
}

/// Read one accessor as f32 VEC3 elements. The factory writer
/// emits tightly packed accessors; a strided or differently typed
/// accessor is rejected fail-closed rather than misread.
fn read_vec3_accessor(
    json: &Value,
    bin: &[u8],
    accessor_idx: usize,
) -> Result<Vec<[f32; 3]>, FactoryError> {
    let accessors = json
        .get("accessors")
        .and_then(Value::as_array)
        .ok_or_else(|| FactoryError::InvalidGlb("glb has no accessors".to_string()))?;
    let views = json
        .get("bufferViews")
        .and_then(Value::as_array)
        .ok_or_else(|| FactoryError::InvalidGlb("glb has no bufferViews".to_string()))?;
    let accessor = accessors
        .get(accessor_idx)
        .ok_or_else(|| FactoryError::InvalidGlb("accessor index out of range".to_string()))?;
    if accessor.get("componentType").and_then(Value::as_u64) != Some(5126)
        || accessor.get("type").and_then(Value::as_str) != Some("VEC3")
    {
        return Err(FactoryError::InvalidGlb(format!(
            "accessor {accessor_idx} is not f32 VEC3"
        )));
    }
    let count = accessor
        .get("count")
        .and_then(Value::as_u64)
        .ok_or_else(|| FactoryError::InvalidGlb("accessor missing count".to_string()))?
        as usize;
    if count > MAX_PART_VERTICES {
        return Err(FactoryError::InvalidGlb(format!(
            "accessor count {count} exceeds bound"
        )));
    }
    let view_idx = accessor
        .get("bufferView")
        .and_then(Value::as_u64)
        .ok_or_else(|| FactoryError::InvalidGlb("accessor missing bufferView".to_string()))?
        as usize;
    let view = views
        .get(view_idx)
        .ok_or_else(|| FactoryError::InvalidGlb("bufferView index out of range".to_string()))?;
    if view.get("byteStride").is_some() {
        return Err(FactoryError::InvalidGlb(
            "interleaved (byteStride) accessors are not supported by the amplitude measurement"
                .to_string(),
        ));
    }
    let view_offset = view.get("byteOffset").and_then(Value::as_u64).unwrap_or(0) as usize;
    let accessor_offset = accessor
        .get("byteOffset")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    let start = view_offset
        .checked_add(accessor_offset)
        .ok_or_else(|| FactoryError::InvalidGlb("accessor offset overflow".to_string()))?;
    let byte_len = count
        .checked_mul(12)
        .ok_or_else(|| FactoryError::InvalidGlb("accessor size overflow".to_string()))?;
    let end = start
        .checked_add(byte_len)
        .filter(|end| *end <= bin.len())
        .ok_or_else(|| FactoryError::InvalidGlb("accessor overruns BIN chunk".to_string()))?;
    let mut out = Vec::with_capacity(count);
    // The range length is exactly count * 12 (checked above), so
    // as_chunks leaves no remainder.
    for chunk in bin[start..end].as_chunks::<12>().0 {
        let f = |b: &[u8]| f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        out.push([f(&chunk[0..4]), f(&chunk[4..8]), f(&chunk[8..12])]);
    }
    Ok(out)
}

/// Measure every named morph target's maximum vertex displacement
/// from a built GLB. The target set comes from the one mesh that
/// carries `extras.targetNames`; targets are accumulated across
/// that mesh's primitives (the writer puts them on the head
/// primitive only). The result is sorted by target name.
pub fn measure_target_amplitudes(glb: &[u8]) -> Result<Vec<TargetAmplitude>, FactoryError> {
    let parts = parse_glb_parts(glb)?;
    let meshes = parts
        .json
        .get("meshes")
        .and_then(Value::as_array)
        .ok_or_else(|| FactoryError::InvalidGlb("glb has no meshes".to_string()))?;
    let mut found: Option<Vec<TargetAmplitude>> = None;
    for mesh in meshes {
        let names: Vec<String> = mesh
            .get("extras")
            .and_then(|e| e.get("targetNames"))
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        if names.is_empty() {
            continue;
        }
        if found.is_some() {
            return Err(FactoryError::InvalidGlb(
                "more than one mesh carries targetNames".to_string(),
            ));
        }
        let mut amplitudes: Vec<f64> = vec![0.0; names.len()];
        let mut counts: Vec<usize> = vec![0; names.len()];
        let primitives = mesh
            .get("primitives")
            .and_then(Value::as_array)
            .ok_or_else(|| FactoryError::InvalidGlb("mesh has no primitives".to_string()))?;
        for primitive in primitives {
            let Some(targets) = primitive.get("targets").and_then(Value::as_array) else {
                continue;
            };
            if targets.len() != names.len() {
                return Err(FactoryError::InvalidGlb(format!(
                    "primitive carries {} targets for {} target names",
                    targets.len(),
                    names.len()
                )));
            }
            for (i, target) in targets.iter().enumerate() {
                let accessor_idx =
                    target
                        .get("POSITION")
                        .and_then(Value::as_u64)
                        .ok_or_else(|| {
                            FactoryError::InvalidGlb("morph target missing POSITION".to_string())
                        })? as usize;
                let deltas = read_vec3_accessor(&parts.json, &parts.bin, accessor_idx)?;
                counts[i] = counts[i].max(deltas.len());
                for delta in &deltas {
                    let n = norm(*delta);
                    if !n.is_finite() {
                        return Err(FactoryError::InvalidGlb(format!(
                            "target {} contains a non-finite delta",
                            names[i]
                        )));
                    }
                    amplitudes[i] = amplitudes[i].max(n);
                }
            }
        }
        found = Some(
            names
                .into_iter()
                .enumerate()
                .map(|(i, name)| TargetAmplitude {
                    max_displacement_m: amplitudes[i],
                    name,
                    vertices: counts[i],
                })
                .collect(),
        );
    }
    let mut out = found.ok_or_else(|| {
        FactoryError::InvalidGlb("no mesh carries extras.targetNames".to_string())
    })?;
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

/// Measure every known channel's maximum displacement at weight
/// 1.0 over the canonical base head, from `blend.rs`'s procedural
/// fields. Sorted by channel name (the `KNOWN_CHANNELS` order).
pub fn channel_amplitudes() -> Result<Vec<ChannelAmplitude>, FactoryError> {
    let mesh = build_base_mesh()?;
    let head = mesh
        .parts
        .iter()
        .find(|part| part.name == "head")
        .ok_or_else(|| FactoryError::MeshInvalid("head part missing".to_string()))?;
    let positions: Vec<[f32; 3]> = head.vertices.iter().map(|v| v.position).collect();
    // The centre blend.rs composes against: the head's bbox centre.
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for position in &positions {
        for axis in 0..3 {
            min[axis] = min[axis].min(position[axis]);
            max[axis] = max[axis].max(position[axis]);
        }
    }
    let center = [
        (min[0] + max[0]) * 0.5,
        (min[1] + max[1]) * 0.5,
        (min[2] + max[2]) * 0.5,
    ];
    let mut out = Vec::with_capacity(KNOWN_CHANNELS.len());
    for channel in KNOWN_CHANNELS {
        let field =
            channel_field(channel, &positions, &mesh.head_masks, center).ok_or_else(|| {
                FactoryError::BadParam(format!("known channel {channel} has no field"))
            })?;
        let mut peak = 0.0f64;
        for delta in &field {
            peak = peak.max(norm(*delta));
        }
        out.push(ChannelAmplitude {
            channel: (*channel).to_string(),
            max_displacement_m: peak,
        });
    }
    Ok(out)
}

impl AmplitudeTable {
    /// Measure a fresh table for `glb` (target amplitudes from the
    /// artifact, channel amplitudes from the canonical head).
    pub fn measure(glb: &[u8], source_name: &str) -> Result<Self, FactoryError> {
        Ok(Self {
            channels: channel_amplitudes()?,
            schema: AMPLITUDES_SCHEMA.to_string(),
            source_glb: source_name.to_string(),
            source_glb_sha256: sha256_hex(glb),
            targets: measure_target_amplitudes(glb)?,
            tolerance_m: AMPLITUDE_TOLERANCE_M,
            version: AMPLITUDES_VERSION,
        })
    }

    /// Re-measure `glb` (and the channel fields) and fail closed,
    /// naming the drifted entry, when anything moved by more than
    /// this table's tolerance, when the target/channel sets differ,
    /// or when the GLB bytes no longer hash to the recorded source.
    /// This is the GAP-11-1 gate: the checked-in table is the spec,
    /// the artifact and the fields must keep matching it.
    pub fn check(&self, glb: &[u8]) -> Result<(), FactoryError> {
        if self.schema != AMPLITUDES_SCHEMA || self.version != AMPLITUDES_VERSION {
            return Err(FactoryError::BadParam(format!(
                "unsupported amplitudes schema {} v{}",
                self.schema, self.version
            )));
        }
        let actual_hash = sha256_hex(glb);
        if actual_hash != self.source_glb_sha256 {
            return Err(FactoryError::BadParam(format!(
                "source GLB hash {actual_hash} != recorded {} (the artifact moved; re-measure the table)",
                self.source_glb_sha256
            )));
        }
        let measured_targets = measure_target_amplitudes(glb)?;
        check_amplitude_sets(
            "target",
            &self
                .targets
                .iter()
                .map(|t| (t.name.as_str(), t.max_displacement_m))
                .collect::<Vec<_>>(),
            &measured_targets
                .iter()
                .map(|t| (t.name.as_str(), t.max_displacement_m))
                .collect::<Vec<_>>(),
            self.tolerance_m,
        )?;
        let measured_channels = channel_amplitudes()?;
        check_amplitude_sets(
            "channel",
            &self
                .channels
                .iter()
                .map(|c| (c.channel.as_str(), c.max_displacement_m))
                .collect::<Vec<_>>(),
            &measured_channels
                .iter()
                .map(|c| (c.channel.as_str(), c.max_displacement_m))
                .collect::<Vec<_>>(),
            self.tolerance_m,
        )?;
        Ok(())
    }

    /// Serialize as JSON (pretty, struct field order — the table is
    /// a spec record, read by humans and diffed in review).
    pub fn to_json(&self) -> Result<String, FactoryError> {
        serde_json::to_string_pretty(self)
            .map_err(|e| FactoryError::BadParam(format!("amplitudes serialize: {e}")))
    }

    /// Parse a table from JSON text. (Drift is [`Self::check`]'s
    /// job, against the live artifact.)
    pub fn from_json(text: &str) -> Result<Self, FactoryError> {
        let table: Self = serde_json::from_str(text)
            .map_err(|e| FactoryError::BadParam(format!("amplitudes parse: {e}")))?;
        if table.schema != AMPLITUDES_SCHEMA || table.version != AMPLITUDES_VERSION {
            return Err(FactoryError::BadParam(format!(
                "unsupported amplitudes schema {} v{}",
                table.schema, table.version
            )));
        }
        Ok(table)
    }
}

/// Compare two `(name, amplitude)` sets: identical name sets, and
/// every amplitude within `tolerance` of its recorded value. The
/// first mismatch fails closed, naming the entry and both values.
fn check_amplitude_sets(
    kind: &str,
    recorded_in: &[(&str, f64)],
    measured_in: &[(&str, f64)],
    tolerance: f64,
) -> Result<(), FactoryError> {
    let mut recorded = recorded_in.to_vec();
    let mut measured = measured_in.to_vec();
    recorded.sort_by(|a, b| a.0.cmp(b.0));
    measured.sort_by(|a, b| a.0.cmp(b.0));
    let recorded = recorded.as_slice();
    let measured = measured.as_slice();
    if recorded.len() != measured.len() {
        return Err(FactoryError::BadParam(format!(
            "{kind} set size differs: recorded {} vs measured {}",
            recorded.len(),
            measured.len()
        )));
    }
    for ((rname, rvalue), (mname, mvalue)) in recorded.iter().zip(measured.iter()) {
        if rname != mname {
            return Err(FactoryError::BadParam(format!(
                "{kind} set differs: recorded {rname} vs measured {mname}"
            )));
        }
        if (rvalue - mvalue).abs() > tolerance {
            return Err(FactoryError::BadParam(format!(
                "{kind} {rname} amplitude drifted: recorded {rvalue:.6} m vs measured {mvalue:.6} m (tolerance {tolerance} m)"
            )));
        }
    }
    Ok(())
}
