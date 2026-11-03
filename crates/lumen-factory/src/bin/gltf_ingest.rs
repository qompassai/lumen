//! gltf-ingest command-line entry point (gltf-import/1, Stage 2
//! Route A, D-633).
//!
//! Usage:
//!   `gltf_ingest <file.glb> [out.json]` — lower a Stage 1 GLB to a
//!   deterministic `torogao-mmd-ingest/1` JSON record (stdout when
//!   no output path is given).
//!   `gltf_ingest --emit-profile [out.ron]` — render
//!   `mmd-profile/1.ron` from the lumen profile tables.
//!
//! Exit codes: 0 = ok, 1 = failure (reason on stderr), 2 = usage.

#![forbid(unsafe_code)]

use std::io::Read;
use std::path::Path;
use std::process::ExitCode;

use lumen_factory::FactoryError;
use lumen_factory::convert::gltf::{emit_profile_ron, ingest_glb};

fn read_capped(path: &Path) -> Result<Vec<u8>, String> {
    let mut file =
        std::fs::File::open(path).map_err(|err| format!("cannot open {path:?}: {err}"))?;
    let cap = lumen_factory::MAX_GLB_BYTES as u64;
    let mut bytes = Vec::new();
    file.by_ref()
        .take(cap + 1)
        .read_to_end(&mut bytes)
        .map_err(|err| format!("cannot read {path:?}: {err}"))?;
    if bytes.len() as u64 > cap {
        return Err(format!(
            "glb exceeds {} byte bound",
            lumen_factory::MAX_GLB_BYTES
        ));
    }
    Ok(bytes)
}

fn write_out(path: Option<&String>, text: &str) -> Result<(), String> {
    match path {
        Some(path) => {
            std::fs::write(path, text).map_err(|err| format!("cannot write {path:?}: {err}"))
        }
        None => {
            print!("{text}");
            Ok(())
        }
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("--emit-profile") {
        if args.len() > 3 {
            return Err("usage: gltf_ingest --emit-profile [out.ron]".to_string());
        }
        return write_out(args.get(2), &emit_profile_ron());
    }
    if args.len() != 2 && args.len() != 3 {
        return Err(
            "usage: gltf_ingest <file.glb> [out.json] | gltf_ingest --emit-profile [out.ron]"
                .to_string(),
        );
    }
    let bytes = read_capped(Path::new(&args[1]))?;
    let record = ingest_glb(&bytes).map_err(|err: FactoryError| err.to_string())?;
    write_out(args.get(2), &format!("{}\n", record.to_json()))
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) if err.starts_with("usage:") => {
            eprintln!("{err}");
            ExitCode::from(2)
        }
        Err(err) => {
            eprintln!("gltf_ingest: {err}");
            ExitCode::FAILURE
        }
    }
}
