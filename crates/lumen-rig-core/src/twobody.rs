//! Two-body scenes: two (or more) validated rigs evaluated in
//! one scene, with attachment contacts solved inside the
//! fixed-step evaluation and every violation surfaced as data.
//!
//! This module is the geometry and contact layer the format
//! deferred (SPEC.md: physics evaluation §5 and rendering are
//! later stages). It implements the geometry semantics SPEC
//! §3.4/§3.7 already pin — lattice payloads are absolute states,
//! interpolation is linear/bilinear in f64, deformer chains
//! apply root-first, a warp group deforms descendant vertices
//! and descendant deformer origins — and adds the two-body
//! contract on top:
//!
//! - A [`Scene`] is validated rigs plus declared contacts and
//!   penetration probes. Construction proves every reference
//!   resolves, refuses self-contact, and refuses driven-rig
//!   cycles, each with a typed [`TwoBodyError`].
//! - Evaluation is a pure function of the scene, the per-rig
//!   parameter values, the authored placements, and the active
//!   contact set. No wall-clock, no map iteration (all maps are
//!   `BTreeMap`), no platform intrinsics beyond f64 arithmetic.
//! - The solver runs a FIXED number of sweeps (never a
//!   convergence-dependent count), in declared contact order,
//!   so a recorded step sequence replays byte-identically.
//! - Contact slip and penetration margins are returned per
//!   frame in [`FrameRead`] — violations are data, never
//!   silently clamped, exactly as the format treats its own
//!   evaluation clamps (§9.3 lives in hosts, not here).

use std::collections::BTreeMap;
use std::fmt;

use serde::Serialize;

use crate::model::{Deformer, Lattice, Mesh, Model, Parameter};

/// Maximum contacts in one scene.
pub const CONTACTS_MAX: usize = 64;
/// Maximum penetration probes in one scene.
pub const PROBES_MAX: usize = 256;
/// Maximum rigs in one scene.
pub const RIGS_MAX: usize = 8;

/// Fixed Gauss-Seidel sweeps per driven rig per frame. Part of
/// the determinism contract: the count never depends on the
/// data, so two runs over the same inputs produce the same bits.
pub const SOLVER_SWEEPS: usize = 8;
/// Fixed separation passes after the attachment sweeps.
pub const SEPARATION_PASSES: usize = 4;
/// Per-sweep rotation clamp, in degrees: one sweep may not
/// swing a driven rig further than this, so a contradictory
/// contact set degrades into reported slip, not a snap.
pub const ROTATION_STEP_MAX_DEGREES: f64 = 10.0;
/// Newton iteration cap for the warp grid inverse (§3.4's grid
/// is inverted per point; the count is fixed for determinism).
pub const WARP_INVERSE_ITERATIONS: usize = 8;
/// Slip at or below this many model units counts as converged
/// for a contact (the algebraic floor of an exactly determined
/// attachment; multi-contact residuals report above it).
pub const SLIP_CONVERGED_MAX: f64 = 1e-6;

/// A twobody failure: a stable `LTB-` code plus a message.
/// Construction-time refusals only — evaluation itself is
/// total over a validated scene and reports violations as
/// data in [`FrameRead`].
#[derive(Debug, Clone, PartialEq)]
pub struct TwoBodyError {
    /// Stable machine-readable code (`LTB-1xx`).
    pub code: &'static str,
    /// Human-readable detail.
    pub message: String,
}

impl TwoBodyError {
    fn new(code: &'static str, message: String) -> TwoBodyError {
        TwoBodyError { code, message }
    }
}

impl fmt::Display for TwoBodyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for TwoBodyError {}

/// A rig's placement in scene space: a rotation about the rig
/// origin (model-space `(0, 0)`) followed by a translation.
/// All values are f64 and finite (validated at scene input).
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct Placement {
    /// Rotation in degrees, counter-clockwise.
    pub rotation_degrees: f64,
    /// Translation in model units.
    pub translation: [f64; 2],
}

impl Placement {
    /// The identity placement.
    pub const IDENTITY: Placement = Placement {
        rotation_degrees: 0.0,
        translation: [0.0, 0.0],
    };

    /// Apply the placement to a model-space point.
    pub fn apply(&self, point: [f64; 2]) -> [f64; 2] {
        let radians = self.rotation_degrees.to_radians();
        let (sin, cos) = radians.sin_cos();
        [
            self.translation[0] + cos * point[0] - sin * point[1],
            self.translation[1] + sin * point[0] + cos * point[1],
        ]
    }
}

/// A named point on a rig, resolved against evaluated geometry.
#[derive(Debug, Clone, PartialEq)]
pub enum ContactEndpoint {
    /// The centroid of the evaluated vertices of every mesh
    /// declared on this part.
    PartCentroid { part: String },
    /// One vertex of one mesh, by index.
    Vertex { mesh: String, vertex: u32 },
}

/// One attachment contact between two rigs. While active, the
/// solver moves `driven_rig`'s placement so the `a` endpoint
/// coincides with a point fixed on the `b` rig: the `b`
/// endpoint plus `offset`, the offset expressed in the `b`
/// endpoint's deformed local frame (it rotates with the
/// endpoint's deformer chain). Two plainer definitions were
/// measured and rejected on the grasp scene: a scene-axes
/// offset breaks the contact's rigidity when the driver
/// rotates (0.585-unit hold slip no solver step could
/// remove), and a model-axes offset slides around the
/// endpoint when the endpoint's own chain deforms (0.060).
#[derive(Debug, Clone, PartialEq)]
pub struct ContactDef {
    /// The endpoint on the (usually driving) rig `a_rig`.
    pub a: ContactEndpoint,
    /// Which rig `a` lives on.
    pub a_rig: usize,
    /// The endpoint on the (usually driven) rig `b_rig`.
    pub b: ContactEndpoint,
    /// Which rig `b` lives on.
    pub b_rig: usize,
    /// The rig whose placement the solver adjusts: either
    /// `a_rig` or `b_rig`. The other rig is the contact's
    /// reference for this frame.
    pub driven_rig: usize,
    /// Contact id (unique per scene).
    pub id: String,
    /// Offset of the contact point from the `b` endpoint,
    /// in the endpoint's deformed local frame: the offset
    /// rotates with the endpoint's deformer chain (rotation
    /// deformers only; a warp in the chain contributes no
    /// rotation - stated approximation). For a centroid
    /// endpoint the frame is the chain rotation of the
    /// part's first declared mesh.
    pub offset: [f64; 2],
}

/// One penetration probe: a vertex of `rig` tested against the
/// evaluated mesh polygons of `against_rig`.
#[derive(Debug, Clone, PartialEq)]
pub struct ProbeDef {
    /// The rig whose polygons the point is tested against.
    pub against_rig: usize,
    /// Probe id (unique per scene).
    pub id: String,
    /// The mesh carrying the probe vertex.
    pub mesh: String,
    /// The rig carrying the probe vertex.
    pub rig: usize,
    /// The probe vertex index.
    pub vertex: u32,
}

/// Resolve a model's full parameter state: driven values win
/// for the parameters they name; scalars otherwise hold their
/// defaults; combined parameters evaluate their §3.1 formula
/// over resolved terms unless a driven value names them.
/// Driven values for unknown parameters, and non-finite driven
/// values, are refused (`LTB-105` / `LTB-104`).
pub fn resolve_parameters(
    model: &Model,
    driven: &BTreeMap<String, f64>,
) -> Result<BTreeMap<String, f64>, TwoBodyError> {
    for (id, value) in driven {
        if !value.is_finite() {
            return Err(TwoBodyError::new(
                "LTB-104",
                format!("driven value for parameter {id} is not finite: {value}"),
            ));
        }
        if !model.parameters.iter().any(|p| p.id() == id) {
            return Err(TwoBodyError::new(
                "LTB-105",
                format!("driven value names unknown parameter {id}"),
            ));
        }
    }
    let mut resolved: BTreeMap<String, f64> = BTreeMap::new();
    for parameter in &model.parameters {
        resolve_one(model, parameter.id(), driven, &mut resolved);
    }
    Ok(resolved)
}

/// Resolve one parameter, recursing into combined terms. The
/// validator proved the combined graph acyclic (§3.1), so the
/// recursion terminates; depth is bounded by PARAMETERS_MAX.
fn resolve_one(
    model: &Model,
    id: &str,
    driven: &BTreeMap<String, f64>,
    resolved: &mut BTreeMap<String, f64>,
) -> f64 {
    if let Some(value) = resolved.get(id) {
        return *value;
    }
    if let Some(value) = driven.get(id) {
        resolved.insert(id.to_string(), *value);
        return *value;
    }
    let parameter = model
        .parameters
        .iter()
        .find(|p| p.id() == id)
        .expect("resolve_one is only called with model parameter ids");
    let value = match parameter {
        Parameter::Scalar(scalar) => f64::from(scalar.default),
        Parameter::Combined(combined) => {
            let mut sum = f64::from(combined.offset);
            for term in &combined.terms {
                sum +=
                    f64::from(term.weight) * resolve_one(model, &term.parameter, driven, resolved);
            }
            sum
        }
    };
    resolved.insert(id.to_string(), value);
    value
}

/// One evaluated mesh: model-space vertices after keyform
/// interpolation and the full deformer chain.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EvaluatedMesh {
    /// Draw order (the format's total order is `(draw_order, id)`).
    pub draw_order: i32,
    /// Mesh id.
    pub id: String,
    /// The part the mesh is declared on.
    pub part: String,
    /// Evaluated vertices, model space, declaration order.
    pub vertices: Vec<[f64; 2]>,
}

/// Lattice sample weights: for each axis, the lower key index
/// and the fraction toward the next key. Parameter values clamp
/// to the outermost keys (§3.7 interpolation is defined between
/// lattice points; outside them the nearest form holds).
fn lattice_weights<F>(lattice: &Lattice<F>, params: &BTreeMap<String, f64>) -> Vec<(usize, f64)> {
    lattice
        .axes
        .iter()
        .zip(lattice.keys.iter())
        .map(|(axis, keys)| {
            let value = params.get(axis).copied().unwrap_or(0.0);
            if value <= f64::from(keys[0]) {
                return (0, 0.0);
            }
            let last = keys.len() - 1;
            if value >= f64::from(keys[last]) {
                return (last - 1, 1.0);
            }
            let mut segment = 0;
            while value >= f64::from(keys[segment + 1]) {
                segment += 1;
            }
            let lo = f64::from(keys[segment]);
            let hi = f64::from(keys[segment + 1]);
            (segment, (value - lo) / (hi - lo))
        })
        .collect()
}

/// The lattice corners touched by a sample: (form index,
/// weight) pairs in canonical form order (§3.7: first axis
/// slowest, so a 2-axis form index is `i * keys[1].len() + j`).
/// Weights sum to 1; zero-weight corners are included so the
/// fold order is data-independent.
fn lattice_corners<F>(lattice: &Lattice<F>, weights: &[(usize, f64)]) -> Vec<(usize, f64)> {
    match weights {
        [(segment, fraction)] => vec![(*segment, 1.0 - fraction), (segment + 1, *fraction)],
        [(seg0, frac0), (seg1, frac1)] => {
            let row = lattice.keys[1].len();
            vec![
                (seg0 * row + seg1, (1.0 - frac0) * (1.0 - frac1)),
                (seg0 * row + seg1 + 1, (1.0 - frac0) * frac1),
                ((seg0 + 1) * row + seg1, frac0 * (1.0 - frac1)),
                ((seg0 + 1) * row + seg1 + 1, frac0 * frac1),
            ]
        }
        _ => unreachable!("a validated lattice carries one or two axes"),
    }
}

/// Interpolate a list-of-points payload (mesh vertices, warp
/// control points) over a lattice.
fn sample_points<F>(
    lattice: &Lattice<F>,
    params: &BTreeMap<String, f64>,
    points_of: impl Fn(&F) -> &Vec<[f32; 2]>,
) -> Vec<[f64; 2]> {
    let weights = lattice_weights(lattice, params);
    let corners = lattice_corners(lattice, &weights);
    let count = points_of(&lattice.forms[0]).len();
    let mut result = vec![[0.0, 0.0]; count];
    for (index, weight) in corners {
        if weight == 0.0 {
            continue;
        }
        for (slot, point) in result.iter_mut().zip(points_of(&lattice.forms[index])) {
            slot[0] += weight * f64::from(point[0]);
            slot[1] += weight * f64::from(point[1]);
        }
    }
    result
}

/// A rotation deformer's evaluated state: the rotation the
/// deformer applies to its subtree (its absolute angle minus
/// its base angle, §3.4) and its scale.
#[derive(Debug, Clone, Copy)]
struct RotationState {
    delta_degrees: f64,
    scale: [f64; 2],
}

/// A deformer's evaluated state, ready to transform points.
#[derive(Debug, Clone)]
enum DeformerState {
    Rotation(RotationState),
    Warp(Vec<[f64; 2]>),
}

/// Per-evaluation deformer context: evaluated states plus the
/// memo of ancestor-transformed origins.
struct DeformerContext<'a> {
    model: &'a Model,
    origins: BTreeMap<String, [f64; 2]>,
    states: BTreeMap<String, DeformerState>,
}

impl<'a> DeformerContext<'a> {
    fn new(model: &'a Model, params: &BTreeMap<String, f64>) -> DeformerContext<'a> {
        let mut states = BTreeMap::new();
        for deformer in &model.deformers {
            let state = match deformer {
                Deformer::Rotation(rotation) => {
                    let (angle, scale) = match &rotation.keyforms {
                        Some(lattice) => {
                            let weights = lattice_weights(lattice, params);
                            let corners = lattice_corners(lattice, &weights);
                            let (mut angle, mut scale) = (0.0, [0.0, 0.0]);
                            for (index, weight) in corners {
                                if weight == 0.0 {
                                    continue;
                                }
                                let form = &lattice.forms[index];
                                angle += weight * f64::from(form.angle_degrees);
                                scale[0] += weight * f64::from(form.scale[0]);
                                scale[1] += weight * f64::from(form.scale[1]);
                            }
                            (angle, scale)
                        }
                        None => (f64::from(rotation.base_angle_degrees), [1.0, 1.0]),
                    };
                    DeformerState::Rotation(RotationState {
                        delta_degrees: angle - f64::from(rotation.base_angle_degrees),
                        scale,
                    })
                }
                Deformer::Warp(warp) => {
                    let points = match &warp.keyforms {
                        Some(lattice) => sample_points(lattice, params, |form| &form.positions),
                        None => warp
                            .rest_positions
                            .iter()
                            .map(|p| [f64::from(p[0]), f64::from(p[1])])
                            .collect(),
                    };
                    DeformerState::Warp(points)
                }
            };
            states.insert(deformer.id().to_string(), state);
        }
        DeformerContext {
            model,
            origins: BTreeMap::new(),
            states,
        }
    }

    /// The deformer's origin after its ancestor chain has been
    /// applied (§3.4: ancestor deformers move descendant
    /// origins). Memoized; the parent walk is bounded by
    /// TREE_DEPTH_MAX via the validator's acyclicity proof.
    fn effective_origin(&mut self, id: &str) -> [f64; 2] {
        if let Some(origin) = self.origins.get(id) {
            return *origin;
        }
        let deformer = self
            .model
            .deformers
            .iter()
            .find(|d| d.id() == id)
            .expect("deformer ids come from the model");
        let authored = match deformer {
            Deformer::Rotation(rotation) => {
                [f64::from(rotation.origin[0]), f64::from(rotation.origin[1])]
            }
            // A warp has no origin of its own; the question does
            // not arise (origins are asked of rotation parents).
            Deformer::Warp(_) => [0.0, 0.0],
        };
        let origin = match deformer.parent() {
            Some(parent) => self.apply_deformer(parent, authored),
            None => authored,
        };
        self.origins.insert(id.to_string(), origin);
        origin
    }

    /// Apply ONE deformer's transform to a point that has
    /// already passed through the deformer's ancestors.
    fn apply_deformer(&mut self, id: &str, point: [f64; 2]) -> [f64; 2] {
        let origin = self.effective_origin(id);
        match self
            .states
            .get(id)
            .expect("states are built for every deformer")
            .clone()
        {
            DeformerState::Rotation(state) => {
                let radians = state.delta_degrees.to_radians();
                let (sin, cos) = radians.sin_cos();
                let local = [
                    (point[0] - origin[0]) * state.scale[0],
                    (point[1] - origin[1]) * state.scale[1],
                ];
                [
                    origin[0] + cos * local[0] - sin * local[1],
                    origin[1] + sin * local[0] + cos * local[1],
                ]
            }
            DeformerState::Warp(points) => {
                // Copy the warp's static shape out of the model
                // first: the ancestor transforms below need
                // `&mut self`, which the model borrow forbids.
                let (columns, rows, rest, parent) = self
                    .model
                    .deformers
                    .iter()
                    .find_map(|d| match d {
                        Deformer::Warp(w) if w.id == id => Some((
                            w.columns as usize,
                            w.rows as usize,
                            w.rest_positions
                                .iter()
                                .map(|p| [f64::from(p[0]), f64::from(p[1])])
                                .collect::<Vec<_>>(),
                            w.parent.clone(),
                        )),
                        _ => None,
                    })
                    .expect("warp state implies a warp deformer");
                // Control points live in the warp's parent
                // space: pass them through the ancestors too.
                let (rest, points) = match &parent {
                    Some(parent) => (
                        rest.iter()
                            .map(|p| self.apply_chain(Some(parent), *p))
                            .collect::<Vec<_>>(),
                        points
                            .iter()
                            .map(|p| self.apply_chain(Some(parent), *p))
                            .collect::<Vec<_>>(),
                    ),
                    None => (rest, points),
                };
                warp_point(&rest, &points, columns, rows, point)
            }
        }
    }

    /// The accumulated rotation of a deformer chain, in
    /// degrees: the sum of the rotation deformers' deltas
    /// along the chain. Warps contribute no rotation.
    fn chain_rotation(&self, deformer: Option<&str>) -> f64 {
        let mut total = 0.0;
        let mut cursor = deformer;
        while let Some(id) = cursor {
            if let Some(DeformerState::Rotation(state)) = self.states.get(id) {
                total += state.delta_degrees;
            }
            cursor = self
                .model
                .deformers
                .iter()
                .find(|d| d.id() == id)
                .expect("deformer ids come from the model")
                .parent();
        }
        total
    }

    /// Apply a whole deformer chain to a point, root-first
    /// (§3.4): the root deformer's transform acts first, the
    /// mesh's own deformer last.
    fn apply_chain(&mut self, deformer: Option<&str>, point: [f64; 2]) -> [f64; 2] {
        // Collect the chain leaf-to-root, then apply reversed.
        let mut chain: Vec<String> = Vec::new();
        let mut cursor = deformer.map(str::to_string);
        while let Some(id) = cursor {
            let parent = self
                .model
                .deformers
                .iter()
                .find(|d| d.id() == id)
                .expect("deformer ids come from the model")
                .parent()
                .map(str::to_string);
            chain.push(id);
            cursor = parent;
        }
        let mut result = point;
        for id in chain.iter().rev() {
            result = self.apply_deformer(id, result);
        }
        result
    }
}

/// Map a point through a warp grid (§3.4): find the point's
/// bilinear coordinates in the REST grid, then evaluate the
/// deformed grid at the same coordinates. The inverse solve is
/// a fixed-iteration Newton per cell, cells scanned row-major;
/// a point outside every cell clamps to the nearest cell's
/// clamped coordinates (a stated degradation, never a panic).
fn warp_point(
    rest: &[[f64; 2]],
    deformed: &[[f64; 2]],
    columns: usize,
    rows: usize,
    point: [f64; 2],
) -> [f64; 2] {
    let stride = columns + 1;
    let cell = |grid: &[[f64; 2]], cx: usize, cy: usize| {
        [
            grid[cy * stride + cx],
            grid[cy * stride + cx + 1],
            grid[(cy + 1) * stride + cx],
            grid[(cy + 1) * stride + cx + 1],
        ]
    };
    let bilinear = |quad: [[f64; 2]; 4], s: f64, t: f64| {
        [
            (1.0 - s) * (1.0 - t) * quad[0][0]
                + s * (1.0 - t) * quad[1][0]
                + (1.0 - s) * t * quad[2][0]
                + s * t * quad[3][0],
            (1.0 - s) * (1.0 - t) * quad[0][1]
                + s * (1.0 - t) * quad[1][1]
                + (1.0 - s) * t * quad[2][1]
                + s * t * quad[3][1],
        ]
    };
    // First cell whose inverse lands inside [0, 1]^2 wins.
    for cy in 0..rows {
        for cx in 0..columns {
            let quad = cell(rest, cx, cy);
            let (mut s, mut t) = (0.5, 0.5);
            for _ in 0..WARP_INVERSE_ITERATIONS {
                let value = bilinear(quad, s, t);
                let error = [point[0] - value[0], point[1] - value[1]];
                // Jacobian by the bilinear partial derivatives.
                let ds = [
                    (1.0 - t) * (quad[1][0] - quad[0][0]) + t * (quad[3][0] - quad[2][0]),
                    (1.0 - t) * (quad[1][1] - quad[0][1]) + t * (quad[3][1] - quad[2][1]),
                ];
                let dt = [
                    (1.0 - s) * (quad[2][0] - quad[0][0]) + s * (quad[3][0] - quad[1][0]),
                    (1.0 - s) * (quad[2][1] - quad[0][1]) + s * (quad[3][1] - quad[1][1]),
                ];
                let determinant = ds[0] * dt[1] - ds[1] * dt[0];
                if determinant == 0.0 {
                    break;
                }
                s += (error[0] * dt[1] - error[1] * dt[0]) / determinant;
                t += (ds[0] * error[1] - ds[1] * error[0]) / determinant;
            }
            if (0.0..=1.0).contains(&s) && (0.0..=1.0).contains(&t) {
                return bilinear(cell(deformed, cx, cy), s, t);
            }
        }
    }
    // Outside the grid: clamp through the nearest cell corner
    // region — evaluate the whole-grid bilinear over the rest
    // grid's bounding parameterization is ill-defined, so fall
    // back to the nearest control point's displacement.
    let mut best: Option<(f64, usize)> = None;
    for (index, rest_point) in rest.iter().enumerate() {
        let distance = (point[0] - rest_point[0]).hypot(point[1] - rest_point[1]);
        if best.is_none_or(|(d, _)| distance < d) {
            best = Some((distance, index));
        }
    }
    let index = best.expect("a warp grid has at least one point").1;
    [
        point[0] + (deformed[index][0] - rest[index][0]),
        point[1] + (deformed[index][1] - rest[index][1]),
    ]
}

/// Evaluate every mesh of a model at a resolved parameter
/// state: keyform interpolation (§3.7), then the deformer
/// chain (§3.4). Meshes return in declaration order.
pub fn evaluate_model(model: &Model, params: &BTreeMap<String, f64>) -> Vec<EvaluatedMesh> {
    let mut context = DeformerContext::new(model, params);
    model
        .meshes
        .iter()
        .map(|mesh: &Mesh| {
            let base: Vec<[f64; 2]> = match &mesh.keyforms {
                Some(lattice) => sample_points(lattice, params, |form| &form.vertices),
                None => mesh
                    .vertices
                    .iter()
                    .map(|v| [f64::from(v[0]), f64::from(v[1])])
                    .collect(),
            };
            let vertices = base
                .iter()
                .map(|point| context.apply_chain(mesh.deformer.as_deref(), *point))
                .collect();
            EvaluatedMesh {
                draw_order: mesh.draw_order,
                id: mesh.id.clone(),
                part: mesh.part.clone(),
                vertices,
            }
        })
        .collect()
}

/// Resolve an endpoint against evaluated meshes (model space).
fn endpoint_model_point(meshes: &[EvaluatedMesh], endpoint: &ContactEndpoint) -> [f64; 2] {
    match endpoint {
        ContactEndpoint::Vertex { mesh, vertex } => {
            meshes
                .iter()
                .find(|m| m.id == *mesh)
                .expect("scene construction proved the mesh exists")
                .vertices[*vertex as usize]
        }
        ContactEndpoint::PartCentroid { part } => {
            let (mut sum, mut count) = ([0.0, 0.0], 0usize);
            for mesh in meshes.iter().filter(|m| m.part == *part) {
                for vertex in &mesh.vertices {
                    sum[0] += vertex[0];
                    sum[1] += vertex[1];
                    count += 1;
                }
            }
            assert!(count > 0, "scene construction proved the part has meshes");
            [sum[0] / count as f64, sum[1] / count as f64]
        }
    }
}

/// An endpoint's scene-space point under a placement.
pub fn endpoint_world(
    meshes: &[EvaluatedMesh],
    placement: &Placement,
    endpoint: &ContactEndpoint,
) -> [f64; 2] {
    placement.apply(endpoint_model_point(meshes, endpoint))
}

/// A validated two-body scene: rigs, contacts, probes, and a
/// root endpoint per rig. Construction is the only fallible
/// step; evaluation over a built scene is total.
#[derive(Debug)]
pub struct Scene {
    contacts: Vec<ContactDef>,
    models: Vec<Model>,
    probes: Vec<ProbeDef>,
    roots: Vec<ContactEndpoint>,
    /// Rigs in solve order: a rig is solved after every rig it
    /// depends on through a contact (dependencies first).
    solve_order: Vec<usize>,
}

impl Scene {
    /// Build and validate a scene. Refusals: a reference that
    /// does not resolve (`LTB-101`), a contact whose endpoints
    /// live on one rig (`LTB-102`), a driven-rig dependency
    /// cycle (`LTB-103`), a non-finite input (`LTB-104`), or a
    /// count beyond the module's bounds (`LTB-106`).
    pub fn new(
        models: Vec<Model>,
        contacts: Vec<ContactDef>,
        probes: Vec<ProbeDef>,
        roots: Vec<ContactEndpoint>,
    ) -> Result<Scene, TwoBodyError> {
        if models.is_empty() || models.len() > RIGS_MAX {
            return Err(TwoBodyError::new(
                "LTB-106",
                format!("scene rig count {} outside 1..={RIGS_MAX}", models.len()),
            ));
        }
        if contacts.len() > CONTACTS_MAX || probes.len() > PROBES_MAX {
            return Err(TwoBodyError::new(
                "LTB-106",
                format!(
                    "scene contact/probe counts {}/{} beyond bounds {CONTACTS_MAX}/{PROBES_MAX}",
                    contacts.len(),
                    probes.len()
                ),
            ));
        }
        if roots.len() != models.len() {
            return Err(TwoBodyError::new(
                "LTB-101",
                format!(
                    "scene declares {} roots for {} rigs",
                    roots.len(),
                    models.len()
                ),
            ));
        }
        let check_endpoint = |rig: usize, endpoint: &ContactEndpoint| -> Result<(), TwoBodyError> {
            let model = models.get(rig).ok_or_else(|| {
                TwoBodyError::new("LTB-101", format!("rig index {rig} out of range"))
            })?;
            match endpoint {
                ContactEndpoint::Vertex { mesh, vertex } => {
                    let found = model.meshes.iter().find(|m| m.id == *mesh).ok_or_else(|| {
                        TwoBodyError::new("LTB-101", format!("rig {rig} has no mesh {mesh}"))
                    })?;
                    if *vertex as usize >= found.vertices.len() {
                        return Err(TwoBodyError::new(
                            "LTB-101",
                            format!(
                                "vertex {vertex} out of range for mesh {mesh} ({} vertices)",
                                found.vertices.len()
                            ),
                        ));
                    }
                    Ok(())
                }
                ContactEndpoint::PartCentroid { part } => {
                    if model.meshes.iter().any(|m| m.part == *part) {
                        Ok(())
                    } else {
                        Err(TwoBodyError::new(
                            "LTB-101",
                            format!("rig {rig} has no mesh on part {part}"),
                        ))
                    }
                }
            }
        };
        for (rig, root) in roots.iter().enumerate() {
            check_endpoint(rig, root)?;
        }
        // Dependency edges: a contact's driven rig depends on
        // the rig at its other end.
        let mut dependencies: Vec<Vec<usize>> = vec![Vec::new(); models.len()];
        for contact in &contacts {
            if contact.a_rig == contact.b_rig {
                return Err(TwoBodyError::new(
                    "LTB-102",
                    format!(
                        "contact {} is a self-contact on rig {}",
                        contact.id, contact.a_rig
                    ),
                ));
            }
            if contact.driven_rig != contact.a_rig && contact.driven_rig != contact.b_rig {
                return Err(TwoBodyError::new(
                    "LTB-101",
                    format!(
                        "contact {} drives rig {}, which is neither endpoint rig",
                        contact.id, contact.driven_rig
                    ),
                ));
            }
            check_endpoint(contact.a_rig, &contact.a)?;
            check_endpoint(contact.b_rig, &contact.b)?;
            if !contact.offset.iter().all(|c| c.is_finite()) {
                return Err(TwoBodyError::new(
                    "LTB-104",
                    format!("contact {} has a non-finite offset", contact.id),
                ));
            }
            let other = if contact.driven_rig == contact.a_rig {
                contact.b_rig
            } else {
                contact.a_rig
            };
            dependencies[contact.driven_rig].push(other);
        }
        for probe in &probes {
            check_endpoint(
                probe.rig,
                &ContactEndpoint::Vertex {
                    mesh: probe.mesh.clone(),
                    vertex: probe.vertex,
                },
            )?;
            if models.get(probe.against_rig).is_none() {
                return Err(TwoBodyError::new(
                    "LTB-101",
                    format!(
                        "probe {} targets unknown rig {}",
                        probe.id, probe.against_rig
                    ),
                ));
            }
        }
        // Topological solve order (dependencies first); a cycle
        // in the driven graph is a refusal, not a hang.
        let mut order: Vec<usize> = Vec::with_capacity(models.len());
        let mut marks: Vec<u8> = vec![0; models.len()];
        fn visit(
            rig: usize,
            dependencies: &[Vec<usize>],
            marks: &mut [u8],
            order: &mut Vec<usize>,
        ) -> Result<(), TwoBodyError> {
            match marks[rig] {
                1 => {
                    return Err(TwoBodyError::new(
                        "LTB-103",
                        format!("driven-rig dependency cycle reaches rig {rig}"),
                    ));
                }
                2 => return Ok(()),
                _ => {}
            }
            marks[rig] = 1;
            for dependency in &dependencies[rig] {
                visit(*dependency, dependencies, marks, order)?;
            }
            marks[rig] = 2;
            order.push(rig);
            Ok(())
        }
        for rig in 0..models.len() {
            visit(rig, &dependencies, &mut marks, &mut order)?;
        }
        Ok(Scene {
            contacts,
            models,
            probes,
            roots,
            solve_order: order,
        })
    }

    /// The scene's contacts, in declared order.
    pub fn contacts(&self) -> &[ContactDef] {
        &self.contacts
    }

    /// The scene's rigs.
    pub fn models(&self) -> &[Model] {
        &self.models
    }
}

/// One frame's inputs: per-rig driven parameter values,
/// per-rig authored placements (a driven rig's entry is the
/// solver's seed; its solved value replaces it in the read),
/// and the active flag per contact, in declared order.
#[derive(Debug, Clone)]
pub struct FrameInput {
    /// Active flag per contact (declared order).
    pub active: Vec<bool>,
    /// Driven parameter values per rig.
    pub driven: Vec<BTreeMap<String, f64>>,
    /// Authored placements per rig.
    pub placements: Vec<Placement>,
    /// Frame index on the fixed-step clock.
    pub step: u64,
    /// Frame time in seconds.
    pub time_seconds: f64,
}

/// One contact's per-frame read. Slip is data: it is reported
/// whether or not it converged.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ContactRead {
    /// True while the contact is engaged this frame.
    pub engaged: bool,
    /// Contact id.
    pub id: String,
    /// Anchor separation BEFORE the solve (authored state).
    pub pre_solve_slip: f64,
    /// Anchor separation after the solve, model units.
    pub slip: f64,
}

/// One frame's full read: solved placements, per-contact slip,
/// the worst penetration margin, and per-rig root points.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FrameRead {
    /// Per-contact reads, declared order.
    pub contacts: Vec<ContactRead>,
    /// True iff every engaged contact is at or below
    /// SLIP_CONVERGED_MAX and no probe is inside its target.
    pub converged: bool,
    /// Minimum signed margin over all probes (positive =
    /// outside, negative = interpenetrating), if any probes.
    pub min_margin: Option<f64>,
    /// The probe that produced `min_margin`, if any probes.
    pub min_margin_probe: Option<String>,
    /// Solved placements per rig (authored values for rigs no
    /// contact drives).
    pub placements: Vec<Placement>,
    /// Root endpoint per rig, scene space.
    pub roots: Vec<[f64; 2]>,
    /// Frame index.
    pub step: u64,
    /// Frame time in seconds.
    pub time_seconds: f64,
}

/// Signed distance from a point to a polygon (the evaluated
/// vertices in order, closed implicitly): positive outside,
/// negative inside (even-odd rule — winding-independent).
fn signed_margin(point: [f64; 2], polygon: &[[f64; 2]]) -> f64 {
    let mut inside = false;
    let mut nearest = f64::INFINITY;
    let edges = polygon
        .iter()
        .zip(polygon.iter().cycle().skip(1).take(polygon.len()));
    for (a, b) in edges {
        let (a, b) = (*a, *b);
        // Distance to the segment.
        let ab = [b[0] - a[0], b[1] - a[1]];
        let length_sq = ab[0] * ab[0] + ab[1] * ab[1];
        let t = if length_sq == 0.0 {
            0.0
        } else {
            (((point[0] - a[0]) * ab[0] + (point[1] - a[1]) * ab[1]) / length_sq).clamp(0.0, 1.0)
        };
        let closest = [a[0] + t * ab[0], a[1] + t * ab[1]];
        let distance = (point[0] - closest[0]).hypot(point[1] - closest[1]);
        nearest = nearest.min(distance);
        // Even-odd ray cast toward +x.
        if (a[1] > point[1]) != (b[1] > point[1]) {
            let crossing_x = a[0] + (point[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]);
            if point[0] < crossing_x {
                inside = !inside;
            }
        }
    }
    if inside { -nearest } else { nearest }
}

/// A probe's margin and push-out direction under the current
/// placements: (signed margin, unit direction that moves the
/// PROBE POINT away from the polygon surface).
fn probe_read(
    evaluated: &[Vec<EvaluatedMesh>],
    placements: &[Placement],
    probe: &ProbeDef,
) -> (f64, [f64; 2]) {
    let point = endpoint_world(
        &evaluated[probe.rig],
        &placements[probe.rig],
        &ContactEndpoint::Vertex {
            mesh: probe.mesh.clone(),
            vertex: probe.vertex,
        },
    );
    let mut margin = f64::INFINITY;
    let mut direction = [1.0, 0.0];
    for mesh in &evaluated[probe.against_rig] {
        let polygon: Vec<[f64; 2]> = mesh
            .vertices
            .iter()
            .map(|v| placements[probe.against_rig].apply(*v))
            .collect();
        let value = signed_margin(point, &polygon);
        if value < margin {
            margin = value;
            // Push direction: from the polygon's centroid
            // toward the point (exact on the boundary normal
            // only for the closest edge; the centroid ray is
            // the bounded, deterministic choice available for
            // an interior point).
            let (mut cx, mut cy) = (0.0, 0.0);
            for vertex in &polygon {
                cx += vertex[0];
                cy += vertex[1];
            }
            let n = polygon.len() as f64;
            let ray = [point[0] - cx / n, point[1] - cy / n];
            let length = ray[0].hypot(ray[1]);
            if length > 0.0 {
                direction = [ray[0] / length, ray[1] / length];
            }
        }
    }
    (margin, direction)
}

/// World position of a contact's fixed point on the `b`
/// rig: the `b` endpoint's model-space position plus the
/// contact offset rotated into the endpoint's deformed
/// frame, under the `b` rig's current placement.
fn offset_point(
    scene: &Scene,
    resolved: &[BTreeMap<String, f64>],
    evaluated: &[Vec<EvaluatedMesh>],
    placements: &[Placement],
    contact: &ContactDef,
) -> [f64; 2] {
    let model = &scene.models[contact.b_rig];
    let local = endpoint_world(&evaluated[contact.b_rig], &Placement::IDENTITY, &contact.b);
    let deformer = match &contact.b {
        ContactEndpoint::Vertex { mesh, .. } => model
            .meshes
            .iter()
            .find(|m| m.id == *mesh)
            .and_then(|m| m.deformer.as_deref()),
        ContactEndpoint::PartCentroid { part } => model
            .meshes
            .iter()
            .find(|m| m.part == *part)
            .and_then(|m| m.deformer.as_deref()),
    };
    let context = DeformerContext::new(model, &resolved[contact.b_rig]);
    let frame = context.chain_rotation(deformer).to_radians();
    let (sin, cos) = frame.sin_cos();
    let rotated = [
        cos * contact.offset[0] - sin * contact.offset[1],
        sin * contact.offset[0] + cos * contact.offset[1],
    ];
    placements[contact.b_rig].apply([local[0] + rotated[0], local[1] + rotated[1]])
}

/// World position of one endpoint under a placement.
fn anchor(
    evaluated: &[Vec<EvaluatedMesh>],
    placements: &[Placement],
    rig: usize,
    endpoint: &ContactEndpoint,
) -> [f64; 2] {
    endpoint_world(&evaluated[rig], &placements[rig], endpoint)
}

/// One exact rigid-fit (Kabsch) step for one driven rig:
/// rotate the driven endpoints about their own centroid by
/// the least-squares angle toward their targets, then
/// translate the centroid onto the target centroid. A
/// rigid-consistent contact set is solved exactly in one
/// step at any pivot distance; alternating translation /
/// rotation about the rig origin is not - it stalls when
/// the contacts sit far from the origin (measured: ratio
/// ~1 - (separation / lever)^2 per sweep, 7.2 -> 1.06 units
/// over 8 sweeps on the grasp scene), because rotation
/// about a distant pivot is nearly a translation.
fn kabsch_step(
    scene: &Scene,
    resolved: &[BTreeMap<String, f64>],
    evaluated: &[Vec<EvaluatedMesh>],
    placements: &mut [Placement],
    rig: usize,
    mine: &[usize],
) {
    let mut pairs: Vec<([f64; 2], [f64; 2])> = Vec::with_capacity(mine.len());
    for &index in mine {
        let contact = &scene.contacts[index];
        let a = anchor(evaluated, placements, contact.a_rig, &contact.a);
        let fixed = offset_point(scene, resolved, evaluated, placements, contact);
        // The driven side moves its contact point onto the
        // other side's: for `b` the moving point is the
        // offset point itself, for `a` the endpoint.
        let pair = if contact.driven_rig == contact.a_rig {
            (a, fixed)
        } else {
            (fixed, a)
        };
        pairs.push(pair);
    }
    let count = pairs.len() as f64;
    let (mut centroid_p, mut centroid_t) = ([0.0, 0.0], [0.0, 0.0]);
    for (point, target) in &pairs {
        centroid_p[0] += point[0];
        centroid_p[1] += point[1];
        centroid_t[0] += target[0];
        centroid_t[1] += target[1];
    }
    centroid_p = [centroid_p[0] / count, centroid_p[1] / count];
    centroid_t = [centroid_t[0] / count, centroid_t[1] / count];
    let (mut cross, mut dot) = (0.0, 0.0);
    for (point, target) in &pairs {
        let lp = [point[0] - centroid_p[0], point[1] - centroid_p[1]];
        let lt = [target[0] - centroid_t[0], target[1] - centroid_t[1]];
        cross += lp[0] * lt[1] - lp[1] * lt[0];
        dot += lp[0] * lt[0] + lp[1] * lt[1];
    }
    let mut degrees = 0.0;
    if cross != 0.0 || dot != 0.0 {
        degrees = cross
            .atan2(dot)
            .to_degrees()
            .clamp(-ROTATION_STEP_MAX_DEGREES, ROTATION_STEP_MAX_DEGREES);
    }
    // Placement update: the driven points must move as
    // p -> R(d)(p - centroid_p) + centroid_t. With
    // p = R(th) x + T that is th += d and
    // T -> R(d)(T - centroid_p) + centroid_t.
    let radians = degrees.to_radians();
    let (sin, cos) = radians.sin_cos();
    let rel = [
        placements[rig].translation[0] - centroid_p[0],
        placements[rig].translation[1] - centroid_p[1],
    ];
    placements[rig].translation = [
        cos * rel[0] - sin * rel[1] + centroid_t[0],
        sin * rel[0] + cos * rel[1] + centroid_t[1],
    ];
    placements[rig].rotation_degrees += degrees;
}

/// Evaluate one frame: resolve parameters, evaluate geometry,
/// solve driven placements in dependency order (fixed sweeps,
/// then fixed separation passes), and read everything back.
/// Placements in the input for driven rigs are seeds; rigs no
/// contact drives keep their authored placement exactly.
pub fn evaluate_frame(scene: &Scene, input: &FrameInput) -> Result<FrameRead, TwoBodyError> {
    let rigs = scene.models.len();
    assert_eq!(input.driven.len(), rigs, "one driven set per rig");
    assert_eq!(input.placements.len(), rigs, "one placement per rig");
    assert_eq!(
        input.active.len(),
        scene.contacts.len(),
        "one active flag per contact"
    );
    for placement in &input.placements {
        if !placement.rotation_degrees.is_finite()
            || !placement.translation.iter().all(|c| c.is_finite())
        {
            return Err(TwoBodyError::new(
                "LTB-104",
                "authored placement is not finite".to_string(),
            ));
        }
    }
    let mut evaluated: Vec<Vec<EvaluatedMesh>> = Vec::with_capacity(rigs);
    let mut resolved: Vec<BTreeMap<String, f64>> = Vec::with_capacity(scene.models.len());
    for (model, driven) in scene.models.iter().zip(input.driven.iter()) {
        let params = resolve_parameters(model, driven)?;
        evaluated.push(evaluate_model(model, &params));
        resolved.push(params);
    }
    // World anchor of a contact endpoint under a placement set.
    // Pre-solve slip, against the authored placements.
    let mut placements = input.placements.clone();
    let pre_solve_slip: Vec<f64> = scene
        .contacts
        .iter()
        .map(|contact| {
            let a = anchor(&evaluated, &placements, contact.a_rig, &contact.a);
            let fixed = offset_point(scene, &resolved, &evaluated, &placements, contact);
            (a[0] - fixed[0]).hypot(a[1] - fixed[1])
        })
        .collect();

    // Solve, rigs in dependency order. Per rig: contact
    // sweeps, then separation passes, then ONE polish contact
    // step - attachment contacts are hard constraints and
    // outrank push-out, so if the two genuinely conflict the
    // final state holds the contacts and the penetration
    // remains visible in the margin reads.
    for &rig in &scene.solve_order {
        let mine: Vec<usize> = scene
            .contacts
            .iter()
            .enumerate()
            .filter(|(index, c)| c.driven_rig == rig && input.active[*index])
            .map(|(index, _)| index)
            .collect();
        if mine.is_empty() {
            continue;
        }
        for _sweep in 0..SOLVER_SWEEPS {
            kabsch_step(scene, &resolved, &evaluated, &mut placements, rig, &mine);
        }
        // Separation passes: push this rig out of the worst
        // penetration it takes part in, a fixed number of
        // times, re-measuring after each push.
        for _pass in 0..SEPARATION_PASSES {
            let mut worst: Option<(f64, [f64; 2], bool)> = None;
            for probe in &scene.probes {
                if probe.rig != rig && probe.against_rig != rig {
                    continue;
                }
                let (margin, direction) = probe_read(&evaluated, &placements, probe);
                if margin < 0.0 && worst.as_ref().is_none_or(|(m, _, _)| margin < *m) {
                    //  moves the probe point out of
                    // the polygon. If the point is ours, move
                    // with it; if the polygon is ours, move
                    // against it.
                    worst = Some((margin, direction, probe.rig == rig));
                }
            }
            match worst {
                None => break,
                Some((margin, direction, point_is_ours)) => {
                    let push = if point_is_ours { -margin } else { margin };
                    placements[rig].translation[0] += direction[0] * push;
                    placements[rig].translation[1] += direction[1] * push;
                }
            }
        }
        kabsch_step(scene, &resolved, &evaluated, &mut placements, rig, &mine);
    }

    // Final reads.
    let contacts: Vec<ContactRead> = scene
        .contacts
        .iter()
        .enumerate()
        .map(|(index, contact)| {
            let a = anchor(&evaluated, &placements, contact.a_rig, &contact.a);
            ContactRead {
                engaged: input.active[index],
                id: contact.id.clone(),
                pre_solve_slip: pre_solve_slip[index],
                slip: {
                    let fixed = offset_point(scene, &resolved, &evaluated, &placements, contact);
                    (a[0] - fixed[0]).hypot(a[1] - fixed[1])
                },
            }
        })
        .collect();
    let mut min_margin: Option<f64> = None;
    let mut min_margin_probe: Option<String> = None;
    for probe in &scene.probes {
        let (margin, _) = probe_read(&evaluated, &placements, probe);
        if min_margin.is_none_or(|m| margin < m) {
            min_margin = Some(margin);
            min_margin_probe = Some(probe.id.clone());
        }
    }
    let roots = scene
        .roots
        .iter()
        .enumerate()
        .map(|(rig, root)| endpoint_world(&evaluated[rig], &placements[rig], root))
        .collect();
    let converged = contacts
        .iter()
        .all(|read| !read.engaged || read.slip <= SLIP_CONVERGED_MAX)
        && min_margin.is_none_or(|m| m >= 0.0);
    Ok(FrameRead {
        contacts,
        converged,
        min_margin,
        min_margin_probe,
        placements,
        roots,
        step: input.step,
        time_seconds: input.time_seconds,
    })
}
