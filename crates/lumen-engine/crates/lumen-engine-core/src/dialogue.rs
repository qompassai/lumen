//! The dialogue runtime: lines type out on the fixed
//! step at a declared character pace, hold, and advance.
//! The visible text at any step is a pure function of the
//! record and the step counter — hosts render it, they do
//! not time it.

use serde::{Deserialize, Serialize};

use crate::error::EngineError;
use crate::record::DialogueRecord;

/// Events the dialogue runtime emits while stepping.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub enum DialogueEvent {
    /// A line started typing.
    LineStarted {
        /// Line index.
        line: usize,
        /// Speaker name.
        speaker: String,
    },
    /// A line finished typing and entered its hold.
    LineCompleted {
        /// Line index.
        line: usize,
    },
    /// The final line's hold elapsed; the dialogue is done.
    DialogueCompleted,
}

/// The dialogue state machine. Construct from a record,
/// step once per fixed step with the current step counter.
#[derive(Clone, Debug)]
pub struct DialogueRuntime {
    lines: Vec<(String, String, usize)>, // speaker, text, char count
    start_step: u64,
    chars_per_step: usize,
    hold_steps: u64,
    line: usize,
    chars_shown: usize,
    hold_left: u64,
    started: bool,
    done: bool,
}

impl DialogueRuntime {
    /// Build from a record. An empty line list or a zero
    /// character pace is refused (LE-105).
    pub fn new(record: &DialogueRecord) -> Result<Self, EngineError> {
        if record.lines.is_empty() {
            return Err(EngineError::Dialogue("dialogue has no lines".into()));
        }
        if record.chars_per_step == 0 {
            return Err(EngineError::Dialogue(
                "chars_per_step must be at least 1".into(),
            ));
        }
        let lines = record
            .lines
            .iter()
            .map(|l| (l.speaker.clone(), l.text.clone(), l.text.chars().count()))
            .collect();
        Ok(Self {
            lines,
            start_step: record.start_step,
            chars_per_step: record.chars_per_step as usize,
            hold_steps: record.hold_steps,
            line: 0,
            chars_shown: 0,
            hold_left: 0,
            started: false,
            done: false,
        })
    }

    /// Step the machine at absolute step `step`, returning
    /// the events this step produced. Steps before the
    /// start step idle.
    pub fn step(&mut self, step: u64) -> Vec<DialogueEvent> {
        let mut events = Vec::new();
        if self.done || step < self.start_step {
            return events;
        }
        if !self.started {
            self.started = true;
            events.push(DialogueEvent::LineStarted {
                line: 0,
                speaker: self.lines[0].0.clone(),
            });
        }
        if self.hold_left > 0 {
            self.hold_left -= 1;
            if self.hold_left == 0 {
                self.advance_line(&mut events);
            }
            return events;
        }
        let total = self.lines[self.line].2;
        self.chars_shown = (self.chars_shown + self.chars_per_step).min(total);
        if self.chars_shown >= total {
            events.push(DialogueEvent::LineCompleted { line: self.line });
            if self.hold_steps == 0 {
                self.advance_line(&mut events);
            } else {
                self.hold_left = self.hold_steps;
            }
        }
        events
    }

    fn advance_line(&mut self, events: &mut Vec<DialogueEvent>) {
        if self.line + 1 >= self.lines.len() {
            self.done = true;
            events.push(DialogueEvent::DialogueCompleted);
        } else {
            self.line += 1;
            self.chars_shown = 0;
            events.push(DialogueEvent::LineStarted {
                line: self.line,
                speaker: self.lines[self.line].0.clone(),
            });
        }
    }

    /// The currently visible (speaker, text prefix), or
    /// `None` before the start step / after completion.
    pub fn visible(&self) -> Option<(&str, String)> {
        if !self.started || self.done {
            return None;
        }
        let (speaker, text, _) = &self.lines[self.line];
        let prefix: String = text.chars().take(self.chars_shown).collect();
        Some((speaker.as_str(), prefix))
    }

    /// Whether the dialogue has completed.
    pub fn is_done(&self) -> bool {
        self.done
    }

    /// Snapshot fields for the state record: line index,
    /// characters shown, hold steps remaining, started and
    /// done flags. A restore from these fields reproduces
    /// the event stream from the snapshot step exactly.
    pub fn snapshot(&self) -> (u32, u32, u64, bool, bool) {
        (
            self.line as u32,
            self.chars_shown as u32,
            self.hold_left,
            self.started,
            self.done,
        )
    }

    /// Restore snapshot fields. The position must belong
    /// to this runtime's record (LE-105 otherwise).
    pub fn restore(
        &mut self,
        line: u32,
        chars_shown: u32,
        hold_left: u64,
        started: bool,
        done: bool,
    ) -> Result<(), EngineError> {
        let line = line as usize;
        if line >= self.lines.len() {
            return Err(EngineError::Dialogue(format!(
                "restore line {line} out of range"
            )));
        }
        if chars_shown as usize > self.lines[line].2 {
            return Err(EngineError::Dialogue(format!(
                "restore chars {chars_shown} beyond line {line}"
            )));
        }
        self.line = line;
        self.chars_shown = chars_shown as usize;
        self.hold_left = hold_left;
        self.started = started;
        self.done = done;
        Ok(())
    }
}
