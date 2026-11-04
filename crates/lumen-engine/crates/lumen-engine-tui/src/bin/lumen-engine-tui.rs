//! lumen-engine-tui: the Owner's-terminal view over
//! the operation core. A ratatui panel per workflow
//! (SPEC section 5.1); pressing `r` runs the panel's
//! verb — the same `ops` function the CLI dispatches
//! to, so the two views cannot drift (SPEC section
//! 5.3). The TUI is a view; it contains no workflow
//! logic of its own.

use std::path::PathBuf;
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use lumen_engine_tui::ops::{self, OpResult, Outcome};
use lumen_engine_tui::termcap;
use lumen_engine_tui::view::{self, AppState};

struct Ctx {
    root: PathBuf,
    imports: PathBuf,
    scene: PathBuf,
}

fn arg_value(args: &[String], key: &str) -> Option<String> {
    args.windows(2).find(|w| w[0] == key).map(|w| w[1].clone())
}

/// Fold a verb result into a displayable outcome; a
/// typed refusal becomes a failed outcome whose line
/// names the refusal, so the panel shows it instead of
/// hiding it.
fn fold(result: OpResult) -> Outcome {
    match result {
        Ok(outcome) => outcome,
        Err(refusal) => Outcome {
            title: "refused".to_string(),
            lines: vec![format!("refusal {}: {}", refusal.code, refusal.message)],
            artifacts: Vec::new(),
            ok: false,
        },
    }
}

/// Compose several verb outcomes into one panel
/// outcome (the panel's verb is a fixed chain of core
/// verbs; each keeps its own readout lines).
fn merge(results: Vec<OpResult>) -> Outcome {
    let mut merged = Outcome {
        title: "panel".to_string(),
        lines: Vec::new(),
        artifacts: Vec::new(),
        ok: true,
    };
    for result in results {
        let outcome = fold(result);
        merged.lines.push(format!("== {}", outcome.title));
        merged.lines.extend(outcome.lines);
        merged.artifacts.extend(outcome.artifacts);
        merged.ok = merged.ok && outcome.ok;
    }
    merged
}

fn run_panel(state: &mut AppState, ctx: &Ctx) {
    let tab = state.tab;
    let outcome = match tab {
        // Author: project + canon handles + keeper chains.
        0 => merge(vec![
            ops::project_open(&ctx.root, &ctx.imports),
            ops::author_handles(&ctx.root),
            ops::author_chains(&ctx.root),
        ]),
        // Stage: scene tree + a live preview frame.
        1 => {
            let runs = ctx.root.join("runs");
            let _ = std::fs::create_dir_all(&runs);
            let png = runs.join("preview.png");
            let cells = runs.join("preview.cells.txt");
            let kitty = runs.join("preview.kitty");
            let actor = ops::load_scene(&ctx.scene)
                .map(|s| s.actors.first().map(|a| a.id.clone()).unwrap_or_default())
                .unwrap_or_default();
            let outcome = merge(vec![
                ops::scene_tree(&ctx.scene),
                ops::scene_preview(&ctx.scene, &ctx.imports, &actor, 5.0, &png, &cells, &kitty),
            ]);
            if png.is_file() {
                if let Ok(img) = image::open(&png) {
                    state.preview = Some(termcap::halfblocks(&img.to_rgba8(), 64));
                }
            }
            outcome
        }
        // Inspect: census of the scene's first rig.
        2 => {
            let rig = ops::load_scene(&ctx.scene)
                .map(|s| ctx.imports.join(&s.actors[0].rig))
                .unwrap_or_else(|_| ctx.imports.clone());
            merge(vec![ops::rig_census(&rig), ops::rig_bindings(&ctx.scene)])
        }
        // Play: the core-grain control surface.
        3 => {
            let steps = ops::load_scene(&ctx.scene).map(|s| s.steps).unwrap_or(720);
            let snapshot_at = if steps > 360 { 360 } else { steps / 2 };
            merge(vec![ops::play_step(&ctx.scene, steps, snapshot_at)])
        }
        // Gates: flash gate, plus the sweep gate when a
        // sweep artifact exists in the project.
        4 => {
            let mut verbs = vec![ops::gate_flash(&ctx.scene)];
            let sweep = ctx.root.join("runs").join("sweep.json");
            if sweep.is_file() {
                verbs.push(ops::gate_sweep(&sweep));
            }
            merge(verbs)
        }
        // Pack: build -> package -> record -> verify,
        // with the host binary that ships beside this
        // one in the workspace target directory.
        _ => {
            let host_bin = std::env::current_exe()
                .map(|exe| exe.with_file_name("lumen-engine-host"))
                .unwrap_or_else(|_| PathBuf::from("lumen-engine-host"));
            let build_dir = ctx.root.join("release-build");
            let archive = ctx.root.join("release.tar.gz");
            let record = ctx.root.join("release.json");
            let project = ctx
                .root
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_else(|| "project".to_string());
            merge(vec![
                ops::pack_build(&ctx.root, &host_bin, &build_dir),
                ops::pack_package(&build_dir, &archive),
                ops::pack_record(&archive, &project, &record),
                ops::pack_verify(&archive, &record),
            ])
        }
    };
    state.status = format!(
        "panel {} verb ran — verdict {} (r re-run, 1-6 panels, q quit)",
        view::TABS[tab],
        if outcome.ok { "PASS" } else { "FAIL" }
    );
    state.outcomes[tab] = Some(outcome);
}

fn main() -> std::io::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = PathBuf::from(arg_value(&args, "--root").unwrap_or_else(|| ".".to_string()));
    let imports = PathBuf::from(
        arg_value(&args, "--imports")
            .or_else(|| std::env::var("LUMEN_IMPORTS").ok())
            .unwrap_or_default(),
    );
    let scene = match arg_value(&args, "--scene") {
        Some(s) => PathBuf::from(s),
        None => ops::project_scenes(&root)
            .ok()
            .and_then(|mut v| {
                if v.is_empty() {
                    None
                } else {
                    Some(v.remove(0))
                }
            })
            .unwrap_or_else(|| root.join("scenes").join("twobody-lift.scene.ron")),
    };
    // Capability probe (D-902 layer): the picker's own
    // stdio query only when explicitly requested — an
    // automated terminal must never be kept waiting on
    // a query response. Default: the halfblocks picker.
    let picker = if std::env::var("LUMEN_TUI_PICKER_QUERY").is_ok() {
        ratatui_image::picker::Picker::from_query_stdio()
            .unwrap_or_else(|_| ratatui_image::picker::Picker::halfblocks())
    } else {
        ratatui_image::picker::Picker::halfblocks()
    };
    let caps = termcap::detect(
        std::env::var("TERM").ok().as_deref(),
        std::env::var("TERM_PROGRAM").ok().as_deref(),
    );
    let header = format!(
        "project {} | scene {} | imports {} | picker {:?}",
        root.display(),
        scene
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        imports.display(),
        picker.protocol_type()
    );
    let ctx = Ctx {
        root,
        imports,
        scene,
    };
    let mut state = AppState::new(header, caps);
    let mut terminal = ratatui::init();
    let result = event_loop(&mut terminal, &mut state, &ctx);
    ratatui::restore();
    result
}

fn event_loop(
    terminal: &mut ratatui::Terminal<ratatui::backend::CrosstermBackend<std::io::Stdout>>,
    state: &mut AppState,
    ctx: &Ctx,
) -> std::io::Result<()> {
    loop {
        terminal.draw(|frame| view::draw(frame, state))?;
        if event::poll(Duration::from_millis(100))? {
            if let Event::Key(key) = event::read()? {
                if key.kind != KeyEventKind::Press {
                    continue;
                }
                match key.code {
                    KeyCode::Char('q') | KeyCode::Esc => return Ok(()),
                    KeyCode::Tab => state.tab = (state.tab + 1) % view::TABS.len(),
                    KeyCode::BackTab => {
                        state.tab = (state.tab + view::TABS.len() - 1) % view::TABS.len();
                    }
                    KeyCode::Char(c @ '1'..='6') => {
                        state.tab = (c as usize - '1' as usize) % view::TABS.len();
                    }
                    KeyCode::Char('r') | KeyCode::Enter => run_panel(state, ctx),
                    _ => {}
                }
            }
        }
    }
}
