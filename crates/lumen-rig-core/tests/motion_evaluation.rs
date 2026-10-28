//! Motion evaluation tests: the section 4.2 interpolation
//! formulas and the section 4.3 fade law, checked against
//! independent analytic expectations (never against the
//! implementation's own output), plus fixed-step determinism.

use std::fs;
use std::path::Path;

use lumen_rig_core::{
    Interpolation, Keyframe, Model, MotionPlayer, curve_value_at, parse_model, parse_motion,
};

fn fixture(relative: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("fixtures")
        .join(relative);
    fs::read(&path).unwrap_or_else(|error| panic!("fixture {relative}: {error}"))
}

fn full_motion() -> (Model, lumen_rig_core::Motion) {
    let model = parse_model(&fixture("full.model.json")).expect("full model parses");
    let motion = parse_motion(&fixture("full.motion.json"), &model).expect("full motion parses");
    (model, motion)
}

fn key(time: f32, value: f32, interpolation: Interpolation) -> Keyframe {
    Keyframe {
        time,
        value,
        interpolation,
        tangent_in: None,
        tangent_out: None,
    }
}

#[test]
fn linear_interpolation_is_exact() {
    let keys = [
        key(0.0, 0.0, Interpolation::Linear),
        key(2.0, 10.0, Interpolation::Linear),
    ];
    assert_eq!(curve_value_at(&keys, 0.0), 0.0);
    assert_eq!(curve_value_at(&keys, 0.5), 2.5);
    assert_eq!(curve_value_at(&keys, 1.0), 5.0);
    assert_eq!(curve_value_at(&keys, 2.0), 10.0);
}

#[test]
fn stepped_holds_until_the_next_segment_owns_the_instant() {
    let keys = [
        key(0.0, 1.0, Interpolation::Stepped),
        key(1.0, 2.0, Interpolation::Stepped),
        key(2.0, 3.0, Interpolation::Linear),
    ];
    assert_eq!(curve_value_at(&keys, 0.999), 1.0);
    assert_eq!(curve_value_at(&keys, 1.0), 2.0);
    assert_eq!(curve_value_at(&keys, 1.5), 2.0);
    assert_eq!(curve_value_at(&keys, 2.0), 3.0);
}

#[test]
fn curve_holds_endpoints_outside_its_keys() {
    let keys = [
        key(0.5, 4.0, Interpolation::Linear),
        key(1.5, 8.0, Interpolation::Linear),
    ];
    assert_eq!(curve_value_at(&keys, 0.0), 4.0);
    assert_eq!(curve_value_at(&keys, 0.25), 4.0);
    assert_eq!(curve_value_at(&keys, 3.0), 8.0);
}

#[test]
fn cubic_matches_the_catmull_rom_polynomial() {
    // The handoff's exactness claim: a Catmull-Rom lane
    // converted with per-key tangents dv = (v[k+1] - v[k-1]) / 6
    // and dt = segment / 3 evaluates, under section 4.2, to the
    // Catmull-Rom (Hermite) polynomial itself. The expectation
    // here is that polynomial, computed independently.
    let times = [0.0_f64, 0.5, 1.25, 2.0];
    let values = [0.0_f64, 10.0, -5.0, 2.5];
    let mut keys: Vec<Keyframe> = Vec::new();
    for k in 0..4_usize {
        let prev = values[k.saturating_sub(1)];
        let next = values[(k + 1).min(3)];
        let dv = (next - prev) / 6.0;
        let tangent_in = if k == 0 {
            None
        } else {
            Some([((times[k] - times[k - 1]) / 3.0) as f32, dv as f32])
        };
        let tangent_out = if k == 3 {
            None
        } else {
            Some([((times[k + 1] - times[k]) / 3.0) as f32, dv as f32])
        };
        keys.push(Keyframe {
            time: times[k] as f32,
            value: values[k] as f32,
            interpolation: Interpolation::Cubic,
            tangent_in,
            tangent_out,
        });
    }
    // Hermite expectation per segment, in segment-fraction
    // space: the engine's Catmull-Rom works on the normalized
    // segment fraction u, with slopes m = (v[k+1] - v[k-1]) / 2
    // per fraction (not per second). That is exactly why the
    // bridge's dv = (v[k+1] - v[k-1]) / 6 conversion is exact
    // at any time spacing: the Bezier control offset is m / 3
    // in the same fraction space, and dt = segment / 3 makes
    // x(u) linear in u.
    let hermite = |segment: usize, u: f64| -> f64 {
        let v0 = values[segment];
        let v1 = values[segment + 1];
        let m0 = (values[(segment + 1).min(3)] - values[segment.saturating_sub(1)]) / 2.0;
        let m1 = (values[(segment + 2).min(3)] - values[segment]) / 2.0;
        let u2 = u * u;
        let u3 = u2 * u;
        (2.0 * u3 - 3.0 * u2 + 1.0) * v0
            + (u3 - 2.0 * u2 + u) * m0
            + (-2.0 * u3 + 3.0 * u2) * v1
            + (u3 - u2) * m1
    };
    let mut t = 0.0_f64;
    while t <= 2.0 {
        let segment = if t < times[1] {
            0
        } else if t < times[2] {
            1
        } else {
            2
        };
        let u = (t - times[segment]) / (times[segment + 1] - times[segment]);
        let expected = hermite(segment, u);
        let actual = curve_value_at(&keys, t);
        assert!(
            (actual - expected).abs() < 1e-5,
            "t={t}: evaluated {actual}, Catmull-Rom {expected}"
        );
        t += 0.0437;
    }
}

#[test]
fn cubic_with_missing_tangents_uses_zero_control_offsets() {
    let mut from = key(0.0, 0.0, Interpolation::Cubic);
    let mut to = key(1.0, 1.0, Interpolation::Linear);
    from.tangent_out = None;
    to.tangent_in = None;
    let keys = [from, to];
    // Control points (0,0), (0,0), (1,1), (1,1): the Bezier
    // through them at u where x(u) = 0.5 is u = 0.5 exactly
    // (x(u) = 3u^2 - 2u^3 is symmetric), so y = 0.5.
    assert!((curve_value_at(&keys, 0.5) - 0.5).abs() < 1e-9);
    assert_eq!(curve_value_at(&keys, 0.0), 0.0);
    assert_eq!(curve_value_at(&keys, 1.0), 1.0);
}

#[test]
fn fade_in_ramps_linearly_and_blends_deviation_not_value() {
    let (_model, motion) = full_motion();
    let mut player = MotionPlayer::new(&motion);
    player.start();
    // angle_x carries fade_in 0.5: at 0.25 s the weight is 0.5.
    // Steps of 1/128 s sum exactly in binary, so the weights
    // below are exact, not approximate.
    for _ in 0..32 {
        player.advance(1.0 / 128.0);
    }
    let evaluated = player.evaluate();
    let angle_x = evaluated
        .iter()
        .find(|entry| entry.parameter == "angle_x")
        .expect("angle_x contribution");
    assert!(
        (angle_x.weight - 0.5).abs() < 1e-9,
        "weight {}",
        angle_x.weight
    );
    // The blend law: weight multiplies deviation from base.
    assert_eq!(MotionPlayer::blend(2.0, 0.5, 10.0), 6.0);
    assert_eq!(MotionPlayer::blend(2.0, 0.0, 10.0), 2.0);
    assert_eq!(MotionPlayer::blend(2.0, 1.0, 10.0), 10.0);
    for _ in 0..32 {
        player.advance(1.0 / 128.0);
    }
    let evaluated = player.evaluate();
    let angle_x = evaluated
        .iter()
        .find(|entry| entry.parameter == "angle_x")
        .expect("angle_x contribution");
    assert_eq!(angle_x.weight, 1.0);
}

#[test]
fn fade_out_runs_to_zero_and_the_clock_stops() {
    let (_model, motion) = full_motion();
    let mut player = MotionPlayer::new(&motion);
    player.start();
    for _ in 0..128 {
        player.advance(1.0 / 128.0);
    }
    player.stop();
    // angle_x fade_out is 0.25 s: 32 steps at 1/128.
    for _ in 0..32 {
        player.advance(1.0 / 128.0);
    }
    let evaluated = player.evaluate();
    let angle_x = evaluated
        .iter()
        .find(|entry| entry.parameter == "angle_x")
        .expect("angle_x contribution");
    assert_eq!(angle_x.weight, 0.0);
    assert!(!player.is_advancing());
    let frozen = player.time_seconds();
    player.advance(1.0 / 128.0);
    assert_eq!(player.time_seconds(), frozen);
}

#[test]
fn interrupted_fades_restart_from_the_current_weight() {
    let (_model, motion) = full_motion();
    let mut player = MotionPlayer::new(&motion);
    player.start();
    for _ in 0..32 {
        player.advance(1.0 / 128.0);
    }
    player.stop();
    for _ in 0..16 {
        player.advance(1.0 / 128.0);
    }
    let evaluated = player.evaluate();
    let angle_x = evaluated
        .iter()
        .find(|entry| entry.parameter == "angle_x")
        .expect("angle_x contribution");
    // Halfway through the 0.25 s fade-out from weight 0.5.
    assert!((angle_x.weight - 0.25).abs() < 1e-9);
    player.start();
    for _ in 0..32 {
        player.advance(1.0 / 128.0);
    }
    let evaluated = player.evaluate();
    let angle_x = evaluated
        .iter()
        .find(|entry| entry.parameter == "angle_x")
        .expect("angle_x contribution");
    // 0.25 s into a 0.5 s fade-in that started at weight 0.25:
    // 0.25 + 0.75 * 0.5 = 0.625.
    assert!((angle_x.weight - 0.625).abs() < 1e-9);
}

#[test]
fn zero_fades_land_instantly() {
    let model = parse_model(&fixture("minimal.model.json")).expect("minimal model parses");
    let motion = parse_motion(&fixture("minimal.motion.json"), &model).expect("motion parses");
    let mut player = MotionPlayer::new(&motion);
    player.start();
    let evaluated = player.evaluate();
    assert_eq!(evaluated[0].weight, 1.0);
    player.stop();
    let evaluated = player.evaluate();
    assert_eq!(evaluated[0].weight, 0.0);
    assert!(!player.is_advancing());
}

#[test]
fn looping_motion_wraps_without_restarting_fades() {
    let (_model, motion) = full_motion();
    let mut player = MotionPlayer::new(&motion);
    player.start();
    player.advance(0.25);
    player.seek(1.75);
    player.advance(0.5);
    assert_eq!(player.time_seconds(), 0.25);
    let evaluated = player.evaluate();
    let angle_x = evaluated
        .iter()
        .find(|entry| entry.parameter == "angle_x")
        .expect("angle_x contribution");
    assert_eq!(angle_x.weight, 1.0, "a loop wrap is not a new start");
}

#[test]
fn non_looping_motion_clamps_and_holds() {
    let model = parse_model(&fixture("minimal.model.json")).expect("minimal model parses");
    let motion = parse_motion(&fixture("minimal.motion.json"), &model).expect("motion parses");
    let mut player = MotionPlayer::new(&motion);
    player.start();
    for _ in 0..300 {
        player.advance(1.0 / 60.0);
    }
    assert_eq!(player.time_seconds(), 1.0);
    let evaluated = player.evaluate();
    assert_eq!(evaluated[0].value, 15.0);
}

/// FNV-1a over the evaluated trace: the determinism instrument
/// shared with the utilization example (same law, both places).
fn trace_hash(player: &MotionPlayer) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let mut mix = |bytes: &[u8]| {
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    mix(&player.time_seconds().to_bits().to_le_bytes());
    for entry in player.evaluate() {
        mix(entry.parameter.as_bytes());
        mix(&entry.weight.to_bits().to_le_bytes());
        mix(&entry.value.to_bits().to_le_bytes());
    }
    hash
}

#[test]
fn fixed_step_replay_is_bit_identical() {
    let (_model, motion) = full_motion();
    let run = || {
        let mut player = MotionPlayer::new(&motion);
        let mut hashes = Vec::new();
        player.start();
        for _ in 0..240 {
            player.advance(1.0 / 120.0);
            hashes.push(trace_hash(&player));
        }
        player.stop();
        for _ in 0..60 {
            player.advance(1.0 / 120.0);
            hashes.push(trace_hash(&player));
        }
        hashes
    };
    let first = run();
    let second = run();
    assert_eq!(first, second);
    assert_eq!(first.len(), 300);
}

#[test]
fn contributions_come_in_contractual_order() {
    let (_model, motion) = full_motion();
    let player = MotionPlayer::new(&motion);
    let evaluated = player.evaluate();
    let names: Vec<&str> = evaluated
        .iter()
        .map(|entry| entry.parameter.as_str())
        .collect();
    assert_eq!(names, ["angle_x", "angle_y", "gaze", "mouth_open"]);
}
