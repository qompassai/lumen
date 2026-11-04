//! State records: the runtime's position, serialized as
//! canonical RON, restorable into a fresh runtime so a
//! scene can resume mid-run with an identical event
//! stream. The integrity tag is FNV-1a over the canonical
//! text — a change detector for gate evidence, not a
//! cryptographic hash.

use serde::{Deserialize, Serialize};

use crate::error::EngineError;

/// The only state-record version this build reads.
pub const STATE_RECORD_VERSION: u32 = 1;

/// A runtime snapshot.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(deny_unknown_fields)]
pub struct StateRecord {
    /// Record format version.
    pub version: u32,
    /// Scene the state belongs to.
    pub scene_id: String,
    /// Step counter position (the next step to run).
    pub step: u64,
    /// Dialogue line index.
    pub dialogue_line: u32,
    /// Dialogue characters shown on the current line.
    pub dialogue_chars: u32,
    /// Dialogue hold steps remaining.
    pub dialogue_hold: u64,
    /// Dialogue started flag.
    pub dialogue_started: bool,
    /// Dialogue done flag.
    pub dialogue_done: bool,
    /// Effect keys already fired ("effect_id:key_index",
    /// sorted).
    pub effects_fired: Vec<String>,
    /// Audio cue events fired so far.
    pub audio_fired: u64,
}

impl StateRecord {
    /// Canonical RON text. Field order is declaration
    /// order and collections are sorted by construction,
    /// so equal states serialize byte-identically.
    pub fn to_ron(&self) -> String {
        ron::ser::to_string_pretty(self, ron::ser::PrettyConfig::new())
            .expect("state record serializes")
    }

    /// Parse a state record from RON text (strict).
    pub fn load_ron(text: &str) -> Result<Self, EngineError> {
        let record: StateRecord =
            ron::de::from_str(text).map_err(|e| EngineError::RecordParse(e.to_string()))?;
        if record.version != STATE_RECORD_VERSION {
            return Err(EngineError::RecordVersion {
                kind: "state",
                found: record.version,
            });
        }
        Ok(record)
    }

    /// FNV-1a integrity tag of the canonical text, as
    /// 16 hex digits.
    pub fn hash(&self) -> String {
        format!("{:016x}", fnv1a64(self.to_ron().as_bytes()))
    }
}

/// FNV-1a 64-bit over `bytes` (wrapping arithmetic only;
/// identical on every platform).
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    hash
}
