//! pmx-convert command-line entry point (pmx-import/1).
//!
//! Usage: `pmx_convert <file.pmx> <out.glb> [report.json]`
//!
//! Converts a PMX file to a deterministic GLB. Textures resolve
//! relative to the PMX file's directory at conversion time; only
//! PNG/JPEG textures embed, everything else is ledgered in the
//! conversion report. With a report path, also writes the census
//! JSON, the D-631 proportion sidecar, and the ledger summary as
//! one deterministic JSON document. Exit codes: 0 = converted,
//! 1 = failure (reason on stderr), 2 = usage error.

#![forbid(unsafe_code)]

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use lumen_factory::convert::pmx::census::census_of;
use lumen_factory::convert::pmx::glb::{ConvertOptions, ResolvedTexture, convert};
use lumen_factory::convert::pmx::parse::{INPUT_BYTES_MAX, parse_model};
use lumen_factory::convert::pmx::profile::{MmdProfile, assess_proportions};
use lumen_factory::sha256_hex;

fn read_capped(path: &Path) -> Result<Vec<u8>, String> {
    let mut file =
        std::fs::File::open(path).map_err(|err| format!("cannot open {path:?}: {err}"))?;
    let cap = INPUT_BYTES_MAX as u64;
    let mut bytes = Vec::new();
    file.by_ref()
        .take(cap + 1)
        .read_to_end(&mut bytes)
        .map_err(|err| format!("cannot read {path:?}: {err}"))?;
    Ok(bytes)
}

/// Resolve one PMX texture path relative to the model's directory.
/// PMX paths are untrusted strings: backslashes are treated as
/// separators (MMD convention), and absolute paths or `..` escapes
/// are refused — the ledger, not the filesystem, is the fallback.
fn resolve_texture(base_dir: &Path, pmx_path: &str) -> Option<PathBuf> {
    let cleaned = pmx_path.replace('\\', "/");
    let candidate = Path::new(&cleaned);
    if candidate.is_absolute() {
        return None;
    }
    let mut joined = base_dir.to_path_buf();
    for component in candidate.components() {
        match component {
            std::path::Component::Normal(part) => joined.push(part),
            std::path::Component::CurDir => {}
            _ => return None,
        }
    }
    Some(joined)
}

fn mime_for(path: &Path) -> String {
    match path
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => "image/png".to_string(),
        "jpg" | "jpeg" => "image/jpeg".to_string(),
        "bmp" => "image/bmp".to_string(),
        "tga" => "image/x-tga".to_string(),
        "gif" => "image/gif".to_string(),
        _ => "application/octet-stream".to_string(),
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 && args.len() != 4 {
        return Err("usage: pmx_convert <file.pmx> <out.glb> [report.json]".to_string());
    }
    let pmx_path = PathBuf::from(&args[1]);
    let out_path = PathBuf::from(&args[2]);
    let bytes = read_capped(&pmx_path)?;
    let model = parse_model(&bytes).map_err(|err| format!("{}: {err}", pmx_path.display()))?;
    let census = census_of(&model, &bytes);

    let base_dir = pmx_path
        .parent()
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    let mut textures: Vec<ResolvedTexture> = Vec::new();
    for (index, texture) in model.textures.iter().enumerate() {
        if let Some(path) = resolve_texture(&base_dir, &texture.path)
            && let Ok(data) = std::fs::read(&path)
        {
            textures.push(ResolvedTexture {
                pmx_index: index,
                bytes: data,
                mime: mime_for(&path),
            });
        }
    }

    let profile = MmdProfile::builtin();
    let options = ConvertOptions {
        scale_to_meters: profile.proportions.scale_to_meters,
        textures,
    };
    let (glb, report) = convert(&model, &options).map_err(|err| err.to_string())?;
    std::fs::write(&out_path, &glb).map_err(|err| format!("cannot write {out_path:?}: {err}"))?;

    if let Some(report_path) = args.get(3) {
        let assessment = assess_proportions(&model, &profile.proportions);
        let summary = serde_json::json!({
            "file": pmx_path.file_name().unwrap_or_default().to_string_lossy(),
            "census": serde_json::from_str::<serde_json::Value>(&census.to_json())
                .unwrap_or(serde_json::Value::Null),
            "proportions_sidecar": serde_json::from_str::<serde_json::Value>(
                &assessment.to_json(&profile.proportions)
            )
            .unwrap_or(serde_json::Value::Null),
            "glb_sha256": sha256_hex(&glb),
            "glb_bytes": glb.len(),
            "conversion": {
                "vertices": report.vertex_count,
                "triangles": report.triangle_count,
                "meshes": report.mesh_count,
                "bones": report.bone_count,
                "weight_corrections": report.weight_corrections,
                "sdef_vertices": report.sdef_vertices,
                "qdef_vertices": report.qdef_vertices,
                "invalid_bone_refs": report.invalid_bone_refs,
                "morph_targets": report.morph_target_count,
                "morphed_attribute_count": report.morphed_attribute_count,
                "exceeds_morph_floor": report.exceeds_morph_floor,
                "spans_adjusted": report.spans_adjusted,
                "scale_to_meters": report.scale_to_meters,
                "unconverted_morphs": report.unconverted_morphs.iter().map(|m| {
                    serde_json::json!({
                        "name": m.name,
                        "type": m.morph_type,
                        "offset_count": m.offset_count,
                    })
                }).collect::<Vec<_>>(),
                "fac_unmapped": report.fac_mapping.iter()
                    .filter(|row| row.channel.is_none())
                    .map(|row| row.morph.clone())
                    .collect::<Vec<_>>(),
                "fac_mapped": report.fac_mapping.iter()
                    .filter(|row| row.channel.is_some())
                    .map(|row| serde_json::json!({
                        "morph": row.morph,
                        "channel": row.channel,
                    }))
                    .collect::<Vec<_>>(),
                "missing_textures": report.missing_textures,
                "embedded_textures": report.embedded_textures,
                "unmapped_bones": report.unmapped_bones,
                "recipes": report.recipes.iter().map(|r| {
                    serde_json::json!({
                        "name": r.name,
                        "complete": r.complete,
                        "drives": r.drives.iter().map(|(name, influence)| {
                            serde_json::json!({"morph": name, "influence": influence})
                        }).collect::<Vec<_>>(),
                    })
                }).collect::<Vec<_>>(),
            },
        });
        let text = serde_json::to_string_pretty(&summary)
            .map_err(|err| format!("report encode: {err}"))?;
        std::fs::write(report_path, format!("{text}\n"))
            .map_err(|err| format!("cannot write report {report_path}: {err}"))?;
    }
    println!(
        "converted {} -> {} ({} bytes, sha256 {})",
        pmx_path.display(),
        out_path.display(),
        glb.len(),
        sha256_hex(&glb)
    );
    Ok(())
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) if err.starts_with("usage:") => {
            eprintln!("{err}");
            ExitCode::from(2)
        }
        Err(err) => {
            eprintln!("pmx_convert: {err}");
            ExitCode::FAILURE
        }
    }
}
