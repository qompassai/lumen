//! lumen — "Lux in motu" (light in motion).
//!
//! MCP stdio server for sprite artistry (default, no subcommand), plus a
//! pipeline CLI: `split`, `pack`, `clean`, `watch`, and `policy` tools.
//! Tool logic lives in the `lumen` library crate; this binary is transport
//! and argument wiring only.
//!
//! As of 0.2.0 (WS2 of the MCP standardization program, decision
//! D-113) the MCP framing runs on rmcp 3.8.0 and speaks the modern
//! protocol era only (2026-07-28): sessions open with
//! `server/discover` and every request carries its protocol
//! version in `_meta` — there is no `initialize` handshake.
//! Legacy contact is refused at the stdio front door (see
//! `lumen::guard`) with the typed `-32022` unsupported-version
//! error. The tools, the policy gate, and the CLI are unchanged.
//!
//! As of 0.3.0 the atelier image-generation lane is folded in
//! (D-143): `atelier_status`, `atelier_models`, `atelier_txt2img`,
//! and `atelier_img2img` are served from `lumen::generation` with
//! atelier's names, schemas, and semantics; atelier retires as a
//! standalone server. Images only (D-144/D-145) — no video or music
//! tools existed in atelier or are added here.

#![forbid(unsafe_code)]

use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use lumen::cli::{self, CLI_DEFAULT_HEIGHT, CLI_DEFAULT_WIDTH};
use lumen::generation::{
    self, GenerationLane, GenerationSetup,
    service::{Img2ImgRequest, ModelsRequest, Txt2ImgRequest},
};
use lumen::guard;
use lumen::policy::{self, DEFAULT_POLICY_TOML, Enforcer, Policy, PolicyError};
use lumen::style::{StyleApplyReq, StyleListReq, style_apply, style_list};
use lumen::vrm::{VrmGradeReq, VrmInspectReq, vrm_grade, vrm_inspect};
use lumen::{bevy_discover, project_root, sprite_info, validate_atlas};
use rmcp::{
    ErrorData, RoleServer, ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, tool::ToolCallContext, wrapper::Parameters},
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, Implementation, IntoContents,
        ProtocolVersion, ServerCapabilities, ServerConfig,
    },
    schemars,
    service::RequestContext,
    tool, tool_handler, tool_router,
    transport::async_rw::AsyncRwTransport,
};

#[derive(Parser)]
#[command(
    name = "lumen",
    version,
    about = "Lux in motu — sprite artistry MCP server and pipeline CLI"
)]
struct Cli {
    /// Subcommand. With none, serves the MCP server on stdio.
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Subcommand)]
enum Command {
    /// Policy tools: check labels, replay scenarios.
    Policy {
        #[command(subcommand)]
        action: PolicyAction,
    },
    /// Split a horizontal strip into cleaned, centered frame PNGs.
    Split {
        /// Strip image path.
        input: PathBuf,
        /// Frames in the strip (width must divide evenly).
        #[arg(long)]
        frames: u32,
        /// Output directory for frame_*.png.
        #[arg(long)]
        out_dir: PathBuf,
        /// Output frame width.
        #[arg(long, default_value_t = CLI_DEFAULT_WIDTH)]
        width: u32,
        /// Output frame height.
        #[arg(long, default_value_t = CLI_DEFAULT_HEIGHT)]
        height: u32,
    },
    /// Pack PNGs from a directory into a sprite sheet + JSON sidecar.
    Pack {
        /// Directory of frame PNGs.
        dir: PathBuf,
        /// Output sheet path stem (`out` -> `out.png` + `out.json`).
        #[arg(long)]
        out: PathBuf,
        /// Starting bin size; doubles until everything fits.
        #[arg(long, default_value_t = 1024)]
        bin_size: u32,
    },
    /// Clean transparency on every PNG in a directory.
    Clean {
        /// Directory of PNGs.
        dir: PathBuf,
        /// Output directory.
        #[arg(long)]
        out_dir: PathBuf,
        /// Also key the border-connected background via connected
        /// components (for solid white panels).
        #[arg(long, default_value_t = false)]
        connected_components: bool,
        /// Use magenta chrominance matting (for #FF00FF backgrounds).
        #[arg(long, default_value_t = false)]
        magenta: bool,
    },
    /// Watch a directory; re-run clean/extract on change until killed.
    Watch {
        /// Directory to watch (recursive).
        dir: PathBuf,
    },
    /// Upscale PNGs 2x via Real-ESRGAN ONNX.
    Upscale {
        /// Directory of PNGs.
        dir: PathBuf,
        /// Output directory.
        #[arg(long)]
        out: PathBuf,
        /// Path to Real-ESRGAN .onnx model.
        #[arg(long)]
        model: PathBuf,
    },
    /// Export one animation per tag from an .aseprite file.
    SplitTags {
        /// Input .aseprite file.
        input: PathBuf,
        /// Output directory.
        #[arg(long)]
        out: PathBuf,
        /// Output format: "png" (sequence + JSON) or "gif".
        #[arg(long, default_value = "png")]
        format: String,
        /// Prefix prepended to every output name.
        #[arg(long, default_value = "")]
        prefix: String,
        /// Overwrite a non-empty output directory.
        #[arg(long, default_value_t = false)]
        force: bool,
    },
}

#[derive(Subcommand)]
enum PolicyAction {
    /// Exit 0 iff the policy loads and every tool is labeled.
    Check,
    /// Exit 0 iff every policy scenario decides as expected.
    Replay,
}

/// The generation lane's runtime state: live under a loaded atelier
/// config, or registered-but-unconfigured when no default config
/// exists (see `lumen::generation` for the startup posture).
#[derive(Debug, Clone)]
pub enum GenerationState {
    Ready(Arc<GenerationLane>),
    Unconfigured { expected_path: PathBuf },
}

#[derive(Debug, Clone)]
pub struct Lumen {
    tool_router: ToolRouter<Self>,
    /// Policy check run before every tools/call; shared by clones so the
    /// session's label history is one per server process.
    enforcer: Arc<Enforcer>,
    /// The folded atelier image lane (D-143).
    generation: GenerationState,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct SpriteInfoRequest {
    /// Project-relative path to a .png sprite.
    pub path: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct ValidateAtlasRequest {
    /// Project-relative path to a candidate Light Show atlas .png.
    pub path: String,
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
pub struct BevyDiscoverRequest {
    /// BRP endpoint URL. Defaults to http://127.0.0.1:15702.
    /// Non-loopback hosts are rejected unless LUMEN_ALLOW_REMOTE_BRP=1.
    pub endpoint: Option<String>,
}

fn ok_json<T: serde::Serialize>(v: &T) -> String {
    serde_json::to_string_pretty(v).unwrap_or_else(|e| {
        serde_json::json!({"error": format!("serialization failed: {e}")}).to_string()
    })
}

fn err_json(e: impl std::fmt::Display) -> String {
    serde_json::json!({"error": e.to_string()}).to_string()
}

impl Lumen {
    /// The live generation lane, or the per-call `config_missing`
    /// refusal every generation tool serves while unconfigured.
    fn generation_lane(&self) -> Result<&GenerationLane, String> {
        match &self.generation {
            GenerationState::Ready(lane) => Ok(lane),
            GenerationState::Unconfigured { expected_path } => {
                Err(generation::unconfigured_refusal(expected_path))
            }
        }
    }
}

#[tool_router]
impl Lumen {
    pub fn new(enforcer: Arc<Enforcer>, generation: GenerationState) -> Self {
        Self {
            tool_router: Self::tool_router(),
            enforcer,
            generation,
        }
    }

    #[tool(
        description = "Report width, height, color type and alpha presence of a project .png sprite."
    )]
    fn sprite_info(&self, Parameters(req): Parameters<SpriteInfoRequest>) -> String {
        match project_root().and_then(|root| sprite_info(&root, &req.path)) {
            Ok(info) => ok_json(&info),
            Err(e) => err_json(e),
        }
    }

    #[tool(
        description = "Validate a .png against the Light Show atlas contract: 384x1152, 4x6 grid of 96x192 cells, RGBA."
    )]
    fn validate_atlas(&self, Parameters(req): Parameters<ValidateAtlasRequest>) -> String {
        match project_root().and_then(|root| validate_atlas(&root, &req.path)) {
            Ok(info) => ok_json(&info),
            Err(e) => err_json(e),
        }
    }

    #[tool(
        description = "Probe a Bevy Remote Protocol endpoint and classify it: BRP-confirmed, JSON-RPC-speaking, unreachable, or not-BRP."
    )]
    async fn bevy_discover(&self, Parameters(req): Parameters<BevyDiscoverRequest>) -> String {
        match bevy_discover(req.endpoint.as_deref()).await {
            Ok(d) => ok_json(&d),
            Err(e) => err_json(e),
        }
    }

    // Generation lane folded from atelier (D-143): names, descriptions,
    // annotations, and semantics are atelier's, unchanged; the tool
    // semantics live in lumen::generation::service. Images only
    // (D-144/D-145): atelier never shipped video or music tools.
    #[tool(
        description = "Report atelier's active profile, version, config status, backend, engine state (stopped/owned_running/unowned_present/degraded), lifecycle owner, GPU, current job, queue depth, admitted models, and last job/cleanup result. Never starts the engine.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    async fn atelier_status(&self) -> Result<String, String> {
        let lane = self.generation_lane()?;
        Ok(generation::to_json(&lane.core().status().await))
    }

    #[tool(
        description = "List every registered model with its modality and availability (checkpoint file present): admitted ones under `models` (backend readiness, pinned-hash result), the rest under `not_admitted` with the refusal; plus the per-tool `defaults` and unimplemented roster names. Optional lane: image, video, or music. Never starts the engine.",
        annotations(read_only_hint = true, open_world_hint = false)
    )]
    fn atelier_models(
        &self,
        Parameters(request): Parameters<ModelsRequest>,
    ) -> Result<String, String> {
        let lane = self.generation_lane()?;
        lane.core()
            .models(&request)
            .map(|value| generation::to_json(&value))
            .map_err(|error| lane.refusal(&error))
    }

    #[tool(
        description = "Generate one image from a text prompt with a registered image model (`model` id, e.g. iris or wai; omitted = the configured txt2img default; an unregistered id is refused as model_unknown) through the pinned txt2img template. The server prepends its default anime style tags (anime coloring, cel shading, flat color, sharp lineart, vibrant colors, detailed eyes, shiny hair) to the prompt and merges default style exclusions (painterly, watercolor, soft shading, semi-realistic, muted/dull colors, airbrushed) into the negative prompt, skipping any already present; the parameters used report the effective prompts. Returns job ID, profile, prompt ID, parameters used, output path/SHA-256/size/dimensions, elapsed ms, and manifest path.",
        annotations(read_only_hint = false, open_world_hint = false)
    )]
    async fn atelier_txt2img(
        &self,
        Parameters(request): Parameters<Txt2ImgRequest>,
    ) -> Result<String, String> {
        let lane = self.generation_lane()?;
        lane.core()
            .txt2img(request)
            .await
            .map(|value| generation::to_json(&value))
            .map_err(|error| lane.refusal(&error))
    }

    #[tool(
        description = "Generate one image with a registered image model (`model` id; omitted = the configured img2img default) from one PNG source under the active profile's allowed source roots, at the requested denoise, through the pinned img2img template. The same default anime style steering as atelier_txt2img is applied to the prompts. Returns the txt2img fields plus the source SHA-256.",
        annotations(read_only_hint = false, open_world_hint = false)
    )]
    async fn atelier_img2img(
        &self,
        Parameters(request): Parameters<Img2ImgRequest>,
    ) -> Result<String, String> {
        let lane = self.generation_lane()?;
        lane.core()
            .img2img(request)
            .await
            .map(|value| generation::to_json(&value))
            .map_err(|error| lane.refusal(&error))
    }

    // Style ops are exposed on BOTH surfaces by decision (Matt, 2026-10-18):
    // this stdio MCP registry and the A2A registry wrap the same library
    // functions (lumen::style), so the two registrations cannot drift.
    #[tool(
        description = "Apply a named style grade preset to every layer of a project .lumen.json document."
    )]
    async fn style_apply(&self, Parameters(req): Parameters<StyleApplyReq>) -> String {
        let root = match project_root() {
            Ok(root) => root,
            Err(e) => return err_json(e),
        };
        match style_apply(&root, req).await {
            Ok(saved) => ok_json(&saved),
            Err(e) => err_json(e),
        }
    }

    #[tool(
        description = "List the style grade presets (name + description) that style_apply accepts."
    )]
    async fn style_list(&self, Parameters(_req): Parameters<StyleListReq>) -> String {
        let root = match project_root() {
            Ok(root) => root,
            Err(e) => return err_json(e),
        };
        match style_list(&root, StyleListReq {}).await {
            Ok(list) => ok_json(&list),
            Err(e) => err_json(e),
        }
    }

    // VRM ops follow the D2 pattern: registered on both surfaces (this
    // stdio registry and the A2A registry), same library functions.
    #[tool(
        description = "Grade a VRM avatar's embedded textures with a named style preset; writes the repackaged .vrm."
    )]
    async fn vrm_grade(&self, Parameters(req): Parameters<VrmGradeReq>) -> String {
        let root = match project_root() {
            Ok(root) => root,
            Err(e) => return err_json(e),
        };
        match vrm_grade(&root, req).await {
            Ok(graded) => ok_json(&graded),
            Err(e) => err_json(e),
        }
    }

    #[tool(
        description = "Inspect a VRM avatar file: flavor (VRM 1.0 or 0.x), meta name and author, embedded textures."
    )]
    async fn vrm_inspect(&self, Parameters(req): Parameters<VrmInspectReq>) -> String {
        let root = match project_root() {
            Ok(root) => root,
            Err(e) => return err_json(e),
        };
        match vrm_inspect(&root, req).await {
            Ok(info) => ok_json(&info),
            Err(e) => err_json(e),
        }
    }
}

#[tool_handler]
impl ServerHandler for Lumen {
    /// Policy enforcement point: every tools/call is admitted (or refused
    /// with a structured denial) before the routed tool runs.
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        if let Err(denial) = self
            .enforcer
            .admit(&request.name, request.arguments.as_ref())
        {
            tracing::warn!(rule = %denial.rule, tool = %denial.tool, "policy denied tools/call");
            return Ok(CallToolResult::error(denial.to_json().into_contents()).into());
        }
        let tcc = ToolCallContext::new(self, request, context);
        self.tool_router.call(tcc).await
    }

    /// Modern era only: `server/discover` advertises exactly this
    /// one version and the SDK validates every request's `_meta`
    /// against it.
    fn supported_protocol_versions(&self) -> Cow<'static, [ProtocolVersion]> {
        Cow::Borrowed(&[ProtocolVersion::V_2026_07_28])
    }

    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(
                Implementation::new("lumen", env!("CARGO_PKG_VERSION")).with_title("Lumen"),
            )
            .with_instructions("lumen — Lux in motu. Cross-media conversion engine: deterministic, code-first bridging between 2D art, 3D models and engine formats. Tools: inspect PNGs and validate atlases, list and apply style grade presets, inspect and grade VRM avatars, discover Bevy Remote Protocol endpoints, and drive the character factory (records in, 2D and 3D out). Generation (atelier folded in, D-143): atelier_status, atelier_models, atelier_txt2img, atelier_img2img — image generation under the configured lane profile.")
    }
}

/// The active policy: the file named by `LUMEN_POLICY` when set, else the
/// compiled-in `lumen-policy.toml`. A named file that fails to load is an
/// error, never a silent fallback to the default.
fn load_policy() -> Result<(String, Policy), PolicyError> {
    match std::env::var("LUMEN_POLICY") {
        Ok(path) if !path.is_empty() => {
            let policy = Policy::load_file(std::path::Path::new(&path))?;
            Ok((path, policy))
        }
        _ => Ok((
            "built-in lumen-policy.toml".to_string(),
            Policy::from_toml_str(DEFAULT_POLICY_TOML)?,
        )),
    }
}

fn registered_tools() -> Vec<String> {
    Lumen::tool_router()
        .list_all()
        .into_iter()
        .map(|t| t.name.to_string())
        .collect()
}

/// `lumen policy check`: exit 0 iff the policy loads and every tool is labeled.
fn policy_check() -> i32 {
    let (source, policy) = match load_policy() {
        Ok(loaded) => loaded,
        Err(e) => {
            println!("FAIL {e}");
            return 1;
        }
    };
    let registered = registered_tools();
    let names: Vec<&str> = registered.iter().map(String::as_str).collect();
    let report = policy::check(&policy, &names);
    println!("policy: {source}");
    println!("registered tools: {} ({})", names.len(), names.join(", "));
    println!("library tools: {}", policy::LIBRARY_TOOLS.len());
    println!(
        "required tools: {}, labeled tools: {}",
        report.required_tools, report.labeled_tools
    );
    println!(
        "deny rules: {}, allow rules: {}",
        report.deny_rules, report.allow_rules
    );
    for problem in &report.problems {
        println!("FAIL {problem}");
    }
    if report.problems.is_empty() {
        println!("PASS every required tool is labeled");
        0
    } else {
        println!("FAIL {} problem(s)", report.problems.len());
        1
    }
}

/// `lumen policy replay`: exit 0 iff every scenario decides as expected.
fn policy_replay() -> i32 {
    let (source, policy) = match load_policy() {
        Ok(loaded) => loaded,
        Err(e) => {
            println!("FAIL {e}");
            return 1;
        }
    };
    println!("policy: {source}");
    let outcomes = policy::replay(&policy, policy::SCENARIOS);
    let mut failed = 0usize;
    for outcome in &outcomes {
        if outcome.passed {
            println!("PASS {}", outcome.id);
        } else {
            failed += 1;
            println!("FAIL {}: {}", outcome.id, outcome.detail);
        }
    }
    let count = |prefix: &str| outcomes.iter().filter(|o| o.id.starts_with(prefix)).count();
    println!(
        "replay: {} scenarios, {} passed, {failed} failed (ok: {}, adv: {})",
        outcomes.len(),
        outcomes.len() - failed,
        count("ok_"),
        count("adv_")
    );
    i32::from(failed != 0)
}

/// Run a CLI subcommand; returns the process exit code.
fn run_cli(command: Command) -> i32 {
    if let Command::Policy { action } = command {
        let code = match action {
            PolicyAction::Check => policy_check(),
            PolicyAction::Replay => policy_replay(),
        };
        std::process::exit(code);
    }
    let result: Result<(), lumen::LumenError> = match command {
        Command::Policy { .. } => unreachable!("handled above"),
        Command::Split {
            input,
            frames,
            out_dir,
            width,
            height,
        } => cli::run_split(&input, frames, &out_dir, width, height).map(|written| {
            println!("split: {} frames -> {}", written.len(), out_dir.display());
        }),
        Command::Pack { dir, out, bin_size } => cli::run_pack(&dir, &out, bin_size).map(|report| {
            println!(
                "pack: {} frames -> {} (+ {}) [{}px bin]",
                report.frames.len(),
                report.sheet.display(),
                report.sidecar.display(),
                report.bin_size
            );
        }),
        Command::Clean {
            dir,
            out_dir,
            connected_components,
            magenta,
        } => cli::run_clean(&dir, &out_dir, connected_components, magenta).map(|written| {
            println!("clean: {} files -> {}", written.len(), out_dir.display());
        }),
        Command::Watch { dir } => cli::run_watch(&dir),
        Command::Upscale { dir, out, model } => lumen::upscale::run_upscale(&dir, &out, &model)
            .map(|written| {
                println!("upscale: {} files -> {}", written.len(), out.display());
            }),
        Command::SplitTags {
            input,
            out,
            format,
            prefix,
            force,
        } => cli::run_split_tags(&input, &out, &format, &prefix, force).map(|reports| {
            for r in &reports {
                println!(
                    "split-tags: {} -> {} ({} frames, {:?})",
                    r.name,
                    r.output.display(),
                    r.frames.len(),
                    r.direction
                );
            }
        }),
    };
    match result {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("lumen: error: {e}");
            1
        }
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();
    if let Some(command) = cli.command {
        std::process::exit(run_cli(command));
    }
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();
    // Fail closed at startup: refuse to serve under a policy that does not
    // load or leaves a tool unlabeled.
    let (source, policy) = load_policy()?;
    let registered = registered_tools();
    let names: Vec<&str> = registered.iter().map(String::as_str).collect();
    let report = policy::check(&policy, &names);
    if !report.problems.is_empty() {
        return Err(format!(
            "policy {source} failed check: {}",
            report.problems.join("; ")
        )
        .into());
    }
    // The generation lane fails closed the same way: a named or
    // present atelier config that does not validate refuses startup.
    // Only an absent default config leaves the lane unconfigured.
    let generation = match generation::setup()? {
        GenerationSetup::Ready(lane) => {
            tracing::info!(profile = lane.profile(), "generation lane configured");
            GenerationState::Ready(Arc::new(lane))
        }
        GenerationSetup::Unconfigured { expected_path } => {
            tracing::warn!(
                path = %expected_path.display(),
                "no atelier config; generation tools will refuse with config_missing"
            );
            GenerationState::Unconfigured { expected_path }
        }
    };
    let handler = Lumen::new(Arc::new(Enforcer::new(policy)), generation);
    serve_modern(handler).await
}

/// Duplex buffer between the era guard and the SDK service. The
/// SDK consumes incrementally, so this only needs to cover
/// request/response bursts, not whole payloads.
const DUPLEX_BUFFER_BYTES: usize = 1024 * 1024;

/// Serve until stdin EOF: the era guard owns stdio, the SDK
/// service sits behind an in-process duplex, and a pump copies
/// its responses back to stdout through the same lock the guard
/// writes with. The `RunningService` is kept alive on
/// `waiting()` for the whole session — dropping it would close
/// the server after the first request.
async fn serve_modern(handler: Lumen) -> Result<(), Box<dyn std::error::Error>> {
    let (client_side, server_side) = tokio::io::duplex(DUPLEX_BUFFER_BYTES);
    let (server_read, server_write) = tokio::io::split(server_side);
    let (client_read, client_write) = tokio::io::split(client_side);
    let stdout: guard::SharedStdout = Arc::new(tokio::sync::Mutex::new(tokio::io::stdout()));

    let ordering = guard::Ordering::new();
    let serve = tokio::spawn(async move {
        let transport = AsyncRwTransport::new_server(server_read, server_write);
        match handler.serve(transport).await {
            Ok(running) => running
                .waiting()
                .await
                .map(|_| ())
                .map_err(|e| std::io::Error::other(e.to_string())),
            // The client closed the connection before selecting a
            // lifecycle (a probe, a spawn check, a client that only
            // ever wanted the refusal): bare EOF is a clean end of
            // session, as it was when the SDK owned stdio directly.
            Err(rmcp::service::ServerInitializeError::ConnectionClosed(_)) => Ok(()),
            Err(e) => Err(std::io::Error::other(e.to_string())),
        }
    });
    let mut pump = tokio::spawn(guard::pump(
        client_read,
        Arc::clone(&stdout),
        Arc::clone(&ordering),
    ));

    // When stdin reaches EOF the guard shuts the duplex write side
    // down, which ends the SDK service; the pump ends when the
    // service's transport drops. The pump wait is bounded as a
    // backstop: a response still in flight when the service ends
    // gets its chance to drain, but a wedged pump cannot hang exit.
    let guard_result = guard::run(tokio::io::stdin(), client_write, stdout, ordering).await;
    let serve_result = match serve.await {
        Ok(result) => result,
        Err(e) => Err(std::io::Error::other(e.to_string())),
    };
    if tokio::time::timeout(std::time::Duration::from_secs(2), &mut pump)
        .await
        .is_err()
    {
        pump.abort();
    }
    guard_result.and(serve_result)?;
    Ok(())
}
