//! Track sampling: pose keyframe interpolation and the
//! authored placement tracks. The sampling arithmetic
//! reproduces the evaluator seam's scene conventions
//! exactly (per-parameter smoothstep in f32 for poses; the
//! pivot-fold placement formula in f64), because the
//! stress test's gate is bit-level agreement with the
//! reads of record those conventions produced.

use crate::contact::Placement;
use crate::error::EngineError;
use crate::expr::NameSource;
use crate::record::{PlacementTrack, PoseKeyRecord};

/// The seam's smoothstep: clamped cubic in f32.
pub fn smoothstep_f32(t: f32, t0: f32, t1: f32) -> f32 {
    let f = ((t - t0) / (t1 - t0)).clamp(0.0, 1.0);
    f * f * (3.0 - 2.0 * f)
}

/// The seam's smoothstep in f64 (placement eases evaluate
/// through the f32 form in the seam's scenes; see
/// [`sample_step_in_out`]).
pub fn smoothstep_f64(t: f64, t0: f64, t1: f64) -> f64 {
    let f = ((t - t0) / (t1 - t0)).clamp(0.0, 1.0);
    f * f * (3.0 - 2.0 * f)
}

/// A pose key with every value form resolved to a plain
/// f32, ready to sample.
#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedPoseKey {
    /// Key time in seconds (f32, as the seam samples).
    pub t: f32,
    /// (parameter, value) pairs in record order.
    pub params: Vec<(String, f32)>,
}

/// Resolve a record pose track against tune names. Every
/// value form must resolve (LE-103 otherwise); the cast to
/// f32 happens once, here, as the seam's pose maps are f32.
pub fn resolve_pose_track(
    keys: &[PoseKeyRecord],
    names: &dyn NameSource,
) -> Result<Vec<ResolvedPoseKey>, EngineError> {
    let mut out = Vec::with_capacity(keys.len());
    for key in keys {
        let mut params = Vec::with_capacity(key.params.len());
        for (name, form) in &key.params {
            params.push((name.clone(), form.eval(names)? as f32));
        }
        out.push(ResolvedPoseKey {
            t: key.t as f32,
            params,
        });
    }
    Ok(out)
}

/// Sample a resolved pose track at time `t` (seconds):
/// per-parameter smoothstep interpolation between the
/// adjacent keys, parameters defaulting to zero when a
/// key does not name them — the seam's `track_at`,
/// semantics unchanged.
pub fn sample_pose_track(track: &[ResolvedPoseKey], t: f32) -> Vec<(String, [f32; 2])> {
    assert!(!track.is_empty(), "pose track must not be empty");
    if t <= track[0].t {
        return pose_as_sets(&track[0]);
    }
    for pair in track.windows(2) {
        let (k0, k1) = (&pair[0], &pair[1]);
        if t <= k1.t {
            let f = if k1.t > k0.t {
                smoothstep_f32(t, k0.t, k1.t)
            } else {
                0.0
            };
            let mut names: Vec<&str> = Vec::new();
            for (name, _) in k0.params.iter().chain(k1.params.iter()) {
                if !names.contains(&name.as_str()) {
                    names.push(name.as_str());
                }
            }
            return names
                .iter()
                .map(|name| {
                    let va = lookup(&k0.params, name);
                    let vb = lookup(&k1.params, name);
                    ((*name).to_string(), [va + (vb - va) * f, 0.0])
                })
                .collect();
        }
    }
    pose_as_sets(&track[track.len() - 1])
}

fn lookup(params: &[(String, f32)], name: &str) -> f32 {
    params
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| *v)
        .unwrap_or(0.0)
}

fn pose_as_sets(key: &ResolvedPoseKey) -> Vec<(String, [f32; 2])> {
    key.params
        .iter()
        .map(|(name, v)| (name.clone(), [*v, 0.0]))
        .collect()
}

/// Sample a placement track at time `t` (seconds).
/// `ground_lift` and `pivot` are host-measured (see
/// [`PlacementTrack`]); `names` resolves the track's value
/// forms.
pub fn sample_placement(
    track: &PlacementTrack,
    names: &dyn NameSource,
    ground_lift: f64,
    pivot: [f64; 2],
    t: f64,
) -> Result<Placement, EngineError> {
    match track {
        PlacementTrack::Fixed { x, y } => Ok(Placement {
            translation: [*x, *y],
            rotation_degrees: 0.0,
        }),
        PlacementTrack::PivotTurn { keys, compensation } => {
            let compensation = compensation.eval(names)?;
            Ok(sample_pivot_turn(keys, compensation, ground_lift, pivot, t))
        }
        PlacementTrack::StepInOut {
            base_x,
            step_in,
            step_in_window,
            step_back,
            step_back_window,
        } => {
            let base_x = base_x.eval(names)?;
            Ok(sample_step_in_out(
                base_x,
                *step_in,
                *step_in_window,
                *step_back,
                *step_back_window,
                ground_lift,
                t,
            ))
        }
    }
}

/// The pivot-fold placement: rotation interpolated
/// linearly over the keys; translation folds about the
/// pivot so the pivot stays fixed, plus ground lift, plus
/// the compensation pro-rata with |sin| of the rotation
/// against the track's peak rotation — the seam's
/// `a_placement`, semantics unchanged.
pub fn sample_pivot_turn(
    keys: &[(f64, f64)],
    compensation: f64,
    ground_lift: f64,
    pivot: [f64; 2],
    t: f64,
) -> Placement {
    assert!(keys.len() >= 2, "pivot-turn needs at least two keys");
    let mut rotation = keys[keys.len() - 1].1;
    for pair in keys.windows(2) {
        let (t0, r0) = pair[0];
        let (t1, r1) = pair[1];
        if t <= t1 {
            let f = if t1 > t0 { (t - t0) / (t1 - t0) } else { 0.0 };
            rotation = r0 + f * (r1 - r0);
            break;
        }
    }
    if t <= keys[0].0 {
        rotation = keys[0].1;
    }
    let peak = keys.iter().map(|k| k.1.abs()).fold(0.0f64, f64::max);
    let (sin, cos) = rotation.to_radians().sin_cos();
    let rotated = [
        cos * pivot[0] - sin * pivot[1],
        sin * pivot[0] + cos * pivot[1],
    ];
    let lift = if peak > 0.0 {
        compensation * sin.abs() / peak.to_radians().sin()
    } else {
        0.0
    };
    Placement {
        translation: [
            pivot[0] - rotated[0],
            ground_lift + lift + pivot[1] - rotated[1],
        ],
        rotation_degrees: rotation,
    }
}

/// The step-in / hold / step-back placement: eases run
/// through the f32 smoothstep, as the seam's `b_placement`
/// computed them; translation is ground-lifted.
pub fn sample_step_in_out(
    base_x: f64,
    step_in: f64,
    step_in_window: (f64, f64),
    step_back: f64,
    step_back_window: (f64, f64),
    ground_lift: f64,
    t: f64,
) -> Placement {
    let tf = t as f32;
    let in_f = smoothstep_f32(tf, step_in_window.0 as f32, step_in_window.1 as f32);
    let back_f = smoothstep_f32(tf, step_back_window.0 as f32, step_back_window.1 as f32);
    Placement {
        translation: [
            base_x + step_in * (1.0 - f64::from(in_f)) + step_back * f64::from(back_f),
            ground_lift,
        ],
        rotation_degrees: 0.0,
    }
}
