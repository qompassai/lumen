//! lumen — "Lux in motu" (light in motion).
//!
//! MCP stdio server for sprite artistry. Tool logic lives in the `lumen`
//! library crate; this binary is transport wiring only.

#![forbid(unsafe_code)]

use std::sync::Arc;

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
            Ok(report) => ok_json(&report),
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

/// `Some(exit code)` when argv selects a policy subcommand, `None` to serve.
fn policy_cli() -> Option<i32> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().map(String::as_str) != Some("policy") {
        return None;
    }
    Some(match args.get(1).map(String::as_str) {
        Some("check") if args.len() == 2 => policy_check(),
        Some("replay") if args.len() == 2 => policy_replay(),
        _ => {
            eprintln!("usage: lumen policy <check|replay>  (policy file: $LUMEN_POLICY or built-in)");
            2
        }
    })
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    if let Some(code) = policy_cli() {
        std::process::exit(code);
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
