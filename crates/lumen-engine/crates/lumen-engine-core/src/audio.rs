//! Audio binding evaluation: at each fixed step, the set
//! of bindings whose schedule covers the step fires as
//! cue events. The core never opens an audio device and
//! never plays anything — it evaluates bindings to data
//! (the estate's audio-silence discipline: generation and
//! verification are silent; the Owner's ear is the gate).

use serde::{Deserialize, Serialize};

use crate::error::EngineError;
use crate::record::AudioBindingRecord;

/// Buses a binding may target.
pub const BUSES: [&str; 3] = ["music", "sfx", "voice"];

/// A cue event fired by a binding at a step.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub struct AudioEvent {
    /// Binding identifier.
    pub binding_id: String,
    /// Cue name.
    pub cue: String,
    /// Bus.
    pub bus: String,
    /// Evaluated gain in dB.
    pub gain_db: f64,
}

/// The set of audio bindings for a scene.
#[derive(Clone, Debug)]
pub struct AudioBindings {
    records: Vec<AudioBindingRecord>,
}

impl AudioBindings {
    /// Build from records. A binding must name a known
    /// bus, carry a cue, and have at least one schedule
    /// (one-shot or window); a window's period must be at
    /// least 1 and its end not before its start (LE-107).
    pub fn new(records: &[AudioBindingRecord]) -> Result<Self, EngineError> {
        for record in records {
            if record.cue.is_empty() {
                return Err(EngineError::Audio(format!(
                    "binding {}: cue is empty",
                    record.id
                )));
            }
            if !BUSES.contains(&record.bus.as_str()) {
                return Err(EngineError::Audio(format!(
                    "binding {}: unknown bus '{}'",
                    record.id, record.bus
                )));
            }
            if record.at_step.is_none() && record.window.is_none() {
                return Err(EngineError::Audio(format!(
                    "binding {}: no schedule (at_step or window required)",
                    record.id
                )));
            }
            if let Some((from, to, every)) = record.window {
                if every == 0 || to < from {
                    return Err(EngineError::Audio(format!(
                        "binding {}: malformed window ({from}, {to}, {every})",
                        record.id
                    )));
                }
            }
        }
        Ok(Self {
            records: records.to_vec(),
        })
    }

    /// The cue events firing at absolute step `step`, in
    /// record order.
    pub fn events_at(&self, step: u64) -> Vec<AudioEvent> {
        self.records
            .iter()
            .filter(|r| {
                r.at_step == Some(step)
                    || r.window.is_some_and(|(from, to, every)| {
                        step >= from && step <= to && (step - from).is_multiple_of(every)
                    })
            })
            .map(|r| AudioEvent {
                binding_id: r.id.clone(),
                cue: r.cue.clone(),
                bus: r.bus.clone(),
                gain_db: r.gain_db,
            })
            .collect()
    }
}
