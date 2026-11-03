//! The D-679 determinism comparison: the VRMC_springBone
//! evaluator under its two time models.
//!
//! One fixture (a ponytail chain and a skirt chain with a
//! sphere collider), one scripted anchor motion (swing,
//! sudden stop, hold, quarter turn, hold), driven as a pure
//! function of simulation time. Each time model is sampled
//! at render cadences of 24, 30, 60, and 120 fps over the
//! same 6 seconds:
//!
//! - PerFrame steps once per rendered frame with the frame
//!   dt, inputs sampled at the frame's start time.
//! - FixedStep (dt = 1/120, the estate convention) steps in
//!   whole steps counted from simulation time, inputs
//!   sampled at each step's start time — as a conforming
//!   fixed-step host with a session-recorded input track
//!   drives it. The evaluation core is shared; only the
//!   time advance differs.
//!
//! Prints a Markdown table: fixed-step double-replay hash
//! identity (the gate), each model's cadence divergence
//! (max tail deviation between its 30 fps and 120 fps
//! samplings, over the sample times both share), and the
//! cross-mode deviation at 60 fps. Run with:
//! `cargo run --example spring_bone_comparison`

use lumen_rig_core::spring_bone::{
    ColliderDef, ColliderShape, Frame, JointDef, JointParams, Quat, SpringChainDef, SpringSystem,
    SpringSystemDef, Vec3,
};

const DURATION_SECONDS: f64 = 6.0;
const STEP_DT: f64 = 1.0 / 120.0;
const CADENCES: [u32; 4] = [24, 30, 60, 120];

/// The scripted anchor (head) motion, a pure function of
/// simulation time: a 2 s swing, a sudden stop, a hold, a
/// quarter turn over 0.5 s, then a hold.
fn anchor_at(t: f64) -> Frame {
    let (sway, yaw, lift) = if t < 2.0 {
        let phase = std::f64::consts::PI * t;
        (0.25 * phase.sin(), 0.5 * phase.sin(), 0.0)
    } else if t < 3.5 {
        (0.0, 0.0, 0.0)
    } else if t < 4.0 {
        let s = (t - 3.5) / 0.5;
        let smooth = s * s * (3.0 - 2.0 * s);
        (0.0, std::f64::consts::FRAC_PI_2 * smooth, 0.05 * smooth)
    } else {
        (0.0, std::f64::consts::FRAC_PI_2, 0.05)
    };
    let rotation = Quat::new(0.0, (yaw / 2.0).sin(), 0.0, (yaw / 2.0).cos());
    Frame::new(Vec3::new(sway, 1.4 + lift, 0.0), rotation)
}

fn joint(node: usize, translation: Vec3, params: JointParams) -> JointDef {
    JointDef {
        node,
        params,
        rest_local_translation: translation,
        rest_local_rotation: Quat::IDENTITY,
    }
}

/// The fixture: chain A, a ponytail trailing back and down
/// from the head; chain B, a skirt strand hanging beside a
/// chest sphere collider (collider node = the anchor node).
fn fixture() -> SpringSystemDef {
    let ponytail = JointParams {
        hit_radius: 0.02,
        stiffness: 0.9,
        gravity_power: 0.7,
        gravity_dir: Vec3::new(0.0, -1.0, 0.0),
        drag_force: 0.35,
    };
    let skirt = JointParams {
        hit_radius: 0.03,
        stiffness: 1.4,
        gravity_power: 0.9,
        gravity_dir: Vec3::new(0.0, -1.0, 0.0),
        drag_force: 0.3,
    };
    SpringSystemDef {
        colliders: vec![ColliderDef {
            node: 0,
            shape: ColliderShape::Sphere {
                offset: Vec3::new(0.0, -0.18, 0.02),
                radius: 0.09,
            },
        }],
        collider_groups: vec![vec![0]],
        springs: vec![
            SpringChainDef {
                joints: (0..5)
                    .map(|index| {
                        let translation = if index == 0 {
                            Vec3::new(0.0, 0.06, -0.09)
                        } else {
                            Vec3::new(0.0, 0.02, -0.055)
                        };
                        joint(index, translation, ponytail)
                    })
                    .collect(),
                collider_groups: Vec::new(),
                center_node: None,
            },
            SpringChainDef {
                joints: (0..4)
                    .map(|index| {
                        let translation = if index == 0 {
                            Vec3::new(0.09, -0.12, 0.0)
                        } else {
                            Vec3::new(0.01, -0.075, 0.0)
                        };
                        joint(10 + index, translation, skirt)
                    })
                    .collect(),
                collider_groups: vec![0],
                center_node: None,
            },
        ],
    }
}

/// One sample: every pair's tail position, both chains in
/// declaration order.
fn sample(system: &SpringSystem) -> Vec<Vec3> {
    let mut out = Vec::new();
    let mut tails = Vec::new();
    for chain in 0..system.chain_count() {
        system.current_tails(chain, &mut tails);
        out.extend(tails.iter().copied());
    }
    out
}

/// FNV-1a over the full state (tails and local rotations),
/// as in the crate's determinism gate test.
fn state_hash(system: &SpringSystem) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let mut mix = |bits: u64| {
        for byte in bits.to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    for chain in 0..system.chain_count() {
        let mut tails = Vec::new();
        system.current_tails(chain, &mut tails);
        for tail in tails {
            mix(tail.x.to_bits());
            mix(tail.y.to_bits());
            mix(tail.z.to_bits());
        }
        for rotation in system.local_rotations(chain) {
            mix(rotation.x.to_bits());
            mix(rotation.y.to_bits());
            mix(rotation.z.to_bits());
            mix(rotation.w.to_bits());
        }
    }
    hash
}

/// Run one time model at one render cadence. Returns the
/// per-frame tail samples and the state hash after each
/// frame.
fn run(mode: &str, cadence: u32) -> (Vec<Vec<Vec3>>, Vec<u64>) {
    let mut system = SpringSystem::new(fixture(), &anchor_at(0.0)).expect("fixture builds");
    let frames = (DURATION_SECONDS * f64::from(cadence)) as usize;
    let mut samples = Vec::with_capacity(frames);
    let mut hashes = Vec::with_capacity(frames);
    match mode {
        "per-frame" => {
            let frame_dt = 1.0 / f64::from(cadence);
            for frame in 0..frames {
                let t = f64::from(frame as u32) * frame_dt;
                let anchor = anchor_at(t);
                system.step(&anchor, &[anchor], frame_dt);
                samples.push(sample(&system));
                hashes.push(state_hash(&system));
            }
        }
        "fixed-step" => {
            let steps_per_frame = (120 / cadence) as usize;
            let total_steps = frames * steps_per_frame;
            for step in 0..total_steps {
                let t = f64::from(step as u32) * STEP_DT;
                let anchor = anchor_at(t);
                system.step(&anchor, &[anchor], STEP_DT);
                if (step + 1) % steps_per_frame == 0 {
                    samples.push(sample(&system));
                    hashes.push(state_hash(&system));
                }
            }
        }
        _ => unreachable!("mode is one of the two literals above"),
    }
    (samples, hashes)
}

/// Max deviation between two sample sequences over their
/// shared sample times (the coarser cadence's frames),
/// across all tails. `fine_stride` is how many fine
/// samples make one coarse frame; coarse frame `k` (ending
/// at time `(k + 1) / coarse_cadence`) aligns with fine
/// sample `k * fine_stride + fine_stride - 1`.
fn max_deviation(
    coarse: &[Vec<Vec3>],
    fine: &[Vec<Vec3>],
    fine_stride: usize,
) -> (f64, usize, usize) {
    let mut max = 0.0_f64;
    let mut at_frame = 0_usize;
    let mut at_tail = 0_usize;
    for (frame, coarse_sample) in coarse.iter().enumerate() {
        let fine_sample = &fine[frame * fine_stride + fine_stride - 1];
        for (tail, (a, b)) in coarse_sample.iter().zip(fine_sample.iter()).enumerate() {
            let deviation =
                ((a.x - b.x).powi(2) + (a.y - b.y).powi(2) + (a.z - b.z).powi(2)).sqrt();
            if deviation > max {
                max = deviation;
                at_frame = frame;
                at_tail = tail;
            }
        }
    }
    (max, at_frame, at_tail)
}

fn main() {
    println!("# D-679 spring-bone determinism comparison (raw output)\n");
    println!("Duration {DURATION_SECONDS} s; fixed step {STEP_DT} s; cadences {CADENCES:?} fps.\n");

    // Gate 1: fixed-step double replay at 60 fps.
    let (_, hashes_a) = run("fixed-step", 60);
    let (_, hashes_b) = run("fixed-step", 60);
    println!(
        "Gate 1 — fixed-step double replay at 60 fps: {} ({} frame hashes compared)",
        if hashes_a == hashes_b {
            "IDENTICAL"
        } else {
            "MISMATCH"
        },
        hashes_a.len()
    );

    // Gate 2 (expected): fixed-step cadence invariance —
    // the 30 fps and 120 fps samplings share every step, so
    // their states at shared times are the same states.
    let (_, hashes_30) = run("fixed-step", 30);
    let (_, hashes_120) = run("fixed-step", 120);
    let shared_equal = hashes_30
        .iter()
        .enumerate()
        .all(|(frame, hash)| *hash == hashes_120[frame * 4 + 3]);
    println!(
        "Gate 2 — fixed-step 30 fps samples == 120 fps samples at shared times: {}",
        if shared_equal {
            "IDENTICAL"
        } else {
            "MISMATCH"
        }
    );

    let mut samples = std::collections::BTreeMap::new();
    for mode in ["per-frame", "fixed-step"] {
        for cadence in CADENCES {
            let (mode_samples, _) = run(mode, cadence);
            samples.insert((mode, cadence), mode_samples);
        }
    }

    let (per_frame_dev, pf_frame, pf_tail) = max_deviation(
        &samples[&("per-frame", 30)],
        &samples[&("per-frame", 120)],
        4,
    );
    let (fixed_dev, fx_frame, fx_tail) = max_deviation(
        &samples[&("fixed-step", 30)],
        &samples[&("fixed-step", 120)],
        4,
    );
    let (cross_dev, x_frame, x_tail) = max_deviation(
        &samples[&("per-frame", 60)],
        &samples[&("fixed-step", 60)],
        1,
    );

    println!("\n| Measure | Max deviation (world units) | At frame | Tail |");
    println!("| --- | --- | --- | --- |");
    println!(
        "| Per-frame cadence divergence (30 vs 120 fps) | {per_frame_dev:.6} | {} (t = {:.3} s) | {pf_tail} |",
        pf_frame + 1,
        f64::from((pf_frame + 1) as u32) / 30.0
    );
    println!(
        "| Fixed-step cadence divergence (30 vs 120 fps) | {fixed_dev:.6} | {} | {fx_tail} |",
        fx_frame + 1
    );
    println!(
        "| Cross-mode deviation at 60 fps | {cross_dev:.6} | {} (t = {:.3} s) | {x_tail} |",
        x_frame + 1,
        f64::from((x_frame + 1) as u32) / 60.0
    );
    println!(
        "\nTails are indexed across both chains in declaration order (0–3 ponytail, 4–6 skirt)."
    );
}
