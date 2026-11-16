//! Cross-MCP trigger layer (DESIGN §D4): a per-member rule engine.
//!
//! Rules are data (`triggers.toml`), listed in the Fleet panel, stamped
//! with BLOCH lane law from birth: `origin_lane` + `content_label`.
//! This member is qompass/sfw; a rule carrying any other stamp is
//! rejected at load. Consequential targets may only fire in `plan`
//! mode — the engine stages a proposal and never dispatches them.
//! Firing is idempotent on a hash of (rule id, canonical event), depth
//! is capped, and every evaluation outcome appends an audit line.

use std::collections::{HashSet, VecDeque};

use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::events::Event;

/// Hard ceiling for trigger chain depth (DESIGN §D4: default 1, max 2).
pub const DEPTH_HARD_MAX: u32 = 2;
/// Maximum rules loaded from one config file.
pub const RULES_MAX: usize = 64;
/// Maximum conditions on one rule.
pub const CONDITIONS_MAX: usize = 8;
/// Maximum firings recorded in the engine's in-memory history.
pub const FIRINGS_MAX: usize = 256;

/// The only lane stamp this member may emit (BLOCH lane law, D-1030).
pub const ORIGIN_LANE: &str = "qompass";
/// The only content label this member may emit.
pub const CONTENT_LABEL: &str = "sfw";

/// Typed rule-load and evaluation failures.
#[derive(Debug, Error)]
pub enum TriggerError {
    /// The TOML did not parse or a rule failed validation.
    #[error("trigger config invalid: {0}")]
    Invalid(String),
}

/// One bounded predicate condition over a named event field.
/// Comparisons and boolean truth only — no expression language.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Condition {
    /// Named event field to read.
    pub field: String,
    /// One of `eq`, `gt`, `is_true`.
    pub op: String,
    /// Comparison value for `eq` / `gt` (ignored for `is_true`).
    #[serde(default)]
    pub value: Value,
}

impl Condition {
    fn matches(&self, event: &Event) -> bool {
        let Some(actual) = event.fields.get(&self.field) else {
            return false;
        };
        match self.op.as_str() {
            "eq" => actual == &self.value,
            "gt" => match (actual.as_f64(), self.value.as_f64()) {
                (Some(a), Some(b)) => a > b,
                _ => false,
            },
            "is_true" => actual.as_bool() == Some(true),
            _ => false,
        }
    }
}

/// Firing mode: `auto` dispatches through the sink; `plan` only stages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleMode {
    /// Dispatch immediately through the sink.
    Auto,
    /// Stage a proposal for operator approval; never dispatch.
    Plan,
}

/// One declared trigger rule (data, not code).
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Rule {
    /// Stable rule id, part of the idempotency key.
    pub id: String,
    /// BLOCH birth stamp; must equal [`ORIGIN_LANE`] for this member.
    pub origin_lane: String,
    /// BLOCH birth stamp; must equal [`CONTENT_LABEL`] for this member.
    pub content_label: String,
    /// Event kind that activates this rule.
    pub event: String,
    /// Bounded conditions, AND-combined. Empty = always matches.
    #[serde(default)]
    pub conditions: Vec<Condition>,
    /// Target member name (display + sink routing label).
    pub target_mcp: String,
    /// Target tool on that member.
    pub target_tool: String,
    /// Event field copied into the call args as `output_path`, if set.
    #[serde(default)]
    pub args_output_field: Option<String>,
    /// Firing mode.
    pub mode: RuleMode,
    /// True when the target tool writes/publishes/deploys: plan-only.
    #[serde(default)]
    pub consequential: bool,
    /// Depth cap for this rule (clamped to [`DEPTH_HARD_MAX`]).
    #[serde(default = "default_max_depth")]
    pub max_depth: u32,
    /// Operator enable switch (Fleet panel mirrors this).
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_max_depth() -> u32 {
    1
}

fn default_enabled() -> bool {
    true
}

#[derive(Debug, Deserialize)]
struct RulesFile {
    #[serde(default)]
    rule: Vec<Rule>,
}

/// The dispatch route for auto rules. The real app wires an MCP client
/// (or an explicit unconfigured sink); tests use [`FakeSink`].
pub trait TriggerSink {
    /// Dispatch one call to `target_mcp`'s `tool` with `args`.
    fn dispatch(&mut self, target_mcp: &str, tool: &str, args: Value) -> Result<Value, String>;
}

/// Sink that refuses everything: used when no target transport is
/// configured, so an auto rule's failure is honest and audited.
#[derive(Debug, Default)]
pub struct UnconfiguredSink;

impl TriggerSink for UnconfiguredSink {
    fn dispatch(&mut self, target_mcp: &str, _tool: &str, _args: Value) -> Result<Value, String> {
        Err(format!("no transport configured for target '{target_mcp}'"))
    }
}

/// Recording sink for tests.
#[derive(Debug, Default)]
pub struct FakeSink {
    /// Every dispatched `(target, tool, args)`, in order.
    pub dispatched: Vec<(String, String, Value)>,
    /// When set, every dispatch fails with this message.
    pub fail_with: Option<String>,
}

impl TriggerSink for FakeSink {
    fn dispatch(&mut self, target_mcp: &str, tool: &str, args: Value) -> Result<Value, String> {
        self.dispatched
            .push((target_mcp.to_string(), tool.to_string(), args));
        match &self.fail_with {
            Some(message) => Err(message.clone()),
            None => Ok(json!({"status": "accepted"})),
        }
    }
}

/// A staged plan-mode proposal: the exact call an operator may approve.
#[derive(Debug, Clone, PartialEq)]
pub struct Proposal {
    /// Rule that produced this proposal.
    pub rule_id: String,
    /// Target member.
    pub target_mcp: String,
    /// Target tool.
    pub target_tool: String,
    /// Exact arguments that approval would dispatch.
    pub args: Value,
}

/// What one engine evaluation decided, per matched rule.
#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    /// Auto rule dispatched through the sink.
    Fired {
        /// Rule id.
        rule_id: String,
    },
    /// Plan rule staged a proposal; nothing was dispatched.
    Proposed {
        /// Rule id.
        rule_id: String,
    },
    /// Duplicate (rule, event) — dropped by idempotency.
    DroppedDuplicate {
        /// Rule id.
        rule_id: String,
    },
    /// Event depth exceeded the rule's cap — dropped.
    DroppedDepth {
        /// Rule id.
        rule_id: String,
    },
    /// Auto dispatch failed at the sink; audited, not retried silently.
    SinkFailed {
        /// Rule id.
        rule_id: String,
        /// Sink error text.
        error: String,
    },
}

/// The rule engine: rules, idempotency set, proposals, audit trail.
#[derive(Debug)]
pub struct Engine {
    rules: Vec<Rule>,
    fired_keys: HashSet<String>,
    proposals: VecDeque<Proposal>,
    audit: VecDeque<String>,
}

impl Engine {
    /// Build an engine from validated rules.
    pub fn new(rules: Vec<Rule>) -> Result<Engine, TriggerError> {
        if rules.len() > RULES_MAX {
            return Err(TriggerError::Invalid(format!(
                "{} rules over bound {RULES_MAX}",
                rules.len()
            )));
        }
        for rule in &rules {
            validate_rule(rule)?;
        }
        Ok(Engine {
            rules,
            fired_keys: HashSet::new(),
            proposals: VecDeque::new(),
            audit: VecDeque::new(),
        })
    }

    /// Load rules from TOML text (the `triggers.toml` shape).
    pub fn from_toml(text: &str) -> Result<Engine, TriggerError> {
        let parsed: RulesFile =
            toml::from_str(text).map_err(|e| TriggerError::Invalid(e.to_string()))?;
        Engine::new(parsed.rule)
    }

    /// The fleet's first lumen rules, embedded so the binary behaves
    /// identically with no config file present. Identical content ships
    /// as `triggers.toml` in the crate for operator inspection.
    pub fn default_engine() -> Result<Engine, TriggerError> {
        Engine::from_toml(DEFAULT_RULES_TOML)
    }

    /// Declared rules, for the Fleet panel.
    pub fn rules(&self) -> &[Rule] {
        &self.rules
    }

    /// Enable or disable one rule by id (takes effect immediately).
    pub fn set_enabled(&mut self, rule_id: &str, enabled: bool) -> bool {
        match self.rules.iter_mut().find(|r| r.id == rule_id) {
            Some(rule) => {
                rule.enabled = enabled;
                true
            }
            None => false,
        }
    }

    /// Staged plan-mode proposals awaiting operator approval.
    pub fn proposals(&self) -> &VecDeque<Proposal> {
        &self.proposals
    }

    /// Remove and return the oldest proposal (operator approved it in
    /// the app, which owns the actual dispatch through its facade).
    pub fn pop_proposal(&mut self) -> Option<Proposal> {
        self.proposals.pop_front()
    }

    /// Discard the oldest proposal without dispatching.
    pub fn drop_proposal(&mut self) -> Option<Proposal> {
        self.proposals.pop_front()
    }

    /// Audit lines, oldest first.
    pub fn audit(&self) -> &VecDeque<String> {
        &self.audit
    }

    fn push_audit(&mut self, rule: &Rule, event: &Event, verdict: &str) {
        let line = json!({
            "rule": rule.id,
            "event": event.kind,
            "verdict": verdict,
            "origin_lane": rule.origin_lane,
            "content_label": rule.content_label,
        })
        .to_string();
        if self.audit.len() >= crate::AUDIT_LINES_MAX {
            self.audit.pop_front();
        }
        self.audit.push_back(line);
    }

    fn args_for(rule: &Rule, event: &Event) -> Value {
        let mut args = json!({
            "origin_lane": rule.origin_lane,
            "content_label": rule.content_label,
            "source_event": event.kind,
        });
        if let Some(field) = &rule.args_output_field
            && let Some(value) = event.fields.get(field)
        {
            args[field] = value.clone();
        }
        if let Some(summary) = event.fields.get("summary") {
            args["summary"] = summary.clone();
        }
        args
    }

    /// Evaluate one event against every rule. Returns one outcome per
    /// rule that matched far enough to decide; unmatched and disabled
    /// rules produce no outcome (but matches always audit).
    pub fn evaluate(&mut self, event: &Event, sink: &mut dyn TriggerSink) -> Vec<Outcome> {
        let mut outcomes = Vec::new();
        // Rules are cloned up front so audit/proposal mutation below
        // does not fight the borrow of the rule list.
        for rule in self.rules.clone() {
            if !rule.enabled || rule.event != event.kind {
                continue;
            }
            if !rule.conditions.iter().all(|c| c.matches(event)) {
                continue;
            }
            if event.depth > rule.max_depth.min(DEPTH_HARD_MAX) {
                self.push_audit(&rule, event, "dropped_depth");
                outcomes.push(Outcome::DroppedDepth {
                    rule_id: rule.id.clone(),
                });
                continue;
            }
            let key = idempotency_key(&rule.id, event);
            if !self.fired_keys.insert(key) {
                self.push_audit(&rule, event, "dropped_duplicate");
                outcomes.push(Outcome::DroppedDuplicate {
                    rule_id: rule.id.clone(),
                });
                continue;
            }
            if self.fired_keys.len() > FIRINGS_MAX * 4 {
                // Bound the set: history beyond this is an operator
                // restart concern, recorded in the audit trail.
                self.fired_keys.clear();
            }
            let args = Self::args_for(&rule, event);
            if rule.mode == RuleMode::Plan || rule.consequential {
                if self.proposals.len() >= FIRINGS_MAX {
                    self.proposals.pop_front();
                }
                self.proposals.push_back(Proposal {
                    rule_id: rule.id.clone(),
                    target_mcp: rule.target_mcp.clone(),
                    target_tool: rule.target_tool.clone(),
                    args,
                });
                self.push_audit(&rule, event, "proposed_plan");
                outcomes.push(Outcome::Proposed {
                    rule_id: rule.id.clone(),
                });
                continue;
            }
            match sink.dispatch(&rule.target_mcp, &rule.target_tool, args) {
                Ok(_) => {
                    self.push_audit(&rule, event, "fired_auto");
                    outcomes.push(Outcome::Fired {
                        rule_id: rule.id.clone(),
                    });
                }
                Err(error) => {
                    self.push_audit(&rule, event, "sink_failed");
                    outcomes.push(Outcome::SinkFailed {
                        rule_id: rule.id.clone(),
                        error,
                    });
                }
            }
        }
        outcomes
    }
}

fn validate_rule(rule: &Rule) -> Result<(), TriggerError> {
    if rule.origin_lane != ORIGIN_LANE || rule.content_label != CONTENT_LABEL {
        return Err(TriggerError::Invalid(format!(
            "rule {}: lane stamp {}/{} refused; this member emits {ORIGIN_LANE}/{CONTENT_LABEL}",
            rule.id, rule.origin_lane, rule.content_label
        )));
    }
    if rule.conditions.len() > CONDITIONS_MAX {
        return Err(TriggerError::Invalid(format!(
            "rule {}: {} conditions over bound {CONDITIONS_MAX}",
            rule.id,
            rule.conditions.len()
        )));
    }
    if rule.consequential && rule.mode == RuleMode::Auto {
        return Err(TriggerError::Invalid(format!(
            "rule {}: consequential targets may only fire in plan mode",
            rule.id
        )));
    }
    if rule.id.is_empty() || rule.target_tool.is_empty() || rule.event.is_empty() {
        return Err(TriggerError::Invalid(
            "rule with empty id, event, or target_tool".into(),
        ));
    }
    Ok(())
}

fn idempotency_key(rule_id: &str, event: &Event) -> String {
    let mut hasher = Sha256::new();
    hasher.update(rule_id.as_bytes());
    hasher.update(b"\n");
    hasher.update(event.canonical_json().as_bytes());
    format!("{:x}", hasher.finalize())
}

/// The default rule set: DESIGN §D4 worked example 2 for lumen.
pub const DEFAULT_RULES_TOML: &str = r#"
[[rule]]
id = "lumen-generation-hf-upload-prep"
origin_lane = "qompass"
content_label = "sfw"
event = "generation.completed"
mode = "plan"
consequential = true
target_mcp = "lumen"
target_tool = "hf_dataset_upload"
args_output_field = "output_path"
max_depth = 1

[[rule.conditions]]
field = "checks_passed"
op = "is_true"

[[rule]]
id = "lumen-generation-bloch-lesson"
origin_lane = "qompass"
content_label = "sfw"
event = "generation.completed"
mode = "auto"
consequential = false
target_mcp = "bloch"
target_tool = "lesson_candidate"
args_output_field = "output_path"
max_depth = 1

[[rule.conditions]]
field = "checks_passed"
op = "is_true"
"#;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::GENERATION_COMPLETED;

    fn engine() -> Engine {
        Engine::default_engine().expect("default rules valid")
    }

    #[test]
    fn generation_completed_fires_both_rules() {
        let mut eng = engine();
        let mut sink = FakeSink::default();
        let event = Event::generation_completed("/out/a.png", true);
        let outcomes = eng.evaluate(&event, &mut sink);
        assert_eq!(outcomes.len(), 2);
        // Plan rule staged, never dispatched.
        assert_eq!(eng.proposals().len(), 1);
        assert_eq!(eng.proposals()[0].target_tool, "hf_dataset_upload");
        // Auto rule dispatched exactly once, to bloch.
        assert_eq!(sink.dispatched.len(), 1);
        assert_eq!(sink.dispatched[0].0, "bloch");
        assert_eq!(sink.dispatched[0].1, "lesson_candidate");
        assert_eq!(sink.dispatched[0].2["origin_lane"], "qompass");
        assert_eq!(sink.dispatched[0].2["content_label"], "sfw");
        assert_eq!(sink.dispatched[0].2["output_path"], "/out/a.png");
    }

    #[test]
    fn unchecked_output_fires_nothing() {
        let mut eng = engine();
        let mut sink = FakeSink::default();
        let event = Event::generation_completed("/out/a.png", false);
        assert!(eng.evaluate(&event, &mut sink).is_empty());
        assert!(sink.dispatched.is_empty());
        assert!(eng.proposals().is_empty());
    }

    #[test]
    fn duplicate_event_is_dropped_by_idempotency() {
        let mut eng = engine();
        let mut sink = FakeSink::default();
        let event = Event::generation_completed("/out/a.png", true);
        let _ = eng.evaluate(&event, &mut sink);
        let second = eng.evaluate(&event, &mut sink);
        assert!(
            second
                .iter()
                .all(|o| matches!(o, Outcome::DroppedDuplicate { .. }))
        );
        assert_eq!(sink.dispatched.len(), 1);
    }

    #[test]
    fn depth_beyond_cap_is_dropped() {
        let mut eng = engine();
        let mut sink = FakeSink::default();
        let mut event = Event::generation_completed("/out/a.png", true);
        event.depth = 2;
        let outcomes = eng.evaluate(&event, &mut sink);
        assert!(
            outcomes
                .iter()
                .all(|o| matches!(o, Outcome::DroppedDepth { .. }))
        );
        assert!(sink.dispatched.is_empty());
    }

    #[test]
    fn wrong_lane_stamp_is_rejected_at_load() {
        let bad = DEFAULT_RULES_TOML.replace("qompass", "ntranch");
        assert!(Engine::from_toml(&bad).is_err());
        let bad_label = DEFAULT_RULES_TOML.replace("sfw", "nsfw");
        assert!(Engine::from_toml(&bad_label).is_err());
    }

    #[test]
    fn consequential_auto_rule_is_rejected_at_load() {
        let bad = r#"
[[rule]]
id = "bad-rule"
origin_lane = "qompass"
content_label = "sfw"
event = "generation.completed"
mode = "auto"
consequential = true
target_mcp = "lumen"
target_tool = "hf_dataset_upload"
"#;
        assert!(Engine::from_toml(bad).is_err());
    }

    #[test]
    fn sink_failure_is_an_outcome_not_a_panic() {
        let mut eng = engine();
        let mut sink = FakeSink {
            dispatched: Vec::new(),
            fail_with: Some("bloch unreachable".into()),
        };
        let event = Event::generation_completed("/out/a.png", true);
        let outcomes = eng.evaluate(&event, &mut sink);
        assert!(outcomes.iter().any(|o| matches!(
            o,
            Outcome::SinkFailed { error, .. } if error.contains("unreachable")
        )));
        // The plan proposal still staged independently.
        assert_eq!(eng.proposals().len(), 1);
    }

    #[test]
    fn disabled_rule_never_fires() {
        let mut eng = engine();
        assert!(eng.set_enabled("lumen-generation-bloch-lesson", false));
        let mut sink = FakeSink::default();
        let event = Event::generation_completed("/out/a.png", true);
        let _ = eng.evaluate(&event, &mut sink);
        assert!(sink.dispatched.is_empty());
    }

    #[test]
    fn audit_lines_carry_lane_stamps() {
        let mut eng = engine();
        let mut sink = FakeSink::default();
        let _ = eng.evaluate(&Event::generation_completed("/o.png", true), &mut sink);
        assert!(!eng.audit().is_empty());
        for line in eng.audit() {
            assert!(line.contains("\"origin_lane\":\"qompass\""), "{line}");
            assert!(line.contains("\"content_label\":\"sfw\""), "{line}");
        }
        let _ = GENERATION_COMPLETED;
    }

    #[test]
    fn malformed_toml_is_a_typed_error() {
        assert!(matches!(
            Engine::from_toml("[[rule]]\nid = 42\n"),
            Err(TriggerError::Invalid(_))
        ));
    }
}
