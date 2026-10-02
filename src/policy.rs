//! Deterministic policy layer: a sensitivity x trust lattice checked before
//! every `tools/call` (POLICY_DESIGN.md, `lumen-policy.toml`).
//!
//! Contract summary:
//! - Two pure stages. `Session::label` turns (tool, JSON arguments, session
//!   history) into a `CallEvent` (labels + sink); `decide` turns
//!   (policy, event) into `Decision`. Neither does network or file I/O, reads
//!   the environment, or depends on hash-map order, so a replay of the same
//!   calls yields the same decisions.
//! - Labels are server-derived only. Tool baselines come from the policy;
//!   argument strings are labeled lexically against `[[sources]]` path
//!   prefixes. Nothing a client puts in the arguments can lower a label.
//! - Session context only grows: every admitted call adds its labels.
//!   Denied calls add nothing (they never execute).
//! - Deny-by-default: a call is allowed only when no deny rule matches and
//!   every label in context is covered by an allow rule for the sink.
//!   Unlabeled tools, unresolvable sinks, and oversized arguments are denied.
//! - Bounds: policy file <= `POLICY_BYTES_MAX`, rule/tool/source counts
//!   capped, argument walk <= `ARGS_DEPTH_MAX` deep, `ARGS_NODES_MAX` nodes
//!   and `ARGS_STRING_BYTES_MAX` string bytes.
//! - Known gap: path labeling is lexical, so a symlink inside the project
//!   that points into `refs/proprietary` is labeled by its own name.

#![forbid(unsafe_code)]

use std::collections::{BTreeMap, BTreeSet};
use std::net::{IpAddr, Ipv6Addr};
use std::path::Path;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use thiserror::Error;

use crate::DEFAULT_BRP_ENDPOINT;

/// The shipped default policy, compiled in from the crate root.
pub const DEFAULT_POLICY_TOML: &str = include_str!("../lumen-policy.toml");
/// The only accepted `version` value.
pub const POLICY_VERSION: u32 = 1;
/// Largest accepted policy file, in bytes.
pub const POLICY_BYTES_MAX: u64 = 64 * 1024;
/// Most deny or allow rules accepted (each list separately).
pub const POLICY_RULES_MAX: usize = 128;
/// Most labeled tools accepted.
pub const POLICY_TOOLS_MAX: usize = 256;
/// Most `[[sources]]` entries accepted.
pub const POLICY_SOURCES_MAX: usize = 64;
/// Longest accepted rule id, in characters.
pub const RULE_ID_CHARS_MAX: usize = 64;
/// Deepest accepted nesting of tool arguments (top-level values are depth 1).
pub const ARGS_DEPTH_MAX: usize = 8;
/// Most JSON nodes (values and object keys) visited in one call's arguments.
pub const ARGS_NODES_MAX: usize = 65_536;
/// Most string bytes (values and keys) labeled in one call's arguments.
pub const ARGS_STRING_BYTES_MAX: usize = 2 * 1024 * 1024;

/// Engine rule: the tool has no labels in the policy.
pub const RULE_UNLABELED_TOOL: &str = "R0-unlabeled-tool";
/// Engine rule: the sink could not be resolved from the arguments.
pub const RULE_SINK_UNRESOLVABLE: &str = "R0-sink-unresolvable";
/// Engine rule: the arguments exceed the labeling bounds.
pub const RULE_ARGS_UNBOUNDED: &str = "R0-args-unbounded";
/// Engine rule: the session state is unusable (a prior panic poisoned it).
pub const RULE_SESSION_UNAVAILABLE: &str = "R0-session-unavailable";
/// Engine rule: no allow rule covers this label for this sink.
pub const RULE_DENY_BY_DEFAULT: &str = "R4-deny-by-default";

/// Tools implemented in the library, labeled ahead of MCP registration.
/// `check` requires labels for these plus every registered tool, and
/// rejects labels or rule filters naming any other tool (typo guard).
pub const LIBRARY_TOOLS: &[&str] = &[
    "bevy_call",
    "bevy_status",
    "export_sprite",
    "export_sheet",
    "export_tag",
    "import_layer",
    "dream_ambient",
    "dream_act",
    "dream_transition",
    "dream_text",
    "text_rasterize",
    "text_measure",
    "palette_presets",
    "palette_apply",
    "palette_extract",
    "palette_ramp",
    "quantize",
    "new_sprite",
    "add_layer",
    "delete_layer",
    "reorder_layer",
    "rename_layer",
    "set_layer_props",
    "add_frame",
    "delete_frame",
    "set_frame_duration",
    "set_frame_mod",
    "add_tag",
    "delete_tag",
    "set_pixel",
    "fill_rect",
    "draw_line",
    "draw_circle",
    "flood_fill",
    "flip",
    "rotate",
    "resize_canvas",
    "crop",
    "tween_frames",
    "build_fullbody_sheet",
    "generate_mature_variant",
    "contact_sheet",
    "fidelity_set",
    "style_list",
    "style_apply",
    "inspect_sprite",
    "inspect_layer",
    "histogram",
    "color_usage",
    "validate_scene",
    "audit_animation",
    "compare_frames",
    "run_lua_script",
];

// ---------------------------------------------------------------------------
// Labels
// ---------------------------------------------------------------------------

/// Input sensitivity. Declaration order is severity order: denials name the
/// most severe offending label.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Sensitivity {
    Low,
    Untrusted,
    High,
    Restricted,
}

/// Destination trust.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Trust {
    Trusted,
    Public,
    Loopback,
    Remote,
    External,
}

/// How a tool's sink is determined.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SinkSpec {
    Trusted,
    Public,
    Loopback,
    Remote,
    External,
    /// Resolved per call from the `endpoint` argument: loopback or remote.
    Brp,
}

/// A tool's declared labels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolLabels {
    pub reads: BTreeSet<Sensitivity>,
    pub sink: SinkSpec,
}

/// One validated deny or allow rule. An empty `tools` set means every tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rule {
    pub id: String,
    pub reason: String,
    pub tools: BTreeSet<String>,
    pub labels: BTreeSet<Sensitivity>,
    pub sinks: BTreeSet<Trust>,
}

impl Rule {
    fn matches(&self, tool: &str, label: Sensitivity, sink: Trust) -> bool {
        (self.tools.is_empty() || self.tools.contains(tool))
            && self.labels.contains(&label)
            && self.sinks.contains(&sink)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Source {
    /// Lowercased path components.
    components: Vec<String>,
    label: Sensitivity,
}

/// A validated policy. Construct with `Policy::from_toml_str` or `load_file`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Policy {
    sources: Vec<Source>,
    tools: BTreeMap<String, ToolLabels>,
    deny: Vec<Rule>,
    allow: Vec<Rule>,
}

/// Why a policy failed to load.
#[derive(Debug, Error)]
pub enum PolicyError {
    #[error("policy io error: {0}")]
    Io(String),
    #[error("policy too large: {0} bytes; limit is {POLICY_BYTES_MAX}")]
    TooLarge(u64),
    #[error("policy parse error: {0}")]
    Parse(String),
    #[error("policy invalid: {0}")]
    Invalid(String),
}

// ---------------------------------------------------------------------------
// Loading and validation
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPolicy {
    version: u32,
    default: String,
    #[serde(default)]
    sources: Vec<RawSource>,
    tools: BTreeMap<String, RawTool>,
    #[serde(default)]
    deny: Vec<RawRule>,
    #[serde(default)]
    allow: Vec<RawRule>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSource {
    prefix: String,
    label: Sensitivity,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTool {
    reads: Vec<Sensitivity>,
    sink: SinkSpec,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawRule {
    id: String,
    reason: String,
    #[serde(default)]
    tools: Vec<String>,
    labels: Vec<Sensitivity>,
    sinks: Vec<Trust>,
}

fn invalid(msg: impl Into<String>) -> PolicyError {
    PolicyError::Invalid(msg.into())
}

impl Policy {
    /// Parse and validate policy TOML. Rejects unknown keys, any `default`
    /// other than `"deny"`, empty label/sink lists, duplicate or reserved
    /// (`R0-`/`R4-`) rule ids, and counts above the `POLICY_*_MAX` bounds.
    pub fn from_toml_str(text: &str) -> Result<Self, PolicyError> {
        let text_bytes = u64::try_from(text.len()).unwrap_or(u64::MAX);
        if text_bytes > POLICY_BYTES_MAX {
            return Err(PolicyError::TooLarge(text_bytes));
        }
        let raw: RawPolicy =
            toml::from_str(text).map_err(|e| PolicyError::Parse(e.message().to_string()))?;
        if raw.version != POLICY_VERSION {
            return Err(invalid(format!(
                "version {} unsupported; expected {POLICY_VERSION}",
                raw.version
            )));
        }
        if raw.default != "deny" {
            return Err(invalid("default must be \"deny\"; deny-by-default is not optional"));
        }
        if raw.sources.len() > POLICY_SOURCES_MAX {
            return Err(invalid(format!("more than {POLICY_SOURCES_MAX} sources")));
        }
        if raw.tools.len() > POLICY_TOOLS_MAX {
            return Err(invalid(format!("more than {POLICY_TOOLS_MAX} tools")));
        }
        let sources = raw.sources.into_iter().map(validate_source).collect::<Result<_, _>>()?;
        let tools = raw
            .tools
            .into_iter()
            .map(|(name, t)| validate_tool(name, t))
            .collect::<Result<_, _>>()?;
        let mut seen_ids = BTreeSet::new();
        let deny = validate_rules(raw.deny, &mut seen_ids)?;
        let allow = validate_rules(raw.allow, &mut seen_ids)?;
        Ok(Self { sources, tools, deny, allow })
    }

    /// Read and validate a policy file, refusing files over `POLICY_BYTES_MAX`
    /// before reading them.
    pub fn load_file(path: &Path) -> Result<Self, PolicyError> {
        let meta = std::fs::metadata(path).map_err(|e| PolicyError::Io(e.to_string()))?;
        if meta.len() > POLICY_BYTES_MAX {
            return Err(PolicyError::TooLarge(meta.len()));
        }
        let text = std::fs::read_to_string(path).map_err(|e| PolicyError::Io(e.to_string()))?;
        Self::from_toml_str(&text)
    }

    /// Labels for `tool`, if the policy declares any.
    pub fn tool_labels(&self, tool: &str) -> Option<&ToolLabels> {
        self.tools.get(tool)
    }

    /// Deny rules in evaluation order.
    pub fn deny_rules(&self) -> &[Rule] {
        &self.deny
    }

    /// Allow rules in evaluation order.
    pub fn allow_rules(&self) -> &[Rule] {
        &self.allow
    }

    /// Labeled tool names, sorted.
    pub fn labeled_tools(&self) -> impl Iterator<Item = &str> {
        self.tools.keys().map(String::as_str)
    }
}

fn validate_source(raw: RawSource) -> Result<Source, PolicyError> {
    let prefix = raw.prefix.trim();
    if prefix.is_empty() || prefix.starts_with('/') || prefix.starts_with('\\') {
        return Err(invalid(format!("source prefix `{}` must be relative", raw.prefix)));
    }
    let mut components = Vec::new();
    for part in prefix.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => return Err(invalid(format!("source prefix `{}` contains ..", raw.prefix))),
            p => components.push(p.to_ascii_lowercase()),
        }
    }
    if components.is_empty() {
        return Err(invalid(format!("source prefix `{}` names no directory", raw.prefix)));
    }
    Ok(Source { components, label: raw.label })
}

fn validate_tool(name: String, raw: RawTool) -> Result<(String, ToolLabels), PolicyError> {
    if name.is_empty() {
        return Err(invalid("tool name is empty"));
    }
    if raw.reads.is_empty() {
        return Err(invalid(format!("tool `{name}` has empty reads")));
    }
    let labels = ToolLabels { reads: raw.reads.into_iter().collect(), sink: raw.sink };
    Ok((name, labels))
}

fn validate_rules(raw: Vec<RawRule>, seen: &mut BTreeSet<String>) -> Result<Vec<Rule>, PolicyError> {
    if raw.len() > POLICY_RULES_MAX {
        return Err(invalid(format!("more than {POLICY_RULES_MAX} rules in one list")));
    }
    let mut rules = Vec::with_capacity(raw.len());
    for r in raw {
        let id_chars = r.id.chars().count();
        if id_chars == 0 || id_chars > RULE_ID_CHARS_MAX {
            return Err(invalid(format!("rule id must be 1..={RULE_ID_CHARS_MAX} characters")));
        }
        if r.id.starts_with("R0-") || r.id.starts_with("R4-") {
            return Err(invalid(format!("rule id `{}` uses a reserved engine prefix", r.id)));
        }
        if !seen.insert(r.id.clone()) {
            return Err(invalid(format!("duplicate rule id `{}`", r.id)));
        }
        if r.labels.is_empty() || r.sinks.is_empty() {
            return Err(invalid(format!("rule `{}` needs non-empty labels and sinks", r.id)));
        }
        if r.tools.iter().any(String::is_empty) {
            return Err(invalid(format!("rule `{}` has an empty tool name", r.id)));
        }
        rules.push(Rule {
            id: r.id,
            reason: r.reason,
            tools: r.tools.into_iter().collect(),
            labels: r.labels.into_iter().collect(),
            sinks: r.sinks.into_iter().collect(),
        });
    }
    Ok(rules)
}

// ---------------------------------------------------------------------------
// Decision
// ---------------------------------------------------------------------------

/// A labeled call: what the decision engine sees.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallEvent {
    pub tool: String,
    pub labels: BTreeSet<Sensitivity>,
    pub sink: Trust,
}

/// A structured refusal naming the rule, the label, and the sink.
/// `label`/`sink` are `None` only for engine rules raised before labeling
/// finished (unlabeled tool, unresolvable sink, unbounded arguments).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Denial {
    pub rule: String,
    pub tool: String,
    pub label: Option<Sensitivity>,
    pub sink: Option<Trust>,
    pub reason: String,
}

impl Denial {
    fn engine(rule: &str, tool: &str, reason: impl Into<String>) -> Self {
        Self { rule: rule.to_string(), tool: tool.to_string(), label: None, sink: None, reason: reason.into() }
    }

    /// The JSON body returned to the MCP client.
    pub fn to_json(&self) -> String {
        serde_json::json!({ "error": "policy denied", "policy": self }).to_string()
    }
}

/// The engine's answer for one call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Allow,
    Deny(Denial),
}

/// Pure decision: deny rules first (file order, most severe label first),
/// then every label must be covered by an allow rule for the sink.
pub fn decide(policy: &Policy, event: &CallEvent) -> Decision {
    assert!(!event.labels.is_empty(), "a labeled call always carries its tool's reads");
    for rule in &policy.deny {
        for &label in event.labels.iter().rev() {
            if rule.matches(&event.tool, label, event.sink) {
                return Decision::Deny(Denial {
                    rule: rule.id.clone(),
                    tool: event.tool.clone(),
                    label: Some(label),
                    sink: Some(event.sink),
                    reason: rule.reason.clone(),
                });
            }
        }
    }
    for &label in event.labels.iter().rev() {
        let covered = policy.allow.iter().any(|r| r.matches(&event.tool, label, event.sink));
        if !covered {
            return Decision::Deny(Denial {
                rule: RULE_DENY_BY_DEFAULT.to_string(),
                tool: event.tool.clone(),
                label: Some(label),
                sink: Some(event.sink),
                reason: "no allow rule covers this label for this sink".to_string(),
            });
        }
    }
    Decision::Allow
}

// ---------------------------------------------------------------------------
// Labeling
// ---------------------------------------------------------------------------

/// Classify a BRP endpoint with the same URL parser the transport uses.
/// Loopback means host `localhost`, an IPv4 address in 127.0.0.0/8, or `::1`
/// after URL normalization; everything else (lookalike domains, userinfo
/// tricks, trailing dots, IPv4-mapped IPv6) is `remote`.
pub fn classify_brp_endpoint(endpoint: &str) -> Result<Trust, String> {
    let url: reqwest::Url = endpoint.parse().map_err(|_| "endpoint is not a valid url".to_string())?;
    let host = url.host_str().ok_or_else(|| "endpoint has no host".to_string())?;
    let bare = host.strip_prefix('[').and_then(|h| h.strip_suffix(']')).unwrap_or(host);
    let loopback = match bare.parse::<IpAddr>() {
        Ok(IpAddr::V4(v4)) => v4.is_loopback(),
        Ok(IpAddr::V6(v6)) => v6 == Ipv6Addr::LOCALHOST,
        Err(_) => bare == "localhost",
    };
    Ok(if loopback { Trust::Loopback } else { Trust::Remote })
}

fn resolve_sink(spec: SinkSpec, args: Option<&Map<String, Value>>) -> Result<Trust, String> {
    match spec {
        SinkSpec::Trusted => Ok(Trust::Trusted),
        SinkSpec::Public => Ok(Trust::Public),
        SinkSpec::Loopback => Ok(Trust::Loopback),
        SinkSpec::Remote => Ok(Trust::Remote),
        SinkSpec::External => Ok(Trust::External),
        SinkSpec::Brp => match args.and_then(|a| a.get("endpoint")) {
            None | Some(Value::Null) => classify_brp_endpoint(DEFAULT_BRP_ENDPOINT),
            Some(Value::String(s)) => classify_brp_endpoint(s),
            Some(_) => Err("endpoint must be a string".to_string()),
        },
    }
}

/// Lexically label one string as a project path. Absolute paths and paths
/// that climb above the root are `restricted`: they cannot be proven to stay
/// out of a labeled directory (`../<root-name>/refs/...` re-enters the root).
fn label_path(sources: &[Source], text: &str, labels: &mut BTreeSet<Sensitivity>) {
    if text.starts_with('/') || text.starts_with('\\') {
        labels.insert(Sensitivity::Restricted);
        return;
    }
    let mut components: Vec<String> = Vec::new();
    for part in text.split(['/', '\\']) {
        match part {
            "" | "." => {}
            ".." => {
                if components.pop().is_none() {
                    labels.insert(Sensitivity::Restricted);
                    return;
                }
            }
            p => components.push(p.to_ascii_lowercase()),
        }
    }
    for source in sources {
        if components.starts_with(&source.components) {
            labels.insert(source.label);
        }
    }
}

/// Label every string (values and object keys) in the arguments, bounded by
/// depth, node count, and total string bytes. Key-name independent on
/// purpose: a new path parameter cannot slip past an allowlist.
fn label_args(
    sources: &[Source],
    args: &Map<String, Value>,
    labels: &mut BTreeSet<Sensitivity>,
) -> Result<(), String> {
    let mut stack: Vec<(&Value, usize)> = Vec::new();
    let mut nodes_visited = 0usize;
    let mut string_bytes = 0usize;
    let mut visit_string = |s: &str, labels: &mut BTreeSet<Sensitivity>| -> Result<(), String> {
        string_bytes = string_bytes.saturating_add(s.len());
        if string_bytes > ARGS_STRING_BYTES_MAX {
            return Err(format!("argument strings exceed {ARGS_STRING_BYTES_MAX} bytes"));
        }
        label_path(sources, s, labels);
        Ok(())
    };
    for (key, value) in args {
        visit_string(key, labels)?;
        stack.push((value, 1));
    }
    while let Some((value, depth)) = stack.pop() {
        nodes_visited += 1;
        if nodes_visited > ARGS_NODES_MAX {
            return Err(format!("arguments exceed {ARGS_NODES_MAX} nodes"));
        }
        if depth > ARGS_DEPTH_MAX {
            return Err(format!("arguments nest deeper than {ARGS_DEPTH_MAX}"));
        }
        match value {
            Value::String(s) => visit_string(s, labels)?,
            Value::Array(items) => stack.extend(items.iter().map(|v| (v, depth + 1))),
            Value::Object(map) => {
                for (key, v) in map {
                    visit_string(key, labels)?;
                    stack.push((v, depth + 1));
                }
            }
            Value::Null | Value::Bool(_) | Value::Number(_) => {}
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Session and enforcement
// ---------------------------------------------------------------------------

/// The labels already in context for one MCP session. Only grows.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Session {
    labels: BTreeSet<Sensitivity>,
}

impl Session {
    pub fn new() -> Self {
        Self::default()
    }

    /// Labels currently in context.
    pub fn labels(&self) -> &BTreeSet<Sensitivity> {
        &self.labels
    }

    /// Stage 1: label a call. Context = tool reads + argument labels +
    /// session labels. Fails (as a denial) for unlabeled tools, unresolvable
    /// sinks, and arguments over the walk bounds.
    pub fn label(
        &self,
        policy: &Policy,
        tool: &str,
        args: Option<&Map<String, Value>>,
    ) -> Result<CallEvent, Denial> {
        let Some(declared) = policy.tool_labels(tool) else {
            return Err(Denial::engine(RULE_UNLABELED_TOOL, tool, "tool has no policy labels"));
        };
        let sink = resolve_sink(declared.sink, args)
            .map_err(|e| Denial::engine(RULE_SINK_UNRESOLVABLE, tool, e))?;
        let mut labels = declared.reads.clone();
        if let Some(args) = args {
            label_args(&policy.sources, args, &mut labels)
                .map_err(|e| Denial::engine(RULE_ARGS_UNBOUNDED, tool, e))?;
        }
        labels.extend(self.labels.iter().copied());
        Ok(CallEvent { tool: tool.to_string(), labels, sink })
    }

    /// Label, decide, and on allow commit the call's labels to the session.
    pub fn admit(
        &mut self,
        policy: &Policy,
        tool: &str,
        args: Option<&Map<String, Value>>,
    ) -> Result<CallEvent, Denial> {
        let event = self.label(policy, tool, args)?;
        self.admit_event(policy, event)
    }

    /// Decide an already-labeled event (session labels are merged in first)
    /// and on allow commit its labels. Used by replay for sinks that have no
    /// registered tool yet.
    pub fn admit_event(&mut self, policy: &Policy, mut event: CallEvent) -> Result<CallEvent, Denial> {
        event.labels.extend(self.labels.iter().copied());
        match decide(policy, &event) {
            Decision::Allow => {
                self.labels.extend(event.labels.iter().copied());
                Ok(event)
            }
            Decision::Deny(denial) => Err(denial),
        }
    }
}

/// Process-wide enforcement point wrapped around the MCP tool router.
/// The mutex guards the session's label set; it is never held across an
/// `.await` (admission is synchronous and completes before the tool runs),
/// so concurrent calls are admitted in a single total order.
#[derive(Debug)]
pub struct Enforcer {
    policy: Policy,
    session: Mutex<Session>,
}

impl Enforcer {
    pub fn new(policy: Policy) -> Self {
        Self { policy, session: Mutex::new(Session::new()) }
    }

    pub fn policy(&self) -> &Policy {
        &self.policy
    }

    /// Check one `tools/call` before execution. Fails closed if the session
    /// lock is poisoned.
    pub fn admit(&self, tool: &str, args: Option<&Map<String, Value>>) -> Result<(), Denial> {
        let mut session = self.session.lock().map_err(|_| {
            Denial::engine(RULE_SESSION_UNAVAILABLE, tool, "session state poisoned")
        })?;
        session.admit(&self.policy, tool, args).map(|_| ())
    }

    /// Snapshot of the session labels.
    pub fn session_labels(&self) -> Result<BTreeSet<Sensitivity>, Denial> {
        let session = self.session.lock().map_err(|_| {
            Denial::engine(RULE_SESSION_UNAVAILABLE, "", "session state poisoned")
        })?;
        Ok(session.labels().clone())
    }
}

// ---------------------------------------------------------------------------
// Check
// ---------------------------------------------------------------------------

/// What `lumen policy check` reports. Passes iff `problems` is empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckReport {
    pub required_tools: usize,
    pub labeled_tools: usize,
    pub deny_rules: usize,
    pub allow_rules: usize,
    pub problems: Vec<String>,
}

/// Every registered tool and every `LIBRARY_TOOLS` entry must be labeled;
/// labels and rule filters may only name those tools. Problems are sorted.
pub fn check(policy: &Policy, registered: &[&str]) -> CheckReport {
    let required: BTreeSet<&str> = registered.iter().chain(LIBRARY_TOOLS).copied().collect();
    let mut problems = Vec::new();
    for tool in &required {
        if policy.tool_labels(tool).is_none() {
            problems.push(format!("tool `{tool}` has no labels"));
        }
    }
    for tool in policy.labeled_tools() {
        if !required.contains(tool) {
            problems.push(format!("labels for unknown tool `{tool}`"));
        }
    }
    for rule in policy.deny.iter().chain(&policy.allow) {
        for tool in &rule.tools {
            if !required.contains(tool.as_str()) {
                problems.push(format!("rule `{}` names unknown tool `{tool}`", rule.id));
            }
        }
    }
    problems.sort();
    CheckReport {
        required_tools: required.len(),
        labeled_tools: policy.tools.len(),
        deny_rules: policy.deny.len(),
        allow_rules: policy.allow.len(),
        problems,
    }
}

// ---------------------------------------------------------------------------
// Replay
// ---------------------------------------------------------------------------

/// One scripted step.
#[derive(Debug, Clone, Copy)]
pub enum StepInput {
    /// A real tool call: full labeling path. `args` is a JSON object.
    Call { tool: &'static str, args: &'static str },
    /// A pre-labeled event for sinks with no registered tool yet
    /// (cart exports, A2A). Session labels are still merged in.
    Event { tool: &'static str, labels: &'static [Sensitivity], sink: Trust },
}

/// Expected decision for a step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Expect {
    Allow,
    Deny(&'static str),
}

#[derive(Debug, Clone, Copy)]
pub struct Step {
    pub input: StepInput,
    pub expect: Expect,
}

/// A scenario runs its steps in order against a fresh session.
#[derive(Debug, Clone, Copy)]
pub struct Scenario {
    pub id: &'static str,
    pub steps: &'static [Step],
}

/// Result of replaying one scenario.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScenarioOutcome {
    pub id: &'static str,
    pub passed: bool,
    /// Empty on pass; otherwise the first unexpected step.
    pub detail: String,
}

/// Replay scenarios against `policy`. Stops each scenario at its first
/// unexpected decision.
pub fn replay(policy: &Policy, scenarios: &[Scenario]) -> Vec<ScenarioOutcome> {
    scenarios.iter().map(|s| replay_one(policy, s)).collect()
}

fn replay_one(policy: &Policy, scenario: &Scenario) -> ScenarioOutcome {
    let mut session = Session::new();
    for (index, step) in scenario.steps.iter().enumerate() {
        let got = match run_step(policy, &mut session, step.input) {
            Ok(got) => got,
            Err(e) => return fail(scenario.id, format!("step {index}: {e}")),
        };
        let matched = match (step.expect, &got) {
            (Expect::Allow, Ok(_)) => true,
            (Expect::Deny(rule), Err(denial)) => denial.rule == rule,
            _ => false,
        };
        if !matched {
            let got_text = match &got {
                Ok(_) => "allow".to_string(),
                Err(d) => format!("deny {}", d.rule),
            };
            return fail(scenario.id, format!("step {index}: expected {:?}, got {got_text}", step.expect));
        }
    }
    ScenarioOutcome { id: scenario.id, passed: true, detail: String::new() }
}

fn fail(id: &'static str, detail: String) -> ScenarioOutcome {
    ScenarioOutcome { id, passed: false, detail }
}

/// Outer `Err` is a malformed scenario (bad JSON), not a policy decision.
fn run_step(
    policy: &Policy,
    session: &mut Session,
    input: StepInput,
) -> Result<Result<CallEvent, Denial>, String> {
    match input {
        StepInput::Call { tool, args } => {
            let parsed: Value = serde_json::from_str(args).map_err(|e| format!("bad args json: {e}"))?;
            let Value::Object(map) = parsed else {
                return Err("args must be a JSON object".to_string());
            };
            Ok(session.admit(policy, tool, Some(&map)))
        }
        StepInput::Event { tool, labels, sink } => {
            if labels.is_empty() {
                return Err("event needs at least one label".to_string());
            }
            let event = CallEvent { tool: tool.to_string(), labels: labels.iter().copied().collect(), sink };
            Ok(session.admit_event(policy, event))
        }
    }
}

const fn call(tool: &'static str, args: &'static str, expect: Expect) -> Step {
    Step { input: StepInput::Call { tool, args }, expect }
}

const fn event(tool: &'static str, labels: &'static [Sensitivity], sink: Trust, expect: Expect) -> Step {
    Step { input: StepInput::Event { tool, labels, sink }, expect }
}

const ALLOW: Expect = Expect::Allow;
const R1: Expect = Expect::Deny("R1-high-never-public");
const R2: Expect = Expect::Deny("R2-lua-refuses-untrusted");
const R3: Expect = Expect::Deny("R3-restricted-loopback-brp-only");
const R4: Expect = Expect::Deny(RULE_DENY_BY_DEFAULT);
const LOW: &[Sensitivity] = &[Sensitivity::Low];
const HIGH: &[Sensitivity] = &[Sensitivity::High];
const RESTRICTED: &[Sensitivity] = &[Sensitivity::Restricted];
const LUA_CLEAN: &str =
    r#"{"doc":"sprites/hero.lumen.json","script":"sprite.fill(0,1,1,1,1)","opt_in":true}"#;
const READ_RESTRICTED: &str = r#"{"path":"refs/restricted/vault.png"}"#;

/// The shipped scenario suite: `ok_*` validation and `adv_*` adversarial
/// scenarios in equal number. Expectations are compiled in, so loosening
/// `lumen-policy.toml` makes `lumen policy replay` fail.
pub const SCENARIOS: &[Scenario] = &[
    Scenario { id: "ok_low_sprite_info_to_trusted", steps: &[
        call("sprite_info", r#"{"path":"sprites/hero.png"}"#, ALLOW),
    ] },
    Scenario { id: "ok_brp_default_endpoint_is_loopback", steps: &[
        call("bevy_discover", "{}", ALLOW),
        call("bevy_discover", r#"{"endpoint":null}"#, ALLOW),
    ] },
    Scenario { id: "ok_high_reference_to_loopback_brp", steps: &[
        call("sprite_info", r#"{"path":"refs/proprietary/sheet.png"}"#, ALLOW),
        call("bevy_call", r#"{"endpoint":"http://127.0.0.1:15702","method":"bevy/list"}"#, ALLOW),
    ] },
    Scenario { id: "ok_high_reference_to_remote_brp", steps: &[
        call("sprite_info", r#"{"path":"refs/licensed/style.png"}"#, ALLOW),
        call("bevy_discover", r#"{"endpoint":"http://192.168.1.20:15702"}"#, ALLOW),
    ] },
    Scenario { id: "ok_restricted_to_loopback_variants", steps: &[
        call("sprite_info", READ_RESTRICTED, ALLOW),
        call("bevy_discover", r#"{"endpoint":"http://localhost:15702"}"#, ALLOW),
        call("bevy_discover", r#"{"endpoint":"http://LOCALHOST:15702"}"#, ALLOW),
        call("bevy_discover", r#"{"endpoint":"http://[::1]:15702"}"#, ALLOW),
        call("bevy_status", r#"{"endpoint":"http://127.0.0.1:15702"}"#, ALLOW),
    ] },
    Scenario { id: "ok_ipv4_shorthand_normalizes_to_loopback", steps: &[
        call("sprite_info", READ_RESTRICTED, ALLOW),
        call("bevy_discover", r#"{"endpoint":"http://127.1:15702"}"#, ALLOW),
    ] },
    Scenario { id: "ok_lua_with_clean_context", steps: &[
        call("sprite_info", r#"{"path":"sprites/hero.png"}"#, ALLOW),
        call("run_lua_script", LUA_CLEAN, ALLOW),
    ] },
    Scenario { id: "ok_low_cart_export_to_public", steps: &[
        event("cart_export_wasm4", LOW, Trust::Public, ALLOW),
    ] },
    Scenario { id: "ok_low_a2a_task_to_external", steps: &[
        event("a2a_dispatch", LOW, Trust::External, ALLOW),
    ] },
    Scenario { id: "ok_untrusted_local_export", steps: &[
        call("sprite_info", r#"{"path":"refs/web/pasted.png"}"#, ALLOW),
        call("export_sprite", r#"{"doc":"sprites/hero.lumen.json","output":"out/hero.png"}"#, ALLOW),
    ] },
    Scenario { id: "ok_lookalike_paths_stay_low", steps: &[
        call("sprite_info", r#"{"path":"sprites/refs/proprietary/a.png"}"#, ALLOW),
        call("sprite_info", r#"{"path":"refs/proprietary-fan/a.png"}"#, ALLOW),
        event("cart_export_tic80", LOW, Trust::Public, ALLOW),
    ] },
    Scenario { id: "ok_plain_text_args_stay_low", steps: &[
        call("text_measure", r#"{"text":"refs proprietary... hello","size":8}"#, ALLOW),
        event("cart_export_wasm4", LOW, Trust::Public, ALLOW),
    ] },
    Scenario { id: "ok_denied_call_does_not_taint", steps: &[
        event("cart_export_wasm4", HIGH, Trust::Public, R1),
        event("cart_export_wasm4", LOW, Trust::Public, ALLOW),
    ] },
    Scenario { id: "adv_prompt_injection_lua_after_web_read", steps: &[
        call("sprite_info", r#"{"path":"refs/web/injected.png"}"#, ALLOW),
        call("validate_atlas", r#"{"path":"sprites/atlas.png"}"#, ALLOW),
        call("bevy_discover", "{}", ALLOW),
        call("run_lua_script", LUA_CLEAN, R2),
    ] },
    Scenario { id: "adv_prompt_injection_lua_on_web_doc", steps: &[
        call(
            "run_lua_script",
            r#"{"doc":"refs/web/payload.lumen.json","script":"sprite.fill(0,0,0,0,0)","opt_in":true}"#,
            R2,
        ),
    ] },
    Scenario { id: "adv_injection_exfil_to_external_agent", steps: &[
        call("sprite_info", r#"{"path":"refs/web/injected.png"}"#, ALLOW),
        event("a2a_dispatch", LOW, Trust::External, R4),
        call("sprite_info", r#"{"path":"refs/proprietary/sheet.png"}"#, ALLOW),
        event("a2a_dispatch", LOW, Trust::External, R1),
    ] },
    Scenario { id: "adv_label_confusion_case_folding", steps: &[
        call("sprite_info", r#"{"path":"REFS/Proprietary/sheet.png"}"#, ALLOW),
        event("cart_export_tic80", LOW, Trust::Public, R1),
    ] },
    Scenario { id: "adv_label_confusion_dot_segments", steps: &[
        call("sprite_info", r#"{"path":"./sprites/../refs//proprietary/./sheet.png"}"#, ALLOW),
        event("cart_export_wasm4", LOW, Trust::Public, R1),
    ] },
    Scenario { id: "adv_label_confusion_backslashes", steps: &[
        call("sprite_info", r#"{"path":"refs\\licensed\\style.png"}"#, ALLOW),
        event("cart_export_wasm4", LOW, Trust::Public, R1),
    ] },
    Scenario { id: "adv_label_confusion_self_declared_low", steps: &[
        call(
            "sprite_info",
            r#"{"path":"refs/licensed/style.png","sensitivity":"low","labels":["low"],"_meta":{"label":"low"}}"#,
            ALLOW,
        ),
        event("cart_export_wasm4", LOW, Trust::Public, R1),
    ] },
    Scenario { id: "adv_label_confusion_escape_and_reenter", steps: &[
        call("sprite_info", r#"{"path":"../sprite-mcp/refs/proprietary/sheet.png"}"#, ALLOW),
        call("bevy_discover", r#"{"endpoint":"http://10.0.0.7:15702"}"#, R3),
        event("cart_export_wasm4", LOW, Trust::Public, R4),
    ] },
    Scenario { id: "adv_sink_spoof_remote_dressed_as_loopback", steps: &[
        call("sprite_info", READ_RESTRICTED, ALLOW),
        call("bevy_discover", r#"{"endpoint":"http://127.0.0.1.nip.io:15702"}"#, R3),
        call("bevy_discover", r#"{"endpoint":"http://localhost@evil.example:15702"}"#, R3),
        call("bevy_discover", r#"{"endpoint":"http://localhost.evil.example:15702"}"#, R3),
        call("bevy_discover", r#"{"endpoint":"http://localhost.:15702"}"#, R3),
        call("bevy_discover", r#"{"endpoint":"http://[::ffff:127.0.0.1]:15702"}"#, R3),
        call("bevy_call", r#"{"endpoint":"http://0.0.0.0:15702","method":"bevy/list"}"#, R3),
    ] },
    Scenario { id: "adv_sink_unresolvable_endpoint", steps: &[
        call("bevy_call", r#"{"endpoint":15702,"method":"bevy/list"}"#, Expect::Deny(RULE_SINK_UNRESOLVABLE)),
        call("bevy_call", r#"{"endpoint":"not a url","method":"bevy/list"}"#, Expect::Deny(RULE_SINK_UNRESOLVABLE)),
        call("bevy_status", r#"{"endpoint":["http://127.0.0.1:15702"]}"#, Expect::Deny(RULE_SINK_UNRESOLVABLE)),
    ] },
    Scenario { id: "adv_missing_label_tool", steps: &[
        call("shadow_exec", r#"{"cmd":"cat ~/.ssh/id_ed25519"}"#, Expect::Deny(RULE_UNLABELED_TOOL)),
        call("Sprite_Info", r#"{"path":"sprites/hero.png"}"#, Expect::Deny(RULE_UNLABELED_TOOL)),
    ] },
    Scenario { id: "adv_restricted_to_public_is_default_denied", steps: &[
        event("cart_export_wasm4", RESTRICTED, Trust::Public, R4),
        event("a2a_dispatch", RESTRICTED, Trust::External, R4),
    ] },
    Scenario { id: "adv_args_depth_bomb", steps: &[
        call("sprite_info", r#"{"path":[[[[[[[[[["refs/web/x.png"]]]]]]]]]]}"#, Expect::Deny(RULE_ARGS_UNBOUNDED)),
        call("run_lua_script", LUA_CLEAN, ALLOW),
    ] },
];
