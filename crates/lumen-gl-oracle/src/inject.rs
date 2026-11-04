//! Lumen → inox2d node pairing and the branch-1 vertex injection.
//!
//! The renderer draws `camera · A_inox · (v_rest + d)` (basic.vert;
//! the `offset` uniform is never set by inox2d-opengl, and every
//! corpus mesh origin is zero). Only `d` is writable
//! (`RenderCtx.vertex_buffers.deforms`, re-uploaded per frame,
//! lib.rs:638). So `d` is solved from lumen's world-space vertex
//! `w`: `xy(A_inox · (v_rest + d)) = w`. The projection is
//! orthographic, so only A's upper-left 2×2 and translation reach
//! screen xy; the 2×2 solve is done in f64 and `d` rounded once to
//! f32 — the named cost of the route. The f32 re-projection residual
//! is measured per frame and reported.

use inox2d::node::InoxNodeUuid;
use inox2d::node::components::TransformStore;
use inox2d::puppet::Puppet;
use inox2d::render::TexturedMeshRenderCtx;
use lumen_rig_core::{OracleState, PuppetStage};

/// Determinant below which A's 2×2 is treated as singular (a part
/// scaled to nothing): its vertices cannot be solved for.
const DETERMINANT_MIN: f64 = 1e-12;
/// Pairing proof: a paired part's rest vertices must agree to this
/// (puppet px). They need not be bit-equal — the importers round
/// the INP JSON decimals independently — and the injection solves
/// against inox2d's own rest vertex, so the bound is a mapping check,
/// not a precision claim.
const REST_DELTA_MAX: f32 = 1e-2;

#[derive(Debug)]
pub enum OracleError {
    Degenerate { node: String },
    Mismatch { context: String },
}

impl std::fmt::Display for OracleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OracleError::Degenerate { node } => write!(f, "singular transform on part {node}"),
            OracleError::Mismatch { context } => write!(f, "lumen/inox2d mismatch: {context}"),
        }
    }
}

impl std::error::Error for OracleError {}

/// Per-frame injection statistics (puppet-space pixels).
#[derive(Debug, Default)]
pub struct InjectStats {
    /// Max |w_lumen − w_inox| before injection: how far the two
    /// evaluators' geometry differs on this frame.
    pub eval_delta_max: f32,
    pub parts: usize,
    /// Max |xy(A_inox·(v+d)) − w_lumen| re-projected in f32 after
    /// injection: the route's own rounding cost.
    pub residual_max: f32,
    /// Max |rest_inox − rest_lumen| over paired vertices.
    pub rest_delta_max: f32,
    pub vertices: usize,
}

/// Pair every lumen stage node with its inox2d node: roots together,
/// then children in order (both keep asset order), checking original
/// names and the node count. Returns the inox uuid per stage index.
pub fn pair_nodes(stage: &PuppetStage, puppet: &Puppet) -> Result<Vec<InoxNodeUuid>, OracleError> {
    let tree = puppet.nodes();
    let original = |id: &str| -> String {
        let renamed = stage
            .provenance
            .as_ref()
            .and_then(|p| p.names.iter().find(|pair| pair[0] == id));
        renamed
            .map_or(id, |pair| pair[1].as_str())
            .trim_end_matches('\0')
            .to_string()
    };
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); stage.nodes.len()];
    let mut root = None;
    for (i, node) in stage.nodes.iter().enumerate() {
        match &node.parent {
            Some(p) => {
                let pi = stage.nodes.iter().position(|n| &n.id == p).ok_or_else(|| {
                    OracleError::Mismatch {
                        context: format!("dangling parent {p}"),
                    }
                })?;
                children[pi].push(i);
            }
            None => root = Some(i),
        }
    }
    let root = root.ok_or(OracleError::Mismatch {
        context: "no root".into(),
    })?;
    let mut map: Vec<Option<InoxNodeUuid>> = vec![None; stage.nodes.len()];
    let mut stack = vec![(root, tree.root_node_id)];
    while let Some((ni, uuid)) = stack.pop() {
        let inox = tree.get_node(uuid).ok_or(OracleError::Mismatch {
            context: "inox uuid".into(),
        })?;
        let want = original(&stage.nodes[ni].id);
        if inox.name.trim_end_matches('\0') != want {
            return Err(OracleError::Mismatch {
                context: format!("node {want:?} paired with inox {:?}", inox.name),
            });
        }
        map[ni] = Some(uuid);
        let inox_kids: Vec<InoxNodeUuid> = tree.get_children(uuid).map(|n| n.uuid).collect();
        if inox_kids.len() != children[ni].len() {
            return Err(OracleError::Mismatch {
                context: format!("child count of {want:?}"),
            });
        }
        stack.extend(children[ni].iter().copied().zip(inox_kids));
    }
    map.into_iter()
        .collect::<Option<Vec<_>>>()
        .ok_or(OracleError::Mismatch {
            context: "unreachable stage node".into(),
        })
}

/// Measure, and if `write` overwrite, every part's deform slice so
/// the renderer draws lumen's world-space vertices.
pub fn inject(
    stage: &PuppetStage,
    map: &[InoxNodeUuid],
    state: &OracleState,
    puppet: &mut Puppet,
    write: bool,
) -> Result<InjectStats, OracleError> {
    let mut stats = InjectStats::default();
    let mut writes: Vec<(usize, glam::Vec2)> = Vec::new();
    {
        let comps = puppet.node_comps();
        let buffers = &puppet
            .render_ctx
            .as_ref()
            .ok_or(OracleError::Mismatch {
                context: "render ctx".into(),
            })?
            .vertex_buffers;
        for (ni, node) in stage.nodes.iter().enumerate() {
            let world = &state.nodes[ni].world_verts;
            let Some(mesh) = node.mesh.as_ref().filter(|_| !world.is_empty()) else {
                continue;
            };
            let missing = || OracleError::Mismatch {
                context: format!("components of {}", node.id),
            };
            let rc = comps
                .get::<TexturedMeshRenderCtx>(map[ni])
                .ok_or_else(missing)?;
            let a = comps
                .get::<TransformStore>(map[ni])
                .ok_or_else(missing)?
                .absolute;
            let off = rc.vert_offset as usize;
            if rc.vert_len != world.len() || rc.vert_len != mesh.vertices.len() {
                return Err(OracleError::Mismatch {
                    context: format!("vertex count of {}", node.id),
                });
            }
            let solve = Solver::new(&a).ok_or(OracleError::Degenerate {
                node: node.id.clone(),
            })?;
            for (i, w) in world.iter().enumerate() {
                let rest = buffers.verts[off + i];
                let rest_delta = (rest - glam::Vec2::from_array(mesh.vertices[i]))
                    .abs()
                    .max_element();
                if rest_delta > REST_DELTA_MAX {
                    return Err(OracleError::Mismatch {
                        context: format!(
                            "rest vertex {i} of {}: inox {rest} lumen {:?}",
                            node.id, mesh.vertices[i]
                        ),
                    });
                }
                stats.rest_delta_max = stats.rest_delta_max.max(rest_delta);
                let w = glam::Vec2::from_array(*w);
                let inox_w = project(&a, rest + buffers.deforms[off + i]);
                stats.eval_delta_max = stats.eval_delta_max.max((w - inox_w).abs().max_element());
                let d = solve.local(w) - rest.as_dvec2();
                let d = d.as_vec2();
                stats.residual_max = stats
                    .residual_max
                    .max((project(&a, rest + d) - w).abs().max_element());
                writes.push((off + i, d));
            }
            stats.parts += 1;
            stats.vertices += world.len();
        }
    }
    if write {
        let deforms = &mut puppet
            .render_ctx
            .as_mut()
            .ok_or(OracleError::Mismatch {
                context: "render ctx".into(),
            })?
            .vertex_buffers
            .deforms;
        for (at, d) in writes {
            deforms[at] = d;
        }
    }
    Ok(stats)
}

/// Screen-plane image of a local point under A (z = 0, w = 1), in f32
/// as the vertex shader computes it.
fn project(a: &glam::Mat4, p: glam::Vec2) -> glam::Vec2 {
    let q = *a * p.extend(0.0).extend(1.0);
    glam::Vec2::new(q.x, q.y)
}

/// f64 inverse of A's screen-plane affine part.
struct Solver {
    inv: glam::DMat2,
    t: glam::DVec2,
}

impl Solver {
    fn new(a: &glam::Mat4) -> Option<Solver> {
        let m = glam::DMat2::from_cols(
            glam::DVec2::new(f64::from(a.x_axis.x), f64::from(a.x_axis.y)),
            glam::DVec2::new(f64::from(a.y_axis.x), f64::from(a.y_axis.y)),
        );
        if m.determinant().abs() < DETERMINANT_MIN {
            return None;
        }
        Some(Solver {
            inv: m.inverse(),
            t: glam::DVec2::new(f64::from(a.w_axis.x), f64::from(a.w_axis.y)),
        })
    }

    fn local(&self, w: glam::Vec2) -> glam::DVec2 {
        self.inv * (w.as_dvec2() - self.t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn solve_round_trips_a_rotated_scaled_transform() {
        let a = glam::Mat4::from_scale_rotation_translation(
            glam::Vec3::new(1.7, 0.6, 1.0),
            glam::Quat::from_rotation_z(0.83),
            glam::Vec3::new(-312.5, 1044.25, 0.0),
        );
        let solver = Solver::new(&a).expect("invertible");
        for &(x, y) in &[(0.0, 0.0), (-812.3, 77.1), (1500.0, -2200.5)] {
            let w = glam::Vec2::new(x, y);
            let back = project(&a, solver.local(w).as_vec2());
            assert!((back - w).abs().max_element() < 1e-3, "{back} vs {w}");
        }
    }

    #[test]
    fn solve_refuses_a_collapsed_transform() {
        let a = glam::Mat4::from_scale(glam::Vec3::new(0.0, 1.0, 1.0));
        assert!(Solver::new(&a).is_none());
    }
}
