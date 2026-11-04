//! INP1 import driver (proof tool, SPEC 13.10): import an
//! `.inp`/`.inx` file into a native model asset, a puppet
//! stage asset, and texture files, and print the census.
//!
//! Usage: `puppet_import <input.inx> <output-dir> [fallback-id]`

use std::path::PathBuf;
use std::process::ExitCode;

use lumen_rig_core::inp::import_inp;

fn main() -> ExitCode {
    let mut args = std::env::args_os().skip(1);
    let (Some(input), Some(out_dir)) = (args.next(), args.next()) else {
        eprintln!("usage: puppet_import <input.inx> <output-dir> [fallback-id]");
        return ExitCode::from(2);
    };
    let fallback = args
        .next()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "imported_puppet".to_string());
    let input = PathBuf::from(input);
    let out_dir = PathBuf::from(out_dir);

    let bytes = match std::fs::read(&input) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("cannot read {}: {e}", input.display());
            return ExitCode::FAILURE;
        }
    };
    let imported = match import_inp(&bytes, &fallback) {
        Ok(i) => i,
        Err(e) => {
            eprintln!("import failed [{}]: {e}", e.code());
            return ExitCode::FAILURE;
        }
    };
    let report = &imported.report;
    println!(
        "census: nodes={} (group={} part={} composite={} mesh_group={} physics={})",
        report.nodes,
        report.group_nodes,
        report.part_nodes,
        report.composite_nodes,
        report.mesh_group_nodes,
        report.physics_nodes
    );
    println!(
        "census: parameters={} (vec2={}) bindings={} (cubic={}) masks={} textures={} ext_entries={} dropped_texture_slots={}",
        report.parameters,
        report.vec2_parameters,
        report.bindings,
        report.cubic_bindings,
        report.masks,
        report.textures,
        report.ext_entries,
        report.dropped_texture_slots
    );
    println!("model id: {}", imported.model.id);

    if let Err(e) = std::fs::create_dir_all(out_dir.join("textures")) {
        eprintln!("cannot create output dir: {e}");
        return ExitCode::FAILURE;
    }
    let model_path = out_dir.join("model.json");
    let stage_path = out_dir.join("puppet.json");
    if let Err(e) = std::fs::write(&model_path, imported.model.to_canonical_json()) {
        eprintln!("cannot write {}: {e}", model_path.display());
        return ExitCode::FAILURE;
    }
    if let Err(e) = std::fs::write(&stage_path, imported.stage.to_canonical_json()) {
        eprintln!("cannot write {}: {e}", stage_path.display());
        return ExitCode::FAILURE;
    }
    for texture in &imported.textures {
        let path = out_dir.join(&texture.source);
        if let Err(e) = std::fs::write(&path, &texture.bytes) {
            eprintln!("cannot write {}: {e}", path.display());
            return ExitCode::FAILURE;
        }
    }
    println!(
        "wrote {} and {}",
        model_path.display(),
        stage_path.display()
    );
    ExitCode::SUCCESS
}
