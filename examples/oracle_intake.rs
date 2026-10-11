//! Driver for the oracle intake (sprite-to-3D factory, oracle arm).
//!
//! ```text
//! cargo run --example oracle_intake -- intake <in.glb> <out.glb> <source> [manifest.json]
//! cargo run --example oracle_intake -- inspect <in.glb>
//! ```
//!
//! `source` is explicit: `pixal3d` (intake rotates 180 degrees about Y)
//! or `trellis` / `trellis-cpp` / `trellis.cpp` (byte-identical
//! pass-through). Lives in `examples/` so the shipped `lumen` binary
//! and its CLI stay untouched while the factory is a prototype. Inputs
//! are size-checked before they are read; outputs are written via
//! temp-file + rename.

#![forbid(unsafe_code)]

use std::path::Path;
use std::process::ExitCode;

use lumen::LumenError;
use lumen::oracle::{
    GLB_BYTES_MAX, OracleSource, inspect_glb_bytes, intake_glb_bytes, world_positions,
};

fn read_bounded(path: &Path) -> Result<Vec<u8>, LumenError> {
    let size = std::fs::metadata(path)?.len();
    if size > GLB_BYTES_MAX {
        return Err(LumenError::BadParam(format!(
            "{} is {size} bytes; at most {GLB_BYTES_MAX} are accepted",
            path.display()
        )));
    }
    Ok(std::fs::read(path)?)
}

fn write_atomic(path: &Path, contents: &[u8]) -> Result<(), LumenError> {
    let mut temp = path.as_os_str().to_owned();
    temp.push(".tmp");
    std::fs::write(&temp, contents)?;
    std::fs::rename(&temp, path)?;
    Ok(())
}

fn intake(
    input: &Path,
    output: &Path,
    source_raw: &str,
    manifest: Option<&Path>,
) -> Result<(), LumenError> {
    let source = OracleSource::parse(source_raw)?;
    let bytes = read_bounded(input)?;
    let (out_bytes, report) = intake_glb_bytes(&bytes, source)?;
    write_atomic(output, &out_bytes)?;
    if let Some(manifest_path) = manifest {
        let mut text = serde_json::to_string_pretty(&report)
            .map_err(|e| LumenError::BadParam(format!("manifest serialize failed: {e}")))?;
        text.push('\n');
        write_atomic(manifest_path, text.as_bytes())?;
    }
    println!(
        "intake {} -> {}: source={} rotation_y={}deg vertices={} triangles={} \
         bin_preserved={} in={}B out={}B",
        input.display(),
        output.display(),
        report.source,
        report.rotation_degrees_y,
        report.vertices,
        report.triangles,
        report.bin_chunk_preserved,
        report.input_bytes,
        report.output_bytes,
    );
    Ok(())
}

fn inspect(input: &Path) -> Result<(), LumenError> {
    let bytes = read_bounded(input)?;
    let stats = inspect_glb_bytes(&bytes)?;
    let positions = world_positions(&bytes)?;
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for point in &positions {
        for axis in 0..3 {
            min[axis] = min[axis].min(point[axis]);
            max[axis] = max[axis].max(point[axis]);
        }
    }
    println!(
        "inspect {}: meshes={} primitives={} vertices={} triangles={} nodes={} scenes={}",
        input.display(),
        stats.meshes,
        stats.primitives,
        stats.vertices,
        stats.triangles,
        stats.nodes,
        stats.scenes,
    );
    println!(
        "world bbox: min=[{:.4}, {:.4}, {:.4}] max=[{:.4}, {:.4}, {:.4}] positions={}",
        min[0],
        min[1],
        min[2],
        max[0],
        max[1],
        max[2],
        positions.len(),
    );
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    let result = match argv.as_slice() {
        ["intake", input, output, source] => {
            intake(Path::new(input), Path::new(output), source, None)
        }
        ["intake", input, output, source, manifest] => intake(
            Path::new(input),
            Path::new(output),
            source,
            Some(Path::new(manifest)),
        ),
        ["inspect", input] => inspect(Path::new(input)),
        _ => Err(LumenError::BadParam(
            "usage: intake <in.glb> <out.glb> <pixal3d|trellis-cpp> [manifest.json] \
             | inspect <in.glb>"
                .into(),
        )),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}
