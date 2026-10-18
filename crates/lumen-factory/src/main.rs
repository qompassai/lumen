//! lumen-factory CLI.
//!
//! Contract summary:
//! - `base --out <path.glb>` builds the procedural base and writes a GLB.
//! - `inspect --in <path.glb>` parse-back validates a GLB and prints its
//!   structural summary.
//! - Failures are typed errors printed to stderr with a nonzero exit.

#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use lumen_factory::base::build_base_mesh;
use lumen_factory::gltf::{parse_glb, write_glb};
use lumen_factory::{FactoryError, sha256_hex};

#[derive(Parser)]
#[command(name = "lumen-factory", version, about = "Lumen sprite-to-3D factory")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Build the procedural base mesh and write it as a GLB.
    Base {
        /// Output GLB path.
        #[arg(long)]
        out: PathBuf,
    },
    /// Parse-back inspect a GLB and print its structural summary.
    Inspect {
        /// Input GLB path.
        #[arg(long = "in")]
        input: PathBuf,
    },
}

fn run() -> Result<(), FactoryError> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Base { out } => {
            let mesh = build_base_mesh()?;
            let bytes = write_glb(&mesh, &[])?;
            std::fs::write(&out, &bytes)?;
            let vertex_count: usize = mesh.parts.iter().map(|p| p.vertices.len()).sum();
            let index_count: usize = mesh.parts.iter().map(|p| p.indices.len()).sum();
            println!("joints: {}", mesh.joints.len());
            println!("parts: {}", mesh.parts.len());
            println!("vertices: {vertex_count}");
            println!("indices: {index_count}");
            println!("head_vertices: {}", mesh.head_masks.len());
            println!("glb_bytes: {}", bytes.len());
            println!("sha256: {}", sha256_hex(&bytes));
            println!("out: {}", out.display());
            Ok(())
        }
        Commands::Inspect { input } => {
            let bytes = std::fs::read(&input)?;
            let summary = parse_glb(&bytes)?;
            println!("nodes: {}", summary.node_count);
            println!("joints: {}", summary.joint_count);
            println!("meshes: {}", summary.mesh_count);
            println!("primitives: {}", summary.primitive_count);
            println!("vertices: {}", summary.vertex_count);
            println!("indices: {}", summary.index_count);
            println!("targets: {}", summary.target_names.join(","));
            println!("weight_sum_min: {:.6}", summary.weight_sum_min);
            println!("weight_sum_max: {:.6}", summary.weight_sum_max);
            println!("sha256: {}", sha256_hex(&bytes));
            Ok(())
        }
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("lumen-factory: {error}");
            ExitCode::FAILURE
        }
    }
}
