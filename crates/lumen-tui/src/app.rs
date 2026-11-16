//! App state machine, workflow execution, line mode, full-screen loop.
//!
//! One execute path serves both modes (DESIGN §D2): a workflow run is
//! staged in plan mode (or whenever the tool is consequential) and
//! only an explicit `approve` dispatches it. Auto mode executes
//! non-consequential workflows directly. Completed generation jobs
//! emit `generation.completed` into the trigger engine.

use std::collections::VecDeque;
use std::io::{BufRead, Write};
use std::path::PathBuf;

use serde_json::{Value, json};

use crate::events::Event;
use crate::facade::LumenFacade;
use crate::media::{self, Capabilities, DisplayPlan};
use crate::screens::{Mode, Screen, StagedCall, WORKFLOWS, merge_args, workflow_by_id};
use crate::triggers::{Engine, TriggerSink};
use crate::{INPUT_LINE_BYTES_MAX, LOG_LINES_MAX, TOOL_RESULT_CHARS_MAX, truncate_chars};

/// The whole TUI state. Invalid states are unrepresentable: a staged
/// call exists only between staging and approve/abort.
pub struct App<F: LumenFacade, S: TriggerSink> {
    facade: F,
    sink: S,
    engine: Engine,
    screen: Screen,
    mode: Mode,
    selected_workflow: usize,
    staged: Option<StagedCall>,
    log: VecDeque<String>,
    media_dir: Option<PathBuf>,
    caps: Capabilities,
    last_display: Option<DisplayPlan>,
}

impl<F: LumenFacade, S: TriggerSink> App<F, S> {
    /// New app in plan mode on the Overview screen (safe defaults:
    /// nothing can fire before the operator acts).
    pub fn new(facade: F, sink: S, engine: Engine) -> App<F, S> {
        App {
            facade,
            sink,
            engine,
            screen: Screen::Overview,
            mode: Mode::Plan,
            selected_workflow: 0,
            staged: None,
            log: VecDeque::new(),
            media_dir: None,
            caps: media::capabilities(),
            last_display: None,
        }
    }

    /// Current screen.
    pub fn screen(&self) -> Screen {
        self.screen
    }

    /// Switch screens.
    pub fn set_screen(&mut self, screen: Screen) {
        self.screen = screen;
    }

    /// Current run mode.
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Switch run mode.
    pub fn set_mode(&mut self, mode: Mode) {
        self.mode = mode;
    }

    /// The staged call awaiting approval, if any.
    pub fn staged(&self) -> Option<&StagedCall> {
        self.staged.as_ref()
    }

    /// The facade (tests inspect recorded calls through this).
    pub fn facade(&self) -> &F {
        &self.facade
    }

    /// The trigger sink (tests inspect dispatches through this).
    pub fn sink(&self) -> &S {
        &self.sink
    }

    /// The trigger engine (Fleet panel + tests).
    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// Set the media browser directory.
    pub fn set_media_dir(&mut self, dir: PathBuf) {
        self.media_dir = Some(dir);
    }

    /// Override capability detection (tests: no graphics terminal).
    pub fn set_caps(&mut self, caps: Capabilities) {
        self.caps = caps;
    }

    /// The most recent media display plan, if any.
    pub fn last_display(&self) -> Option<&DisplayPlan> {
        self.last_display.as_ref()
    }

    fn log_push(&mut self, line: String) {
        if self.log.len() >= LOG_LINES_MAX {
            self.log.pop_front();
        }
        self.log.push_back(line);
    }

    // -- Screen content -------------------------------------------------

    /// Content lines for the active screen.
    pub fn screen_lines(&mut self) -> Vec<String> {
        match self.screen {
            Screen::Overview => self.overview_lines(),
            Screen::Workflows => self.workflows_lines(),
            Screen::Log => self.log.iter().cloned().collect(),
            Screen::Fleet => self.fleet_lines(),
            Screen::Media => self.media_lines(),
        }
    }

    fn overview_lines(&mut self) -> Vec<String> {
        let mut lines = vec![format!(
            "member: lumen {} (TUI {})",
            crate::MEMBER_VERSION,
            crate::TUI_VERSION
        )];
        match self.facade.status() {
            Ok(status) => {
                lines.push(format!(
                    "status: {}",
                    truncate_chars(&status.to_string(), TOOL_RESULT_CHARS_MAX)
                ));
                if let Some(lane) = status.get("generation") {
                    lines.push(format!("generation lane (atelier tools folded in): {lane}"));
                }
            }
            Err(e) => lines.push(format!("status: unavailable — {e}")),
        }
        match self.facade.factory_status() {
            Ok(factory) => lines.push(format!("factory: {factory}")),
            Err(e) => lines.push(format!("factory: unavailable — {e}")),
        }
        match self.facade.tools() {
            Ok(tools) => lines.push(format!("tools: {} registered", tools.len())),
            Err(e) => lines.push(format!("tools: unavailable — {e}")),
        }
        lines.push(format!(
            "trigger rules: {} declared (Fleet screen)",
            self.engine.rules().len()
        ));
        lines.push(format!(
            "mode: {} — plan stages, approve dispatches",
            self.mode.name()
        ));
        lines
    }

    fn workflows_lines(&self) -> Vec<String> {
        let mut lines = vec![format!(
            "run controls: mode={} (plan stages exact tool+args; approve dispatches)",
            self.mode.name()
        )];
        for (i, w) in WORKFLOWS.iter().enumerate() {
            let marker = if i == self.selected_workflow {
                ">"
            } else {
                " "
            };
            let gate = if w.consequential {
                " [plan-only: consequential]"
            } else {
                ""
            };
            lines.push(format!("{marker} {:<14} {}{}", w.id, w.title, gate));
            lines.push(format!("    {}", w.route.ui_line()));
        }
        if let Some(staged) = &self.staged {
            lines.push(format!("awaiting approval: {}", staged.describe()));
            lines.push("approve with `approve`, discard with `abort`".to_string());
        }
        lines
    }

    fn fleet_lines(&self) -> Vec<String> {
        let mut lines = vec![
            "fleet: lumen (this member) — trigger targets reached as MCP clients".to_string(),
            "rules (visible = existent; nothing fires that is not listed):".to_string(),
        ];
        for rule in self.engine.rules() {
            lines.push(format!(
                "  {} [{}] on {} -> {}.{} mode={:?} lane={}/{}",
                rule.id,
                if rule.enabled { "on" } else { "off" },
                rule.event,
                rule.target_mcp,
                rule.target_tool,
                rule.mode,
                rule.origin_lane,
                rule.content_label
            ));
        }
        lines.push("audit tail:".to_string());
        for line in self.engine.audit().iter().rev().take(20) {
            lines.push(format!("  {line}"));
        }
        lines
    }

    fn media_lines(&self) -> Vec<String> {
        let mut lines = vec![
            format!(
                "media ladder rung 1: kitty={}, sixel={:?} (follow-on rung), video={:?} (follow-on rung), half-block={:?}",
                self.caps.kitty, self.caps.sixel, self.caps.video, self.caps.half_block
            ),
            media::display_label(media::choose_protocol(&self.caps)),
        ];
        match &self.media_dir {
            None => lines.push("no media directory set (pass --media <dir>)".to_string()),
            Some(dir) => match media::list_media(dir) {
                Ok(entries) => {
                    lines.push(format!("{} image(s) in {}", entries.len(), dir.display()));
                    for entry in entries.iter().take(50) {
                        lines.push(format!("  {} ({} bytes)", entry.name, entry.bytes));
                    }
                }
                Err(e) => lines.push(format!("media list failed: {e}")),
            },
        }
        if let Some(plan) = &self.last_display {
            lines.push(format!("displaying: {}", plan.label));
        }
        lines
    }

    // -- Workflow execution ----------------------------------------------

    /// Run one workflow by id with caller JSON args (or a `path` /
    /// `prompt` shorthand object). Plan mode — and consequential tools
    /// in any mode — stage instead of calling.
    pub fn run_workflow(&mut self, id: &str, caller_args: &Value) -> Vec<String> {
        let Some(workflow) = workflow_by_id(id) else {
            return vec![format!("unknown workflow: {id}")];
        };
        let args = match merge_args(&workflow.args_template(), caller_args) {
            Ok(args) => args,
            Err(e) => return vec![format!("refused: {e}")],
        };
        let staged = StagedCall {
            workflow_id: workflow.id.to_string(),
            tool: workflow.tool.to_string(),
            args,
            from_trigger: false,
        };
        if self.mode == Mode::Plan || workflow.consequential {
            let note = if workflow.consequential && self.mode == Mode::Auto {
                "consequential tool: auto mode refused, staged for approval instead"
            } else {
                "staged (plan mode): no call made"
            };
            self.log_push(format!("{note}: {}", staged.describe()));
            self.staged = Some(staged.clone());
            return vec![note.to_string(), staged.describe()];
        }
        self.execute(staged)
    }

    /// Approve the staged call or the oldest trigger proposal and
    /// dispatch it. Returns what happened, for display.
    pub fn approve(&mut self) -> Vec<String> {
        if let Some(staged) = self.staged.take() {
            return self.execute(staged);
        }
        if let Some(proposal) = self.engine.pop_proposal() {
            let staged = StagedCall {
                workflow_id: proposal.rule_id.clone(),
                tool: proposal.target_tool.clone(),
                args: proposal.args.clone(),
                from_trigger: true,
            };
            if proposal.target_mcp == crate::MEMBER_NAME {
                return self.execute(staged);
            }
            return match self.sink.dispatch(
                &proposal.target_mcp,
                &proposal.target_tool,
                proposal.args,
            ) {
                Ok(value) => {
                    let line = format!(
                        "proposal dispatched: {} -> {}",
                        proposal.rule_id,
                        truncate_chars(&value.to_string(), TOOL_RESULT_CHARS_MAX)
                    );
                    self.log_push(line.clone());
                    vec![line]
                }
                Err(e) => {
                    let line = format!("proposal dispatch failed: {e}");
                    self.log_push(line.clone());
                    vec![line]
                }
            };
        }
        vec!["nothing staged to approve".to_string()]
    }

    /// Abort: discard the staged call / oldest proposal, dispatching
    /// nothing.
    pub fn abort(&mut self) -> Vec<String> {
        if let Some(staged) = self.staged.take() {
            let line = format!("aborted without calling: {}", staged.describe());
            self.log_push(line.clone());
            return vec![line];
        }
        if let Some(proposal) = self.engine.drop_proposal() {
            return vec![format!("proposal discarded: {}", proposal.rule_id)];
        }
        vec!["nothing staged to abort".to_string()]
    }

    fn execute(&mut self, staged: StagedCall) -> Vec<String> {
        let workflow = workflow_by_id(&staged.workflow_id);
        self.log_push(format!("started: {} {}", staged.tool, staged.args));
        match self.facade.call(&staged.tool, staged.args.clone()) {
            Ok(result) => {
                let rendered = truncate_chars(&result.to_string(), TOOL_RESULT_CHARS_MAX);
                self.log_push(format!("done: {} -> {}", staged.tool, rendered));
                if let Some(w) = workflow
                    && w.generation
                {
                    self.after_generation(&result);
                }
                vec![format!("done: {rendered}")]
            }
            Err(e) => {
                self.log_push(format!("failed: {} — {e}", staged.tool));
                vec![format!("failed: {e}")]
            }
        }
    }

    /// Emit `generation.completed` when the result carries an output
    /// and no error, then run the trigger engine over it.
    fn after_generation(&mut self, result: &Value) {
        if result.get("error").is_some() {
            self.log_push("generation result carried an error: no event emitted".into());
            return;
        }
        let output = result
            .get("output_path")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                result
                    .pointer("/output/path")
                    .and_then(Value::as_str)
                    .map(str::to_string)
            });
        let Some(output_path) = output else {
            self.log_push("generation result had no output path: no event emitted".into());
            return;
        };
        let event = Event::generation_completed(&output_path, true).with_field(
            "summary",
            Value::String(truncate_chars(&result.to_string(), 500)),
        );
        self.log_push(format!("event: generation.completed output={output_path}"));
        let outcomes = self.engine.evaluate(&event, &mut self.sink);
        for outcome in &outcomes {
            self.log_push(format!("trigger: {outcome:?}"));
        }
    }

    // -- Media ------------------------------------------------------------

    /// Display one image from the media directory by file name.
    pub fn show_media(&mut self, name: &str) -> Vec<String> {
        let Some(dir) = self.media_dir.clone() else {
            return vec!["no media directory set".to_string()];
        };
        if name.contains('/') || name.contains('\\') || name.contains("..") {
            return vec![format!(
                "refused: media name must be a plain file name: {name}"
            )];
        }
        let path = dir.join(name);
        let bytes = match media::read_image_file(&path) {
            Ok(bytes) => bytes,
            Err(e) => return vec![format!("media read failed: {e}")],
        };
        match media::plan_display(&bytes, &self.caps, 80, 24) {
            Ok(plan) => {
                let lines = vec![plan.label.clone(), plan.payload.clone()];
                self.last_display = Some(plan);
                lines
            }
            Err(e) => vec![format!("media render failed: {e}")],
        }
    }

    // -- Line mode ----------------------------------------------------------

    /// Handle one line-mode command; returns output lines and whether
    /// the session should end. Shares the execute path with the
    /// full-screen loop.
    pub fn handle_command(&mut self, line: &str) -> (Vec<String>, bool) {
        let line = line.trim();
        if line.len() > INPUT_LINE_BYTES_MAX {
            return (vec!["refused: input line over bound".to_string()], false);
        }
        let mut parts = line.splitn(3, ' ');
        let cmd = parts.next().unwrap_or("");
        let rest = parts.next().unwrap_or("");
        let tail = parts.next().unwrap_or("");
        match cmd {
            "" => (Vec::new(), false),
            "quit" | "exit" => (vec!["bye".to_string()], true),
            "help" => (help_lines(), false),
            "overview" => {
                self.screen = Screen::Overview;
                (self.screen_lines(), false)
            }
            "workflows" => {
                self.screen = Screen::Workflows;
                (self.screen_lines(), false)
            }
            "log" => {
                self.screen = Screen::Log;
                (self.screen_lines(), false)
            }
            "fleet" => {
                self.screen = Screen::Fleet;
                (self.screen_lines(), false)
            }
            "media" if rest.is_empty() => {
                self.screen = Screen::Media;
                (self.screen_lines(), false)
            }
            "media" if rest == "show" => (self.show_media(tail), false),
            "media" => (vec![format!("unknown media command: {rest}")], false),
            "caps" => (vec![format!("{:?}", self.caps)], false),
            "mode" => {
                match rest {
                    "plan" => self.mode = Mode::Plan,
                    "auto" => self.mode = Mode::Auto,
                    _ => return (vec!["usage: mode plan|auto".to_string()], false),
                }
                (vec![format!("mode: {}", self.mode.name())], false)
            }
            "run" => {
                let args = parse_run_args(rest, tail);
                match args {
                    Ok((id, value)) => (self.run_workflow(&id, &value), false),
                    Err(e) => (vec![format!("refused: {e}")], false),
                }
            }
            "approve" => (self.approve(), false),
            "abort" => (self.abort(), false),
            "triggers" => (self.fleet_lines(), false),
            _ => (vec![format!("unknown command: {cmd} (try `help`)")], false),
        }
    }
}

/// Parse `run` arguments: `<id>` plus either nothing, a JSON object,
/// or one bare word taken as `path` (sprite tools) / `prompt` (txt2img).
fn parse_run_args(id: &str, tail: &str) -> Result<(String, Value), String> {
    if id.is_empty() {
        return Err("usage: run <workflow-id> [json-args|word]".to_string());
    }
    if tail.is_empty() {
        return Ok((id.to_string(), Value::Null));
    }
    if tail.starts_with('{') {
        let parsed: Value =
            serde_json::from_str(tail).map_err(|e| format!("args are not valid JSON: {e}"))?;
        return Ok((id.to_string(), parsed));
    }
    let key = match id {
        "txt2img" => "prompt",
        _ => "path",
    };
    Ok((id.to_string(), json!({key: tail})))
}

fn help_lines() -> Vec<String> {
    vec![
        "commands: overview | workflows | log | fleet | media | media show <name> | caps",
        "          mode plan|auto | run <id> [json|word] | approve | abort | triggers | quit",
        "plan mode stages the exact tool+args; nothing calls before `approve`.",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

/// Line mode: read commands from `reader`, write replies to `writer`.
/// Ends on `quit` or EOF. This is the scriptable surface (DESIGN §D2).
pub fn run_line_mode<F: LumenFacade, S: TriggerSink>(
    app: &mut App<F, S>,
    reader: &mut dyn BufRead,
    writer: &mut dyn Write,
) -> std::io::Result<()> {
    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            return Ok(());
        }
        let (out, quit) = app.handle_command(&line);
        for text in out {
            writeln!(writer, "{text}")?;
        }
        writer.flush()?;
        if quit {
            return Ok(());
        }
    }
}

/// Full-screen ratatui loop. Keys: `q` quit, Tab/1-5 screens,
/// `m` toggle plan/auto, Enter run selected workflow, `a` approve,
/// `x` abort, Up/Down move the workflow selection.
pub fn run_fullscreen<F: LumenFacade, S: TriggerSink>(app: &mut App<F, S>) -> std::io::Result<()> {
    use crossterm::event::read;
    use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
    use ratatui::Terminal;
    use ratatui::backend::CrosstermBackend;

    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    crossterm::execute!(stdout, crossterm::terminal::EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;
    let result = fullscreen_loop(app, &mut terminal, &mut |()| read());
    let _ = disable_raw_mode();
    let _ = crossterm::execute!(
        terminal.backend_mut(),
        crossterm::terminal::LeaveAlternateScreen
    );
    let _ = terminal.show_cursor();
    result
}

fn fullscreen_loop<F: LumenFacade, S: TriggerSink>(
    app: &mut App<F, S>,
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    next_event: &mut dyn FnMut(()) -> std::io::Result<crossterm::event::Event>,
) -> std::io::Result<()> {
    use crossterm::event::KeyCode;
    loop {
        let lines = app.screen_lines();
        let header = crate::panels::header_lines(app.screen, app.mode.name());
        terminal.draw(|frame| {
            let area = frame.area();
            let header_area = ratatui::layout::Rect { height: 3, ..area };
            frame.render_widget(crate::panels::text_panel("lumen-tui", &header), header_area);
            frame.render_widget(
                crate::panels::text_panel(app.screen.name(), &lines),
                crate::panels::panel_area(area),
            );
        })?;
        if let crossterm::event::Event::Key(key) = next_event(())? {
            match key.code {
                KeyCode::Char('q') => return Ok(()),
                KeyCode::Tab => app.set_screen(app.screen.next()),
                KeyCode::Char(c @ '1'..='5') => {
                    let idx = (c as usize) - ('1' as usize);
                    app.set_screen(Screen::ALL[idx]);
                }
                KeyCode::Char('m') => {
                    app.set_mode(match app.mode {
                        Mode::Plan => Mode::Auto,
                        Mode::Auto => Mode::Plan,
                    });
                }
                KeyCode::Char('a') => {
                    let _ = app.approve();
                }
                KeyCode::Char('x') => {
                    let _ = app.abort();
                }
                KeyCode::Down => {
                    app.selected_workflow = (app.selected_workflow + 1) % WORKFLOWS.len();
                }
                KeyCode::Up => {
                    app.selected_workflow = app
                        .selected_workflow
                        .checked_sub(1)
                        .unwrap_or(WORKFLOWS.len() - 1);
                }
                KeyCode::Enter => {
                    let id = WORKFLOWS[app.selected_workflow].id;
                    let _ = app.run_workflow(id, &Value::Null);
                }
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::facade::FakeFacade;
    use crate::media::detect_capabilities;
    use crate::triggers::{Engine, FakeSink};
    use serde_json::json;

    fn test_app() -> App<FakeFacade, FakeSink> {
        let mut app = App::new(
            FakeFacade::new(),
            FakeSink::default(),
            Engine::default_engine().expect("engine"),
        );
        app.set_caps(detect_capabilities(Some("xterm-256color"), None));
        app
    }

    #[test]
    fn plan_mode_makes_no_call_before_approval() {
        let mut app = test_app();
        let out = app.run_workflow("status", &Value::Null);
        assert!(out[0].contains("staged"));
        assert!(app.facade().calls.is_empty(), "plan mode must not call");
        assert!(app.staged().is_some());
        let done = app.approve();
        assert!(done[0].contains("done"));
        assert_eq!(app.facade().calls.len(), 1);
        assert_eq!(app.facade().calls[0].0, "lumen_status");
    }

    #[test]
    fn approval_then_abort_dispatches_nothing() {
        let mut app = test_app();
        let _ = app.run_workflow("status", &Value::Null);
        let out = app.abort();
        assert!(out[0].contains("aborted"));
        assert!(app.facade().calls.is_empty());
        // A later approve has nothing to dispatch.
        let out = app.approve();
        assert!(out[0].contains("nothing staged"));
        assert!(app.facade().calls.is_empty());
    }

    #[test]
    fn auto_mode_executes_non_consequential_directly() {
        let mut app = test_app();
        app.set_mode(Mode::Auto);
        let out = app.run_workflow("status", &Value::Null);
        assert!(out[0].contains("done"));
        assert_eq!(app.facade().calls.len(), 1);
    }

    #[test]
    fn consequential_tool_is_plan_only_even_in_auto() {
        let mut app = test_app();
        app.set_mode(Mode::Auto);
        let out = app.run_workflow("hf-upload", &json!({"manifest_path": "m.json"}));
        assert!(out[0].contains("consequential"));
        assert!(app.facade().calls.is_empty());
        let _ = app.approve();
        assert_eq!(app.facade().calls.len(), 1);
        assert_eq!(app.facade().calls[0].0, "hf_dataset_upload");
    }

    #[test]
    fn malformed_tool_result_renders_as_failure_not_panic() {
        let mut app = test_app();
        app.set_mode(Mode::Auto);
        app.facade.queue_call(Ok(json!({"weird": true})));
        // An object is well-formed; queue a bare array next via a fresh app.
        let out = app.run_workflow("status", &Value::Null);
        assert!(out[0].contains("done"));
        let mut app2 = test_app();
        app2.set_mode(Mode::Auto);
        // FakeFacade validates: a JSON string result is malformed.
        app2.facade
            .queue_call(Ok(Value::String("garbage".to_string())));
        let out = app2.run_workflow("status", &Value::Null);
        assert!(out[0].contains("failed"));
    }

    #[test]
    fn generation_completion_fires_triggers() {
        let mut app = test_app();
        app.set_mode(Mode::Auto);
        app.facade.queue_call(Ok(json!({
            "output": {"path": "/out/gen.png"},
            "sha256": "abc"
        })));
        let _ = app.run_workflow("txt2img", &json!({"prompt": "a sprite"}));
        // bloch lesson auto-fired through the sink.
        assert_eq!(app.sink().dispatched.len(), 1);
        assert_eq!(app.sink().dispatched[0].1, "lesson_candidate");
        // HF upload staged as a plan proposal, not dispatched.
        assert_eq!(app.engine().proposals().len(), 1);
        assert_eq!(app.facade().calls.len(), 1, "only the txt2img call itself");
    }

    #[test]
    fn generation_without_output_emits_no_event() {
        let mut app = test_app();
        app.set_mode(Mode::Auto);
        app.facade.queue_call(Ok(json!({"status": "ok"})));
        let _ = app.run_workflow("txt2img", &json!({"prompt": "x"}));
        assert!(app.sink().dispatched.is_empty());
        assert!(app.engine().proposals().is_empty());
    }

    #[test]
    fn line_mode_answers_one_command_from_a_pipe() {
        let mut app = test_app();
        let mut input: &[u8] = b"workflows\nquit\n";
        let mut out: Vec<u8> = Vec::new();
        run_line_mode(&mut app, &mut input, &mut out).expect("line mode");
        let text = String::from_utf8(out).expect("utf8");
        assert!(text.contains("txt2img"));
        assert!(text.contains("bye"));
    }

    #[test]
    fn line_mode_run_stages_then_approve_dispatches() {
        let mut app = test_app();
        let (out, _) = app.handle_command("run sprite-info hero.png");
        assert!(out[0].contains("staged"));
        assert!(app.facade().calls.is_empty());
        let _ = app.handle_command("approve");
        assert_eq!(app.facade().calls.len(), 1);
        assert_eq!(app.facade().calls[0].1["path"], "hero.png");
    }

    #[test]
    fn unknown_workflow_and_bad_json_are_refusals() {
        let mut app = test_app();
        let out = app.run_workflow("nope", &Value::Null);
        assert!(out[0].contains("unknown workflow"));
        let (out, _) = app.handle_command("run status {bad json");
        assert!(out[0].contains("refused"));
        assert!(app.facade().calls.is_empty());
    }

    #[test]
    fn media_show_rejects_path_traversal_names() {
        let mut app = test_app();
        app.set_media_dir(PathBuf::from("/tmp"));
        let out = app.show_media("../secret.png");
        assert!(out[0].contains("refused"));
    }

    #[test]
    fn log_is_bounded() {
        let mut app = test_app();
        app.set_mode(Mode::Auto);
        for _ in 0..(LOG_LINES_MAX + 50) {
            let _ = app.run_workflow("status", &Value::Null);
        }
        app.set_screen(Screen::Log);
        assert!(app.screen_lines().len() <= LOG_LINES_MAX);
    }
}
