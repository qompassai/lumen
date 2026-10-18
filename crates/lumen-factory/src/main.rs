//! lumen-factory CLI.
//!
//! Contract summary:
//! - `base --out <path.glb>` builds the procedural base and writes a GLB.
//! - `blend --emotions <path> --out <path.glb>` builds the base with
//!   morph targets composed from the emotion templates at `<path>`
//!   (head primitive only) and writes a GLB.
//! - `build --identity <record.json> --emotions <path> --out <model.glb>
//!   [--toon]` projects an identity record into a textured character GLB.
//! - `ingest --art-dir <dir> --companion <name> --out <record.json>`
//!   ingests canonical companion art into an identity record.
//! - `inspect --in <path.glb>` parse-back validates a GLB and prints its
//!   structural summary, including morph target names when present.
//! - `vrm-inspect --in <file.vrm>` prints the VRM summary (flavor, meta,
//!   humanoid bones, expressions, derived proportions).
//! - `vrm-wrap --in <glb> --name <model name> --out <file.vrm>` wraps a
//!   factory GLB as a VRM 1.0 file (VRMC_vrm humanoid + expressions).
//! - Failures are typed errors printed to stderr with a nonzero exit.

#![forbid(unsafe_code)]

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};
use lumen_factory::base::build_base_mesh;
use lumen_factory::blend::{compose_targets, load_templates};
use lumen_factory::gltf::{MorphTarget, parse_glb, write_glb};
use lumen_factory::identity::{ingest, load_record};
use lumen_factory::project::{BuildOptions, build_character};
use lumen_factory::vrm::{WrapMeta, inspect_vrm, wrap_as_vrm};
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
    /// Build the base mesh with composed blendshape morph targets.
    Blend {
        /// Path to the emotion templates JSON.
        #[arg(long)]
        emotions: PathBuf,
        /// Output GLB path.
        #[arg(long)]
        out: PathBuf,
    },
    /// Project an identity record into a textured character GLB.
    Build {
        /// Path to the emotion templates JSON.
        #[arg(long)]
        emotions: PathBuf,
        /// Path to the identity record JSON.
        #[arg(long)]
        identity: PathBuf,
        /// Output GLB path.
        #[arg(long)]
        out: PathBuf,
        /// Apply the demo_toon grade to the atlas.
        #[arg(long)]
        toon: bool,
    },
    /// Ingest canonical companion art into an identity record.
    Ingest {
        /// Art root directory (contains companions/ and expressions/).
        #[arg(long)]
        art_dir: PathBuf,
        /// Companion name (art file prefix).
        #[arg(long)]
        companion: String,
        /// Output identity record JSON path.
        #[arg(long)]
        out: PathBuf,
    },
    /// Parse-back inspect a GLB and print its structural summary.
    Inspect {
        /// Input GLB path.
        #[arg(long = "in")]
        input: PathBuf,
    },
    /// Inspect a VRM file and print its summary.
    VrmInspect {
        /// Input VRM path.
        #[arg(long = "in")]
        input: PathBuf,
    },
    /// Wrap a factory GLB as a VRM 1.0 file.
    VrmWrap {
        /// Input GLB path.
        #[arg(long = "in")]
        input: PathBuf,
        /// Model name for the VRM meta block.
        #[arg(long)]
        name: String,
        /// Output VRM path.
        #[arg(long)]
        out: PathBuf,
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
        Commands::Blend { emotions, out } => {
            let mesh = build_base_mesh()?;
            let templates = load_templates(&emotions)?;
            let head = mesh
                .parts
                .iter()
                .find(|part| part.name == "head")
                .ok_or_else(|| FactoryError::MeshInvalid("head part missing".to_string()))?;
            let head_positions: Vec<[f32; 3]> =
                head.vertices.iter().map(|vertex| vertex.position).collect();
            let composed = compose_targets(&templates, &head_positions, &mesh.head_masks)?;
            let target_names: Vec<String> = composed.iter().map(|(name, _)| name.clone()).collect();
            let morph_targets: Vec<MorphTarget> = composed
                .into_iter()
                .map(|(name, deltas)| MorphTarget { deltas, name })
                .collect();
            let bytes = write_glb(&mesh, &morph_targets)?;
            std::fs::write(&out, &bytes)?;
            println!("emotions: {}", templates.emotions.len());
            println!(
                "referenced_channels: {}",
                templates
                    .referenced_channels()
                    .into_iter()
                    .collect::<Vec<_>>()
                    .join(",")
            );
            println!("head_vertices: {}", head_positions.len());
            println!("targets: {}", target_names.len());
            println!("target_names: {}", target_names.join(","));
            println!("glb_bytes: {}", bytes.len());
            println!("sha256: {}", sha256_hex(&bytes));
            println!("out: {}", out.display());
            Ok(())
        }
        Commands::Build {
            emotions,
            identity,
            out,
            toon,
        } => {
            let record = load_record(&identity)?;
            let templates = load_templates(&emotions)?;
            let bytes = build_character(&record, &templates, BuildOptions { toon })?;
            std::fs::write(&out, &bytes)?;
            println!("companion: {}", record.companion);
            println!("record_hash: {}", record.record_hash);
            println!("toon: {toon}");
            println!("glb_bytes: {}", bytes.len());
            println!("sha256: {}", sha256_hex(&bytes));
            println!("out: {}", out.display());
            Ok(())
        }
        Commands::Ingest {
            art_dir,
            companion,
            out,
        } => {
            let record = ingest(&art_dir, &companion)?;
            let json = record.canonical_json()?;
            std::fs::write(&out, format!("{json}\n"))?;
            println!("companion: {}", record.companion);
            println!("sources: {}", record.sources.len());
            println!("hair: {}", record.palette.hair.hex);
            println!("skin: {}", record.palette.skin.hex);
            println!(
                "eye: {}",
                record
                    .palette
                    .eye
                    .as_ref()
                    .map(|c| c.hex.clone())
                    .unwrap_or_else(|| "none".to_string())
            );
            println!(
                "lips: {}",
                record
                    .palette
                    .lips
                    .as_ref()
                    .map(|c| c.hex.clone())
                    .unwrap_or_else(|| "none".to_string())
            );
            println!(
                "outfit: {}",
                record
                    .palette
                    .outfit
                    .iter()
                    .map(|c| c.hex.clone())
                    .collect::<Vec<_>>()
                    .join(",")
            );
            println!("head_ratio: {:.4}", record.proportions.head_ratio);
            println!("torso_ratio: {:.4}", record.proportions.torso_ratio);
            println!("leg_ratio: {:.4}", record.proportions.leg_ratio);
            println!("record_hash: {}", record.record_hash);
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
            println!("target_count: {}", summary.target_names.len());
            println!("targets: {}", summary.target_names.join(","));
            println!("weight_sum_min: {:.6}", summary.weight_sum_min);
            println!("weight_sum_max: {:.6}", summary.weight_sum_max);
            println!("sha256: {}", sha256_hex(&bytes));
            Ok(())
        }
        Commands::VrmInspect { input } => {
            let bytes = std::fs::read(&input)?;
            let summary = inspect_vrm(&bytes)?;
            println!("flavor: {}", summary.flavor);
            println!(
                "spec_version: {}",
                summary.spec_version.as_deref().unwrap_or("none")
            );
            println!("name: {}", summary.meta.name.as_deref().unwrap_or("none"));
            println!(
                "version: {}",
                summary.meta.version.as_deref().unwrap_or("none")
            );
            println!(
                "authors: {}",
                if summary.meta.authors.is_empty() {
                    "none".to_string()
                } else {
                    summary.meta.authors.join(",")
                }
            );
            println!(
                "license_url: {}",
                summary.meta.license_url.as_deref().unwrap_or("none")
            );
            println!("nodes: {}", summary.node_count);
            println!("bones: {}", summary.humanoid_bones.len());
            println!(
                "bone_map: {}",
                summary
                    .humanoid_bones
                    .iter()
                    .map(|(bone, node)| format!("{bone}={node}"))
                    .collect::<Vec<_>>()
                    .join(",")
            );
            println!(
                "expressions_preset: {}",
                summary
                    .preset_expressions
                    .iter()
                    .map(|e| e.name.clone())
                    .collect::<Vec<_>>()
                    .join(",")
            );
            println!(
                "expressions_custom: {}",
                summary
                    .custom_expressions
                    .iter()
                    .map(|e| e.name.clone())
                    .collect::<Vec<_>>()
                    .join(",")
            );
            match summary.proportions {
                Some(p) => {
                    println!("total_height: {:.4}", p.total_height);
                    println!("head_ratio: {:.4}", p.head_ratio);
                    println!("torso_ratio: {:.4}", p.torso_ratio);
                    println!("leg_ratio: {:.4}", p.leg_ratio);
                }
                None => println!("proportions: none"),
            }
            println!("sha256: {}", sha256_hex(&bytes));
            Ok(())
        }
        Commands::VrmWrap { input, name, out } => {
            let bytes = std::fs::read(&input)?;
            let wrapped = wrap_as_vrm(&bytes, &WrapMeta::new(&name))?;
            std::fs::write(&out, &wrapped)?;
            let summary = inspect_vrm(&wrapped)?;
            println!("flavor: {}", summary.flavor);
            println!("name: {}", summary.meta.name.as_deref().unwrap_or("none"));
            println!("bones: {}", summary.humanoid_bones.len());
            println!("expressions_preset: {}", summary.preset_expressions.len());
            println!("expressions_custom: {}", summary.custom_expressions.len());
            println!("glb_bytes: {}", wrapped.len());
            println!("sha256: {}", sha256_hex(&wrapped));
            println!("out: {}", out.display());
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
