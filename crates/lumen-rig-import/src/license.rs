//! The per-model license gate.
//!
//! Conversion is refused unless the operator supplies a license
//! record that explicitly grants both game use and conversion.
//! The gate runs before any model file is read, and it fails
//! closed: a missing, malformed, or incomplete record is a
//! refusal, never a default-allow.

use serde::Deserialize;

use crate::error::ConvertError;

/// A per-model license record, as supplied by the operator.
///
/// All fields are required. `game_use` and `conversion_allowed`
/// must both be `true` for conversion to proceed.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LicenseRecord {
    /// Where the model came from (publisher, store page, or the
    /// operator's own authorship note).
    pub source: String,
    /// The license the model is used under, by name.
    pub license_name: String,
    /// Canonical URL of the license text, when one exists.
    pub license_url: Option<String>,
    /// The license permits using the model in a shipped game.
    pub game_use: bool,
    /// The license permits converting the model to another format.
    pub conversion_allowed: bool,
    /// Who recorded this record (a person or role, not a secret).
    pub recorded_by: String,
    /// Date the record was made, ISO 8601 (`YYYY-MM-DD`).
    pub date: String,
}

impl LicenseRecord {
    /// Parse a license record from JSON text. Any parse or schema
    /// problem is `LicenseUnreadable` — the gate fails closed.
    pub fn parse(text: &str) -> Result<LicenseRecord, ConvertError> {
        let record: LicenseRecord =
            serde_json::from_str(text).map_err(|error| ConvertError::LicenseUnreadable {
                message: error.to_string(),
            })?;
        for (field, value) in [
            ("source", &record.source),
            ("license_name", &record.license_name),
            ("recorded_by", &record.recorded_by),
            ("date", &record.date),
        ] {
            if value.trim().is_empty() {
                return Err(ConvertError::LicenseUnreadable {
                    message: format!("field {field} is empty"),
                });
            }
        }
        Ok(record)
    }

    /// The gate itself: both grants must be present and true.
    pub fn check(&self) -> Result<(), ConvertError> {
        if !self.game_use {
            return Err(ConvertError::LicenseDenied {
                reason: format!(
                    "record for {:?} under {:?} does not grant game use",
                    self.source, self.license_name
                ),
            });
        }
        if !self.conversion_allowed {
            return Err(ConvertError::LicenseDenied {
                reason: format!(
                    "record for {:?} under {:?} does not grant conversion",
                    self.source, self.license_name
                ),
            });
        }
        Ok(())
    }
}
