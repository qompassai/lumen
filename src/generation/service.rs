//! The atelier service: tool semantics independent of MCP transport.
//!
//! Contract for a generation job, in order: validate arguments and select
//! the model (pure: the requested id, else the configured default for the
//! tool; an unregistered id is `model_unknown`, never a fallback) → require
//! that model's admission → admit the source (img2img) → take a queue slot
//! (full queue is `engine_busy`) → take the job lock (one job at a time) →
//! create the per-job output dir → start the owned engine → preflight
//! checkpoint visibility and node classes → upload (img2img) → submit →
//! poll history to the job deadline → collect and verify the output → stop
//! the engine → write the manifest (which records the cleanup result).
//! A cleanup failure turns the job into `cleanup_failed` even if the image
//! is good; the output and manifest stay on disk and are named in the error.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{Mutex, Semaphore};

use crate::generation::admission::{Admission, Lane, UNIMPLEMENTED};
use crate::generation::backend::{HistoryState, MediaBackend};
use crate::generation::config::{Config, Modality};
use crate::generation::error::{AtelierError, ErrorCode, Result, err};
use crate::generation::lifecycle::{
    CleanupResult, GpuProbe, Launcher, Lifecycle, SessionEvidence, with_cleanup,
};
use crate::generation::log::{EventLog, now_ms, rfc3339};
use crate::generation::paths::{self, AdmittedSource, ImageInfo};
use crate::generation::template::{
    CFG_MAX, CFG_MIN, DENOISE_MAX, DENOISE_MIN, IRIS_IMG2IMG, IRIS_TXT2IMG, SAMPLERS, SCHEDULERS,
    Template, TemplateIdentity, TemplateKind,
};

pub const HISTORY_POLL: Duration = Duration::from_millis(500);
const SAVE_NODE: &str = "9";

/// Default anime style steering, prepended to every positive prompt
/// (Owner ruling, 2026-10-21: the tag set proven in the anime-style
/// A/B run against the first-cut prompts becomes the server default).
pub const STYLE_POSITIVE: &str = "anime coloring, cel shading, flat color, \
    sharp lineart, vibrant colors, detailed eyes, shiny hair";

/// Default style exclusions, merged into every negative prompt.
pub const STYLE_NEGATIVE: &str = "painterly, oil_painting, watercolor, \
    soft_shading, semi-realistic, muted_colors, dull_colors, airbrushed";

/// Split a comma-separated tag list into trimmed, nonempty tags.
fn tags(text: &str) -> Vec<&str> {
    text.split(',')
        .map(str::trim)
        .filter(|t| !t.is_empty())
        .collect()
}

/// Whether `tag` already appears in `text` (case-insensitive, whole tag).
fn has_tag(text: &str, tag: &str) -> bool {
    tags(text).iter().any(|t| t.eq_ignore_ascii_case(tag))
}

/// The effective positive prompt: the default style tags first, then the
/// caller's prompt, skipping any style tag the caller already supplied.
pub fn style_positive(prompt: &str) -> String {
    let mut out: Vec<&str> = tags(STYLE_POSITIVE)
        .into_iter()
        .filter(|t| !has_tag(prompt, t))
        .collect();
    out.push(prompt.trim());
    out.join(", ")
}

/// The effective negative prompt: the caller's tags, then any default
/// style exclusions the caller did not already supply.
pub fn style_negative(negative: &str) -> String {
    let mut out: Vec<&str> = tags(negative);
    for tag in tags(STYLE_NEGATIVE) {
        if !has_tag(negative, tag) {
            out.push(tag);
        }
    }
    out.join(", ")
}

/// `atelier_txt2img` arguments. Unknown fields (e.g. `profile`) are refused.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Txt2ImgRequest {
    /// Registered image model id (e.g. `iris`, `wai`); absent selects the
    /// configured `defaults.txt2img`.
    pub model: Option<String>,
    pub prompt: String,
    pub negative_prompt: Option<String>,
    /// Positive multiple of 8 within the configured ceilings.
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub seed: Option<u64>,
    pub steps: Option<u32>,
    pub cfg: Option<f64>,
    pub sampler_name: Option<String>,
    pub scheduler: Option<String>,
}

/// `atelier_img2img` arguments. Unknown fields are refused.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Img2ImgRequest {
    /// Registered image model id; absent selects the configured
    /// `defaults.img2img`.
    pub model: Option<String>,
    pub prompt: String,
    pub negative_prompt: Option<String>,
    /// Absolute path of a PNG under one of the active profile's source roots.
    pub source_path: String,
    /// Finite, in [0.05, 1.0] (template operating range inside [0, 1]).
    pub denoise: Option<f64>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub seed: Option<u64>,
    pub steps: Option<u32>,
    pub cfg: Option<f64>,
    pub sampler_name: Option<String>,
    pub scheduler: Option<String>,
}

/// `atelier_models` arguments.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ModelsRequest {
    /// `image`, `video`, or `music`.
    pub lane: Option<String>,
}

/// Parameters actually used (echoed and recorded).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct UsedParams {
    pub prompt: String,
    pub negative_prompt: String,
    pub width: u32,
    pub height: u32,
    pub seed: u64,
    pub steps: u32,
    pub cfg: f64,
    pub sampler_name: String,
    pub scheduler: String,
    pub denoise: f64,
    pub batch_size: u32,
}

struct Common<'a> {
    model: Option<&'a str>,
    prompt: &'a str,
    negative_prompt: Option<&'a str>,
    width: Option<u32>,
    height: Option<u32>,
    seed: Option<u64>,
    steps: Option<u32>,
    cfg: Option<f64>,
    sampler_name: Option<&'a str>,
    scheduler: Option<&'a str>,
    denoise: Option<f64>,
}

fn bad(message: impl Into<String>) -> AtelierError {
    AtelierError::new(ErrorCode::ArgumentInvalid, message)
}

fn default_u64(template: &Template, name: &str) -> u64 {
    let value = template.default_of(name).as_u64();
    assert!(value.is_some(), "template default {name} is not an integer");
    value.unwrap_or_default()
}

fn default_f64(template: &Template, name: &str) -> f64 {
    let value = template.default_of(name).as_f64();
    assert!(value.is_some(), "template default {name} is not a number");
    value.unwrap_or_default()
}

fn default_str(template: &Template, name: &str) -> String {
    template
        .default_of(name)
        .as_str()
        .unwrap_or_default()
        .to_string()
}

fn checked_u32(value: u64, name: &str) -> Result<u32> {
    u32::try_from(value).map_err(|_| bad(format!("{name} out of range")))
}

/// The model id a generation uses: `requested` if given, else the configured
/// default for `kind`. An unregistered id is `model_unknown` naming the
/// registered ids; there is no fallback.
pub fn select_model<'a>(
    config: &'a Config,
    kind: TemplateKind,
    requested: Option<&'a str>,
) -> Result<&'a str> {
    let id = requested.unwrap_or(match kind {
        TemplateKind::Txt2Img => &config.defaults.txt2img,
        TemplateKind::Img2Img => &config.defaults.img2img,
    });
    let model = config.model(id)?;
    // Config admits only image models; the generation tools serve only them.
    assert_eq!(model.modality, Modality::Image);
    Ok(id)
}

fn resolve_params(config: &Config, template: &Template, args: &Common) -> Result<UsedParams> {
    select_model(config, template.kind, args.model)?;
    let limits = &config.limits;
    let negative = args.negative_prompt.unwrap_or_default();
    if args.prompt.trim().is_empty() {
        return Err(bad("prompt must be nonempty"));
    }
    if args.prompt.len() > limits.prompt_bytes_max || negative.len() > limits.prompt_bytes_max {
        return Err(bad(format!(
            "prompt or negative_prompt exceeds {} bytes",
            limits.prompt_bytes_max
        )));
    }
    // The byte ceilings above bind what the caller sends; the default
    // style steering is a fixed server-side addition applied after them.
    let prompt = style_positive(args.prompt);
    let negative = style_negative(negative);
    let width = args
        .width
        .unwrap_or(checked_u32(default_u64(template, "width"), "width")?);
    let height = args
        .height
        .unwrap_or(checked_u32(default_u64(template, "height"), "height")?);
    for (name, axis) in [("width", width), ("height", height)] {
        if axis == 0 || axis % 8 != 0 || axis > limits.axis_pixels_max {
            return Err(bad(format!(
                "{name} {axis} must be a positive multiple of 8 <= {}",
                limits.axis_pixels_max
            )));
        }
    }
    if u64::from(width) * u64::from(height) > limits.total_pixels_max {
        return Err(bad(format!(
            "{width}x{height} exceeds {} total pixels",
            limits.total_pixels_max
        )));
    }
    let steps = args
        .steps
        .unwrap_or(checked_u32(default_u64(template, "steps"), "steps")?);
    if steps == 0 || steps > limits.steps_max {
        return Err(bad(format!("steps must be 1..={}", limits.steps_max)));
    }
    let cfg = args.cfg.unwrap_or(default_f64(template, "cfg"));
    if !cfg.is_finite() || !(CFG_MIN..=CFG_MAX).contains(&cfg) {
        return Err(bad(format!("cfg must be finite in [{CFG_MIN}, {CFG_MAX}]")));
    }
    let sampler_name = args
        .sampler_name
        .map_or_else(|| default_str(template, "sampler_name"), str::to_string);
    if !SAMPLERS.contains(&sampler_name.as_str()) {
        return Err(bad(format!("sampler_name must be one of {SAMPLERS:?}")));
    }
    let scheduler = args
        .scheduler
        .map_or_else(|| default_str(template, "scheduler"), str::to_string);
    if !SCHEDULERS.contains(&scheduler.as_str()) {
        return Err(bad(format!("scheduler must be one of {SCHEDULERS:?}")));
    }
    let denoise = match template.kind {
        TemplateKind::Txt2Img => {
            assert!(args.denoise.is_none());
            1.0
        }
        TemplateKind::Img2Img => {
            let denoise = args.denoise.unwrap_or(default_f64(template, "denoise"));
            if !denoise.is_finite() || !(DENOISE_MIN..=DENOISE_MAX).contains(&denoise) {
                return Err(bad(format!(
                    "denoise must be finite in [{DENOISE_MIN}, {DENOISE_MAX}]"
                )));
            }
            denoise
        }
    };
    Ok(UsedParams {
        prompt,
        negative_prompt: negative,
        width,
        height,
        seed: args.seed.unwrap_or(default_u64(template, "seed")),
        steps,
        cfg,
        sampler_name,
        scheduler,
        denoise,
        batch_size: 1,
    })
}

pub fn txt2img_params(
    config: &Config,
    template: &Template,
    request: &Txt2ImgRequest,
) -> Result<UsedParams> {
    resolve_params(
        config,
        template,
        &Common {
            model: request.model.as_deref(),
            prompt: &request.prompt,
            negative_prompt: request.negative_prompt.as_deref(),
            width: request.width,
            height: request.height,
            seed: request.seed,
            steps: request.steps,
            cfg: request.cfg,
            sampler_name: request.sampler_name.as_deref(),
            scheduler: request.scheduler.as_deref(),
            denoise: None,
        },
    )
}

pub fn img2img_params(
    config: &Config,
    template: &Template,
    request: &Img2ImgRequest,
) -> Result<UsedParams> {
    resolve_params(
        config,
        template,
        &Common {
            model: request.model.as_deref(),
            prompt: &request.prompt,
            negative_prompt: request.negative_prompt.as_deref(),
            width: request.width,
            height: request.height,
            seed: request.seed,
            steps: request.steps,
            cfg: request.cfg,
            sampler_name: request.sampler_name.as_deref(),
            scheduler: request.scheduler.as_deref(),
            denoise: request.denoise,
        },
    )
}

/// Build the patched graph for a job.
pub fn build_graph(
    template: &Template,
    params: &UsedParams,
    checkpoint_name: &str,
    job_id: &str,
    uploaded_image: Option<&str>,
) -> serde_json::Map<String, Value> {
    let mut values: Vec<(&str, Value)> = vec![
        ("checkpoint", json!(checkpoint_name)),
        ("positive", json!(params.prompt)),
        ("negative", json!(params.negative_prompt)),
        ("width", json!(params.width)),
        ("height", json!(params.height)),
        ("seed", json!(params.seed)),
        ("steps", json!(params.steps)),
        ("cfg", json!(params.cfg)),
        ("sampler_name", json!(params.sampler_name)),
        ("scheduler", json!(params.scheduler)),
        ("denoise", json!(params.denoise)),
        ("filename_prefix", json!(format!("atelier-{job_id}"))),
    ];
    match (template.kind, uploaded_image) {
        (TemplateKind::Img2Img, Some(name)) => values.push(("image", json!(name))),
        (TemplateKind::Txt2Img, None) => {}
        _ => panic!("uploaded image presence must match the template kind"),
    }
    template.patch(&values)
}

/// Output facts returned and recorded.
#[derive(Debug, Clone, Serialize)]
pub struct OutputRecord {
    pub path: PathBuf,
    pub filename: String,
    pub sha256: String,
    pub bytes: u64,
    #[serde(flatten)]
    pub image: ImageInfo,
}

#[derive(Debug, Clone, Serialize)]
pub struct SourceRecord {
    pub reference: String,
    pub sha256: String,
    pub bytes: u64,
    pub width: u32,
    pub height: u32,
    pub staged_path: PathBuf,
    pub engine_upload: crate::generation::backend::UploadedImage,
}

/// The successful generation result (also the manifest core).
#[derive(Debug, Clone, Serialize)]
pub struct GenerationResult {
    pub job_id: String,
    pub profile: &'static str,
    pub content_class: &'static str,
    pub prompt_id: String,
    pub model: String,
    pub parameters: UsedParams,
    pub source: Option<SourceRecord>,
    pub output: OutputRecord,
    pub elapsed_ms: u64,
    pub manifest_path: PathBuf,
    pub license_alert: bool,
    pub cleanup: CleanupResult,
}

#[derive(Debug, Clone, Serialize)]
struct LastJob {
    job_id: String,
    ok: bool,
    code: Option<&'static str>,
    finished: String,
}

#[derive(Debug, Default)]
struct Observed {
    current_job: Option<String>,
    last_job: Option<LastJob>,
    last_cleanup: Option<CleanupResult>,
    last_evidence: Option<SessionEvidence>,
}

pub struct Atelier<B: MediaBackend, L: Launcher, G: GpuProbe> {
    pub config: Config,
    /// One record per registered model; keys equal `config.models` keys.
    pub admissions: BTreeMap<String, Admission>,
    pub backend: B,
    lifecycle: Mutex<Lifecycle<L, G>>,
    queue: Semaphore,
    observed: StdMutex<Observed>,
    pub log: EventLog,
    txt2img: Template,
    img2img: Template,
    job_counter: AtomicU64,
}

impl<B: MediaBackend, L: Launcher, G: GpuProbe> Atelier<B, L, G> {
    pub fn new(
        config: Config,
        admissions: BTreeMap<String, Admission>,
        backend: B,
        launcher: L,
        gpu: G,
    ) -> Self {
        assert!(
            admissions.keys().eq(config.models.keys()),
            "admissions must cover exactly the registered models"
        );
        let log = EventLog::open(
            config.active_roots().log_file(config.profile),
            config.profile,
        );
        let lifecycle = Lifecycle::new(
            config.engine.clone(),
            config.state_root.clone(),
            launcher,
            gpu,
        );
        let queue = Semaphore::new(config.limits.queue_depth_max);
        let atelier = Atelier {
            admissions,
            backend,
            lifecycle: Mutex::new(lifecycle),
            queue,
            observed: StdMutex::new(Observed::default()),
            log,
            txt2img: Template::embedded(&IRIS_TXT2IMG),
            img2img: Template::embedded(&IRIS_IMG2IMG),
            job_counter: AtomicU64::new(0),
            config,
        };
        for admission in atelier.admissions.values() {
            atelier.log.event(
                if admission.license_alert {
                    "warn"
                } else {
                    "info"
                },
                "startup",
                json!({
                    "model": admission.model,
                    "admitted": admission.admitted,
                    "license": admission.license,
                    "license_alert": admission.license_alert,
                }),
            );
        }
        atelier
    }

    /// The admission record of a registered model id.
    pub fn admission(&self, id: &str) -> Result<&Admission> {
        self.config.model(id)?;
        // `new` asserted the admission keys equal the registered ids.
        Ok(&self.admissions[id])
    }

    fn observed(&self) -> std::sync::MutexGuard<'_, Observed> {
        // A poisoned observation mutex only means a panic mid-update of
        // status fields; the data is still the best record we have.
        self.observed
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn template(&self, kind: TemplateKind) -> &Template {
        match kind {
            TemplateKind::Txt2Img => &self.txt2img,
            TemplateKind::Img2Img => &self.img2img,
        }
    }

    fn next_job_id(&self) -> String {
        let counter = self.job_counter.fetch_add(1, Ordering::Relaxed);
        let stamp = rfc3339(now_ms())
            .replace([':', '.'], "")
            .replace('T', "t")
            .replace('Z', "z");
        format!(
            "j{}-{}-{counter}",
            stamp.to_ascii_lowercase(),
            std::process::id()
        )
    }

    /// `atelier_status`. Never starts the engine.
    pub async fn status(&self) -> Value {
        let (engine_state, owned, degraded, gpu) = {
            let lifecycle = self.lifecycle.lock().await;
            (
                lifecycle.state().await,
                lifecycle.owns_engine(),
                lifecycle.degraded_reason().map(str::to_string),
                lifecycle.gpu_snapshot().await,
            )
        };
        let in_use = self.config.limits.queue_depth_max - self.queue.available_permits();
        let observed = self.observed();
        json!({
            "profile": self.config.profile.as_str(),
            "content_class": self.config.profile.content_class(),
            "server": {"name": "atelier", "version": env!("CARGO_PKG_VERSION")},
            "config": {"status": "valid", "state_root": self.config.state_root},
            "backend": self.backend.kind(),
            "engine": {
                "state": engine_state,
                "lifecycle_owner": if owned { "atelier" } else { "none" },
                "degraded_reason": degraded,
                "address": self.config.engine.base_url(),
            },
            "gpu": {
                "index": self.config.engine.gpu_index,
                "expected_name": self.config.engine.expected_gpu_name,
                "snapshot": gpu,
            },
            "current_job": observed.current_job,
            "queue_depth": in_use,
            "queue_depth_max": self.config.limits.queue_depth_max,
            "admitted_models": self.admissions.values()
                .filter(|admission| admission.admitted)
                .map(|admission| admission.model.as_str())
                .collect::<Vec<_>>(),
            "last_job": observed.last_job,
            "last_cleanup": observed.last_cleanup,
            "last_engine_evidence": observed.last_evidence,
            "log_write_failures": self.log.write_failures.load(Ordering::Relaxed),
        })
    }

    /// `atelier_models`. Lists every registered model (admitted ones under
    /// `models`, the rest under `not_admitted`) with its modality and
    /// availability (checkpoint file present now). Never starts the engine.
    pub fn models(&self, request: &ModelsRequest) -> Result<Value> {
        let lane = request.lane.as_deref().map(Lane::parse).transpose()?;
        let mut models = Vec::new();
        let mut not_admitted = Vec::new();
        for (id, model) in &self.config.models {
            let model_lane = match model.modality {
                Modality::Image => Lane::Image,
            };
            if !lane.is_none_or(|lane| lane == model_lane) {
                continue;
            }
            let admission = self.admission(id)?;
            let available =
                std::fs::metadata(&model.checkpoint_path).is_ok_and(|metadata| metadata.is_file());
            if admission.admitted {
                models.push(json!({
                    "model": id,
                    "modality": model.modality,
                    "available": available,
                    "lane": model_lane,
                    "capabilities": ["txt2img", "img2img"],
                    "display_name": admission.display_name,
                    "source": admission.source,
                    "notes": model.notes,
                    "checkpoint": admission.canonical_path,
                    "sha256": admission.expected_sha256,
                    "hash_check": "match",
                    "license": admission.license,
                    "license_alert": admission.license_alert,
                    "backend": self.backend.kind(),
                    "backend_readiness": "on_demand",
                }));
            } else {
                not_admitted.push(json!({
                    "model": id,
                    "modality": model.modality,
                    "available": available,
                    "refusal_code": admission.refusal_code,
                    "refusal": admission.refusal,
                }));
            }
        }
        let unimplemented: Vec<Value> = UNIMPLEMENTED
            .iter()
            .filter(|(_, entry_lane)| lane.is_none_or(|lane| lane == *entry_lane))
            .map(|(name, entry_lane)| json!({"model": name, "lane": entry_lane, "status": "unimplemented"}))
            .collect();
        Ok(json!({
            "profile": self.config.profile.as_str(),
            "defaults": self.config.defaults,
            "models": models,
            "not_admitted": not_admitted,
            "unimplemented": unimplemented,
        }))
    }

    pub async fn txt2img(&self, request: Txt2ImgRequest) -> Result<GenerationResult> {
        let params = txt2img_params(&self.config, &self.txt2img, &request)?;
        let model = select_model(
            &self.config,
            TemplateKind::Txt2Img,
            request.model.as_deref(),
        )?;
        self.admission(model)?.require()?;
        self.run(TemplateKind::Txt2Img, model, params, None).await
    }

    pub async fn img2img(&self, request: Img2ImgRequest) -> Result<GenerationResult> {
        let params = img2img_params(&self.config, &self.img2img, &request)?;
        let model = select_model(
            &self.config,
            TemplateKind::Img2Img,
            request.model.as_deref(),
        )?;
        self.admission(model)?.require()?;
        let source = paths::admit_source(
            &request.source_path,
            &self.config.active_roots().source_roots,
            self.config.limits.source_bytes_max,
        )?;
        self.run(TemplateKind::Img2Img, model, params, Some(source))
            .await
    }

    async fn run(
        &self,
        kind: TemplateKind,
        model: &str,
        params: UsedParams,
        source: Option<AdmittedSource>,
    ) -> Result<GenerationResult> {
        let Ok(_slot) = self.queue.try_acquire() else {
            return err(ErrorCode::EngineBusy, "atelier job queue is full");
        };
        let mut lifecycle = self.lifecycle.lock().await;
        let job_id = self.next_job_id();
        self.observed().current_job = Some(job_id.clone());
        let started_ms = now_ms();
        let clock = Instant::now();
        self.log.event(
            "info",
            "job_start",
            json!({"job_id": job_id, "model": model, "template": format!("{kind:?}")}),
        );
        let outcome = self
            .execute(
                &mut lifecycle,
                kind,
                model,
                &job_id,
                &params,
                source.as_ref(),
            )
            .await;
        let cleanup = lifecycle.stop().await;
        let evidence = lifecycle.evidence.clone();
        drop(lifecycle);
        let result = match outcome {
            Ok(collected) => self.finish(
                collected,
                model,
                &job_id,
                params,
                started_ms,
                clock,
                cleanup.clone(),
                &evidence,
            ),
            Err(error) => Err(with_cleanup(error, &cleanup)),
        };
        {
            let mut observed = self.observed();
            observed.current_job = None;
            observed.last_cleanup = Some(cleanup.clone());
            observed.last_evidence = Some(evidence);
            observed.last_job = Some(LastJob {
                job_id: job_id.clone(),
                ok: result.is_ok(),
                code: result.as_ref().err().map(|error| error.code.as_str()),
                finished: rfc3339(now_ms()),
            });
        }
        self.log.event(
            if cleanup.ok { "info" } else { "error" },
            "engine_cleanup",
            json!({"job_id": job_id, "ok": cleanup.ok, "detail": cleanup.detail}),
        );
        match &result {
            Ok(done) => self.log.event(
                "info",
                "job_done",
                json!({"job_id": job_id,
                "prompt_id": done.prompt_id, "output_sha256": done.output.sha256}),
            ),
            Err(error) => self.log.event(
                "error",
                "job_failed",
                json!({"job_id": job_id,
                "code": error.code.as_str(), "message": error.message}),
            ),
        }
        result
    }
}

/// What `execute` hands to `finish`.
pub struct Collected {
    prompt_id: String,
    output: OutputRecord,
    source: Option<SourceRecord>,
    template: TemplateIdentity,
    job_dir: PathBuf,
}

impl<B: MediaBackend, L: Launcher, G: GpuProbe> Atelier<B, L, G> {
    async fn execute(
        &self,
        lifecycle: &mut Lifecycle<L, G>,
        kind: TemplateKind,
        model: &str,
        job_id: &str,
        params: &UsedParams,
        source: Option<&AdmittedSource>,
    ) -> Result<Collected> {
        let roots = self.config.active_roots();
        let template = self.template(kind);
        let engine_name = &self.config.model(model)?.engine_name;
        lifecycle.start(&self.backend).await?;
        self.preflight(template, engine_name).await?;
        let job_dir = paths::create_job_dir(&roots.output_root, job_id)?;
        let mut source_record = None;
        let mut uploaded_name = None;
        if let Some(source) = source {
            let staged_dir = paths::create_job_dir(&roots.staged_input_root, job_id)?;
            let staged = paths::write_new(
                &roots.staged_input_root,
                &staged_dir,
                "source.png",
                &source.bytes,
            )?;
            let upload = self
                .backend
                .upload_image(&format!("atelier-{job_id}.png"), source.bytes.clone())
                .await?;
            if !upload.subfolder.is_empty() || upload.kind != "input" {
                return err(
                    ErrorCode::BackendProtocol,
                    format!("unexpected upload placement {upload:?}"),
                );
            }
            self.log.event("info", "source_uploaded", json!({"job_id": job_id,
                "source": source.label, "source_sha256": source.sha256, "engine_name": upload.name}));
            uploaded_name = Some(upload.name.clone());
            source_record = Some(SourceRecord {
                reference: source.label.clone(),
                sha256: source.sha256.clone(),
                bytes: source.bytes.len() as u64,
                width: source.width,
                height: source.height,
                staged_path: staged,
                engine_upload: upload,
            });
        }
        let graph = build_graph(
            template,
            params,
            engine_name,
            job_id,
            uploaded_name.as_deref(),
        );
        let prompt_id = self.backend.submit(graph, job_id).await?;
        self.log.event(
            "info",
            "prompt_submitted",
            json!({"job_id": job_id, "prompt_id": prompt_id}),
        );
        let descriptors = self.await_history(lifecycle, &prompt_id).await;
        let descriptors = match descriptors {
            Ok(descriptors) => descriptors,
            Err(error) => {
                if lifecycle.check_alive().is_ok() {
                    // Best-effort: the engine is stopped right after anyway.
                    let interrupted = self.backend.interrupt().await.is_ok();
                    self.log.event(
                        "warn",
                        "prompt_interrupted",
                        json!({"job_id": job_id,
                        "prompt_id": prompt_id, "interrupt_ok": interrupted}),
                    );
                }
                return Err(error);
            }
        };
        let output = self
            .collect(&descriptors, &job_dir, &roots.output_root)
            .await?;
        lifecycle.evidence.gpu_after_collection = lifecycle.gpu_snapshot().await;
        Ok(Collected {
            prompt_id,
            output,
            source: source_record,
            template: template.identity(),
            job_dir,
        })
    }

    async fn preflight(&self, template: &Template, engine_name: &str) -> Result<()> {
        let checkpoints = self.backend.checkpoints().await?;
        if !checkpoints.iter().any(|listed| listed == engine_name) {
            return err(
                ErrorCode::CheckpointNotVisible,
                format!("engine does not list {engine_name}"),
            );
        }
        for class in template.node_classes() {
            if !self.backend.has_node_class(&class).await? {
                return err(
                    ErrorCode::NodeClassMissing,
                    format!("engine lacks node class {class}"),
                );
            }
        }
        Ok(())
    }

    async fn await_history(
        &self,
        lifecycle: &mut Lifecycle<L, G>,
        prompt_id: &str,
    ) -> Result<Vec<crate::generation::backend::OutputDescriptor>> {
        let deadline = Instant::now() + Duration::from_secs(self.config.engine.job_timeout_s);
        loop {
            lifecycle.check_alive()?;
            match self.backend.history(prompt_id, SAVE_NODE).await? {
                HistoryState::Success(descriptors) => return Ok(descriptors),
                HistoryState::Pending => {
                    if !self.backend.queue().await?.contains(prompt_id) {
                        // Re-read once: completion may land between the two reads.
                        if let HistoryState::Success(descriptors) =
                            self.backend.history(prompt_id, SAVE_NODE).await?
                        {
                            return Ok(descriptors);
                        }
                        return err(
                            ErrorCode::HistoryMissing,
                            format!("prompt {prompt_id} is neither queued nor in history"),
                        );
                    }
                }
            }
            if Instant::now() >= deadline {
                return err(
                    ErrorCode::JobTimeout,
                    format!("job exceeded {} s", self.config.engine.job_timeout_s),
                );
            }
            tokio::time::sleep(HISTORY_POLL).await;
        }
    }

    async fn collect(
        &self,
        descriptors: &[crate::generation::backend::OutputDescriptor],
        job_dir: &std::path::Path,
        output_root: &std::path::Path,
    ) -> Result<OutputRecord> {
        let [descriptor] = descriptors else {
            return err(
                ErrorCode::OutputDescriptorMissing,
                format!(
                    "expected exactly 1 output (batch 1), got {}",
                    descriptors.len()
                ),
            );
        };
        if descriptor.kind != "output" {
            return err(
                ErrorCode::OutputEscapeRefused,
                format!("output type `{}`", descriptor.kind),
            );
        }
        let filename = paths::safe_output_name(&descriptor.filename, &descriptor.subfolder)?;
        let bytes = self.backend.view(descriptor).await?;
        let image = paths::decode_png(&bytes)?;
        let path = paths::write_new(output_root, job_dir, &filename, &bytes)?;
        Ok(OutputRecord {
            path,
            filename,
            sha256: crate::generation::admission::sha256_bytes(&bytes),
            bytes: bytes.len() as u64,
            image,
        })
    }

    #[allow(clippy::too_many_arguments)]
    fn finish(
        &self,
        collected: Collected,
        model: &str,
        job_id: &str,
        params: UsedParams,
        started_ms: u64,
        clock: Instant,
        cleanup: CleanupResult,
        evidence: &SessionEvidence,
    ) -> Result<GenerationResult> {
        let admission = self.admission(model)?;
        let elapsed_ms = u64::try_from(clock.elapsed().as_millis()).unwrap_or(u64::MAX);
        let result = GenerationResult {
            job_id: job_id.to_string(),
            profile: self.config.profile.as_str(),
            content_class: self.config.profile.content_class(),
            prompt_id: collected.prompt_id,
            model: model.to_string(),
            parameters: params,
            source: collected.source,
            output: collected.output,
            elapsed_ms,
            manifest_path: collected.job_dir.join("manifest.json"),
            license_alert: admission.license_alert,
            cleanup: cleanup.clone(),
        };
        let manifest = json!({
            "manifest_version": 1,
            "job_id": result.job_id,
            "profile": result.profile,
            "content_class": result.content_class,
            "model": {
                "key": model,
                "display_name": admission.display_name,
                "source": admission.source,
                "checkpoint": admission.canonical_path,
                "sha256": admission.expected_sha256,
                "engine_name": admission.engine_name,
                "license": admission.license,
                "license_alert": admission.license_alert,
                "cleared_for_production": !admission.license_alert,
            },
            "backend": {
                "kind": self.backend.kind(),
                "engine_version": evidence.engine_version,
                "engine_device": evidence.engine_device,
                "prompt_id": result.prompt_id,
            },
            "template": collected.template,
            "parameters": result.parameters,
            "source": result.source,
            "output": result.output,
            "started": rfc3339(started_ms),
            "finished": rfc3339(now_ms()),
            "elapsed_ms": elapsed_ms,
            "lifecycle": {"cleanup": cleanup, "evidence": evidence},
        });
        let text = serde_json::to_string_pretty(&manifest)
            .map_err(|cause| AtelierError::new(ErrorCode::OutputWriteFailed, cause.to_string()))?;
        paths::write_new(
            &self.config.active_roots().output_root,
            &collected.job_dir,
            "manifest.json",
            text.as_bytes(),
        )?;
        if !cleanup.ok {
            return err(
                ErrorCode::CleanupFailed,
                format!(
                    "image collected at {} (manifest {}) but engine cleanup failed: {}",
                    result.output.path.display(),
                    result.manifest_path.display(),
                    cleanup.detail
                ),
            );
        }
        Ok(result)
    }
}
