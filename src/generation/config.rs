//! Fail-closed TOML configuration.
//!
//! Contract: the file named by `$ATELIER_CONFIG`, else
//! `~/.config/atelier/atelier.toml`, must exist, be at most
//! [`CONFIG_BYTES_MAX`] bytes, parse with no unknown keys, and validate
//! completely. There is no built-in default configuration and no profile
//! default: `profile` is required and both profile blocks must be present so
//! their write roots can be proven disjoint. Every root is canonicalized at
//! load; overlap between the two profiles' write roots (output, staged input,
//! log) refuses startup as `profile_root_overlap`.
//!
//! Models are a registry: `[models.<id>]` tables keyed by model id, each
//! carrying a required `modality`. `image` (a ComfyUI checkpoint) is the only
//! modality this stage admits; future engines (audio: kokoro, dia, ace-step)
//! register under the same `[models.<id>]` key with their own modality, which
//! is refused as an unknown variant until implemented. `[defaults]` names the
//! model each generation tool uses when a request omits `model`; both must be
//! registered image models.

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::generation::error::{AtelierError, ErrorCode, Result, err};

/// Largest config file accepted, in bytes.
pub const CONFIG_BYTES_MAX: u64 = 64 * 1024;
/// Environment variable naming an explicit config file.
pub const CONFIG_ENV: &str = "ATELIER_CONFIG";
/// Most allowed source roots one profile may name.
pub const SOURCE_ROOT_COUNT_MAX: usize = 16;
/// Hard ceilings the config's own limits may not exceed.
pub const AXIS_PIXELS_CEILING: u32 = 4096;
pub const TOTAL_PIXELS_CEILING: u64 = 4096 * 4096;
pub const STEPS_CEILING: u32 = 150;
pub const PROMPT_BYTES_CEILING: usize = 16 * 1024;
pub const SOURCE_BYTES_CEILING: u64 = 64 * 1024 * 1024;
pub const QUEUE_DEPTH_CEILING: usize = 8;
pub const STARTUP_TIMEOUT_S_MAX: u64 = 900;
pub const JOB_TIMEOUT_S_MAX: u64 = 3600;
/// Most models the registry may hold.
pub const MODEL_COUNT_MAX: usize = 16;
/// Longest model id, in bytes.
pub const MODEL_ID_BYTES_MAX: usize = 32;

/// The two S1 lane profiles. The content class is fixed per profile.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Profile {
    Qompass,
    Ntranch,
}

impl Profile {
    pub const ALL: [Profile; 2] = [Profile::Qompass, Profile::Ntranch];

    pub fn as_str(self) -> &'static str {
        match self {
            Profile::Qompass => "qompass",
            Profile::Ntranch => "ntranch",
        }
    }

    pub fn content_class(self) -> &'static str {
        match self {
            Profile::Qompass => "sfw",
            Profile::Ntranch => "adult",
        }
    }

    pub fn parse(value: &str) -> Result<Profile> {
        match value {
            "qompass" => Ok(Profile::Qompass),
            "ntranch" => Ok(Profile::Ntranch),
            other => err(
                ErrorCode::ProfileUnknown,
                format!("profile `{other}` is not one of: qompass, ntranch"),
            ),
        }
    }
}

/// One profile's validated, canonicalized roots.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileRoots {
    pub source_roots: Vec<PathBuf>,
    pub staged_input_root: PathBuf,
    pub output_root: PathBuf,
    pub log_root: PathBuf,
}

impl ProfileRoots {
    /// Path of this profile's append-only log file.
    pub fn log_file(&self, profile: Profile) -> PathBuf {
        self.log_root
            .join(format!("atelier-{}.jsonl", profile.as_str()))
    }
}

/// Engine launch and admission facts (pinned by `~/ComfyUI-LAUNCH.md`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineConfig {
    pub host: IpAddr,
    pub port: u16,
    pub program: PathBuf,
    pub args: Vec<String>,
    pub working_directory: PathBuf,
    pub cuda_device_order: String,
    pub cuda_visible_devices: String,
    /// nvidia-smi index of the selected GPU (PCI bus order).
    pub gpu_index: u32,
    /// Substring the engine-reported device name must contain.
    pub expected_gpu_name: String,
    pub vram_free_mib_min: u64,
    pub startup_timeout_s: u64,
    pub job_timeout_s: u64,
}

impl EngineConfig {
    pub fn base_url(&self) -> String {
        format!("http://{}:{}", self.host, self.port)
    }
}

/// What kind of engine a registered model feeds. Only `image` is
/// implemented; a new modality is a new variant plus its own validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Modality {
    Image,
}

/// One registered model's admission record (Q-ATELIER-5: name, source,
/// hash; license optional).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModelConfig {
    pub modality: Modality,
    pub checkpoint_path: PathBuf,
    pub sha256: String,
    pub engine_name: String,
    pub engine_model_dir: PathBuf,
    pub display_name: String,
    pub source: String,
    pub license: Option<String>,
    pub notes: Option<String>,
}

/// Model used by each generation tool when a request omits `model`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModelDefaults {
    pub txt2img: String,
    pub img2img: String,
}

/// Parameter ceilings for one generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Limits {
    pub axis_pixels_max: u32,
    pub total_pixels_max: u64,
    pub steps_max: u32,
    pub prompt_bytes_max: usize,
    pub source_bytes_max: u64,
    pub queue_depth_max: usize,
}

/// A fully validated configuration. Construct only via [`Config::from_toml_str`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub profile: Profile,
    pub state_root: PathBuf,
    pub qompass: ProfileRoots,
    pub ntranch: ProfileRoots,
    pub engine: EngineConfig,
    pub models: BTreeMap<String, ModelConfig>,
    pub defaults: ModelDefaults,
    pub limits: Limits,
}

impl Config {
    pub fn active_roots(&self) -> &ProfileRoots {
        self.roots(self.profile)
    }

    pub fn roots(&self, profile: Profile) -> &ProfileRoots {
        match profile {
            Profile::Qompass => &self.qompass,
            Profile::Ntranch => &self.ntranch,
        }
    }

    /// The registered model `id`, or `model_unknown` naming every
    /// registered id. Never falls back to another model.
    pub fn model(&self, id: &str) -> Result<&ModelConfig> {
        match self.models.get(id) {
            Some(model) => Ok(model),
            None => err(
                ErrorCode::ModelUnknown,
                format!(
                    "model `{id}` is not registered; registered: {}",
                    registered_ids(&self.models)
                ),
            ),
        }
    }
}

fn registered_ids(models: &BTreeMap<String, ModelConfig>) -> String {
    models
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .join(", ")
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawConfig {
    profile: Option<String>,
    state_root: String,
    #[serde(default)]
    profiles: RawProfiles,
    engine: RawEngine,
    models: BTreeMap<String, RawModel>,
    defaults: RawDefaults,
    limits: RawLimits,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RawProfiles {
    qompass: Option<RawProfile>,
    ntranch: Option<RawProfile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProfile {
    source_roots: Vec<String>,
    staged_input_root: String,
    output_root: String,
    log_root: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawEngine {
    host: String,
    port: u16,
    program: String,
    #[serde(default)]
    args: Vec<String>,
    working_directory: String,
    cuda_device_order: String,
    cuda_visible_devices: String,
    gpu_index: u32,
    expected_gpu_name: String,
    vram_free_mib_min: u64,
    startup_timeout_s: u64,
    job_timeout_s: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawModel {
    modality: Modality,
    checkpoint_path: String,
    sha256: String,
    engine_name: String,
    engine_model_dir: String,
    display_name: String,
    source: String,
    license: Option<String>,
    notes: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDefaults {
    txt2img: String,
    img2img: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLimits {
    axis_pixels_max: u32,
    total_pixels_max: u64,
    steps_max: u32,
    prompt_bytes_max: usize,
    source_bytes_max: u64,
    queue_depth_max: usize,
}

fn invalid(field: &str, reason: impl Into<String>) -> AtelierError {
    AtelierError::new(
        ErrorCode::ConfigInvalid,
        format!("config field `{field}`: {}", reason.into()),
    )
}

/// Canonicalize an absolute, existing directory.
fn canonical_dir(field: &str, raw: &str) -> Result<PathBuf> {
    let path = Path::new(raw);
    if !path.is_absolute() {
        return err(
            ErrorCode::ProfileRootInvalid,
            format!("`{field}` must be an absolute path"),
        );
    }
    let canonical = path.canonicalize().map_err(|cause| {
        AtelierError::new(
            ErrorCode::ProfileRootInvalid,
            format!("`{field}` ({raw}) cannot be canonicalized: {cause}"),
        )
    })?;
    if !canonical.is_dir() {
        return err(
            ErrorCode::ProfileRootInvalid,
            format!("`{field}` ({raw}) is not a directory"),
        );
    }
    Ok(canonical)
}

fn validate_profile(name: &str, raw: &RawProfile) -> Result<ProfileRoots> {
    if raw.source_roots.is_empty() || raw.source_roots.len() > SOURCE_ROOT_COUNT_MAX {
        return Err(invalid(
            &format!("profiles.{name}.source_roots"),
            format!("must name 1..={SOURCE_ROOT_COUNT_MAX} roots"),
        ));
    }
    let mut source_roots = Vec::with_capacity(raw.source_roots.len());
    for (index, root) in raw.source_roots.iter().enumerate() {
        source_roots.push(canonical_dir(
            &format!("profiles.{name}.source_roots[{index}]"),
            root,
        )?);
    }
    let roots = ProfileRoots {
        source_roots,
        staged_input_root: canonical_dir(
            &format!("profiles.{name}.staged_input_root"),
            &raw.staged_input_root,
        )?,
        output_root: canonical_dir(&format!("profiles.{name}.output_root"), &raw.output_root)?,
        log_root: canonical_dir(&format!("profiles.{name}.log_root"), &raw.log_root)?,
    };
    let writes = [
        &roots.staged_input_root,
        &roots.output_root,
        &roots.log_root,
    ];
    for (i, left) in writes.iter().enumerate() {
        for right in writes.iter().skip(i + 1) {
            if overlaps(left, right) {
                return err(
                    ErrorCode::ProfileRootOverlap,
                    format!(
                        "profile {name}: write roots {} and {} overlap",
                        left.display(),
                        right.display()
                    ),
                );
            }
        }
    }
    Ok(roots)
}

/// Equal or nested (component-wise, after canonicalization).
pub fn overlaps(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

/// Every write root of one profile must be disjoint from every write root of
/// the other.
fn check_cross_profile(qompass: &ProfileRoots, ntranch: &ProfileRoots) -> Result<()> {
    let labelled = |roots: &ProfileRoots| {
        [
            ("staged_input_root", roots.staged_input_root.clone()),
            ("output_root", roots.output_root.clone()),
            ("log_root", roots.log_root.clone()),
        ]
    };
    for (q_label, q_path) in labelled(qompass) {
        for (n_label, n_path) in labelled(ntranch) {
            if overlaps(&q_path, &n_path) {
                return err(
                    ErrorCode::ProfileRootOverlap,
                    format!(
                        "qompass {q_label} ({}) overlaps ntranch {n_label} ({})",
                        q_path.display(),
                        n_path.display()
                    ),
                );
            }
        }
    }
    Ok(())
}

fn validate_engine(raw: &RawEngine) -> Result<EngineConfig> {
    let host: IpAddr = raw
        .host
        .parse()
        .map_err(|_| invalid("engine.host", "must be a literal IP address"))?;
    if !host.is_loopback() {
        return err(
            ErrorCode::EngineAddressNotLoopback,
            format!("engine.host {host} is not loopback; atelier refuses non-loopback engines"),
        );
    }
    if raw.port == 0 {
        return Err(invalid("engine.port", "must be nonzero"));
    }
    let program = PathBuf::from(&raw.program);
    if !program.is_absolute() {
        return Err(invalid("engine.program", "must be an absolute path"));
    }
    let working_directory = PathBuf::from(&raw.working_directory);
    if !working_directory.is_absolute() {
        return Err(invalid(
            "engine.working_directory",
            "must be an absolute path",
        ));
    }
    if raw.expected_gpu_name.trim().is_empty() {
        return Err(invalid("engine.expected_gpu_name", "must be nonempty"));
    }
    if raw.startup_timeout_s == 0 || raw.startup_timeout_s > STARTUP_TIMEOUT_S_MAX {
        return Err(invalid(
            "engine.startup_timeout_s",
            format!("must be 1..={STARTUP_TIMEOUT_S_MAX}"),
        ));
    }
    if raw.job_timeout_s == 0 || raw.job_timeout_s > JOB_TIMEOUT_S_MAX {
        return Err(invalid(
            "engine.job_timeout_s",
            format!("must be 1..={JOB_TIMEOUT_S_MAX}"),
        ));
    }
    Ok(EngineConfig {
        host,
        port: raw.port,
        program,
        args: raw.args.clone(),
        working_directory,
        cuda_device_order: raw.cuda_device_order.clone(),
        cuda_visible_devices: raw.cuda_visible_devices.clone(),
        gpu_index: raw.gpu_index,
        expected_gpu_name: raw.expected_gpu_name.clone(),
        vram_free_mib_min: raw.vram_free_mib_min,
        startup_timeout_s: raw.startup_timeout_s,
        job_timeout_s: raw.job_timeout_s,
    })
}

fn validate_model(id: &str, raw: &RawModel) -> Result<ModelConfig> {
    let id_ok = !id.is_empty()
        && id.len() <= MODEL_ID_BYTES_MAX
        && id.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"_-".contains(&byte)
        });
    if !id_ok {
        return Err(invalid(
            &format!("models.{id}"),
            format!("model id must be 1..={MODEL_ID_BYTES_MAX} bytes of [a-z0-9_-]"),
        ));
    }
    let hash_ok = raw.sha256.len() == 64
        && raw
            .sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    if !hash_ok {
        return Err(invalid(
            &format!("models.{id}.sha256"),
            "must be a full 64-hex-digit lowercase SHA-256",
        ));
    }
    let checkpoint_path = PathBuf::from(&raw.checkpoint_path);
    let engine_model_dir = PathBuf::from(&raw.engine_model_dir);
    if !checkpoint_path.is_absolute() || !engine_model_dir.is_absolute() {
        return Err(invalid(
            &format!("models.{id}"),
            "checkpoint_path and engine_model_dir must be absolute",
        ));
    }
    let name_ok = !raw.engine_name.is_empty()
        && !raw.engine_name.contains('/')
        && raw.engine_name != ".."
        && raw.engine_name != ".";
    if !name_ok {
        return Err(invalid(
            &format!("models.{id}.engine_name"),
            "must be a bare file name",
        ));
    }
    if raw.display_name.trim().is_empty() || raw.source.trim().is_empty() {
        return Err(invalid(
            &format!("models.{id}"),
            "display_name and source must be recorded before admission",
        ));
    }
    Ok(ModelConfig {
        modality: raw.modality,
        checkpoint_path,
        sha256: raw.sha256.clone(),
        engine_name: raw.engine_name.clone(),
        engine_model_dir,
        display_name: raw.display_name.clone(),
        source: raw.source.clone(),
        license: raw.license.clone().filter(|value| !value.trim().is_empty()),
        notes: raw.notes.clone().filter(|value| !value.trim().is_empty()),
    })
}

fn validate_models(raw: &BTreeMap<String, RawModel>) -> Result<BTreeMap<String, ModelConfig>> {
    if raw.is_empty() || raw.len() > MODEL_COUNT_MAX {
        return Err(invalid(
            "models",
            format!("must register 1..={MODEL_COUNT_MAX} models"),
        ));
    }
    let mut models = BTreeMap::new();
    for (id, raw_model) in raw {
        models.insert(id.clone(), validate_model(id, raw_model)?);
    }
    Ok(models)
}

/// Each default must name a registered image model.
fn validate_defaults(
    raw: &RawDefaults,
    models: &BTreeMap<String, ModelConfig>,
) -> Result<ModelDefaults> {
    for (field, id) in [
        ("defaults.txt2img", &raw.txt2img),
        ("defaults.img2img", &raw.img2img),
    ] {
        match models.get(id) {
            Some(model) if model.modality == Modality::Image => {}
            Some(_) => {
                return Err(invalid(
                    field,
                    format!("model `{id}` is not an image model"),
                ));
            }
            None => {
                return Err(invalid(
                    field,
                    format!(
                        "model `{id}` is not registered; registered: {}",
                        registered_ids(models)
                    ),
                ));
            }
        }
    }
    Ok(ModelDefaults {
        txt2img: raw.txt2img.clone(),
        img2img: raw.img2img.clone(),
    })
}

fn validate_limits(raw: &RawLimits) -> Result<Limits> {
    let axis_ok = raw.axis_pixels_max >= 64 && raw.axis_pixels_max <= AXIS_PIXELS_CEILING;
    let total_ok = raw.total_pixels_max >= 64 * 64 && raw.total_pixels_max <= TOTAL_PIXELS_CEILING;
    let steps_ok = raw.steps_max >= 1 && raw.steps_max <= STEPS_CEILING;
    let prompt_ok = raw.prompt_bytes_max >= 1 && raw.prompt_bytes_max <= PROMPT_BYTES_CEILING;
    let source_ok = raw.source_bytes_max >= 1 && raw.source_bytes_max <= SOURCE_BYTES_CEILING;
    let queue_ok = raw.queue_depth_max >= 1 && raw.queue_depth_max <= QUEUE_DEPTH_CEILING;
    let checks = [
        (axis_ok, "limits.axis_pixels_max"),
        (total_ok, "limits.total_pixels_max"),
        (steps_ok, "limits.steps_max"),
        (prompt_ok, "limits.prompt_bytes_max"),
        (source_ok, "limits.source_bytes_max"),
        (queue_ok, "limits.queue_depth_max"),
    ];
    for (ok, field) in checks {
        if !ok {
            return Err(invalid(field, "outside the compiled ceiling"));
        }
    }
    Ok(Limits {
        axis_pixels_max: raw.axis_pixels_max,
        total_pixels_max: raw.total_pixels_max,
        steps_max: raw.steps_max,
        prompt_bytes_max: raw.prompt_bytes_max,
        source_bytes_max: raw.source_bytes_max,
        queue_depth_max: raw.queue_depth_max,
    })
}

impl Config {
    /// Parse and validate. Rejection leaves nothing behind.
    pub fn from_toml_str(text: &str) -> Result<Config> {
        let raw: RawConfig = toml::from_str(text)
            .map_err(|cause| AtelierError::new(ErrorCode::ConfigInvalid, cause.to_string()))?;
        let Some(profile_raw) = raw.profile.as_deref() else {
            return err(
                ErrorCode::ProfileMissing,
                "top-level `profile` is required (qompass or ntranch); there is no default",
            );
        };
        let profile = Profile::parse(profile_raw)?;
        let block = |name: &str, value: &Option<RawProfile>| -> Result<ProfileRoots> {
            match value {
                Some(raw_profile) => validate_profile(name, raw_profile),
                None => err(
                    ErrorCode::ProfileBlockMissing,
                    format!("[profiles.{name}] is required (both blocks, for separation checks)"),
                ),
            }
        };
        let qompass = block("qompass", &raw.profiles.qompass)?;
        let ntranch = block("ntranch", &raw.profiles.ntranch)?;
        check_cross_profile(&qompass, &ntranch)?;
        let state_root = canonical_dir("state_root", &raw.state_root)?;
        for roots in [&qompass, &ntranch] {
            for write_root in [
                &roots.output_root,
                &roots.staged_input_root,
                &roots.log_root,
            ] {
                if overlaps(write_root, &state_root) {
                    return err(
                        ErrorCode::ProfileRootOverlap,
                        format!("state_root overlaps profile root {}", write_root.display()),
                    );
                }
            }
        }
        let models = validate_models(&raw.models)?;
        let defaults = validate_defaults(&raw.defaults, &models)?;
        let config = Config {
            profile,
            state_root,
            qompass,
            ntranch,
            engine: validate_engine(&raw.engine)?,
            models,
            defaults,
            limits: validate_limits(&raw.limits)?,
        };
        config.assert_invariants();
        Ok(config)
    }

    fn assert_invariants(&self) {
        assert!(self.engine.host.is_loopback());
        assert!(!overlaps(
            &self.qompass.output_root,
            &self.ntranch.output_root
        ));
        assert!(!overlaps(&self.qompass.log_root, &self.ntranch.log_root));
        assert_ne!(
            self.qompass.log_file(Profile::Qompass),
            self.ntranch.log_file(Profile::Ntranch)
        );
        assert!(!self.models.is_empty() && self.models.len() <= MODEL_COUNT_MAX);
        assert!(self.models.contains_key(&self.defaults.txt2img));
        assert!(self.models.contains_key(&self.defaults.img2img));
    }

    /// Read and validate one file, bounded by [`CONFIG_BYTES_MAX`].
    pub fn load(path: &Path) -> Result<Config> {
        let metadata = match std::fs::metadata(path) {
            Ok(metadata) => metadata,
            Err(cause) if cause.kind() == std::io::ErrorKind::NotFound => {
                return err(
                    ErrorCode::ConfigMissing,
                    format!(
                        "config {} does not exist; atelier has no default",
                        path.display()
                    ),
                );
            }
            Err(cause) => {
                return err(
                    ErrorCode::ConfigUnreadable,
                    format!("config {}: {cause}", path.display()),
                );
            }
        };
        if !metadata.is_file() {
            return err(
                ErrorCode::ConfigUnreadable,
                format!("config {} is not a regular file", path.display()),
            );
        }
        if metadata.len() > CONFIG_BYTES_MAX {
            return err(
                ErrorCode::ConfigInvalid,
                format!("config exceeds the {CONFIG_BYTES_MAX}-byte limit"),
            );
        }
        let text = std::fs::read_to_string(path).map_err(|cause| {
            AtelierError::new(
                ErrorCode::ConfigUnreadable,
                format!("config {}: {cause}", path.display()),
            )
        })?;
        Config::from_toml_str(&text)
    }
}

/// Resolve the config path: `$ATELIER_CONFIG` if set, else the home default.
pub fn resolve_path(env_value: Option<&str>, home: Option<&str>) -> Result<PathBuf> {
    if let Some(value) = env_value.filter(|value| !value.is_empty()) {
        return Ok(PathBuf::from(value));
    }
    match home.filter(|value| !value.is_empty()) {
        Some(home) => Ok(Path::new(home).join(".config/atelier/atelier.toml")),
        None => err(
            ErrorCode::ConfigMissing,
            "no $ATELIER_CONFIG and no $HOME for the default config path",
        ),
    }
}
