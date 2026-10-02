//! A2A agent layer: lumen as a dispatchable Agent2Agent v0.3 endpoint.
//!
//! MCP is lumen's hands; A2A is its dispatch surface. Both call the same
//! library operations: every A2A skill is a row in `registry::SKILLS` that
//! forwards to the function the MCP tool uses.
//!
//! Contract:
//! - Transport: JSON-RPC 2.0 over HTTP/1.1 on a LOOPBACK socket only.
//!   Methods: `message/send`, `message/stream` (SSE), `tasks/get`,
//!   `tasks/cancel`. Agent card at `/.well-known/agent-card.json`, generated
//!   from the registry at bind time.
//! - Off by default: nothing listens until the binary calls `bind` (see
//!   `A2aConfig::from_env`, which returns `None` unless `LUMEN_A2A_PORT`
//!   is set).
//! - Task payload: exactly one message part, either a `data` part or a `text`
//!   part holding a JSON object: `{"skill", "arguments"}` or
//!   `{"steps": [{"skill", "arguments"}, ...], "review_after": k}`. With
//!   `review_after`, the task pauses at `input-required` after step `k`;
//!   reply on the same `taskId` with `{"decision": "approve" | "reject",
//!   "reasons": [...]}`.
//! - Bounds: request body `REQUEST_BODY_BYTES_MAX`, `task::STEPS_MAX` steps,
//!   `task::LIVE_TASKS_MAX` live tasks, `task::RETAINED_TERMINAL_TASKS_MAX`
//!   retained, a per-run deadline, `http::CONNECTIONS_MAX` connections.
//! - Cancellation is cooperative: the cancel flag is read only between
//!   steps, so a running operation finishes its (bounded) step, keeps its
//!   artifact, and the task then moves to `canceled`.
//! - Terminal states carry artifacts (project-relative paths plus the
//!   operation's JSON result) or a structured error object, never a bare
//!   string.

mod http;
mod registry;
mod task;

use std::net::SocketAddr;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::Deserialize;
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tokio::sync::broadcast;

use crate::LumenError;

pub use registry::{Capability, SKILLS, SkillSpec, find_skill};
pub use task::TaskState;
use task::{Decision, Resumed, Step, Store, TaskEvent};

/// Largest accepted HTTP request body, in bytes.
pub const REQUEST_BODY_BYTES_MAX: usize = 256 * 1024;
/// Default per-run task deadline.
pub const TASK_DEADLINE_DEFAULT: Duration = Duration::from_secs(300);
/// Largest configurable per-run task deadline.
pub const TASK_DEADLINE_MAX: Duration = Duration::from_secs(3600);
/// How long `tasks/cancel` waits for a running step to reach its boundary.
pub const CANCEL_ACK_WAIT: Duration = Duration::from_secs(5);
/// Longest accepted id string (JSON-RPC id, task id, context id), in bytes.
const ID_BYTES_MAX: usize = 128;
/// Bounds on a review rejection's reasons.
const REASONS_MAX: usize = 16;
const REASON_CHARS_MAX: usize = 1024;
/// Most message parts inspected; more is rejected, not truncated.
const MESSAGE_PARTS_MAX: usize = 8;

/// Listener configuration. Construct with `loopback` or `from_env`.
#[derive(Debug, Clone)]
pub struct A2aConfig {
    /// Must be a loopback address; `bind` refuses anything else.
    pub bind: SocketAddr,
    /// Root every operation and artifact path is bounded to.
    pub project_root: PathBuf,
    /// Wall-clock budget for one uninterrupted run of a task's steps
    /// (time paused at `input-required` does not count).
    pub task_deadline: Duration,
}

impl A2aConfig {
    /// `127.0.0.1:<port>` with the default deadline. Port 0 picks a free port.
    pub fn loopback(port: u16, project_root: PathBuf) -> Self {
        Self {
            bind: SocketAddr::from(([127, 0, 0, 1], port)),
            project_root,
            task_deadline: TASK_DEADLINE_DEFAULT,
        }
    }

    /// `Some` only when `LUMEN_A2A_PORT` is set to a port in 1..=65535; unset
    /// or empty means the A2A listener stays off. Reads, never mutates, env.
    pub fn from_env(project_root: PathBuf) -> Result<Option<Self>, LumenError> {
        let raw = match std::env::var("LUMEN_A2A_PORT") {
            Ok(v) if !v.is_empty() => v,
            _ => return Ok(None),
        };
        match raw.parse::<u16>() {
            Ok(port) if port != 0 => Ok(Some(Self::loopback(port, project_root))),
            _ => Err(LumenError::BadParam("LUMEN_A2A_PORT must be 1..=65535".to_string())),
        }
    }
}

/// State shared by the listener, every connection, and every task runner.
pub(crate) struct Shared {
    pub(crate) root: PathBuf,
    pub(crate) task_deadline: Duration,
    pub(crate) store: Mutex<Store>,
    card: Value,
}

/// The protocol core, independent of the socket. Cheap to clone (`Arc`).
#[derive(Clone)]
pub struct Agent {
    shared: Arc<Shared>,
}

/// A bound, not-yet-serving listener.
pub struct A2aServer {
    listener: TcpListener,
    agent: Agent,
    local_addr: SocketAddr,
}

/// Validate the config and bind. Fails closed on non-loopback addresses,
/// an inaccessible root, or a deadline outside `1ms..=TASK_DEADLINE_MAX`.
pub async fn bind(config: A2aConfig) -> Result<A2aServer, LumenError> {
    if !config.bind.ip().is_loopback() {
        return Err(LumenError::BadParam(
            "the A2A listener binds loopback addresses only".to_string(),
        ));
    }
    if config.task_deadline.is_zero() || config.task_deadline > TASK_DEADLINE_MAX {
        return Err(LumenError::BadParam(format!(
            "task_deadline must be within 1ms..={}s",
            TASK_DEADLINE_MAX.as_secs()
        )));
    }
    let root = config.project_root.canonicalize().map_err(|e| {
        LumenError::PathRejected(format!("project root not accessible: {e}"))
    })?;
    let listener = TcpListener::bind(config.bind).await?;
    let local_addr = listener.local_addr()?;
    let shared = Shared {
        root,
        task_deadline: config.task_deadline,
        store: Mutex::new(Store::default()),
        card: build_card(&format!("http://{local_addr}/")),
    };
    Ok(A2aServer { listener, agent: Agent { shared: Arc::new(shared) }, local_addr })
}

impl A2aServer {
    /// The actual bound address (resolves port 0).
    pub fn local_addr(&self) -> SocketAddr {
        self.local_addr
    }

    /// A handle on the protocol core, for in-process callers and tests.
    pub fn agent(&self) -> Agent {
        self.agent.clone()
    }

    /// Accept connections until the listener fails. Never returns `Ok`.
    pub async fn serve(self) -> Result<(), LumenError> {
        http::serve(self.listener, self.agent).await
    }
}

/// Resolve an artifact path reported by an operation to its project-relative
/// `/`-separated form. Fails closed when the path does not exist, does not
/// canonicalize inside `root` (symlinks are followed first), is the root
/// itself, or is not UTF-8.
pub fn artifact_relative_path(root: &Path, path: &Path) -> Result<String, LumenError> {
    let root = root
        .canonicalize()
        .map_err(|e| LumenError::PathRejected(format!("project root not accessible: {e}")))?;
    let joined = if path.is_absolute() { path.to_path_buf() } else { root.join(path) };
    let canonical = joined
        .canonicalize()
        .map_err(|_| LumenError::PathRejected("artifact does not exist".to_string()))?;
    let rel = canonical
        .strip_prefix(&root)
        .map_err(|_| LumenError::PathRejected("artifact escapes the project root".to_string()))?;
    let mut parts = Vec::new();
    for component in rel.components() {
        let Component::Normal(name) = component else {
            return Err(LumenError::PathRejected("artifact path is not normal".to_string()));
        };
        let name = name
            .to_str()
            .ok_or_else(|| LumenError::PathRejected("artifact path is not utf-8".to_string()))?;
        parts.push(name);
    }
    if parts.is_empty() {
        return Err(LumenError::PathRejected("artifact is the project root".to_string()));
    }
    Ok(parts.join("/"))
}

// ---------------------------------------------------------------------------
// Agent card
// ---------------------------------------------------------------------------

fn build_card(url: &str) -> Value {
    let mut names: Vec<&str> = SKILLS.iter().map(|s| s.name).collect();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), SKILLS.len(), "skill names must be unique");
    let mut groups: Vec<Capability> = SKILLS.iter().map(|s| s.capability).collect();
    groups.sort_unstable();
    groups.dedup();
    let group_ids: Vec<&str> = groups.iter().map(|c| c.as_str()).collect();
    let skills: Vec<Value> = SKILLS
        .iter()
        .map(|s| {
            json!({
                "id": s.name,
                "name": s.name,
                "description": s.description,
                "tags": [s.capability.as_str()],
                "inputModes": ["application/json", "text/plain"],
                "outputModes": ["application/json"],
            })
        })
        .collect();
    json!({
        "protocolVersion": "0.3.0",
        "name": "lumen",
        "description": format!(
            "Lux in motu: sprite artistry agent. Capabilities: {}. Send one data part \
             (or a text part holding JSON): {{\"skill\", \"arguments\"}} or \
             {{\"steps\": [...], \"review_after\": k}}.",
            group_ids.join(", ")
        ),
        "url": url,
        "preferredTransport": "JSONRPC",
        "supportedInterfaces": [{"url": url, "protocolBinding": "JSONRPC"}],
        "version": env!("CARGO_PKG_VERSION"),
        "capabilities": {
            "streaming": true,
            "pushNotifications": false,
            "stateTransitionHistory": false,
        },
        "defaultInputModes": ["application/json", "text/plain"],
        "defaultOutputModes": ["application/json"],
        "skills": skills,
    })
}

// ---------------------------------------------------------------------------
// JSON-RPC envelope
// ---------------------------------------------------------------------------

/// JSON-RPC and A2A error codes used on the wire.
pub mod codes {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL_ERROR: i64 = -32603;
    pub const TASK_NOT_FOUND: i64 = -32001;
    pub const TASK_NOT_CANCELABLE: i64 = -32002;
    pub const PUSH_NOT_SUPPORTED: i64 = -32003;
    pub const UNSUPPORTED_OPERATION: i64 = -32004;
    /// Implementation-defined (JSON-RPC server-error range): live-task bound.
    pub const SERVER_BUSY: i64 = -32050;
}

#[derive(Debug)]
pub(crate) struct RpcError {
    code: i64,
    message: String,
}

fn rpc_err(code: i64, message: impl Into<String>) -> RpcError {
    RpcError { code, message: message.into() }
}

fn invalid_params(message: impl Into<String>) -> RpcError {
    rpc_err(codes::INVALID_PARAMS, message)
}

pub(crate) fn error_envelope(id: &Value, err: &RpcError) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": err.code, "message": err.message}})
}

pub(crate) fn result_envelope(id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

/// What a dispatched request produced.
pub(crate) enum Reply {
    Json(Value),
    /// SSE: `first` is the task snapshot; then events until one is final.
    Stream { id: Value, first: Value, events: broadcast::Receiver<TaskEvent> },
}

struct RpcRequest {
    id: Value,
    method: String,
    params: Value,
}

fn parse_request(body: &[u8]) -> Result<RpcRequest, (Value, RpcError)> {
    let value: Value = serde_json::from_slice(body)
        .map_err(|_| (Value::Null, rpc_err(codes::PARSE_ERROR, "body is not valid JSON")))?;
    let Value::Object(mut obj) = value else {
        return Err((Value::Null, rpc_err(codes::INVALID_REQUEST, "batch/non-object request")));
    };
    let id = match obj.remove("id") {
        Some(id @ Value::Number(_)) => id,
        Some(Value::String(s)) if s.len() <= ID_BYTES_MAX => Value::String(s),
        _ => {
            let err = rpc_err(codes::INVALID_REQUEST, "id must be a number or short string");
            return Err((Value::Null, err));
        }
    };
    if obj.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err((id, rpc_err(codes::INVALID_REQUEST, "jsonrpc must be \"2.0\"")));
    }
    let Some(Value::String(method)) = obj.remove("method") else {
        return Err((id, rpc_err(codes::INVALID_REQUEST, "method must be a string")));
    };
    let params = obj.remove("params").unwrap_or(Value::Null);
    Ok(RpcRequest { id, method, params })
}

// ---------------------------------------------------------------------------
// Message payloads
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StepPayload {
    skill: String,
    #[serde(default)]
    arguments: Option<Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TaskPayload {
    skill: Option<String>,
    arguments: Option<Value>,
    steps: Option<Vec<StepPayload>>,
    review_after: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DecisionPayload {
    decision: String,
    #[serde(default)]
    reasons: Vec<String>,
}

enum Incoming {
    New { context_id: Option<String>, steps: Vec<Step>, review_after: Option<usize> },
    Continue { task_id: String, decision: Decision },
}

fn short_id(value: Option<&Value>, field: &str) -> Result<Option<String>, RpcError> {
    match value {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s))
            if !s.is_empty() && s.len() <= ID_BYTES_MAX && !s.chars().any(char::is_control) =>
        {
            Ok(Some(s.clone()))
        }
        Some(_) => Err(invalid_params(format!("{field} must be a short printable string"))),
    }
}

/// Exactly one part carries the payload: one `data` part, or (no data parts
/// and) one `text` part holding a JSON object. `file` parts are refused.
fn extract_payload(message: &Value) -> Result<Value, RpcError> {
    let parts = message
        .get("parts")
        .and_then(Value::as_array)
        .filter(|p| !p.is_empty() && p.len() <= MESSAGE_PARTS_MAX)
        .ok_or_else(|| invalid_params(format!("parts must hold 1..={MESSAGE_PARTS_MAX}")))?;
    if parts.len() != 1 {
        return Err(invalid_params("send exactly one data or text part"));
    }
    let part = &parts[0];
    match part.get("kind").and_then(Value::as_str) {
        Some("data") => match part.get("data") {
            Some(obj @ Value::Object(_)) => Ok(obj.clone()),
            _ => Err(invalid_params("data part must carry a JSON object")),
        },
        Some("text") => {
            let text = part.get("text").and_then(Value::as_str).unwrap_or_default();
            match serde_json::from_str::<Value>(text) {
                Ok(obj @ Value::Object(_)) => Ok(obj),
                _ => Err(invalid_params("text part must hold a JSON object task payload")),
            }
        }
        _ => Err(invalid_params("part kind must be \"data\" or \"text\"")),
    }
}

fn parse_steps(payload: Value) -> Result<(Vec<Step>, Option<usize>), RpcError> {
    let p: TaskPayload = serde_json::from_value(payload)
        .map_err(|e| invalid_params(format!("task payload: {e}")))?;
    let raw = match (p.skill, p.steps) {
        (Some(skill), None) => vec![StepPayload { skill, arguments: p.arguments }],
        (None, Some(steps)) if p.arguments.is_none() => steps,
        _ => return Err(invalid_params("give either skill+arguments or steps, not both")),
    };
    if raw.is_empty() || raw.len() > task::STEPS_MAX {
        return Err(invalid_params(format!("steps must hold 1..={}", task::STEPS_MAX)));
    }
    if p.review_after.is_some_and(|k| k >= raw.len()) {
        return Err(invalid_params("review_after must index an existing step"));
    }
    let mut steps = Vec::with_capacity(raw.len());
    for (index, s) in raw.into_iter().enumerate() {
        let skill = find_skill(&s.skill)
            .ok_or_else(|| invalid_params(format!("step {index}: unknown skill")))?;
        let arguments = match s.arguments {
            None => Value::Object(serde_json::Map::new()),
            Some(obj @ Value::Object(_)) => obj,
            Some(_) => return Err(invalid_params(format!("step {index}: arguments not object"))),
        };
        steps.push(Step { skill, arguments });
    }
    Ok((steps, p.review_after))
}

fn parse_decision(payload: Value) -> Result<Decision, RpcError> {
    let d: DecisionPayload = serde_json::from_value(payload)
        .map_err(|e| invalid_params(format!("decision payload: {e}")))?;
    let reasons_ok = d.reasons.len() <= REASONS_MAX
        && d.reasons.iter().all(|r| r.chars().count() <= REASON_CHARS_MAX);
    if !reasons_ok {
        return Err(invalid_params(format!("reasons: at most {REASONS_MAX} short strings")));
    }
    match d.decision.as_str() {
        "approve" if d.reasons.is_empty() => Ok(Decision::Approve),
        "reject" => Ok(Decision::Reject(d.reasons)),
        _ => Err(invalid_params("decision must be \"approve\" (no reasons) or \"reject\"")),
    }
}

/// Returns the incoming request plus the `blocking` flag (default `true`:
/// `message/send` answers once the task settles, which is what diver's
/// client expects).
fn parse_message(params: &Value) -> Result<(Incoming, bool), RpcError> {
    let message = params
        .get("message")
        .filter(|m| m.is_object())
        .ok_or_else(|| invalid_params("params.message must be an object"))?;
    if message.get("kind").is_some_and(|k| k != "message") {
        return Err(invalid_params("message.kind must be \"message\""));
    }
    if message.get("role").and_then(Value::as_str) != Some("user") {
        return Err(invalid_params("message.role must be \"user\""));
    }
    let blocking = params
        .pointer("/configuration/blocking")
        .and_then(Value::as_bool)
        .unwrap_or(true);
    let payload = extract_payload(message)?;
    let incoming = match short_id(message.get("taskId"), "message.taskId")? {
        Some(task_id) => Incoming::Continue { task_id, decision: parse_decision(payload)? },
        None => {
            let context_id = short_id(message.get("contextId"), "message.contextId")?;
            let (steps, review_after) = parse_steps(payload)?;
            Incoming::New { context_id, steps, review_after }
        }
    };
    Ok((incoming, blocking))
}

fn task_id_param(params: &Value) -> Result<String, RpcError> {
    short_id(params.get("id"), "params.id")?.ok_or_else(|| invalid_params("params.id required"))
}

// ---------------------------------------------------------------------------
// Methods
// ---------------------------------------------------------------------------

fn not_found(id: &str) -> RpcError {
    rpc_err(codes::TASK_NOT_FOUND, format!("task not found: {id}"))
}

fn poisoned() -> RpcError {
    rpc_err(codes::INTERNAL_ERROR, "task store unavailable")
}

/// Wait until a final event arrives, the channel closes, or `limit` passes.
async fn wait_settled(events: &mut broadcast::Receiver<TaskEvent>, limit: Duration) {
    let deadline = tokio::time::Instant::now() + limit;
    // Each iteration consumes one event; a run emits a bounded number.
    for _ in 0..task::EVENTS_PER_RUN_MAX {
        match tokio::time::timeout_at(deadline, events.recv()).await {
            Ok(Ok(event)) if event.is_final => return,
            Ok(Ok(_)) | Ok(Err(broadcast::error::RecvError::Lagged(_))) => {}
            Ok(Err(broadcast::error::RecvError::Closed)) | Err(_) => return,
        }
    }
}

impl Agent {
    /// The generated agent card.
    pub fn card(&self) -> &Value {
        &self.shared.card
    }

    /// Handle one JSON-RPC request body and return the response envelope.
    /// `message/stream` needs the HTTP transport and is refused here.
    pub async fn handle_rpc(&self, body: &[u8]) -> Value {
        match self.dispatch(body, false).await {
            Reply::Json(v) => v,
            Reply::Stream { id, .. } => {
                error_envelope(&id, &rpc_err(codes::INTERNAL_ERROR, "stream not allowed"))
            }
        }
    }

    pub(crate) async fn dispatch(&self, body: &[u8], allow_stream: bool) -> Reply {
        let req = match parse_request(body) {
            Ok(req) => req,
            Err((id, err)) => return Reply::Json(error_envelope(&id, &err)),
        };
        let id = req.id;
        let outcome = match req.method.as_str() {
            "message/send" => self.message(&req.params, false).await,
            "message/stream" if allow_stream => self.message(&req.params, true).await,
            "message/stream" => Err(rpc_err(codes::UNSUPPORTED_OPERATION, "use HTTP for SSE")),
            "tasks/get" => self.tasks_get(&req.params).map(Reply::Json),
            "tasks/cancel" => self.tasks_cancel(&req.params).await.map(Reply::Json),
            m if m.starts_with("tasks/pushNotificationConfig/") => {
                Err(rpc_err(codes::PUSH_NOT_SUPPORTED, "push notifications are not supported"))
            }
            "tasks/resubscribe" | "agent/getAuthenticatedExtendedCard" => {
                Err(rpc_err(codes::UNSUPPORTED_OPERATION, "method not supported"))
            }
            _ => Err(rpc_err(codes::METHOD_NOT_FOUND, "method not found")),
        };
        match outcome {
            Ok(Reply::Json(result)) => Reply::Json(result_envelope(&id, result)),
            Ok(Reply::Stream { first, events, .. }) => Reply::Stream { id, first, events },
            Err(err) => Reply::Json(error_envelope(&id, &err)),
        }
    }

    fn snapshot(&self, task_id: &str) -> Result<Value, RpcError> {
        let store = self.shared.store.lock().map_err(|_| poisoned())?;
        store.get(task_id).map(|t| t.to_wire()).ok_or_else(|| not_found(task_id))
    }

    fn spawn_runner(&self, task_id: &str) {
        let Some(seq) = task::parse_task_id(task_id) else { return };
        tokio::spawn(task::run_task(Arc::clone(&self.shared), seq));
    }

    /// Validate fully, then create or resume, subscribing BEFORE the runner
    /// starts so no event can be missed.
    async fn message(&self, params: &Value, stream: bool) -> Result<Reply, RpcError> {
        let (incoming, blocking) = parse_message(params)?;
        let (task_id, mut events, run) = {
            let mut store = self.shared.store.lock().map_err(|_| poisoned())?;
            match incoming {
                Incoming::New { context_id, steps, review_after } => {
                    let seq = store.create(context_id, steps, review_after).ok_or_else(|| {
                        rpc_err(codes::SERVER_BUSY, "live task limit reached; retry later")
                    })?;
                    let id = format!("lumen-task-{seq}");
                    let entry = store.get(&id).ok_or_else(poisoned)?;
                    (id, entry.events.subscribe(), true)
                }
                Incoming::Continue { task_id, decision } => {
                    let entry = store.get_mut(&task_id).ok_or_else(|| not_found(&task_id))?;
                    if entry.state != TaskState::InputRequired {
                        let state = entry.state.as_str();
                        return Err(invalid_params(format!("task is {state}, not input-required")));
                    }
                    let events = entry.events.subscribe();
                    let run = matches!(task::apply_decision(entry, decision), Resumed::Run);
                    (task_id, events, run)
                }
            }
        };
        if run {
            self.spawn_runner(&task_id);
        }
        if stream {
            let first = self.snapshot(&task_id)?;
            return Ok(Reply::Stream { id: Value::Null, first, events });
        }
        if blocking {
            wait_settled(&mut events, self.shared.task_deadline + CANCEL_ACK_WAIT).await;
        }
        self.snapshot(&task_id).map(Reply::Json)
    }

    fn tasks_get(&self, params: &Value) -> Result<Value, RpcError> {
        let task_id = task_id_param(params)?;
        let mut store = self.shared.store.lock().map_err(|_| poisoned())?;
        store.sweep(std::time::Instant::now());
        store.get(&task_id).map(|t| t.to_wire()).ok_or_else(|| not_found(&task_id))
    }

    async fn tasks_cancel(&self, params: &Value) -> Result<Value, RpcError> {
        let task_id = task_id_param(params)?;
        let mut events = {
            let mut store = self.shared.store.lock().map_err(|_| poisoned())?;
            let entry = store.get_mut(&task_id).ok_or_else(|| not_found(&task_id))?;
            if entry.state.is_terminal() {
                let state = entry.state.as_str();
                return Err(rpc_err(codes::TASK_NOT_CANCELABLE, format!("task is {state}")));
            }
            if entry.state == TaskState::InputRequired {
                task::cancel_paused(entry);
                return Ok(entry.to_wire());
            }
            entry.cancel.store(true, Ordering::Release);
            entry.events.subscribe()
        };
        // The running step finishes first; report whatever state it reached.
        wait_settled(&mut events, CANCEL_ACK_WAIT).await;
        self.snapshot(&task_id)
    }
}
