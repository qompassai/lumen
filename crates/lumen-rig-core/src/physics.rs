//! Physics evaluation: SPEC.md section 5, exactly as
//! written. Fixed-step Verlet strands with a seeded,
//! index-pure turbulence draw; no other forces exist in
//! v1 (no collisions, no floor, no particle interaction).
//!
//! The strand's pinned root (section 5.2's "particle 0") is
//! the anchor point itself — the anchor object's evaluated
//! position plus the strand offset. The strand's listed
//! particles are the free particles: listed particle `k` is
//! joined to the previous point by a segment of its own
//! `length`, with stiffness `1 - mobility` (section 5.2
//! step 3 names the mobility of particle `k + 1`, counting
//! the anchor point as particle 0). This is the only
//! reading under which the landed one-particle exports (one
//! pendulum bob per strand, written by the torogao bridge)
//! have a tip, a segment, and an output angle at all.
//!
//! What this is **not**: the INP reference integrates rigid
//! and spring pendulums with an RK4 solver in anchor space
//! (torogao-engine's `physics.rs`). Section 5's Verlet chain
//! is a different integrator over different state, chosen by
//! the format; trajectories differ from INP's, as the bridge
//! exporter already records on every converted strand
//! ("INP integrates a rigid/spring pendulum in anchor space
//! while lumen integrates a Verlet chain — trajectories will
//! differ"). Nothing here approximates RK4; section 5 is
//! implemented as specified, and the INP behaviours with no
//! section 5 counterpart (spring rest-angle forces, length
//! and frequency offsets, per-stage provider re-reads) are
//! named here as not carried.
//!
//! Determinism: the simulation is a pure function of the
//! model, the parameter states it is stepped under, the
//! anchor positions it is handed, the step count, and the
//! run seed. Turbulence draws are a pure function of
//! (run seed, group id, step, particle ordinal) — there is
//! no RNG state to drift (section 5.3).

use crate::model::{Anchor, AnchorKind, Model, PhysicsGroup};
use crate::pose::{EvaluatedMesh, ParameterValues, deformer_position, mesh_centroid};

/// Constraint relaxation passes per step (section 5.2
/// step 3: exactly 4).
const CONSTRAINT_ITERATIONS: usize = 4;

/// One strand's simulation state: the free particles'
/// positions and previous positions (f64 model-space
/// points). The pinned root is not stored — it is the
/// anchor position of the current step.
#[derive(Debug, Clone, PartialEq)]
struct StrandState {
    positions: Vec<[f64; 2]>,
    previous: Vec<[f64; 2]>,
}

/// The physics simulation for one model: strand states in
/// (group, strand) declaration order, plus the step counter
/// the turbulence stream indexes by.
#[derive(Debug, Clone, PartialEq)]
pub struct PhysicsSim {
    strands: Vec<StrandState>,
    step_index: u64,
    run_seed: u64,
}

impl PhysicsSim {
    /// A simulation at rest for `model` under `values`:
    /// every strand's particles lie along +x from its anchor
    /// (section 5.1's pinned rest construction), previous
    /// positions equal to positions (zero velocity).
    pub fn new(model: &Model, values: &ParameterValues, run_seed: u64) -> PhysicsSim {
        let evaluated = crate::pose::evaluate_pose(model, values);
        let mut strands = Vec::new();
        for group in &model.physics {
            for strand in &group.strands {
                let anchor = anchor_position(model, values, &evaluated, &strand.anchor);
                let mut positions = Vec::with_capacity(strand.particles.len());
                let mut cursor = anchor;
                for particle in &strand.particles {
                    cursor = [cursor[0] + f64::from(particle.length), cursor[1]];
                    positions.push(cursor);
                }
                strands.push(StrandState {
                    previous: positions.clone(),
                    positions,
                });
            }
        }
        PhysicsSim {
            strands,
            step_index: 0,
            run_seed,
        }
    }

    /// The number of steps taken since construction.
    pub fn step_index(&self) -> u64 {
        self.step_index
    }

    /// The current free-particle positions of one strand,
    /// addressed by (group index, strand index) in
    /// declaration order within `model`. Exposed for
    /// conformance tests and host debugging; the render
    /// path never reads these.
    pub fn strand_positions(
        &self,
        model: &Model,
        group: usize,
        strand: usize,
    ) -> Option<&[[f64; 2]]> {
        let mut flat = 0_usize;
        for (group_index, physics_group) in model.physics.iter().enumerate() {
            if group_index == group {
                return self
                    .strands
                    .get(flat + strand)
                    .map(|state| state.positions.as_slice());
            }
            flat += physics_group.strands.len();
        }
        None
    }

    /// Advance the whole simulation one fixed step of
    /// `dt_seconds`, in section 5.2's exact order: for each
    /// group, for each strand — effective gravity from the
    /// current parameter values, Verlet integration against
    /// this step's anchor positions, four constraint passes,
    /// then the output write (through the sanctioned clamp,
    /// via [`ParameterValues::set`]).
    ///
    /// `anchors` holds every strand's anchor position for
    /// this step, in (group, strand) declaration order,
    /// computed by the caller from the pose at step entry —
    /// a step never sees its own output writes in its
    /// anchors, only in later strands' gravity (the order
    /// section 5.2 writes down).
    pub fn step(
        &mut self,
        model: &Model,
        values: &mut ParameterValues,
        anchors: &[[f64; 2]],
        dt_seconds: f64,
    ) {
        assert!(
            dt_seconds.is_finite() && dt_seconds > 0.0,
            "physics dt must be finite and positive, got {dt_seconds}"
        );
        let step = self.step_index;
        let mut flat = 0_usize;
        let mut ordinal_base = 0_usize;
        for group in &model.physics {
            for strand in &group.strands {
                let anchor = anchors[flat];
                let state = &mut self.strands[flat];
                step_strand(
                    model,
                    group,
                    strand,
                    state,
                    anchor,
                    values,
                    self.run_seed,
                    step,
                    ordinal_base,
                    dt_seconds,
                );
                if let Some(output) = &strand.output {
                    let tip = *state.positions.last().expect("a strand has particles");
                    let prev = if state.positions.len() >= 2 {
                        state.positions[state.positions.len() - 2]
                    } else {
                        anchor
                    };
                    let theta = (tip[1] - prev[1]).atan2(tip[0] - prev[0]).to_degrees();
                    let written = f64::from(output.offset) + f64::from(output.scale) * theta;
                    values.set(model, &output.parameter, written as f32);
                }
                ordinal_base += strand.particles.len();
                flat += 1;
            }
        }
        self.step_index += 1;
    }
}

/// The anchor position of one strand under the current
/// pose (section 5.2 step 2): the anchor object's evaluated
/// position plus the strand offset. Deformer anchors use
/// the deformer's evaluated origin; mesh anchors use the
/// evaluated mesh's centroid.
pub fn anchor_position(
    model: &Model,
    values: &ParameterValues,
    evaluated: &[EvaluatedMesh],
    anchor: &Anchor,
) -> [f64; 2] {
    let base = match anchor.kind {
        AnchorKind::Deformer => deformer_position(model, values, &anchor.id).unwrap_or([0.0, 0.0]),
        AnchorKind::Mesh => evaluated
            .iter()
            .find(|mesh| mesh.id == anchor.id)
            .map(mesh_centroid)
            .unwrap_or([0.0, 0.0]),
    };
    [
        base[0] + f64::from(anchor.offset[0]),
        base[1] + f64::from(anchor.offset[1]),
    ]
}

/// Every strand's anchor position under the current pose,
/// in (group, strand) declaration order — the input a
/// physics step is handed.
pub fn anchor_positions(
    model: &Model,
    values: &ParameterValues,
    evaluated: &[EvaluatedMesh],
) -> Vec<[f64; 2]> {
    let mut out = Vec::new();
    for group in &model.physics {
        for strand in &group.strands {
            out.push(anchor_position(model, values, evaluated, &strand.anchor));
        }
    }
    out
}

/// One strand's integration and constraint solve (section
/// 5.2 steps 1–3; the output write is the caller's, since
/// it mutates the shared parameter state).
#[allow(clippy::too_many_arguments)]
fn step_strand(
    model: &Model,
    group: &PhysicsGroup,
    strand: &crate::model::Strand,
    state: &mut StrandState,
    anchor: [f64; 2],
    values: &ParameterValues,
    run_seed: u64,
    step: u64,
    ordinal_base: usize,
    dt_seconds: f64,
) {
    // Step 1: effective gravity — the group's gravity
    // rotated by the inputs' weighted angle, plus wind and
    // per-particle turbulence.
    let angle_degrees = group_angle_degrees(model, group, values);
    let radians = angle_degrees.to_radians();
    let (sin, cos) = radians.sin_cos();
    let gravity = [
        cos * f64::from(group.gravity[0]) - sin * f64::from(group.gravity[1]),
        sin * f64::from(group.gravity[0]) + cos * f64::from(group.gravity[1]),
    ];
    let dt2 = dt_seconds * dt_seconds;
    // Step 2: Verlet integration of the free particles.
    for (slot, particle) in strand.particles.iter().enumerate() {
        let turbulence = turbulence_accel(
            run_seed,
            &group.id,
            step,
            ordinal_base + slot,
            f64::from(group.turbulence_amplitude),
        );
        let accel = [
            gravity[0] + f64::from(group.wind[0]) + turbulence[0],
            gravity[1] + f64::from(group.wind[1]) + turbulence[1],
        ];
        let position = state.positions[slot];
        let previous = state.previous[slot];
        let keep = 1.0 - f64::from(particle.damping);
        state.positions[slot] = [
            position[0] + (position[0] - previous[0]) * keep + accel[0] * dt2,
            position[1] + (position[1] - previous[1]) * keep + accel[1] * dt2,
        ];
        state.previous[slot] = position;
    }
    // Step 3: exactly four relaxation passes; the root is
    // pinned at the anchor every pass.
    for _ in 0..CONSTRAINT_ITERATIONS {
        let mut previous_point = anchor;
        for (slot, particle) in strand.particles.iter().enumerate() {
            let length = f64::from(particle.length);
            let stiffness = 1.0 - f64::from(particle.mobility);
            let current = state.positions[slot];
            let delta = [
                current[0] - previous_point[0],
                current[1] - previous_point[1],
            ];
            let distance = (delta[0] * delta[0] + delta[1] * delta[1]).sqrt();
            if distance > 0.0 {
                let diff = (distance - length) / distance * stiffness;
                if slot == 0 {
                    // The root endpoint never moves: the
                    // free particle takes the whole scaled
                    // correction.
                    state.positions[slot] =
                        [current[0] - delta[0] * diff, current[1] - delta[1] * diff];
                } else {
                    let half = diff / 2.0;
                    state.positions[slot - 1] = [
                        state.positions[slot - 1][0] + delta[0] * half,
                        state.positions[slot - 1][1] + delta[1] * half,
                    ];
                    state.positions[slot] =
                        [current[0] - delta[0] * half, current[1] - delta[1] * half];
                }
            }
            previous_point = state.positions[slot];
        }
    }
}

/// The effective-gravity rotation of one group (section
/// 5.2 step 1): `Σ scale_degrees_i * norm_i` with
/// `norm_i = (value_i - min_i) / (max_i - min_i)`.
fn group_angle_degrees(model: &Model, group: &PhysicsGroup, values: &ParameterValues) -> f64 {
    let mut angle = 0.0_f64;
    for input in &group.inputs {
        if let Some(norm) = values.normalized(model, &input.parameter) {
            angle += f64::from(input.scale_degrees) * norm;
        }
    }
    angle
}

/// The turbulence acceleration for one particle (section
/// 5.3): two index-pure draws, one per axis.
fn turbulence_accel(
    run_seed: u64,
    group_id: &str,
    step: u64,
    ordinal: usize,
    amplitude: f64,
) -> [f64; 2] {
    if amplitude == 0.0 {
        return [0.0, 0.0];
    }
    let stream_seed = run_seed ^ fnv1a64(group_id.as_bytes());
    let draw = |draw_index: usize| -> f64 {
        let index = (step << 32) | (draw_index as u64);
        let z = splitmix64_finalize(
            stream_seed.wrapping_add(index.wrapping_mul(0x9E37_79B9_7F4A_7C15)),
        );
        let u = ((z >> 11) as f64) * (1.0 / (1_u64 << 53) as f64);
        amplitude * (2.0 * u - 1.0)
    };
    [draw(ordinal * 2), draw(ordinal * 2 + 1)]
}

/// FNV-1a 64 over bytes (section 5.3's exact constants).
fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xCBF2_9CE4_8422_2325_u64;
    for byte in bytes {
        hash = (hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01B3);
    }
    hash
}

/// The splitmix64 finalizer (section 5.3's exact shifts
/// and multipliers, wrapping u64 throughout).
fn splitmix64_finalize(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}
