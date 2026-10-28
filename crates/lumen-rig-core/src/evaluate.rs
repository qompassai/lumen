//! The motion evaluator: SPEC.md section 4.2 curve sampling
//! and the section 4.3 fade law, over a fixed-step clock.
//!
//! Determinism is contractual (section 7): evaluation is a
//! pure function of the validated motion, the step sequence,
//! and nothing else — no wall-clock, no map iteration, no
//! platform intrinsics. All arithmetic is f64; values round to
//! f32 once, at the host's write into its parameter state (the
//! sanctioned clamp of section 9.3 is the host's write too).
//! The iteration counts in the cubic solver are part of the
//! format: an evaluator that iterates differently produces
//! different bits and is non-conforming.

use crate::motion::{Contribution, Interpolation, Keyframe, Motion};

/// Newton–Raphson iteration cap for the cubic inverse
/// (SPEC.md section 4.2). Part of the format.
const NEWTON_ITERATIONS_MAX: usize = 8;
/// Bisection iteration count for the cubic inverse fallback
/// (SPEC.md section 4.2). Part of the format.
const BISECTION_ITERATIONS: usize = 24;
/// Convergence tolerance of the cubic inverse, in seconds.
const SOLVE_TOLERANCE_SECONDS: f64 = 1e-6;

/// Evaluate one curve's keyframes at `time_seconds` (f64).
///
/// Before the first key and after the last key the curve holds
/// the endpoint value; nothing extrapolates (section 4.1). At
/// a segment's closing instant the NEXT segment owns the
/// instant (the stepped rule, applied uniformly: sampling is
/// continuous from the right at interior keys).
pub fn curve_value_at(keyframes: &[Keyframe], time_seconds: f64) -> f64 {
    assert!(
        !keyframes.is_empty(),
        "a validated curve carries at least one keyframe"
    );
    let first = &keyframes[0];
    let last = &keyframes[keyframes.len() - 1];
    if time_seconds <= f64::from(first.time) {
        return f64::from(first.value);
    }
    if time_seconds >= f64::from(last.time) {
        return f64::from(last.value);
    }
    // The segment whose closing key is the first key after t.
    // Keyframe counts are bounded (KEYFRAMES_PER_CURVE_MAX) and
    // this scan is over validated, strictly increasing times.
    let mut segment = 0;
    while time_seconds >= f64::from(keyframes[segment + 1].time) {
        segment += 1;
    }
    let from = &keyframes[segment];
    let to = &keyframes[segment + 1];
    let t0 = f64::from(from.time);
    let t1 = f64::from(to.time);
    let v0 = f64::from(from.value);
    let v1 = f64::from(to.value);
    match from.interpolation {
        Interpolation::Stepped => v0,
        Interpolation::Linear => v0 + (v1 - v0) * (time_seconds - t0) / (t1 - t0),
        Interpolation::Cubic => cubic_value(from, to, time_seconds),
    }
}

/// The cubic segment: a Bézier in (time, value) space whose
/// control points are the keys plus their tangents (section
/// 4.2). Solves `x(u) = t` by Newton–Raphson from the segment
/// fraction, then bisection, with the format's exact iteration
/// counts; the value is `y(u)`.
fn cubic_value(from: &Keyframe, to: &Keyframe, time_seconds: f64) -> f64 {
    let (dt_out, dv_out) = tangent_or_zero(from.tangent_out);
    let (dt_in, dv_in) = tangent_or_zero(to.tangent_in);
    let t0 = f64::from(from.time);
    let t1 = f64::from(to.time);
    let v0 = f64::from(from.value);
    let v1 = f64::from(to.value);
    let px = [t0, t0 + dt_out, t1 - dt_in, t1];
    let py = [v0, v0 + dv_out, v1 - dv_in, v1];
    let mut u = (time_seconds - t0) / (t1 - t0);
    let mut converged = false;
    for _ in 0..NEWTON_ITERATIONS_MAX {
        let error = bezier(px, u) - time_seconds;
        if error.abs() <= SOLVE_TOLERANCE_SECONDS {
            converged = true;
            break;
        }
        let derivative = bezier_time_derivative(px, u);
        if derivative == 0.0 {
            break;
        }
        u -= error / derivative;
    }
    if !converged {
        let (mut lo, mut hi) = (0.0_f64, 1.0_f64);
        for _ in 0..BISECTION_ITERATIONS {
            u = (lo + hi) / 2.0;
            if bezier(px, u) < time_seconds {
                lo = u;
            } else {
                hi = u;
            }
        }
        u = (lo + hi) / 2.0;
    }
    bezier(py, u)
}

/// A missing tangent defaults to `[0, 0]` (section 4.2).
fn tangent_or_zero(tangent: Option<[f32; 2]>) -> (f64, f64) {
    match tangent {
        Some([dt, dv]) => (f64::from(dt), f64::from(dv)),
        None => (0.0, 0.0),
    }
}

/// The cubic Bézier over control values `p` at parameter `u`.
fn bezier(p: [f64; 4], u: f64) -> f64 {
    let w = 1.0 - u;
    w * w * w * p[0] + 3.0 * w * w * u * p[1] + 3.0 * w * u * u * p[2] + u * u * u * p[3]
}

/// The derivative of the Bézier time component at `u`.
fn bezier_time_derivative(px: [f64; 4], u: f64) -> f64 {
    let w = 1.0 - u;
    3.0 * w * w * (px[1] - px[0]) + 6.0 * w * u * (px[2] - px[1]) + 3.0 * u * u * (px[3] - px[2])
}

/// One contribution's evaluated product at the player's current
/// time: the parameter it targets, its current fade weight,
/// and the curve's raw value. The host blends with section
/// 4.3's law: `base + weight * (value - base)`, where `base`
/// is the value the parameter would hold without this motion.
#[derive(Debug, Clone, PartialEq)]
pub struct EvaluatedContribution {
    pub parameter: String,
    pub weight: f64,
    pub value: f64,
}

/// Where one contribution's fade envelope stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FadePhase {
    /// Ramping toward weight 1 over `fade_in_seconds`.
    In,
    /// Fully faded in; weight is 1.
    Active,
    /// Ramping toward weight 0 over `fade_out_seconds`.
    Out,
    /// Fully faded out; weight is 0 and the contribution no
    /// longer advances the player's clock.
    Done,
}

/// One contribution's runtime fade state. Ramps are linear in
/// f64 over the fixed-step clock (section 4.3); an interrupted
/// ramp restarts from the weight at the interruption instant,
/// over the full named fade duration.
#[derive(Debug, Clone, Copy)]
struct FadeState {
    phase: FadePhase,
    weight: f64,
    ramp_from: f64,
    ramp_elapsed_seconds: f64,
}

impl FadeState {
    fn new() -> FadeState {
        FadeState {
            phase: FadePhase::Done,
            weight: 0.0,
            ramp_from: 0.0,
            ramp_elapsed_seconds: 0.0,
        }
    }

    /// Begin (or re-begin) the fade-in ramp from the current
    /// weight. A zero fade duration lands instantly.
    fn start(&mut self, fade_in_seconds: f32) {
        if fade_in_seconds <= 0.0 {
            self.phase = FadePhase::Active;
            self.weight = 1.0;
            return;
        }
        self.phase = FadePhase::In;
        self.ramp_from = self.weight;
        self.ramp_elapsed_seconds = 0.0;
    }

    /// Begin the fade-out ramp from the current weight.
    fn stop(&mut self, fade_out_seconds: f32) {
        if self.phase == FadePhase::Done {
            return;
        }
        if fade_out_seconds <= 0.0 {
            self.phase = FadePhase::Done;
            self.weight = 0.0;
            return;
        }
        self.phase = FadePhase::Out;
        self.ramp_from = self.weight;
        self.ramp_elapsed_seconds = 0.0;
    }

    /// Advance the ramp by one fixed step.
    fn advance(&mut self, contribution: Contribution<'_>, dt_seconds: f64) {
        let (target, duration) = match self.phase {
            FadePhase::In => (1.0, f64::from(contribution.fade_in_seconds)),
            FadePhase::Out => (0.0, f64::from(contribution.fade_out_seconds)),
            FadePhase::Active | FadePhase::Done => return,
        };
        self.ramp_elapsed_seconds += dt_seconds;
        let fraction = (self.ramp_elapsed_seconds / duration).min(1.0);
        self.weight = self.ramp_from + (target - self.ramp_from) * fraction;
        if fraction >= 1.0 {
            self.phase = if target == 1.0 {
                FadePhase::Active
            } else {
                FadePhase::Done
            };
        }
    }
}

/// A playing (or fading) motion: the fixed-step runtime for one
/// validated `Motion`.
///
/// Contract: the host advances the player with a fixed `dt`
/// of its declared step (section 7.5) and reads contributions
/// after each step. The player owns no clock and no state
/// beyond the motion time and the fade envelopes, so a
/// recorded step sequence replays bit-identically on any
/// conforming machine.
#[derive(Debug)]
pub struct MotionPlayer<'a> {
    motion: &'a Motion,
    contributions: Vec<Contribution<'a>>,
    fades: Vec<FadeState>,
    time_seconds: f64,
    playing: bool,
}

impl<'a> MotionPlayer<'a> {
    /// A player for `motion`, stopped at time 0 with every
    /// contribution's weight at 0.
    pub fn new(motion: &'a Motion) -> MotionPlayer<'a> {
        let contributions = motion.contributions();
        let fades = vec![FadeState::new(); contributions.len()];
        MotionPlayer {
            motion,
            contributions,
            fades,
            time_seconds: 0.0,
            playing: false,
        }
    }

    /// Start (or resume) the motion: every contribution begins
    /// its fade-in ramp from its current weight (section 4.3's
    /// interruption rule). Does not reset the motion time —
    /// restarting time is the host's explicit act
    /// (`seek(0.0)`), so a resume cannot silently rewind.
    pub fn start(&mut self) {
        self.playing = true;
        for (fade, contribution) in self.fades.iter_mut().zip(self.contributions.iter()) {
            fade.start(contribution.fade_in_seconds);
        }
    }

    /// End the motion: every contribution begins its fade-out
    /// ramp from its current weight. The clock keeps advancing
    /// while any contribution is still fading out, so curves
    /// keep moving under their dying weights; it stops when
    /// the last contribution reaches weight 0.
    pub fn stop(&mut self) {
        self.playing = false;
        for (fade, contribution) in self.fades.iter_mut().zip(self.contributions.iter()) {
            fade.stop(contribution.fade_out_seconds);
        }
    }

    /// Set the motion time directly, clamped to
    /// `[0, duration_seconds]`. Fade state is untouched: a seek
    /// moves the curves, not their envelopes.
    pub fn seek(&mut self, time_seconds: f64) {
        assert!(
            time_seconds.is_finite(),
            "seek time must be finite, got {time_seconds}"
        );
        self.time_seconds = time_seconds.clamp(0.0, f64::from(self.motion.duration_seconds));
    }

    /// The current motion time, in seconds.
    pub fn time_seconds(&self) -> f64 {
        self.time_seconds
    }

    /// True while the clock advances: the motion is playing,
    /// or stopped but still fading out.
    pub fn is_advancing(&self) -> bool {
        self.playing || self.fades.iter().any(|fade| fade.phase == FadePhase::Out)
    }

    /// Advance the fixed-step clock by `dt_seconds` (must be
    /// finite and positive — the host's declared step, never a
    /// measured frame time). A looping motion wraps its time
    /// modulo the duration (fades are NOT restarted: looping
    /// is a player argument, not a new start). A non-looping
    /// motion clamps at its duration and holds.
    pub fn advance(&mut self, dt_seconds: f64) {
        assert!(
            dt_seconds.is_finite() && dt_seconds > 0.0,
            "dt must be finite and positive, got {dt_seconds}"
        );
        if !self.is_advancing() {
            return;
        }
        self.time_seconds += dt_seconds;
        let duration = f64::from(self.motion.duration_seconds);
        if self.motion.looping {
            self.time_seconds = self.time_seconds.rem_euclid(duration);
        } else if self.time_seconds > duration {
            self.time_seconds = duration;
        }
        for (fade, contribution) in self.fades.iter_mut().zip(self.contributions.iter()) {
            fade.advance(*contribution, dt_seconds);
        }
    }

    /// Every contribution evaluated at the current time, in the
    /// motion's contractual order (plain curves, then baked
    /// sets). Weight 0 contributions are included — the host
    /// sees the whole envelope, and a zero weight blends to the
    /// base value exactly.
    pub fn evaluate(&self) -> Vec<EvaluatedContribution> {
        self.contributions
            .iter()
            .zip(self.fades.iter())
            .map(|(contribution, fade)| EvaluatedContribution {
                parameter: contribution.parameter.to_string(),
                weight: fade.weight,
                value: curve_value_at(contribution.keyframes, self.time_seconds),
            })
            .collect()
    }

    /// The section 4.3 blend law, named once so hosts do not
    /// re-derive it: a weight multiplies the curve's deviation
    /// from the parameter's base value, never the absolute
    /// value.
    pub fn blend(base: f64, weight: f64, value: f64) -> f64 {
        base + weight * (value - base)
    }
}
