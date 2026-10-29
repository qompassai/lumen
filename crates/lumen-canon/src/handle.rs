//! The handle protocol (SPEC §5): `canon://<lane>/<character>@<version>`
//! plus the record's SHA-256. Consumers fetch by handle and verify
//! the hash before any use; hash mismatch or version skew is the
//! typed refusal `record_stale`. The lane check of SPEC §2.3 runs in
//! server code, here, never on client trust: sfw-labelled records may
//! be fetched by any caller; nsfw-labelled records are refused to
//! qompass-lane callers; unclassified records default to nsfw.

use crate::model::{sha256_hex, CanonRecord, ContentLabel, Lane};
use crate::refusal::Refusal;
use crate::validate::validate_record;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanonHandle {
    pub lane: Lane,
    pub character: String,
    pub version: u64,
}

impl CanonHandle {
    /// Parse a handle strictly: exact scheme, exactly one path
    /// separator, one `@`, a known lane, a non-empty character
    /// segment, and a numeric version >= 1.
    pub fn parse(text: &str) -> Result<Self, Refusal> {
        let malformed = |detail: &str| Refusal::MalformedHandle {
            detail: detail.to_string(),
        };
        let rest = text
            .strip_prefix("canon://")
            .ok_or_else(|| malformed("handle must start with canon://"))?;
        let (path, version_text) = rest
            .rsplit_once('@')
            .ok_or_else(|| malformed("handle has no @<version>"))?;
        if version_text.is_empty() || !version_text.bytes().all(|b| b.is_ascii_digit()) {
            return Err(malformed("version is not a non-negative integer"));
        }
        let version: u64 = version_text
            .parse()
            .map_err(|_| malformed("version out of range"))?;
        if version == 0 {
            return Err(malformed("version must be >= 1"));
        }
        let mut segments = path.split('/');
        let lane_text = segments.next().unwrap_or_default();
        let character = segments.next().unwrap_or_default();
        if segments.next().is_some() {
            return Err(malformed("handle has more than two path segments"));
        }
        let lane: Lane = lane_text
            .parse()
            .map_err(|_| malformed("unknown lane segment"))?;
        if character.is_empty()
            || !character.bytes().all(|b| {
                b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'_' | b'.')
            })
        {
            return Err(malformed(
                "character segment is empty or has illegal characters",
            ));
        }
        Ok(CanonHandle {
            lane,
            character: character.to_string(),
            version,
        })
    }

    /// The record id this handle names (`<lane>/<character>`).
    pub fn record_id(&self) -> String {
        format!("{}/{}", self.lane, self.character)
    }
}

impl fmt::Display for CanonHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "canon://{}@{}", self.record_id(), self.version)
    }
}

/// The lane check of SPEC §2.3, enforced on the fetching side: an
/// nsfw (or unclassified — defaulted nsfw) record is refused to a
/// qompass-lane caller with `lane_mismatch`. Sfw records may be
/// fetched by any caller.
pub fn check_lane_access(record: &CanonRecord, caller: Lane) -> Result<(), Refusal> {
    if record.effective_label() == ContentLabel::Nsfw && caller == Lane::Qompass {
        return Err(Refusal::LaneMismatch {
            record_id: record.id.clone(),
            caller_lane: caller.to_string(),
        });
    }
    Ok(())
}

/// Resolve a handle against record bytes: parse the handle, verify
/// the SHA-256 pin, parse and validate the record, confirm the bytes
/// are the record the handle names (id + version), then run the lane
/// check for `caller`. Any failure is a typed refusal; nothing is
/// returned on partial trust.
pub fn resolve(
    handle_text: &str,
    expected_sha256: &str,
    record_bytes: &[u8],
    caller: Lane,
) -> Result<CanonRecord, Refusal> {
    let handle = CanonHandle::parse(handle_text)?;

    let actual = sha256_hex(record_bytes);
    if actual != expected_sha256 {
        return Err(Refusal::RecordStale {
            detail: format!(
                "sha256 mismatch: handle pins {expected_sha256}, bytes hash to {actual}"
            ),
        });
    }

    let record: CanonRecord =
        serde_json::from_slice(record_bytes).map_err(|e| Refusal::MalformedRecord {
            detail: e.to_string(),
        })?;

    let issues = validate_record(&record);
    if !issues.is_empty() {
        return Err(Refusal::MalformedRecord {
            detail: issues
                .iter()
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join("; "),
        });
    }

    if record.id != handle.record_id() {
        return Err(Refusal::RecordStale {
            detail: format!(
                "handle names {}, bytes are record {}",
                handle.record_id(),
                record.id
            ),
        });
    }
    if record.version != handle.version {
        return Err(Refusal::RecordStale {
            detail: format!(
                "version skew: handle wants @{}, record is version {}",
                handle.version, record.version
            ),
        });
    }

    check_lane_access(&record, caller)?;
    Ok(record)
}
