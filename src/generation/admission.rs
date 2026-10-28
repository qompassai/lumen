//! Model admission: a registered model is admitted only when its canonical
//! checkpoint exists, hashes to the configured full SHA-256, and the name the
//! engine will load (`engine_model_dir/engine_name`) resolves to that same
//! file.
//!
//! Admission is computed once per registered model at startup (hashing ~7 GB
//! per checkpoint is not per-call work) and is immutable for the life of the
//! process. Vera and Aria are roster names only in S1: reported as
//! unimplemented, never admitted.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::generation::config::{Config, ModelConfig};
use crate::generation::error::{AtelierError, ErrorCode, Result, err};

/// Largest checkpoint atelier will hash, in bytes.
pub const CHECKPOINT_BYTES_MAX: u64 = 32 * 1024 * 1024 * 1024;
const HASH_CHUNK_BYTES: usize = 4 * 1024 * 1024;

/// Lanes known to the roster.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Lane {
    Image,
    Video,
    Music,
}

impl Lane {
    pub fn parse(value: &str) -> Result<Lane> {
        match value {
            "image" => Ok(Lane::Image),
            "video" => Ok(Lane::Video),
            "music" => Ok(Lane::Music),
            other => err(
                ErrorCode::ArgumentInvalid,
                format!("lane `{other}` is not one of: image, video, music"),
            ),
        }
    }
}

/// Roster entries that are named but not implemented in S1.
pub const UNIMPLEMENTED: [(&str, Lane); 2] = [("vera", Lane::Video), ("aria", Lane::Music)];

/// The immutable admission result for one registered model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Admission {
    pub model: String,
    pub lane: Lane,
    pub admitted: bool,
    /// `None` when admitted; otherwise the refusal code.
    pub refusal_code: Option<&'static str>,
    pub refusal: Option<String>,
    pub canonical_path: Option<PathBuf>,
    pub expected_sha256: String,
    pub observed_sha256: Option<String>,
    pub engine_name: String,
    pub display_name: String,
    pub source: String,
    pub license: String,
    pub license_alert: bool,
}

impl Admission {
    /// Refuse generation unless admitted.
    pub fn require(&self) -> Result<()> {
        if self.admitted {
            return Ok(());
        }
        err(
            ErrorCode::ModelNotAdmitted,
            format!(
                "{} is not admitted: {} ({})",
                self.model,
                self.refusal.as_deref().unwrap_or("unknown"),
                self.refusal_code.unwrap_or("unknown")
            ),
        )
    }
}

/// Streaming SHA-256 of a regular file, bounded by `bytes_max`.
pub fn sha256_file(path: &Path, bytes_max: u64) -> std::io::Result<String> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; HASH_CHUNK_BYTES];
    let mut total: u64 = 0;
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        total = total.saturating_add(read as u64);
        if total > bytes_max {
            return Err(std::io::Error::other("file exceeds hash bound"));
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex(&hasher.finalize()))
}

pub fn sha256_bytes(bytes: &[u8]) -> String {
    hex(&Sha256::digest(bytes))
}

fn hex(digest: &[u8]) -> String {
    use std::fmt::Write;
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        // Writing to a String cannot fail.
        let _ = write!(out, "{byte:02x}");
    }
    out
}

fn check(model: &ModelConfig) -> Result<(PathBuf, String)> {
    let canonical = model.checkpoint_path.canonicalize().map_err(|cause| {
        AtelierError::new(
            ErrorCode::CheckpointMissing,
            format!("checkpoint {}: {cause}", model.checkpoint_path.display()),
        )
    })?;
    if !canonical.is_file() {
        return err(
            ErrorCode::CheckpointMissing,
            "checkpoint is not a regular file",
        );
    }
    let engine_path = model.engine_model_dir.join(&model.engine_name);
    let engine_canonical = engine_path.canonicalize().map_err(|cause| {
        AtelierError::new(
            ErrorCode::CheckpointNameMismatch,
            format!(
                "engine name {} does not resolve: {cause}",
                engine_path.display()
            ),
        )
    })?;
    if engine_canonical != canonical {
        return err(
            ErrorCode::CheckpointNameMismatch,
            format!(
                "engine name {} resolves to {}, not the admitted checkpoint",
                model.engine_name,
                engine_canonical.display()
            ),
        );
    }
    let observed = sha256_file(&canonical, CHECKPOINT_BYTES_MAX).map_err(|cause| {
        AtelierError::new(
            ErrorCode::CheckpointMissing,
            format!("hashing failed: {cause}"),
        )
    })?;
    Ok((canonical, observed))
}

/// Admission records for every registered model, keyed by model id.
pub fn admit_all(config: &Config) -> BTreeMap<String, Admission> {
    config
        .models
        .iter()
        .map(|(id, model)| (id.clone(), admit(id, model)))
        .collect()
}

/// Compute one model's admission record. Never panics on external state.
pub fn admit(id: &str, model: &ModelConfig) -> Admission {
    let mut admission = Admission {
        model: id.to_string(),
        lane: Lane::Image,
        admitted: false,
        refusal_code: None,
        refusal: None,
        canonical_path: None,
        expected_sha256: model.sha256.clone(),
        observed_sha256: None,
        engine_name: model.engine_name.clone(),
        display_name: model.display_name.clone(),
        source: model.source.clone(),
        license: model
            .license
            .clone()
            .unwrap_or_else(|| "UNRECORDED".to_string()),
        license_alert: model.license.is_none(),
    };
    match check(model) {
        Ok((canonical, observed)) => {
            admission.canonical_path = Some(canonical);
            if observed == model.sha256 {
                admission.admitted = true;
            } else {
                admission.refusal_code = Some(ErrorCode::CheckpointHashMismatch.as_str());
                admission.refusal = Some("observed SHA-256 differs from the pinned hash".into());
            }
            admission.observed_sha256 = Some(observed);
        }
        Err(error) => {
            admission.refusal_code = Some(error.code.as_str());
            admission.refusal = Some(error.message);
        }
    }
    admission
}
