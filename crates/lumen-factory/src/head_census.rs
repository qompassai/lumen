//! Head-mesh census and the texture-step checklist (four-video
//! T-6 + T-13).
//!
//! Contract summary:
//! - [`census_head_mesh`] measures one head mesh — as extracted
//!   from a `.blend` by the blend census — against the three
//!   hygiene rules of D-734, each as its own named check:
//!   (a) `duplicate_vertex_coordinates`: no two vertices at exactly
//!   identical coordinates (coincident vertices break shape keys
//!   on engine import); (b) `lower_lid_behind_upper_lid`: the
//!   named lower-lid loop sits behind the named upper-lid loop
//!   along the facing axis (+Z, factory convention), measured by
//!   loop centroid depth; (c) `subdivision_cage_level`: the
//!   recorded subdivision cage is exactly one level-1 modifier —
//!   the level is recorded in the census either way.
//! - The report also carries the face plausibility gate inputs
//!   ([`FacePlausibilityInputs`]): the three rule verdicts plus
//!   UV-layer presence, as plain data a gate can consume without
//!   re-measuring.
//! - The texture checklist (D-742) gates steps on preconditions
//!   *before* any work runs: the permanent-shadow step refuses
//!   cleanly with the named reason
//!   `permanent_shadow_requires_uv_layer` when the mesh has no UV
//!   layer, instead of dying mid-pipeline the way the source
//!   video's run did. The checklist owns the gate; the painting
//!   itself is a downstream desk step.
//! - All inputs are validated fail-closed before measurement:
//!   bounded vertex counts, finite coordinates, loop indices in
//!   range and unique within their loop, named UV layers.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use serde::Serialize;

use crate::FactoryError;
use crate::MAX_PART_VERTICES;
use crate::validate::ValidationCheck;

/// Schema identifier for the blend census report.
pub const BLEND_CENSUS_SCHEMA: &str = "lumen.blend-census";
/// Schema version for the blend census report.
pub const BLEND_CENSUS_VERSION: u32 = 1;
/// Highest subdivision level the census will record (Blender's
/// Subdivision Surface modifier caps at 11; anything above is a
/// corrupt extraction, not a level).
pub const SUBDIVISION_LEVEL_MAX: u8 = 11;
/// The permanent-shadow texture step's checklist name.
pub const PERMANENT_SHADOW_STEP: &str = "permanent_shadow";
/// The named refusal reason when the permanent-shadow step is
/// asked to run on a mesh with no UV layer (D-742).
pub const PERMANENT_SHADOW_UV_REASON: &str = "permanent_shadow_requires_uv_layer: the permanent-shadow step paints into a UV-mapped texture slot, and this mesh has no UV layer";

/// One head mesh as extracted by the blend census: vertex
/// positions plus the named structures the hygiene rules measure.
/// (Face UV *contract* assessment for SDF shading lives in
/// [`crate::face_uv`]; this record carries layer presence only.)
#[derive(Clone, Debug, PartialEq)]
pub struct HeadMeshData {
    /// Vertex indices of the lower eyelid loop, in loop order.
    pub lower_lid_loop: Vec<usize>,
    /// Subdivision Surface modifier levels, in stack order
    /// (empty when the mesh carries no such modifier).
    pub subdivision_levels: Vec<u8>,
    /// Vertex indices of the upper eyelid loop, in loop order.
    pub upper_lid_loop: Vec<usize>,
    /// UV layer names present on the mesh.
    pub uv_layers: Vec<String>,
    /// Vertex positions in meters (Y-up, facing +Z).
    pub vertices: Vec<[f32; 3]>,
}

/// The face plausibility gate inputs, distilled from the census:
/// one boolean per rule, plus UV-layer presence.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct FacePlausibilityInputs {
    /// Rule (a): no exact-duplicate vertex coordinates.
    pub duplicate_vertices_clean: bool,
    /// Rule (b): lower lid behind upper lid.
    pub lid_depth_order_clean: bool,
    /// Rule (c): subdivision cage is exactly one level-1 modifier.
    pub subdivision_cage_level_one: bool,
    /// A UV layer is present (the T-13 precondition).
    pub uv_layer_present: bool,
}

/// The raw measurements behind the hygiene checks.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct HeadCensusMeasurements {
    /// Vertices beyond the first at an already-seen coordinate.
    pub duplicate_vertex_excess: usize,
    /// Coordinates shared by two or more vertices.
    pub duplicate_vertex_groups: usize,
    /// Lower-lid loop centroid depth (mean Z, meters).
    pub lower_lid_centroid_depth: f64,
    /// Recorded subdivision modifier levels, in stack order.
    pub subdivision_levels: Vec<u8>,
    /// Upper-lid loop centroid depth (mean Z, meters).
    pub upper_lid_centroid_depth: f64,
}

/// The blend census report for one head mesh: the three named
/// hygiene checks in rule order, their measurements, and the
/// face plausibility gate inputs.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BlendCensusReport {
    /// The hygiene checks, in rule order (a), (b), (c).
    pub checks: Vec<ValidationCheck>,
    /// The distilled gate inputs.
    pub face_plausibility: FacePlausibilityInputs,
    /// The measurements the checks were decided from.
    pub head: HeadCensusMeasurements,
    /// True iff every hygiene check passed.
    pub pass: bool,
    /// Schema identifier, always [`BLEND_CENSUS_SCHEMA`].
    pub schema: String,
    /// Number of UV layers recorded on the mesh.
    pub uv_layer_count: usize,
    /// Schema version, always [`BLEND_CENSUS_VERSION`].
    pub version: u32,
}

fn validate_loop(
    vertices: &[[f32; 3]],
    loop_indices: &[usize],
    name: &str,
) -> Result<(), FactoryError> {
    if loop_indices.is_empty() {
        return Err(FactoryError::BadParam(format!("{name} loop is empty")));
    }
    let mut seen: Vec<usize> = Vec::with_capacity(loop_indices.len());
    for &index in loop_indices {
        if index >= vertices.len() {
            return Err(FactoryError::BadParam(format!(
                "{name} loop index {index} out of range for {} vertices",
                vertices.len()
            )));
        }
        if seen.contains(&index) {
            return Err(FactoryError::BadParam(format!(
                "{name} loop repeats vertex {index}"
            )));
        }
        seen.push(index);
    }
    Ok(())
}

fn validate_head_mesh_data(data: &HeadMeshData) -> Result<(), FactoryError> {
    if data.vertices.is_empty() {
        return Err(FactoryError::BadParam(
            "head mesh has no vertices".to_string(),
        ));
    }
    if data.vertices.len() > MAX_PART_VERTICES {
        return Err(FactoryError::BadParam(format!(
            "head mesh vertex count {} exceeds bound {MAX_PART_VERTICES}",
            data.vertices.len()
        )));
    }
    if data
        .vertices
        .iter()
        .any(|vertex| vertex.iter().any(|component| !component.is_finite()))
    {
        return Err(FactoryError::BadParam(
            "head mesh carries a non-finite vertex coordinate".to_string(),
        ));
    }
    validate_loop(&data.vertices, &data.lower_lid_loop, "lower_lid")?;
    validate_loop(&data.vertices, &data.upper_lid_loop, "upper_lid")?;
    if data
        .subdivision_levels
        .iter()
        .any(|&level| level > SUBDIVISION_LEVEL_MAX)
    {
        return Err(FactoryError::BadParam(format!(
            "subdivision level above {SUBDIVISION_LEVEL_MAX} is not a real modifier level"
        )));
    }
    if data.uv_layers.iter().any(|name| name.is_empty()) {
        return Err(FactoryError::BadParam(
            "UV layer with an empty name is not a layer".to_string(),
        ));
    }
    Ok(())
}

fn canonical_coordinate_key(vertex: &[f32; 3]) -> [u32; 3] {
    vertex.map(|component| {
        // -0.0 and 0.0 are the same coordinate; canonicalize so the
        // duplicate scan compares positions, not sign bits.
        if component == 0.0 {
            0.0f32.to_bits()
        } else {
            component.to_bits()
        }
    })
}

fn loop_centroid_depth(vertices: &[[f32; 3]], loop_indices: &[usize]) -> f64 {
    let sum: f64 = loop_indices
        .iter()
        .map(|&index| f64::from(vertices[index][2]))
        .sum();
    sum / loop_indices.len() as f64
}

/// Census one head mesh against the three T-6 hygiene rules.
/// Fails closed (typed error) on structurally invalid input;
/// rule violations are report content, not errors.
pub fn census_head_mesh(data: &HeadMeshData) -> Result<BlendCensusReport, FactoryError> {
    validate_head_mesh_data(data)?;

    // Rule (a): exact-duplicate vertex coordinates.
    let mut coordinate_counts: BTreeMap<[u32; 3], usize> = BTreeMap::new();
    for vertex in &data.vertices {
        *coordinate_counts
            .entry(canonical_coordinate_key(vertex))
            .or_default() += 1;
    }
    let duplicate_vertex_groups = coordinate_counts
        .values()
        .filter(|&&count| count > 1)
        .count();
    let duplicate_vertex_excess: usize = coordinate_counts
        .values()
        .filter(|&&count| count > 1)
        .map(|&count| count - 1)
        .sum();
    let duplicates_clean = duplicate_vertex_groups == 0;

    // Rule (b): lower lid behind upper lid along +Z.
    let lower_depth = loop_centroid_depth(&data.vertices, &data.lower_lid_loop);
    let upper_depth = loop_centroid_depth(&data.vertices, &data.upper_lid_loop);
    let lid_order_clean = lower_depth < upper_depth;

    // Rule (c): the cage is exactly one level-1 modifier.
    let cage_level_one = data.subdivision_levels == [1];

    let checks = vec![
        ValidationCheck {
            detail: if duplicates_clean {
                format!(
                    "0 duplicate coordinates across {} vertices",
                    data.vertices.len()
                )
            } else {
                format!(
                    "{duplicate_vertex_groups} coordinates shared by multiple vertices ({duplicate_vertex_excess} excess vertices)"
                )
            },
            name: "duplicate_vertex_coordinates".to_string(),
            pass: duplicates_clean,
        },
        ValidationCheck {
            detail: format!(
                "lower lid centroid depth {lower_depth:.6} m, upper lid {upper_depth:.6} m (lower must be smaller)"
            ),
            name: "lower_lid_behind_upper_lid".to_string(),
            pass: lid_order_clean,
        },
        ValidationCheck {
            detail: format!(
                "recorded subdivision levels {:?} (cage discipline: exactly [1])",
                data.subdivision_levels
            ),
            name: "subdivision_cage_level".to_string(),
            pass: cage_level_one,
        },
    ];
    Ok(BlendCensusReport {
        pass: checks.iter().all(|check| check.pass),
        checks,
        face_plausibility: FacePlausibilityInputs {
            duplicate_vertices_clean: duplicates_clean,
            lid_depth_order_clean: lid_order_clean,
            subdivision_cage_level_one: cage_level_one,
            uv_layer_present: !data.uv_layers.is_empty(),
        },
        head: HeadCensusMeasurements {
            duplicate_vertex_excess,
            duplicate_vertex_groups,
            lower_lid_centroid_depth: lower_depth,
            subdivision_levels: data.subdivision_levels.clone(),
            upper_lid_centroid_depth: upper_depth,
        },
        schema: BLEND_CENSUS_SCHEMA.to_string(),
        uv_layer_count: data.uv_layers.len(),
        version: BLEND_CENSUS_VERSION,
    })
}

/// A texture step the checklist can gate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextureStep {
    /// Painted/AO permanent shadow (D-742): requires a UV layer.
    PermanentShadow,
}

impl TextureStep {
    /// The step's checklist name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::PermanentShadow => PERMANENT_SHADOW_STEP,
        }
    }
}

/// One step's preflight outcome: ready, or refused with the named
/// reason — decided before any work runs (D-742).
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct StepPreflight {
    /// The named refusal reason when `ready` is false.
    pub reason: Option<String>,
    /// True when the step's preconditions hold.
    pub ready: bool,
    /// The step's checklist name.
    pub step: String,
}

/// Preflight one texture step against a mesh's census data. The
/// permanent-shadow step requires a UV layer: without one it is
/// refused by name, never attempted.
pub fn preflight_texture_step(
    data: &HeadMeshData,
    step: TextureStep,
) -> Result<StepPreflight, FactoryError> {
    validate_head_mesh_data(data)?;
    match step {
        TextureStep::PermanentShadow => {
            let ready = !data.uv_layers.is_empty();
            Ok(StepPreflight {
                reason: if ready {
                    None
                } else {
                    Some(PERMANENT_SHADOW_UV_REASON.to_string())
                },
                ready,
                step: step.as_str().to_string(),
            })
        }
    }
}

/// Run one texture step's gate: `Ok` with the preflight when the
/// step may proceed, a typed refusal naming the failed
/// precondition when it may not. (The painting itself is a
/// downstream desk step; this is the checklist gate D-742
/// specifies, and it is the only path by which the step starts.)
pub fn run_texture_step(
    data: &HeadMeshData,
    step: TextureStep,
) -> Result<StepPreflight, FactoryError> {
    let preflight = preflight_texture_step(data, step)?;
    if preflight.ready {
        Ok(preflight)
    } else {
        Err(FactoryError::BadParam(
            preflight
                .reason
                .clone()
                .unwrap_or_else(|| "step refused without a reason".to_string()),
        ))
    }
}
