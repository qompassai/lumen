//! `normals-from-proxy/1` (T-12, D-741): transfer custom normals
//! from a sculpted proxy mesh onto a target mesh — the Rust
//! implementation of the Blender desk recipe (Data Transfer
//! modifier: Face Corner Data → Custom Normals, mapping
//! `POLYINTERP_NEAREST`, "Nearest Face Interpolated",
//! vertex-group scoped), with the desk script beside it in
//! `blender/normals_from_proxy.py`.
//!
//! Contract summary:
//! - For every target vertex the converter finds the closest
//!   point on the proxy's triangles (degenerate triangles
//!   skipped; ties break to the lowest face index, so the result
//!   is deterministic) and interpolates the proxy's *vertex
//!   normals* at that point by barycentric weight — smooth
//!   normals from a smooth source, never a faceted face normal.
//! - Scoping is per-vertex weights in `[0, 1]` (the vertex
//!   group): 0 keeps the target's normal exactly, 1 takes the
//!   transferred normal, values between blend and renormalize.
//! - Modes: [`NormalsMode::Direct`] refuses a skinned target
//!   (custom normals must never be written through an armature
//!   deformation the transfer did not see);
//!   [`NormalsMode::RigSafeStaticDuplicate`] is the rig-safe
//!   variant: the transfer is computed against the static proxy
//!   and only normals are written back — joints and weights are
//!   never touched, and the report proves it by comparison
//!   (`skin_preserved`).
//! - Every target part must name a proxy part and a scope entry;
//!   unmatched parts or scopes refuse by name rather than passing
//!   through silently.
//! - The converter's own contract ([`contract`]) carries the
//!   stage-1 clauses: GLB export with read-back verification
//!   required (T-4); it performs no boolean union (T-2 clause
//!   absent).

#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use crate::FactoryError;
use crate::base::{BaseMesh, MeshPart};
use crate::convert::contract::{ConverterContract, ExportFormat, ReadBackRequirement};

/// This converter's registry id.
pub const CONVERTER_ID: &str = "normals-from-proxy/1";

/// Smallest squared interpolated-normal length the transfer will
/// accept before falling back to the target's own normal
/// (opposing proxy normals can cancel to ~zero at a seam).
const MIN_TRANSFER_LENGTH_SQ: f32 = 1e-12;
/// Smallest squared triangle area scale treated as a real face;
/// below it the triangle is degenerate and skipped.
const MIN_TRIANGLE_AREA_SQ: f32 = 1e-12;

/// How the transfer treats skinning (see module docs).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NormalsMode {
    /// Refuse skinned targets outright.
    Direct,
    /// Transfer against the static proxy and write back only
    /// normals; the skin is preserved by construction and proven
    /// by comparison in the report.
    RigSafeStaticDuplicate,
}

impl NormalsMode {
    /// The mode's record name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::RigSafeStaticDuplicate => "rig_safe_static_duplicate",
        }
    }
}

/// The outcome of one converter run.
#[derive(Clone, Debug, PartialEq)]
pub struct NormalsReport {
    /// The mode the run executed under.
    pub mode: NormalsMode,
    /// Parts transferred (one per target part).
    pub parts_transferred: usize,
    /// True iff every output joint/weight array is identical to
    /// the input's (always required in rig-safe mode; in direct
    /// mode the target was unskinned, so this is trivially true).
    pub skin_preserved: bool,
    /// Vertices whose scope weight was > 0 (blended or replaced).
    pub vertices_blended: usize,
    /// Vertices examined across all parts.
    pub vertices_considered: usize,
    /// Vertices whose scope weight was 0 or whose transfer fell
    /// back to the original normal.
    pub vertices_unchanged: usize,
}

/// This converter's stage-1 contract record.
pub fn contract() -> ConverterContract {
    ConverterContract {
        converter_id: CONVERTER_ID.to_string(),
        export_format: ExportFormat::Glb,
        read_back: ReadBackRequirement::Required,
        union: None,
    }
}

fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn closest_point_on_triangle(p: [f32; 3], a: [f32; 3], b: [f32; 3], c: [f32; 3]) -> [f32; 3] {
    // Ericson's closest-point-on-triangle: region tests against
    // the Voronoi regions of the triangle's vertices and edges.
    let ab = sub(b, a);
    let ac = sub(c, a);
    let ap = sub(p, a);
    let d1 = dot(ab, ap);
    let d2 = dot(ac, ap);
    if d1 <= 0.0 && d2 <= 0.0 {
        return [1.0, 0.0, 0.0];
    }
    let bp = sub(p, b);
    let d3 = dot(ab, bp);
    let d4 = dot(ac, bp);
    if d3 >= 0.0 && d4 <= d3 {
        return [0.0, 1.0, 0.0];
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        let v = d1 / (d1 - d3);
        return [1.0 - v, v, 0.0];
    }
    let cp = sub(p, c);
    let d5 = dot(ab, cp);
    let d6 = dot(ac, cp);
    if d6 >= 0.0 && d5 <= d6 {
        return [0.0, 0.0, 1.0];
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        let w = d2 / (d2 - d6);
        return [1.0 - w, 0.0, w];
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        let w = (d4 - d3) / ((d4 - d3) + (d5 - d6));
        return [0.0, 1.0 - w, w];
    }
    let denom = 1.0 / (va + vb + vc);
    let v = vb * denom;
    let w = vc * denom;
    [1.0 - v - w, v, w]
}

fn transfer_normal(
    position: [f32; 3],
    original: [f32; 3],
    weight: f32,
    proxy: &MeshPart,
) -> Result<([f32; 3], bool), FactoryError> {
    let mut best: Option<(f32, [f32; 3])> = None;
    for triangle in proxy.indices.as_chunks::<3>().0 {
        let [ia, ib, ic] = *triangle;
        let (a, b, c) = (
            proxy.vertices[ia as usize],
            proxy.vertices[ib as usize],
            proxy.vertices[ic as usize],
        );
        let face = cross(sub(b.position, a.position), sub(c.position, a.position));
        if dot(face, face) < MIN_TRIANGLE_AREA_SQ {
            continue; // Degenerate proxy face: no surface to sample.
        }
        let barycentric = closest_point_on_triangle(position, a.position, b.position, c.position);
        let point = [
            barycentric[0] * a.position[0]
                + barycentric[1] * b.position[0]
                + barycentric[2] * c.position[0],
            barycentric[0] * a.position[1]
                + barycentric[1] * b.position[1]
                + barycentric[2] * c.position[1],
            barycentric[0] * a.position[2]
                + barycentric[1] * b.position[2]
                + barycentric[2] * c.position[2],
        ];
        let delta = sub(position, point);
        let distance_sq = dot(delta, delta);
        // Strictly-smaller wins: the lowest face index takes ties,
        // so the transfer is deterministic.
        if best.is_none_or(|(best_distance, _)| distance_sq < best_distance) {
            let interpolated = [
                barycentric[0] * a.normal[0]
                    + barycentric[1] * b.normal[0]
                    + barycentric[2] * c.normal[0],
                barycentric[0] * a.normal[1]
                    + barycentric[1] * b.normal[1]
                    + barycentric[2] * c.normal[1],
                barycentric[0] * a.normal[2]
                    + barycentric[1] * b.normal[2]
                    + barycentric[2] * c.normal[2],
            ];
            best = Some((distance_sq, interpolated));
        }
    }
    let Some((_, interpolated)) = best else {
        return Err(FactoryError::MeshInvalid(format!(
            "proxy part '{}' has no non-degenerate triangle to transfer from",
            proxy.name
        )));
    };
    // The transferred normal is the barycentric interpolation,
    // normalized — Blender's custom normals are unit vectors, and
    // the interpolation of unit normals is only a direction once
    // renormalized. If the interpolation cancels to ~zero
    // (opposing proxy normals at a seam) there is no direction to
    // transfer: keep the target's own normal.
    let interpolated_length_sq = dot(interpolated, interpolated);
    if interpolated_length_sq < MIN_TRANSFER_LENGTH_SQ {
        return Ok((original, false));
    }
    let interpolated_length = interpolated_length_sq.sqrt();
    let transferred = [
        interpolated[0] / interpolated_length,
        interpolated[1] / interpolated_length,
        interpolated[2] / interpolated_length,
    ];
    let blended = [
        original[0] + (transferred[0] - original[0]) * weight,
        original[1] + (transferred[1] - original[1]) * weight,
        original[2] + (transferred[2] - original[2]) * weight,
    ];
    let length_sq = dot(blended, blended);
    if length_sq < MIN_TRANSFER_LENGTH_SQ {
        // Opposing normals cancelled: keep the target's own normal
        // rather than write a zero or NaN normal.
        return Ok((original, false));
    }
    let length = length_sq.sqrt();
    Ok((
        [
            blended[0] / length,
            blended[1] / length,
            blended[2] / length,
        ],
        true,
    ))
}

fn validate_part(part: &MeshPart, role: &str) -> Result<(), FactoryError> {
    if part.vertices.is_empty() {
        return Err(FactoryError::BadParam(format!(
            "{role} part '{}' has no vertices",
            part.name
        )));
    }
    if !part.indices.len().is_multiple_of(3) {
        return Err(FactoryError::BadParam(format!(
            "{role} part '{}' index count is not a multiple of three",
            part.name
        )));
    }
    let bound = u32::try_from(part.vertices.len()).unwrap_or(u32::MAX);
    if part.indices.iter().any(|&index| index >= bound) {
        return Err(FactoryError::BadParam(format!(
            "{role} part '{}' has an out-of-range index",
            part.name
        )));
    }
    if part.vertices.iter().any(|vertex| {
        vertex
            .position
            .iter()
            .chain(vertex.normal.iter())
            .any(|component| !component.is_finite())
    }) {
        return Err(FactoryError::BadParam(format!(
            "{role} part '{}' carries a non-finite position or normal",
            part.name
        )));
    }
    Ok(())
}

/// Run `normals-from-proxy/1`: transfer normals from `proxy` onto
/// a clone of `target` (the inputs are never mutated), scoped per
/// part by `scopes` (part name → one weight per target vertex).
/// The output mesh is ready for GLB export and the stage-1
/// read-back census (see [`contract`]).
pub fn run_normals_from_proxy(
    target: &BaseMesh,
    proxy: &BaseMesh,
    scopes: &BTreeMap<String, Vec<f32>>,
    mode: NormalsMode,
) -> Result<(BaseMesh, NormalsReport), FactoryError> {
    contract().validate()?;
    if target.parts.is_empty() {
        return Err(FactoryError::BadParam(
            "target mesh has no parts".to_string(),
        ));
    }
    let skinned = !target.joints.is_empty()
        || target
            .parts
            .iter()
            .flat_map(|part| &part.vertices)
            .any(|vertex| vertex.weights.iter().any(|&weight| weight != 0.0));
    if mode == NormalsMode::Direct && skinned {
        return Err(FactoryError::BadParam(
            "direct normals transfer refuses a skinned target; use the rig-safe static-duplicate mode"
                .to_string(),
        ));
    }
    for key in scopes.keys() {
        if !target.parts.iter().any(|part| &part.name == key) {
            return Err(FactoryError::BadParam(format!(
                "scope names part '{key}', which the target does not have"
            )));
        }
    }
    let mut output = target.clone();
    let mut report = NormalsReport {
        mode,
        parts_transferred: 0,
        skin_preserved: false,
        vertices_blended: 0,
        vertices_considered: 0,
        vertices_unchanged: 0,
    };
    for part in &mut output.parts {
        let proxy_part = proxy
            .parts
            .iter()
            .find(|candidate| candidate.name == part.name)
            .ok_or_else(|| {
                FactoryError::BadParam(format!(
                    "proxy has no part named '{}' to transfer from",
                    part.name
                ))
            })?;
        let scope = scopes.get(&part.name).ok_or_else(|| {
            FactoryError::BadParam(format!("no scope weights given for part '{}'", part.name))
        })?;
        if scope.len() != part.vertices.len() {
            return Err(FactoryError::BadParam(format!(
                "scope for part '{}' has {} weights for {} vertices",
                part.name,
                scope.len(),
                part.vertices.len()
            )));
        }
        if scope
            .iter()
            .any(|&weight| !weight.is_finite() || !(0.0..=1.0).contains(&weight))
        {
            return Err(FactoryError::BadParam(format!(
                "scope for part '{}' carries a weight outside [0, 1]",
                part.name
            )));
        }
        validate_part(part, "target")?;
        validate_part(proxy_part, "proxy")?;
        for (vertex, &weight) in part.vertices.iter_mut().zip(scope.iter()) {
            report.vertices_considered += 1;
            if weight == 0.0 {
                report.vertices_unchanged += 1;
                continue;
            }
            let (normal, changed) =
                transfer_normal(vertex.position, vertex.normal, weight, proxy_part)?;
            vertex.normal = normal;
            if changed {
                report.vertices_blended += 1;
            } else {
                report.vertices_unchanged += 1;
            }
        }
        report.parts_transferred += 1;
    }
    report.skin_preserved = output.joints.len() == target.joints.len()
        && output
            .parts
            .iter()
            .zip(target.parts.iter())
            .all(|(out, in_)| {
                out.vertices
                    .iter()
                    .zip(in_.vertices.iter())
                    .all(|(out_v, in_v)| {
                        out_v.joints == in_v.joints && out_v.weights == in_v.weights
                    })
            });
    if mode == NormalsMode::RigSafeStaticDuplicate && !report.skin_preserved {
        return Err(FactoryError::MeshInvalid(
            "rig-safe transfer altered skin data".to_string(),
        ));
    }
    Ok((output, report))
}
