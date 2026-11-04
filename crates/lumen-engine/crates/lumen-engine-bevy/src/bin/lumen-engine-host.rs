//! lumen-engine-host: run one engine scene record
//! through the Bevy host and write its reads, summary,
//! evidence, frames, state snapshot, and runtime probe
//! log to an output directory.
//!
//! Usage:
//!   lumen-engine-host <scene.ron> <imports-base> <out-dir>
//!                     [--reference <verdict-pack-dir>]
//!                     [--snapshot-step <n>]
//!
//! Exit status is non-zero on any staging, tune, or
//! determinism failure; the summary JSON is printed on
//! success.

use std::path::PathBuf;
use std::process::ExitCode;

use lumen_engine_bevy::{run, HostJob};
use lumen_engine_core::SceneRecord;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let (Some(scene_path), Some(imports_base), Some(out_dir)) =
        (args.next(), args.next(), args.next())
    else {
        eprintln!(
            "usage: lumen-engine-host <scene.ron> <imports-base> <out-dir> \
             [--reference <dir>] [--snapshot-step <n>]"
        );
        return ExitCode::from(2);
    };
    let mut reference_dir: Option<PathBuf> = None;
    let mut snapshot_step: u64 = 360;
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--reference" => {
                reference_dir = args.next().map(PathBuf::from);
            }
            "--snapshot-step" => {
                snapshot_step = args
                    .next()
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(snapshot_step);
            }
            other => {
                eprintln!("unknown flag: {other}");
                return ExitCode::from(2);
            }
        }
    }
    let text = match std::fs::read_to_string(&scene_path) {
        Ok(text) => text,
        Err(e) => {
            eprintln!("scene record read: {e}");
            return ExitCode::FAILURE;
        }
    };
    let scene = match SceneRecord::load_ron(&text) {
        Ok(scene) => scene,
        Err(e) => {
            eprintln!("scene record refused: {e}");
            return ExitCode::FAILURE;
        }
    };
    let job = HostJob {
        scene,
        imports_base: PathBuf::from(imports_base),
        out_dir: PathBuf::from(out_dir),
        reference_dir,
        snapshot_step,
    };
    match run(job) {
        Ok(report) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&report.summary).expect("summary serializes")
            );
            if let Some(matches) = report.evidence.pointer("/tune_check/matches_expected") {
                println!("tune matches expected: {matches}");
            }
            if let Some(matches) = report
                .evidence
                .pointer("/runtime_probe/restore_event_stream_matches")
            {
                println!("runtime restore matches: {matches}");
            }
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("host run failed: {message}");
            ExitCode::FAILURE
        }
    }
}
