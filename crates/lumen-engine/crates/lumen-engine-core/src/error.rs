//! Typed refusals for the engine core. Every failure the
//! core can produce is one of these variants; the `LE-`
//! codes are stable identifiers for gate output and logs
//! (the estate's error-code discipline, as in lumen-rig-core's
//! `LTB-` seam codes).

use std::fmt;

/// Error prefix for every code in this crate.
pub const ERROR_PREFIX: &str = "LE";

#[derive(Debug)]
pub enum EngineError {
    /// LE-101: a record failed strict parsing.
    RecordParse(String),
    /// LE-102: a record carries an unsupported version.
    RecordVersion { kind: &'static str, found: u32 },
    /// LE-103: a tune-name reference could not be resolved.
    UnknownName(String),
    /// LE-104: a track or window shape is malformed (empty
    /// key list, unsorted keys, inverted window).
    TrackShape(String),
    /// LE-105: the dialogue runtime refused its record or a
    /// state restore.
    Dialogue(String),
    /// LE-106: the effect player refused its record (shape,
    /// or the flash law: more than
    /// [`crate::effect::FLASH_MAX_PER_SECOND`] strobe keys in
    /// any one-second window).
    Effect(String),
    /// LE-107: the audio binding evaluation refused its
    /// record.
    Audio(String),
    /// LE-108: a state record does not belong to the scene
    /// it is restored into.
    StateMismatch(String),
    /// LE-109: the fixed clock refused its rate.
    Clock(String),
}

impl EngineError {
    /// The stable `LE-` code for this refusal.
    pub fn code(&self) -> &'static str {
        match self {
            EngineError::RecordParse(_) => "LE-101",
            EngineError::RecordVersion { .. } => "LE-102",
            EngineError::UnknownName(_) => "LE-103",
            EngineError::TrackShape(_) => "LE-104",
            EngineError::Dialogue(_) => "LE-105",
            EngineError::Effect(_) => "LE-106",
            EngineError::Audio(_) => "LE-107",
            EngineError::StateMismatch(_) => "LE-108",
            EngineError::Clock(_) => "LE-109",
        }
    }
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let code = self.code();
        match self {
            EngineError::RecordParse(message) => {
                write!(f, "{code}: record parse refused: {message}")
            }
            EngineError::RecordVersion { kind, found } => {
                write!(f, "{code}: unsupported {kind} record version {found}")
            }
            EngineError::UnknownName(name) => {
                write!(f, "{code}: unresolved tune name '{name}'")
            }
            EngineError::TrackShape(message) => {
                write!(f, "{code}: track shape refused: {message}")
            }
            EngineError::Dialogue(message) => {
                write!(f, "{code}: dialogue refused: {message}")
            }
            EngineError::Effect(message) => {
                write!(f, "{code}: effect refused: {message}")
            }
            EngineError::Audio(message) => {
                write!(f, "{code}: audio binding refused: {message}")
            }
            EngineError::StateMismatch(message) => {
                write!(f, "{code}: state restore refused: {message}")
            }
            EngineError::Clock(message) => {
                write!(f, "{code}: clock refused: {message}")
            }
        }
    }
}

impl std::error::Error for EngineError {}
