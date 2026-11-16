//! Typed events on the TUI's own event stream.
//!
//! The same stream feeds the Log screen and the trigger engine (DESIGN
//! §D4). An event is a kind string plus named fields; trigger predicates
//! may only read these named fields — there is no expression language.

use std::collections::BTreeMap;

use serde_json::Value;

/// Event kind emitted when a generation job finishes and its output
/// passed lumen's own checks (the tool result carries an output path
/// and no error).
pub const GENERATION_COMPLETED: &str = "generation.completed";
/// Event kind emitted when any other workflow finishes successfully.
pub const WORKFLOW_COMPLETED: &str = "workflow.completed";
/// Event kind emitted when a workflow fails or is refused.
pub const WORKFLOW_FAILED: &str = "workflow.failed";

/// One typed event. `depth` counts trigger-chain hops: events produced
/// by a triggered call carry the firing depth so rules can cap chains.
#[derive(Debug, Clone, PartialEq)]
pub struct Event {
    /// Dotted event kind, e.g. `generation.completed`.
    pub kind: String,
    /// Named fields predicates may inspect. Sorted keys (BTreeMap) keep
    /// the idempotency hash deterministic.
    pub fields: BTreeMap<String, Value>,
    /// Trigger depth of the call that produced this event (0 = operator).
    pub depth: u32,
}

impl Event {
    /// A fresh operator-depth event with no fields.
    pub fn new(kind: &str) -> Event {
        Event {
            kind: kind.to_string(),
            fields: BTreeMap::new(),
            depth: 0,
        }
    }

    /// Builder: set one named field.
    pub fn with_field(mut self, name: &str, value: Value) -> Event {
        self.fields.insert(name.to_string(), value);
        self
    }

    /// A `generation.completed` event for `output_path`.
    pub fn generation_completed(output_path: &str, checks_passed: bool) -> Event {
        Event::new(GENERATION_COMPLETED)
            .with_field("output_path", Value::String(output_path.to_string()))
            .with_field("checks_passed", Value::Bool(checks_passed))
    }

    /// Canonical JSON used for idempotency hashing: kind + sorted fields.
    pub fn canonical_json(&self) -> String {
        serde_json::json!({"kind": self.kind, "fields": self.fields}).to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_json_is_field_order_independent() {
        let a = Event::new("x")
            .with_field("b", Value::from(2))
            .with_field("a", Value::from(1));
        let b = Event::new("x")
            .with_field("a", Value::from(1))
            .with_field("b", Value::from(2));
        assert_eq!(a.canonical_json(), b.canonical_json());
    }

    #[test]
    fn generation_completed_carries_named_fields() {
        let e = Event::generation_completed("/tmp/out.png", true);
        assert_eq!(e.kind, GENERATION_COMPLETED);
        assert_eq!(e.fields["checks_passed"], Value::Bool(true));
        assert_eq!(e.depth, 0);
    }
}
