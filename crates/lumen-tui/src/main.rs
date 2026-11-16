//! `lumen-tui` binary: parse args, build the real facade, run.
//!
//! Real wiring: [`McpStdioFacade`] spawns `lumen-mcp-server` (override
//! with `--server <path>` or `LUMEN_MCP_BIN`). The trigger sink for
//! bloch is an MCP client only when `BLOCH_MCP_BIN` names a bloch
//! server binary; otherwise it is the explicit
//! [`UnconfiguredSink`] and auto-rule failures are audited as such —
//! never silently treated as fired.

#![forbid(unsafe_code)]

use std::io::{BufReader, IsTerminal};
use std::path::PathBuf;
use std::process::ExitCode;

use lumen_tui::app::{App, run_fullscreen, run_line_mode};
use lumen_tui::facade::McpStdioFacade;
use lumen_tui::triggers::{Engine, TriggerSink, UnconfiguredSink};
use serde_json::Value;

/// Trigger sink that dispatches to a bloch MCP server over stdio,
/// spawning it per dispatch (bounded, owned, killed on completion).
struct BlochSink {
    server_bin: PathBuf,
    project_root: PathBuf,
}

impl TriggerSink for BlochSink {
    fn dispatch(&mut self, _target_mcp: &str, tool: &str, args: Value) -> Result<Value, String> {
        let mut facade = McpStdioFacade::spawn(&self.server_bin, &self.project_root)
            .map_err(|e| e.to_string())?;
        facade_call(&mut facade, tool, args)
    }
}

fn facade_call(facade: &mut McpStdioFacade, tool: &str, args: Value) -> Result<Value, String> {
    use lumen_tui::facade::LumenFacade;
    facade.call(tool, args).map_err(|e| e.to_string())
}

struct Args {
    line_mode: bool,
    server: PathBuf,
    root: PathBuf,
    triggers: Option<PathBuf>,
    media: Option<PathBuf>,
}

fn parse_args() -> Result<Args, String> {
    let mut args = Args {
        line_mode: false,
        server: std::env::var("LUMEN_MCP_BIN")
            .map(PathBuf::from)
            .unwrap_or_else(|_| PathBuf::from("lumen-mcp-server")),
        root: std::env::current_dir().map_err(|e| e.to_string())?,
        triggers: None,
        media: None,
    };
    let mut iter = std::env::args().skip(1);
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--line" => args.line_mode = true,
            "--server" => {
                args.server = PathBuf::from(iter.next().ok_or("--server needs a path")?);
            }
            "--root" => {
                args.root = PathBuf::from(iter.next().ok_or("--root needs a path")?);
            }
            "--triggers" => {
                args.triggers = Some(PathBuf::from(iter.next().ok_or("--triggers needs a path")?));
            }
            "--media" => {
                args.media = Some(PathBuf::from(iter.next().ok_or("--media needs a path")?));
            }
            "--help" | "-h" => {
                println!(
                    "usage: lumen-tui [--line] [--server <lumen-mcp-server>] [--root <dir>] \
                     [--triggers <triggers.toml>] [--media <dir>]"
                );
                std::process::exit(0);
            }
            other => return Err(format!("unknown argument: {other} (try --help)")),
        }
    }
    Ok(args)
}

fn load_engine(path: Option<&PathBuf>) -> Result<Engine, String> {
    match path {
        None => Engine::default_engine().map_err(|e| e.to_string()),
        Some(path) => {
            let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
            Engine::from_toml(&text).map_err(|e| e.to_string())
        }
    }
}

fn main() -> ExitCode {
    let args = match parse_args() {
        Ok(args) => args,
        Err(e) => {
            eprintln!("lumen-tui: {e}");
            return ExitCode::FAILURE;
        }
    };
    let engine = match load_engine(args.triggers.as_ref()) {
        Ok(engine) => engine,
        Err(e) => {
            eprintln!("lumen-tui: trigger config: {e}");
            return ExitCode::FAILURE;
        }
    };
    let facade = match McpStdioFacade::spawn(&args.server, &args.root) {
        Ok(facade) => facade,
        Err(e) => {
            eprintln!("lumen-tui: cannot reach lumen server: {e}");
            return ExitCode::FAILURE;
        }
    };
    let stdin_is_tty = std::io::stdin().is_terminal();
    let line_mode = args.line_mode || !stdin_is_tty;
    match std::env::var("BLOCH_MCP_BIN") {
        Ok(bin) if !bin.is_empty() => {
            let sink = BlochSink {
                server_bin: PathBuf::from(bin),
                project_root: args.root.clone(),
            };
            run_with(args, engine, facade, sink, line_mode)
        }
        _ => run_with(args, engine, facade, UnconfiguredSink, line_mode),
    }
}

fn run_with<S: TriggerSink>(
    args: Args,
    engine: Engine,
    facade: McpStdioFacade,
    sink: S,
    line_mode: bool,
) -> ExitCode {
    let mut app = App::new(facade, sink, engine);
    if let Some(media) = args.media {
        app.set_media_dir(media);
    }
    let result = if line_mode {
        let stdin = std::io::stdin();
        let mut reader = BufReader::new(stdin);
        let mut stdout = std::io::stdout();
        run_line_mode(&mut app, &mut reader, &mut stdout)
    } else {
        run_fullscreen(&mut app)
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("lumen-tui: {e}");
            ExitCode::FAILURE
        }
    }
}
