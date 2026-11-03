//! The D-679 visual companion: the VRMC_springBone
//! evaluator driving two chest spring chains on a torso
//! anchor, under one scripted rhythmic bounce, evaluated
//! three ways — PerFrame at 30 fps, PerFrame at 120 fps,
//! and FixedStep (dt = 1/120 s, sampled at 60 fps).
//!
//! The motion is a pure function of simulation time: a
//! 1.2 Hz bounce cycle (fast attack, smooth release, dwell)
//! for 4.5 s, an amplitude settle over 0.7 s, a quarter
//! turn over 0.6 s starting at 5.0 s, then a hold to 6 s.
//! All three runs are exported on one shared 60 fps output
//! timeline (360 frames) as JSON: per frame, the anchor
//! (torso root) frame actually used by the step that
//! produced the state, and every joint's world position
//! for both chains. The renderer consumes only this file;
//! the evaluator is the source of truth.
//!
//! Also computed here, from the exported trajectories:
//! the frame of maximum divergence between the PerFrame
//! 30 fps chain tips and the FixedStep chain tips.
//!
//! Run with:
//! `cargo run --example spring_bone_visual -- <out.json>`

use lumen_rig_core::spring_bone::{
    ColliderDef, ColliderShape, Frame, JointDef, JointParams, Quat, SpringChainDef, SpringSystem,
    SpringSystemDef, Vec3,
};
use serde_json::{Value, json};

const DURATION_SECONDS: f64 = 6.0;
const OUTPUT_FPS: u32 = 60;
const STEP_DT: f64 = 1.0 / 120.0;
const BOUNCE_HZ: f64 = 1.2;
const BOUNCE_END_SECONDS: f64 = 4.5;
const SETTLE_END_SECONDS: f64 = 5.2;
const TURN_START_SECONDS: f64 = 5.0;
const TURN_END_SECONDS: f64 = 5.6;
const ANCHOR_REST_Y: f64 = 1.08;

/// Chain joint rest translations, shared by the fixture
/// builder and the joint-position reconstruction: joint 0
/// sits on the chest front (x lateral, z forward), the
/// two segments run down and slightly out.
const CHAIN_JOINT0: [Vec3; 2] = [
    Vec3 {
        x: -0.095,
        y: -0.01,
        z: 0.105,
    },
    Vec3 {
        x: 0.095,
        y: -0.01,
        z: 0.105,
    },
];
const CHAIN_JOINT1: [Vec3; 2] = [
    Vec3 {
        x: -0.012,
        y: -0.058,
        z: 0.038,
    },
    Vec3 {
        x: 0.012,
        y: -0.058,
        z: 0.038,
    },
];
const CHAIN_JOINT2: [Vec3; 2] = [
    Vec3 {
        x: -0.008,
        y: -0.052,
        z: 0.026,
    },
    Vec3 {
        x: 0.008,
        y: -0.052,
        z: 0.026,
    },
];

/// Component-wise vector sum (the crate's Vec3 arithmetic
/// is crate-private; hosts compose from the components).
fn vadd(a: Vec3, b: Vec3) -> Vec3 {
    Vec3::new(a.x + b.x, a.y + b.y, a.z + b.z)
}

/// The Hamilton product `a * b` (applies `b`, then `a`),
/// matching the crate's composition convention.
fn qmul(a: Quat, b: Quat) -> Quat {
    Quat::new(
        a.w * b.x + a.x * b.w + a.y * b.z - a.z * b.y,
        a.w * b.y - a.x * b.z + a.y * b.w + a.z * b.x,
        a.w * b.z + a.x * b.y - a.y * b.x + a.z * b.w,
        a.w * b.w - a.x * b.x - a.y * b.y - a.z * b.z,
    )
}

fn smoothstep(edge0: f64, edge1: f64, x: f64) -> f64 {
    let s = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    s * s * (3.0 - 2.0 * s)
}

/// One bounce cycle as a function of phase in [0, 1):
/// fast attack, smooth release, dwell at rest.
fn bounce_waveform(phase: f64) -> f64 {
    let attack = smoothstep(0.0, 0.22, phase);
    let release = 1.0 - smoothstep(0.30, 0.78, phase);
    attack * release
}

/// The scripted torso anchor, a pure function of
/// simulation time. Returns the frame, the pitch, and
/// the yaw (the renderer poses the torso mesh from the
/// two angles; the chains consume only the frame).
fn anchor_at(t: f64) -> (Frame, f64, f64) {
    let envelope = if t < BOUNCE_END_SECONDS {
        1.0
    } else if t < SETTLE_END_SECONDS {
        1.0 - smoothstep(BOUNCE_END_SECONDS, SETTLE_END_SECONDS, t)
    } else {
        0.0
    };
    let wave = bounce_waveform((t * BOUNCE_HZ).fract()) * envelope;
    let yaw = if t < TURN_START_SECONDS {
        0.0
    } else if t < TURN_END_SECONDS {
        std::f64::consts::FRAC_PI_2 * smoothstep(TURN_START_SECONDS, TURN_END_SECONDS, t)
    } else {
        std::f64::consts::FRAC_PI_2
    };
    let pitch = 0.06 * wave;
    let position = Vec3::new(0.0, ANCHOR_REST_Y - 0.06 * wave, 0.0);
    let yaw_quat = Quat::new(0.0, (yaw / 2.0).sin(), 0.0, (yaw / 2.0).cos());
    let pitch_quat = Quat::new((pitch / 2.0).sin(), 0.0, 0.0, (pitch / 2.0).cos());
    (Frame::new(position, qmul(yaw_quat, pitch_quat)), pitch, yaw)
}

fn joint(node: usize, translation: Vec3, params: JointParams) -> JointDef {
    JointDef {
        node,
        params,
        rest_local_translation: translation,
        rest_local_rotation: Quat::IDENTITY,
    }
}

/// The fixture: two chest chains (left, right), each of
/// two segments, colliding against one sphere carried by
/// the anchor (the torso volume), parameters in the
/// D-679 fixture family.
fn fixture() -> SpringSystemDef {
    let params = JointParams {
        hit_radius: 0.02,
        stiffness: 1.1,
        gravity_power: 0.6,
        gravity_dir: Vec3::new(0.0, -1.0, 0.0),
        drag_force: 0.35,
    };
    let chains = (0..2)
        .map(|side| SpringChainDef {
            joints: vec![
                joint(10 * side, CHAIN_JOINT0[side], params),
                joint(10 * side + 1, CHAIN_JOINT1[side], params),
                joint(10 * side + 2, CHAIN_JOINT2[side], params),
            ],
            collider_groups: vec![0],
            center_node: None,
        })
        .collect();
    SpringSystemDef {
        colliders: vec![ColliderDef {
            node: 0,
            shape: ColliderShape::Sphere {
                offset: Vec3::new(0.0, -0.04, 0.0),
                radius: 0.115,
            },
        }],
        collider_groups: vec![vec![0]],
        springs: chains,
    }
}

/// One exported frame: the anchor frame used by the step
/// that produced this state, and every joint's world
/// position (joint 0 from the anchor walk, the rest from
/// the pairs' current tails — the simulated positions).
struct Sample {
    anchor: Frame,
    pitch: f64,
    yaw: f64,
    joints: [[Vec3; 3]; 2],
}

fn sample(system: &SpringSystem, anchor: &Frame, pitch: f64, yaw: f64) -> Sample {
    let mut joints = [[Vec3::ZERO; 3]; 2];
    let mut tails = Vec::new();
    for (side, chain) in joints.iter_mut().enumerate() {
        chain[0] = vadd(anchor.position, anchor.rotation.rotate(CHAIN_JOINT0[side]));
        system.current_tails(side, &mut tails);
        chain[1] = tails[0];
        chain[2] = tails[1];
    }
    Sample {
        anchor: *anchor,
        pitch,
        yaw,
        joints,
    }
}

/// PerFrame at `cadence` fps: one step per rendered
/// frame with the frame dt, inputs sampled at the
/// frame's start. Each step's state fills the output
/// frames it covers on the 60 fps timeline.
fn run_per_frame(cadence: u32) -> Vec<Sample> {
    let (rest, _, _) = anchor_at(0.0);
    let mut system = SpringSystem::new(fixture(), &rest).expect("fixture builds");
    let frame_dt = 1.0 / f64::from(cadence);
    let total_frames = (DURATION_SECONDS * f64::from(OUTPUT_FPS)) as usize;
    // Below the output rate, each step's state fills the
    // output frames it covers; above it, only the steps
    // landing on the output timeline are sampled.
    let fill = (OUTPUT_FPS / cadence).max(1) as usize;
    let stride = (cadence / OUTPUT_FPS).max(1) as usize;
    let mut out = Vec::with_capacity(total_frames);
    let steps = (DURATION_SECONDS * f64::from(cadence)) as usize;
    for step in 0..steps {
        let t = f64::from(step as u32) * frame_dt;
        let (anchor, pitch, yaw) = anchor_at(t);
        system.step(&anchor, &[anchor], frame_dt);
        if cadence > OUTPUT_FPS && (step + 1) % stride != 0 {
            continue;
        }
        let taken = sample(&system, &anchor, pitch, yaw);
        for _ in 0..fill {
            out.push(Sample {
                anchor: taken.anchor,
                pitch: taken.pitch,
                yaw: taken.yaw,
                joints: taken.joints,
            });
        }
    }
    out.truncate(total_frames);
    out
}

/// FixedStep: whole steps of dt = 1/120 s, inputs
/// sampled at each step's start (a conforming host with
/// a recorded input track), sampled onto the 60 fps
/// timeline after every second step.
fn run_fixed_step() -> Vec<Sample> {
    let (rest, _, _) = anchor_at(0.0);
    let mut system = SpringSystem::new(fixture(), &rest).expect("fixture builds");
    let total_frames = (DURATION_SECONDS * f64::from(OUTPUT_FPS)) as usize;
    let mut out = Vec::with_capacity(total_frames);
    let steps = (DURATION_SECONDS / STEP_DT) as usize;
    for step in 0..steps {
        let t = f64::from(step as u32) * STEP_DT;
        let (anchor, pitch, yaw) = anchor_at(t);
        system.step(&anchor, &[anchor], STEP_DT);
        if (step + 1) % 2 == 0 {
            out.push(sample(&system, &anchor, pitch, yaw));
        }
    }
    out
}

fn vec_json(v: Vec3) -> Value {
    json!([v.x, v.y, v.z])
}

fn samples_json(samples: &[Sample]) -> Value {
    Value::Array(
        samples
            .iter()
            .map(|s| {
                json!({
                    "root": vec_json(s.anchor.position),
                    "quat": [s.anchor.rotation.x, s.anchor.rotation.y,
                             s.anchor.rotation.z, s.anchor.rotation.w],
                    "pitch": s.pitch,
                    "yaw": s.yaw,
                    "chains": [
                        [vec_json(s.joints[0][0]), vec_json(s.joints[0][1]),
                         vec_json(s.joints[0][2])],
                        [vec_json(s.joints[1][0]), vec_json(s.joints[1][1]),
                         vec_json(s.joints[1][2])],
                    ],
                })
            })
            .collect(),
    )
}

fn distance(a: Vec3, b: Vec3) -> f64 {
    ((a.x - b.x).powi(2) + (a.y - b.y).powi(2) + (a.z - b.z).powi(2)).sqrt()
}

fn main() {
    let out_path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "spring_visual.json".to_string());
    let per_frame_30 = run_per_frame(30);
    let per_frame_120 = run_per_frame(120);
    let fixed_step = run_fixed_step();

    // Maximum tip divergence, PerFrame 30 vs FixedStep,
    // over both chains' tips (joint 2).
    let mut max_30 = (0.0_f64, 0_usize, 0_usize);
    let mut max_120 = (0.0_f64, 0_usize, 0_usize);
    for frame in 0..per_frame_30.len() {
        let a = &per_frame_30[frame];
        let b = &per_frame_120[frame];
        let c = &fixed_step[frame];
        for side in 0..2 {
            let d30 = distance(a.joints[side][2], c.joints[side][2]);
            if d30 > max_30.0 {
                max_30 = (d30, frame, side);
            }
            let d120 = distance(b.joints[side][2], c.joints[side][2]);
            if d120 > max_120.0 {
                max_120 = (d120, frame, side);
            }
        }
    }
    let max_frame = max_30.1;
    let max_time = f64::from((max_frame + 1) as u32) / f64::from(OUTPUT_FPS);

    let document = json!({
        "meta": {
            "duration_seconds": DURATION_SECONDS,
            "output_fps": OUTPUT_FPS,
            "bounce_hz": BOUNCE_HZ,
            "fixed_step_dt": STEP_DT,
            "max_divergence_perframe30_vs_fixed": {
                "value": max_30.0,
                "frame_index": max_30.1,
                "frame_1based": max_30.1 + 1,
                "time_seconds": max_time,
                "chain": if max_30.2 == 0 { "left" } else { "right" },
            },
            "max_divergence_perframe120_vs_fixed": {
                "value": max_120.0,
                "frame_index": max_120.1,
            },
        },
        "modes": {
            "perframe30": samples_json(&per_frame_30),
            "perframe120": samples_json(&per_frame_120),
            "fixedstep": samples_json(&fixed_step),
        },
    });
    std::fs::write(&out_path, document.to_string()).expect("write output json");
    println!(
        "frames per mode: {} / {} / {}",
        per_frame_30.len(),
        per_frame_120.len(),
        fixed_step.len()
    );
    println!(
        "max divergence PF30 vs Fixed: {:.6} at frame {} (t = {:.3} s, {} chain tip)",
        max_30.0,
        max_30.1 + 1,
        max_time,
        if max_30.2 == 0 { "left" } else { "right" }
    );
    println!(
        "max divergence PF120 vs Fixed: {:.6} at frame {}",
        max_120.0,
        max_120.1 + 1
    );
    println!("wrote {out_path}");
}
