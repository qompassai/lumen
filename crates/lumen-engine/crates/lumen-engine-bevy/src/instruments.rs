//! Host-side measurement instruments for staged scenes:
//! the C2 triangle signed-margin metric and its helpers,
//! ported unchanged from the evaluator seam's two-body
//! scene driver (the D-857/D-875 instrument). These are
//! measurements over solved frames, computed by the host
//! — the engine core never sees them.

use lumen_engine_core::contact::{EvaluatedMesh, Placement};
use lumen_rig_core::Model;

/// One rig's evaluated meshes placed in the scene:
/// world-space triangles plus their AABB.
pub struct WorldMesh {
    /// ([min x, min y], [max x, max y]).
    pub aabb: ([f64; 2], [f64; 2]),
    /// World-space triangles.
    pub tris: Vec<[[f64; 2]; 3]>,
}

/// Place evaluated geometry by a solved placement.
pub fn world_meshes(
    model: &Model,
    evaluated: &[EvaluatedMesh],
    placement: &Placement,
) -> Vec<WorldMesh> {
    model
        .meshes
        .iter()
        .zip(evaluated.iter())
        .map(|(declared, eval)| {
            let world: Vec<[f64; 2]> = eval.vertices.iter().map(|v| placement.apply(*v)).collect();
            let mut lo = [f64::INFINITY; 2];
            let mut hi = [f64::NEG_INFINITY; 2];
            for v in &world {
                lo = [lo[0].min(v[0]), lo[1].min(v[1])];
                hi = [hi[0].max(v[0]), hi[1].max(v[1])];
            }
            let tris = declared
                .indices
                .as_chunks::<3>()
                .0
                .iter()
                .map(|tri| {
                    [
                        world[tri[0] as usize],
                        world[tri[1] as usize],
                        world[tri[2] as usize],
                    ]
                })
                .collect();
            WorldMesh {
                aabb: (lo, hi),
                tris,
            }
        })
        .collect()
}

/// Distance from a point to the closest point of a
/// segment.
pub fn point_segment_distance(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let ab = [b[0] - a[0], b[1] - a[1]];
    let len2 = ab[0] * ab[0] + ab[1] * ab[1];
    let f = if len2 > 0.0 {
        (((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1]) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let q = [a[0] + f * ab[0], a[1] + f * ab[1]];
    (p[0] - q[0]).hypot(p[1] - q[1])
}

/// Signed distance from a point to a triangle: negative
/// when the point lies inside.
pub fn point_triangle_signed(p: [f64; 2], tri: &[[f64; 2]; 3]) -> f64 {
    let cross = |a: [f64; 2], b: [f64; 2], q: [f64; 2]| {
        (b[0] - a[0]) * (q[1] - a[1]) - (b[1] - a[1]) * (q[0] - a[0])
    };
    let d0 = cross(tri[0], tri[1], p);
    let d1 = cross(tri[1], tri[2], p);
    let d2 = cross(tri[2], tri[0], p);
    let inside = (d0 >= 0.0 && d1 >= 0.0 && d2 >= 0.0) || (d0 <= 0.0 && d1 <= 0.0 && d2 <= 0.0);
    let dist = point_segment_distance(p, tri[0], tri[1])
        .min(point_segment_distance(p, tri[1], tri[2]))
        .min(point_segment_distance(p, tri[2], tri[0]));
    if inside {
        -dist
    } else {
        dist
    }
}

/// Minimum signed margin of `points` against the other
/// rig's triangle soup.
pub fn margin_of(points: &[[f64; 2]], others: &[WorldMesh]) -> f64 {
    let mut best = f64::INFINITY;
    for &p in points {
        for mesh in others {
            let (lo, hi) = mesh.aabb;
            let dx = (lo[0] - p[0]).max(p[0] - hi[0]).max(0.0);
            let dy = (lo[1] - p[1]).max(p[1] - hi[1]).max(0.0);
            if (dx * dx + dy * dy).sqrt() > best {
                continue;
            }
            for tri in &mesh.tris {
                let m = point_triangle_signed(p, tri);
                if m < best {
                    best = m;
                }
            }
        }
    }
    best
}

/// Leg parts (feet/shins/thighs), the floor-clearance set.
pub fn is_leg_part(id: &str) -> bool {
    id.starts_with("Foot") || id.starts_with("Shin") || id.starts_with("Thigh")
}

/// World-space vertices of every evaluated mesh,
/// optionally excluding the hand parts (the no-hands C2
/// variant).
pub fn points_of(
    evaluated: &[EvaluatedMesh],
    placement: &Placement,
    exclude_hands: bool,
) -> Vec<[f64; 2]> {
    evaluated
        .iter()
        .filter(|m| !exclude_hands || !(m.id == "HandL" || m.id == "HandR"))
        .flat_map(|m| m.vertices.iter().map(|v| placement.apply(*v)))
        .collect()
}

/// Distance from a point to an axis-aligned box (0 when
/// inside).
pub fn box_distance(p: [f64; 2], lo: [f64; 2], hi: [f64; 2]) -> f64 {
    let dx = (lo[0] - p[0]).max(p[0] - hi[0]).max(0.0);
    let dy = (lo[1] - p[1]).max(p[1] - hi[1]).max(0.0);
    dx.hypot(dy)
}

/// Nearest-rank percentile of a sorted series.
pub fn percentile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let index = ((sorted.len() - 1) as f64 * q).round() as usize;
    sorted[index]
}
