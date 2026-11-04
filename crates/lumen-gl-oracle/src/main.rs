//! D-870 GL same-route oracle. Renders one corpus pose through the
//! vendored, unmodified `inox2d-opengl` rasterizer, with geometry from
//! either evaluator:
//!
//! - `--arm inox`: inox2d's own evaluation (what the spike driver
//!   does) — arm A;
//! - `--arm lumen`: lumen's evaluation, injected into the deform
//!   buffer after inox2d's update and before draw — arm B.
//!
//! Both arms run both evaluators in lockstep on the same poses (fixed
//! 1/60 s steps from the same initial state), so every run also
//! reports the draw-trace comparison (order / masks / opacity — the
//! fields branch 1 cannot inject) and the evaluators' geometry delta.
//!
//! `--dispatch inox` draws with inox2d's own dispatcher (draw order
//! and opacity then stay inox2d-evaluated: branch 1 "position-only");
//! `--dispatch harness` drives the same unmodified renderer methods
//! in the arm's own evaluator's order/opacity (see `dispatch`).
//!
//! usage: lumen-gl-oracle --arm inox|lumen --semantics skip|apply
//!     --dispatch inox|harness --inx FILE --frames N
//!     [--set Name=x,y]... --out PNG --report TXT

#![deny(unsafe_code)]
#![deny(unsafe_op_in_unsafe_fn)]

mod dispatch;
mod gl_window;
mod inject;
mod trace;

use std::fmt::Write as _;
use std::path::PathBuf;
use std::process::ExitCode;

use glam::Vec2;
use inox2d::formats::inp::parse_inp;
use inox2d::model::Model;
use inox2d::node::InoxNodeUuid;
use inox2d::render::InoxRendererExt;
use inox2d_opengl::OpenglRenderer;
use lumen_rig_core::{PuppetEval, PuppetStage, RgbaImage, import_inp};
use winit::window::WindowBuilder;

use crate::gl_window::{App, AppFrame};

/// Fixed simulation step, the spike driver's and lumen's.
const DT: f32 = 1.0 / 60.0;
const FRAMES_MAX: u32 = 1000;
/// The corpus convention: 600×800 at camera scale 0.15.
const HEIGHT: u32 = 800;
const INX_BYTES_MAX: u64 = 256 * 1024 * 1024;
const SCALE: f32 = 0.15;
const SETS_MAX: usize = 32;
const WIDTH: u32 = 600;

#[derive(Clone, Copy, PartialEq)]
enum Arm {
    Inox,
    Lumen,
}

struct Cli {
    arm: Arm,
    frames: u32,
    /// Draw via [`dispatch`] (order/opacity from the arm's own
    /// evaluator) instead of inox2d's `InoxRendererExt::draw`.
    harness_dispatch: bool,
    inx: PathBuf,
    out: PathBuf,
    report: PathBuf,
    sets: Vec<(String, [f32; 2])>,
    zero_skip: bool,
}

fn parse_cli() -> Result<Cli, String> {
    let (mut arm, mut zero_skip, mut frames, mut harness_dispatch) = (None, None, None, None);
    let (mut inx, mut out, mut report) = (None, None, None);
    let mut sets = Vec::new();
    let mut args = std::env::args().skip(1);
    while let Some(flag) = args.next() {
        let mut value = || args.next().ok_or(format!("{flag} needs a value"));
        match flag.as_str() {
            "--arm" => {
                arm = Some(match value()?.as_str() {
                    "inox" => Arm::Inox,
                    "lumen" => Arm::Lumen,
                    other => return Err(format!("bad --arm {other}")),
                })
            }
            "--dispatch" => {
                harness_dispatch = Some(match value()?.as_str() {
                    "harness" => true,
                    "inox" => false,
                    other => return Err(format!("bad --dispatch {other}")),
                })
            }
            "--frames" => frames = Some(value()?.parse::<u32>().map_err(|_| "bad --frames")?),
            "--inx" => inx = Some(PathBuf::from(value()?)),
            "--out" => out = Some(PathBuf::from(value()?)),
            "--report" => report = Some(PathBuf::from(value()?)),
            "--semantics" => {
                zero_skip = Some(match value()?.as_str() {
                    "apply" => false,
                    "skip" => true,
                    other => return Err(format!("bad --semantics {other}")),
                })
            }
            "--set" => sets.push(parse_set(&value()?)?),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    let frames = frames.ok_or("--frames is required")?;
    if frames == 0 || frames > FRAMES_MAX || sets.len() > SETS_MAX {
        return Err(format!(
            "frames must be 1..={FRAMES_MAX}, sets at most {SETS_MAX}"
        ));
    }
    Ok(Cli {
        arm: arm.ok_or("--arm is required")?,
        frames,
        harness_dispatch: harness_dispatch.ok_or("--dispatch is required")?,
        inx: inx.ok_or("--inx is required")?,
        out: out.ok_or("--out is required")?,
        report: report.ok_or("--report is required")?,
        sets,
        zero_skip: zero_skip.ok_or("--semantics is required")?,
    })
}

fn parse_set(spec: &str) -> Result<(String, [f32; 2]), String> {
    let (name, vals) = spec.rsplit_once('=').ok_or("--set must be Name=x,y")?;
    let mut it = vals.split(',');
    let x: f32 = it.next().unwrap_or("0").parse().map_err(|_| "bad set x")?;
    let y: f32 = it
        .next()
        .map_or(Ok(0.0), str::parse)
        .map_err(|_| "bad set y")?;
    if !x.is_finite() || !y.is_finite() {
        return Err(format!("non-finite set {spec}"));
    }
    Ok((name.to_string(), [x, y]))
}

fn main() -> ExitCode {
    match parse_cli().and_then(|cli| run(cli).map_err(|e| e.to_string())) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("lumen-gl-oracle: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(cli: Cli) -> Result<(), Box<dyn std::error::Error>> {
    if std::fs::metadata(&cli.inx)?.len() > INX_BYTES_MAX {
        return Err(format!("{} exceeds {INX_BYTES_MAX} bytes", cli.inx.display()).into());
    }
    let bytes = std::fs::read(&cli.inx)?;
    let mut model = parse_inp(bytes.as_slice())?;
    model.puppet.init_transforms();
    model.puppet.init_rendering();
    model.puppet.init_params();
    model.puppet.init_physics();
    let imported = import_inp(&bytes, "oracle")?;
    let textures = decode_textures(&imported.textures)?;
    let map = inject::pair_nodes(&imported.stage, &model.puppet)?;
    let lumen_sets = resolve_sets(&imported.stage, &cli.sets)?;
    // The lumen session borrows the stage and textures for the whole
    // event loop, which never returns: leak them to 'static.
    let stage: &'static PuppetStage = Box::leak(Box::new(imported.stage));
    let textures: &'static [RgbaImage] = Box::leak(textures.into_boxed_slice());
    let mut eval = PuppetEval::new(stage, textures)?;
    eval.set_zero_skip_for_proof(cli.zero_skip);
    let frame = AppFrame::init(
        WindowBuilder::new()
            .with_transparent(true)
            .with_resizable(false)
            .with_inner_size(winit::dpi::PhysicalSize::new(WIDTH, HEIGHT))
            .with_title("lumen GL oracle"),
    )?;
    frame.run(Oracle {
        cli,
        eval,
        frame: 0,
        lumen_sets,
        map,
        model,
        readback: None,
        renderer: None,
        stage,
    })
}

fn decode_textures(
    textures: &[lumen_rig_core::ImportedTexture],
) -> Result<Vec<RgbaImage>, image::ImageError> {
    textures
        .iter()
        .map(|t| {
            // By extension, as lumen's puppet_render decodes (TGA
            // has no magic bytes to sniff).
            let format = image::ImageFormat::from_path(&t.source)?;
            let img = image::load_from_memory_with_format(&t.bytes, format)?.to_rgba8();
            Ok(RgbaImage {
                width: img.width() as usize,
                height: img.height() as usize,
                rgba: img.into_raw(),
            })
        })
        .collect()
}

/// Map `--set` names (INP display names, as the spike driver takes
/// them) to lumen stage ids: exact id, else the provenance table.
fn resolve_sets(
    stage: &PuppetStage,
    sets: &[(String, [f32; 2])],
) -> Result<Vec<(String, [f32; 2])>, String> {
    sets.iter()
        .map(|(name, value)| {
            if stage.parameters.iter().any(|p| &p.id == name) {
                return Ok((name.clone(), *value));
            }
            stage
                .provenance
                .as_ref()
                .and_then(|p| p.names.iter().find(|pair| &pair[1] == name))
                .map(|pair| (pair[0].clone(), *value))
                .ok_or(format!("unknown parameter {name:?}"))
        })
        .collect()
}

struct Oracle {
    cli: Cli,
    eval: PuppetEval<'static>,
    frame: u32,
    lumen_sets: Vec<(String, [f32; 2])>,
    map: Vec<InoxNodeUuid>,
    model: Model,
    readback: Option<glow::Context>,
    renderer: Option<OpenglRenderer>,
    stage: &'static PuppetStage,
}

impl App for Oracle {
    fn draw(&mut self) {
        if let Err(e) = self.step() {
            eprintln!("lumen-gl-oracle: {e}");
            std::process::exit(2);
        }
    }

    fn resize(&mut self, width: u32, height: u32) {
        if let Some(renderer) = &mut self.renderer {
            renderer.resize(width, height);
        }
    }

    fn resume_window(&mut self, gl: glow::Context) {
        match OpenglRenderer::new(gl, &self.model) {
            Ok(mut renderer) => {
                renderer.resize(WIDTH, HEIGHT);
                renderer.camera.scale = Vec2::splat(SCALE);
                self.renderer = Some(renderer);
            }
            Err(e) => {
                eprintln!("lumen-gl-oracle: renderer init failed: {e}");
                std::process::exit(3);
            }
        }
    }

    fn set_readback(&mut self, gl: glow::Context) {
        self.readback = Some(gl);
    }
}

impl Oracle {
    /// One fixed step of both evaluators, the injection (arm B), and
    /// the draw; on the last frame, the readback and the report.
    fn step(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        let puppet = &mut self.model.puppet;
        // Restore inox2d's own zero deform for parts without a
        // DeformStack (its update rewrites only stacked parts and
        // zero-fills them first, deform.rs:12): makes arm B's previous
        // injection invisible to this frame's inox2d state.
        let render_ctx = puppet.render_ctx.as_mut().ok_or("render ctx")?;
        render_ctx.vertex_buffers.deforms.fill(Vec2::ZERO);
        puppet.begin_frame();
        let param_ctx = puppet.param_ctx.as_mut().ok_or("param ctx")?;
        for (name, value) in &self.cli.sets {
            param_ctx.set(name, Vec2::from_array(*value))?;
        }
        puppet.end_frame(DT);
        self.eval.begin_frame();
        for (id, value) in &self.lumen_sets {
            self.eval.set_param(id, *value)?;
        }
        self.eval.end_frame(DT)?;
        let state = self.eval.oracle_state();
        let write = self.cli.arm == Arm::Lumen;
        let stats = inject::inject(self.stage, &self.map, &state, puppet, write)?;
        let recorder = trace::Recorder::default();
        recorder.draw(puppet);
        let inox_trace = recorder.events.into_inner();
        let renderer = self.renderer.as_ref().ok_or("renderer")?;
        renderer.clear();
        renderer.on_begin_draw(puppet);
        if self.cli.harness_dispatch {
            let plan = match self.cli.arm {
                Arm::Inox => dispatch::Plan::from_inox(&inox_trace),
                Arm::Lumen => dispatch::Plan::from_lumen(&state, &self.map),
            };
            dispatch::draw(renderer, puppet.node_comps(), &plan);
        } else {
            renderer.draw(puppet);
        }
        renderer.on_end_draw(puppet);
        self.frame += 1;
        if self.frame < self.cli.frames {
            return Ok(());
        }
        let lumen_trace = trace::lumen_trace(self.stage, &state, &self.map);
        let diff = trace::diff(&inox_trace, &lumen_trace);
        let gl = self.readback.as_ref().ok_or("readback context")?;
        let gl_strings = gl_window::gl_strings(gl);
        let rgba = gl_window::read_rgba(gl, WIDTH, HEIGHT);
        let opaque = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p.iter().any(|&c| c != 0))
            .count();
        image::RgbaImage::from_raw(WIDTH, HEIGHT, rgba)
            .ok_or("framebuffer size")?
            .save(&self.cli.out)?;
        let mut report = self.report(&gl_strings, &stats, &diff, opaque)?;
        report.push_str(&self.divergence_window(&inox_trace, &lumen_trace, &diff));
        std::fs::write(&self.cli.report, &report)?;
        print!("{report}");
        std::process::exit(0);
    }

    /// Both traces around the first shape divergence, with names.
    fn divergence_window(
        &self,
        inox: &[trace::DrawEvent],
        lumen: &[trace::DrawEvent],
        diff: &trace::TraceDiff,
    ) -> String {
        const CONTEXT: usize = 4;
        let Some(at) = diff.first_shape_divergence else {
            return String::new();
        };
        let tree = self.model.puppet.nodes();
        let name = |u: InoxNodeUuid| tree.get_node(u).map_or("?".into(), |n| n.name.clone());
        let mut out = String::new();
        for (label, events) in [("inox", inox), ("lumen", lumen)] {
            let lo = at.saturating_sub(CONTEXT);
            let hi = (at + CONTEXT + 1).min(events.len());
            for (i, e) in events.iter().enumerate().take(hi).skip(lo) {
                let mark = if i == at { ">>" } else { "  " };
                out.push_str(&format!(
                    "{mark} {label}[{i}] {}\n",
                    trace::describe(*e, &name)
                ));
            }
        }
        out
    }

    fn report(
        &self,
        gl: &[String; 3],
        stats: &inject::InjectStats,
        diff: &trace::TraceDiff,
        opaque: usize,
    ) -> Result<String, std::fmt::Error> {
        let mut r = String::new();
        let arm = if self.cli.arm == Arm::Lumen {
            "B-lumen"
        } else {
            "A-inox"
        };
        let semantics = if self.cli.zero_skip { "skip" } else { "apply" };
        let dispatch = if self.cli.harness_dispatch {
            "harness"
        } else {
            "inox"
        };
        writeln!(
            r,
            "arm={arm} semantics={semantics} dispatch={dispatch} frames={}",
            self.cli.frames
        )?;
        writeln!(r, "inx={} sets={:?}", self.cli.inx.display(), self.cli.sets)?;
        writeln!(r, "out={}", self.cli.out.display())?;
        writeln!(
            r,
            "GL_VENDOR={}\nGL_RENDERER={}\nGL_VERSION={}",
            gl[0], gl[1], gl[2]
        )?;
        writeln!(r, "nontransparent_px={opaque}")?;
        writeln!(r, "parts={} vertices={}", stats.parts, stats.vertices)?;
        writeln!(r, "rest_delta_max_px={:e}", stats.rest_delta_max)?;
        writeln!(r, "eval_delta_max_px={:e}", stats.eval_delta_max)?;
        let residual = if self.cli.arm == Arm::Lumen {
            format!("{:e}", stats.residual_max)
        } else {
            "n/a".into()
        };
        writeln!(r, "injection_residual_max_px={residual}")?;
        writeln!(
            r,
            "trace_events inox={} lumen={}",
            diff.events_inox, diff.events_lumen
        )?;
        writeln!(
            r,
            "trace_first_shape_divergence={:?}",
            diff.first_shape_divergence
        )?;
        writeln!(
            r,
            "trace_opacity_mismatches={} max_abs_delta={:e}",
            diff.opacity_mismatches, diff.opacity_max_abs_delta
        )?;
        writeln!(r, "trace_identical={}", diff.identical())?;
        Ok(r)
    }
}
