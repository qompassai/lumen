//! The engine runtime: a fixed-step clock plus the
//! record-pure sub-runtimes (dialogue, effects, audio
//! bindings) stepped in lockstep. Scene geometry — pose
//! and placement sampling ([`crate::track`]) and the
//! contact seam ([`crate::contact`]) — is evaluated by
//! the host through the same records and the same step
//! counter; this type owns everything that is a pure
//! function of the records and the step.
//!
//! Snapshot/restore contract: [`Runtime::snapshot`] at a
//! step boundary, serialized through
//! [`crate::state::StateRecord`], restores into a fresh
//! runtime whose subsequent event stream is identical to
//! the uninterrupted run's.

use serde::{Deserialize, Serialize};

use crate::audio::{AudioBindings, AudioEvent};
use crate::clock::FixedClock;
use crate::dialogue::{DialogueEvent, DialogueRuntime};
use crate::effect::{EffectEvent, EffectPlayer};
use crate::error::EngineError;
use crate::record::SceneRecord;
use crate::state::{StateRecord, STATE_RECORD_VERSION};

/// Everything one fixed step produced.
#[derive(Clone, Debug, Default, Deserialize, PartialEq, Serialize)]
pub struct StepEvents {
    /// The step these events belong to.
    pub step: u64,
    /// Dialogue events.
    pub dialogue: Vec<DialogueEvent>,
    /// Effect spawn events.
    pub effects: Vec<EffectEvent>,
    /// Audio cue events.
    pub audio: Vec<AudioEvent>,
}

impl StepEvents {
    /// Whether the step produced nothing.
    pub fn is_empty(&self) -> bool {
        self.dialogue.is_empty() && self.effects.is_empty() && self.audio.is_empty()
    }
}

/// The deterministic runtime over one scene record.
#[derive(Debug)]
pub struct Runtime {
    scene_id: String,
    clock: FixedClock,
    dialogue: Option<DialogueRuntime>,
    effects: EffectPlayer,
    audio: AudioBindings,
    audio_fired: u64,
}

impl Runtime {
    /// Build from a validated scene record. Sub-runtime
    /// refusals (dialogue shape, flash law, audio shape)
    /// surface as their typed errors.
    pub fn load(scene: &SceneRecord) -> Result<Self, EngineError> {
        scene.validate()?;
        let dialogue = scene
            .dialogue
            .as_ref()
            .map(DialogueRuntime::new)
            .transpose()?;
        Ok(Self {
            scene_id: scene.id.clone(),
            clock: FixedClock::new(scene.rate)?,
            dialogue,
            effects: EffectPlayer::new(&scene.effects, scene.rate)?,
            audio: AudioBindings::new(&scene.audio)?,
            audio_fired: 0,
        })
    }

    /// Restore a runtime positioned by a state record.
    /// The state must belong to this scene (LE-108).
    pub fn restore(scene: &SceneRecord, state: &StateRecord) -> Result<Self, EngineError> {
        if state.version != STATE_RECORD_VERSION {
            return Err(EngineError::RecordVersion {
                kind: "state",
                found: state.version,
            });
        }
        if state.scene_id != scene.id {
            return Err(EngineError::StateMismatch(format!(
                "state belongs to scene '{}', not '{}'",
                state.scene_id, scene.id
            )));
        }
        let mut runtime = Self::load(scene)?;
        runtime.clock.seek(state.step);
        if let Some(dialogue) = &mut runtime.dialogue {
            dialogue.restore(
                state.dialogue_line,
                state.dialogue_chars,
                state.dialogue_hold,
                state.dialogue_started,
                state.dialogue_done,
            )?;
        }
        runtime.effects.restore_fired(&state.effects_fired)?;
        runtime.audio_fired = state.audio_fired;
        Ok(runtime)
    }

    /// The scene identifier.
    pub fn scene_id(&self) -> &str {
        &self.scene_id
    }

    /// The clock (step counter and rate).
    pub fn clock(&self) -> &FixedClock {
        &self.clock
    }

    /// Run one fixed step: evaluate the sub-runtimes at
    /// the current step, then advance the clock.
    pub fn step(&mut self) -> StepEvents {
        let step = self.clock.step();
        let mut events = StepEvents {
            step,
            ..StepEvents::default()
        };
        if let Some(dialogue) = &mut self.dialogue {
            events.dialogue = dialogue.step(step);
        }
        events.effects = self.effects.step(step);
        events.audio = self.audio.events_at(step);
        self.audio_fired += events.audio.len() as u64;
        self.clock.advance();
        events
    }

    /// The dialogue runtime, when the scene carries one.
    pub fn dialogue(&self) -> Option<&DialogueRuntime> {
        self.dialogue.as_ref()
    }

    /// The effect player (live-particle reads).
    pub fn effects(&self) -> &EffectPlayer {
        &self.effects
    }

    /// Snapshot the runtime position as a state record.
    pub fn snapshot(&self) -> StateRecord {
        let (line, chars, hold, started, done) = self
            .dialogue
            .as_ref()
            .map(DialogueRuntime::snapshot)
            .unwrap_or((0, 0, 0, false, false));
        StateRecord {
            version: STATE_RECORD_VERSION,
            scene_id: self.scene_id.clone(),
            step: self.clock.step(),
            dialogue_line: line,
            dialogue_chars: chars,
            dialogue_hold: hold,
            dialogue_started: started,
            dialogue_done: done,
            effects_fired: self.effects.fired_ids(),
            audio_fired: self.audio_fired,
        }
    }
}
