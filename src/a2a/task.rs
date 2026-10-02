//! Task state machine, the bounded task store, and the step runner.
//!
//! Lock discipline: one `std::sync::Mutex<Store>` guards every task entry.
//! It is never held across `.await`; operations run with the lock released
//! and commit their outcome by re-locking. Every state change goes through
//! `TaskEntry::transition`, which refuses illegal moves, so a published state
//! is always reachable from `submitted` along the documented edges.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tokio::sync::broadcast;

use super::registry::SkillSpec;
use super::{Shared, artifact_relative_path};

/// Maximum steps in one task. Bounds the runner loop and the artifact list.
pub(crate) const STEPS_MAX: usize = 16;
/// Non-terminal tasks (submitted, working, input-required) held at once.
pub(crate) const LIVE_TASKS_MAX: usize = 16;
/// Terminal tasks retained for `tasks/get`; the oldest are evicted first.
pub(crate) const RETAINED_TERMINAL_TASKS_MAX: usize = 128;
/// How long a task may sit in `input-required` before it fails closed.
pub(crate) const REVIEW_WINDOW: Duration = Duration::from_secs(24 * 60 * 60);
/// Per-task event channel capacity. A task emits at most 2 events per step
/// plus 2 (start, final), so 64 cannot lag for `STEPS_MAX = 16`.
const EVENT_CHANNEL_CAPACITY: usize = 64;
/// Upper bound on events one run emits; consumers loop at most this often.
pub(crate) const EVENTS_PER_RUN_MAX: usize = 2 * STEPS_MAX + 4;
const _: () = assert!(EVENTS_PER_RUN_MAX <= EVENT_CHANNEL_CAPACITY);
const TASK_ID_PREFIX: &str = "lumen-task-";

/// A2A task states. Wire form is kebab-case (`as_str`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    Submitted,
    Working,
    InputRequired,
    Completed,
    Failed,
    Canceled,
    Rejected,
}

impl TaskState {
    /// Kebab-case wire name.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Submitted => "submitted",
            Self::Working => "working",
            Self::InputRequired => "input-required",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Canceled => "canceled",
            Self::Rejected => "rejected",
        }
    }

    /// Terminal states accept no further transitions.
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Canceled | Self::Rejected)
    }

    /// The legal edges. Progress within `working` is not a transition.
    /// `rejected` is reachable only from review (`input-required`).
    pub fn can_transition_to(self, next: TaskState) -> bool {
        use TaskState::{Canceled, Completed, Failed, InputRequired, Rejected, Submitted, Working};
        match self {
            Submitted => matches!(next, Working | Canceled | Failed),
            Working => matches!(next, InputRequired | Completed | Failed | Canceled),
            InputRequired => matches!(next, Working | Completed | Rejected | Canceled | Failed),
            Completed | Failed | Canceled | Rejected => false,
        }
    }
}

/// One validated step: a registry skill plus its JSON arguments object.
pub(crate) struct Step {
    pub(crate) skill: &'static SkillSpec,
    pub(crate) arguments: Value,
}

/// Structured failure carried by `failed`, `canceled`, and `rejected`.
#[derive(Debug, Clone)]
pub(crate) struct TaskError {
    pub(crate) code: &'static str,
    pub(crate) message: String,
    pub(crate) step: Option<usize>,
    pub(crate) skill: Option<&'static str>,
    pub(crate) reasons: Vec<String>,
}

impl TaskError {
    pub(crate) fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self { code, message: message.into(), step: None, skill: None, reasons: Vec::new() }
    }

    fn at(mut self, step: usize, skill: &'static str) -> Self {
        self.step = Some(step);
        self.skill = Some(skill);
        self
    }

    fn to_wire(&self) -> Value {
        json!({
            "code": self.code,
            "message": self.message,
            "step": self.step,
            "skill": self.skill,
            "reasons": self.reasons,
        })
    }
}

/// One event for stream subscribers: a JSON-RPC `result` payload.
#[derive(Debug, Clone)]
pub(crate) struct TaskEvent {
    pub(crate) result: Value,
    pub(crate) is_final: bool,
}

pub(crate) struct TaskEntry {
    pub(crate) id: String,
    context_id: String,
    pub(crate) state: TaskState,
    progress: Option<String>,
    artifacts: Vec<Value>,
    error: Option<TaskError>,
    steps: Vec<Step>,
    next_step: usize,
    review_after: Option<usize>,
    pub(crate) cancel: Arc<AtomicBool>,
    pub(crate) events: broadcast::Sender<TaskEvent>,
    run_started: Instant,
    paused_at: Option<Instant>,
}

impl TaskEntry {
    /// Commit a state change and publish it. Illegal edges change nothing.
    fn transition(&mut self, next: TaskState) -> Result<(), TaskState> {
        if !self.state.can_transition_to(next) {
            return Err(self.state);
        }
        self.state = next;
        self.paused_at = (next == TaskState::InputRequired).then(Instant::now);
        if next != TaskState::Working {
            self.progress = None;
        }
        self.publish(self.status_event());
        Ok(())
    }

    fn finish(&mut self, next: TaskState, error: Option<TaskError>) {
        assert!(next.is_terminal() || next == TaskState::InputRequired);
        self.error = error;
        if let Err(from) = self.transition(next) {
            // Only reachable if a caller skipped its own state check; the
            // published state stays as it was.
            debug_assert!(false, "illegal transition {} -> {}", from.as_str(), next.as_str());
        }
    }

    fn publish(&self, event: TaskEvent) {
        // No subscribers is normal (non-streaming caller); not an error.
        let _ = self.events.send(event);
    }

    fn status_wire(&self) -> Value {
        let mut parts = Vec::new();
        if let Some(text) = &self.progress {
            parts.push(json!({"kind": "text", "text": text}));
        }
        if let Some(err) = &self.error {
            parts.push(json!({"kind": "data", "data": {"error": err.to_wire()}}));
        }
        if self.state == TaskState::InputRequired {
            parts.push(json!({"kind": "data", "data": {
                "awaiting": "review",
                "reply": {"decision": "approve | reject", "reasons": ["optional"]},
                "remaining_steps": self.steps.len().saturating_sub(self.next_step),
            }}));
        }
        let message = (!parts.is_empty()).then(|| {
            json!({
                "kind": "message",
                "role": "agent",
                "messageId": format!("{}-{}", self.id, self.state.as_str()),
                "taskId": self.id,
                "contextId": self.context_id,
                "parts": parts,
            })
        });
        json!({"state": self.state.as_str(), "message": message})
    }

    fn status_event(&self) -> TaskEvent {
        let is_final = self.state.is_terminal() || self.state == TaskState::InputRequired;
        TaskEvent {
            result: json!({
                "kind": "status-update",
                "taskId": self.id,
                "contextId": self.context_id,
                "status": self.status_wire(),
                "final": is_final,
            }),
            is_final,
        }
    }

    /// The A2A `Task` object.
    pub(crate) fn to_wire(&self) -> Value {
        json!({
            "kind": "task",
            "id": self.id,
            "contextId": self.context_id,
            "status": self.status_wire(),
            "artifacts": self.artifacts,
        })
    }
}

/// All tasks, keyed by sequence number so eviction is oldest-first.
#[derive(Default)]
pub(crate) struct Store {
    tasks: BTreeMap<u64, TaskEntry>,
    next_seq: u64,
}

/// Parse `lumen-task-<seq>`; anything else is simply not a task id.
pub(crate) fn parse_task_id(id: &str) -> Option<u64> {
    let digits = id.strip_prefix(TASK_ID_PREFIX)?;
    if digits.is_empty() || digits.len() > 20 || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

impl Store {
    pub(crate) fn get(&self, id: &str) -> Option<&TaskEntry> {
        self.tasks.get(&parse_task_id(id)?)
    }

    pub(crate) fn get_mut(&mut self, id: &str) -> Option<&mut TaskEntry> {
        self.tasks.get_mut(&parse_task_id(id)?)
    }

    fn live_count(&self) -> usize {
        self.tasks.values().filter(|t| !t.state.is_terminal()).count()
    }

    /// Fail reviews left open past `REVIEW_WINDOW`, then evict the oldest
    /// terminal tasks beyond the retention bound. Bounded by store size.
    pub(crate) fn sweep(&mut self, now: Instant) {
        for entry in self.tasks.values_mut() {
            let expired = entry.state == TaskState::InputRequired
                && entry.paused_at.is_some_and(|t| now.duration_since(t) >= REVIEW_WINDOW);
            if expired {
                let err = TaskError::new("review_expired", "no review decision within window");
                entry.finish(TaskState::Failed, Some(err));
            }
        }
        let terminal: Vec<u64> =
            self.tasks.iter().filter(|(_, t)| t.state.is_terminal()).map(|(k, _)| *k).collect();
        let excess = terminal.len().saturating_sub(RETAINED_TERMINAL_TASKS_MAX);
        for seq in terminal.into_iter().take(excess) {
            self.tasks.remove(&seq);
        }
    }

    /// Validate capacity, then insert a `submitted` task. Returns its seq.
    pub(crate) fn create(
        &mut self,
        context_id: Option<String>,
        steps: Vec<Step>,
        review_after: Option<usize>,
    ) -> Option<u64> {
        assert!(!steps.is_empty() && steps.len() <= STEPS_MAX);
        assert!(review_after.is_none_or(|k| k < steps.len()));
        self.sweep(Instant::now());
        if self.live_count() >= LIVE_TASKS_MAX {
            return None;
        }
        let seq = self.next_seq;
        self.next_seq = self.next_seq.checked_add(1)?;
        let id = format!("{TASK_ID_PREFIX}{seq}");
        let (events, _) = broadcast::channel(EVENT_CHANNEL_CAPACITY);
        let entry = TaskEntry {
            context_id: context_id.unwrap_or_else(|| format!("lumen-ctx-{seq}")),
            id,
            state: TaskState::Submitted,
            progress: None,
            artifacts: Vec::new(),
            error: None,
            steps,
            next_step: 0,
            review_after,
            cancel: Arc::new(AtomicBool::new(false)),
            events,
            run_started: Instant::now(),
            paused_at: None,
        };
        self.tasks.insert(seq, entry);
        Some(seq)
    }
}

/// The reviewer's verdict on an `input-required` task.
pub(crate) enum Decision {
    Approve,
    Reject(Vec<String>),
}

/// Outcome of applying a review decision.
pub(crate) enum Resumed {
    /// Remaining steps exist; the caller must spawn the runner.
    Run,
    /// The task is now terminal.
    Settled,
}

/// Apply a review decision. The caller has verified `input-required`.
pub(crate) fn apply_decision(entry: &mut TaskEntry, decision: Decision) -> Resumed {
    assert_eq!(entry.state, TaskState::InputRequired);
    entry.review_after = None;
    match decision {
        Decision::Reject(reasons) => {
            let mut err = TaskError::new("review_rejected", "reviewer rejected the trial");
            err.reasons = reasons;
            entry.finish(TaskState::Rejected, Some(err));
            Resumed::Settled
        }
        Decision::Approve if entry.next_step >= entry.steps.len() => {
            entry.finish(TaskState::Completed, None);
            Resumed::Settled
        }
        Decision::Approve => {
            entry.run_started = Instant::now();
            let moved = entry.transition(TaskState::Working);
            assert!(moved.is_ok(), "input-required -> working is a legal edge");
            Resumed::Run
        }
    }
}

/// Cancel a task that is not running a step (`input-required`).
pub(crate) fn cancel_paused(entry: &mut TaskEntry) {
    assert_eq!(entry.state, TaskState::InputRequired);
    let err = TaskError::new("canceled", "canceled while awaiting review");
    entry.finish(TaskState::Canceled, Some(err));
}

/// What the runner should do at a step boundary.
enum Boundary {
    Run {
        index: usize,
        skill: &'static str,
        op: super::registry::OpFn,
        args: Value,
        budget: Duration,
    },
    Stop,
}

/// Decide the next action under the lock. This is the only place the cancel
/// flag and deadline are consulted, so cancellation lands between steps.
fn at_boundary(entry: &mut TaskEntry, deadline: Duration) -> Boundary {
    if entry.state.is_terminal() || entry.state == TaskState::InputRequired {
        return Boundary::Stop;
    }
    let index = entry.next_step;
    if entry.cancel.load(Ordering::Acquire) {
        let mut err = TaskError::new("canceled", "canceled at a step boundary");
        err.step = Some(index);
        entry.finish(TaskState::Canceled, Some(err));
        return Boundary::Stop;
    }
    let elapsed = entry.run_started.elapsed();
    let Some(budget) = deadline.checked_sub(elapsed).filter(|b| !b.is_zero()) else {
        entry.finish(TaskState::Failed, Some(TaskError::new("timeout", "task deadline reached")));
        return Boundary::Stop;
    };
    if index > 0 && entry.review_after == Some(index - 1) {
        entry.finish(TaskState::InputRequired, None);
        return Boundary::Stop;
    }
    if index >= entry.steps.len() {
        entry.finish(TaskState::Completed, None);
        return Boundary::Stop;
    }
    if entry.state == TaskState::Submitted {
        let moved = entry.transition(TaskState::Working);
        assert!(moved.is_ok(), "submitted -> working is a legal edge");
    }
    let step = &entry.steps[index];
    entry.progress =
        Some(format!("step {}/{}: {}", index + 1, entry.steps.len(), step.skill.name));
    entry.publish(entry.status_event());
    Boundary::Run {
        index,
        skill: step.skill.name,
        op: step.skill.run,
        args: step.arguments.clone(),
        budget,
    }
}

/// Turn one step's output into an A2A artifact. Any `path` the operation
/// reports must resolve inside the project root; it is rewritten to its
/// project-relative form so absolute host paths never leave the server.
fn build_artifact(
    root: &Path,
    task_id: &str,
    index: usize,
    skill: &str,
    mut output: Value,
) -> Result<Value, TaskError> {
    let mut parts = Vec::new();
    if let Some(path) = output.get("path").and_then(Value::as_str) {
        let rel = artifact_relative_path(root, Path::new(path))
            .map_err(|e| TaskError::new("artifact_rejected", e.to_string()))?;
        let mime = if rel.ends_with(".png") { "image/png" } else { "application/json" };
        let name = rel.rsplit('/').next().unwrap_or(&rel).to_string();
        parts.push(json!({"kind": "file", "file": {"uri": rel, "name": name, "mimeType": mime}}));
        output["path"] = Value::String(rel);
    }
    parts.insert(0, json!({"kind": "data", "data": output}));
    Ok(json!({
        "artifactId": format!("{task_id}-step-{index}"),
        "name": skill,
        "parts": parts,
    }))
}

/// Run steps until the task leaves `working`. Owned by a detached tokio task
/// whose lifetime is bounded by `STEPS_MAX` iterations and the deadline.
pub(crate) async fn run_task(shared: Arc<Shared>, seq: u64) {
    for _ in 0..=STEPS_MAX {
        let boundary = {
            let Ok(mut store) = shared.store.lock() else { return };
            let Some(entry) = store.tasks.get_mut(&seq) else { return };
            at_boundary(entry, shared.task_deadline)
        };
        let Boundary::Run { index, skill, op, args, budget } = boundary else { return };
        let outcome = tokio::time::timeout(budget, op(shared.root.clone(), args)).await;
        let task_id = format!("{TASK_ID_PREFIX}{seq}");
        // Path canonicalization touches the filesystem: do it before locking.
        let output = match outcome {
            Err(_) => Err(TaskError::new("timeout", "task deadline reached mid-step")),
            Ok(Err(e)) => Err(TaskError::new("operation_failed", e.to_string())),
            Ok(Ok(value)) => build_artifact(&shared.root, &task_id, index, skill, value),
        };
        let Ok(mut store) = shared.store.lock() else { return };
        let Some(entry) = store.tasks.get_mut(&seq) else { return };
        match output {
            Ok(artifact) => {
                entry.publish(TaskEvent {
                    result: json!({
                        "kind": "artifact-update",
                        "taskId": entry.id,
                        "contextId": entry.context_id,
                        "artifact": artifact,
                        "append": false,
                        "lastChunk": true,
                    }),
                    is_final: false,
                });
                entry.artifacts.push(artifact);
                entry.next_step = index + 1;
            }
            Err(err) => {
                entry.finish(TaskState::Failed, Some(err.at(index, skill)));
                return;
            }
        }
    }
}
