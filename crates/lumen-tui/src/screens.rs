//! Screen content models: pure functions from app state to text lines.
//!
//! Both the full-screen renderer (via [`crate::panels`]) and the line
//! mode print these lines, so the two modes can never diverge in what
//! they claim. Workflows are declared here as data.

use serde_json::{Value, json};

/// Which screen the state machine is showing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Screen {
    /// Member/version/lane/factory summary.
    Overview,
    /// Workflow list + run controls.
    Workflows,
    /// Bounded event log.
    Log,
    /// Fleet members, trigger rules, audit tail.
    Fleet,
    /// Media ladder browser + viewer.
    Media,
}

impl Screen {
    /// All screens in tab order.
    pub const ALL: [Screen; 5] = [
        Screen::Overview,
        Screen::Workflows,
        Screen::Log,
        Screen::Fleet,
        Screen::Media,
    ];

    /// Display name.
    pub fn name(self) -> &'static str {
        match self {
            Screen::Overview => "Overview",
            Screen::Workflows => "Workflows",
            Screen::Log => "Log/Live",
            Screen::Fleet => "Fleet",
            Screen::Media => "Media",
        }
    }

    /// Next screen in tab order (wraps).
    pub fn next(self) -> Screen {
        let idx = Screen::ALL.iter().position(|s| *s == self).unwrap_or(0);
        Screen::ALL[(idx + 1) % Screen::ALL.len()]
    }
}

/// Run mode: plan stages a call for approval; auto executes directly.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Stage the exact tool + args; nothing calls before approval.
    Plan,
    /// Execute declared non-consequential operations directly.
    Auto,
}

impl Mode {
    /// Display name.
    pub fn name(self) -> &'static str {
        match self {
            Mode::Plan => "plan",
            Mode::Auto => "auto",
        }
    }
}

/// Whether a workflow routes model calls (provider control honesty).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderRoute {
    /// Generation lane: lumen routes model calls to its local backend.
    GenerationLocal,
    /// Deterministic conversion/inspection: no model call exists, so
    /// no provider control is offered — the UI says why.
    Deterministic,
}

impl ProviderRoute {
    /// The provider line shown in the Workflows screen.
    pub fn ui_line(self) -> &'static str {
        match self {
            ProviderRoute::GenerationLocal => {
                "provider: local (generation lane backend; model chosen per job)"
            }
            ProviderRoute::Deterministic => {
                "provider: n/a — deterministic conversion, no model call to route"
            }
        }
    }
}

/// One declared workflow: a named operation over one MCP tool.
#[derive(Debug, Clone, PartialEq)]
pub struct Workflow {
    /// Stable id used on the command line (`run <id>`).
    pub id: &'static str,
    /// Display title.
    pub title: &'static str,
    /// MCP tool the run calls.
    pub tool: &'static str,
    /// Provider routing honesty for this workflow.
    pub route: ProviderRoute,
    /// Consequential tools (writes/uploads) are plan-only, always.
    pub consequential: bool,
    /// True for generation jobs (completion emits `generation.completed`).
    pub generation: bool,
    /// Argument template as JSON text (parsed per run; `Value` is
    /// not const-constructible, and this table is a constant).
    pub args_template_json: &'static str,
}

impl Workflow {
    /// The argument template as a value; invalid stored JSON (a bug in
    /// this table, guarded by test) degrades to an empty object.
    pub fn args_template(&self) -> Value {
        serde_json::from_str(self.args_template_json).unwrap_or_else(|_| json!({}))
    }
}

/// The declared workflow set. Nothing runs that is not listed here.
pub const WORKFLOWS: &[Workflow] = &[
    Workflow {
        id: "status",
        title: "Generation lane status (incl. atelier tools)",
        tool: "lumen_status",
        route: ProviderRoute::GenerationLocal,
        consequential: false,
        generation: false,
        args_template_json: "{}",
    },
    Workflow {
        id: "models",
        title: "List registered generation models",
        tool: "lumen_models",
        route: ProviderRoute::GenerationLocal,
        consequential: false,
        generation: false,
        args_template_json: "{}",
    },
    Workflow {
        id: "sprite-info",
        title: "Inspect a sprite PNG (path arg)",
        tool: "sprite_info",
        route: ProviderRoute::Deterministic,
        consequential: false,
        generation: false,
        args_template_json: "{}",
    },
    Workflow {
        id: "validate-atlas",
        title: "Validate a PNG against the atlas contract (path arg)",
        tool: "validate_atlas",
        route: ProviderRoute::Deterministic,
        consequential: false,
        generation: false,
        args_template_json: "{}",
    },
    Workflow {
        id: "txt2img",
        title: "Generate one image from a prompt (prompt arg)",
        tool: "lumen_txt2img",
        route: ProviderRoute::GenerationLocal,
        consequential: false,
        generation: true,
        args_template_json: "{}",
    },
    Workflow {
        id: "img2img",
        title: "Generate one image from a source PNG (JSON args)",
        tool: "lumen_img2img",
        route: ProviderRoute::GenerationLocal,
        consequential: false,
        generation: true,
        args_template_json: "{}",
    },
    Workflow {
        id: "canon-resolve",
        title: "Resolve a canon handle (JSON args)",
        tool: "canon_resolve",
        route: ProviderRoute::Deterministic,
        consequential: false,
        generation: false,
        args_template_json: "{}",
    },
    Workflow {
        id: "canon-convert",
        title: "Convert a PNG under a canon record (JSON args)",
        tool: "canon_convert",
        route: ProviderRoute::Deterministic,
        consequential: true,
        generation: true,
        args_template_json: "{}",
    },
    Workflow {
        id: "hf-upload",
        title: "Upload generated media to a HF dataset (JSON args)",
        tool: "hf_dataset_upload",
        route: ProviderRoute::Deterministic,
        consequential: true,
        generation: false,
        args_template_json: "{\"dry_run\": false}",
    },
];

/// Look up a workflow by id.
pub fn workflow_by_id(id: &str) -> Option<&'static Workflow> {
    WORKFLOWS.iter().find(|w| w.id == id)
}

/// A staged call: the exact tool + args awaiting operator approval.
#[derive(Debug, Clone, PartialEq)]
pub struct StagedCall {
    /// Workflow id that staged this call.
    pub workflow_id: String,
    /// MCP tool to call on approval.
    pub tool: String,
    /// Exact arguments to send on approval.
    pub args: Value,
    /// True when this came from a trigger proposal rather than the
    /// operator's own workflow selection.
    pub from_trigger: bool,
}

impl StagedCall {
    /// One-line rendering of the staged call for approval prompts.
    pub fn describe(&self) -> String {
        format!(
            "{} tool={} args={}",
            if self.from_trigger {
                "proposal"
            } else {
                "staged"
            },
            self.tool,
            self.args
        )
    }
}

/// Merge caller JSON over a workflow's argument template. A non-object
/// caller value is refused (typed as a string the app renders).
pub fn merge_args(template: &Value, caller: &Value) -> Result<Value, String> {
    if caller.is_null() {
        return Ok(template.clone());
    }
    let Value::Object(caller_map) = caller else {
        return Err("workflow args must be a JSON object".to_string());
    };
    let mut merged = template.clone();
    if !merged.is_object() {
        merged = json!({});
    }
    for (key, value) in caller_map {
        merged[key] = value.clone();
    }
    Ok(merged)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn screen_tab_order_wraps() {
        assert_eq!(Screen::Media.next(), Screen::Overview);
        assert_eq!(Screen::Overview.next(), Screen::Workflows);
    }

    #[test]
    fn every_workflow_has_a_tool_and_unique_id() {
        for (i, w) in WORKFLOWS.iter().enumerate() {
            assert!(!w.tool.is_empty());
            assert!(WORKFLOWS[..i].iter().all(|o| o.id != w.id));
        }
    }

    #[test]
    fn every_workflow_template_parses_to_an_object() {
        for w in WORKFLOWS {
            let parsed: Value =
                serde_json::from_str(w.args_template_json).expect("template parses");
            assert!(parsed.is_object(), "{} template not an object", w.id);
            assert_eq!(w.args_template(), parsed);
        }
    }

    #[test]
    fn deterministic_workflows_say_why_no_provider() {
        assert!(
            ProviderRoute::Deterministic
                .ui_line()
                .contains("deterministic")
        );
        assert!(ProviderRoute::GenerationLocal.ui_line().contains("local"));
    }

    #[test]
    fn merge_args_rejects_non_objects() {
        assert!(merge_args(&json!({}), &json!([1, 2])).is_err());
        assert!(merge_args(&json!({}), &json!("text")).is_err());
    }

    #[test]
    fn merge_args_overlays_caller_keys() {
        let merged = merge_args(&json!({"a": 1, "b": 2}), &json!({"b": 3})).expect("merge");
        assert_eq!(merged, json!({"a": 1, "b": 3}));
    }
}
