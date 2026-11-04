//! The puppet evaluation stage (SPEC.md section 13.4–13.7):
//! parameter grids, node transform composition, mesh
//! deformation, drawable opacity/blending, masks, and
//! fixed-step pendulum physics, plus the deterministic CPU
//! rasterizer the conformance proof renders with.
//!
//! Arithmetic law (SPEC 13.2): this stage evaluates in f32,
//! mirroring the reference runtime's integrators and grid
//! math operation-for-operation, because the stage's contract
//! is parity with that runtime. (The model's section-5 strand
//! physics is a different system with its own f64 law.)
//! Iteration order is fixed everywhere — parameters, bindings,
//! nodes, and physics all run in asset order — so evaluation
//! is a pure function of (stage, textures, parameter values,
//! step sequence).

use std::collections::HashMap;

use crate::puppet::{
    PuppetBindingKind, PuppetBlendMode, PuppetError, PuppetInterpolate, PuppetMapMode,
    PuppetMaskMode, PuppetNodeKind, PuppetPhysicsModel, PuppetStage,
};

/// One decoded RGBA8 texture, rows top-first, in stage
/// texture-table order at the evaluator.
#[derive(Debug, Clone, PartialEq)]
pub struct RgbaImage {
    /// Width in pixels.
    pub width: usize,
    /// Height in pixels.
    pub height: usize,
    /// `width * height * 4` bytes, RGBA per pixel.
    pub rgba: Vec<u8>,
}

/* ------------------------------------------------------------------ */
/* Minimal f32 4x4 matrix (column-major, glam-compatible layout)       */
/* ------------------------------------------------------------------ */

#[derive(Debug, Clone, Copy)]
struct Mat4([f32; 16]);

impl Mat4 {
    fn identity() -> Self {
        let mut m = [0.0f32; 16];
        m[0] = 1.0;
        m[5] = 1.0;
        m[10] = 1.0;
        m[15] = 1.0;
        Mat4(m)
    }

    /// `T * R * S` with R from Euler XYZ angles in radians —
    /// the reference transform composition.
    fn from_trs(translation: [f32; 3], rotation_rad: [f32; 3], scale: [f32; 2]) -> Self {
        // Quaternion q = qx * qy * qz (Hamilton product),
        // matching glam's EulerRot::XYZ.
        let (sx, cx) = ((rotation_rad[0] / 2.0).sin(), (rotation_rad[0] / 2.0).cos());
        let (sy, cy) = ((rotation_rad[1] / 2.0).sin(), (rotation_rad[1] / 2.0).cos());
        let (sz, cz) = ((rotation_rad[2] / 2.0).sin(), (rotation_rad[2] / 2.0).cos());
        // qy * qz first, then qx * (qy * qz).
        let r = quat_mul([0.0, sy, 0.0, cy], [0.0, 0.0, sz, cz]);
        let q = quat_mul([sx, 0.0, 0.0, cx], r);
        let (x, y, z, w) = (q[0], q[1], q[2], q[3]);
        // Rotation matrix from the quaternion (glam's layout).
        let (xx, yy, zz) = (x * x, y * y, z * z);
        let (xy, xz, yz) = (x * y, x * z, y * z);
        let (wx, wy, wz) = (w * x, w * y, w * z);
        let mut m = [0.0f32; 16];
        m[0] = (1.0 - 2.0 * (yy + zz)) * scale[0];
        m[1] = (2.0 * (xy + wz)) * scale[0];
        m[2] = (2.0 * (xz - wy)) * scale[0];
        m[4] = (2.0 * (xy - wz)) * scale[1];
        m[5] = (1.0 - 2.0 * (xx + zz)) * scale[1];
        m[6] = (2.0 * (yz + wx)) * scale[1];
        m[8] = 2.0 * (xz + wy);
        m[9] = 2.0 * (yz - wx);
        m[10] = 1.0 - 2.0 * (xx + yy);
        m[12] = translation[0];
        m[13] = translation[1];
        m[14] = translation[2];
        m[15] = 1.0;
        Mat4(m)
    }

    fn mul(&self, rhs: &Mat4) -> Mat4 {
        let mut out = [0.0f32; 16];
        for col in 0..4 {
            for row in 0..4 {
                let mut sum = 0.0f32;
                for k in 0..4 {
                    sum += self.0[k * 4 + row] * rhs.0[col * 4 + k];
                }
                out[col * 4 + row] = sum;
            }
        }
        Mat4(out)
    }

    fn transform_point(&self, p: [f32; 3]) -> [f32; 3] {
        let m = &self.0;
        [
            m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12],
            m[1] * p[0] + m[5] * p[1] + m[9] * p[2] + m[13],
            m[2] * p[0] + m[6] * p[1] + m[10] * p[2] + m[14],
        ]
    }

    /// General 4x4 inverse (adjugate). Singular input yields
    /// non-finite entries, as the reference's inverse does.
    fn inverse(&self) -> Mat4 {
        let m = &self.0;
        let mut inv = [0.0f32; 16];
        inv[0] = m[5] * m[10] * m[15] - m[5] * m[11] * m[14] - m[9] * m[6] * m[15]
            + m[9] * m[7] * m[14]
            + m[13] * m[6] * m[11]
            - m[13] * m[7] * m[10];
        inv[4] = -m[4] * m[10] * m[15] + m[4] * m[11] * m[14] + m[8] * m[6] * m[15]
            - m[8] * m[7] * m[14]
            - m[12] * m[6] * m[11]
            + m[12] * m[7] * m[10];
        inv[8] = m[4] * m[9] * m[15] - m[4] * m[11] * m[13] - m[8] * m[5] * m[15]
            + m[8] * m[7] * m[13]
            + m[12] * m[5] * m[11]
            - m[12] * m[7] * m[9];
        inv[12] = -m[4] * m[9] * m[14] + m[4] * m[10] * m[13] + m[8] * m[5] * m[14]
            - m[8] * m[6] * m[13]
            - m[12] * m[5] * m[10]
            + m[12] * m[6] * m[9];
        inv[1] = -m[1] * m[10] * m[15] + m[1] * m[11] * m[14] + m[9] * m[2] * m[15]
            - m[9] * m[3] * m[14]
            - m[13] * m[2] * m[11]
            + m[13] * m[3] * m[10];
        inv[5] = m[0] * m[10] * m[15] - m[0] * m[11] * m[14] - m[8] * m[2] * m[15]
            + m[8] * m[3] * m[14]
            + m[12] * m[2] * m[11]
            - m[12] * m[3] * m[10];
        inv[9] = -m[0] * m[9] * m[15] + m[0] * m[11] * m[13] + m[8] * m[1] * m[15]
            - m[8] * m[3] * m[13]
            - m[12] * m[1] * m[11]
            + m[12] * m[3] * m[9];
        inv[13] = m[0] * m[9] * m[14] - m[0] * m[10] * m[13] - m[8] * m[1] * m[14]
            + m[8] * m[2] * m[13]
            + m[12] * m[1] * m[10]
            - m[12] * m[2] * m[9];
        inv[2] = m[1] * m[6] * m[15] - m[1] * m[7] * m[14] - m[5] * m[2] * m[15]
            + m[5] * m[3] * m[14]
            + m[13] * m[2] * m[7]
            - m[13] * m[3] * m[6];
        inv[6] = -m[0] * m[6] * m[15] + m[0] * m[7] * m[14] + m[4] * m[2] * m[15]
            - m[4] * m[3] * m[14]
            - m[12] * m[2] * m[7]
            + m[12] * m[3] * m[6];
        inv[10] = m[0] * m[5] * m[15] - m[0] * m[7] * m[13] - m[4] * m[1] * m[15]
            + m[4] * m[3] * m[13]
            + m[12] * m[1] * m[7]
            - m[12] * m[3] * m[5];
        inv[14] = -m[0] * m[5] * m[14] + m[0] * m[6] * m[13] + m[4] * m[1] * m[14]
            - m[4] * m[2] * m[13]
            - m[12] * m[1] * m[6]
            + m[12] * m[2] * m[5];
        inv[3] = -m[1] * m[6] * m[11] + m[1] * m[7] * m[10] + m[5] * m[2] * m[11]
            - m[5] * m[3] * m[10]
            - m[9] * m[2] * m[7]
            + m[9] * m[3] * m[6];
        inv[7] = m[0] * m[6] * m[11] - m[0] * m[7] * m[10] - m[4] * m[2] * m[11]
            + m[4] * m[3] * m[10]
            + m[8] * m[2] * m[7]
            - m[8] * m[3] * m[6];
        inv[11] = -m[0] * m[5] * m[11] + m[0] * m[7] * m[9] + m[4] * m[1] * m[11]
            - m[4] * m[3] * m[9]
            - m[8] * m[1] * m[7]
            + m[8] * m[3] * m[5];
        inv[15] = m[0] * m[5] * m[10] - m[0] * m[6] * m[9] - m[4] * m[1] * m[10]
            + m[4] * m[2] * m[9]
            + m[8] * m[1] * m[6]
            - m[8] * m[2] * m[5];
        let det = m[0] * inv[0] + m[1] * inv[4] + m[2] * inv[8] + m[3] * inv[12];
        let inv_det = 1.0 / det;
        for v in inv.iter_mut() {
            *v *= inv_det;
        }
        Mat4(inv)
    }
}

fn quat_mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let (ax, ay, az, aw) = (a[0], a[1], a[2], a[3]);
    let (bx, by, bz, bw) = (b[0], b[1], b[2], b[3]);
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

/* ------------------------------------------------------------------ */
/* Grid math                                                           */
/* ------------------------------------------------------------------ */

/// Locate `v` on an axis: `(low index, high index, offset)`.
/// Mirrors the reference search, including its clamping at
/// both ends (SPEC 13.4).
fn axis_interval(points: &[f32], v: f32) -> (usize, usize, f32) {
    let n = points.len();
    if n == 1 {
        return (0, 0, 0.0);
    }
    if v <= points[0] {
        return (0, 1, 0.0);
    }
    if v >= points[n - 1] {
        return (n - 2, n - 1, 1.0);
    }
    let mut i = 0usize;
    while points[i + 1] <= v {
        i += 1;
    }
    (i, i + 1, (v - points[i]) / (points[i + 1] - points[i]))
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Sample a scalar grid at `(ix, iy)` with offsets `(fx, fy)`.
fn sample_scalar(
    grid: &[Vec<f32>],
    ix: (usize, usize),
    iy: (usize, usize),
    fx: f32,
    fy: f32,
    mode: PuppetInterpolate,
) -> f32 {
    match mode {
        PuppetInterpolate::Nearest => {
            let x = if fx < 0.5 { ix.0 } else { ix.1 };
            let y = if fy < 0.5 { iy.0 } else { iy.1 };
            grid[x][y]
        }
        _ => {
            let top = lerp(grid[ix.0][iy.0], grid[ix.1][iy.0], fx);
            let bottom = lerp(grid[ix.0][iy.1], grid[ix.1][iy.1], fx);
            lerp(top, bottom, fy)
        }
    }
}

/// Sample a deform grid: per-vertex interpolated displacement.
fn sample_deform(
    grid: &[Vec<Vec<[f32; 2]>>],
    ix: (usize, usize),
    iy: (usize, usize),
    fx: f32,
    fy: f32,
    mode: PuppetInterpolate,
    out: &mut [[f32; 2]],
) {
    match mode {
        PuppetInterpolate::Nearest => {
            let x = if fx < 0.5 { ix.0 } else { ix.1 };
            let y = if fy < 0.5 { iy.0 } else { iy.1 };
            out.copy_from_slice(&grid[x][y]);
        }
        _ => {
            for (i, o) in out.iter_mut().enumerate() {
                let top = [
                    lerp(grid[ix.0][iy.0][i][0], grid[ix.1][iy.0][i][0], fx),
                    lerp(grid[ix.0][iy.0][i][1], grid[ix.1][iy.0][i][1], fx),
                ];
                let bottom = [
                    lerp(grid[ix.0][iy.1][i][0], grid[ix.1][iy.1][i][0], fx),
                    lerp(grid[ix.0][iy.1][i][1], grid[ix.1][iy.1][i][1], fx),
                ];
                *o = [lerp(top[0], bottom[0], fy), lerp(top[1], bottom[1], fy)];
            }
        }
    }
}

/* ------------------------------------------------------------------ */
/* Pendulum physics (f32 port of the reference integrators)            */
/* ------------------------------------------------------------------ */

#[derive(Debug, Clone, Copy)]
enum PendulumState {
    Rigid {
        theta: f32,
        omega: f32,
        bob: [f32; 2],
    },
    Spring {
        pos: [f32; 2],
        vel: [f32; 2],
    },
}

/// Classic RK4 with the reference's non-finite guard: a step
/// producing any non-finite component is reverted whole.
fn rk4<const N: usize>(vars: &mut [f32; N], h: f32, eval: &dyn Fn(&[f32; N]) -> [f32; N]) {
    let curs = *vars;
    let k1 = eval(&curs);
    let mut stage = [0.0f32; N];
    for i in 0..N {
        stage[i] = curs[i] + h * k1[i] / 2.0;
    }
    let k2 = eval(&stage);
    for i in 0..N {
        stage[i] = curs[i] + h * k2[i] / 2.0;
    }
    let k3 = eval(&stage);
    for i in 0..N {
        stage[i] = curs[i] + h * k3[i];
    }
    let k4 = eval(&stage);
    let mut next = [0.0f32; N];
    for i in 0..N {
        next[i] = curs[i] + h * (k1[i] + 2.0 * k2[i] + 2.0 * k3[i] + k4[i]) / 6.0;
        if !next[i].is_finite() {
            return; // revert: keep `vars` untouched
        }
    }
    *vars = next;
}

#[derive(Debug)]
struct PhysDef {
    node: usize,
    param: usize,
    state: PendulumState,
}

/* ------------------------------------------------------------------ */
/* The evaluator                                                       */
/* ------------------------------------------------------------------ */

/// A puppet evaluation session: a validated stage, its
/// textures, current parameter values, and physics state.
///
/// Frame protocol (mirrors the reference driver contract):
/// [`begin_frame`](PuppetEval::begin_frame) resets parameter
/// values to defaults, the caller sets values, then
/// [`end_frame`](PuppetEval::end_frame) evaluates the frame;
/// [`render`](PuppetEval::render) rasterizes the result.
#[derive(Debug)]
pub struct PuppetEval<'a> {
    stage: &'a PuppetStage,
    textures: &'a [RgbaImage],
    param_index: HashMap<&'a str, usize>,
    node_index: HashMap<&'a str, usize>,
    texture_index: HashMap<&'a str, usize>,
    /// Pre-order traversal of the node forest.
    preorder: Vec<usize>,
    /// Children per node, in asset order.
    children: Vec<Vec<usize>>,
    root: usize,
    /// Bindings grouped by parameter index, asset order kept.
    bindings_by_param: Vec<Vec<usize>>,
    physics_defs: Vec<PhysDef>,
    values: Vec<[f32; 2]>,
    // Per-frame scratch, rebuilt by every apply.
    rel_translation: Vec<[f32; 3]>,
    rel_rotation_deg: Vec<[f32; 3]>,
    rel_scale: Vec<[f32; 2]>,
    rel_zsort: Vec<f32>,
    opacity_factor: Vec<f32>,
    deforms: Vec<Vec<[f32; 2]>>,
    absolute: Vec<Mat4>,
    zsort_abs: Vec<f32>,
    /// Proof-harness switch: reproduce the reference
    /// runtime's (0,0)-skip in place of the reference
    /// semantics this stage implements (SPEC 13.4). Never set
    /// outside the parity proof.
    zero_skip: bool,
}

impl<'a> PuppetEval<'a> {
    /// Build a session from a validated stage and its decoded
    /// textures (in stage texture-table order).
    pub fn new(stage: &'a PuppetStage, textures: &'a [RgbaImage]) -> Result<Self, PuppetError> {
        if textures.len() != stage.textures.len() {
            return Err(PuppetError::TextureCount {
                expected: stage.textures.len(),
                found: textures.len(),
            });
        }
        for img in textures.iter() {
            if img.rgba.len() != img.width * img.height * 4 {
                return Err(PuppetError::TextureCount {
                    expected: img.width * img.height * 4,
                    found: img.rgba.len(),
                });
            }
        }
        let param_index: HashMap<&str, usize> = stage
            .parameters
            .iter()
            .enumerate()
            .map(|(i, p)| (p.id.as_str(), i))
            .collect();
        let node_index: HashMap<&str, usize> = stage
            .nodes
            .iter()
            .enumerate()
            .map(|(i, n)| (n.id.as_str(), i))
            .collect();
        let texture_index: HashMap<&str, usize> = stage
            .textures
            .iter()
            .enumerate()
            .map(|(i, t)| (t.id.as_str(), i))
            .collect();

        let mut children: Vec<Vec<usize>> = vec![Vec::new(); stage.nodes.len()];
        let mut root = 0usize;
        for (i, node) in stage.nodes.iter().enumerate() {
            match &node.parent {
                Some(parent) => children[node_index[parent.as_str()]].push(i),
                None => root = i,
            }
        }
        let mut preorder: Vec<usize> = Vec::with_capacity(stage.nodes.len());
        let mut stack: Vec<usize> = vec![root];
        while let Some(cur) = stack.pop() {
            preorder.push(cur);
            for &child in children[cur].iter().rev() {
                stack.push(child);
            }
        }

        let mut bindings_by_param: Vec<Vec<usize>> = vec![Vec::new(); stage.parameters.len()];
        for (bi, binding) in stage.bindings.iter().enumerate() {
            bindings_by_param[param_index[binding.parameter.as_str()]].push(bi);
        }

        let mut physics_defs: Vec<PhysDef> = Vec::new();
        for (ni, node) in stage.nodes.iter().enumerate() {
            if let Some(phys) = &node.physics {
                let state = match phys.model {
                    PuppetPhysicsModel::Rigid => PendulumState::Rigid {
                        theta: 0.0,
                        omega: 0.0,
                        bob: [0.0, 0.0],
                    },
                    PuppetPhysicsModel::Spring => PendulumState::Spring {
                        pos: [0.0, 0.0],
                        vel: [0.0, 0.0],
                    },
                };
                physics_defs.push(PhysDef {
                    node: ni,
                    param: param_index[phys.parameter.as_str()],
                    state,
                });
            }
        }

        let node_count = stage.nodes.len();
        Ok(PuppetEval {
            stage,
            textures,
            param_index,
            node_index,
            texture_index,
            preorder,
            children,
            root,
            bindings_by_param,
            physics_defs,
            values: stage.parameters.iter().map(|p| p.default).collect(),
            rel_translation: vec![[0.0; 3]; node_count],
            rel_rotation_deg: vec![[0.0; 3]; node_count],
            rel_scale: vec![[1.0; 2]; node_count],
            rel_zsort: vec![0.0; node_count],
            opacity_factor: vec![1.0; node_count],
            deforms: stage
                .nodes
                .iter()
                .map(|n| {
                    n.mesh
                        .as_ref()
                        .map(|m| vec![[0.0f32; 2]; m.vertices.len()])
                        .unwrap_or_default()
                })
                .collect(),
            absolute: vec![Mat4::identity(); node_count],
            zsort_abs: vec![0.0; node_count],
            zero_skip: false,
        })
    }

    /// Enable the proof-only (0,0)-skip reproduction. This
    /// exists so the parity proof can decompose the frame
    /// delta: with the skip on, this evaluator reproduces the
    /// shipped reference runtime's effective behavior.
    pub fn set_zero_skip_for_proof(&mut self, on: bool) {
        self.zero_skip = on;
    }

    /// Reset every parameter value to its default. Physics
    /// state persists across frames, as in the reference.
    pub fn begin_frame(&mut self) {
        for (value, param) in self.values.iter_mut().zip(self.stage.parameters.iter()) {
            *value = param.default;
        }
    }

    /// Set one parameter's value by stage parameter id.
    /// Values are clamped to the parameter's range when the
    /// frame applies (the reference's apply-time clamp).
    pub fn set_param(&mut self, id: &str, value: [f32; 2]) -> Result<(), PuppetError> {
        match self.param_index.get(id) {
            Some(&i) => {
                self.values[i] = value;
                Ok(())
            }
            None => Err(PuppetError::DanglingReference {
                context: "set_param".to_string(),
                id: id.to_string(),
            }),
        }
    }

    /// The current (unclamped) value of a parameter.
    pub fn param_value(&self, id: &str) -> Option<[f32; 2]> {
        self.param_index.get(id).map(|&i| self.values[i])
    }

    /// Evaluate one frame: apply parameters, compose
    /// transforms, step physics `dt` seconds, write physics
    /// outputs, then re-apply and re-compose (the reference
    /// frame order, SPEC 13.7).
    pub fn end_frame(&mut self, dt: f32) -> Result<(), PuppetError> {
        if !dt.is_finite() || dt < 0.0 {
            return Err(PuppetError::InvalidStep { dt });
        }
        self.apply_params();
        self.compose();
        if dt > 0.0 {
            let outputs = self.step_physics(dt);
            for (param, value) in outputs {
                self.values[param] = value;
            }
        }
        self.apply_params();
        self.compose();
        Ok(())
    }

    /// Reset the per-frame scratch to base state and apply
    /// every parameter's bindings. Reference semantics: a
    /// parameter applies at EVERY value, including exactly
    /// (0, 0) (SPEC 13.4).
    fn apply_params(&mut self) {
        for (i, node) in self.stage.nodes.iter().enumerate() {
            self.rel_translation[i] = node.transform.translation;
            self.rel_rotation_deg[i] = node.transform.rotation_degrees;
            self.rel_scale[i] = node.transform.scale;
            self.rel_zsort[i] = node.zsort;
            self.opacity_factor[i] = 1.0;
            for d in self.deforms[i].iter_mut() {
                *d = [0.0, 0.0];
            }
        }
        // Disjoint field borrows for the deform scratch.
        let stage = self.stage;
        let values = &self.values;
        let zero_skip = self.zero_skip;
        for (pi, param) in stage.parameters.iter().enumerate() {
            let raw = values[pi];
            if zero_skip && raw == [0.0, 0.0] {
                continue;
            }
            let v = [
                raw[0].clamp(param.min[0], param.max[0]),
                raw[1].clamp(param.min[1], param.max[1]),
            ];
            let (x0, x1, fx) = axis_interval(&param.axis_points[0], v[0]);
            let (y0, y1, fy) = if param.y.is_some() {
                axis_interval(&param.axis_points[1], v[1])
            } else {
                (0, 0, 0.0)
            };
            for &bi in &self.bindings_by_param[pi] {
                let binding = &stage.bindings[bi];
                if binding.interpolate == PuppetInterpolate::Cubic {
                    // Carried, not applied (SPEC 13.4).
                    continue;
                }
                let ni = self.node_index[binding.node.as_str()];
                match &binding.values {
                    crate::puppet::PuppetBindingValues::Scalar(grid) => {
                        let val =
                            sample_scalar(grid, (x0, x1), (y0, y1), fx, fy, binding.interpolate);
                        match binding.kind {
                            PuppetBindingKind::TranslationX => self.rel_translation[ni][0] += val,
                            PuppetBindingKind::TranslationY => self.rel_translation[ni][1] += val,
                            PuppetBindingKind::ScaleX => self.rel_scale[ni][0] *= val,
                            PuppetBindingKind::ScaleY => self.rel_scale[ni][1] *= val,
                            PuppetBindingKind::RotationX => self.rel_rotation_deg[ni][0] += val,
                            PuppetBindingKind::RotationY => self.rel_rotation_deg[ni][1] += val,
                            PuppetBindingKind::RotationZ => self.rel_rotation_deg[ni][2] += val,
                            PuppetBindingKind::ZSort => self.rel_zsort[ni] += val,
                            PuppetBindingKind::Opacity => self.opacity_factor[ni] *= val,
                            PuppetBindingKind::Deform => {
                                unreachable!("deform bindings carry deform grids")
                            }
                        }
                    }
                    crate::puppet::PuppetBindingValues::Deform(grid) => {
                        // Mesh-group deform bindings are inert:
                        // the reference runtime has no mesh to
                        // receive them (SPEC 13.5).
                        if stage.nodes[ni].mesh.is_some() {
                            let mut sampled = vec![[0.0f32; 2]; self.deforms[ni].len()];
                            sample_deform(
                                grid,
                                (x0, x1),
                                (y0, y1),
                                fx,
                                fy,
                                binding.interpolate,
                                &mut sampled,
                            );
                            for (dst, src) in self.deforms[ni].iter_mut().zip(sampled.iter()) {
                                dst[0] += src[0];
                                dst[1] += src[1];
                            }
                        }
                    }
                }
            }
        }
    }

    /// Compose absolute transforms and accumulated zsorts in
    /// pre-order (parents before children).
    fn compose(&mut self) {
        for &ni in &self.preorder {
            let node = &self.stage.nodes[ni];
            let local = Mat4::from_trs(
                self.rel_translation[ni],
                [
                    self.rel_rotation_deg[ni][0].to_radians(),
                    self.rel_rotation_deg[ni][1].to_radians(),
                    self.rel_rotation_deg[ni][2].to_radians(),
                ],
                self.rel_scale[ni],
            );
            let (base_mat, base_z) = if node.parent.is_none() {
                (Mat4::identity(), 0.0)
            } else if node.lock_to_root {
                (self.absolute[self.root], self.zsort_abs[self.root])
            } else {
                let parent = self.node_index[node.parent.as_deref().expect("validated")];
                (self.absolute[parent], self.zsort_abs[parent])
            };
            self.absolute[ni] = if node.parent.is_none() {
                local
            } else {
                base_mat.mul(&local)
            };
            self.zsort_abs[ni] =
                self.rel_zsort[ni] + if node.parent.is_none() { 0.0 } else { base_z };
        }
    }

    /// Step every physics node; returns (parameter index,
    /// output value) pairs to write.
    fn step_physics(&mut self, dt: f32) -> Vec<(usize, [f32; 2])> {
        let globals = &self.stage.physics;
        let mut outputs: Vec<(usize, [f32; 2])> = Vec::with_capacity(self.physics_defs.len());
        for di in 0..self.physics_defs.len() {
            let node_idx = self.physics_defs[di].node;
            let phys = self.stage.nodes[node_idx]
                .physics
                .as_ref()
                .expect("physics def");
            let anchor = if phys.local_only {
                [
                    self.rel_translation[node_idx][0],
                    self.rel_translation[node_idx][1],
                ]
            } else {
                let p = self.absolute[node_idx].transform_point([0.0, 0.0, 0.0]);
                [p[0], p[1]]
            };
            // Substeps of at most 0.01 s under a 10 s clamp —
            // the reference's step partition (SPEC 13.7).
            let mut remaining = dt.min(10.0);
            while remaining > 0.0 {
                let h = remaining.min(0.01);
                tick_pendulum(&mut self.physics_defs[di].state, phys, globals, anchor, h);
                remaining -= 0.01;
            }
            let value = calc_output(
                &self.physics_defs[di].state,
                phys,
                &self.absolute[node_idx],
                anchor,
            );
            outputs.push((self.physics_defs[di].param, value));
        }
        outputs
    }

    /* -------------------------------------------------------------- */
    /* Rendering                                                       */
    /* -------------------------------------------------------------- */

    /// Rasterize the current evaluated state to an RGBA8
    /// frame (`width * height * 4` bytes, rows top-first).
    /// The camera is the reference driver's: puppet origin at
    /// the frame center, `scale` pixels per puppet pixel,
    /// transparent background (SPEC 13.6).
    pub fn render(&self, width: usize, height: usize, scale: f32) -> Vec<u8> {
        let mut frame = FrameBuf::new(width, height);
        // Root drawables: drawables whose parent is not a
        // composite, in pre-order, drawn highest zsort first
        // (stable: ties keep traversal order).
        let mut draw_list: Vec<usize> = Vec::new();
        for &ni in &self.preorder {
            if ni == self.root {
                continue;
            }
            let node = &self.stage.nodes[ni];
            if node.drawable.is_none() {
                continue;
            }
            let parent_is_composite = node
                .parent
                .as_ref()
                .map(|p| {
                    self.stage.nodes[self.node_index[p.as_str()]].kind == PuppetNodeKind::Composite
                })
                .unwrap_or(false);
            if !parent_is_composite {
                draw_list.push(ni);
            }
        }
        draw_list.sort_by(|&a, &b| self.zsort_abs[b].total_cmp(&self.zsort_abs[a]));
        for ni in draw_list {
            self.draw_drawable(&mut frame, ni, scale, None);
        }
        frame.to_rgba8()
    }

    /// A node's world-space vertices (parts only): the node's
    /// absolute transform applied to `vertex - origin +
    /// deform`.
    fn world_verts(&self, ni: usize) -> Vec<[f32; 2]> {
        let node = &self.stage.nodes[ni];
        let mesh = node.mesh.as_ref().expect("part has a mesh");
        let mat = &self.absolute[ni];
        mesh.vertices
            .iter()
            .zip(self.deforms[ni].iter())
            .map(|(v, d)| {
                let local = [
                    v[0] - mesh.origin[0] + d[0],
                    v[1] - mesh.origin[1] + d[1],
                    0.0,
                ];
                let w = mat.transform_point(local);
                [w[0], w[1]]
            })
            .collect()
    }

    fn draw_drawable(
        &self,
        frame: &mut FrameBuf,
        ni: usize,
        scale: f32,
        coverage: Option<&[bool]>,
    ) {
        let node = &self.stage.nodes[ni];
        // Masks: build this drawable's own coverage and
        // intersect with any inherited coverage.
        if node.masks.is_empty() {
            self.draw_drawable_body(frame, ni, scale, coverage);
            return;
        }
        let own = self.mask_coverage(ni, frame.width, frame.height, scale);
        match coverage {
            None => self.draw_drawable_body(frame, ni, scale, Some(&own)),
            Some(parent) => {
                let merged: Vec<bool> = parent
                    .iter()
                    .zip(own.iter())
                    .map(|(a, b)| *a && *b)
                    .collect();
                self.draw_drawable_body(frame, ni, scale, Some(&merged));
            }
        }
    }

    fn draw_drawable_body(
        &self,
        frame: &mut FrameBuf,
        ni: usize,
        scale: f32,
        coverage: Option<&[bool]>,
    ) {
        let node = &self.stage.nodes[ni];
        match node.kind {
            PuppetNodeKind::Part => {
                self.rasterize_part(frame, ni, scale, coverage, false);
            }
            PuppetNodeKind::Composite => {
                // Direct drawable children only (the
                // reference's grouping rule), zsort descending.
                let mut kids: Vec<usize> = self.children[ni]
                    .iter()
                    .copied()
                    .filter(|&c| self.stage.nodes[c].drawable.is_some())
                    .collect();
                if kids.is_empty() {
                    return;
                }
                kids.sort_by(|&a, &b| self.zsort_abs[b].total_cmp(&self.zsort_abs[a]));
                let mut off = FrameBuf::new(frame.width, frame.height);
                for kid in kids {
                    self.draw_drawable(&mut off, kid, scale, None);
                }
                // Composite pass: the offscreen buffer sampled
                // 1:1 through the composite's drawable state.
                let drawable = node.drawable.as_ref().expect("composite drawable");
                let opacity = (drawable.opacity * self.opacity_factor[ni]).clamp(0.0, 1.0);
                for (i, texel) in off.px.iter().enumerate() {
                    if let Some(cov) = coverage
                        && !cov[i]
                    {
                        continue;
                    }
                    let src = shade(*texel, drawable, opacity);
                    frame.blend(i % frame.width, i / frame.width, src, drawable.blend_mode);
                }
            }
            _ => {}
        }
    }

    /// Coverage buffer for one masked drawable: mask sources
    /// applied in order with REPLACE semantics (a Mask source
    /// sets covered pixels, a DodgeMask source clears them),
    /// exactly the reference's stencil flow.
    fn mask_coverage(&self, ni: usize, width: usize, height: usize, scale: f32) -> Vec<bool> {
        let node = &self.stage.nodes[ni];
        let mut coverage = vec![false; width * height];
        for mask in &node.masks {
            let source = self.node_index[mask.source.as_str()];
            let passed = self.mask_source_alpha(source, width, height, scale, node.mask_threshold);
            match mask.mode {
                PuppetMaskMode::Mask => {
                    for (c, p) in coverage.iter_mut().zip(passed.iter()) {
                        if *p {
                            *c = true;
                        }
                    }
                }
                PuppetMaskMode::DodgeMask => {
                    for (c, p) in coverage.iter_mut().zip(passed.iter()) {
                        if *p {
                            *c = false;
                        }
                    }
                }
            }
        }
        coverage
    }

    /// Where a mask source's albedo alpha passes the
    /// threshold, honoring the source's own masks recursively.
    fn mask_source_alpha(
        &self,
        ni: usize,
        width: usize,
        height: usize,
        scale: f32,
        threshold: f32,
    ) -> Vec<bool> {
        let node = &self.stage.nodes[ni];
        let coverage = if node.masks.is_empty() {
            None
        } else {
            Some(self.mask_coverage(ni, width, height, scale))
        };
        let mut alpha = vec![0.0f32; width * height];
        self.rasterize_part_alpha(&mut alpha, width, height, ni, scale, coverage.as_deref());
        alpha.iter().map(|a| *a > threshold).collect()
    }

    /// Rasterize a part's triangles into the frame through
    /// its drawable state.
    fn rasterize_part(
        &self,
        frame: &mut FrameBuf,
        ni: usize,
        scale: f32,
        coverage: Option<&[bool]>,
        _as_mask: bool,
    ) {
        let node = &self.stage.nodes[ni];
        let mesh = node.mesh.as_ref().expect("part mesh");
        let drawable = node.drawable.as_ref().expect("part drawable");
        let opacity = drawable.opacity * self.opacity_factor[ni];
        let verts = self.world_verts(ni);
        let texture =
            &self.textures[self.texture_index[mesh.texture.as_deref().expect("part texture")]];
        rasterize(
            frame.width,
            frame.height,
            scale,
            &verts,
            &mesh.uvs,
            &mesh.indices,
            &mut |x, y, uv| {
                if let Some(cov) = coverage
                    && !cov[y * frame.width + x]
                {
                    return;
                }
                let texel = sample_bilinear(texture, uv[0], uv[1]);
                // No transparent-texel shortcut: the reference
                // shader's rgb is not premultiplied by alpha,
                // so a zero-alpha texel can still contribute
                // color (SPEC 13.6).
                let src = shade(texel, drawable, opacity);
                frame.blend(x, y, src, drawable.blend_mode);
            },
        );
    }

    /// Rasterize a part's sampled albedo alpha (mask pass:
    /// no opacity, no tint — the reference mask shader).
    fn rasterize_part_alpha(
        &self,
        out: &mut [f32],
        width: usize,
        height: usize,
        ni: usize,
        scale: f32,
        coverage: Option<&[bool]>,
    ) {
        let node = &self.stage.nodes[ni];
        let mesh = node.mesh.as_ref().expect("part mesh");
        let verts = self.world_verts(ni);
        let texture =
            &self.textures[self.texture_index[mesh.texture.as_deref().expect("part texture")]];
        rasterize(
            width,
            height,
            scale,
            &verts,
            &mesh.uvs,
            &mesh.indices,
            &mut |x, y, uv| {
                if let Some(cov) = coverage
                    && !cov[y * width + x]
                {
                    return;
                }
                let a = sample_bilinear(texture, uv[0], uv[1])[3];
                if a > out[y * width + x] {
                    out[y * width + x] = a;
                }
            },
        );
    }
}

/* ------------------------------------------------------------------ */
/* Pendulum tick + output (reference formulas, f32)                    */
/* ------------------------------------------------------------------ */

fn tick_pendulum(
    state: &mut PendulumState,
    phys: &crate::puppet::PuppetPhysicsNode,
    globals: &crate::puppet::PuppetPhysics,
    anchor: [f32; 2],
    h: f32,
) {
    let g = phys.gravity * globals.pixels_per_meter * globals.gravity;
    match state {
        PendulumState::Rigid { theta, omega, bob } => {
            // The angle is re-derived from the bob each tick.
            let d = [bob[0] - anchor[0], bob[1] - anchor[1]];
            *theta = (-d[0]).atan2(d[1]);
            let r = phys.length;
            let crit_damp = 2.0 * (g / r).sqrt();
            let mut vars = [*theta, *omega];
            rk4(&mut vars, h, &|v: &[f32; 2]| {
                [
                    v[1],
                    -(g / r) * v[0].sin() - v[1] * phys.angle_damping * crit_damp,
                ]
            });
            *theta = vars[0];
            *omega = vars[1];
            *bob = [
                anchor[0] + (-theta.sin()) * phys.length,
                anchor[1] + theta.cos() * phys.length,
            ];
        }
        PendulumState::Spring { pos, vel } => {
            let spring_ksqrt = phys.frequency * 2.0 * std::f32::consts::PI;
            let spring_k = spring_ksqrt * spring_ksqrt;
            let rest_length = phys.length - g / spring_k;
            let crit_damp_angle = 2.0 * (g / phys.length).sqrt();
            let crit_damp_length = 2.0 * spring_ksqrt;
            let mut vars = [pos[0], pos[1], vel[0], vel[1]];
            rk4(&mut vars, h, &|v: &[f32; 4]| {
                let off = [v[0] - anchor[0], v[1] - anchor[1]];
                let dist = (off[0] * off[0] + off[1] * off[1]).sqrt();
                let off_norm = [off[0] / dist, off[1] / dist];
                let force = [
                    0.0 - off_norm[0] * (dist - rest_length) * spring_k,
                    g - off_norm[1] * (dist - rest_length) * spring_k,
                ];
                let d_rot = [
                    v[2] * off_norm[1] + v[3] * off_norm[0],
                    v[3] * off_norm[1] - v[2] * off_norm[0],
                ];
                let dd_rot = [
                    -d_rot[0] * phys.angle_damping * crit_damp_angle,
                    -d_rot[1] * phys.length_damping * crit_damp_length,
                ];
                let dd_damp = [
                    dd_rot[0] * off_norm[1] - d_rot[1] * off_norm[0],
                    dd_rot[1] * off_norm[1] + d_rot[0] * off_norm[0],
                ];
                [v[2], v[3], force[0] + dd_damp[0], force[1] + dd_damp[1]]
            });
            *pos = [vars[0], vars[1]];
            *vel = [vars[2], vars[3]];
        }
    }
}

fn calc_output(
    state: &PendulumState,
    phys: &crate::puppet::PuppetPhysicsNode,
    absolute: &Mat4,
    anchor: [f32; 2],
) -> [f32; 2] {
    let bob = match state {
        PendulumState::Rigid { bob, .. } => *bob,
        PendulumState::Spring { pos, .. } => *pos,
    };
    let local = if phys.local_only {
        bob
    } else {
        let p = absolute.inverse().transform_point([bob[0], bob[1], 0.0]);
        [p[0], p[1]]
    };
    let len = (local[0] * local[0] + local[1] * local[1]).sqrt();
    let local_angle = [local[0] / len, local[1] / len];
    let rel_length =
        ((bob[0] - anchor[0]).powi(2) + (bob[1] - anchor[1]).powi(2)).sqrt() / phys.length;
    let result = match phys.map_mode {
        PuppetMapMode::Xy => {
            let n = [local_angle[0] * rel_length, local_angle[1] * rel_length];
            [n[0], -(n[1] - 1.0)]
        }
        PuppetMapMode::Yx => {
            let n = [local_angle[0] * rel_length, local_angle[1] * rel_length];
            [-(n[1] - 1.0), n[0]]
        }
        PuppetMapMode::AngleLength => {
            let a = (-local_angle[0]).atan2(local_angle[1]) / std::f32::consts::PI;
            [a, rel_length]
        }
        PuppetMapMode::LengthAngle => {
            let a = (-local_angle[0]).atan2(local_angle[1]) / std::f32::consts::PI;
            [rel_length, a]
        }
    };
    [
        result[0] * phys.output_scale[0],
        result[1] * phys.output_scale[1],
    ]
}

/* ------------------------------------------------------------------ */
/* Rasterizer                                                          */
/* ------------------------------------------------------------------ */

/// An f32 RGBA frame; writes clamp and quantize to 1/255
/// steps, mirroring an RGBA8 render target.
struct FrameBuf {
    width: usize,
    height: usize,
    px: Vec<[f32; 4]>,
}

impl FrameBuf {
    fn new(width: usize, height: usize) -> Self {
        FrameBuf {
            width,
            height,
            px: vec![[0.0; 4]; width * height],
        }
    }

    fn blend(&mut self, x: usize, y: usize, src: [f32; 4], mode: PuppetBlendMode) {
        let idx = y * self.width + x;
        let dst = self.px[idx];
        let out = blend_pixel(src, dst, mode);
        self.px[idx] = [
            quantize(out[0]),
            quantize(out[1]),
            quantize(out[2]),
            quantize(out[3]),
        ];
    }

    fn to_rgba8(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.px.len() * 4);
        for p in &self.px {
            for c in p {
                out.push((c.clamp(0.0, 1.0) * 255.0).round() as u8);
            }
        }
        out
    }
}

fn quantize(v: f32) -> f32 {
    (v.clamp(0.0, 1.0) * 255.0).round() / 255.0
}

/// The reference fragment shading: screen-tint math, tint
/// multiply, opacity application (SPEC 13.6).
fn shade(texel: [f32; 4], drawable: &crate::puppet::PuppetDrawable, opacity: f32) -> [f32; 4] {
    let mut rgb = [0.0f32; 3];
    for c in 0..3 {
        let screen_out = 1.0 - (1.0 - texel[c]) * (1.0 - drawable.screen_tint[c] * texel[3]);
        rgb[c] = screen_out * drawable.tint[c] * opacity;
    }
    [rgb[0], rgb[1], rgb[2], texel[3] * opacity]
}

/// The seven blend equations of the stage's blend family,
/// as fixed-function factors over `(src, dst)` (SPEC 13.6).
fn blend_pixel(src: [f32; 4], dst: [f32; 4], mode: PuppetBlendMode) -> [f32; 4] {
    let mut out = [0.0f32; 4];
    match mode {
        PuppetBlendMode::Normal => {
            for c in 0..4 {
                out[c] = src[c] + dst[c] * (1.0 - src[3]);
            }
        }
        PuppetBlendMode::Multiply => {
            for c in 0..4 {
                out[c] = src[c] * dst[c] + dst[c] * (1.0 - src[3]);
            }
        }
        PuppetBlendMode::ColorDodge => {
            for c in 0..4 {
                out[c] = src[c] * dst[c] + dst[c];
            }
        }
        PuppetBlendMode::LinearDodge => {
            for c in 0..4 {
                out[c] = src[c] + dst[c];
            }
        }
        PuppetBlendMode::Screen => {
            for c in 0..4 {
                out[c] = src[c] + dst[c] * (1.0 - src[c]);
            }
        }
        PuppetBlendMode::ClipToLower => {
            for c in 0..4 {
                out[c] = src[c] * dst[3] + dst[c] * (1.0 - src[3]);
            }
        }
        PuppetBlendMode::SliceFromLower => {
            for c in 0..4 {
                out[c] = src[c] * (1.0 - dst[3]) - dst[c] * (1.0 - src[3]);
            }
        }
    }
    out
}

/// Bilinear sample with clamp-to-border (transparent black
/// beyond the edges) — the reference texture state.
fn sample_bilinear(tex: &RgbaImage, u: f32, v: f32) -> [f32; 4] {
    let x = u * tex.width as f32 - 0.5;
    let y = v * tex.height as f32 - 0.5;
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;
    let fetch = |ix: i32, iy: i32| -> [f32; 4] {
        if ix < 0 || iy < 0 || ix >= tex.width as i32 || iy >= tex.height as i32 {
            return [0.0; 4];
        }
        let i = ((iy as usize) * tex.width + ix as usize) * 4;
        [
            tex.rgba[i] as f32 / 255.0,
            tex.rgba[i + 1] as f32 / 255.0,
            tex.rgba[i + 2] as f32 / 255.0,
            tex.rgba[i + 3] as f32 / 255.0,
        ]
    };
    let c00 = fetch(x0, y0);
    let c10 = fetch(x0 + 1, y0);
    let c01 = fetch(x0, y0 + 1);
    let c11 = fetch(x0 + 1, y0 + 1);
    let mut out = [0.0f32; 4];
    for c in 0..4 {
        out[c] = lerp(lerp(c00[c], c10[c], fx), lerp(c01[c], c11[c], fx), fy);
    }
    out
}

/// Triangle rasterization at pixel centers with barycentric
/// UV interpolation (affine == perspective-correct under the
/// stage's orthographic camera). `emit(x, y, uv)` per pixel.
fn rasterize(
    width: usize,
    height: usize,
    scale: f32,
    verts: &[[f32; 2]],
    uvs: &[[f32; 2]],
    indices: &[u32],
    emit: &mut dyn FnMut(usize, usize, [f32; 2]),
) {
    let to_screen = |p: [f32; 2]| -> [f32; 2] {
        [
            p[0] * scale + width as f32 / 2.0,
            p[1] * scale + height as f32 / 2.0,
        ]
    };
    for tri in indices.as_chunks::<3>().0 {
        let (i0, i1, i2) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
        let a = to_screen(verts[i0]);
        let b = to_screen(verts[i1]);
        let c = to_screen(verts[i2]);
        let denom = (b[1] - c[1]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[1] - c[1]);
        if denom == 0.0 || !denom.is_finite() {
            continue;
        }
        let min_x = a[0].min(b[0]).min(c[0]).floor().max(0.0) as usize;
        let max_x = (a[0].max(b[0]).max(c[0]).ceil() as i32).min(width as i32 - 1);
        let min_y = a[1].min(b[1]).min(c[1]).floor().max(0.0) as usize;
        let max_y = (a[1].max(b[1]).max(c[1]).ceil() as i32).min(height as i32 - 1);
        if max_x < 0 || max_y < 0 {
            continue;
        }
        for y in min_y..=(max_y as usize) {
            for x in min_x..=(max_x as usize) {
                let px = x as f32 + 0.5;
                let py = y as f32 + 0.5;
                let w0 = ((b[1] - c[1]) * (px - c[0]) + (c[0] - b[0]) * (py - c[1])) / denom;
                let w1 = ((c[1] - a[1]) * (px - c[0]) + (a[0] - c[0]) * (py - c[1])) / denom;
                let w2 = 1.0 - w0 - w1;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let uv = [
                    w0 * uvs[i0][0] + w1 * uvs[i1][0] + w2 * uvs[i2][0],
                    w0 * uvs[i0][1] + w1 * uvs[i1][1] + w2 * uvs[i2][1],
                ];
                emit(x, y, uv);
            }
        }
    }
}
