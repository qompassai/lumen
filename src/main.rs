//! lumen — "Lux in motu" (light in motion).
//!
//! MCP stdio server for sprite artistry (default, no subcommand), plus a
//! pipeline CLI: `split`, `pack`, `clean`, `watch`, and `policy` tools.
//! Tool logic lives in the `lumen` library crate; this binary is transport
//! and argument wiring only.

#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::sync::Arc;

use clap::{Parser, Subcommand};
use lumen::cli::{self, CLI_DEFAULT_HEIGHT, CLI_DEFAULT_WIDTH};
use lumen::policy::{self, DEFAULT_POLICY_TOML, Enforcer, Policy, PolicyError};
use lumen::{bevy_discover, project_root, sprite_info, validate_atlas};
use rmcp::{
    ErrorData, RoleServer, ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, tool::ToolCallContext, wrapper::Parameters},
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, IntoContents, ServerCapabilities,
        ServerConfig,
    },
    schemars,
    service::RequestContext,
    tool, tool_handler, tool_router,
    transport::io::stdio,
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

#[derive(Debug, Clone)]
pub struct Lumen {
    tool_router: ToolRouter<Self>,
    /// Policy check run before every tools/call; shared by clones so the
    /// session's label history is one per server process.
    enforcer: Arc<Enforcer>,
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

#[tool_router]
impl Lumen {
    pub fn new(enforcer: Arc<Enforcer>) -> Self {
        Self {
            tool_router: Self::tool_router(),
            enforcer,
        }
    }

    #[tool(description = "Report width, height, color type and alpha presence of a project .png sprite.")]
    fn sprite_info(&self, Parameters(req): Parameters<SpriteInfoRequest>) -> String {
        match project_root().and_then(|root| sprite_info(&root, &req.path)) {
            Ok(info) => ok_json(&info),
            Err(e) => err_json(e),
        }
    }

    #[tool(description = "Validate a .png against the Light Show atlas contract: 384x1152, 4x6 grid of 96x192 cells, RGBA.")]
    fn validate_atlas(&self, Parameters(req): Parameters<ValidateAtlasRequest>) -> String {
        match project_root().and_then(|root| validate_atlas(&root, &req.path)) {
            Ok(info) => ok_json(&info),
            Err(e) => err_json(e),
        }
    }

    #[tool(description = "Probe a Bevy Remote Protocol endpoint and classify it: BRP-confirmed, JSON-RPC-speaking, unreachable, or not-BRP.")]
    async fn bevy_discover(&self, Parameters(req): Parameters<BevyDiscoverRequest>) -> String {
        match bevy_discover(req.endpoint.as_deref()).await {
            Ok(d) => ok_json(&d),
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
        if let Err(denial) = self.enforcer.admit(&request.name, request.arguments.as_ref()) {
            tracing::warn!(rule = %denial.rule, tool = %denial.tool, "policy denied tools/call");
            return Ok(CallToolResult::error(denial.to_json().into_contents()).into());
        }
        let tcc = ToolCallContext::new(self, request, context);
        self.tool_router.call(tcc).await
    }

    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions("lumen — Lux in motu. Sprite artistry tools: inspect PNGs, validate Light Show atlases, discover Bevy Remote Protocol endpoints.")
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
        _ => Ok(("built-in lumen-policy.toml".to_string(), Policy::from_toml_str(DEFAULT_POLICY_TOML)?)),
    }
}

fn registered_tools() -> Vec<String> {
    Lumen::tool_router().list_all().into_iter().map(|t| t.name.to_string()).collect()
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
    println!("required tools: {}, labeled tools: {}", report.required_tools, report.labeled_tools);
    println!("deny rules: {}, allow rules: {}", report.deny_rules, report.allow_rules);
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
        Command::Split { input, frames, out_dir, width, height } => {
            cli::run_split(&input, frames, &out_dir, width, height).map(|written| {
                println!("split: {} frames -> {}", written.len(), out_dir.display());
            })
        }
        Command::Pack { dir, out, bin_size } => {
            cli::run_pack(&dir, &out, bin_size).map(|report| {
                println!(
                    "pack: {} frames -> {} (+ {}) [{}px bin]",
                    report.frames.len(),
                    report.sheet.display(),
                    report.sidecar.display(),
                    report.bin_size
                );
            })
        }
        Command::Clean { dir, out_dir, connected_components, magenta } => {
            cli::run_clean(&dir, &out_dir, connected_components, magenta).map(|written| {
                println!("clean: {} files -> {}", written.len(), out_dir.display());
            })
        }
        Command::Watch { dir } => cli::run_watch(&dir),
        Command::Upscale { dir, out, model } => {
            lumen::upscale::run_upscale(&dir, &out, &model).map(|written| {
                println!("upscale: {} files -> {}", written.len(), out.display());
            })
        }
        Command::SplitTags { input, out, format, prefix, force } => {
            cli::run_split_tags(&input, &out, &format, &prefix, force).map(|reports| {
                for r in &reports {
                    println!(
                        "split-tags: {} -> {} ({} frames, {:?})",
                        r.name,
                        r.output.display(),
                        r.frames.len(),
                        r.direction
                    );
                }
            })
        }
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
        return Err(format!("policy {source} failed check: {}", report.problems.join("; ")).into());
    }
    let service = Lumen::new(Arc::new(Enforcer::new(policy))).serve(stdio()).await?;
    service.waiting().await?;
    Ok(())
}
