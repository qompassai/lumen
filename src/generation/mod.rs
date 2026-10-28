//! generation — the estate's image-generation lane, folded from the
//! standalone **atelier** server into lumen (Owner ruling D-143; the
//! convergence the atelier program planned from 2026-10-20: "the
//! profile system survives intact inside lumen's generation module").
//!
//! Scope of the fold (D-144/D-145): atelier is **images only**. The
//! four S1 tools — `atelier_status`, `atelier_models`,
//! `atelier_txt2img`, `atelier_img2img` — are ported with their
//! names, schemas, and behavior against atelier's measured baseline
//! (primo `~/atelier` @ `11f027a`, branch `pax/atelier-s1`). Atelier
//! never shipped video or music tools: `vera`/`aria` exist only as
//! unimplemented roster names inside `atelier_models` output, and
//! they are carried as data, unchanged. Audio belongs to torori
//! (finishing/delivery) and the composition lane (music authoring);
//! nothing of it is ported here.
//!
//! The lane keeps atelier's contracts verbatim: fail-closed TOML
//! configuration (`$ATELIER_CONFIG`, else
//! `~/.config/atelier/atelier.toml`), the two config-selected lane
//! profiles (`qompass` / `ntranch`) with disjoint roots and
//! profile-stamped logs and manifests, model admission by pinned
//! SHA-256, versioned workflow templates, the owned-engine lifecycle
//! on the pinned GPU, and the stable error-code set in
//! [`error`]. Tool calls cannot switch the profile.
//!
//! Startup posture (the one deliberate fold delta): standalone
//! atelier served nothing else, so a missing config was a startup
//! failure. The folded server also carries lumen's own surface, so
//! an *absent default* config leaves the lane registered but
//! unconfigured — its tools answer with atelier's typed
//! `config_missing` refusal — while a config that is *named*
//! (`$ATELIER_CONFIG` set) or *present* but invalid refuses startup,
//! exactly as both parents fail closed on a named, broken config
//! (mirroring `LUMEN_POLICY` handling in the binary).

pub mod admission;
pub mod backend;
pub mod config;
pub mod error;
pub mod lifecycle;
pub mod log;
pub mod paths;
pub mod service;
pub mod template;

#[cfg(test)]
mod tests;

use std::path::PathBuf;
use std::sync::Arc;

use admission::admit_all;
use backend::ComfyBackend;
use config::Config;
use error::{AtelierError, ErrorCode};
use lifecycle::{CommandLauncher, NvidiaSmi};
use service::Atelier;

/// Fixed path of the GPU probe (no PATH lookup) — atelier's rule, kept.
pub const NVIDIA_SMI: &str = "/usr/bin/nvidia-smi";

/// The concrete generation core the folded server runs.
pub type Core = Atelier<ComfyBackend, CommandLauncher, NvidiaSmi>;

/// The configured generation lane, held by the MCP handler.
pub struct GenerationLane {
    core: Arc<Core>,
}

impl GenerationLane {
    /// The generation core (tool semantics live in [`service`]).
    pub fn core(&self) -> &Core {
        &self.core
    }

    /// The active lane profile name (`qompass` or `ntranch`).
    pub fn profile(&self) -> &'static str {
        self.core.config.profile.as_str()
    }

    /// Atelier's refusal wire shape for a failed generation call: the
    /// error object plus the active profile stamp.
    pub fn refusal(&self, error: &AtelierError) -> String {
        let mut value = error.to_json();
        value["profile"] = serde_json::json!(self.profile());
        to_json(&value)
    }
}

impl std::fmt::Debug for GenerationLane {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GenerationLane")
            .field("profile", &self.profile())
            .finish_non_exhaustive()
    }
}

/// Startup outcome for the generation lane.
pub enum GenerationSetup {
    /// A valid config was loaded; the lane is live.
    Ready(GenerationLane),
    /// No config exists at the resolved path; tools stay registered
    /// and refuse per call with `config_missing`.
    Unconfigured { expected_path: PathBuf },
}

/// Resolve and (when present) load the atelier configuration,
/// following the startup posture in the module docs. A present but
/// invalid config is an error, never a silent fallback.
pub fn setup() -> Result<GenerationSetup, AtelierError> {
    let env_path = std::env::var(config::CONFIG_ENV).ok();
    let named = env_path.as_deref().is_some_and(|value| !value.is_empty());
    let home = std::env::var("HOME").ok();
    let path = config::resolve_path(env_path.as_deref(), home.as_deref())?;
    if !named && !path.exists() {
        return Ok(GenerationSetup::Unconfigured {
            expected_path: path,
        });
    }
    let config = Config::load(&path)?;
    let admissions = admit_all(&config);
    let backend = ComfyBackend::new(config.engine.base_url())?;
    let gpu = NvidiaSmi {
        program: PathBuf::from(NVIDIA_SMI),
    };
    let core = Atelier::new(config, admissions, backend, CommandLauncher, gpu);
    Ok(GenerationSetup::Ready(GenerationLane {
        core: Arc::new(core),
    }))
}

/// Serialize a tool result exactly as standalone atelier did
/// (pretty JSON; a serialization failure becomes an
/// `output_write_failed` error object rather than a panic).
pub fn to_json<T: serde::Serialize>(value: &T) -> String {
    serde_json::to_string_pretty(value).unwrap_or_else(|cause| {
        serde_json::json!({"error": {"code": "output_write_failed",
            "message": format!("serialization failed: {cause}")}})
        .to_string()
    })
}

/// The per-call refusal served by every generation tool while the
/// lane is unconfigured: atelier's error shape with `config_missing`.
pub fn unconfigured_refusal(expected_path: &std::path::Path) -> String {
    let error = AtelierError::new(
        ErrorCode::ConfigMissing,
        format!(
            "generation lane is not configured: no config at {} \
             (set $ATELIER_CONFIG or write the default config; there is no built-in default)",
            expected_path.display()
        ),
    );
    serde_json::to_string_pretty(&error.to_json()).unwrap_or_else(|cause| {
        serde_json::json!({"error": {"code": "output_write_failed",
            "message": format!("serialization failed: {cause}")}})
        .to_string()
    })
}
