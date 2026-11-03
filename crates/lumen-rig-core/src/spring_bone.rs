//! VRMC_springBone evaluation: the spring-chain procedural
//! animation of the VRM 1.0 specification (the glTF extension
//! `VRMC_springBone`, vrm-c/vrm-specification), implemented
//! from that specification's text.
//!
//! This module is a separate evaluation surface, not the
//! format's section 5 strand physics: section 5 is a 2D,
//! in-plane strand model defined by this crate's own SPEC.md,
//! while this module evaluates the 3D, world-space spring
//! chains of the VRM specification over a host-supplied node
//! hierarchy. The two share this crate's determinism law
//! (SPEC.md section 7): evaluation is a pure function of the
//! definition, the input frames, and the step sequence — no
//! wall-clock, no map iteration, no platform intrinsics. All
//! arithmetic is f64.
//!
//! Host contract. The host owns the node hierarchy and its
//! animation. For each spring chain the host supplies, per
//! step, one [`Frame`] for the parent node of the chain's
//! first joint (the *anchor*) and one [`Frame`] per collider
//! node. The evaluator walks each chain root-first (the
//! specification's calculation order), integrating one verlet
//! tail point per joint pair, and produces the local rotation
//! of every joint whose rotation the spring drives — every
//! joint except each chain's last, which is a tail only. A
//! joint's local rotation is its rest local rotation composed
//! with the spring swing, exactly as the specification's
//! applying-rotation step prescribes; animation of a spring
//! joint itself is therefore the host's rest pose, and
//! animation reaches the chain through the anchor.
//!
//! Choices recorded where the specification leaves room:
//! - The inertia step follows the reference pseudocode
//!   literally: the stiffness term is
//!   `dt * stiffness * (parentWorldRotation *
//!   initialLocalRotation * boneAxis)` — a displacement along
//!   the rest tail direction, *not* scaled by bone length.
//! - The applying-rotation step is computed in its
//!   rotation-only form (directions relative to the head's
//!   world position), which is the matrix form of the
//!   pseudocode reduced to the direction it normalizes.
//! - The capsule listing in the specification references
//!   `delta` before its assignment line; it is implemented in
//!   the only consistent order (delta computed first), which
//!   matches the behaviour of the reference implementations.
//! - `center` (center space) is not supported: a definition
//!   that sets it fails at construction with
//!   [`SpringError::CenterUnsupported`] rather than being
//!   silently evaluated in world space.
//! - The collision radius of a pair is the *head* joint's
//!   `hit_radius` (the joint that carries the pair's
//!   settings).
//! - Anchor and collider frames are held across the substeps
//!   of one [`SpringSystem::advance`] call: inputs are sampled
//!   at call boundaries. Hosts that need per-step input
//!   sampling drive [`SpringSystem::step`] directly.
//! - Step counting in fixed-step mode uses a remainder
//!   accumulator (advance by whole steps, carry the rest),
//!   which equals `floor(elapsed / dt)` in exact arithmetic
//!   (SPEC.md section 7 item 5) while keeping floating-point
//!   accumulation error bounded by one step instead of
//!   growing with elapsed time.
//!
//! Bounds: chain, joint, and collider counts are capped by
//! the `*_MAX` constants in this module; definitions beyond
//! them fail at construction before any state is built.

use std::fmt;

/// Maximum number of spring chains in one system.
pub const SPRING_CHAINS_MAX: usize = 64;
/// Maximum number of joints in one spring chain.
pub const SPRING_JOINTS_PER_CHAIN_MAX: usize = 64;
/// Maximum number of colliders in one system.
pub const SPRING_COLLIDERS_MAX: usize = 128;
/// Maximum number of collider groups in one system.
pub const SPRING_COLLIDER_GROUPS_MAX: usize = 64;
/// Maximum number of colliders in one collider group.
pub const SPRING_COLLIDERS_PER_GROUP_MAX: usize = 128;
/// A rest bone at or below this length, in world units, is
/// degenerate: its direction, and so its spring, is undefined.
const BONE_LENGTH_MIN: f64 = 1e-9;
/// The fixed step of the estate convention (SPEC.md section
/// 5.1): conforming runtimes step physics at 1/120 s unless
/// the host declares 1/30 or 1/60.
pub const STEP_DT_DEFAULT_SECONDS: f64 = 1.0 / 120.0;

/// A 3D vector in f64. World-space unless a field says
/// otherwise; units are the host model's world units (meters
/// in VRM assets).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Vec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Vec3 {
    pub const ZERO: Vec3 = Vec3 {
        x: 0.0,
        y: 0.0,
        z: 0.0,
    };

    pub fn new(x: f64, y: f64, z: f64) -> Vec3 {
        Vec3 { x, y, z }
    }

    fn add(self, other: Vec3) -> Vec3 {
        Vec3::new(self.x + other.x, self.y + other.y, self.z + other.z)
    }

    fn sub(self, other: Vec3) -> Vec3 {
        Vec3::new(self.x - other.x, self.y - other.y, self.z - other.z)
    }

    fn scale(self, factor: f64) -> Vec3 {
        Vec3::new(self.x * factor, self.y * factor, self.z * factor)
    }

    fn dot(self, other: Vec3) -> f64 {
        self.x * other.x + self.y * other.y + self.z * other.z
    }

    fn cross(self, other: Vec3) -> Vec3 {
        Vec3::new(
            self.y * other.z - self.z * other.y,
            self.z * other.x - self.x * other.z,
            self.x * other.y - self.y * other.x,
        )
    }

    fn length(self) -> f64 {
        self.dot(self).sqrt()
    }

    /// The unit vector in this vector's direction, or
    /// `fallback` when the length is zero or non-finite.
    fn normalized_or(self, fallback: Vec3) -> Vec3 {
        let length = self.length();
        if length > 0.0 && length.is_finite() {
            self.scale(1.0 / length)
        } else {
            fallback
        }
    }

    fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }
}

/// A unit quaternion in f64, `(x, y, z, w)`. Composition is
/// the Hamilton product with the crate's convention:
/// `(a * b)` applies `b` first, then `a`, so a node's world
/// rotation is `parent_world * local`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quat {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub w: f64,
}

impl Quat {
    pub const IDENTITY: Quat = Quat {
        x: 0.0,
        y: 0.0,
        z: 0.0,
        w: 1.0,
    };

    pub fn new(x: f64, y: f64, z: f64, w: f64) -> Quat {
        Quat { x, y, z, w }
    }

    /// The Hamilton product `self * other`.
    fn mul(self, other: Quat) -> Quat {
        Quat::new(
            self.w * other.x + self.x * other.w + self.y * other.z - self.z * other.y,
            self.w * other.y - self.x * other.z + self.y * other.w + self.z * other.x,
            self.w * other.z + self.x * other.y - self.y * other.x + self.z * other.w,
            self.w * other.w - self.x * other.x - self.y * other.y - self.z * other.z,
        )
    }

    /// The inverse of a unit quaternion (its conjugate).
    fn inverse(self) -> Quat {
        Quat::new(-self.x, -self.y, -self.z, self.w)
    }

    fn normalized(self) -> Quat {
        let length = (self.x * self.x + self.y * self.y + self.z * self.z + self.w * self.w).sqrt();
        if length > 0.0 && length.is_finite() {
            let inv = 1.0 / length;
            Quat::new(self.x * inv, self.y * inv, self.z * inv, self.w * inv)
        } else {
            Quat::IDENTITY
        }
    }

    /// Rotate `vec` by this quaternion.
    pub fn rotate(self, vec: Vec3) -> Vec3 {
        let q = Vec3::new(self.x, self.y, self.z);
        let uv = q.cross(vec);
        let uuv = q.cross(uv);
        vec.add(uv.scale(2.0 * self.w)).add(uuv.scale(2.0))
    }

    /// The shortest-arc rotation taking unit vector `from`
    /// to unit vector `to`. The degenerate opposite case
    /// picks a deterministic orthogonal axis (the coordinate
    /// axis least aligned with `from`, in x, y, z order), so
    /// the result is a pure function of the inputs.
    fn from_to(from: Vec3, to: Vec3) -> Quat {
        let dot = from.dot(to);
        if dot > 1.0 - 1e-12 {
            return Quat::IDENTITY;
        }
        if dot < -1.0 + 1e-12 {
            let axis_hint = if from.x.abs() <= from.y.abs() && from.x.abs() <= from.z.abs() {
                Vec3::new(1.0, 0.0, 0.0)
            } else if from.y.abs() <= from.z.abs() {
                Vec3::new(0.0, 1.0, 0.0)
            } else {
                Vec3::new(0.0, 0.0, 1.0)
            };
            let axis = from
                .cross(axis_hint)
                .normalized_or(Vec3::new(1.0, 0.0, 0.0));
            return Quat::new(axis.x, axis.y, axis.z, 0.0);
        }
        let cross = from.cross(to);
        Quat::new(cross.x, cross.y, cross.z, 1.0 + dot).normalized()
    }

    fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite() && self.w.is_finite()
    }
}

/// A node transform in world space: position and rotation.
/// Scale is not part of the spring evaluation; hosts bake it
/// into the frames they supply.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    pub position: Vec3,
    pub rotation: Quat,
}

impl Frame {
    pub fn new(position: Vec3, rotation: Quat) -> Frame {
        Frame { position, rotation }
    }

    /// The transform at the origin with identity rotation.
    pub const IDENTITY: Frame = Frame {
        position: Vec3::ZERO,
        rotation: Quat::IDENTITY,
    };

    fn assert_finite(&self, role: &str) {
        assert!(
            self.position.is_finite() && self.rotation.is_finite(),
            "{role} frame must be finite, got position {:?}, rotation {:?}",
            self.position,
            self.rotation
        );
    }
}

/// The spring parameters of one joint, mirroring
/// `VRMC_springBone.springs[*].joints[*]`. The parameters of
/// a chain's last joint are unused (it is a tail only).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JointParams {
    /// Collision radius of this joint's tail point, in world
    /// units (the specification's `hitRadius`).
    pub hit_radius: f64,
    /// Rigidity: the force returning the tail toward its
    /// rest direction (the specification's `stiffness`).
    /// Must be finite and at least 0.
    pub stiffness: f64,
    /// Gravity strength (the specification's
    /// `gravityPower`). Must be finite and at least 0.
    pub gravity_power: f64,
    /// Gravity direction (the specification's `gravityDir`);
    /// normalized once at construction. Must be finite and
    /// nonzero.
    pub gravity_dir: Vec3,
    /// Deceleration in `[0, 1]` (the specification's
    /// `dragForce`): the fraction of tail velocity shed per
    /// step.
    pub drag_force: f64,
}

impl Default for JointParams {
    /// The specification's example values.
    fn default() -> JointParams {
        JointParams {
            hit_radius: 0.0,
            stiffness: 1.0,
            gravity_power: 0.0,
            gravity_dir: Vec3::new(0.0, -1.0, 0.0),
            drag_force: 0.5,
        }
    }
}

/// One joint of a spring chain: its node identity, its rest
/// local transform within the chain, and its parameters.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JointDef {
    /// The host's node index for this joint (the glTF node
    /// index in a VRM asset). Carried so hosts can map
    /// outputs back to their hierarchy.
    pub node: usize,
    pub params: JointParams,
    /// Rest position of this joint in its parent's space.
    /// For the first joint the parent is the anchor node;
    /// for every later joint the parent is the previous
    /// joint (chains are consecutive parent-child runs, as
    /// the specification requires).
    pub rest_local_translation: Vec3,
    /// Rest orientation of this joint in its parent's space
    /// (the specification's `initialLocalRotation`). Expected
    /// to be a unit quaternion; normalized once at
    /// construction.
    pub rest_local_rotation: Quat,
}

/// The shape of one collider, in the collider node's local
/// space, mirroring `VRMC_springBone.colliders[*].shape`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ColliderShape {
    Sphere {
        offset: Vec3,
        radius: f64,
    },
    Capsule {
        offset: Vec3,
        radius: f64,
        tail: Vec3,
    },
}

/// One collider: its node identity and its shape.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ColliderDef {
    /// The host's node index for this collider.
    pub node: usize,
    pub shape: ColliderShape,
}

/// One spring chain: a consecutive run of joints, the
/// collider groups it collides against (indices into the
/// system's collider groups), and its optional center node.
#[derive(Debug, Clone, PartialEq)]
pub struct SpringChainDef {
    pub joints: Vec<JointDef>,
    /// Indices into `SpringSystemDef::collider_groups`.
    pub collider_groups: Vec<usize>,
    /// The center-space node of the specification, if any.
    /// Center space is unsupported by this evaluator: a
    /// definition that sets it fails at construction.
    pub center_node: Option<usize>,
}

/// A complete spring system definition, mirroring the
/// `VRMC_springBone` extension's structure.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SpringSystemDef {
    pub colliders: Vec<ColliderDef>,
    /// Groups of collider indices, as in the extension's
    /// `colliderGroups`.
    pub collider_groups: Vec<Vec<usize>>,
    pub springs: Vec<SpringChainDef>,
}

/// How [`SpringSystem::advance`] turns host frame times
/// into evaluation steps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeMode {
    /// One step per `advance` call, with the caller's frame
    /// `dt` — the reference implementations' per-frame
    /// update, and therefore a function of frame cadence.
    PerFrame,
    /// Whole steps of a fixed `dt`, counted from accumulated
    /// frame time — the estate's fixed-step law (SPEC.md
    /// section 7): the step sequence is a function of
    /// simulation time, not of frame delivery.
    FixedStep,
}

/// The time configuration of a [`SpringSystem`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpringConfig {
    pub mode: TimeMode,
    /// The fixed step in seconds; used in
    /// [`TimeMode::FixedStep`] only. Must be one of the
    /// estate convention's values (1/30, 1/60, or the default
    /// 1/120 — SPEC.md section 5.1).
    pub step_dt_seconds: f64,
}

impl Default for SpringConfig {
    fn default() -> SpringConfig {
        SpringConfig {
            mode: TimeMode::FixedStep,
            step_dt_seconds: STEP_DT_DEFAULT_SECONDS,
        }
    }
}

/// A spring definition or configuration failure. Returned
/// before any state is built; a failed construction leaves
/// nothing behind.
#[derive(Debug, Clone, PartialEq)]
pub enum SpringError {
    /// The system declares no spring chains.
    EmptySystem,
    /// More chains than `SPRING_CHAINS_MAX`.
    TooManyChains { count: usize, max: usize },
    /// A chain declares no joints (a chain needs at least a
    /// head and a tail).
    EmptyChain { chain: usize },
    /// A chain declares a single joint; a pair needs two.
    ChainTooShort { chain: usize, count: usize },
    /// More joints in one chain than
    /// `SPRING_JOINTS_PER_CHAIN_MAX`.
    ChainTooLong {
        chain: usize,
        count: usize,
        max: usize,
    },
    /// More colliders than `SPRING_COLLIDERS_MAX`.
    TooManyColliders { count: usize, max: usize },
    /// More collider groups than
    /// `SPRING_COLLIDER_GROUPS_MAX`.
    TooManyColliderGroups { count: usize, max: usize },
    /// More colliders in one group than
    /// `SPRING_COLLIDERS_PER_GROUP_MAX`.
    ColliderGroupTooLong {
        group: usize,
        count: usize,
        max: usize,
    },
    /// A group names a collider index the system does not
    /// declare.
    UnknownCollider { group: usize, collider: usize },
    /// A chain names a collider group index the system does
    /// not declare.
    UnknownColliderGroup { chain: usize, group: usize },
    /// A chain sets a center node; center space is not
    /// supported by this evaluator.
    CenterUnsupported { chain: usize },
    /// A joint parameter is non-finite, negative where the
    /// specification requires at least 0, a drag force
    /// outside `[0, 1]`, or a zero gravity direction.
    InvalidJointParam {
        chain: usize,
        joint: usize,
        field: &'static str,
    },
    /// A joint's rest transform is non-finite.
    InvalidJointTransform { chain: usize, joint: usize },
    /// A collider shape parameter is non-finite or negative.
    InvalidCollider {
        collider: usize,
        field: &'static str,
    },
    /// A pair's rest bone length is at or below
    /// `BONE_LENGTH_MIN`: its direction is undefined.
    DegenerateBone { chain: usize, joint: usize },
    /// The fixed step is not one of the convention's values.
    InvalidStepDt { found: f64 },
}

impl fmt::Display for SpringError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SpringError::EmptySystem => write!(f, "spring system declares no chains"),
            SpringError::TooManyChains { count, max } => {
                write!(f, "spring chains {count} exceeds max {max}")
            }
            SpringError::EmptyChain { chain } => write!(f, "spring chain {chain} is empty"),
            SpringError::ChainTooShort { chain, count } => {
                write!(
                    f,
                    "spring chain {chain} has {count} joint(s), needs at least 2"
                )
            }
            SpringError::ChainTooLong { chain, count, max } => {
                write!(
                    f,
                    "spring chain {chain} has {count} joints, exceeds max {max}"
                )
            }
            SpringError::TooManyColliders { count, max } => {
                write!(f, "colliders {count} exceeds max {max}")
            }
            SpringError::TooManyColliderGroups { count, max } => {
                write!(f, "collider groups {count} exceeds max {max}")
            }
            SpringError::ColliderGroupTooLong { group, count, max } => write!(
                f,
                "collider group {group} has {count} colliders, exceeds max {max}"
            ),
            SpringError::UnknownCollider { group, collider } => {
                write!(
                    f,
                    "collider group {group} names unknown collider {collider}"
                )
            }
            SpringError::UnknownColliderGroup { chain, group } => {
                write!(
                    f,
                    "spring chain {chain} names unknown collider group {group}"
                )
            }
            SpringError::CenterUnsupported { chain } => {
                write!(
                    f,
                    "spring chain {chain} sets a center node; center space is unsupported"
                )
            }
            SpringError::InvalidJointParam {
                chain,
                joint,
                field,
            } => write!(f, "spring chain {chain} joint {joint}: invalid {field}"),
            SpringError::InvalidJointTransform { chain, joint } => {
                write!(
                    f,
                    "spring chain {chain} joint {joint}: non-finite rest transform"
                )
            }
            SpringError::InvalidCollider { collider, field } => {
                write!(f, "collider {collider}: invalid {field}")
            }
            SpringError::DegenerateBone { chain, joint } => {
                write!(
                    f,
                    "spring chain {chain} joint {joint}: degenerate rest bone"
                )
            }
            SpringError::InvalidStepDt { found } => {
                write!(f, "fixed step {found} is not 1/30, 1/60, or 1/120 seconds")
            }
        }
    }
}

impl std::error::Error for SpringError {}

/// Per-pair runtime state, mirroring the specification's
/// `SpringBoneJointState` (the matrices are implicit in the
/// definition's rest transforms).
#[derive(Debug, Clone, Copy)]
struct PairState {
    prev_tail: Vec3,
    current_tail: Vec3,
    bone_axis: Vec3,
    bone_length: f64,
}

/// One chain's runtime: validated joints, resolved
/// colliders, pair state, and the local-rotation outputs.
/// All buffers are built at construction; stepping never
/// allocates.
#[derive(Debug)]
struct SpringChain {
    joints: Vec<JointDef>,
    /// Flattened collider indices, in declared group order,
    /// groups in the chain's declared order, duplicates kept
    /// (the specification's collision loop runs per listed
    /// collider).
    colliders: Vec<usize>,
    pairs: Vec<PairState>,
    /// Local rotation per joint; the last entry is always
    /// the rest rotation (the tail joint is never rotated).
    rotations: Vec<Quat>,
}

/// A running spring system: validated chains plus the time
/// state of the fixed-step accumulator.
///
/// Contract: the host calls [`SpringSystem::advance`] (or
/// [`SpringSystem::step`] directly) with the anchor and
/// collider frames for the step, then reads each chain's
/// local rotations and writes them into its hierarchy. A
/// recorded sequence of calls replays bit-identically on
/// any conforming machine: all arithmetic is f64, in
/// declaration order, with fixed iteration counts.
#[derive(Debug)]
pub struct SpringSystem {
    chains: Vec<SpringChain>,
    colliders: Vec<ColliderDef>,
    config: SpringConfig,
    /// Frame time not yet consumed by whole steps
    /// (fixed-step mode only).
    pending_seconds: f64,
    step_count: u64,
    /// Sum of the `dt` of every step taken, in seconds.
    stepped_seconds: f64,
}

impl SpringSystem {
    /// Build a system with the default configuration
    /// (fixed-step at 1/120 s) from `def`, initialized at
    /// rest under `rest_anchor` (the anchor frame in the
    /// rest pose). All pair state starts at the rest tail
    /// positions with zero velocity.
    pub fn new(def: SpringSystemDef, rest_anchor: &Frame) -> Result<SpringSystem, SpringError> {
        SpringSystem::with_config(def, rest_anchor, SpringConfig::default())
    }

    /// Build a system with an explicit time configuration.
    pub fn with_config(
        def: SpringSystemDef,
        rest_anchor: &Frame,
        config: SpringConfig,
    ) -> Result<SpringSystem, SpringError> {
        if config.mode == TimeMode::FixedStep && !is_convention_step_dt(config.step_dt_seconds) {
            return Err(SpringError::InvalidStepDt {
                found: config.step_dt_seconds,
            });
        }
        rest_anchor.assert_finite("rest anchor");
        validate_system(&def)?;
        let chains = def
            .springs
            .iter()
            .enumerate()
            .map(|(index, chain_def)| build_chain(index, chain_def, &def, rest_anchor))
            .collect::<Result<Vec<SpringChain>, SpringError>>()?;
        Ok(SpringSystem {
            chains,
            colliders: def.colliders,
            config,
            pending_seconds: 0.0,
            step_count: 0,
            stepped_seconds: 0.0,
        })
    }

    /// The number of chains in the system.
    pub fn chain_count(&self) -> usize {
        self.chains.len()
    }

    /// The number of joints in `chain` (0 for an unknown
    /// chain index).
    pub fn joint_count(&self, chain: usize) -> usize {
        self.chains.get(chain).map_or(0, |chain| chain.joints.len())
    }

    /// The local rotations of `chain`'s joints from the
    /// latest step, in joint order. The last entry is the
    /// rest rotation (tails are never rotated). Empty for an
    /// unknown chain index.
    pub fn local_rotations(&self, chain: usize) -> &[Quat] {
        self.chains.get(chain).map_or(&[], |chain| &chain.rotations)
    }

    /// The current tail position of every pair in `chain`,
    /// in pair order, written into `out` (cleared first).
    /// Exposed for hosts that render or record the spring
    /// state, and for conformance harnesses.
    pub fn current_tails(&self, chain: usize, out: &mut Vec<Vec3>) {
        out.clear();
        if let Some(chain) = self.chains.get(chain) {
            out.extend(chain.pairs.iter().map(|pair| pair.current_tail));
        }
    }

    /// Total steps taken since construction.
    pub fn step_count(&self) -> u64 {
        self.step_count
    }

    /// Sum of the step durations taken, in seconds.
    pub fn stepped_seconds(&self) -> f64 {
        self.stepped_seconds
    }

    /// Advance the system by one host frame of
    /// `frame_dt_seconds` (finite and positive), under the
    /// system's time mode. Returns the number of evaluation
    /// steps taken: always 1 in per-frame mode; in
    /// fixed-step mode, the whole steps that fit the
    /// accumulated frame time (the remainder is carried).
    /// The anchor and collider frames are held across the
    /// substeps of the call. For the collision response to
    /// be meaningful, `collider_frames` carries one frame
    /// per collider in the definition, in order.
    pub fn advance(
        &mut self,
        anchor: &Frame,
        collider_frames: &[Frame],
        frame_dt_seconds: f64,
    ) -> u64 {
        assert!(
            frame_dt_seconds.is_finite() && frame_dt_seconds > 0.0,
            "frame dt must be finite and positive, got {frame_dt_seconds}"
        );
        match self.config.mode {
            TimeMode::PerFrame => {
                self.step(anchor, collider_frames, frame_dt_seconds);
                1
            }
            TimeMode::FixedStep => {
                self.pending_seconds += frame_dt_seconds;
                let step_dt = self.config.step_dt_seconds;
                let mut taken = 0_u64;
                while self.pending_seconds >= step_dt {
                    self.step(anchor, collider_frames, step_dt);
                    self.pending_seconds -= step_dt;
                    taken += 1;
                }
                taken
            }
        }
    }

    /// One evaluation step of exactly `dt_seconds` (finite
    /// and positive), shared by both time modes: the modes
    /// differ only in how step durations are chosen, never
    /// in the evaluation itself. Updates every chain
    /// root-first, in declaration order.
    pub fn step(&mut self, anchor: &Frame, collider_frames: &[Frame], dt_seconds: f64) {
        assert!(
            dt_seconds.is_finite() && dt_seconds > 0.0,
            "step dt must be finite and positive, got {dt_seconds}"
        );
        anchor.assert_finite("anchor");
        assert!(
            collider_frames.len() == self.colliders.len(),
            "collider frames {} != colliders {}",
            collider_frames.len(),
            self.colliders.len()
        );
        for frame in collider_frames {
            frame.assert_finite("collider");
        }
        let colliders = &self.colliders;
        for chain in &mut self.chains {
            step_chain(chain, colliders, anchor, collider_frames, dt_seconds);
        }
        self.step_count += 1;
        self.stepped_seconds += dt_seconds;
    }
}

/// True when `dt_seconds` is one of the estate convention's
/// fixed steps (SPEC.md section 5.1): 1/30, 1/60, or 1/120.
fn is_convention_step_dt(dt_seconds: f64) -> bool {
    dt_seconds == 1.0 / 30.0 || dt_seconds == 1.0 / 60.0 || dt_seconds == STEP_DT_DEFAULT_SECONDS
}

/// Validate a system definition completely, before any
/// state is built: counts, group references, parameters,
/// and rest transforms.
fn validate_system(def: &SpringSystemDef) -> Result<(), SpringError> {
    if def.springs.is_empty() {
        return Err(SpringError::EmptySystem);
    }
    if def.springs.len() > SPRING_CHAINS_MAX {
        return Err(SpringError::TooManyChains {
            count: def.springs.len(),
            max: SPRING_CHAINS_MAX,
        });
    }
    if def.colliders.len() > SPRING_COLLIDERS_MAX {
        return Err(SpringError::TooManyColliders {
            count: def.colliders.len(),
            max: SPRING_COLLIDERS_MAX,
        });
    }
    if def.collider_groups.len() > SPRING_COLLIDER_GROUPS_MAX {
        return Err(SpringError::TooManyColliderGroups {
            count: def.collider_groups.len(),
            max: SPRING_COLLIDER_GROUPS_MAX,
        });
    }
    for (index, collider) in def.colliders.iter().enumerate() {
        validate_collider(index, collider)?;
    }
    for (group_index, group) in def.collider_groups.iter().enumerate() {
        if group.len() > SPRING_COLLIDERS_PER_GROUP_MAX {
            return Err(SpringError::ColliderGroupTooLong {
                group: group_index,
                count: group.len(),
                max: SPRING_COLLIDERS_PER_GROUP_MAX,
            });
        }
        for &collider in group {
            if collider >= def.colliders.len() {
                return Err(SpringError::UnknownCollider {
                    group: group_index,
                    collider,
                });
            }
        }
    }
    for (chain_index, chain) in def.springs.iter().enumerate() {
        validate_chain(chain_index, chain, def.collider_groups.len())?;
    }
    Ok(())
}

/// Validate one collider's shape parameters.
fn validate_collider(index: usize, collider: &ColliderDef) -> Result<(), SpringError> {
    let (offset, radius, tail) = match collider.shape {
        ColliderShape::Sphere { offset, radius } => (offset, radius, None),
        ColliderShape::Capsule {
            offset,
            radius,
            tail,
        } => (offset, radius, Some(tail)),
    };
    if !offset.is_finite() {
        return Err(SpringError::InvalidCollider {
            collider: index,
            field: "offset",
        });
    }
    if !radius.is_finite() || radius < 0.0 {
        return Err(SpringError::InvalidCollider {
            collider: index,
            field: "radius",
        });
    }
    if let Some(tail) = tail
        && !tail.is_finite()
    {
        return Err(SpringError::InvalidCollider {
            collider: index,
            field: "tail",
        });
    }
    Ok(())
}

/// Validate one chain: shape, group references, and every
/// joint's parameters and rest transform.
fn validate_chain(
    chain_index: usize,
    chain: &SpringChainDef,
    group_count: usize,
) -> Result<(), SpringError> {
    if chain.joints.is_empty() {
        return Err(SpringError::EmptyChain { chain: chain_index });
    }
    if chain.joints.len() < 2 {
        return Err(SpringError::ChainTooShort {
            chain: chain_index,
            count: chain.joints.len(),
        });
    }
    if chain.joints.len() > SPRING_JOINTS_PER_CHAIN_MAX {
        return Err(SpringError::ChainTooLong {
            chain: chain_index,
            count: chain.joints.len(),
            max: SPRING_JOINTS_PER_CHAIN_MAX,
        });
    }
    if chain.center_node.is_some() {
        return Err(SpringError::CenterUnsupported { chain: chain_index });
    }
    for &group in &chain.collider_groups {
        if group >= group_count {
            return Err(SpringError::UnknownColliderGroup {
                chain: chain_index,
                group,
            });
        }
    }
    for (joint_index, joint) in chain.joints.iter().enumerate() {
        validate_joint(chain_index, joint_index, joint)?;
    }
    Ok(())
}

/// Validate one joint's parameters and rest transform. The
/// last joint's parameters are unused by the algorithm but
/// are validated anyway: a definition is well-formed or it
/// is not, regardless of which fields a step reads.
fn validate_joint(chain: usize, joint_index: usize, joint: &JointDef) -> Result<(), SpringError> {
    let params = &joint.params;
    let invalid = |field: &'static str| SpringError::InvalidJointParam {
        chain,
        joint: joint_index,
        field,
    };
    if !params.hit_radius.is_finite() || params.hit_radius < 0.0 {
        return Err(invalid("hit_radius"));
    }
    if !params.stiffness.is_finite() || params.stiffness < 0.0 {
        return Err(invalid("stiffness"));
    }
    if !params.gravity_power.is_finite() || params.gravity_power < 0.0 {
        return Err(invalid("gravity_power"));
    }
    if !params.gravity_dir.is_finite() || params.gravity_dir.length() == 0.0 {
        return Err(invalid("gravity_dir"));
    }
    if !params.drag_force.is_finite() || !(0.0..=1.0).contains(&params.drag_force) {
        return Err(invalid("drag_force"));
    }
    if !joint.rest_local_translation.is_finite() || !joint.rest_local_rotation.is_finite() {
        return Err(SpringError::InvalidJointTransform {
            chain,
            joint: joint_index,
        });
    }
    Ok(())
}

/// Build one chain's runtime state at rest: walk the rest
/// transforms down from `rest_anchor`, deriving each pair's
/// bone axis (the rest tail direction in the head's rest
/// local frame) and bone length, with tails at rest.
fn build_chain(
    chain_index: usize,
    chain_def: &SpringChainDef,
    def: &SpringSystemDef,
    rest_anchor: &Frame,
) -> Result<SpringChain, SpringError> {
    let mut joints = chain_def.joints.clone();
    for joint in &mut joints {
        joint.params.gravity_dir = joint
            .params
            .gravity_dir
            .normalized_or(Vec3::new(0.0, -1.0, 0.0));
        joint.rest_local_rotation = joint.rest_local_rotation.normalized();
    }
    let mut colliders = Vec::new();
    for &group in &chain_def.collider_groups {
        colliders.extend_from_slice(&def.collider_groups[group]);
    }
    // Rest world transforms, walked root-first: the cursor
    // is the world transform of the current joint's parent.
    let mut cursor = *rest_anchor;
    let mut rest_positions = Vec::with_capacity(joints.len());
    let mut rest_world_rotations = Vec::with_capacity(joints.len());
    for joint in &joints {
        let position = cursor
            .position
            .add(cursor.rotation.rotate(joint.rest_local_translation));
        let rotation = cursor.rotation.mul(joint.rest_local_rotation);
        rest_positions.push(position);
        rest_world_rotations.push(rotation);
        cursor = Frame::new(position, rotation);
    }
    let mut pairs = Vec::with_capacity(joints.len() - 1);
    for index in 0..joints.len() - 1 {
        let delta = rest_positions[index + 1].sub(rest_positions[index]);
        let bone_length = delta.length();
        if bone_length.is_nan() || bone_length <= BONE_LENGTH_MIN {
            return Err(SpringError::DegenerateBone {
                chain: chain_index,
                joint: index,
            });
        }
        // The bone axis is the rest tail direction expressed
        // in the frame the applying-rotation step solves in:
        // the head's rest local frame, reached through its
        // parent's rest world rotation and its rest local
        // rotation.
        let parent_rest_rotation = if index == 0 {
            rest_anchor.rotation
        } else {
            rest_world_rotations[index - 1]
        };
        let local_frame = parent_rest_rotation.mul(joints[index].rest_local_rotation);
        let bone_axis = local_frame
            .inverse()
            .rotate(delta)
            .normalized_or(Vec3::new(1.0, 0.0, 0.0));
        pairs.push(PairState {
            prev_tail: rest_positions[index + 1],
            current_tail: rest_positions[index + 1],
            bone_axis,
            bone_length,
        });
    }
    let rotations = joints
        .iter()
        .map(|joint| joint.rest_local_rotation)
        .collect();
    Ok(SpringChain {
        joints,
        colliders,
        pairs,
        rotations,
    })
}

/// Advance one chain by one step: walk the pairs root-first
/// from the anchor, integrating each pair's tail and
/// solving its head's local rotation before descending —
/// the specification's calculation order.
fn step_chain(
    chain: &mut SpringChain,
    colliders: &[ColliderDef],
    anchor: &Frame,
    collider_frames: &[Frame],
    dt_seconds: f64,
) {
    // The cursor is the world transform of the current
    // pair's head parent: the anchor for pair 0, then each
    // head's updated world transform in turn.
    let mut cursor = *anchor;
    for index in 0..chain.pairs.len() {
        let head = chain.joints[index];
        let params = head.params;
        let pair = chain.pairs[index];
        let head_position = cursor
            .position
            .add(cursor.rotation.rotate(head.rest_local_translation));
        // Inertia calculation (specification pseudocode,
        // followed literally).
        let inertia = pair
            .current_tail
            .sub(pair.prev_tail)
            .scale(1.0 - params.drag_force);
        let rest_direction = cursor
            .rotation
            .mul(head.rest_local_rotation)
            .rotate(pair.bone_axis);
        let stiffness = rest_direction.scale(dt_seconds * params.stiffness);
        let external = params.gravity_dir.scale(dt_seconds * params.gravity_power);
        let mut next_tail = pair.current_tail.add(inertia).add(stiffness).add(external);
        next_tail = constrain_length(next_tail, head_position, pair.bone_length, rest_direction);
        // Collision with the chain's colliders, in declared
        // order, each push followed by the length
        // constraint, as the pseudocode prescribes.
        for &collider_index in &chain.colliders {
            let collider = &colliders[collider_index];
            let frame = &collider_frames[collider_index];
            if let Some((direction, distance)) =
                collision(collider, frame, next_tail, params.hit_radius)
                && distance < 0.0
            {
                next_tail = next_tail.sub(direction.scale(distance));
                next_tail =
                    constrain_length(next_tail, head_position, pair.bone_length, rest_direction);
            }
        }
        // Applying rotation: solve the head's local rotation
        // from the tail's solved position, then commit the
        // pair state (prev, current) := (current, next).
        let local_frame = cursor.rotation.mul(head.rest_local_rotation);
        let to = local_frame
            .inverse()
            .rotate(next_tail.sub(head_position))
            .normalized_or(pair.bone_axis);
        let swing = Quat::from_to(pair.bone_axis, to);
        let local_rotation = head.rest_local_rotation.mul(swing).normalized();
        chain.pairs[index] = PairState {
            prev_tail: pair.current_tail,
            current_tail: next_tail,
            ..pair
        };
        chain.rotations[index] = local_rotation;
        cursor = Frame::new(head_position, cursor.rotation.mul(local_rotation));
    }
}

/// Constrain `tail` to `bone_length` from `head`: the
/// specification's length constraint. A tail that lands
/// exactly on the head has no direction; it is placed along
/// the rest direction instead.
fn constrain_length(tail: Vec3, head: Vec3, bone_length: f64, rest_direction: Vec3) -> Vec3 {
    let delta = tail.sub(head);
    let length = delta.length();
    if length > BONE_LENGTH_MIN {
        head.add(delta.scale(bone_length / length))
    } else {
        head.add(rest_direction.scale(bone_length))
    }
}

/// The signed distance and push direction from a collider
/// to a tail point, following the specification's sphere
/// and capsule listings: `None` when the point coincides
/// with the collider's spine (direction undefined), a
/// negative distance when the point is inside the
/// collider's surface expanded by `joint_radius`.
fn collision(
    collider: &ColliderDef,
    frame: &Frame,
    tail: Vec3,
    joint_radius: f64,
) -> Option<(Vec3, f64)> {
    match collider.shape {
        ColliderShape::Sphere { offset, radius } => {
            let center = frame.position.add(frame.rotation.rotate(offset));
            sphere_distance(center, radius, tail, joint_radius)
        }
        ColliderShape::Capsule {
            offset,
            radius,
            tail: capsule_tail,
        } => {
            let head = frame.position.add(frame.rotation.rotate(offset));
            let tip = frame.position.add(frame.rotation.rotate(capsule_tail));
            let spine = tip.sub(head);
            let delta = tail.sub(head);
            let spine_length_sq = spine.dot(spine);
            let closest = if spine_length_sq > 0.0 {
                let along = spine.dot(delta);
                if along < 0.0 {
                    head
                } else if along > spine_length_sq {
                    tip
                } else {
                    head.add(spine.scale(along / spine_length_sq))
                }
            } else {
                head
            };
            sphere_distance(closest, radius, tail, joint_radius)
        }
    }
}

/// Distance from a sphere's surface (expanded by
/// `joint_radius`) to `tail`, and the direction from the
/// sphere's center to the tail.
fn sphere_distance(
    center: Vec3,
    radius: f64,
    tail: Vec3,
    joint_radius: f64,
) -> Option<(Vec3, f64)> {
    let delta = tail.sub(center);
    let length = delta.length();
    if length == 0.0 {
        return None;
    }
    Some((delta.scale(1.0 / length), length - radius - joint_radius))
}
