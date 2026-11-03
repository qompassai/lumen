//! Behaviour tests for the VRMC_springBone evaluator:
//! rest stability, gravity, collision response, and the
//! fixed-step determinism gate (SPEC.md section 7).

use lumen_rig_core::spring_bone::{
    ColliderDef, ColliderShape, Frame, JointDef, JointParams, Quat, SpringChainDef, SpringConfig,
    SpringSystem, SpringSystemDef, TimeMode, Vec3,
};

fn params(stiffness: f64, gravity_power: f64, drag_force: f64) -> JointParams {
    JointParams {
        hit_radius: 0.01,
        stiffness,
        gravity_power,
        gravity_dir: Vec3::new(0.0, -1.0, 0.0),
        drag_force,
    }
}

fn joint(node: usize, translation: Vec3, params: JointParams) -> JointDef {
    JointDef {
        node,
        params,
        rest_local_translation: translation,
        rest_local_rotation: Quat::IDENTITY,
    }
}

/// A chain of `count` joints along +x from the anchor, each
/// segment 0.1 long, all sharing `params`.
fn chain_along_x(count: usize, params: JointParams) -> SpringChainDef {
    let joints = (0..count)
        .map(|index| {
            let translation = if index == 0 {
                Vec3::ZERO
            } else {
                Vec3::new(0.1, 0.0, 0.0)
            };
            joint(index, translation, params)
        })
        .collect();
    SpringChainDef {
        joints,
        collider_groups: Vec::new(),
        center_node: None,
    }
}

fn system(def: SpringSystemDef) -> SpringSystem {
    SpringSystem::new(def, &Frame::IDENTITY).expect("test system builds")
}

fn tails(system: &SpringSystem, chain: usize) -> Vec<Vec3> {
    let mut out = Vec::new();
    system.current_tails(chain, &mut out);
    out
}

/// FNV-1a over the f64 bit patterns of every tail and every
/// local rotation, in declaration order — the state hash of
/// the determinism gate.
fn state_hash(system: &SpringSystem) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let mut mix = |bits: u64| {
        for byte in bits.to_le_bytes() {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    for chain in 0..system.chain_count() {
        for tail in tails(system, chain) {
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

#[test]
fn rest_state_is_bit_stable_without_forces() {
    let def = SpringSystemDef {
        springs: vec![chain_along_x(4, params(0.0, 0.0, 0.5))],
        ..SpringSystemDef::default()
    };
    let mut system = system(def);
    let before = state_hash(&system);
    for _ in 0..60 {
        system.step(&Frame::IDENTITY, &[], 1.0 / 60.0);
    }
    assert_eq!(
        state_hash(&system),
        before,
        "a forceless rest chain must not drift"
    );
    for (index, rotation) in system.local_rotations(0).iter().enumerate() {
        assert_eq!(*rotation, Quat::IDENTITY, "joint {index} rotated at rest");
    }
}

#[test]
fn gravity_pulls_tail_down_and_preserves_bone_length() {
    let def = SpringSystemDef {
        springs: vec![chain_along_x(2, params(0.0, 1.0, 0.2))],
        ..SpringSystemDef::default()
    };
    let mut system = system(def);
    for _ in 0..30 {
        system.step(&Frame::IDENTITY, &[], 1.0 / 60.0);
    }
    let tail = tails(&system, 0)[0];
    assert!(
        tail.y < -0.01,
        "tail should sag under gravity, got {tail:?}"
    );
    let length = (tail.x * tail.x + tail.y * tail.y + tail.z * tail.z).sqrt();
    assert!(
        (length - 0.1).abs() < 1e-9,
        "bone length must be preserved, got {length}"
    );
}

#[test]
fn stiffness_returns_tail_toward_rest_direction() {
    // Displace the chain by moving the anchor sideways for
    // one step, then hold the anchor still: with stiffness
    // the tail's deviation from the rest direction decays
    // further than without stiffness over the same steps.
    let run = |stiffness: f64| {
        let def = SpringSystemDef {
            springs: vec![chain_along_x(3, params(stiffness, 0.0, 0.4))],
            ..SpringSystemDef::default()
        };
        let mut system = system(def);
        let shifted = Frame::new(Vec3::new(0.0, 0.0, 0.2), Quat::IDENTITY);
        system.step(&shifted, &[], 1.0 / 60.0);
        for _ in 0..120 {
            system.step(&Frame::IDENTITY, &[], 1.0 / 60.0);
        }
        // Deviation of the last tail from its rest position.
        let rest = Vec3::new(0.2, 0.0, 0.0);
        let tail = tails(&system, 0)[1];
        (tail.x - rest.x).abs() + (tail.y - rest.y).abs() + (tail.z - rest.z).abs()
    };
    assert!(
        run(2.0) < run(0.0),
        "stiffness should reduce the residual deviation"
    );
}

#[test]
fn sphere_collider_pushes_tail_out() {
    // The sphere sits just above the tail's rest position;
    // the tail starts inside its expanded surface and must
    // be pushed away (downward, the push direction).
    let def = SpringSystemDef {
        colliders: vec![ColliderDef {
            node: 90,
            shape: ColliderShape::Sphere {
                offset: Vec3::new(0.1, 0.02, 0.0),
                radius: 0.05,
            },
        }],
        collider_groups: vec![vec![0]],
        springs: vec![SpringChainDef {
            collider_groups: vec![0],
            ..chain_along_x(2, params(0.0, 0.0, 0.5))
        }],
    };
    let mut system = system(def);
    let frames = [Frame::IDENTITY];
    system.step(&Frame::IDENTITY, &frames, 1.0 / 60.0);
    let tail = tails(&system, 0)[0];
    assert!(
        tail.y < -0.01,
        "collider should push the tail down, got {tail:?}"
    );
}

#[test]
fn capsule_collider_pushes_tail_out() {
    // A vertical capsule spine at x = 0.1; the tail rests
    // 0.02 from the spine in +z, inside the capsule radius.
    let mut chain = chain_along_x(2, params(0.0, 0.0, 0.5));
    chain.joints[1].rest_local_translation = Vec3::new(0.1, 0.0, 0.02);
    chain.collider_groups = vec![0];
    let def = SpringSystemDef {
        colliders: vec![ColliderDef {
            node: 90,
            shape: ColliderShape::Capsule {
                offset: Vec3::new(0.1, -0.05, 0.0),
                radius: 0.04,
                tail: Vec3::new(0.1, 0.05, 0.0),
            },
        }],
        collider_groups: vec![vec![0]],
        springs: vec![chain],
    };
    let mut system = system(def);
    let frames = [Frame::IDENTITY];
    system.step(&Frame::IDENTITY, &frames, 1.0 / 60.0);
    let tail = tails(&system, 0)[0];
    assert!(
        tail.z > 0.03,
        "capsule should push the tail out in +z, got {tail:?}"
    );
}

/// A scripted anchor sequence: swings in +x, then holds.
/// Pure function of the call index, so two runs see
/// identical inputs.
fn scripted_anchor(call: u64) -> Frame {
    let phase = (call % 120) as f64 / 120.0;
    let x = if phase < 0.5 {
        0.2 * (phase * std::f64::consts::TAU).sin()
    } else {
        0.0
    };
    Frame::new(Vec3::new(x, 1.4, 0.0), Quat::IDENTITY)
}

fn scripted_system(config: SpringConfig) -> SpringSystem {
    let def = SpringSystemDef {
        springs: vec![
            chain_along_x(5, params(0.9, 0.7, 0.35)),
            chain_along_x(4, params(1.4, 0.9, 0.3)),
        ],
        ..SpringSystemDef::default()
    };
    SpringSystem::with_config(def, &Frame::IDENTITY, config).expect("scripted system builds")
}

#[test]
fn fixed_step_replay_is_bit_identical() {
    // The determinism gate: the same recorded call sequence
    // through advance() in fixed-step mode, twice, must
    // produce identical state hashes after every call.
    let run = || {
        let mut system = scripted_system(SpringConfig::default());
        let mut hashes = Vec::new();
        for call in 0..240_u64 {
            let anchor = scripted_anchor(call);
            let frame_dt = match call % 3 {
                0 => 1.0 / 30.0,
                1 => 1.0 / 60.0,
                _ => 1.0 / 120.0,
            };
            system.advance(&anchor, &[], frame_dt);
            hashes.push(state_hash(&system));
        }
        hashes
    };
    assert_eq!(run(), run(), "fixed-step replay must be bit-identical");
}

#[test]
fn fixed_step_counts_whole_steps_and_carries_the_remainder() {
    let mut system = scripted_system(SpringConfig::default());
    let anchor = Frame::IDENTITY;
    assert_eq!(system.advance(&anchor, &[], 1.0 / 30.0), 4);
    assert_eq!(system.advance(&anchor, &[], 1.0 / 120.0), 1);
    assert_eq!(system.advance(&anchor, &[], 1.0 / 240.0), 0);
    assert_eq!(system.advance(&anchor, &[], 1.0 / 240.0), 1);
    assert_eq!(system.step_count(), 6);
}

#[test]
fn per_frame_mode_steps_once_with_the_frame_dt() {
    let config = SpringConfig {
        mode: TimeMode::PerFrame,
        step_dt_seconds: 1.0 / 120.0,
    };
    let mut system = scripted_system(config);
    assert_eq!(system.advance(&Frame::IDENTITY, &[], 0.5), 1);
    assert_eq!(system.step_count(), 1);
    assert_eq!(system.stepped_seconds(), 0.5);
}
