//! Utilization driver for the R2 motion evaluator: load a
//! model and a motion through the public parse path, play the
//! motion on a fixed-step clock, blend every contribution
//! onto the model's rest state with the section 4.3 law, and
//! print an FNV-1a hash of the full evaluated trace.
//!
//! Usage: `evaluate_motion <model.json> <motion.json> [steps]`
//!
//! The hash is the determinism instrument: the same command on
//! the same files prints the same hash on every run and every
//! conforming machine. Steps default to 480 at 1/120 s.

use std::collections::HashMap;
use std::env;
use std::fs;
use std::process::ExitCode;

use lumen_rig_core::{MotionPlayer, Parameter, parse_model, parse_motion};

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 || args.len() > 4 {
        eprintln!("usage: evaluate_motion <model.json> <motion.json> [steps]");
        return ExitCode::FAILURE;
    }
    let steps: usize = match args.get(3) {
        Some(text) => match text.parse() {
            Ok(steps) => steps,
            Err(_) => {
                eprintln!("steps must be a non-negative integer");
                return ExitCode::FAILURE;
            }
        },
        None => 480,
    };
    let model_bytes = read_file(&args[1]);
    let motion_bytes = read_file(&args[2]);
    let (model_bytes, motion_bytes) = match (model_bytes, motion_bytes) {
        (Some(model), Some(motion)) => (model, motion),
        _ => return ExitCode::FAILURE,
    };
    let model = match parse_model(&model_bytes) {
        Ok(model) => model,
        Err(error) => {
            eprintln!("model refused: {error}");
            return ExitCode::FAILURE;
        }
    };
    let motion = match parse_motion(&motion_bytes, &model) {
        Ok(motion) => motion,
        Err(error) => {
            eprintln!("motion refused: {error}");
            return ExitCode::FAILURE;
        }
    };

    // Rest state: scalar defaults; combined parameters resolve
    // from their terms (SPEC.md section 3.1), in declaration
    // order until all resolve — the model's graph is acyclic
    // by validation, so this terminates.
    let mut values: HashMap<String, f64> = HashMap::new();
    for param in &model.parameters {
        if let Parameter::Scalar(scalar) = param {
            values.insert(scalar.id.clone(), f64::from(scalar.default));
        }
    }
    loop {
        let mut progress = false;
        for param in &model.parameters {
            if let Parameter::Combined(combined) = param
                && !values.contains_key(&combined.id)
                && combined
                    .terms
                    .iter()
                    .all(|term| values.contains_key(&term.parameter))
            {
                let value = f64::from(combined.offset)
                    + combined
                        .terms
                        .iter()
                        .map(|term| f64::from(term.weight) * values[&term.parameter])
                        .sum::<f64>();
                values.insert(combined.id.clone(), value);
                progress = true;
            }
        }
        if !progress {
            break;
        }
    }

    let mut player = MotionPlayer::new(&motion);
    player.start();
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    let mut mix = |bytes: &[u8]| {
        for byte in bytes {
            hash ^= u64::from(*byte);
            hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    for _ in 0..steps {
        player.advance(1.0 / 120.0);
        mix(&player.time_seconds().to_bits().to_le_bytes());
        for entry in player.evaluate() {
            // The host write: blend onto the current base, then
            // the sanctioned clamp into the declared range,
            // rounded once to f32 (sections 4.3, 7.2, 9.3).
            let base = values[&entry.parameter];
            let blended = MotionPlayer::blend(base, entry.weight, entry.value);
            let param = model
                .parameters
                .iter()
                .find(|param| param.id() == entry.parameter)
                .expect("a validated contribution targets a model parameter");
            let (min, max) = match param {
                Parameter::Scalar(scalar) => (f64::from(scalar.min), f64::from(scalar.max)),
                Parameter::Combined(combined) => (f64::from(combined.min), f64::from(combined.max)),
            };
            let written = blended.clamp(min, max) as f32;
            values.insert(entry.parameter.clone(), f64::from(written));
            mix(entry.parameter.as_bytes());
            mix(&written.to_bits().to_le_bytes());
        }
    }
    println!(
        "model={} motion={} steps={} trace_hash={:016x}",
        model.id, motion.id, steps, hash
    );
    ExitCode::SUCCESS
}

fn read_file(path: &str) -> Option<Vec<u8>> {
    match fs::read(path) {
        Ok(bytes) => Some(bytes),
        Err(error) => {
            eprintln!("cannot read {path}: {error}");
            None
        }
    }
}
