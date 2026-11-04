//! Puppet render driver (proof tool, SPEC 13): load an
//! imported puppet (model + stage + texture files), run a
//! fixed-step scenario, and save the final frame as a PNG.
//! Mirrors the reference driver's conventions: 600×800
//! canvas, camera scale 0.15, 60 Hz steps, parameter sets
//! re-applied every frame.
//!
//! Usage:
//!   puppet_render --dir <import-dir> --out <frame.png>
//!       [--frames N] [--set Name=x,y]... [--zero-skip]
//!       [--width W] [--height H] [--scale S]

use std::path::PathBuf;
use std::process::ExitCode;

use lumen_rig_core::{PuppetEval, RgbaImage, parse_model, parse_puppet};

struct Cli {
    dir: PathBuf,
    out: PathBuf,
    frames: u32,
    sets: Vec<(String, [f32; 2])>,
    zero_skip: bool,
    width: usize,
    height: usize,
    scale: f32,
}

fn parse_cli() -> Result<Cli, String> {
    let mut cli = Cli {
        dir: PathBuf::new(),
        out: PathBuf::new(),
        frames: 30,
        sets: Vec::new(),
        zero_skip: false,
        width: 600,
        height: 800,
        scale: 0.15,
    };
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--dir" => cli.dir = PathBuf::from(args.next().ok_or("--dir needs a value")?),
            "--out" => cli.out = PathBuf::from(args.next().ok_or("--out needs a value")?),
            "--frames" => {
                cli.frames = args
                    .next()
                    .ok_or("--frames needs a value")?
                    .parse()
                    .map_err(|_| "bad --frames")?;
            }
            "--set" => {
                let spec = args.next().ok_or("--set needs a value")?;
                let (name, vals) = spec.rsplit_once('=').ok_or("--set must be Name=x,y")?;
                let mut it = vals.split(',');
                let x: f32 = it.next().unwrap_or("0").parse().map_err(|_| "bad set x")?;
                let y: f32 = it
                    .next()
                    .map(|v| v.parse().map_err(|_| "bad set y"))
                    .transpose()?
                    .unwrap_or(0.0);
                cli.sets.push((name.to_string(), [x, y]));
            }
            "--zero-skip" => cli.zero_skip = true,
            "--width" => {
                cli.width = args
                    .next()
                    .ok_or("--width needs a value")?
                    .parse()
                    .map_err(|_| "bad --width")?;
            }
            "--height" => {
                cli.height = args
                    .next()
                    .ok_or("--height needs a value")?
                    .parse()
                    .map_err(|_| "bad --height")?;
            }
            "--scale" => {
                cli.scale = args
                    .next()
                    .ok_or("--scale needs a value")?
                    .parse()
                    .map_err(|_| "bad --scale")?;
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    if cli.dir.as_os_str().is_empty() || cli.out.as_os_str().is_empty() {
        return Err("--dir and --out are required".to_string());
    }
    Ok(cli)
}

fn run(cli: &Cli) -> Result<(), String> {
    let model_bytes =
        std::fs::read(cli.dir.join("model.json")).map_err(|e| format!("model read: {e}"))?;
    let model = parse_model(&model_bytes).map_err(|e| format!("model parse: {e}"))?;
    let stage_bytes =
        std::fs::read(cli.dir.join("puppet.json")).map_err(|e| format!("stage read: {e}"))?;
    let stage = parse_puppet(&stage_bytes, &model).map_err(|e| format!("stage parse: {e}"))?;

    // Resolve set names: exact stage id first, then the
    // provenance table's original names.
    let resolve = |name: &str| -> Option<String> {
        if stage.parameters.iter().any(|p| p.id == name) {
            return Some(name.to_string());
        }
        stage.provenance.as_ref().and_then(|prov| {
            prov.names
                .iter()
                .find(|pair| pair[1] == name)
                .map(|pair| pair[0].clone())
        })
    };
    let mut sets: Vec<(String, [f32; 2])> = Vec::new();
    for (name, value) in &cli.sets {
        let id = resolve(name).ok_or_else(|| format!("unknown parameter {name:?}"))?;
        sets.push((id, *value));
    }

    let mut textures: Vec<RgbaImage> = Vec::with_capacity(stage.textures.len());
    for tex in &stage.textures {
        let path = cli.dir.join(&tex.source);
        let img = image::open(&path)
            .map_err(|e| format!("texture {} decode: {e}", path.display()))?
            .to_rgba8();
        let (w, h) = (img.width() as usize, img.height() as usize);
        textures.push(RgbaImage {
            width: w,
            height: h,
            rgba: img.into_raw(),
        });
    }

    let mut eval = PuppetEval::new(&stage, &textures).map_err(|e| format!("eval init: {e}"))?;
    eval.set_zero_skip_for_proof(cli.zero_skip);
    for _ in 0..cli.frames {
        eval.begin_frame();
        for (id, value) in &sets {
            eval.set_param(id, *value)
                .map_err(|e| format!("set {id}: {e}"))?;
        }
        eval.end_frame(1.0 / 60.0)
            .map_err(|e| format!("end_frame: {e}"))?;
    }
    let frame = eval.render(cli.width, cli.height, cli.scale);
    let nontransparent = frame
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|p| !(p[0] == 0 && p[1] == 0 && p[2] == 0 && p[3] == 0))
        .count();
    let img = image::RgbaImage::from_raw(cli.width as u32, cli.height as u32, frame)
        .ok_or("frame size mismatch")?;
    img.save(&cli.out)
        .map_err(|e| format!("save {}: {e}", cli.out.display()))?;
    println!(
        "saved {} frames={} nontransparent_pixels={}",
        cli.out.display(),
        cli.frames,
        nontransparent
    );
    Ok(())
}

fn main() -> ExitCode {
    match parse_cli().and_then(|cli| run(&cli)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("puppet_render: {e}");
            ExitCode::FAILURE
        }
    }
}
