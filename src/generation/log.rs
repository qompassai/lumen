//! Append-only JSONL event log under the active profile's log root.
//!
//! Every line carries `ts`, `level`, `event`, and `profile`; job events add
//! `job_id` and (once known) `prompt_id`. Logging is best-effort by design:
//! a failed log write must not fail a job, but it is counted and reported by
//! `atelier_status` (`log_write_failures`) rather than silently dropped.

use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{Map, Value};

use crate::generation::config::Profile;

#[derive(Debug)]
pub struct EventLog {
    pub path: PathBuf,
    profile: Profile,
    file: Mutex<Option<std::fs::File>>,
    pub write_failures: AtomicU64,
}

impl EventLog {
    pub fn open(path: PathBuf, profile: Profile) -> EventLog {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok();
        let failures = if file.is_some() { 0 } else { 1 };
        EventLog {
            path,
            profile,
            file: Mutex::new(file),
            write_failures: AtomicU64::new(failures),
        }
    }

    /// Append one event. `fields` may add `job_id`, `prompt_id`, details.
    pub fn event(&self, level: &str, event: &str, fields: Value) {
        let mut line = Map::new();
        line.insert("ts".into(), Value::String(now_rfc3339()));
        line.insert("level".into(), Value::String(level.into()));
        line.insert("event".into(), Value::String(event.into()));
        line.insert(
            "profile".into(),
            Value::String(self.profile.as_str().into()),
        );
        if let Value::Object(extra) = fields {
            for (key, value) in extra {
                if !line.contains_key(&key) {
                    line.insert(key, value);
                }
            }
        }
        let mut text = Value::Object(line).to_string();
        text.push('\n');
        let written = match self.file.lock() {
            Ok(mut guard) => guard
                .as_mut()
                .is_some_and(|file| file.write_all(text.as_bytes()).is_ok()),
            Err(_) => false,
        };
        if !written {
            self.write_failures.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// Milliseconds since the Unix epoch.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
        .unwrap_or(0)
}

/// UTC RFC 3339 timestamp with millisecond precision.
pub fn now_rfc3339() -> String {
    rfc3339(now_ms())
}

pub fn rfc3339(epoch_ms: u64) -> String {
    let seconds = epoch_ms / 1000;
    let millis = epoch_ms % 1000;
    let days = i64::try_from(seconds / 86_400).unwrap_or(0);
    let second_of_day = seconds % 86_400;
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{millis:03}Z",
        second_of_day / 3600,
        (second_of_day % 3600) / 60,
        second_of_day % 60
    )
}

/// Howard Hinnant's days-to-civil algorithm (proleptic Gregorian).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = u32::try_from(day_of_year - (153 * month_index + 2) / 5 + 1).unwrap_or(1);
    let month = u32::try_from(if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    })
    .unwrap_or(1);
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}
