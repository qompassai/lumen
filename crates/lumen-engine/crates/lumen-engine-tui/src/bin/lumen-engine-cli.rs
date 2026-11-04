//! lumen-engine-cli: the headless view over the
//! operation core. Agents operate the TUI surface
//! through this binary with semantics identical to the
//! ratatui view (SPEC section 5.3): every verb is an
//! `ops` function, this file only parses flags, prints
//! the outcome, and maps the result to an exit code —
//! 0 pass, 1 gate fail, 2 typed refusal, 3 usage error.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::process::ExitCode;

use lumen_engine_tui::ops::{self, OpResult, Outcome};

struct Flags(BTreeMap<String, String>);

impl Flags {
    fn get(&self, key: &str) -> Result<&str, String> {
        self.0
            .get(key)
            .map(String::as_str)
            .ok_or_else(|| format!("missing required flag --{key}"))
    }
    fn path(&self, key: &str) -> Result<PathBuf, String> {
        Ok(PathBuf::from(self.get(key)?))
    }
    fn opt_path(&self, key: &str) -> Option<PathBuf> {
        self.0.get(key).map(PathBuf::from)
    }
    fn u64(&self, key: &str, default: u64) -> Result<u64, String> {
        match self.0.get(key) {
            Some(v) => v
                .parse()
                .map_err(|_| format!("--{key} must be a whole number")),
            None => Ok(default),
        }
    }
    fn f64(&self, key: &str, default: f64) -> Result<f64, String> {
        match self.0.get(key) {
            Some(v) => v.parse().map_err(|_| format!("--{key} must be a number")),
            None => Ok(default),
        }
    }
}

fn parse_flags(args: &[String]) -> Result<Flags, String> {
    let mut map = BTreeMap::new();
    let mut i = 0;
    while i < args.len() {
        let key = args[i]
            .strip_prefix("--")
            .ok_or_else(|| format!("expected --flag, got {:?}", args[i]))?;
        let value = args
            .get(i + 1)
            .ok_or_else(|| format!("--{key} needs a value"))?;
        map.insert(key.to_string(), value.clone());
        i += 2;
    }
    Ok(Flags(map))
}

fn dispatch(group: &str, verb: &str, flags: &Flags) -> Result<OpResult, String> {
    let root = || flags.path("root");
    let scene = || flags.path("scene");
    let imports = || flags.path("imports");
    Ok(match (group, verb) {
        ("project", "open") => ops::project_open(&root()?, &imports()?),
        ("author", "handles") => ops::author_handles(&root()?),
        ("author", "chains") => ops::author_chains(&root()?),
        ("author", "describe") => ops::author_describe(
            &root()?,
            flags.get("id")?,
            flags.0.get("label").map(String::as_str),
            flags.get("text")?,
        ),
        ("author", "propose") => ops::author_propose(
            &root()?,
            flags.get("id")?,
            flags.get("scene")?,
            flags.get("from")?,
            flags.get("to")?,
        ),
        ("author", "freeze") => ops::author_freeze(&root()?, flags.get("id")?),
        ("scene", "tree") => ops::scene_tree(&scene()?),
        ("scene", "preview") => ops::scene_preview(
            &scene()?,
            &imports()?,
            flags.get("actor")?,
            flags.f64("t", 0.0)?,
            &flags.path("out")?,
            &flags.path("cells")?,
            &flags.path("kitty")?,
        ),
        ("rig", "census") => ops::rig_census(&flags.path("rig")?),
        ("rig", "sweep") => ops::rig_sweep(&flags.path("rig")?, &flags.path("out")?),
        ("rig", "bindings") => ops::rig_bindings(&scene()?),
        ("play", "step") => ops::play_step(
            &scene()?,
            flags.u64("steps", 720)?,
            flags.u64("snapshot-at", 360)?,
        ),
        ("play", "run") => ops::play_run(
            &scene()?,
            &imports()?,
            &flags.path("out")?,
            flags.opt_path("reference").as_deref(),
        ),
        ("play", "replay") => ops::play_replay(
            &scene()?,
            &imports()?,
            &flags.path("out-a")?,
            &flags.path("out-b")?,
        ),
        ("play", "inspect") => ops::play_inspect(&flags.path("state")?),
        ("gate", "sweep") => ops::gate_sweep(&flags.path("sweep")?),
        ("gate", "flash") => ops::gate_flash(&scene()?),
        ("gate", "compare") => ops::gate_compare(&flags.path("run")?, &flags.path("reference")?),
        ("verdict", "pack") => {
            ops::verdict_pack(&flags.path("run")?, &scene()?, &flags.path("out")?)
        }
        ("pack", "build") => {
            ops::pack_build(&root()?, &flags.path("host-bin")?, &flags.path("out")?)
        }
        ("pack", "package") => ops::pack_package(&flags.path("dir")?, &flags.path("out")?),
        ("pack", "record") => ops::pack_record(
            &flags.path("archive")?,
            flags.get("project")?,
            &flags.path("out")?,
        ),
        ("pack", "verify") => ops::pack_verify(&flags.path("archive")?, &flags.path("record")?),
        _ => return Err(format!("unknown verb {group} {verb}")),
    })
}

fn print_outcome(outcome: &Outcome) {
    println!("== {}", outcome.title);
    for line in &outcome.lines {
        println!("{line}");
    }
    for artifact in &outcome.artifacts {
        println!("artifact: {}", artifact.display());
    }
    println!("verdict: {}", if outcome.ok { "PASS" } else { "FAIL" });
}

const USAGE: &str = "\
lumen-engine-cli — headless view over the lumen engine operation core

  project open    --root <dir> --imports <dir>
  author handles  --root <dir>
  author chains   --root <dir>
  author describe --root <dir> --id <id> --label <sfw|nsfw|...> --text <text>
  author propose  --root <dir> --id <id> --scene <relpath> --from <s> --to <s>
  author freeze   --root <dir> --id <id>
  scene tree      --scene <file>
  scene preview   --scene <file> --imports <dir> --actor <id> --t <secs> --out <png> --cells <txt> --kitty <txt>
  rig census      --rig <dir>
  rig sweep       --rig <dir> --out <json>
  rig bindings    --scene <file>
  play step       --scene <file> [--steps n] [--snapshot-at n]
  play run        --scene <file> --imports <dir> --out <dir> [--reference <dir>]
  play replay     --scene <file> --imports <dir> --out-a <dir> --out-b <dir>
  play inspect    --state <file.ron>
  gate sweep      --sweep <json>
  gate flash      --scene <file>
  gate compare    --run <dir> --reference <dir>
  verdict pack    --run <dir> --scene <file> --out <dir>
  pack build      --root <dir> --host-bin <file> --out <dir>
  pack package    --dir <dir> --out <file.tar.gz>
  pack record     --archive <file.tar.gz> --project <name> --out <json>
  pack verify     --archive <file.tar.gz> --record <json>

exit codes: 0 pass, 1 gate fail, 2 typed refusal, 3 usage error";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.len() < 2 || args[0] == "--help" {
        println!("{USAGE}");
        return if args.is_empty() {
            ExitCode::from(3)
        } else {
            ExitCode::SUCCESS
        };
    }
    let flags = match parse_flags(&args[2..]) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("usage error: {e}");
            return ExitCode::from(3);
        }
    };
    match dispatch(&args[0], &args[1], &flags) {
        Ok(Ok(outcome)) => {
            let ok = outcome.ok;
            print_outcome(&outcome);
            if ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }
        Ok(Err(refusal)) => {
            eprintln!("refusal {}: {}", refusal.code, refusal.message);
            ExitCode::from(2)
        }
        Err(e) => {
            eprintln!("usage error: {e}");
            ExitCode::from(3)
        }
    }
}
