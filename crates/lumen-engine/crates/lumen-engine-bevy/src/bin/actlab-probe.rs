//! actlab-probe: run one contact-pair probe spec
//! (ActLab Tool 2, D-943) and write its machine-readable
//! report plus a human summary table.
//!
//! Usage:
//!   actlab-probe <spec.ron> <inx-base> <out-dir>
//!
//! `<inx-base>` resolves the spec's relative `.inx`
//! record paths. Outputs: `report.json` (the probe
//! report, byte-stable across repeat runs) and
//! `summary.txt` (per-pair aggregates + clearance).
//! Exit status is non-zero on any refusal (spec shape,
//! hash mismatch, illegal timeline, unresolvable
//! reference); the refusal is printed with its code.

use std::path::PathBuf;
use std::process::ExitCode;

use lumen_engine_bevy::probe::run_probe;
use lumen_engine_core::probe::ProbeSpec;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let (Some(spec_path), Some(inx_base), Some(out_dir)) = (args.next(), args.next(), args.next())
    else {
        eprintln!("usage: actlab-probe <spec.ron> <inx-base> <out-dir>");
        return ExitCode::from(2);
    };
    let text = match std::fs::read_to_string(&spec_path) {
        Ok(text) => text,
        Err(e) => {
            eprintln!("probe spec read: {e}");
            return ExitCode::FAILURE;
        }
    };
    let spec = match ProbeSpec::load_ron(&text) {
        Ok(spec) => spec,
        Err(e) => {
            eprintln!("probe spec refused: {e}");
            return ExitCode::FAILURE;
        }
    };
    match run_probe(&spec, &PathBuf::from(inx_base)) {
        Ok(report) => {
            let out = PathBuf::from(out_dir);
            if let Err(e) = std::fs::create_dir_all(&out) {
                eprintln!("output dir: {e}");
                return ExitCode::FAILURE;
            }
            let json = match serde_json::to_string(&report) {
                Ok(json) => json,
                Err(e) => {
                    eprintln!("report serializes: {e}");
                    return ExitCode::FAILURE;
                }
            };
            if let Err(e) = std::fs::write(out.join("report.json"), &json) {
                eprintln!("report write: {e}");
                return ExitCode::FAILURE;
            }
            let summary = summarize(&report);
            if let Err(e) = std::fs::write(out.join("summary.txt"), &summary) {
                eprintln!("summary write: {e}");
                return ExitCode::FAILURE;
            }
            println!("{summary}");
            ExitCode::SUCCESS
        }
        Err(message) => {
            eprintln!("probe run refused: {message}");
            ExitCode::FAILURE
        }
    }
}

/// The human summary table: one row per pair (engaged
/// aggregates), plus the clearance reads.
fn summarize(report: &lumen_engine_core::probe::ProbeReport) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "probe {} — {} steps\n",
        report.spec_id, report.steps
    ));
    for rig in &report.rigs {
        out.push_str(&format!(
            "rig {}: {} ({} parameters, {} parts) sha256 {}\n",
            rig.id, rig.inx, rig.parameters, rig.parts, rig.sha256
        ));
    }
    out.push_str(
        "pair | engaged steps | distance max | distance mean | band max | band mean \
         | penetration min\n",
    );
    for pair in &report.pairs {
        let fmt = |v: Option<f64>| v.map(|x| format!("{x:.6}")).unwrap_or_else(|| "-".into());
        out.push_str(&format!(
            "{} | {} | {} | {} | {} | {} | {}\n",
            pair.id,
            pair.engaged_steps,
            fmt(pair.distance.map(|a| a.max)),
            fmt(pair.distance.map(|a| a.mean)),
            fmt(pair.band_distance.map(|a| a.max)),
            fmt(pair.band_distance.map(|a| a.mean)),
            fmt(pair.penetration.map(|a| a.min)),
        ));
    }
    if let (Some(rig), Some(min)) = (report.clearance_rig, report.clearance_min_engaged) {
        out.push_str(&format!(
            "clearance rig {rig}: min over engaged steps {min:.6}\n"
        ));
    }
    for checkpoint in &report.checkpoints {
        out.push_str(&format!(
            "checkpoint {} (step {}): clearance {}\n",
            checkpoint.label,
            checkpoint.step,
            checkpoint
                .clearance
                .map(|v| format!("{v:.6}"))
                .unwrap_or_else(|| "-".into())
        ));
    }
    out
}
