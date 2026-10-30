//! RENDER-1 batch driver: render one PNG frame of a lumen
//! model — at rest, at a parameter vector, or at a motion
//! time — with the CPU software rasteriser. No GPU is
//! involved at any point.
//!
//! Usage:
//!   render_frame --model <model.json> --out <frame.png>
//!     [--params <params.json>]        JSON object: parameter id -> value
//!     [--motion <motion.json> --time <seconds>]
//!     [--physics-steps <n>] [--dt <seconds>]   fixed step, default 1/120
//!     [--seed <u64>]                  physics turbulence seed, default 0
//!     [--size <pixels>]               square canvas, default 1024
//!
//! Textures resolve relative to the model file's directory
//! (the format's `source` paths). The camera is fit once,
//! at the model's rest pose, and held for the frame — a
//! batch sequence shoots through one camera. The frame's
//! FNV-1a hash is printed: the same inputs print the same
//! hash on every run (raster determinism).

use std::collections::HashMap;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use lumen_rig_core::{
    FrameMapping, MotionPlayer, Rig, decode_png, encode_png, parse_model, parse_motion,
    render_frame,
};

struct Options {
    model: PathBuf,
    out: PathBuf,
    params: Option<PathBuf>,
    motion: Option<PathBuf>,
    time: f64,
    physics_steps: usize,
    dt: f64,
    seed: u64,
    size: u32,
}

fn parse_options(args: &[String]) -> Result<Options, String> {
    let mut options = Options {
        model: PathBuf::new(),
        out: PathBuf::new(),
        params: None,
        motion: None,
        time: 0.0,
        physics_steps: 0,
        dt: 1.0 / 120.0,
        seed: 0,
        size: 1024,
    };
    let mut iter = args.iter().skip(1);
    while let Some(flag) = iter.next() {
        let value = |iter: &mut std::iter::Skip<std::slice::Iter<'_, String>>| {
            iter.next()
                .cloned()
                .ok_or_else(|| format!("{flag} needs a value"))
        };
        match flag.as_str() {
            "--model" => options.model = PathBuf::from(value(&mut iter)?),
            "--out" => options.out = PathBuf::from(value(&mut iter)?),
            "--params" => options.params = Some(PathBuf::from(value(&mut iter)?)),
            "--motion" => options.motion = Some(PathBuf::from(value(&mut iter)?)),
            "--time" => {
                options.time = value(&mut iter)?
                    .parse()
                    .map_err(|_| "--time must be a number".to_string())?
            }
            "--physics-steps" => {
                options.physics_steps = value(&mut iter)?
                    .parse()
                    .map_err(|_| "--physics-steps must be an integer".to_string())?
            }
            "--dt" => {
                options.dt = value(&mut iter)?
                    .parse()
                    .map_err(|_| "--dt must be a number".to_string())?
            }
            "--seed" => {
                options.seed = value(&mut iter)?
                    .parse()
                    .map_err(|_| "--seed must be an integer".to_string())?
            }
            "--size" => {
                options.size = value(&mut iter)?
                    .parse()
                    .map_err(|_| "--size must be an integer".to_string())?
            }
            other => return Err(format!("unknown flag {other}")),
        }
    }
    if options.model.as_os_str().is_empty() || options.out.as_os_str().is_empty() {
        return Err("--model and --out are required".to_string());
    }
    Ok(options)
}

fn main() -> ExitCode {
    let args: Vec<String> = env::args().collect();
    let options = match parse_options(&args) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("render_frame: {error}");
            eprintln!(
                "usage: render_frame --model <model.json> --out <frame.png> \
                 [--params <params.json>] [--motion <motion.json> --time <s>] \
                 [--physics-steps <n>] [--dt <s>] [--seed <n>] [--size <px>]"
            );
            return ExitCode::FAILURE;
        }
    };
    match run(&options) {
        Ok(report) => {
            println!("{report}");
            ExitCode::SUCCESS
        }
        Err(error) => {
            eprintln!("render_frame: {error}");
            ExitCode::FAILURE
        }
    }
}

fn run(options: &Options) -> Result<String, String> {
    let model_bytes = fs::read(&options.model)
        .map_err(|error| format!("cannot read {}: {error}", options.model.display()))?;
    let model = parse_model(&model_bytes).map_err(|error| format!("model refused: {error}"))?;

    // Textures resolve against the model file's directory.
    let base_dir = options
        .model
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_default();
    let mut textures = HashMap::new();
    for texture in &model.textures {
        let path = base_dir.join(&texture.source);
        let bytes = fs::read(&path)
            .map_err(|error| format!("cannot read texture {}: {error}", path.display()))?;
        let image = decode_png(&bytes)
            .map_err(|error| format!("texture {} refused: {error}", path.display()))?;
        textures.insert(texture.id.clone(), image);
    }

    let mut rig = Rig::new(model, options.seed);
    // The camera is fit at rest and held.
    let mapping = FrameMapping::fit(rig.meshes(), options.size, options.size, 0.02)
        .ok_or("the model has no rendered vertices to frame")?;

    if let Some(params_path) = &options.params {
        let text = fs::read_to_string(params_path)
            .map_err(|error| format!("cannot read {}: {error}", params_path.display()))?;
        let document: serde_json::Value =
            serde_json::from_str(&text).map_err(|error| format!("params refused: {error}"))?;
        let object = document
            .as_object()
            .ok_or("params must be a JSON object of id -> value")?;
        for (id, value) in object {
            let value = value
                .as_f64()
                .ok_or_else(|| format!("param {id}: value must be a number"))?;
            if !rig.set_parameter(id, value as f32) {
                return Err(format!("param {id}: unknown or derived parameter"));
            }
        }
    }

    if let Some(motion_path) = &options.motion {
        let motion_bytes = fs::read(motion_path)
            .map_err(|error| format!("cannot read {}: {error}", motion_path.display()))?;
        let motion = parse_motion(&motion_bytes, rig.model())
            .map_err(|error| format!("motion refused: {error}"))?;
        let mut player = MotionPlayer::new(&motion);
        player.start();
        let steps = (options.time * 120.0).round() as usize;
        for _ in 0..steps {
            player.advance(1.0 / 120.0);
            rig.apply_contributions(&player.evaluate());
        }
    }

    for _ in 0..options.physics_steps {
        rig.step(options.dt);
    }

    let frame = render_frame(rig.meshes(), &textures, &mapping)
        .map_err(|error| format!("render refused: {error}"))?;
    let png = encode_png(&frame);
    fs::write(&options.out, &png)
        .map_err(|error| format!("cannot write {}: {error}", options.out.display()))?;
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in &png {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
    }
    Ok(format!(
        "model={} size={} physics_steps={} png_bytes={} png_hash={:016x} out={}",
        rig.model().id,
        options.size,
        options.physics_steps,
        png.len(),
        hash,
        options.out.display()
    ))
}
