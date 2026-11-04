//! The fixed-step clock. Engine time is a step counter at
//! a declared rate — never a wall clock — so a recorded
//! step sequence replays identically in any process
//! (SPEC section 4.1: determinism is the currency).

use crate::error::EngineError;

/// A fixed-step clock: `step` ticks at `rate` steps per
/// second. Time reads are exact divisions of the counter,
/// matching the evaluator seam's convention
/// (`time_seconds = step / rate`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FixedClock {
    rate: u32,
    step: u64,
}

impl FixedClock {
    /// A clock at step 0 ticking `rate` times per second.
    /// A zero rate is refused (LE-109).
    pub fn new(rate: u32) -> Result<Self, EngineError> {
        if rate == 0 {
            return Err(EngineError::Clock("step rate must be at least 1".into()));
        }
        Ok(Self { rate, step: 0 })
    }

    /// Steps per second.
    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// The current step counter.
    pub fn step(&self) -> u64 {
        self.step
    }

    /// Current time in seconds: `step / rate`, computed in
    /// f64 exactly as the contact seam's frame inputs are.
    pub fn time_seconds(&self) -> f64 {
        self.step as f64 / f64::from(self.rate)
    }

    /// Current time in seconds as f32 (pose-track sampling
    /// works in f32, as the puppet seam does).
    pub fn time_f32(&self) -> f32 {
        self.time_seconds() as f32
    }

    /// One step of fixed duration: `1 / rate` seconds.
    pub fn dt_seconds(&self) -> f32 {
        1.0 / self.rate as f32
    }

    /// Advance one step.
    pub fn advance(&mut self) {
        self.step += 1;
    }

    /// Move the counter to an absolute step (state restore).
    pub fn seek(&mut self, step: u64) {
        self.step = step;
    }
}
