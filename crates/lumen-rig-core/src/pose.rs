//! Pose evaluation: parameter-state resolution and geometry.
//!
//! This is the R2 evaluation stage the format deferred to:
//! given a validated [`Model`] and a parameter state, produce
//! every mesh's deformed vertices. The pipeline per mesh is
//! the one SPEC.md sections 3.2, 3.4, 3.5 and 3.7 define:
//!
//! 1. Mesh keyforms (absolute vertex states) interpolate over
//!    their lattice; without keyforms the rest vertices hold.
//! 2. The mesh's deformer chain applies **root-first**
//!    (section 3.4): each deformer acts with its keyform state
//!    (or its base state), about an origin — or through a
//!    control grid — that its own ancestors have already
//!    transformed. A warp maps a point by locating it in the
//!    rest grid's cells (inverse bilinear) and evaluating the
//!    same bilinear coordinates in the deformed grid; points
//!    outside every rest cell map through the nearest cell
//!    with clamped coordinates (edge extension).
//! 3. Part translations (BIND-1, section 3.2) accumulate down
//!    the part chain and translate the mesh in model space.
//!    They apply after the deformer chain: the format keeps
//!    the part forest and the deformer forest as separate
//!    trees, so no interleaving between a part node and a
//!    deformer node is recorded, and a model-space
//!    translation is the only order the data supports. (In
//!    the INP reference a node translation acts in the node's
//!    local frame and also moves descendant deformer pivots;
//!    neither the local frame nor a part-to-deformer
//!    ownership link exists in this format, so those two
//!    behaviours are named here as not carried — see
//!    SPEC.md section 3.2.)
//!
//! All interpolation and transform arithmetic is f64;
//! values round to f32 once, at write (section 3.7's law).
//! Evaluation is a pure function of the model and the
//! parameter state: no clocks, no wall time, and no map
//! iteration (every walk follows declaration order).

use std::collections::HashMap;

use crate::model::{
    BlendMode, Deformer, Lattice, Mesh, MeshForm, Model, Parameter, RotationForm, TranslationForm,
    WarpForm,
};

/// A resolved parameter state: one f32 per parameter of the
/// model, in declaration order, every value inside its
/// parameter's `[min, max]` (section 2.2). Scalar values are
/// written directly; combined parameters are always derived
/// (section 3.1's formula), never written.
#[derive(Debug, Clone, PartialEq)]
pub struct ParameterValues {
    values: Vec<f32>,
    index: HashMap<String, usize>,
}

impl ParameterValues {
    /// The model's rest state: scalar defaults, combined
    /// parameters derived from them.
    pub fn rest(model: &Model) -> ParameterValues {
        let mut state = ParameterValues {
            values: vec![0.0; model.parameters.len()],
            index: model
                .parameters
                .iter()
                .enumerate()
                .map(|(slot, param)| (param.id().to_string(), slot))
                .collect(),
        };
        for (slot, param) in model.parameters.iter().enumerate() {
            if let Parameter::Scalar(scalar) = param {
                state.values[slot] = scalar.default;
            }
        }
        state.resolve_combined(model);
        state
    }

    /// The value of one parameter, or `None` for an id the
    /// model does not declare.
    pub fn get(&self, id: &str) -> Option<f32> {
        self.index.get(id).map(|&slot| self.values[slot])
    }

    /// The parameter's value normalised to `[0, 1]` over
    /// its declared range — the `norm_i` of the physics
    /// input law (section 5.2 step 1). `None` for an unknown
    /// id; combined parameters normalise over their declared
    /// range like any other.
    pub fn normalized(&self, model: &Model, id: &str) -> Option<f64> {
        let &slot = self.index.get(id)?;
        let (min, max) = match &model.parameters[slot] {
            Parameter::Scalar(scalar) => (scalar.min, scalar.max),
            Parameter::Combined(combined) => (combined.min, combined.max),
        };
        let value = self.values[slot];
        Some((f64::from(value) - f64::from(min)) / (f64::from(max) - f64::from(min)))
    }

    /// Write one scalar parameter, through the sanctioned
    /// clamp (section 9.3: a write clamps into `[min, max]`),
    /// then re-derive every combined parameter. Returns
    /// `false` — and changes nothing — for an unknown id or
    /// a combined parameter (combined values are derived,
    /// never written).
    pub fn set(&mut self, model: &Model, id: &str, value: f32) -> bool {
        let Some(&slot) = self.index.get(id) else {
            return false;
        };
        let Parameter::Scalar(scalar) = &model.parameters[slot] else {
            return false;
        };
        self.values[slot] = value.clamp(scalar.min, scalar.max);
        self.resolve_combined(model);
        true
    }

    /// The motion host write (section 4.3, as the format's
    /// trace harness performs it): blend `value` onto the
    /// value this parameter currently holds —
    /// `base + weight * (value - base)` — clamp into the
    /// declared range in f64, round once to f32. Unlike
    /// [`ParameterValues::set`], this write may target a
    /// combined parameter: a motion that drives one holds
    /// the written value until the next scalar `set`
    /// re-derives it. Returns `false` for an unknown id.
    pub fn write_blended(&mut self, model: &Model, id: &str, weight: f64, value: f64) -> bool {
        let Some(&slot) = self.index.get(id) else {
            return false;
        };
        let (min, max) = match &model.parameters[slot] {
            Parameter::Scalar(scalar) => (f64::from(scalar.min), f64::from(scalar.max)),
            Parameter::Combined(combined) => (f64::from(combined.min), f64::from(combined.max)),
        };
        let base = f64::from(self.values[slot]);
        let blended = base + weight * (value - base);
        self.values[slot] = blended.clamp(min, max) as f32;
        true
    }

    /// Re-derive all combined parameters from the current
    /// scalar values. The validator proved the combined
    /// graph acyclic with reachable ranges inside the
    /// declared ranges (LRF-205, LRF-304), so bounded passes
    /// in declaration order settle every value; the final
    /// clamp only absorbs f64-to-f32 rounding at the edges.
    fn resolve_combined(&mut self, model: &Model) {
        let combined_count = model
            .parameters
            .iter()
            .filter(|param| matches!(param, Parameter::Combined(_)))
            .count();
        for _ in 0..=combined_count {
            let mut settled = true;
            for (slot, param) in model.parameters.iter().enumerate() {
                let Parameter::Combined(combined) = param else {
                    continue;
                };
                let mut sum = f64::from(combined.offset);
                for term in &combined.terms {
                    let term_value = self.get(&term.parameter).unwrap_or(0.0);
                    sum += f64::from(term.weight) * f64::from(term_value);
                }
                let value = (sum as f32).clamp(combined.min, combined.max);
                if self.values[slot] != value {
                    self.values[slot] = value;
                    settled = false;
                }
            }
            if settled {
                break;
            }
        }
    }
}

/// One mesh after pose evaluation: deformed vertices in
/// model space, plus the render inputs the rasteriser
/// consumes (effective opacity per section 3.2, draw order,
/// blend mode, and the clipping relation — carried here,
/// interpreted by the renderer).
#[derive(Debug, Clone, PartialEq)]
pub struct EvaluatedMesh {
    pub id: String,
    pub part: String,
    pub texture: String,
    pub vertices: Vec<[f32; 2]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    pub opacity: f32,
    pub blend_mode: BlendMode,
    pub draw_order: i32,
    pub rendered: bool,
    pub clipping_mask_ids: Vec<String>,
}

/// Evaluate every mesh of `model` at `values`, in the
/// model's declaration order. Rendering order is the
/// caller's sort (section 3.5: `(draw_order, id)`).
pub fn evaluate_pose(model: &Model, values: &ParameterValues) -> Vec<EvaluatedMesh> {
    let mut evaluator = PoseEvaluator::new(model, values);
    model
        .meshes
        .iter()
        .map(|mesh| evaluator.evaluate_mesh(mesh))
        .collect()
}

/// The evaluated position of a deformer: its origin carried
/// through its ancestor chain (its own rotation does not
/// move its own origin). This is the physics anchor position
/// for deformer anchors (section 5.2). `None` for an id the
/// model does not declare.
pub fn deformer_position(model: &Model, values: &ParameterValues, id: &str) -> Option<[f64; 2]> {
    let mut evaluator = PoseEvaluator::new(model, values);
    evaluator.deformer_origin(id)
}

/// The centroid (vertex mean) of an evaluated mesh — the
/// physics anchor position for mesh anchors (section 5.2),
/// matching the reference point the bridge exporter records
/// for a mesh anchor (the rest-vertex centroid).
pub fn mesh_centroid(mesh: &EvaluatedMesh) -> [f64; 2] {
    let mut sum = [0.0_f64; 2];
    for vertex in &mesh.vertices {
        sum[0] += f64::from(vertex[0]);
        sum[1] += f64::from(vertex[1]);
    }
    let count = mesh.vertices.len() as f64;
    [sum[0] / count, sum[1] / count]
}

// ----------------------------------------------------------------
// Lattice sampling (section 3.7: linear / bilinear, in f64)

/// The `at` coordinate of a lattice payload. The four
/// payload types share no trait in the model layer; this
/// module-local trait is the one place that names the role.
trait LatticePayload {
    fn at(&self) -> &[u32];
}

impl LatticePayload for MeshForm {
    fn at(&self) -> &[u32] {
        &self.at
    }
}

impl LatticePayload for RotationForm {
    fn at(&self) -> &[u32] {
        &self.at
    }
}

impl LatticePayload for TranslationForm {
    fn at(&self) -> &[u32] {
        &self.at
    }
}

impl LatticePayload for WarpForm {
    fn at(&self) -> &[u32] {
        &self.at
    }
}

/// One axis of a lattice sample: the segment `[lo, hi]` of
/// key indices and the fraction along it. Values outside
/// the key range clamp to the edge segment with fraction 0
/// or 1 — the lattice holds its edge forms, it never
/// extrapolates.
struct AxisSample {
    lo: usize,
    hi: usize,
    fraction: f64,
}

fn axis_sample(keys: &[f32], value: f32) -> AxisSample {
    let value = f64::from(value);
    let last = keys.len() - 1;
    if value <= f64::from(keys[0]) {
        return AxisSample {
            lo: 0,
            hi: 1,
            fraction: 0.0,
        };
    }
    if value >= f64::from(keys[last]) {
        return AxisSample {
            lo: last - 1,
            hi: last,
            fraction: 1.0,
        };
    }
    let mut lo = 0;
    while value >= f64::from(keys[lo + 1]) {
        lo += 1;
    }
    let span = f64::from(keys[lo + 1]) - f64::from(keys[lo]);
    AxisSample {
        lo,
        hi: lo + 1,
        fraction: (value - f64::from(keys[lo])) / span,
    }
}

/// The lattice's corner forms and their multilinear weights
/// at the current parameter values: 2 corners for one axis,
/// 4 for two; the weights sum to 1.
fn lattice_corners<'f, F: LatticePayload>(
    lattice: &'f Lattice<F>,
    values: &ParameterValues,
) -> Vec<(&'f F, f64)> {
    let mut samples = Vec::with_capacity(lattice.axes.len());
    for (axis, keys) in lattice.axes.iter().zip(lattice.keys.iter()) {
        let value = values.get(axis).unwrap_or(0.0);
        samples.push(axis_sample(keys, value));
    }
    let mut corners = Vec::with_capacity(4);
    if samples.len() == 1 {
        let s = &samples[0];
        corners.push((form_at(lattice, &[s.lo]), 1.0 - s.fraction));
        corners.push((form_at(lattice, &[s.hi]), s.fraction));
    } else {
        let (a, b) = (&samples[0], &samples[1]);
        for (i, wi) in [(a.lo, 1.0 - a.fraction), (a.hi, a.fraction)] {
            for (j, wj) in [(b.lo, 1.0 - b.fraction), (b.hi, b.fraction)] {
                corners.push((form_at(lattice, &[i, j]), wi * wj));
            }
        }
    }
    corners
}

/// The form at one lattice coordinate. The validator proved
/// the lattice complete (one form per coordinate), so the
/// search always lands.
fn form_at<'f, F: LatticePayload>(lattice: &'f Lattice<F>, at: &[usize]) -> &'f F {
    lattice
        .forms
        .iter()
        .find(|form| {
            form.at().len() == at.len()
                && form
                    .at()
                    .iter()
                    .zip(at.iter())
                    .all(|(form_index, &wanted)| *form_index as usize == wanted)
        })
        .expect("a validated lattice carries every coordinate")
}

/// Interpolate a point-set payload (mesh vertices, warp
/// control points): every point is the weight sum of the
/// corner forms' points.
fn sample_point_sets<F: LatticePayload>(
    lattice: &Lattice<F>,
    values: &ParameterValues,
    points_of: impl Fn(&F) -> &[[f32; 2]],
) -> Vec<[f64; 2]> {
    let corners = lattice_corners(lattice, values);
    let point_count = points_of(corners[0].0).len();
    let mut out = vec![[0.0_f64; 2]; point_count];
    for (form, weight) in corners {
        for (slot, point) in points_of(form).iter().enumerate() {
            out[slot][0] += weight * f64::from(point[0]);
            out[slot][1] += weight * f64::from(point[1]);
        }
    }
    out
}

/// Interpolate a rotation payload: absolute angle in
/// degrees and absolute scale.
fn sample_rotation(lattice: &Lattice<RotationForm>, values: &ParameterValues) -> (f64, [f64; 2]) {
    let (mut angle, mut scale) = (0.0_f64, [0.0_f64; 2]);
    for (form, weight) in lattice_corners(lattice, values) {
        angle += weight * f64::from(form.angle_degrees);
        scale[0] += weight * f64::from(form.scale[0]);
        scale[1] += weight * f64::from(form.scale[1]);
    }
    (angle, scale)
}

/// Interpolate a part-translation payload: the absolute
/// model-space translation of the part's subtree.
fn sample_translation(lattice: &Lattice<TranslationForm>, values: &ParameterValues) -> [f64; 2] {
    let mut out = [0.0_f64; 2];
    for (form, weight) in lattice_corners(lattice, values) {
        out[0] += weight * f64::from(form.translation[0]);
        out[1] += weight * f64::from(form.translation[1]);
    }
    out
}

// ----------------------------------------------------------------
// The evaluator

/// Pose evaluation state: the model, the parameter values,
/// and per-deformer caches of ancestor-transformed origins
/// and warp grids (a deformer's effective frame depends only
/// on its ancestors and the parameter state, both fixed for
/// one evaluation pass).
struct PoseEvaluator<'m> {
    model: &'m Model,
    values: &'m ParameterValues,
    deformer_index: HashMap<&'m str, usize>,
    part_index: HashMap<&'m str, usize>,
    origin_cache: HashMap<usize, [f64; 2]>,
    warp_cache: HashMap<usize, WarpSets>,
}

/// A warp deformer's two grids, both carried through its
/// ancestors: the rest grid (for cell location) and the
/// current grid (the keyform state, for the mapping).
struct WarpSets {
    rest: Vec<[f64; 2]>,
    current: Vec<[f64; 2]>,
}

impl<'m> PoseEvaluator<'m> {
    fn new(model: &'m Model, values: &'m ParameterValues) -> PoseEvaluator<'m> {
        PoseEvaluator {
            model,
            values,
            deformer_index: model
                .deformers
                .iter()
                .enumerate()
                .map(|(slot, deformer)| (deformer.id(), slot))
                .collect(),
            part_index: model
                .parts
                .iter()
                .enumerate()
                .map(|(slot, part)| (part.id.as_str(), slot))
                .collect(),
            origin_cache: HashMap::new(),
            warp_cache: HashMap::new(),
        }
    }

    fn evaluate_mesh(&mut self, mesh: &Mesh) -> EvaluatedMesh {
        // Step 1: mesh keyforms, or the rest vertices.
        let mut points: Vec<[f64; 2]> = match &mesh.keyforms {
            Some(lattice) => sample_point_sets(lattice, self.values, |form| &form.vertices),
            None => mesh
                .vertices
                .iter()
                .map(|point| [f64::from(point[0]), f64::from(point[1])])
                .collect(),
        };
        // Step 2: the deformer chain, root-first.
        if let Some(deformer_id) = &mesh.deformer {
            let chain = self.chain_of(deformer_id);
            for (position, &slot) in chain.iter().enumerate() {
                let ancestors = &chain[..position];
                points = points
                    .iter()
                    .map(|&point| self.apply_deformer(slot, ancestors, point))
                    .collect();
            }
        }
        // Step 3: accumulated part translations.
        let translation = self.part_translation(&mesh.part);
        let vertices = points
            .iter()
            .map(|point| {
                [
                    (point[0] + translation[0]) as f32,
                    (point[1] + translation[1]) as f32,
                ]
            })
            .collect();
        EvaluatedMesh {
            id: mesh.id.clone(),
            part: mesh.part.clone(),
            texture: mesh.texture.clone(),
            vertices,
            uvs: mesh.uvs.clone(),
            indices: mesh.indices.clone(),
            opacity: self.effective_opacity(mesh),
            blend_mode: mesh.blend_mode,
            draw_order: mesh.draw_order,
            rendered: mesh.rendered,
            clipping_mask_ids: mesh.clipping_mask_ids.clone(),
        }
    }

    /// The deformer chain for `id`, root-first (the mesh's
    /// own deformer last). The validator proved the forest
    /// acyclic and every parent present.
    fn chain_of(&self, id: &str) -> Vec<usize> {
        let mut chain = Vec::new();
        let mut cursor = self.deformer_index.get(id).copied();
        while let Some(slot) = cursor {
            chain.push(slot);
            cursor = self.model.deformers[slot]
                .parent()
                .and_then(|parent| self.deformer_index.get(parent).copied());
        }
        chain.reverse();
        chain
    }

    /// Apply one deformer to one point. `ancestors` is the
    /// deformer's own chain prefix (root-first, excluding
    /// the deformer itself): the deformer's origin or grid
    /// is carried through them before it acts.
    fn apply_deformer(&mut self, slot: usize, ancestors: &[usize], point: [f64; 2]) -> [f64; 2] {
        // Read the deformer's own state first (plain data),
        // so no model borrow is live across the cache calls.
        enum Action {
            Rotate { angle: f64, scale: [f64; 2] },
            Warp { columns: usize, rows: usize },
        }
        let action = match &self.model.deformers[slot] {
            Deformer::Rotation(rotation) => {
                let (angle, scale) = match &rotation.keyforms {
                    Some(lattice) => sample_rotation(lattice, self.values),
                    None => (f64::from(rotation.base_angle_degrees), [1.0, 1.0]),
                };
                Action::Rotate { angle, scale }
            }
            Deformer::Warp(warp) => Action::Warp {
                columns: warp.columns as usize,
                rows: warp.rows as usize,
            },
        };
        match action {
            Action::Rotate { angle, scale } => {
                let origin = self.effective_origin(slot, ancestors);
                rotate_scale(point, origin, angle, scale)
            }
            Action::Warp { columns, rows } => {
                let sets = self.effective_warp_sets(slot, ancestors);
                warp_map(&sets, columns, rows, point)
            }
        }
    }

    /// A deformer's origin after its ancestors have acted:
    /// the rest origin pushed through the ancestor chain.
    fn effective_origin(&mut self, slot: usize, ancestors: &[usize]) -> [f64; 2] {
        if let Some(&origin) = self.origin_cache.get(&slot) {
            return origin;
        }
        let origin = match &self.model.deformers[slot] {
            Deformer::Rotation(rotation) => {
                let mut origin = [f64::from(rotation.origin[0]), f64::from(rotation.origin[1])];
                for (position, &ancestor) in ancestors.iter().enumerate() {
                    origin = self.apply_deformer(ancestor, &ancestors[..position], origin);
                }
                origin
            }
            Deformer::Warp(_) => {
                // A warp deformer has no single origin in
                // the format; its anchor position is the
                // centroid of its evaluated control grid
                // (the R2 pin recorded in SPEC.md section
                // 5.2).
                let sets = self.effective_warp_sets(slot, ancestors);
                let mut sum = [0.0_f64; 2];
                for point in &sets.current {
                    sum[0] += point[0];
                    sum[1] += point[1];
                }
                let count = sets.current.len() as f64;
                [sum[0] / count, sum[1] / count]
            }
        };
        self.origin_cache.insert(slot, origin);
        origin
    }

    /// The public-shaped origin query: ancestors resolved
    /// from the deformer's own parent chain.
    fn deformer_origin(&mut self, id: &str) -> Option<[f64; 2]> {
        let slot = *self.deformer_index.get(id)?;
        let chain = self.chain_of(id);
        let ancestors = &chain[..chain.len() - 1];
        Some(self.effective_origin(slot, ancestors))
    }

    /// A warp deformer's rest and current grids after its
    /// ancestors have acted on both.
    fn effective_warp_sets(&mut self, slot: usize, ancestors: &[usize]) -> WarpSets {
        let Deformer::Warp(warp) = &self.model.deformers[slot] else {
            unreachable!("effective_warp_sets is only called for warp deformers")
        };
        if let Some(sets) = self.warp_cache.get(&slot) {
            return WarpSets {
                rest: sets.rest.clone(),
                current: sets.current.clone(),
            };
        }
        let state: Vec<[f64; 2]> = match &warp.keyforms {
            Some(lattice) => sample_point_sets(lattice, self.values, |form| &form.positions),
            None => warp
                .rest_positions
                .iter()
                .map(|point| [f64::from(point[0]), f64::from(point[1])])
                .collect(),
        };
        let mut rest: Vec<[f64; 2]> = warp
            .rest_positions
            .iter()
            .map(|point| [f64::from(point[0]), f64::from(point[1])])
            .collect();
        let mut current = state;
        for (position, &ancestor) in ancestors.iter().enumerate() {
            let prefix = &ancestors[..position];
            rest = rest
                .iter()
                .map(|&point| self.apply_deformer(ancestor, prefix, point))
                .collect();
            current = current
                .iter()
                .map(|&point| self.apply_deformer(ancestor, prefix, point))
                .collect();
        }
        let sets = WarpSets { rest, current };
        self.warp_cache.insert(
            slot,
            WarpSets {
                rest: sets.rest.clone(),
                current: sets.current.clone(),
            },
        );
        sets
    }

    /// The accumulated translation of a part chain: the
    /// part's own sampled translation plus every ancestor's,
    /// summed root-down (translations commute, so the sum is
    /// the composition).
    fn part_translation(&self, part_id: &str) -> [f64; 2] {
        let mut total = [0.0_f64; 2];
        let mut cursor = self.part_index.get(part_id).copied();
        while let Some(slot) = cursor {
            let part = &self.model.parts[slot];
            if let Some(lattice) = &part.translation_keyforms {
                let own = sample_translation(lattice, self.values);
                total[0] += own[0];
                total[1] += own[1];
            }
            cursor = part
                .parent
                .as_deref()
                .and_then(|parent| self.part_index.get(parent).copied());
        }
        total
    }

    /// Section 3.2: a mesh's effective opacity is its own
    /// opacity times the product of its part chain's.
    fn effective_opacity(&self, mesh: &Mesh) -> f32 {
        let mut opacity = mesh.opacity;
        let mut cursor = self.part_index.get(mesh.part.as_str()).copied();
        while let Some(slot) = cursor {
            let part = &self.model.parts[slot];
            opacity *= part.opacity;
            cursor = part
                .parent
                .as_deref()
                .and_then(|parent| self.part_index.get(parent).copied());
        }
        opacity
    }
}

/// Rotate and scale a point about an origin: the INP node
/// transform's action on a point, `origin + R(angle) * S *
/// (point - origin)`, angle in degrees, counter-clockwise
/// (the T * R * S composition order of the reference
/// transform).
fn rotate_scale(
    point: [f64; 2],
    origin: [f64; 2],
    angle_degrees: f64,
    scale: [f64; 2],
) -> [f64; 2] {
    let local = [
        (point[0] - origin[0]) * scale[0],
        (point[1] - origin[1]) * scale[1],
    ];
    let radians = angle_degrees.to_radians();
    let (sin, cos) = radians.sin_cos();
    [
        origin[0] + cos * local[0] - sin * local[1],
        origin[1] + sin * local[0] + cos * local[1],
    ]
}

/// Map one point through a warp grid: locate the point in
/// the rest grid, solve the cell's inverse bilinear, and
/// evaluate the same coordinates in the current grid.
/// Points outside every rest cell use the nearest cell (by
/// rest-centroid distance) with coordinates clamped to the
/// cell (edge extension). Cell scan order is row-major, so
/// shared-edge points resolve deterministically.
fn warp_map(sets: &WarpSets, columns: usize, rows: usize, point: [f64; 2]) -> [f64; 2] {
    let stride = columns + 1;
    let corner = |grid: &[[f64; 2]], row: usize, col: usize| grid[row * stride + col];
    let mut nearest: Option<(usize, usize, f64)> = None;
    for row in 0..rows {
        for col in 0..columns {
            let rest_quad = [
                corner(&sets.rest, row, col),
                corner(&sets.rest, row, col + 1),
                corner(&sets.rest, row + 1, col + 1),
                corner(&sets.rest, row + 1, col),
            ];
            if let Some((u, v)) = inverse_bilinear(rest_quad, point) {
                if (-1e-9..=1.0 + 1e-9).contains(&u) && (-1e-9..=1.0 + 1e-9).contains(&v) {
                    let current_quad = [
                        corner(&sets.current, row, col),
                        corner(&sets.current, row, col + 1),
                        corner(&sets.current, row + 1, col + 1),
                        corner(&sets.current, row + 1, col),
                    ];
                    return bilinear(current_quad, u.clamp(0.0, 1.0), v.clamp(0.0, 1.0));
                }
                let centroid = [
                    (rest_quad[0][0] + rest_quad[2][0]) / 2.0,
                    (rest_quad[0][1] + rest_quad[2][1]) / 2.0,
                ];
                let distance = (centroid[0] - point[0]).powi(2) + (centroid[1] - point[1]).powi(2);
                if nearest.is_none_or(|(_, _, best)| distance < best) {
                    nearest = Some((row, col, distance));
                }
            }
        }
    }
    // Outside every cell (or a degenerate grid): nearest
    // cell, clamped coordinates. A grid whose every cell is
    // degenerate leaves the point unmoved.
    let Some((row, col, _)) = nearest else {
        return point;
    };
    let rest_quad = [
        corner(&sets.rest, row, col),
        corner(&sets.rest, row, col + 1),
        corner(&sets.rest, row + 1, col + 1),
        corner(&sets.rest, row + 1, col),
    ];
    let (u, v) = inverse_bilinear(rest_quad, point).unwrap_or((0.5, 0.5));
    let current_quad = [
        corner(&sets.current, row, col),
        corner(&sets.current, row, col + 1),
        corner(&sets.current, row + 1, col + 1),
        corner(&sets.current, row + 1, col),
    ];
    bilinear(current_quad, u.clamp(0.0, 1.0), v.clamp(0.0, 1.0))
}

/// The bilinear patch: `(1-u)(1-v) A + u(1-v) B + u v C +
/// (1-u) v D` for corners A=(0,0), B=(1,0), C=(1,1),
/// D=(0,1).
fn bilinear(quad: [[f64; 2]; 4], u: f64, v: f64) -> [f64; 2] {
    let weights = [(1.0 - u) * (1.0 - v), u * (1.0 - v), u * v, (1.0 - u) * v];
    let mut out = [0.0_f64; 2];
    for (corner, weight) in quad.iter().zip(weights.iter()) {
        out[0] += weight * corner[0];
        out[1] += weight * corner[1];
    }
    out
}

/// Solve the inverse bilinear for `point` in `quad` by
/// Newton-Raphson from the cell centre: a fixed 16
/// iterations (the format's determinism law — an evaluator
/// that iterates differently produces different bits), with
/// a barycentric fallback on the first triangle when the
/// Jacobian vanishes. Returns `None` only when both fail.
fn inverse_bilinear(quad: [[f64; 2]; 4], point: [f64; 2]) -> Option<(f64, f64)> {
    let (mut u, mut v) = (0.5_f64, 0.5_f64);
    for _ in 0..16 {
        let current = bilinear(quad, u, v);
        let error = [point[0] - current[0], point[1] - current[1]];
        if error[0].abs() <= 1e-12 && error[1].abs() <= 1e-12 {
            return Some((u, v));
        }
        // Jacobian of the bilinear patch at (u, v).
        let du = [
            (1.0 - v) * (quad[1][0] - quad[0][0]) + v * (quad[2][0] - quad[3][0]),
            (1.0 - v) * (quad[1][1] - quad[0][1]) + v * (quad[2][1] - quad[3][1]),
        ];
        let dv = [
            (1.0 - u) * (quad[3][0] - quad[0][0]) + u * (quad[2][0] - quad[1][0]),
            (1.0 - u) * (quad[3][1] - quad[0][1]) + u * (quad[2][1] - quad[1][1]),
        ];
        let determinant = du[0] * dv[1] - du[1] * dv[0];
        if determinant.abs() <= 1e-15 {
            return barycentric_fallback(quad, point);
        }
        u += (error[0] * dv[1] - error[1] * dv[0]) / determinant;
        v += (du[0] * error[1] - du[1] * error[0]) / determinant;
    }
    let current = bilinear(quad, u, v);
    if (point[0] - current[0]).abs() <= 1e-9 && (point[1] - current[1]).abs() <= 1e-9 {
        Some((u, v))
    } else {
        barycentric_fallback(quad, point)
    }
}

/// Barycentric coordinates of `point` in the quad's first
/// triangle (A, B, C), reported as bilinear coordinates —
/// exact for affine cells, a defined fallback otherwise.
fn barycentric_fallback(quad: [[f64; 2]; 4], point: [f64; 2]) -> Option<(f64, f64)> {
    let [a, b, c] = [quad[0], quad[1], quad[2]];
    let denominator = (b[1] - c[1]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[1] - c[1]);
    if denominator.abs() <= 1e-15 {
        return None;
    }
    let wa = ((b[1] - c[1]) * (point[0] - c[0]) + (c[0] - b[0]) * (point[1] - c[1])) / denominator;
    let wb = ((c[1] - a[1]) * (point[0] - c[0]) + (a[0] - c[0]) * (point[1] - c[1])) / denominator;
    let wc = 1.0 - wa - wb;
    // Bilinear (u, v) whose patch weights match (wa, wb, wc)
    // on the first triangle: u = wb + wc, v = wc.
    Some((wb + wc, wc))
}
