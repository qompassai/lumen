//! Canonical skeleton and procedural base mesh.
//!
//! Contract summary:
//! - Skeleton: Mixamo-compatible joint names, Y-up, facing +Z, meters.
//!   Bind positions are world-space positions for a 1.70 m mature female
//!   base. Joint local translations are derived as world minus parent
//!   world by the GLB writer.
//! - Mesh: deterministic procedural generation only (no external assets).
//!   Parts are separate primitives under one skin: `arm_left`, `arm_right`,
//!   `body`, `eye_left`, `eye_right`, `hair`, `hand_left`, `hand_right`,
//!   `head`, `leg_left`, `leg_right`, `outfit_legs`, `outfit_torso`
//!   (stored in construction order, listed alphabetically here).
//! - Skinning: at most four influences per vertex, weights from
//!   distance-to-bone-segment falloff, normalized to sum to 1 — except
//!   the construction-weighted parts (D-1290): the eyeballs are rigid
//!   on their eye joint, and the hands carry per-segment ownership
//!   weights assigned at construction (prima cast census, see
//!   [`build_hand`]). Head and hair strip eye-joint influence: the
//!   skull never follows gaze.
//! - Per-character frame (GAP-11-3 / GAP-11-4): a [`ScaledBuild`]
//!   may carry per-band width/depth scales (profile rings, limb
//!   tubes, head radii, joint spans) and a bust delta shaped into
//!   the torso profile ([`apply_bust_delta`]). Both default to the
//!   canonical base exactly (scales 1.0, delta absent), so the
//!   unparameterized build is bit-for-bit the Phase 1a base.
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
///
/// Union skeleton (D-1282): the table carries 67 nodes — the 54
/// articulation joints of the union set (the 52 joints shared with
/// the torogao cast skeleton plus `LeftEye` / `RightEye`) and the 13
/// `*_End` landmark nodes. Landmarks are construction points, not
/// articulation: they never enter a skin's joint list and never
/// receive skin weights (see [`is_landmark_joint`] and
/// [`skin_joint_indices`]); they remain nodes so construction reads
/// (total height, leg-tube endpoints, fidelity landmarks) keep
/// working by name.
pub fn joint_table() -> Vec<Joint> {
    let mut out: Vec<Joint> = Vec::with_capacity(67);
    out.push(j("Hips", None, [0.0, 1.02, 0.0]));
    out.push(j("Spine", Some("Hips"), [0.0, 1.14, 0.0]));
    out.push(j("Spine1", Some("Spine"), [0.0, 1.26, 0.0]));
    out.push(j("Spine2", Some("Spine1"), [0.0, 1.38, 0.0]));
    out.push(j("Neck", Some("Spine2"), [0.0, 1.50, 0.0]));
    out.push(j("Head", Some("Neck"), [0.0, 1.58, 0.0]));
    out.push(j("HeadTop_End", Some("Head"), [0.0, 1.78, 0.0]));
    // Eye joints (union D-1282): centred on the painted-eye centre
    // the head masks anchor to (`EYE_CENTER_NX` / `EYE_CENTER_NY`
    // against the default head ellipsoid), at eyeball-centre depth
    // (0.62 of the front radius from the ellipsoid centre plane).
    // The X span (±0.031 m) matches the cast skeleton's eye offset
    // (±0.0325 m from its Head joint) within 1.5 mm.
    let eye_pos = |sign: f32| {
        [
            sign * EYE_CENTER_NX * HEAD_RADII[0],
            HEAD_CENTER[1] + EYE_CENTER_NY * HEAD_RADII[1],
            HEAD_CENTER[2] + EYE_DEPTH_FRACTION * HEAD_RADII[2],
        ]
    };
    out.push(j("LeftEye", Some("Head"), eye_pos(-1.0)));
    out.push(j("RightEye", Some("Head"), eye_pos(1.0)));
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

/// Landmark joints are the `*_End` terminal nodes of the union
/// table (D-1282): construction points (total height, tube chain
/// endpoints, fidelity landmarks), never articulation. A landmark
/// has no child segment, carries no skin influence, and is excluded
/// from every skin's joint list; it remains a node so name-based
/// construction reads keep working.
pub fn is_landmark_joint(name: &str) -> bool {
    name.ends_with("_End")
}

/// Indices (into `joints`) of the skin's joints, in table order:
/// every non-landmark joint. For the canonical union table this is
/// the 54 articulation joints.
pub fn skin_joint_indices(joints: &[Joint]) -> Vec<usize> {
    joints
        .iter()
        .enumerate()
        .filter(|(_, joint)| !is_landmark_joint(joint.name))
        .map(|(idx, _)| idx)
        .collect()
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
/// Landmark joints (`*_End`, see [`is_landmark_joint`]) are never
/// scored: they carry no skin influence in the union skeleton.
pub fn compute_skin_weights(
    position: [f32; 3],
    joints: &[Joint],
) -> ([u16; MAX_SKIN_INFLUENCES], [f32; MAX_SKIN_INFLUENCES]) {
    // Score per joint: best (smallest) distance to any segment ending at
    // that joint; the root uses distance to its point.
    let mut scored: Vec<(usize, f32)> = Vec::with_capacity(joints.len());
    for (idx, joint) in joints.iter().enumerate() {
        if is_landmark_joint(joint.name) {
            continue;
        }
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

/// A vertex with construction-assigned weights (D-1290): the
/// eyeballs and hands do not use distance falloff — their weight
/// scheme is part of their construction contract. `influences` is
/// (joint table index, weight) in descending weight order, weights
/// summing to 1; unused slots repeat the first joint at weight 0.
fn weighted_vertex(position: [f32; 3], uv: [f32; 2], influences: &[(u16, f32)]) -> Vertex {
    debug_assert!(!influences.is_empty());
    debug_assert!(influences.iter().all(|(_, w)| w.is_finite() && *w >= 0.0));
    let mut joints = [influences.first().map_or(0, |&(idx, _)| idx); MAX_SKIN_INFLUENCES];
    let mut weights = [0.0f32; MAX_SKIN_INFLUENCES];
    for (slot, &(idx, w)) in influences.iter().take(MAX_SKIN_INFLUENCES).enumerate() {
        joints[slot] = idx;
        weights[slot] = w;
    }
    Vertex {
        joints,
        normal: [0.0, 1.0, 0.0],
        position,
        uv,
        weights,
    }
}

/// Remove the named joints' influence from every vertex and
/// renormalize what remains (D-1290 skull pinning: head and hair
/// vertices must not follow the eye joints — gaze moves only the
/// eyeball parts). A vertex left with no influence at all falls
/// back to the Head joint at weight 1; for head/hair geometry the
/// Head segment always scores, so the fallback is defensive only.
fn strip_joint_influence(vertices: &mut [Vertex], joints: &[Joint], names: &[&str]) {
    let banned: Vec<u16> = names
        .iter()
        .filter_map(|name| joint_index(joints, name))
        .map(|idx| idx as u16)
        .collect();
    if banned.is_empty() {
        return;
    }
    let head_idx = joint_index(joints, "Head").map(|idx| idx as u16);
    for vertex in vertices.iter_mut() {
        let mut kept: [(u16, f32); MAX_SKIN_INFLUENCES] = [(0, 0.0); MAX_SKIN_INFLUENCES];
        let mut kept_len = 0usize;
        let mut sum = 0.0f32;
        for slot in 0..MAX_SKIN_INFLUENCES {
            let (idx, w) = (vertex.joints[slot], vertex.weights[slot]);
            if w > 0.0 && !banned.contains(&idx) {
                kept[kept_len] = (idx, w);
                kept_len += 1;
                sum += w;
            }
        }
        if kept_len == 0 || sum <= 0.0 {
            if let Some(head) = head_idx {
                vertex.joints = [head; MAX_SKIN_INFLUENCES];
                vertex.weights = [1.0, 0.0, 0.0, 0.0];
            }
            continue;
        }
        for slot in 0..MAX_SKIN_INFLUENCES {
            if slot < kept_len {
                vertex.joints[slot] = kept[slot].0;
                vertex.weights[slot] = kept[slot].1 / sum;
            } else {
                vertex.joints[slot] = kept[0].0;
                vertex.weights[slot] = 0.0;
            }
        }
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

/// Minimum accepted bust delta (bust − underbust circumference), cm.
/// The f64 parameter record validates against these same bounds
/// (`params.rs` converts; there is one range, owned here next to the
/// shaping that consumes it).
pub const BUST_DELTA_MIN_CM: f32 = 5.0;
/// Maximum accepted bust delta (bust − underbust circumference), cm.
pub const BUST_DELTA_MAX_CM: f32 = 35.0;

/// Circumference of a lathe ring (an ellipse with semi-axes `rx`,
/// `rz`), Ramanujan's first approximation. The error is far below
/// shaping tolerance at the profile's mild eccentricities, and the
/// perimeter is homogeneous of degree 1: scaling both semi-axes by
/// `k` scales the circumference by exactly `k`, which is what makes
/// [`apply_bust_delta`] exact at the bust ring.
fn ring_circumference(rx: f32, rz: f32) -> f32 {
    let a = rx;
    let b = rz;
    std::f32::consts::PI * (3.0 * (a + b) - ((3.0 * a + b) * (a + 3.0 * b)).sqrt())
}

/// Profile heights of the underbust and bust rings located by
/// [`apply_bust_delta`], returned so callers can measure the shaped
/// result at the right heights.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BustAnchors {
    /// Height of the bust ring (metres).
    pub bust_y: f32,
    /// Height of the underbust ring (metres).
    pub under_y: f32,
}

/// A torso profile after bust shaping, with the anchors the
/// shaping located in the source profile.
#[derive(Clone, Debug, PartialEq)]
pub struct ShapedProfile {
    /// Underbust/bust ring heights of the source profile.
    pub anchors: BustAnchors,
    /// The shaped profile rings.
    pub profile: Vec<(f32, f32, f32)>,
}

/// Shape a torso profile so its bust circumference exceeds its
/// underbust circumference by `delta_cm` centimetres.
///
/// The underbust ring is the profile's narrowest interior ring (the
/// torso's waist); the bust ring is the widest ring above it. (The
/// widest ring overall may be the hip — it is not the bust.) The
/// bust ring scales by exactly the factor that lands its
/// circumference on `underbust + delta` (degree-1 homogeneity, see
/// [`ring_circumference`]); rings between underbust and bust follow
/// a smoothstep ramp, rings above the bust decay with a gaussian,
/// and rings at or below the underbust never move — so the
/// underbust circumference is an invariant of the shaping and the
/// delta is controlled by the bust ring alone. The outfit torso
/// lathe consumes the same profile, so a fitted garment follows
/// the body. Fails closed on a non-finite or out-of-range delta, a
/// profile with no ring below the bust ring, or a shaped profile
/// that fails validation.
///
/// Prior art: this shaping was first built on the factory Stage-3
/// line (bust-param branch, merged there as 65e3c9a) which never
/// landed on main; it is ported here onto the landed factory as
/// the GAP-11-3 fix, wired end-to-end through the identity and
/// parameter records.
pub fn apply_bust_delta(
    profile: &[(f32, f32, f32)],
    delta_cm: f32,
) -> Result<ShapedProfile, FactoryError> {
    if !delta_cm.is_finite() || !(BUST_DELTA_MIN_CM..=BUST_DELTA_MAX_CM).contains(&delta_cm) {
        return Err(FactoryError::BadParam(format!(
            "bust delta {delta_cm} cm outside [{BUST_DELTA_MIN_CM}, {BUST_DELTA_MAX_CM}] (fail closed, not clamped)"
        )));
    }
    shape_profile(profile, delta_cm)
}

/// The shaping core of [`apply_bust_delta`], without the public
/// range check — [`shape_bust`]'s correction pass may legitimately
/// overshoot the public range by the modelled flattening error.
fn shape_profile(
    profile: &[(f32, f32, f32)],
    delta_cm: f32,
) -> Result<ShapedProfile, FactoryError> {
    validate_profile(profile)?;
    if profile.len() < 3 {
        return Err(FactoryError::BadParam(
            "profile needs at least three rings to locate a bust".to_string(),
        ));
    }
    let circs: Vec<f32> = profile
        .iter()
        .map(|&(_, rx, rz)| ring_circumference(rx, rz))
        .collect();
    let under_i = (1..profile.len() - 1)
        .min_by(|&a, &b| {
            circs[a]
                .partial_cmp(&circs[b])
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap_or(1);
    let bust_i = (under_i + 1..profile.len())
        .max_by(|&a, &b| {
            circs[a]
                .partial_cmp(&circs[b])
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap_or(under_i + 1);
    let target = circs[under_i] + delta_cm / 100.0;
    let k = target / circs[bust_i];
    let y_b = profile[bust_i].0;
    let y_u = profile[under_i].0;
    let y_top = profile[profile.len() - 1].0;
    let shaped: Vec<(f32, f32, f32)> = profile
        .iter()
        .map(|&(y, rx, rz)| {
            let w = if y <= y_u {
                0.0
            } else if y <= y_b {
                let t = (y - y_u) / (y_b - y_u);
                t * t * (3.0 - 2.0 * t)
            } else if y_top > y_b {
                let d = (y - y_b) / (y_top - y_b);
                (-d * d).exp()
            } else {
                1.0
            };
            let scale = 1.0 + w * (k - 1.0);
            (y, rx * scale, rz * scale)
        })
        .collect();
    validate_profile(&shaped)?;
    Ok(ShapedProfile {
        anchors: BustAnchors {
            bust_y: y_b,
            under_y: y_u,
        },
        profile: shaped,
    })
}

/// The bust delta (cm) the built mesh will actually measure for a
/// shaped profile, modelled without building the mesh: sample the
/// profile at the body lathe's ring heights (the lathe only ever
/// sees those samples, which flattens extrema between rings), scan
/// section circumferences around the anchors, and apply the
/// inscribed-polygon factor of the lathe's 16 radial segments.
fn model_delta_cm(shaped: &[(f32, f32, f32)], body_y: (f32, f32), anchors: BustAnchors) -> f32 {
    const RING_STEPS: usize = 18; // the body lathe's ring count
    const POLYGON_FACTOR: f32 = 0.993_59; // sin(pi/16) / (pi/16)
    let effective: Vec<(f32, f32, f32)> = (0..=RING_STEPS)
        .map(|i| {
            let y = body_y.0 + (body_y.1 - body_y.0) * (i as f32 / RING_STEPS as f32);
            let (rx, rz) = profile_at(shaped, y);
            (y, rx, rz)
        })
        .collect();
    let section = |y: f32| {
        let (rx, rz) = profile_at(&effective, y);
        ring_circumference(rx, rz) * POLYGON_FACTOR
    };
    let mut bust = 0.0f32;
    let mut y = anchors.bust_y - 0.035;
    while y <= anchors.bust_y + 0.005 {
        bust = bust.max(section(y));
        y += 0.001;
    }
    let mut under = f32::INFINITY;
    let mut y = anchors.under_y - 0.035;
    while y <= anchors.under_y + 0.035 {
        under = under.min(section(y));
        y += 0.001;
    }
    (bust - under) * 100.0
}

/// Shape a profile to a bust delta, then correct once against the
/// mesh model: the lathe's ring sampling flattens the bust peak, so
/// a profile shaped to the analytic target measures up to ~1 cm
/// small on the built mesh. One deterministic correction pass —
/// reshape from the original profile at `delta + (delta − modelled)`
/// — lands the mesh measure on target. This is what makes the
/// parameter a measured fact about the generated character rather
/// than a profile-space intention.
fn shape_bust(
    profile: &[(f32, f32, f32)],
    delta_cm: f32,
    body_y: (f32, f32),
) -> Result<Vec<(f32, f32, f32)>, FactoryError> {
    let shaped = apply_bust_delta(profile, delta_cm)?;
    let err = delta_cm - model_delta_cm(&shaped.profile, body_y, shaped.anchors);
    if err.abs() <= 0.05 {
        return Ok(shaped.profile);
    }
    let reshaped = shape_profile(profile, delta_cm + err)?;
    Ok(reshaped.profile)
}

/// Band index of a height under the estate band definitions
/// (0 = head, 1 = torso, 2 = leg), using the same anchors as
/// [`scaled_joint_table`]: at or below the hips is the leg band,
/// at or below the neck is the torso band, above it the head band.
fn band_of_y(y: f32, hips_y: f32, neck_y: f32) -> usize {
    if y <= hips_y {
        2
    } else if y <= neck_y {
        1
    } else {
        0
    }
}

/// Apply per-band width (X) and depth (Z) scales to a torso
/// profile: each ring's radii are multiplied by the scales of the
/// band its height falls in (band anchors taken from `joints`, the
/// same table the profile was remapped against). With every scale
/// at exactly 1.0 each product is the identity, so the default
/// build's profile is bit-for-bit the input profile.
fn band_scaled_profile(
    profile: &[(f32, f32, f32)],
    joints: &[Joint],
    width_scales: [f32; 3],
    depth_scales: [f32; 3],
) -> Vec<(f32, f32, f32)> {
    let hips_y = joint_pos(joints, "Hips")[1];
    let neck_y = joint_pos(joints, "Neck")[1];
    profile
        .iter()
        .map(|&(y, rx, rz)| {
            let band = band_of_y(y, hips_y, neck_y);
            (y, rx * width_scales[band], rz * depth_scales[band])
        })
        .collect()
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

/// Eye deform-region centre, normalized head X (`nx`): the masks
/// anchor lid/corner deformation here, and `project.rs` paints the
/// eye features at the same point (it re-exports this constant —
/// one source of truth, locked by test).
///
/// GAP-11-2 reconciliation (the two "eye span" numbers): this
/// centre puts the eye-centre span at 2 × 0.36 = 0.72 head radii,
/// i.e. a span share of **0.36 of the head-ellipsoid width**. The
/// 2D canon band (A0 / Canon-01) puts the eye-centre span at
/// **0.704 of the face width**, where "face width" is the widest
/// span of the colour-segmented *skin component* of a drawing
/// (`identity.rs`'s face segmentation) — a sub-region bounded by
/// hair and sidelocks, systematically narrower than the full head
/// width the mesh number divides by. The two numbers therefore
/// measure different quantities against different denominators;
/// neither is a misprint of the other, and converting between
/// them requires the source art's face-width ÷ head-width ratio,
/// which no estate record currently measures. This constant is a
/// deform anchor + paint centre, NOT a claim that a frontal
/// render reproduces the 2D band: closing that render-side gap is
/// the sprite→3D fidelity work (PARITY leg 2c), and renumbering
/// this anchor to 0.70 would misplace the lid deformation fields
/// without moving any rendered eye. (Since D-1290 the eyeballs
/// are real geometry pivoting on this same anchor, and
/// `project.rs` paints from it; the band question above is
/// unchanged.)
pub const EYE_CENTER_NX: f32 = 0.36;
/// Eye deform-region centre, normalized head Y (`ny`).
pub const EYE_CENTER_NY: f32 = 0.12;

/// Depth of the eye *joint* (eyeball centre) as a fraction of the
/// head ellipsoid's front radius from its centre plane: between the
/// centre plane and the front surface at the eye centre. Used only
/// by [`joint_table`] to place `LeftEye` / `RightEye`.
pub const EYE_DEPTH_FRACTION: f32 = 0.62;

/// Eyeball diameter as a fraction of the head ellipsoid's width
/// (D-1290). The torogao cast's prima.glb measures 0.1514 (ball
/// diameter 0.0250 m over a 0.1651 m head width; 396 verts per eye,
/// all at weight 1.000 on the eye joint, ball radius 0.0130 m) —
/// but the factory's painted eye is larger than prima's: its
/// sclera spans 0.20 of the head width (`project.rs`'s
/// EYE_HALF_WIDTH 0.20 of the half-width, each side). A cast-ratio
/// ball presented a visibly smaller eye than the flat paint it
/// replaces (Stage B render comparison), so the ball is sized to
/// the factory's own painted sclera: diameter ratio 0.20, and the
/// ball's front hemisphere carries the painted eye at flat-face
/// proportions (see [`build_eyeball`]).
pub const EYEBALL_WIDTH_RATIO: f32 = 0.20;
/// Eyeball grid latitude steps (with the longitude steps below this
/// yields the cast's 396 verts per eye: 18 rows x 22).
const EYEBALL_LAT_STEPS: usize = 17;
/// Eyeball grid longitude steps.
const EYEBALL_LON_STEPS: usize = 22;
/// Head grid resolution for the hair shell (the original grid).
const HEAD_LAT_STEPS: usize = 14;
/// Head grid longitude steps for the hair shell.
const HEAD_LON_STEPS: usize = 18;
/// Head grid resolution for the head part itself (D-1290): doubled
/// so the eye socket can be formed; every coarse-grid vertex is a
/// strict subset (k/14 = 2k/28, k/18 = 2k/36), so un-displaced head
/// vertices keep their exact positions.
const HEAD_FINE_LAT_STEPS: usize = 28;
/// Head grid longitude steps for the head part itself.
const HEAD_FINE_LON_STEPS: usize = 36;
/// Eye-socket opening ellipse half-extents in normalized head
/// coordinates (nx, ny): between the painted sclera ellipse
/// (0.20, 0.13) and the lash band (0.22, 0.155) of `project.rs`.
const SOCKET_ELLIPSE: [f32; 2] = [0.215, 0.150];
/// Socket displacement reaches full projection at this ellipse
/// metric and blends out to [`SOCKET_BLEND_OUTER`].
const SOCKET_BLEND_INNER: f32 = 0.85;
/// Socket displacement reaches zero at this ellipse metric.
const SOCKET_BLEND_OUTER: f32 = 1.40;
/// Radial clearance between the eyeball and its socket wall
/// (the cast's socket verts sit inside the ball's radius; the
/// factory keeps a small positive gap instead).
const SOCKET_CLEARANCE: f32 = 0.0007;

/// Eyeball radius for a head with the given radii: the cast ratio
/// applied to the head's width.
fn eyeball_radius(head_radii: [f32; 3]) -> f32 {
    EYEBALL_WIDTH_RATIO * head_radii[0]
}

/// Default head ellipsoid center (meters, world space).
pub const HEAD_CENTER: [f32; 3] = [0.0, 1.665, 0.005];
/// Default head ellipsoid radii (meters).
pub const HEAD_RADII: [f32; 3] = [0.086, 0.106, 0.096];
/// Default jaw-taper threshold height (meters).
pub const HEAD_JAW_Y: f32 = 1.62;

/// Hair length of the generic base build ([`ScaledBuild::default`]).
pub const HAIR_LENGTH_DEFAULT: f32 = 0.65;
/// Hair cap shell scale over the head radii (projection v2: ~1.5 cm
/// of hair volume at the default head; v1's 1.07 sat ~6 mm off the
/// head and read as a rim).
pub const HAIR_SHELL_SCALE: f32 = 1.15;
/// Hairline height on the hair cap (normalized head Y): the cap
/// covers the front of the head only above it.
pub const HAIR_HAIRLINE_NY: f32 = 0.55;
/// The hair cap has no geometry below this normalized head Y (its
/// underside, which would sit under the chin).
pub const HAIR_CAP_BOTTOM_NY: f32 = -0.9;
/// Outfit torso shell scale over the body profile (v1: 1.035).
pub const OUTFIT_TORSO_SCALE: f32 = 1.06;
/// Outfit leg shell scale over the leg tube radii (v1: 1.05).
pub const OUTFIT_LEGS_SCALE: f32 = 1.08;
/// Hair tail root radius as a fraction of the head's X radius.
pub const HAIR_TAIL_ROOT_FRACTION: f32 = 0.40;
/// Hair tail tip radius as a fraction of its root radius.
pub const HAIR_TAIL_TIP_FRACTION: f32 = 0.25;
/// Ring count of the hair tail's vertical drop (bounded geometry).
pub const HAIR_TAIL_DROP_RINGS: usize = 4;

/// Displace one head-surface vertex into the eye socket (D-1290).
///
/// The factory head is a closed ellipsoid; the cast construction
/// (prima.glb census) wraps the head surface into a real socket
/// around the eyeball, with wall verts reaching inside the ball's
/// radius. The parametric equivalent: vertices inside the socket
/// ellipse (normalized head coords against [`SOCKET_ELLIPSE`]) are
/// pulled along their ray from the eye joint onto a sphere of
/// eyeball radius + [`SOCKET_CLEARANCE`] around the joint, with a
/// smoothstep blend out to [`SOCKET_BLEND_OUTER`] so the funnel
/// wall rises back to the untouched surface. Vertices keep their
/// UVs, so the funnel wall samples the painted sclera/lash through
/// the head's own mapping — the visible annulus between the ball's
/// silhouette and the funnel mouth reads as the eye's outer
/// structure, exactly the role the wall plays in the cast mesh.
fn socket_displace(pos: [f32; 3], center: [f32; 3], radii: [f32; 3], joints: &[Joint]) -> [f32; 3] {
    let nx = (pos[0] - center[0]) / radii[0];
    let ny = (pos[1] - center[1]) / radii[1];
    let nz = (pos[2] - center[2]) / radii[2];
    // Front hemisphere only (the flatten gate's region, widened):
    // the back of the head shares (nx, ny) with the eye region.
    if nz <= 0.25 {
        return pos;
    }
    let mut out = pos;
    for (name, sign) in [("LeftEye", -1.0f32), ("RightEye", 1.0f32)] {
        let dx = (nx - sign * EYE_CENTER_NX) / SOCKET_ELLIPSE[0];
        let dy = (ny - EYE_CENTER_NY) / SOCKET_ELLIPSE[1];
        let metric = (dx * dx + dy * dy).sqrt();
        if metric >= SOCKET_BLEND_OUTER {
            continue;
        }
        let Some(eye) = joint_index(joints, name).map(|idx| joints[idx].position) else {
            continue;
        };
        let ray = sub(out, eye);
        let dist = length(ray);
        let wall = eyeball_radius(radii) + SOCKET_CLEARANCE;
        if dist <= wall || dist <= 1e-9 {
            continue;
        }
        let t = ((SOCKET_BLEND_OUTER - metric) / (SOCKET_BLEND_OUTER - SOCKET_BLEND_INNER))
            .clamp(0.0, 1.0);
        let blend = t * t * (3.0 - 2.0 * t);
        let new_dist = dist + (wall - dist) * blend;
        let k = new_dist / dist;
        out = [
            eye[0] + ray[0] * k,
            eye[1] + ray[1] * k,
            eye[2] + ray[2] * k,
        ];
    }
    out
}

/// Ellipsoid head (also the basis for the hair shell when `hair` is set).
/// The head part builds on the doubled grid ([`HEAD_FINE_LAT_STEPS`] x
/// [`HEAD_FINE_LON_STEPS`]) so the eye sockets can be formed; the hair
/// shell keeps the original grid ([`HEAD_LAT_STEPS`] x [`HEAD_LON_STEPS`]).
fn build_head(
    name: &str,
    hair: bool,
    joints: &[Joint],
    center: [f32; 3],
    radii: [f32; 3],
    jaw_y: f32,
    grid: (usize, usize),
) -> (MeshPart, Vec<HeadRegionWeights>) {
    let (lat_steps, lon_steps) = grid;
    let scale = if hair { HAIR_SHELL_SCALE } else { 1.0 };
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
                // Hair covers the back, the sides, and the top-front
                // above the hairline; the face is open all the way
                // down, and the cap's underside is dropped. (v1 closed
                // the front below ny -0.55, a band across the chin
                // that read as a hood once the shell was visible.)
                let face_opening =
                    (dir[2] > 0.35 && dir[1] < HAIR_HAIRLINE_NY) || dir[1] < HAIR_CAP_BOTTOM_NY;
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
                // Form the eye sockets around the eyeball joints
                // (D-1290); the eyeball parts themselves are built
                // by `build_eyeball`.
                pos = socket_displace(pos, center, radii, joints);
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
    // Skull pinning (D-1290): neither the head nor the hair shell
    // follows the eye joints. The union build's interim falloff eye
    // region on the head surface is replaced by the rigid eyeball
    // parts; gaze rotation must move nothing else.
    strip_joint_influence(&mut vertices, joints, &["LeftEye", "RightEye"]);
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

/// The painted sclera's half-extents in normalized head
/// coordinates (`project.rs`'s `EYE_HALF_WIDTH` / `EYE_HALF_HEIGHT`,
/// the source of truth; duplicated here because the paint module
/// builds on this one, not the reverse).
const PAINT_SCLERA_HALF: [f32; 2] = [0.20, 0.13];
/// Sine of the eyeball angle at which the ball-local paint chart
/// reaches the sclera's edge: 1.0, the ball's equator — the front
/// hemisphere IS the painted eye disc. The chart is linear in the
/// direction components (a sine chart), so every paint feature
/// lands at the same fraction of the visible eye as on the flat
/// face: the iris (0.11 against the sclera's 0.20) edges at
/// sin 0.55, the pupil (0.05) at sin 0.25.
const EYEBALL_PAINT_EDGE_SIN: f32 = 1.0;

/// One eyeball (D-1290): a sphere of [`eyeball_radius`] centred
/// exactly on its eye joint, so the joint is the ball's pivot and
/// gaze rotation is rigid by construction. 396 verts, matching the
/// torogao cast's count (prima.glb census: 396 verts per eye, all
/// at weight 1.000 on the eye joint).
///
/// The UV chart is ball-local, not the head's sphere mapping: a
/// direction-from-head-centre chart has its scale set by the head
/// radius (~0.10 m) and compresses the painted iris ~6x on a
/// 0.013 m ball (measured on the first fix build: the iris landed
/// as a ~3 mm patch, displaced from the ball's front). Instead
/// each vertex's direction from the joint is expressed in the
/// eye's frame (forward = joint -> face surface at the eye
/// centre) and mapped linearly onto paint coordinates around the
/// eye centre, reaching the sclera's edge at
/// [`EYEBALL_PAINT_EDGE_SIN`]; the resulting (nx, ny) converts to
/// UVs through `project.rs`'s `face_point_px` sphere mapping, so
/// the ball samples the same atlas texels as the face, at ball
/// scale, and the paint rides the ball when the eye joint rotates.
fn build_eyeball(
    name: &str,
    eye_joint: &str,
    joints: &[Joint],
    head_center: [f32; 3],
    head_radii: [f32; 3],
) -> MeshPart {
    let center = joint_pos(joints, eye_joint);
    let eye_idx = joint_index(joints, eye_joint).map_or(0, |idx| idx as u16);
    let radius = eyeball_radius(head_radii);
    // Eye frame: forward from the joint to the face surface at the
    // eye centre (the generating direction of the painted eye,
    // scaled by the head radii), side and up completing it.
    let sign = if center[0] < head_center[0] {
        -1.0
    } else {
        1.0
    };
    let gen_z = (1.0 - EYE_CENTER_NX * EYE_CENTER_NX - EYE_CENTER_NY * EYE_CENTER_NY)
        .max(0.0)
        .sqrt();
    let surface = [
        head_center[0] + sign * EYE_CENTER_NX * head_radii[0],
        head_center[1] + EYE_CENTER_NY * head_radii[1],
        head_center[2] + gen_z * head_radii[2],
    ];
    let forward = normalize_or(sub(surface, center), [0.0, 0.0, 1.0]);
    let side = normalize_or(cross([0.0, 1.0, 0.0], forward), [1.0, 0.0, 0.0]);
    let up = cross(forward, side);
    let mut vertices: Vec<Vertex> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let mut grid: Vec<Vec<Option<u32>>> = Vec::new();
    for lat in 0..=EYEBALL_LAT_STEPS {
        let phi = (lat as f32 / EYEBALL_LAT_STEPS as f32) * std::f32::consts::PI;
        let mut row: Vec<Option<u32>> = Vec::new();
        for lon in 0..EYEBALL_LON_STEPS {
            let theta = (lon as f32 / EYEBALL_LON_STEPS as f32) * std::f32::consts::TAU;
            let dir = [phi.sin() * theta.cos(), phi.cos(), phi.sin() * theta.sin()];
            let pos = [
                center[0] + dir[0] * radius,
                center[1] + dir[1] * radius,
                center[2] + dir[2] * radius,
            ];
            // Ball-local paint chart (see the fn docs), then the
            // shared sphere mapping of `project.rs::face_point_px`.
            let mut pnx = sign * EYE_CENTER_NX
                + dot(dir, side) * (PAINT_SCLERA_HALF[0] / EYEBALL_PAINT_EDGE_SIN);
            let mut pny =
                EYE_CENTER_NY + dot(dir, up) * (PAINT_SCLERA_HALF[1] / EYEBALL_PAINT_EDGE_SIN);
            let disc = pnx * pnx + pny * pny;
            if disc > 0.998 {
                let k = 0.999 / disc.sqrt();
                pnx *= k;
                pny *= k;
            }
            let pnz = (1.0 - pnx * pnx - pny * pny).max(0.0).sqrt();
            let mut theta_c = pnz.atan2(pnx);
            if theta_c < 0.0 {
                theta_c += std::f32::consts::TAU;
            }
            let phi_c = pny.clamp(-1.0, 1.0).acos();
            let uv = [
                theta_c / std::f32::consts::TAU * 0.25,
                phi_c / std::f32::consts::PI * 0.25,
            ];
            let idx = vertices.len() as u32;
            vertices.push(weighted_vertex(pos, uv, &[(eye_idx, 1.0)]));
            row.push(Some(idx));
        }
        grid.push(row);
    }
    for lat in 0..EYEBALL_LAT_STEPS {
        for lon in 0..EYEBALL_LON_STEPS {
            let next = (lon + 1) % EYEBALL_LON_STEPS;
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
    MeshPart {
        indices,
        name: name.to_string(),
        vertices,
    }
}

/// The hair tail (ponytail mass, projection v2): a tapered tube from
/// the back of the hair cap near the crown out to a vertical drop
/// line behind the head and back, then straight down to
/// `neck_y + (hips_y - neck_y) * hair_length` on the (scaled) joints.
/// `None` at hair length 0.0 (no hair below the neck).
fn build_hair_tail(
    params: &ScaledBuild,
    joints: &[Joint],
    profile: &[(f32, f32, f32)],
    radii: [f32; 3],
) -> Option<MeshPart> {
    if params.hair_length <= 0.0 {
        return None;
    }
    let center = params.head_center;
    let neck_y = joint_pos(joints, "Neck")[1];
    let hips_y = joint_pos(joints, "Hips")[1];
    let bottom_y = neck_y + (hips_y - neck_y) * params.hair_length;
    let root_radius = HAIR_TAIL_ROOT_FRACTION * radii[0];
    let body_back = profile.iter().map(|&(_, _, rz)| rz).fold(0.0f32, f32::max)
        * params.radial_scale
        * OUTFIT_TORSO_SCALE;
    let head_back = radii[2] * HAIR_SHELL_SCALE;
    // The drop line clears both the hair cap and the outfit's back.
    let drop_z = center[2] - head_back.max(body_back) - root_radius;
    let mut points = vec![[
        center[0],
        center[1] + 0.35 * radii[1],
        center[2] - 0.9 * head_back,
    ]];
    for ring in 0..=HAIR_TAIL_DROP_RINGS {
        let t = ring as f32 / HAIR_TAIL_DROP_RINGS as f32;
        points.push([center[0], center[1] + (bottom_y - center[1]) * t, drop_z]);
    }
    let last = (points.len() - 1) as f32;
    let tube_radii: Vec<f32> = (0..points.len())
        .map(|ring| root_radius * (1.0 - (1.0 - HAIR_TAIL_TIP_FRACTION) * ring as f32 / last))
        .collect();
    let mut tail = build_tube(
        "hair",
        &points,
        &tube_radii,
        10,
        false,
        (0.50, 0.00, 0.75, 0.25),
        joints,
    );
    // The tail merges into the hair part, so it takes the hair
    // part's skull pinning too (D-1290): no eye-joint influence.
    strip_joint_influence(&mut tail.vertices, joints, &["LeftEye", "RightEye"]);
    Some(tail)
}

fn gauss(d: f32, sigma: f32) -> f32 {
    (-(d * d) / (2.0 * sigma * sigma)).exp().clamp(0.0, 1.0)
}

fn head_region_weights(pos: [f32; 3], center: [f32; 3], radii: [f32; 3]) -> HeadRegionWeights {
    let nx = (pos[0] - center[0]) / radii[0];
    let ny = (pos[1] - center[1]) / radii[1];
    let nz = ((pos[2] - center[2]) / radii[2]).clamp(-1.0, 1.0);
    let front = nz.clamp(0.0, 1.0);
    let eye_y = EYE_CENTER_NY;
    let eye_x = EYE_CENTER_NX;
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
/// to the final segment's rings (toe boxes).
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

/// Finger cross-section vertex count: the adopted hand method's
/// magic number (D-643 / dossier H-07 — model a hexagon, subdivide
/// once, and the finger's section has exactly 12 edges; sockets
/// and bridging all key off that count).
const FINGER_RADIAL: usize = 12;
/// Palm loft cross-section vertex count.
const PALM_SEGMENTS: usize = 20;
/// Palm cross-section superellipse exponent: a slab with rounded
/// edges (palm + thumb-base thickness, dossier H-04/H-14).
const PALM_SUPERELLIPSE_EXP: f32 = 3.5;
/// Joint flare (dossier H-05/H-21): at a finger ring that sits on
/// a joint, palm-side (-Y) offsets widen by this factor and the
/// ring centre shifts [`FINGER_FLARE_SHIFT`] toward the back of
/// the hand — the rotated loop cut of the modelled method, in
/// parametric form, so curled fingers do not self-clip.
const FINGER_FLARE: f32 = 1.18;
/// Joint flare centre shift toward the back of the hand (+Y).
const FINGER_FLARE_SHIFT: f32 = 0.0008;
/// How far a finger tube's root ring sits inside the palm, behind
/// its first joint (the factory composes parts by overlap; the
/// modelled method's socket-and-bridge join, dossier H-13/H-26,
/// is served by rooting the tube inside the palm loft).
const FINGER_ROOT_DEPTH: f32 = 0.014;
/// Fingertip cap extension beyond the finger's End landmark.
const FINGERTIP_EXT: f32 = 0.006;

/// Palm loft stations: (axial distance from the Hand joint along
/// the Hand->Middle1 axis, half width in Z, half thickness in Y,
/// centre Y offset, centre Z offset). The front ring reuses the
/// last station's section with a per-vertex axial offset from
/// [`PALM_FRONT`].
const PALM_STATIONS: [(f32, f32, f32, f32, f32); 5] = [
    (0.000, 0.030, 0.0155, 0.000, 0.000),
    (0.025, 0.036, 0.0145, 0.001, -0.005),
    (0.050, 0.044, 0.0135, 0.001, -0.011),
    (0.070, 0.050, 0.0125, 0.001, -0.015),
    (0.082, 0.053, 0.0115, 0.000, -0.018),
];

/// The palm's scalloped front edge — pre-formed webbing (dossier
/// H-13/H-27): (world Z, axial distance) control points, piecewise
/// linear, clamped at the ends. The front edge advances at each
/// finger's base and recedes in the valleys between fingers; on
/// the thumb side it falls back to the thumb mound, where the palm
/// wedges toward the thumb's first joint.
const PALM_FRONT: [(f32, f32); 9] = [
    (0.058, 0.045),
    (0.045, 0.088),
    (0.030, 0.080),
    (0.015, 0.090),
    (0.000, 0.082),
    (-0.015, 0.088),
    (-0.030, 0.080),
    (-0.042, 0.084),
    (-0.055, 0.070),
];

/// Axial distance of the palm's front edge at world Z `z`.
fn palm_front_dx(z: f32) -> f32 {
    if z >= PALM_FRONT[0].0 {
        return PALM_FRONT[0].1;
    }
    for pair in PALM_FRONT.windows(2) {
        let (z0, d0) = pair[0];
        let (z1, d1) = pair[1];
        if z <= z0 && z >= z1 {
            let t = (z - z0) / (z1 - z0);
            return d0 + (d1 - d0) * t;
        }
    }
    PALM_FRONT[PALM_FRONT.len() - 1].1
}

/// (chain name fragment, base radius at the first joint) per
/// finger, in joint-table order. Base radii are scaled from the
/// prima cast census: proximal mean radial distances 0.0073–0.0096
/// at a 0.086 m palm run; the factory's palm run is 0.09 m and the
/// ring radius sits between the cast's mean and max radial.
const FINGERS: [(&str, f32); 5] = [
    ("Thumb", 0.0102),
    ("Index", 0.0089),
    ("Middle", 0.0092),
    ("Ring", 0.0087),
    ("Pinky", 0.0076),
];

/// One finger tube (D-1290, hand-build/1 in parametric form): the
/// dossier's one finished finger — 12-edge sections, taper toward
/// the tip, joint flare at the bending rings — instantiated per
/// finger from the joint table's chain.
///
/// One ring station of a finger tube: where the ring sits, how
/// wide it is relative to the finger's base radius, which joints
/// own it, and whether it flares (rings on joints do).
struct FingerStation<'a> {
    /// Ring centre (world space).
    centre: [f32; 3],
    /// Whether this ring sits on a joint and takes the flare.
    flare: bool,
    /// Construction weights as (joint id, weight), descending.
    influences: &'a [(u16, f32)],
    /// Ring radius as a factor of the finger's base radius.
    radius_factor: f32,
}

/// Weights are construction weights in the cast's measured scheme
/// (prima.glb census): a segment is owned by its proximal joint,
/// the ring on a distal joint blends 50/50 with the next owner,
/// and the root ring inside the palm is shared with the Hand
/// joint, whose influence the cast likewise starts inside the
/// palm. Vertices are appended to the hand part being built.
#[allow(clippy::too_many_arguments)]
fn build_finger(
    vertices: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    chain: [[f32; 3]; 4],
    joint_ids: [u16; 4],
    base_radius: f32,
    radial_scale: f32,
) {
    let (j1, j2, j3, end) = (chain[0], chain[1], chain[2], chain[3]);
    let (hand_id, j1_id, j2_id, j3_id) = (joint_ids[0], joint_ids[1], joint_ids[2], joint_ids[3]);
    let dir_a = normalize_or(sub(j2, j1), [1.0, 0.0, 0.0]);
    let dir_c = normalize_or(sub(end, j3), [1.0, 0.0, 0.0]);
    let mid = |a: [f32; 3], b: [f32; 3]| {
        [
            (a[0] + b[0]) * 0.5,
            (a[1] + b[1]) * 0.5,
            (a[2] + b[2]) * 0.5,
        ]
    };
    let root = [
        j1[0] - dir_a[0] * FINGER_ROOT_DEPTH,
        j1[1] - dir_a[1] * FINGER_ROOT_DEPTH,
        j1[2] - dir_a[2] * FINGER_ROOT_DEPTH,
    ];
    let w_root: [(u16, f32); 2] = [(j1_id, 0.55), (hand_id, 0.45)];
    let w_j1: [(u16, f32); 1] = [(j1_id, 1.0)];
    let w_j12: [(u16, f32); 2] = [(j1_id, 0.5), (j2_id, 0.5)];
    let w_j2: [(u16, f32); 1] = [(j2_id, 1.0)];
    let w_j23: [(u16, f32); 2] = [(j2_id, 0.5), (j3_id, 0.5)];
    let w_j3: [(u16, f32); 1] = [(j3_id, 1.0)];
    let stations: [FingerStation<'_>; 8] = [
        FingerStation {
            centre: root,
            flare: false,
            influences: &w_root,
            radius_factor: 1.06,
        },
        FingerStation {
            centre: j1,
            flare: true,
            influences: &w_j1,
            radius_factor: 1.00,
        },
        FingerStation {
            centre: mid(j1, j2),
            flare: false,
            influences: &w_j1,
            radius_factor: 0.95,
        },
        FingerStation {
            centre: j2,
            flare: true,
            influences: &w_j12,
            radius_factor: 0.93,
        },
        FingerStation {
            centre: mid(j2, j3),
            flare: false,
            influences: &w_j2,
            radius_factor: 0.88,
        },
        FingerStation {
            centre: j3,
            flare: true,
            influences: &w_j23,
            radius_factor: 0.85,
        },
        FingerStation {
            centre: mid(j3, end),
            flare: false,
            influences: &w_j3,
            radius_factor: 0.76,
        },
        FingerStation {
            centre: end,
            flare: false,
            influences: &w_j3,
            radius_factor: 0.60,
        },
    ];
    let base = vertices.len() as u32;
    let ring_count = stations.len();
    for (ring, station) in stations.iter().enumerate() {
        let centre0 = station.centre;
        let tangent = if ring + 1 < ring_count {
            sub(stations[ring + 1].centre, centre0)
        } else {
            sub(centre0, stations[ring - 1].centre)
        };
        let tangent = normalize_or(tangent, [0.0, -1.0, 0.0]);
        let helper = if tangent[1].abs() > 0.9 {
            [1.0, 0.0, 0.0]
        } else {
            [0.0, 1.0, 0.0]
        };
        let n_axis = normalize_or(cross(helper, tangent), [1.0, 0.0, 0.0]);
        let b_axis = normalize_or(cross(tangent, n_axis), [0.0, 0.0, 1.0]);
        let mut centre = centre0;
        if station.flare {
            centre[1] += FINGER_FLARE_SHIFT;
        }
        let radius = base_radius * station.radius_factor * radial_scale;
        for seg in 0..FINGER_RADIAL {
            let angle = (seg as f32 / FINGER_RADIAL as f32) * std::f32::consts::TAU;
            let local = [
                n_axis[0] * angle.cos() + b_axis[0] * angle.sin(),
                n_axis[1] * angle.cos() + b_axis[1] * angle.sin(),
                n_axis[2] * angle.cos() + b_axis[2] * angle.sin(),
            ];
            let mut offset = [local[0] * radius, local[1] * radius, local[2] * radius];
            if station.flare && offset[1] < 0.0 {
                offset[1] *= FINGER_FLARE;
            }
            let pos = [
                centre[0] + offset[0],
                centre[1] + offset[1],
                centre[2] + offset[2],
            ];
            let uv = [
                (seg as f32 / FINGER_RADIAL as f32) * 0.25,
                0.44 + (ring as f32 / (ring_count - 1) as f32) * 0.06,
            ];
            vertices.push(weighted_vertex(pos, uv, station.influences));
        }
    }
    for ring in 0..ring_count - 1 {
        for seg in 0..FINGER_RADIAL {
            let next = (seg + 1) % FINGER_RADIAL;
            let a = base + (ring * FINGER_RADIAL + seg) as u32;
            let b = base + (ring * FINGER_RADIAL + next) as u32;
            let c = base + ((ring + 1) * FINGER_RADIAL + seg) as u32;
            let d = base + ((ring + 1) * FINGER_RADIAL + next) as u32;
            indices.extend_from_slice(&[a, c, b, b, c, d]);
        }
    }
    // Root cap (inside the palm; kept so the part stays watertight).
    let root_center = vertices.len() as u32;
    vertices.push(weighted_vertex(
        stations[0].centre,
        [0.125, 0.44],
        stations[0].influences,
    ));
    for seg in 0..FINGER_RADIAL {
        let next = (seg + 1) % FINGER_RADIAL;
        indices.extend_from_slice(&[root_center, base + next as u32, base + seg as u32]);
    }
    // Fingertip cap.
    let tip = [
        end[0] + dir_c[0] * FINGERTIP_EXT,
        end[1] + dir_c[1] * FINGERTIP_EXT,
        end[2] + dir_c[2] * FINGERTIP_EXT,
    ];
    let tip_center = vertices.len() as u32;
    vertices.push(weighted_vertex(tip, [0.125, 0.50], &w_j3));
    let last_ring = base + ((ring_count - 1) * FINGER_RADIAL) as u32;
    for seg in 0..FINGER_RADIAL {
        let next = (seg + 1) % FINGER_RADIAL;
        indices.extend_from_slice(&[tip_center, last_ring + seg as u32, last_ring + next as u32]);
    }
}

/// One hand (D-1290): a palm loft with a scalloped front edge and
/// five finger tubes, replacing the arm tube's mitten segment.
/// The palm side faces -Y (palm down in the arm-extended rest
/// pose; the joint table's thumb at +Z is then anatomically
/// forward, and fingers curl toward -Y, the flare's wide side).
fn build_hand(prefix: &str, name: &str, joints: &[Joint], radial_scale: f32) -> MeshPart {
    let mut vertices: Vec<Vertex> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    let hand_name = format!("{prefix}Hand");
    let hand_pos = joint_pos(joints, &hand_name);
    let hand_id = joint_index(joints, &hand_name).map_or(0, |idx| idx as u16);
    let axis = normalize_or(
        sub(joint_pos(joints, &format!("{prefix}HandMiddle1")), hand_pos),
        [1.0, 0.0, 0.0],
    );
    let helper = if axis[1].abs() > 0.9 {
        [1.0, 0.0, 0.0]
    } else {
        [0.0, 1.0, 0.0]
    };
    let n_axis = normalize_or(cross(helper, axis), [1.0, 0.0, 0.0]);
    let b_axis = normalize_or(cross(axis, n_axis), [0.0, 0.0, 1.0]);
    // (base Z, joint id) per finger's first joint, for the palm's
    // distal blend toward the fingers (the cast starts each first
    // joint's influence inside the palm).
    let finger_bases: Vec<(f32, u16)> = FINGERS
        .iter()
        .map(|(finger, _)| {
            let name = format!("{prefix}Hand{finger}1");
            (
                joint_pos(joints, &name)[2],
                joint_index(joints, &name).map_or(0, |idx| idx as u16),
            )
        })
        .collect();
    let nearest_finger = |z: f32| -> u16 {
        finger_bases
            .iter()
            .min_by(|a, b| (z - a.0).abs().total_cmp(&(z - b.0).abs()))
            .map_or(hand_id, |&(_, idx)| idx)
    };
    // Palm loft: the stations plus the scalloped front ring.
    let ring_count = PALM_STATIONS.len() + 1;
    for ring in 0..ring_count {
        let front = ring == ring_count - 1;
        let (dx0, half_w, half_t, off_y, off_z) = PALM_STATIONS[ring.min(PALM_STATIONS.len() - 1)];
        let centre = [
            hand_pos[0] + axis[0] * dx0,
            hand_pos[1] + axis[1] * dx0 + off_y,
            hand_pos[2] + axis[2] * dx0 + off_z,
        ];
        for seg in 0..PALM_SEGMENTS {
            let angle = (seg as f32 / PALM_SEGMENTS as f32) * std::f32::consts::TAU;
            let shape = |t: f32| t.signum() * t.abs().powf(2.0 / PALM_SUPERELLIPSE_EXP);
            let off_n = shape(angle.cos()) * half_w * radial_scale;
            let off_b = shape(angle.sin()) * half_t * radial_scale;
            let offset = [
                n_axis[0] * off_n + b_axis[0] * off_b,
                n_axis[1] * off_n + b_axis[1] * off_b,
                n_axis[2] * off_n + b_axis[2] * off_b,
            ];
            let mut pos = [
                centre[0] + offset[0],
                centre[1] + offset[1],
                centre[2] + offset[2],
            ];
            if front {
                let dx = palm_front_dx(pos[2]);
                pos = [
                    pos[0] + axis[0] * (dx - dx0),
                    pos[1] + axis[1] * (dx - dx0),
                    pos[2] + axis[2] * (dx - dx0),
                ];
            }
            let w_hand: [(u16, f32); 1] = [(hand_id, 1.0)];
            let w_blend85: [(u16, f32); 2] = [(hand_id, 0.85), (nearest_finger(pos[2]), 0.15)];
            let w_blend70: [(u16, f32); 2] = [(hand_id, 0.70), (nearest_finger(pos[2]), 0.30)];
            let influences: &[(u16, f32)] = if front {
                &w_blend70
            } else if ring == PALM_STATIONS.len() - 1 {
                &w_blend85
            } else {
                &w_hand
            };
            let uv = [
                ((pos[2] + 0.055) / 0.113 * 0.25).clamp(0.0, 0.25),
                0.44 + (ring as f32 / (ring_count - 1) as f32) * 0.06,
            ];
            vertices.push(weighted_vertex(pos, uv, influences));
        }
    }
    for ring in 0..ring_count - 1 {
        for seg in 0..PALM_SEGMENTS {
            let next = (seg + 1) % PALM_SEGMENTS;
            let a = (ring * PALM_SEGMENTS + seg) as u32;
            let b = (ring * PALM_SEGMENTS + next) as u32;
            let c = ((ring + 1) * PALM_SEGMENTS + seg) as u32;
            let d = ((ring + 1) * PALM_SEGMENTS + next) as u32;
            indices.extend_from_slice(&[a, c, b, b, c, d]);
        }
    }
    // Wrist cap (inside the arm tube's end; watertightness).
    let wrist_center = vertices.len() as u32;
    vertices.push(weighted_vertex(hand_pos, [0.125, 0.44], &[(hand_id, 1.0)]));
    for seg in 0..PALM_SEGMENTS {
        let next = (seg + 1) % PALM_SEGMENTS;
        indices.extend_from_slice(&[wrist_center, next as u32, seg as u32]);
    }
    // Fingers: one tube design per finger, from the joint table.
    for (finger, base_radius) in FINGERS {
        let chain = [
            joint_pos(joints, &format!("{prefix}Hand{finger}1")),
            joint_pos(joints, &format!("{prefix}Hand{finger}2")),
            joint_pos(joints, &format!("{prefix}Hand{finger}3")),
            joint_pos(joints, &format!("{prefix}Hand{finger}_End")),
        ];
        let joint_ids = [
            hand_id,
            joint_index(joints, &format!("{prefix}Hand{finger}1")).map_or(0, |idx| idx as u16),
            joint_index(joints, &format!("{prefix}Hand{finger}2")).map_or(0, |idx| idx as u16),
            joint_index(joints, &format!("{prefix}Hand{finger}3")).map_or(0, |idx| idx as u16),
        ];
        build_finger(
            &mut vertices,
            &mut indices,
            chain,
            joint_ids,
            base_radius,
            radial_scale,
        );
    }
    MeshPart {
        indices,
        name: name.to_string(),
        vertices,
    }
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
    /// Per-band depth (Z) scales `(head, torso, leg)` applied to
    /// profile ring depths and the head's Z radius. Exactly 1.0 per
    /// band (the default) leaves every depth bit-identical.
    pub band_depth_scales: [f32; 3],
    /// Per-band width (X) scales `(head, torso, leg)` applied to
    /// profile ring widths, limb tube radii, and the head's X
    /// radius. Exactly 1.0 per band (the default) leaves every
    /// width bit-identical.
    pub band_width_scales: [f32; 3],
    /// Body lathe height span `(y0, y1)`.
    pub body_y: (f32, f32),
    /// Optional bust delta (bust − underbust circumference) in
    /// centimetres, shaped into the profile by [`shape_bust`].
    /// `None` leaves the profile untouched, reproducing the
    /// canonical base byte-for-byte.
    pub bust_delta_cm: Option<f32>,
    /// Head ellipsoid center (world space).
    pub head_center: [f32; 3],
    /// Head ellipsoid radii.
    pub head_radii: [f32; 3],
    /// Hair length in `0.0..=1.0`: how far the hair tail reaches
    /// below the `Neck` joint, as a fraction of the way to the `Hips`
    /// joint; 0.0 builds no tail.
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
            band_depth_scales: [1.0, 1.0, 1.0],
            band_width_scales: [1.0, 1.0, 1.0],
            body_y: (0.92, 1.50),
            bust_delta_cm: None,
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

/// The canonical joint table with Y positions remapped by band
/// (as [`scaled_joint_table`]) and X positions scaled per band by
/// `width_scales` `(head, torso, leg)`: a joint's band is read off
/// its canonical height, so the shoulder span takes the torso
/// width scale, the hip span the leg width scale, and the head
/// joint the head width scale. Z positions are anatomical offsets
/// (toe and finger advances), not widths, and are unchanged — the
/// depth axis is carried by the profile rings and the head radii.
/// With all width scales at exactly 1.0 the result equals
/// [`scaled_joint_table`] bit-for-bit.
pub fn scaled_joint_table_with_frame(
    head_scale: f32,
    torso_scale: f32,
    leg_scale: f32,
    width_scales: (f32, f32, f32),
) -> Vec<Joint> {
    let canonical = joint_table();
    let hips_y = canonical
        .iter()
        .find(|joint| joint.name == "Hips")
        .map(|joint| joint.position[1])
        .unwrap_or(0.0);
    let neck_y = canonical
        .iter()
        .find(|joint| joint.name == "Neck")
        .map(|joint| joint.position[1])
        .unwrap_or(0.0);
    let widths = [width_scales.0, width_scales.1, width_scales.2];
    let mut joints = scaled_joint_table(head_scale, torso_scale, leg_scale);
    // scaled_joint_table never touches X, so each joint's X is its
    // canonical X; scale it by the width of the joint's band.
    for (joint, canon) in joints.iter_mut().zip(canonical.iter()) {
        let band = band_of_y(canon.position[1], hips_y, neck_y);
        joint.position[0] = canon.position[0] * widths[band];
    }
    joints
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
    for (axis, scales) in [
        ("width", params.band_width_scales),
        ("depth", params.band_depth_scales),
    ] {
        if !scales.iter().all(|s| s.is_finite() && *s > 0.0) {
            return Err(FactoryError::BadParam(format!(
                "band {axis} scales must be finite and positive: {scales:?}"
            )));
        }
    }
    validate_profile(&params.profile)?;
    let joints = params.joints.clone();
    validate_joints(&joints)?;
    let radial = params.radial_scale;
    // The effective profile: per-band width/depth scales first (so
    // the bust delta is measured against the character's actual
    // underbust), then bust shaping when the parameter is present.
    // With scales at 1.0 and no bust delta this is the input
    // profile bit-for-bit.
    let banded = band_scaled_profile(
        &params.profile,
        &joints,
        params.band_width_scales,
        params.band_depth_scales,
    );
    let shaped;
    let profile: &[(f32, f32, f32)] = match params.bust_delta_cm {
        Some(delta) => {
            shaped = shape_bust(&banded, delta, params.body_y)?;
            &shaped
        }
        None => &banded,
    };
    // The effective head radii: the head band's width/depth scales
    // apply to the ellipsoid the head, hair shell, and hair tail
    // are built from.
    let head_radii = [
        params.head_radii[0] * params.band_width_scales[0],
        params.head_radii[1],
        params.head_radii[2] * params.band_depth_scales[0],
    ];

    let mut parts: Vec<MeshPart> = Vec::new();

    // Arms (upper arm .045->.038, forearm .038->.030). The tube
    // ends at the wrist; the hand is its own part (D-1290 replaced
    // the tube's flattened mitten segment with `build_hand`).
    // The arm tubes take the torso band's width scale.
    let arm_radial = radial * params.band_width_scales[1];
    for (side_name, prefix) in [("left", "Left"), ("right", "Right")] {
        let chain = [
            joint_pos(&joints, &format!("{prefix}Arm")),
            joint_pos(&joints, &format!("{prefix}ForeArm")),
            joint_pos(&joints, &format!("{prefix}Hand")),
        ];
        parts.push(finish_part(build_tube(
            &format!("arm_{side_name}"),
            &chain,
            &[0.045 * arm_radial, 0.038 * arm_radial, 0.030 * arm_radial],
            10,
            false,
            (0.0, 0.25, 0.25, 0.50),
            &joints,
        ))?);
    }

    // Hands (palm loft + five finger tubes per side, D-1290).
    for (side_name, prefix) in [("left", "Left"), ("right", "Right")] {
        parts.push(finish_part(build_hand(
            prefix,
            &format!("hand_{side_name}"),
            &joints,
            arm_radial,
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

    // Hair shell (cap plus tail, one part) and head.
    let (mut hair_part, _) = build_head(
        "hair",
        true,
        &joints,
        params.head_center,
        head_radii,
        params.jaw_y,
        (HEAD_LAT_STEPS, HEAD_LON_STEPS),
    );
    if let Some(tail) = build_hair_tail(params, &joints, profile, head_radii) {
        let base_idx = hair_part.vertices.len() as u32;
        hair_part.vertices.extend(tail.vertices);
        hair_part
            .indices
            .extend(tail.indices.iter().map(|i| i + base_idx));
    }
    parts.push(finish_part(hair_part)?);
    let (head_part, head_masks) = build_head(
        "head",
        false,
        &joints,
        params.head_center,
        head_radii,
        params.jaw_y,
        (HEAD_FINE_LAT_STEPS, HEAD_FINE_LON_STEPS),
    );
    let head_part = finish_part(head_part)?;

    // Legs (thigh .078->.056, shin .054->.036, foot toe box).
    // The leg tubes take the leg band's width scale.
    let leg_radial = radial * params.band_width_scales[2];
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
                0.078 * leg_radial,
                0.054 * leg_radial,
                0.036 * leg_radial,
                0.030 * leg_radial,
                0.018 * leg_radial,
            ],
            12,
            true,
            (0.0, 0.25, 0.25, 0.50),
            &joints,
        ))?);
    }

    // Outfit leg shells (both legs merged into one part, tubes x OUTFIT_LEGS_SCALE).
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
                    0.078 * OUTFIT_LEGS_SCALE * leg_radial,
                    0.054 * OUTFIT_LEGS_SCALE * leg_radial,
                    0.036 * OUTFIT_LEGS_SCALE * leg_radial,
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

    // Outfit torso shell (lathe x OUTFIT_TORSO_SCALE, y 0.97..1.47).
    parts.push(finish_part(build_lathe(
        "outfit_torso",
        profile,
        params.outfit_y.0,
        params.outfit_y.1,
        16,
        16,
        OUTFIT_TORSO_SCALE * radial,
        (0.50, 0.25, 0.75, 0.50),
        &joints,
    ))?);

    parts.push(head_part);

    // Eyeballs (rigid spheres on the eye joints, D-1290).
    parts.push(finish_part(build_eyeball(
        "eye_left",
        "LeftEye",
        &joints,
        params.head_center,
        head_radii,
    ))?);
    parts.push(finish_part(build_eyeball(
        "eye_right",
        "RightEye",
        &joints,
        params.head_center,
        head_radii,
    ))?);

    Ok(BaseMesh {
        head_masks,
        joints,
        parts,
    })
}
