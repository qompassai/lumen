//! Canonical skeleton and procedural base mesh.
//!
//! Contract summary:
//! - Skeleton: Mixamo-compatible joint names, Y-up, facing +Z, meters.
//!   Bind positions are world-space positions for a 1.70 m mature female
//!   base. Joint local translations are derived as world minus parent
//!   world by the GLB writer.
//! - Mesh: deterministic procedural generation only (no external assets).
//!   Parts are separate primitives under one skin: `arm_left`, `arm_right`,
//!   `body`, `hair`, `head`, `leg_left`, `leg_right`, `outfit_legs`,
//!   `outfit_torso` (stored in construction order, listed alphabetically
//!   here).
//! - Skinning: at most four influences per vertex, weights from
//!   distance-to-bone-segment falloff, normalized to sum to 1.
//! - Every loop is bounded by the table/profile sizes validated up front;
//!   non-finite or non-positive profile values are rejected fail-closed.
//!
//! Texture atlas layout (single 0..1 UV space, documented zones):
//! - head / face:  `(0.00, 0.00) - (0.25, 0.25)` (reserved face region)
//! - limbs (skin): `(0.00, 0.25) - (0.25, 0.50)`
//! - outfit legs:  `(0.00, 0.50) - (0.25, 0.75)`
//! - body:         `(0.25, 0.00) - (0.50, 0.50)`
//! - hair:         `(0.50, 0.00) - (0.75, 0.25)`
//! - outfit torso: `(0.50, 0.25) - (0.75, 0.50)`

#![forbid(unsafe_code)]

use crate::{FactoryError, MAX_JOINTS, MAX_PART_VERTICES, MAX_SKIN_INFLUENCES};

/// One skeleton joint: name, parent name, world-space bind position.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Joint {
    /// Mixamo-compatible joint name (e.g. `LeftHandThumb1`).
    pub name: &'static str,
    /// Parent joint name, or `None` for the root (`Hips`).
    pub parent: Option<&'static str>,
    /// World-space bind position in meters (Y-up, facing +Z).
    pub position: [f32; 3],
}

/// One skinned mesh vertex.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Vertex {
    /// Joint indices (into the joint table), zero-padded.
    pub joints: [u16; MAX_SKIN_INFLUENCES],
    /// Joint normal (unit length for lit vertices).
    pub normal: [f32; 3],
    /// World-space position in meters.
    pub position: [f32; 3],
    /// Texture coordinates inside the documented atlas zones.
    pub uv: [f32; 2],
    /// Skin weights aligned with `joints`, summing to 1.
    pub weights: [f32; MAX_SKIN_INFLUENCES],
}

/// One named mesh part, emitted as one glTF primitive.
#[derive(Clone, Debug, PartialEq)]
pub struct MeshPart {
    /// Triangle indices into `vertices`.
    pub indices: Vec<u32>,
    /// Part name (also its material name).
    pub name: String,
    /// Vertices in deterministic construction order.
    pub vertices: Vec<Vertex>,
}

/// Per-head-vertex blend-region weights for Phase 1b blend fields.
///
/// All values lie in `0.0..=1.0` and are derived from head-local
/// normalized coordinates (front of face is +Z).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HeadRegionWeights {
    /// Brow region weight.
    pub brow: f32,
    /// Cheek region weight.
    pub cheek: f32,
    /// Eye-corner region weight.
    pub eye_corner: f32,
    /// Jaw region weight.
    pub jaw: f32,
    /// Lips region weight.
    pub lips: f32,
    /// Lower eyelid region weight.
    pub lower_lid: f32,
    /// Mouth region weight.
    pub mouth: f32,
    /// Upper eyelid region weight.
    pub upper_lid: f32,
}

/// The generated base: skeleton, mesh parts, and head region masks.
///
/// `head_masks` is aligned 1:1 with the vertices of the part named `head`.
#[derive(Clone, Debug, PartialEq)]
pub struct BaseMesh {
    /// Per-head-vertex region masks (aligned with the `head` part).
    pub head_masks: Vec<HeadRegionWeights>,
    /// Canonical joint table (same order as [`joint_table`]).
    pub joints: Vec<Joint>,
    /// Mesh parts in deterministic construction order.
    pub parts: Vec<MeshPart>,
}

const fn j(name: &'static str, parent: Option<&'static str>, position: [f32; 3]) -> Joint {
    Joint {
        name,
        parent,
        position,
    }
}

/// Canonical joint table in hierarchy-friendly (parent-before-child)
/// construction order. Left side has negative X.
pub fn joint_table() -> Vec<Joint> {
    let mut out: Vec<Joint> = Vec::with_capacity(65);
    out.push(j("Hips", None, [0.0, 1.02, 0.0]));
    out.push(j("Spine", Some("Hips"), [0.0, 1.14, 0.0]));
    out.push(j("Spine1", Some("Spine"), [0.0, 1.26, 0.0]));
    out.push(j("Spine2", Some("Spine1"), [0.0, 1.38, 0.0]));
    out.push(j("Neck", Some("Spine2"), [0.0, 1.50, 0.0]));
    out.push(j("Head", Some("Neck"), [0.0, 1.58, 0.0]));
    out.push(j("HeadTop_End", Some("Head"), [0.0, 1.78, 0.0]));
    for side in [Side::Left, Side::Right] {
        build_side(&mut out, side);
    }
    out
}

#[derive(Clone, Copy)]
enum Side {
    Left,
    Right,
}

impl Side {
    fn sign(self) -> f32 {
        match self {
            Side::Left => -1.0,
            Side::Right => 1.0,
        }
    }
}

// Leak-free static naming: finger/other names are built from a fixed
// table of literals per side so `&'static str` names stay exact.
fn build_side(out: &mut Vec<Joint>, side: Side) {
    let s = side.sign();
    let x = |v: f32| s * v;
    match side {
        Side::Left => {
            out.push(j("LeftShoulder", Some("Spine2"), [x(0.07), 1.465, 0.0]));
            out.push(j("LeftArm", Some("LeftShoulder"), [x(0.21), 1.45, 0.0]));
            out.push(j("LeftForeArm", Some("LeftArm"), [x(0.47), 1.44, 0.01]));
            out.push(j("LeftHand", Some("LeftForeArm"), [x(0.71), 1.435, 0.02]));
            push_fingers_left(out);
            out.push(j("LeftUpLeg", Some("Hips"), [x(0.10), 0.98, 0.0]));
            out.push(j("LeftLeg", Some("LeftUpLeg"), [x(0.105), 0.52, 0.005]));
            out.push(j("LeftFoot", Some("LeftLeg"), [x(0.11), 0.09, 0.01]));
            out.push(j("LeftToeBase", Some("LeftFoot"), [x(0.11), 0.045, 0.13]));
            out.push(j("LeftToe_End", Some("LeftToeBase"), [x(0.11), 0.03, 0.23]));
        }
        Side::Right => {
            out.push(j("RightShoulder", Some("Spine2"), [x(0.07), 1.465, 0.0]));
            out.push(j("RightArm", Some("RightShoulder"), [x(0.21), 1.45, 0.0]));
            out.push(j("RightForeArm", Some("RightArm"), [x(0.47), 1.44, 0.01]));
            out.push(j("RightHand", Some("RightForeArm"), [x(0.71), 1.435, 0.02]));
            push_fingers_right(out);
            out.push(j("RightUpLeg", Some("Hips"), [x(0.10), 0.98, 0.0]));
            out.push(j("RightLeg", Some("RightUpLeg"), [x(0.105), 0.52, 0.005]));
            out.push(j("RightFoot", Some("RightLeg"), [x(0.11), 0.09, 0.01]));
            out.push(j("RightToeBase", Some("RightFoot"), [x(0.11), 0.045, 0.13]));
            out.push(j(
                "RightToe_End",
                Some("RightToeBase"),
                [x(0.11), 0.03, 0.23],
            ));
        }
    }
}

fn push_fingers_left(out: &mut Vec<Joint>) {
    let s = -1.0f32;
    let x = |v: f32| s * v;
    let hand = "LeftHand";
    out.push(j("LeftHandThumb1", Some(hand), [x(0.74), 1.42, 0.05]));
    out.push(j(
        "LeftHandThumb2",
        Some("LeftHandThumb1"),
        [x(0.78), 1.415, 0.075],
    ));
    out.push(j(
        "LeftHandThumb3",
        Some("LeftHandThumb2"),
        [x(0.815), 1.41, 0.09],
    ));
    out.push(j(
        "LeftHandThumb_End",
        Some("LeftHandThumb3"),
        [x(0.845), 1.405, 0.10],
    ));
    out.push(j("LeftHandIndex1", Some(hand), [x(0.80), 1.44, 0.045]));
    out.push(j(
        "LeftHandIndex2",
        Some("LeftHandIndex1"),
        [x(0.85), 1.44, 0.048],
    ));
    out.push(j(
        "LeftHandIndex3",
        Some("LeftHandIndex2"),
        [x(0.885), 1.44, 0.05],
    ));
    out.push(j(
        "LeftHandIndex_End",
        Some("LeftHandIndex3"),
        [x(0.915), 1.44, 0.05],
    ));
    out.push(j("LeftHandMiddle1", Some(hand), [x(0.80), 1.44, 0.015]));
    out.push(j(
        "LeftHandMiddle2",
        Some("LeftHandMiddle1"),
        [x(0.855), 1.44, 0.015],
    ));
    out.push(j(
        "LeftHandMiddle3",
        Some("LeftHandMiddle2"),
        [x(0.895), 1.44, 0.015],
    ));
    out.push(j(
        "LeftHandMiddle_End",
        Some("LeftHandMiddle3"),
        [x(0.93), 1.44, 0.015],
    ));
    out.push(j("LeftHandRing1", Some(hand), [x(0.80), 1.44, -0.015]));
    out.push(j(
        "LeftHandRing2",
        Some("LeftHandRing1"),
        [x(0.85), 1.44, -0.015],
    ));
    out.push(j(
        "LeftHandRing3",
        Some("LeftHandRing2"),
        [x(0.885), 1.44, -0.015],
    ));
    out.push(j(
        "LeftHandRing_End",
        Some("LeftHandRing3"),
        [x(0.915), 1.44, -0.015],
    ));
    out.push(j("LeftHandPinky1", Some(hand), [x(0.795), 1.435, -0.042]));
    out.push(j(
        "LeftHandPinky2",
        Some("LeftHandPinky1"),
        [x(0.835), 1.435, -0.045],
    ));
    out.push(j(
        "LeftHandPinky3",
        Some("LeftHandPinky2"),
        [x(0.865), 1.435, -0.047],
    ));
    out.push(j(
        "LeftHandPinky_End",
        Some("LeftHandPinky3"),
        [x(0.89), 1.435, -0.048],
    ));
}

fn push_fingers_right(out: &mut Vec<Joint>) {
    let s = 1.0f32;
    let x = |v: f32| s * v;
    let hand = "RightHand";
    out.push(j("RightHandThumb1", Some(hand), [x(0.74), 1.42, 0.05]));
    out.push(j(
        "RightHandThumb2",
        Some("RightHandThumb1"),
        [x(0.78), 1.415, 0.075],
    ));
    out.push(j(
        "RightHandThumb3",
        Some("RightHandThumb2"),
        [x(0.815), 1.41, 0.09],
    ));
    out.push(j(
        "RightHandThumb_End",
        Some("RightHandThumb3"),
        [x(0.845), 1.405, 0.10],
    ));
    out.push(j("RightHandIndex1", Some(hand), [x(0.80), 1.44, 0.045]));
    out.push(j(
        "RightHandIndex2",
        Some("RightHandIndex1"),
        [x(0.85), 1.44, 0.048],
    ));
    out.push(j(
        "RightHandIndex3",
        Some("RightHandIndex2"),
        [x(0.885), 1.44, 0.05],
    ));
    out.push(j(
        "RightHandIndex_End",
        Some("RightHandIndex3"),
        [x(0.915), 1.44, 0.05],
    ));
    out.push(j("RightHandMiddle1", Some(hand), [x(0.80), 1.44, 0.015]));
    out.push(j(
        "RightHandMiddle2",
        Some("RightHandMiddle1"),
        [x(0.855), 1.44, 0.015],
    ));
    out.push(j(
        "RightHandMiddle3",
        Some("RightHandMiddle2"),
        [x(0.895), 1.44, 0.015],
    ));
    out.push(j(
        "RightHandMiddle_End",
        Some("RightHandMiddle3"),
        [x(0.93), 1.44, 0.015],
    ));
    out.push(j("RightHandRing1", Some(hand), [x(0.80), 1.44, -0.015]));
    out.push(j(
        "RightHandRing2",
        Some("RightHandRing1"),
        [x(0.85), 1.44, -0.015],
    ));
    out.push(j(
        "RightHandRing3",
        Some("RightHandRing2"),
        [x(0.885), 1.44, -0.015],
    ));
    out.push(j(
        "RightHandRing_End",
        Some("RightHandRing3"),
        [x(0.915), 1.44, -0.015],
    ));
    out.push(j("RightHandPinky1", Some(hand), [x(0.795), 1.435, -0.042]));
    out.push(j(
        "RightHandPinky2",
        Some("RightHandPinky1"),
        [x(0.835), 1.435, -0.045],
    ));
    out.push(j(
        "RightHandPinky3",
        Some("RightHandPinky2"),
        [x(0.865), 1.435, -0.047],
    ));
    out.push(j(
        "RightHandPinky_End",
        Some("RightHandPinky3"),
        [x(0.89), 1.435, -0.048],
    ));
}

/// Index of a joint by name, if present.
pub fn joint_index(joints: &[Joint], name: &str) -> Option<usize> {
    joints.iter().position(|joint| joint.name == name)
}

/// Validate the joint table: parents exist, no cycles, finite positions,
/// count within [`MAX_JOINTS`].
pub fn validate_joints(joints: &[Joint]) -> Result<(), FactoryError> {
    if joints.is_empty() || joints.len() > MAX_JOINTS {
        return Err(FactoryError::BadParam(format!(
            "joint count {} out of bounds",
            joints.len()
        )));
    }
    for joint in joints {
        if !joint.position.iter().all(|v| v.is_finite()) {
            return Err(FactoryError::BadParam(format!(
                "joint {} has non-finite position",
                joint.name
            )));
        }
        if let Some(parent) = joint.parent
            && joint_index(joints, parent).is_none()
        {
            return Err(FactoryError::BadParam(format!(
                "joint {} parent {parent} missing",
                joint.name
            )));
        }
    }
    // Acyclicity: walk each chain with a bounded step count.
    for joint in joints {
        let mut current = Some(joint);
        for _ in 0..=joints.len() {
            match current {
                None => break,
                Some(node) => {
                    current = node
                        .parent
                        .and_then(|p| joint_index(joints, p))
                        .map(|idx| &joints[idx]);
                    if current.is_none() {
                        break;
                    }
                }
            }
        }
        if current.is_some() {
            return Err(FactoryError::BadParam(format!(
                "joint hierarchy cycle at {}",
                joint.name
            )));
        }
    }
    Ok(())
}

/// Default torso profile: `(y, radius_x, radius_z)` rings, ascending y.
pub fn default_body_profile() -> Vec<(f32, f32, f32)> {
    vec![
        (0.92, 0.155, 0.108),
        (0.98, 0.165, 0.115),
        (1.14, 0.125, 0.090),
        (1.31, 0.160, 0.120),
        (1.42, 0.140, 0.100),
        (1.50, 0.055, 0.050),
    ]
}

fn validate_profile(profile: &[(f32, f32, f32)]) -> Result<(), FactoryError> {
    if profile.len() < 2 || profile.len() > 64 {
        return Err(FactoryError::BadParam(format!(
            "profile ring count {} out of bounds",
            profile.len()
        )));
    }
    for &(y, rx, rz) in profile {
        if !y.is_finite() || !rx.is_finite() || !rz.is_finite() {
            return Err(FactoryError::BadParam(
                "profile contains non-finite value".to_string(),
            ));
        }
        if rx <= 0.0 || rz <= 0.0 {
            return Err(FactoryError::BadParam(
                "profile radii must be positive".to_string(),
            ));
        }
    }
    for pair in profile.windows(2) {
        if pair[1].0 <= pair[0].0 {
            return Err(FactoryError::BadParam(
                "profile y values must be strictly ascending".to_string(),
            ));
        }
    }
    Ok(())
}

// --- small vector helpers (deterministic, allocation-free) ---

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
fn length(a: [f32; 3]) -> f32 {
    dot(a, a).sqrt()
}
fn normalize_or(a: [f32; 3], fallback: [f32; 3]) -> [f32; 3] {
    let len = length(a);
    if len > 1e-9 && len.is_finite() {
        [a[0] / len, a[1] / len, a[2] / len]
    } else {
        fallback
    }
}
fn dist_point_segment(p: [f32; 3], a: [f32; 3], b: [f32; 3]) -> f32 {
    let ab = sub(b, a);
    let denom = dot(ab, ab);
    let t = if denom > 1e-12 {
        (dot(sub(p, a), ab) / denom).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let closest = [a[0] + ab[0] * t, a[1] + ab[1] * t, a[2] + ab[2] * t];
    length(sub(p, closest))
}

/// Skin weights for one vertex: inverse-square falloff over bone
/// segments (parent -> joint), top four influences, normalized.
pub fn compute_skin_weights(
    position: [f32; 3],
    joints: &[Joint],
) -> ([u16; MAX_SKIN_INFLUENCES], [f32; MAX_SKIN_INFLUENCES]) {
    // Score per joint: best (smallest) distance to any segment ending at
    // that joint; the root uses distance to its point.
    let mut scored: Vec<(usize, f32)> = Vec::with_capacity(joints.len());
    for (idx, joint) in joints.iter().enumerate() {
        let d = match joint.parent.and_then(|p| joint_index(joints, p)) {
            Some(parent_idx) => {
                dist_point_segment(position, joints[parent_idx].position, joint.position)
            }
            None => length(sub(position, joint.position)),
        };
        let w = 1.0 / (d * d + 0.0004);
        scored.push((idx, w));
    }
    // Deterministic selection: sort by weight desc, then index asc.
    scored.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut indices = [0u16; MAX_SKIN_INFLUENCES];
    let mut weights = [0.0f32; MAX_SKIN_INFLUENCES];
    let mut sum = 0.0f32;
    for slot in 0..MAX_SKIN_INFLUENCES {
        if let Some(&(joint_idx, w)) = scored.get(slot) {
            indices[slot] = joint_idx as u16;
            weights[slot] = w;
            sum += w;
        }
    }
    if sum > 0.0 && sum.is_finite() {
        for w in &mut weights {
            *w /= sum;
        }
    } else {
        weights = [1.0, 0.0, 0.0, 0.0];
    }
    (indices, weights)
}

fn make_vertex(position: [f32; 3], uv: [f32; 2], joints: &[Joint]) -> Vertex {
    let (jidx, weights) = compute_skin_weights(position, joints);
    Vertex {
        joints: jidx,
        normal: [0.0, 1.0, 0.0],
        position,
        uv,
        weights,
    }
}

fn compute_normals(part: &mut MeshPart) {
    let mut normals = vec![[0.0f32; 3]; part.vertices.len()];
    for tri in part.indices.as_chunks::<3>().0 {
        let (i0, i1, i2) = (tri[0] as usize, tri[1] as usize, tri[2] as usize);
        if i0 >= part.vertices.len() || i1 >= part.vertices.len() || i2 >= part.vertices.len() {
            continue;
        }
        let p0 = part.vertices[i0].position;
        let p1 = part.vertices[i1].position;
        let p2 = part.vertices[i2].position;
        let n = cross(sub(p1, p0), sub(p2, p0));
        for idx in [i0, i1, i2] {
            normals[idx][0] += n[0];
            normals[idx][1] += n[1];
            normals[idx][2] += n[2];
        }
    }
    for (vertex, n) in part.vertices.iter_mut().zip(normals.iter()) {
        vertex.normal = normalize_or(*n, [0.0, 1.0, 0.0]);
    }
}

fn finish_part(mut part: MeshPart) -> Result<MeshPart, FactoryError> {
    if part.vertices.is_empty() || part.indices.is_empty() {
        return Err(FactoryError::MeshInvalid(format!(
            "part {} is empty",
            part.name
        )));
    }
    if part.vertices.len() > MAX_PART_VERTICES {
        return Err(FactoryError::MeshInvalid(format!(
            "part {} exceeds vertex bound",
            part.name
        )));
    }
    compute_normals(&mut part);
    Ok(part)
}

/// Interpolate the profile at height `y` (clamped to the profile range).
fn profile_at(profile: &[(f32, f32, f32)], y: f32) -> (f32, f32) {
    if y <= profile[0].0 {
        return (profile[0].1, profile[0].2);
    }
    for pair in profile.windows(2) {
        let (y0, rx0, rz0) = pair[0];
        let (y1, rx1, rz1) = pair[1];
        if y <= y1 {
            let t = (y - y0) / (y1 - y0);
            return (rx0 + (rx1 - rx0) * t, rz0 + (rz1 - rz0) * t);
        }
    }
    let last = profile[profile.len() - 1];
    (last.1, last.2)
}

/// Lathe (elliptical rings around the Y axis) between `y0..=y1`.
#[allow(clippy::too_many_arguments)]
fn build_lathe(
    name: &str,
    profile: &[(f32, f32, f32)],
    y0: f32,
    y1: f32,
    ring_steps: usize,
    radial: usize,
    scale: f32,
    uv_zone: (f32, f32, f32, f32),
    joints: &[Joint],
) -> MeshPart {
    let mut vertices: Vec<Vertex> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let (u0, v0, u1, v1) = uv_zone;
    for step in 0..=ring_steps {
        let t = step as f32 / ring_steps as f32;
        let y = y0 + (y1 - y0) * t;
        let (rx, rz) = profile_at(profile, y);
        let (rx, rz) = (rx * scale, rz * scale);
        for seg in 0..radial {
            let angle = (seg as f32 / radial as f32) * std::f32::consts::TAU;
            let pos = [rx * angle.cos(), y, rz * angle.sin()];
            let u = u0 + (seg as f32 / radial as f32) * (u1 - u0);
            let v = v0 + t * (v1 - v0);
            vertices.push(make_vertex(pos, [u, v], joints));
        }
    }
    for step in 0..ring_steps {
        for seg in 0..radial {
            let next = (seg + 1) % radial;
            let a = (step * radial + seg) as u32;
            let b = (step * radial + next) as u32;
            let c = ((step + 1) * radial + seg) as u32;
            let d = ((step + 1) * radial + next) as u32;
            indices.extend_from_slice(&[a, c, b, b, c, d]);
        }
    }
    // Bottom cap fan and top cap fan.
    let bottom_center = vertices.len() as u32;
    vertices.push(make_vertex([0.0, y0, 0.0], [(u0 + u1) * 0.5, v0], joints));
    for seg in 0..radial {
        let next = (seg + 1) % radial;
        indices.extend_from_slice(&[bottom_center, next as u32, seg as u32]);
    }
    let top_offset = ring_steps * radial;
    let top_center = vertices.len() as u32;
    vertices.push(make_vertex([0.0, y1, 0.0], [(u0 + u1) * 0.5, v1], joints));
    for seg in 0..radial {
        let next = (seg + 1) % radial;
        indices.extend_from_slice(&[
            top_center,
            (top_offset + seg) as u32,
            (top_offset + next) as u32,
        ]);
    }
    MeshPart {
        indices,
        name: name.to_string(),
        vertices,
    }
}

/// Default head ellipsoid center (meters, world space).
pub const HEAD_CENTER: [f32; 3] = [0.0, 1.665, 0.005];
/// Default head ellipsoid radii (meters).
pub const HEAD_RADII: [f32; 3] = [0.086, 0.106, 0.096];
/// Default jaw-taper threshold height (meters).
pub const HEAD_JAW_Y: f32 = 1.62;

/// Hair-length parameter's neutral point (Phase 3): at this length
/// the hair shell is exactly the Phase 1/2 shell, so the Séraphine
/// regression anchor reproduces byte-for-byte.
pub const HAIR_LENGTH_DEFAULT: f32 = 0.65;
/// Canonical hair back-panel bottom Y at length 1.0 (long).
pub const HAIR_BOTTOM_LONG_Y: f32 = 1.28;
/// Canonical hair back-panel bottom Y at length 0.0 (short).
pub const HAIR_BOTTOM_SHORT_Y: f32 = 1.50;

/// Canonical hair back-panel bottom Y for a hair `length` in
/// `0.0..=1.0`: a plain lerp between [`HAIR_BOTTOM_SHORT_Y`] and
/// [`HAIR_BOTTOM_LONG_Y`]. The shell implements this as a shift
/// relative to the neutral point [`HAIR_LENGTH_DEFAULT`] (see
/// `build_head`): the bottom vertex moves by
/// `(hair_bottom_y(HAIR_LENGTH_DEFAULT) - hair_bottom_y(length))`
/// scaled by the head's Y scale, so the neutral length changes no
/// vertex at all.
pub fn hair_bottom_y(length: f32) -> f32 {
    HAIR_BOTTOM_SHORT_Y + (HAIR_BOTTOM_LONG_Y - HAIR_BOTTOM_SHORT_Y) * length
}

/// Ellipsoid head (also the basis for the hair shell when `hair` is set).
fn build_head(
    name: &str,
    hair: bool,
    joints: &[Joint],
    center: [f32; 3],
    radii: [f32; 3],
    jaw_y: f32,
    hair_length: f32,
) -> (MeshPart, Vec<HeadRegionWeights>) {
    let lat_steps = 14usize;
    let lon_steps = 18usize;
    let scale = if hair { 1.07 } else { 1.0 };
    let mut vertices: Vec<Vertex> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let mut grid: Vec<Vec<Option<u32>>> = Vec::new();
    for lat in 0..=lat_steps {
        let phi = (lat as f32 / lat_steps as f32) * std::f32::consts::PI;
        let mut row: Vec<Option<u32>> = Vec::new();
        for lon in 0..lon_steps {
            let theta = (lon as f32 / lon_steps as f32) * std::f32::consts::TAU;
            let dir = [phi.sin() * theta.cos(), phi.cos(), phi.sin() * theta.sin()];
            let mut pos = [
                center[0] + dir[0] * radii[0] * scale,
                center[1] + dir[1] * radii[1] * scale,
                center[2] + dir[2] * radii[2] * scale,
            ];
            if hair {
                pos[2] -= 0.008;
                pos[1] += 0.005;
                // Extend the back/side coverage down toward y ~= 1.32.
                let backness = (-dir[2]).clamp(0.0, 1.0);
                if dir[1] < 0.0 {
                    pos[1] -= 0.24 * backness * (-dir[1]);
                }
                // Hair length (Phase 3): shift the back panel toward
                // the length's canonical bottom Y (see hair_bottom_y),
                // weighted by backness * downwardness so the panel's
                // lowest vertex moves by the full shift and the face
                // side never moves. The shift is exactly 0.0 at the
                // neutral length, leaving Phase 1/2 shells untouched.
                let head_y_scale = radii[1] / HEAD_RADII[1];
                let shift = (hair_bottom_y(HAIR_LENGTH_DEFAULT) - hair_bottom_y(hair_length))
                    * head_y_scale;
                if shift != 0.0 {
                    let weight = (backness * (-dir[1]).max(0.0) * 2.0).clamp(0.0, 1.0);
                    pos[1] -= shift * weight;
                }
                // Hair covers back and sides; skip the face opening.
                let face_opening = dir[2] > 0.35 && dir[1] > -0.55;
                if face_opening {
                    row.push(None);
                    continue;
                }
            } else {
                // Flatten the face toward +Z.
                if dir[2] > 0.3 {
                    pos[2] = center[2] + (pos[2] - center[2]) * 0.92;
                }
                // Jaw taper below the jaw threshold height; the taper
                // span scales with the head so a scaled head stays a
                // similar shape (exactly 0.12 m at the default radii).
                if pos[1] < jaw_y {
                    let taper_span = 0.12 * (radii[1] / HEAD_RADII[1]);
                    let t = ((jaw_y - pos[1]) / taper_span).clamp(0.0, 1.0);
                    pos[0] *= 1.0 - 0.22 * t;
                }
            }
            let (zone_u, zone_v) = if hair {
                (
                    0.50 + (lon as f32 / lon_steps as f32) * 0.25,
                    (lat as f32 / lat_steps as f32) * 0.25,
                )
            } else {
                (
                    (lon as f32 / lon_steps as f32) * 0.25,
                    (lat as f32 / lat_steps as f32) * 0.25,
                )
            };
            let idx = vertices.len() as u32;
            vertices.push(make_vertex(pos, [zone_u, zone_v], joints));
            row.push(Some(idx));
        }
        grid.push(row);
    }
    for lat in 0..lat_steps {
        for lon in 0..lon_steps {
            let next = (lon + 1) % lon_steps;
            let quad = [
                grid[lat][lon],
                grid[lat][next],
                grid[lat + 1][lon],
                grid[lat + 1][next],
            ];
            if let [Some(a), Some(b), Some(c), Some(d)] = quad {
                indices.extend_from_slice(&[a, c, b, b, c, d]);
            }
        }
    }
    let masks = if hair {
        Vec::new()
    } else {
        vertices
            .iter()
            .map(|v| head_region_weights(v.position, center, radii))
            .collect()
    };
    (
        MeshPart {
            indices,
            name: name.to_string(),
            vertices,
        },
        masks,
    )
}

fn gauss(d: f32, sigma: f32) -> f32 {
    (-(d * d) / (2.0 * sigma * sigma)).exp().clamp(0.0, 1.0)
}

fn head_region_weights(pos: [f32; 3], center: [f32; 3], radii: [f32; 3]) -> HeadRegionWeights {
    let nx = (pos[0] - center[0]) / radii[0];
    let ny = (pos[1] - center[1]) / radii[1];
    let nz = ((pos[2] - center[2]) / radii[2]).clamp(-1.0, 1.0);
    let front = nz.clamp(0.0, 1.0);
    let eye_y = 0.12f32;
    let eye_x = 0.36f32;
    let dist_eye = |sign: f32| -> f32 {
        let dx = nx - sign * eye_x;
        let dy = ny - eye_y;
        (dx * dx + dy * dy).sqrt()
    };
    let eye_dist = dist_eye(1.0).min(dist_eye(-1.0));
    let mouth_dist = (nx * nx + (ny + 0.48) * (ny + 0.48)).sqrt();
    let lips_dist = (nx * nx * 0.6 + (ny + 0.44) * (ny + 0.44)).sqrt();
    HeadRegionWeights {
        brow: gauss((ny - 0.30).abs(), 0.10) * gauss(nx.abs(), 0.55) * front,
        cheek: gauss(((nx.abs() - 0.48).abs()).hypot(ny + 0.18), 0.28) * front,
        eye_corner: gauss(eye_dist - 0.16, 0.07) * front,
        jaw: gauss((ny + 0.72).abs(), 0.20) * (0.35 + 0.65 * front),
        lips: gauss(lips_dist, 0.16) * front,
        lower_lid: gauss(eye_dist, 0.13) * gauss((ny - (eye_y - 0.10)).abs(), 0.06) * front,
        mouth: gauss(mouth_dist, 0.22) * front,
        upper_lid: gauss(eye_dist, 0.13) * gauss((ny - (eye_y + 0.10)).abs(), 0.06) * front,
    }
}

/// Tapered tube along a polyline. `flatten_last` applies a 0.45 Z scale
/// to the final segment's rings (hands / toe boxes).
fn build_tube(
    name: &str,
    points: &[[f32; 3]],
    radii: &[f32],
    radial: usize,
    flatten_last: bool,
    uv_zone: (f32, f32, f32, f32),
    joints: &[Joint],
) -> MeshPart {
    let (u0, v0, u1, v1) = uv_zone;
    let mut vertices: Vec<Vertex> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let ring_count = points.len();
    for (ring, (&center, &radius)) in points.iter().zip(radii.iter()).enumerate() {
        // Frame from the local tangent.
        let tangent = if ring + 1 < ring_count {
            sub(points[ring + 1], center)
        } else {
            sub(center, points[ring - 1])
        };
        let tangent = normalize_or(tangent, [0.0, -1.0, 0.0]);
        let helper = if tangent[1].abs() > 0.9 {
            [1.0, 0.0, 0.0]
        } else {
            [0.0, 1.0, 0.0]
        };
        let n_axis = normalize_or(cross(helper, tangent), [1.0, 0.0, 0.0]);
        let b_axis = normalize_or(cross(tangent, n_axis), [0.0, 0.0, 1.0]);
        let z_scale = if flatten_last && ring + 1 >= ring_count.saturating_sub(1) {
            0.45
        } else {
            1.0
        };
        for seg in 0..radial {
            let angle = (seg as f32 / radial as f32) * std::f32::consts::TAU;
            let local = [
                n_axis[0] * angle.cos() + b_axis[0] * angle.sin(),
                n_axis[1] * angle.cos() + b_axis[1] * angle.sin(),
                n_axis[2] * angle.cos() + b_axis[2] * angle.sin(),
            ];
            let mut offset = [local[0] * radius, local[1] * radius, local[2] * radius];
            // Flatten in world Z for hands/feet (approximation of the
            // segment frame flatten; deterministic and bounded).
            offset[2] *= z_scale;
            let pos = [
                center[0] + offset[0],
                center[1] + offset[1],
                center[2] + offset[2],
            ];
            let u = u0 + (seg as f32 / radial as f32) * (u1 - u0);
            let v = v0 + (ring as f32 / (ring_count - 1).max(1) as f32) * (v1 - v0);
            vertices.push(make_vertex(pos, [u, v], joints));
        }
    }
    for ring in 0..ring_count - 1 {
        for seg in 0..radial {
            let next = (seg + 1) % radial;
            let a = (ring * radial + seg) as u32;
            let b = (ring * radial + next) as u32;
            let c = ((ring + 1) * radial + seg) as u32;
            let d = ((ring + 1) * radial + next) as u32;
            indices.extend_from_slice(&[a, c, b, b, c, d]);
        }
    }
    // End caps (fans around the first/last ring centers).
    let start_center = vertices.len() as u32;
    vertices.push(make_vertex(points[0], [u0, v0], joints));
    for seg in 0..radial {
        let next = (seg + 1) % radial;
        indices.extend_from_slice(&[start_center, next as u32, seg as u32]);
    }
    let end_center = vertices.len() as u32;
    let last_ring = (ring_count - 1) * radial;
    vertices.push(make_vertex(points[ring_count - 1], [u1, v1], joints));
    for seg in 0..radial {
        let next = (seg + 1) % radial;
        indices.extend_from_slice(&[
            end_center,
            (last_ring + seg) as u32,
            (last_ring + next) as u32,
        ]);
    }
    MeshPart {
        indices,
        name: name.to_string(),
        vertices,
    }
}

fn joint_pos(joints: &[Joint], name: &str) -> [f32; 3] {
    joint_index(joints, name)
        .map(|idx| joints[idx].position)
        .unwrap_or([0.0, 0.0, 0.0])
}

/// Build the base mesh with the default body profile.
pub fn build_base_mesh() -> Result<BaseMesh, FactoryError> {
    build_base_mesh_with_profile(&default_body_profile())
}

/// Parameters for a scaled/parameterized base build (Phase 2
/// projection). The default parameter set (canonical joints, default
/// profile, radial scale 1.0, default head ellipsoid) reproduces the
/// Phase 1a base byte-for-byte: every scale enters the math as a
/// multiplication by exactly 1.0 or as a literal-identical constant.
#[derive(Clone, Debug, PartialEq)]
pub struct ScaledBuild {
    /// Body lathe height span `(y0, y1)`.
    pub body_y: (f32, f32),
    /// Head ellipsoid center (world space).
    pub head_center: [f32; 3],
    /// Head ellipsoid radii.
    pub head_radii: [f32; 3],
    /// Hair length in `0.0..=1.0` (see [`hair_bottom_y`]); the
    /// neutral [`HAIR_LENGTH_DEFAULT`] reproduces the Phase 1/2
    /// hair shell exactly.
    pub hair_length: f32,
    /// Jaw-taper threshold height.
    pub jaw_y: f32,
    /// Joint table to build against (possibly Y-scaled).
    pub joints: Vec<Joint>,
    /// Outfit torso lathe height span `(y0, y1)`.
    pub outfit_y: (f32, f32),
    /// Torso profile (possibly Y-remapped).
    pub profile: Vec<(f32, f32, f32)>,
    /// Uniform radial (XZ) scale for tube/lathe radii.
    pub radial_scale: f32,
}

impl Default for ScaledBuild {
    fn default() -> Self {
        Self {
            body_y: (0.92, 1.50),
            head_center: HEAD_CENTER,
            head_radii: HEAD_RADII,
            hair_length: HAIR_LENGTH_DEFAULT,
            jaw_y: HEAD_JAW_Y,
            joints: joint_table(),
            outfit_y: (0.97, 1.47),
            profile: default_body_profile(),
            radial_scale: 1.0,
        }
    }
}

/// Base proportions from the canonical joint table, using the same
/// band definitions as the sprite measurement in `identity.rs`:
/// total height is the `HeadTop_End` height; the head band runs from
/// the neck joint to the top; the torso band from the hips joint to
/// the neck joint; the leg span from the ground to the hips joint.
/// Returns `(head_ratio, torso_ratio, leg_ratio)`; they sum to 1.
pub fn base_proportions() -> (f32, f32, f32) {
    let joints = joint_table();
    let y_of = |name: &str| {
        joints
            .iter()
            .find(|joint| joint.name == name)
            .map(|joint| joint.position[1])
            .unwrap_or(0.0)
    };
    let total = y_of("HeadTop_End");
    let neck = y_of("Neck");
    let hips = y_of("Hips");
    ((total - neck) / total, (neck - hips) / total, hips / total)
}

/// The canonical base's neck allowance below the chin, as a fraction
/// of its skull height: `(chin_y - neck_joint_y) / skull_height`, with
/// the chin at the bottom of the default head ellipsoid. The sprite
/// measurement in `identity.rs` locates the chin by color and uses
/// this ratio to place the band boundary that corresponds to the
/// base's `Neck` joint, so sprite and base head bands share one
/// definition.
pub fn base_neck_per_skull() -> f32 {
    let skull = 2.0 * HEAD_RADII[1];
    let chin_y = HEAD_CENTER[1] - HEAD_RADII[1];
    let neck_y = joint_table()
        .iter()
        .find(|joint| joint.name == "Neck")
        .map(|joint| joint.position[1])
        .unwrap_or(0.0);
    (chin_y - neck_y) / skull
}

/// The canonical joint table with Y positions remapped piecewise by
/// body band: leg-band heights (at or below the hips) scale by
/// `leg_scale`, torso-band heights by `torso_scale` on top of the
/// scaled hips, and head-band heights by `head_scale` on top of the
/// scaled neck. X/Z are unchanged. With all scales at 1.0 the result
/// equals [`joint_table`] exactly.
pub fn scaled_joint_table(head_scale: f32, torso_scale: f32, leg_scale: f32) -> Vec<Joint> {
    let joints = joint_table();
    let y_of = |name: &str| {
        joints
            .iter()
            .find(|joint| joint.name == name)
            .map(|joint| joint.position[1])
            .unwrap_or(0.0)
    };
    let hips_y = y_of("Hips");
    let neck_y = y_of("Neck");
    joints
        .into_iter()
        .map(|mut joint| {
            let y = joint.position[1];
            joint.position[1] = remap_band_y(y, hips_y, neck_y, head_scale, torso_scale, leg_scale);
            joint
        })
        .collect()
}

/// Piecewise band remap used by [`scaled_joint_table`] and by the
/// profile remap in `project.rs` (same anchors, same math).
pub fn remap_band_y(
    y: f32,
    hips_y: f32,
    neck_y: f32,
    head_scale: f32,
    torso_scale: f32,
    leg_scale: f32,
) -> f32 {
    let scaled_hips = hips_y * leg_scale;
    if y <= hips_y {
        y * leg_scale
    } else if y <= neck_y {
        scaled_hips + (y - hips_y) * torso_scale
    } else {
        scaled_hips + (neck_y - hips_y) * torso_scale + (y - neck_y) * head_scale
    }
}

/// Build the base mesh with a caller-supplied torso profile.
///
/// Fails closed on non-finite values, non-positive radii, or a
/// non-ascending / out-of-bounds profile.
pub fn build_base_mesh_with_profile(profile: &[(f32, f32, f32)]) -> Result<BaseMesh, FactoryError> {
    let params = ScaledBuild {
        profile: profile.to_vec(),
        ..ScaledBuild::default()
    };
    build_base_mesh_parametric(&params)
}

/// Build the base mesh from explicit scaled parameters.
///
/// Fails closed on the same profile/joint validation as
/// [`build_base_mesh_with_profile`], plus non-finite or non-positive
/// radial scale / head radii.
pub fn build_base_mesh_parametric(params: &ScaledBuild) -> Result<BaseMesh, FactoryError> {
    if !params.radial_scale.is_finite() || params.radial_scale <= 0.0 {
        return Err(FactoryError::BadParam(
            "radial scale must be finite and positive".to_string(),
        ));
    }
    if !params.head_center.iter().all(|v| v.is_finite())
        || !params.head_radii.iter().all(|v| v.is_finite() && *v > 0.0)
        || !params.jaw_y.is_finite()
    {
        return Err(FactoryError::BadParam(
            "head parameters must be finite with positive radii".to_string(),
        ));
    }
    if !params.hair_length.is_finite() || !(0.0..=1.0).contains(&params.hair_length) {
        return Err(FactoryError::BadParam(format!(
            "hair length {} outside [0.0, 1.0]",
            params.hair_length
        )));
    }
    let profile: &[(f32, f32, f32)] = &params.profile;
    validate_profile(profile)?;
    let joints = params.joints.clone();
    validate_joints(&joints)?;
    let radial = params.radial_scale;

    let mut parts: Vec<MeshPart> = Vec::new();

    // Arms (upper arm .045->.038, forearm .038->.030, flattened hand).
    for (side_name, prefix) in [("left", "Left"), ("right", "Right")] {
        let chain = [
            joint_pos(&joints, &format!("{prefix}Arm")),
            joint_pos(&joints, &format!("{prefix}ForeArm")),
            joint_pos(&joints, &format!("{prefix}Hand")),
            joint_pos(&joints, &format!("{prefix}HandMiddle1")),
        ];
        parts.push(finish_part(build_tube(
            &format!("arm_{side_name}"),
            &chain,
            &[
                0.045 * radial,
                0.038 * radial,
                0.030 * radial,
                0.026 * radial,
            ],
            10,
            true,
            (0.0, 0.25, 0.25, 0.50),
            &joints,
        ))?);
    }

    // Body lathe.
    parts.push(finish_part(build_lathe(
        "body",
        profile,
        params.body_y.0,
        params.body_y.1,
        18,
        16,
        radial,
        (0.25, 0.0, 0.50, 0.50),
        &joints,
    ))?);

    // Hair shell and head.
    let (hair_part, _) = build_head(
        "hair",
        true,
        &joints,
        params.head_center,
        params.head_radii,
        params.jaw_y,
        params.hair_length,
    );
    parts.push(finish_part(hair_part)?);
    let (head_part, head_masks) = build_head(
        "head",
        false,
        &joints,
        params.head_center,
        params.head_radii,
        params.jaw_y,
        params.hair_length,
    );
    let head_part = finish_part(head_part)?;

    // Legs (thigh .078->.056, shin .054->.036, foot toe box).
    for (side_name, prefix) in [("left", "Left"), ("right", "Right")] {
        let chain = [
            joint_pos(&joints, &format!("{prefix}UpLeg")),
            joint_pos(&joints, &format!("{prefix}Leg")),
            joint_pos(&joints, &format!("{prefix}Foot")),
            joint_pos(&joints, &format!("{prefix}ToeBase")),
            joint_pos(&joints, &format!("{prefix}Toe_End")),
        ];
        parts.push(finish_part(build_tube(
            &format!("leg_{side_name}"),
            &chain,
            &[
                0.078 * radial,
                0.054 * radial,
                0.036 * radial,
                0.030 * radial,
                0.018 * radial,
            ],
            12,
            true,
            (0.0, 0.25, 0.25, 0.50),
            &joints,
        ))?);
    }

    // Outfit leg shells (both legs merged into one part, tubes x1.05).
    {
        let mut merged_vertices: Vec<Vertex> = Vec::new();
        let mut merged_indices: Vec<u32> = Vec::new();
        for prefix in ["Left", "Right"] {
            let chain = [
                joint_pos(&joints, &format!("{prefix}UpLeg")),
                joint_pos(&joints, &format!("{prefix}Leg")),
                joint_pos(&joints, &format!("{prefix}Foot")),
            ];
            let tube = build_tube(
                "outfit_legs",
                &chain,
                &[
                    0.078 * 1.05 * radial,
                    0.054 * 1.05 * radial,
                    0.036 * 1.05 * radial,
                ],
                12,
                false,
                (0.0, 0.50, 0.25, 0.75),
                &joints,
            );
            let base_idx = merged_vertices.len() as u32;
            merged_vertices.extend(tube.vertices);
            merged_indices.extend(tube.indices.iter().map(|i| i + base_idx));
        }
        parts.push(finish_part(MeshPart {
            indices: merged_indices,
            name: "outfit_legs".to_string(),
            vertices: merged_vertices,
        })?);
    }

    // Outfit torso shell (lathe x1.035, y 0.97..1.47).
    parts.push(finish_part(build_lathe(
        "outfit_torso",
        profile,
        params.outfit_y.0,
        params.outfit_y.1,
        16,
        16,
        1.035 * radial,
        (0.50, 0.25, 0.75, 0.50),
        &joints,
    ))?);

    parts.push(head_part);

    Ok(BaseMesh {
        head_masks,
        joints,
        parts,
    })
}
