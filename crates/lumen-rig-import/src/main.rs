//! `rig-import` command line: `convert` (Cubism -> native),
//! `export` (native -> moc3, exporter phase E1), and `inp`
//! (INP/.inx -> native, exporter phase E2's upstream stage).
//!
//! Usage:
//! - `rig-import convert --model <dir> --license <record.json> --out <dir>`
//! - `rig-import export --model <model.json> --out <dir>
//!     --canvas-pixels-per-unit <f32> --canvas-origin-x <f32>
//!     --canvas-origin-y <f32> --canvas-width <f32>
//!     --canvas-height <f32>`
//! - `rig-import inp --inx <file.inx> --out <dir>`
//!
//! The export canvas is an exporter input (MAPPING.md Q1
//! option (a)): the native model carries no canvas, so all
//! five values are required flags — the tool invents none.

use std::path::PathBuf;
use std::process::ExitCode;

use lumen_rig_import::{
    CanvasSpec, ConversionInputs, ConvertError, Moc3Version, convert, convert_inp, export_model,
};

const USAGE: &str = "usage:\n  rig-import convert --model <dir> --license <record.json> --out <dir>\n  rig-import export --model <model.json> --out <dir> --canvas-pixels-per-unit <f32> --canvas-origin-x <f32> --canvas-origin-y <f32> --canvas-width <f32> --canvas-height <f32>\n  rig-import inp --inx <file.inx> --out <dir>";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(summary) => {
            println!("{summary}");
            ExitCode::SUCCESS
        }
        Err(ConvertError::Cli { message }) => {
            eprintln!("usage error: {message}");
            eprintln!("{USAGE}");
            ExitCode::from(2)
        }
        Err(error) => {
            eprintln!("failed: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<String, ConvertError> {
    let mut rest = args.iter();
    match rest.next().map(String::as_str) {
        Some("convert") => run_convert(&mut rest),
        Some("export") => run_export(&mut rest),
        Some("inp") => run_inp(&mut rest),
        _ => Err(ConvertError::Cli {
            message: "expected the `convert`, `export`, or `inp` subcommand".to_string(),
        }),
    }
}

/// Take the value for the flag just read from the stream.
fn next_value<'a>(
    rest: &mut impl Iterator<Item = &'a String>,
    flag: &str,
) -> Result<&'a str, ConvertError> {
    rest.next()
        .map(|value| value.as_str())
        .ok_or_else(|| ConvertError::Cli {
            message: format!("flag {flag} needs a value"),
        })
}

fn run_convert<'a>(rest: &mut impl Iterator<Item = &'a String>) -> Result<String, ConvertError> {
    let mut model: Option<PathBuf> = None;
    let mut license: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    while let Some(flag) = rest.next().map(String::as_str) {
        let value = next_value(rest, flag)?;
        match flag {
            "--model" => model = Some(PathBuf::from(value)),
            "--license" => license = Some(PathBuf::from(value)),
            "--out" => out = Some(PathBuf::from(value)),
            other => {
                return Err(ConvertError::Cli {
                    message: format!("unknown flag {other}"),
                });
            }
        }
    }
    let inputs = ConversionInputs {
        model_dir: model.ok_or_else(|| ConvertError::Cli {
            message: "--model <dir> is required".to_string(),
        })?,
        license_path: license.ok_or_else(|| ConvertError::Cli {
            message: "--license <record.json> is required".to_string(),
        })?,
        out_dir: out.ok_or_else(|| ConvertError::Cli {
            message: "--out <dir> is required".to_string(),
        })?,
    };
    let outcome = convert(&inputs)?;
    let converted = &outcome.report.converted;
    Ok(format!(
        "converted {}: {} parameters, {} parts, {} deformers, {} meshes, {} physics groups, {} motions, {} expressions; {} files written to {}",
        outcome.model_id,
        converted.parameters,
        converted.parts,
        converted.deformers,
        converted.meshes,
        converted.physics_groups,
        converted.motions,
        converted.expressions,
        outcome.files_written.len(),
        inputs.out_dir.display()
    ))
}

fn run_inp<'a>(rest: &mut impl Iterator<Item = &'a String>) -> Result<String, ConvertError> {
    let mut inx: Option<PathBuf> = None;
    let mut out_dir: Option<PathBuf> = None;
    while let Some(flag) = rest.next().map(String::as_str) {
        let value = next_value(rest, flag)?;
        match flag {
            "--inx" => inx = Some(PathBuf::from(value)),
            "--out" => out_dir = Some(PathBuf::from(value)),
            other => {
                return Err(ConvertError::Cli {
                    message: format!("unknown flag {other}"),
                });
            }
        }
    }
    let inx = inx.ok_or_else(|| ConvertError::Cli {
        message: "--inx <file.inx> is required".to_string(),
    })?;
    let out_dir = out_dir.ok_or_else(|| ConvertError::Cli {
        message: "--out <dir> is required".to_string(),
    })?;
    let model_id = inx
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .ok_or_else(|| ConvertError::Cli {
            message: format!("cannot derive a model id from {}", inx.display()),
        })?
        .to_string();
    let bytes = std::fs::read(&inx).map_err(|error| ConvertError::Io {
        path: inx.clone(),
        message: error.to_string(),
    })?;
    let outcome = convert_inp(&bytes, &model_id)?;
    std::fs::create_dir_all(&out_dir).map_err(|error| ConvertError::Io {
        path: out_dir.clone(),
        message: error.to_string(),
    })?;
    let model_path = out_dir.join(format!("{model_id}.model.json"));
    std::fs::write(&model_path, outcome.model.to_canonical_json()).map_err(|error| {
        ConvertError::Io {
            path: model_path.clone(),
            message: error.to_string(),
        }
    })?;
    let report_path = out_dir.join(format!("{model_id}.inp-report.json"));
    std::fs::write(&report_path, outcome.report.to_canonical_json()).map_err(|error| {
        ConvertError::Io {
            path: report_path.clone(),
            message: error.to_string(),
        }
    })?;
    let counts = &outcome.report.counts;
    Ok(format!(
        "ingested {} from INP: {} parameters ({} source), {} parts, {} deformers, {} meshes, {} physics groups from {} nodes and {} bindings; model written to {} and report to {}",
        model_id,
        counts.native_parameters,
        counts.inp_parameters,
        counts.native_parts,
        counts.native_deformers,
        counts.native_meshes,
        counts.physics_groups,
        counts.inp_nodes,
        counts.bindings_total,
        model_path.display(),
        report_path.display()
    ))
}

fn parse_f32_flag(flag: &str, value: &str) -> Result<f32, ConvertError> {
    value.parse::<f32>().map_err(|_| ConvertError::Cli {
        message: format!("flag {flag} needs a number, got {value:?}"),
    })
}

fn run_export<'a>(rest: &mut impl Iterator<Item = &'a String>) -> Result<String, ConvertError> {
    let mut model_path: Option<PathBuf> = None;
    let mut out_dir: Option<PathBuf> = None;
    let mut pixels_per_unit: Option<f32> = None;
    let mut origin_x: Option<f32> = None;
    let mut origin_y: Option<f32> = None;
    let mut width: Option<f32> = None;
    let mut height: Option<f32> = None;
    while let Some(flag) = rest.next().map(String::as_str) {
        let value = next_value(rest, flag)?;
        match flag {
            "--model" => model_path = Some(PathBuf::from(value)),
            "--out" => out_dir = Some(PathBuf::from(value)),
            "--canvas-pixels-per-unit" => {
                pixels_per_unit = Some(parse_f32_flag(flag, value)?);
            }
            "--canvas-origin-x" => origin_x = Some(parse_f32_flag(flag, value)?),
            "--canvas-origin-y" => origin_y = Some(parse_f32_flag(flag, value)?),
            "--canvas-width" => width = Some(parse_f32_flag(flag, value)?),
            "--canvas-height" => height = Some(parse_f32_flag(flag, value)?),
            other => {
                return Err(ConvertError::Cli {
                    message: format!("unknown flag {other}"),
                });
            }
        }
    }
    let required = |value: Option<f32>, flag: &str| {
        value.ok_or_else(|| ConvertError::Cli {
            message: format!("{flag} <f32> is required (canvas is an exporter input)"),
        })
    };
    let canvas = CanvasSpec {
        pixels_per_unit: required(pixels_per_unit, "--canvas-pixels-per-unit")?,
        origin_x: required(origin_x, "--canvas-origin-x")?,
        origin_y: required(origin_y, "--canvas-origin-y")?,
        width: required(width, "--canvas-width")?,
        height: required(height, "--canvas-height")?,
    };
    let model_path = model_path.ok_or_else(|| ConvertError::Cli {
        message: "--model <model.json> is required".to_string(),
    })?;
    let out_dir = out_dir.ok_or_else(|| ConvertError::Cli {
        message: "--out <dir> is required".to_string(),
    })?;
    let text = std::fs::read_to_string(&model_path).map_err(|error| ConvertError::Io {
        path: model_path.clone(),
        message: error.to_string(),
    })?;
    let model = lumen_rig_core::parse_model(text.as_bytes())?;
    let outcome = export_model(&model, &canvas, Moc3Version::V3_0)?;
    std::fs::create_dir_all(&out_dir).map_err(|error| ConvertError::Io {
        path: out_dir.clone(),
        message: error.to_string(),
    })?;
    let moc3_path = out_dir.join(format!("{}.moc3", model.id));
    std::fs::write(&moc3_path, &outcome.bytes).map_err(|error| ConvertError::Io {
        path: moc3_path.clone(),
        message: error.to_string(),
    })?;
    let report_path = out_dir.join(format!("{}.export-report.json", model.id));
    std::fs::write(&report_path, outcome.report.to_canonical_json()).map_err(|error| {
        ConvertError::Io {
            path: report_path.clone(),
            message: error.to_string(),
        }
    })?;
    let counts = &outcome.report.counts;
    Ok(format!(
        "exported {} (moc3 {}): {} parameters, {} parts, {} deformers, {} meshes; {} bytes written to {} and report to {}",
        model.id,
        outcome.report.moc3_version,
        counts.parameters,
        counts.parts,
        counts.deformers,
        counts.art_meshes,
        outcome.bytes.len(),
        moc3_path.display(),
        report_path.display()
    ))
}
