//! lumen-engine-bevy: the minimal Bevy host binding for
//! lumen-engine-core (SPEC section 4.3).
//!
//! The binding is deliberately small: a headless Bevy
//! `App` whose `Update` schedule advances a phase machine
//! — load staging, tune, simulate pass 1 (rendering key
//! frames and stepping the core runtime in lockstep),
//! simulate pass 2 (worst frames + the in-process
//! determinism assert), summarize and write outputs. The
//! host never consults a wall clock: each `update()` runs
//! one phase, and the scene's own fixed-step counter is
//! the only time in the building.
//!
//! Rendering binds the evaluator of record
//! (lumen-rig-core's puppet stage) under the solved
//! placements; instruments measure the solved frames.
//! Nothing here evaluates a rig — the engine consumes
//! the evaluator exactly as the games do (DP-8).

pub mod instruments;
pub mod render;
pub mod staging;

use std::path::PathBuf;

use bevy_app::{App, Update};
use bevy_ecs::prelude::{ResMut, Resource};
use lumen_engine_core::contact::{ContactDef, Scene};
use lumen_engine_core::{Runtime, SceneRecord, StateRecord};
use serde_json::json;

use crate::instruments::percentile;
use crate::render::{annotate, composite};
use crate::staging::{
    build_contact_scene, load_staging, run_tune, simulate, verify_family_consistency, PassOut,
    Staging, TuneOutcome,
};

/// A host job: run one scene record to completion.
pub struct HostJob {
    /// The scene record.
    pub scene: SceneRecord,
    /// Base directory the actors' rig paths resolve
    /// against.
    pub imports_base: PathBuf,
    /// Output directory (created).
    pub out_dir: PathBuf,
    /// Optional reference frame directory (the reads of
    /// record's verdict pack) for pixel deltas.
    pub reference_dir: Option<PathBuf>,
    /// Step at which the runtime state snapshot/restore
    /// probe runs.
    pub snapshot_step: u64,
}

/// What a finished host run produced.
pub struct HostReport {
    /// The verdict-pack-shaped summary document.
    pub summary: serde_json::Value,
    /// The engine evidence document.
    pub evidence: serde_json::Value,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Load,
    Tune,
    Pass1,
    Pass2,
    Finish,
    Done,
}

/// Host state as a Bevy resource: the phase machine's
/// working set.
#[derive(Resource)]
pub struct HostState {
    job: HostJob,
    phase: Phase,
    result: Option<Result<HostReport, String>>,
    staging: Option<Staging>,
    tune: Option<TuneOutcome>,
    contact_scene: Option<Scene>,
    contacts: Vec<ContactDef>,
    offsets: [[f64; 2]; 2],
    tracks: Vec<Vec<lumen_engine_core::ResolvedPoseKey>>,
    centre: [f64; 2],
    pass1: Option<PassOut>,
    frames_written: Vec<String>,
    runtime_log: Vec<(u64, String)>,
    snapshot: Option<StateRecord>,
    tune_diffs: Vec<String>,
}

/// Build the Bevy app for a job.
pub fn build_app(job: HostJob) -> App {
    let mut app = App::new();
    app.insert_resource(HostState {
        job,
        phase: Phase::Load,
        result: None,
        staging: None,
        tune: None,
        contact_scene: None,
        contacts: Vec::new(),
        offsets: [[0.0; 2]; 2],
        tracks: Vec::new(),
        centre: [0.0, 0.0],
        pass1: None,
        frames_written: Vec::new(),
        runtime_log: Vec::new(),
        snapshot: None,
        tune_diffs: Vec::new(),
    });
    app.add_systems(Update, step_system);
    app
}

/// Run a job to completion and return its report.
pub fn run(job: HostJob) -> Result<HostReport, String> {
    let mut app = build_app(job);
    for _ in 0..64 {
        app.update();
        let done = app
            .world()
            .get_resource::<HostState>()
            .is_some_and(|s| s.result.is_some());
        if done {
            break;
        }
    }
    let mut state = app
        .world_mut()
        .remove_resource::<HostState>()
        .ok_or("host state missing")?;
    match state.result.take() {
        Some(Ok(report)) => Ok(report),
        Some(Err(message)) => Err(message),
        None => Err("host did not finish within the phase budget".to_string()),
    }
}

fn step_system(mut state: ResMut<HostState>) {
    if state.phase == Phase::Done {
        return;
    }
    let outcome = advance(&mut state);
    if let Err(message) = outcome {
        state.result = Some(Err(message));
        state.phase = Phase::Done;
    }
}

fn advance(state: &mut HostState) -> Result<(), String> {
    match state.phase {
        Phase::Load => {
            std::fs::create_dir_all(&state.job.out_dir).map_err(|e| e.to_string())?;
            let staging = load_staging(&state.job.scene, &state.job.imports_base)?;
            println!(
                "roles: A (lifter) = {} H={:.1} foot_y={:.1}; B (lifted) = {} H={:.1} foot_y={:.1}",
                state.job.scene.actors[0].display,
                staging.bounds[0].height,
                staging.bounds[0].foot_y,
                state.job.scene.actors[1].display,
                staging.bounds[1].height,
                staging.bounds[1].foot_y
            );
            state.staging = Some(staging);
            state.phase = Phase::Tune;
        }
        Phase::Tune => {
            let scene = &state.job.scene;
            let staging = state.staging.as_ref().expect("staging loaded");
            let tune_record = scene
                .tune
                .as_ref()
                .ok_or("this host stages tuned scenes; the record carries no tune block")?;
            let outcome = run_tune(scene, tune_record, staging)?;
            verify_family_consistency(scene, tune_record, &outcome)?;
            let (contact_scene, contacts, offsets) = build_contact_scene(scene, &outcome, staging)?;
            state.tune_diffs = outcome.diff_expected(&tune_record.expected, offsets);
            let names = outcome.names();
            let mut tracks = Vec::new();
            for actor in &scene.actors {
                tracks.push(
                    lumen_engine_core::resolve_pose_track(&actor.pose_track, &names)
                        .map_err(|e| e.to_string())?,
                );
            }
            state.centre = [outcome.b_x / 2.0, staging.bounds[1].height / 2.0];
            state.tune = Some(outcome);
            state.contact_scene = Some(contact_scene);
            state.contacts = contacts;
            state.offsets = offsets;
            state.tracks = tracks;
            state.phase = Phase::Pass1;
        }
        Phase::Pass1 => {
            let out_dir = state.job.out_dir.clone();
            let centre = state.centre;
            let key_frames = state.job.scene.key_frames.clone();
            let canvas = state.job.scene.canvas;
            let contacts = state.contacts.clone();
            let cfg = staging::SimCfg {
                record: &state.job.scene,
                staging: state.staging.as_ref().expect("staging"),
                contact_scene: state.contact_scene.as_ref().expect("contact scene"),
                tune: state.tune.as_ref().expect("tune"),
                tracks: &state.tracks,
                hand_parts: state
                    .job
                    .scene
                    .tune
                    .as_ref()
                    .expect("tune record")
                    .hand_parts
                    .clone(),
            };
            // The core runtime steps in lockstep with the
            // scene; its events are the probe log, and a
            // snapshot at the probe step feeds the restore
            // check in the Finish phase.
            let mut runtime = Runtime::load(&state.job.scene).map_err(|e| e.to_string())?;
            let snapshot_step = state.job.snapshot_step;
            let mut log: Vec<(u64, String)> = Vec::new();
            let mut snapshot: Option<StateRecord> = None;
            let mut frames: Vec<String> = Vec::new();
            let pass = simulate(
                &cfg,
                |step, eval_a, eval_b, read, geo_a, geo_b| {
                    if let Some(frame) = key_frames.iter().find(|f| f.step == step) {
                        let img = composite(
                            eval_a,
                            eval_b,
                            read,
                            centre,
                            canvas.width as usize,
                            canvas.height as usize,
                            canvas.scale,
                        );
                        let mut rgb =
                            image::RgbImage::from_fn(canvas.width, canvas.height, |x, y| {
                                let p = img.get_pixel(x, y);
                                image::Rgb([p[0], p[1], p[2]])
                            });
                        annotate(
                            &mut rgb,
                            read,
                            geo_a,
                            geo_b,
                            &contacts,
                            centre,
                            canvas.width as usize,
                            canvas.height as usize,
                            canvas.scale,
                        );
                        let name = format!("frame-{}.png", frame.label);
                        rgb.save(out_dir.join(&name)).expect("key frame saves");
                        frames.push(name);
                    }
                },
                |_step| {
                    let events = runtime.step();
                    for line in format_events(&events) {
                        log.push((events.step, line));
                    }
                    if runtime.clock().step() == snapshot_step && snapshot.is_none() {
                        snapshot = Some(runtime.snapshot());
                    }
                },
            )?;
            state.runtime_log = log;
            state.snapshot = snapshot;
            state.frames_written.extend(frames);
            state.pass1 = Some(pass);
            state.phase = Phase::Pass2;
        }
        Phase::Pass2 => {
            let out_dir = state.job.out_dir.clone();
            let centre = state.centre;
            let canvas = state.job.scene.canvas;
            let contacts = state.contacts.clone();
            let pass1 = state.pass1.as_ref().expect("pass 1 done");
            // Worst-frame selection, the seam's rule:
            // largest engaged slip over 0.03% of A's
            // height, else smallest margin; plus the
            // whole-scene margin worst and the
            // approach-window (first 3 s) margin worst.
            let by_slip = pass1
                .reads
                .iter()
                .enumerate()
                .map(|(i, r)| {
                    (
                        r.contacts
                            .iter()
                            .filter(|c| c.engaged)
                            .map(|c| c.slip)
                            .fold(0.0f64, f64::max),
                        i,
                    )
                })
                .fold((0.0f64, 0usize), |acc, x| if x.0 > acc.0 { x } else { acc });
            let (margin_worst, margin_worst_step) =
                pass1
                    .margins
                    .iter()
                    .enumerate()
                    .fold((f64::INFINITY, 0usize), |acc, (i, &m)| {
                        if m < acc.0 {
                            (m, i)
                        } else {
                            acc
                        }
                    });
            let approach_window = (3 * state.job.scene.rate) as usize;
            let (approach_worst, approach_worst_step) = pass1.margins[..approach_window]
                .iter()
                .enumerate()
                .fold((f64::INFINITY, 0usize), |acc, (i, &m)| {
                    if m < acc.0 {
                        (m, i)
                    } else {
                        acc
                    }
                });
            let _ = (margin_worst, approach_worst);
            let worst_step =
                if by_slip.0 > 0.0003 * state.staging.as_ref().expect("staging").bounds[0].height {
                    by_slip.1
                } else {
                    margin_worst_step
                };
            let mut wanted: Vec<(u64, String)> = vec![
                (worst_step as u64, "worst".to_string()),
                (margin_worst_step as u64, "margin-worst".to_string()),
                (approach_worst_step as u64, "approach-worst".to_string()),
            ];
            wanted.sort();
            wanted.dedup();
            let cfg = staging::SimCfg {
                record: &state.job.scene,
                staging: state.staging.as_ref().expect("staging"),
                contact_scene: state.contact_scene.as_ref().expect("contact scene"),
                tune: state.tune.as_ref().expect("tune"),
                tracks: &state.tracks,
                hand_parts: state
                    .job
                    .scene
                    .tune
                    .as_ref()
                    .expect("tune record")
                    .hand_parts
                    .clone(),
            };
            let mut frames: Vec<String> = Vec::new();
            let pass2 = simulate(
                &cfg,
                |step, eval_a, eval_b, read, geo_a, geo_b| {
                    if let Some((_, label)) = wanted.iter().find(|(s, _)| *s == step) {
                        let img = composite(
                            eval_a,
                            eval_b,
                            read,
                            centre,
                            canvas.width as usize,
                            canvas.height as usize,
                            canvas.scale,
                        );
                        let mut rgb =
                            image::RgbImage::from_fn(canvas.width, canvas.height, |x, y| {
                                let p = img.get_pixel(x, y);
                                image::Rgb([p[0], p[1], p[2]])
                            });
                        annotate(
                            &mut rgb,
                            read,
                            geo_a,
                            geo_b,
                            &contacts,
                            centre,
                            canvas.width as usize,
                            canvas.height as usize,
                            canvas.scale,
                        );
                        let name = format!("frame-{label}.png");
                        rgb.save(out_dir.join(&name)).expect("worst frame saves");
                        frames.push(name);
                    }
                },
                |_| {},
            )?;
            // In-process determinism: pass 2 reads must
            // reproduce pass 1 exactly.
            let reads1 = serde_json::to_string(&state.pass1.as_ref().expect("pass 1").reads)
                .map_err(|e| e.to_string())?;
            let reads2 = serde_json::to_string(&pass2.reads).map_err(|e| e.to_string())?;
            if reads1 != reads2 {
                return Err("pass 2 reads diverge from pass 1 (in-process determinism)".into());
            }
            state.frames_written.extend(frames);
            state.phase = Phase::Finish;
        }
        Phase::Finish => {
            let report = finish(state)?;
            state.result = Some(Ok(report));
            state.phase = Phase::Done;
        }
        Phase::Done => {}
    }
    Ok(())
}

/// Format one step's runtime events as probe-log lines.
fn format_events(events: &lumen_engine_core::StepEvents) -> Vec<String> {
    let mut lines = Vec::new();
    for event in &events.dialogue {
        lines.push(format!("dialogue {event:?}"));
    }
    for event in &events.effects {
        lines.push(format!(
            "effect {} key {} {:?} count {} seed {}",
            event.effect_id, event.key_index, event.kind, event.count, event.seed
        ));
    }
    for event in &events.audio {
        lines.push(format!(
            "audio {} bus {} gain {}",
            event.cue, event.bus, event.gain_db
        ));
    }
    lines
}

/// The Finish phase: restore probe, summary + evidence
/// assembly, output writes.
fn finish(state: &mut HostState) -> Result<HostReport, String> {
    let scene = &state.job.scene;
    let staging = state.staging.as_ref().expect("staging");
    let tune = state.tune.as_ref().expect("tune");
    let pass1 = state.pass1.as_ref().expect("pass 1");
    let out = &state.job.out_dir;

    // Restore probe: a fresh runtime restored from the
    // snapshot must reproduce the uninterrupted event
    // stream from the snapshot step.
    let snapshot = state
        .snapshot
        .as_ref()
        .ok_or("runtime snapshot was not taken")?;
    let mut restored = Runtime::restore(scene, snapshot).map_err(|e| e.to_string())?;
    let mut tail: Vec<(u64, String)> = Vec::new();
    for _ in snapshot.step..scene.steps {
        let events = restored.step();
        for line in format_events(&events) {
            tail.push((events.step, line));
        }
    }
    let expected_tail: Vec<(u64, String)> = state
        .runtime_log
        .iter()
        .filter(|(step, _)| *step >= snapshot.step)
        .cloned()
        .collect();
    let restore_matches = tail == expected_tail;

    // reads.json + the state record + the probe log.
    std::fs::write(
        out.join("reads.json"),
        serde_json::to_string(&pass1.reads).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    std::fs::write(
        out.join(format!("state-at-{}.ron", snapshot.step)),
        snapshot.to_ron(),
    )
    .map_err(|e| e.to_string())?;
    let log_text: String = state
        .runtime_log
        .iter()
        .map(|(step, line)| format!("step {step} {line}\n"))
        .collect();
    std::fs::write(out.join("runtime-events.log"), log_text).map_err(|e| e.to_string())?;

    let summary = build_summary(scene, staging, tune, state.offsets, pass1);
    std::fs::write(
        out.join("summary.json"),
        serde_json::to_string_pretty(&summary).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;

    // Frame deltas against the reference pack, when given.
    let frame_deltas = match &state.job.reference_dir {
        Some(reference) => {
            let mut deltas = serde_json::Map::new();
            for (mine, reference_name) in frame_reference_pairs(scene, pass1) {
                let delta = pixel_delta(&out.join(&mine), &reference.join(&reference_name))?;
                deltas.insert(mine, delta);
            }
            json!(deltas)
        }
        None => json!(null),
    };

    let evidence = json!({
        "scene": scene.id,
        "steps": scene.steps,
        "rate": scene.rate,
        "tune_check": {
            "matches_expected": state.tune_diffs.is_empty(),
            "diffs": state.tune_diffs,
        },
        "family_consistency": "verified",
        "runtime_probe": {
            "snapshot_step": snapshot.step,
            "state_hash": snapshot.hash(),
            "events_total": state.runtime_log.len(),
            "restore_event_stream_matches": restore_matches,
        },
        "determinism_in_process": "pass2 reads byte-identical to pass1 reads",
        "frames_written": state.frames_written,
        "frame_deltas_vs_reference": frame_deltas,
    });
    std::fs::write(
        out.join("evidence.json"),
        serde_json::to_string_pretty(&evidence).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    Ok(HostReport { summary, evidence })
}

/// (my frame, reference frame) pairs for the pixel-delta
/// comparison. The verdict pack keeps the seam driver's
/// own frame names, which are exactly the names this host
/// writes (`frame-<label>.png`).
fn frame_reference_pairs(scene: &SceneRecord, _pass1: &PassOut) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    for frame in &scene.key_frames {
        let name = format!("frame-{}.png", frame.label);
        pairs.push((name.clone(), name));
    }
    pairs.push(("frame-worst.png".to_string(), "frame-worst.png".to_string()));
    pairs.push((
        "frame-margin-worst.png".to_string(),
        "frame-margin-worst.png".to_string(),
    ));
    pairs.push((
        "frame-approach-worst.png".to_string(),
        "frame-approach-worst.png".to_string(),
    ));
    pairs
}

/// Differing-pixel count between two PNGs of equal size.
fn pixel_delta(
    mine: &std::path::Path,
    reference: &std::path::Path,
) -> Result<serde_json::Value, String> {
    let a = image::open(mine).map_err(|e| e.to_string())?.to_rgba8();
    let b = image::open(reference)
        .map_err(|e| e.to_string())?
        .to_rgba8();
    if (a.width(), a.height()) != (b.width(), b.height()) {
        return Ok(json!({
            "reference": reference.file_name().map(|n| n.to_string_lossy().into_owned()),
            "error": "dimension mismatch",
        }));
    }
    let mut differing = 0u64;
    for (pa, pb) in a.pixels().zip(b.pixels()) {
        if pa != pb {
            differing += 1;
        }
    }
    Ok(json!({
        "reference": reference.file_name().map(|n| n.to_string_lossy().into_owned()),
        "differing_pixels": differing,
        "total_pixels": a.width() as u64 * a.height() as u64,
    }))
}

/// The verdict-pack-shaped summary (the seam driver's
/// measurements section, value-for-value).
fn build_summary(
    scene: &SceneRecord,
    staging: &Staging,
    tune: &TuneOutcome,
    offsets: [[f64; 2]; 2],
    pass1: &PassOut,
) -> serde_json::Value {
    let bounds_a = &staging.bounds[0];
    let bounds_b = &staging.bounds[1];
    let phase = |lo: usize, hi: usize| -> (f64, f64) {
        let mut slips: Vec<f64> = pass1.reads[lo..hi]
            .iter()
            .flat_map(|r| r.contacts.iter().filter(|c| c.engaged).map(|c| c.slip))
            .collect();
        slips.sort_by(f64::total_cmp);
        (
            percentile(&slips, 0.95),
            slips.last().copied().unwrap_or(f64::NAN),
        )
    };
    let (engage_p95, engage_max) = phase(120, 240);
    let (lift_p95, lift_max) = phase(240, 480);
    let (hold_p95, hold_max) = phase(480, 600);
    let release_max = pass1.reads[600..720]
        .iter()
        .flat_map(|r| r.contacts.iter().map(|c| c.slip))
        .fold(0.0f64, f64::max);
    let engaged_max = pass1
        .reads
        .iter()
        .flat_map(|r| r.contacts.iter().filter(|c| c.engaged).map(|c| c.slip))
        .fold(0.0f64, f64::max);
    let window_min = |series: &[f64], lo: usize, hi: usize| -> (f64, usize) {
        series[lo..hi]
            .iter()
            .enumerate()
            .fold((f64::INFINITY, 0usize), |acc, (i, &m)| {
                if m < acc.0 {
                    (m, i + lo)
                } else {
                    acc
                }
            })
    };
    let (engaged_margin, engaged_margin_step) = window_min(&pass1.margins, 120, 600);
    let (approach_margin, approach_margin_step) = window_min(&pass1.margins, 0, 120);
    let (engaged_margin_nh, engaged_margin_nh_step) = window_min(&pass1.margins_no_hands, 120, 600);
    let (margin_worst, margin_worst_step) =
        pass1
            .margins
            .iter()
            .enumerate()
            .fold((f64::INFINITY, 0usize), |acc, (i, &m)| {
                if m < acc.0 {
                    (m, i)
                } else {
                    acc
                }
            });
    let approach_window = (3 * scene.rate) as usize;
    let (approach_worst, approach_worst_step) = pass1.margins[..approach_window]
        .iter()
        .enumerate()
        .fold((f64::INFINITY, 0usize), |acc, (i, &m)| {
            if m < acc.0 {
                (m, i)
            } else {
                acc
            }
        });
    let by_slip = pass1
        .reads
        .iter()
        .enumerate()
        .map(|(i, r)| {
            (
                r.contacts
                    .iter()
                    .filter(|c| c.engaged)
                    .map(|c| c.slip)
                    .fold(0.0f64, f64::max),
                i,
            )
        })
        .fold((0.0f64, 0usize), |acc, x| if x.0 > acc.0 { x } else { acc });
    let worst_step = if by_slip.0 > 0.0003 * bounds_a.height {
        by_slip.1
    } else {
        margin_worst_step
    };
    let root_a: Vec<f64> = pass1.reads.iter().map(|r| r.roots[0][1]).collect();
    let root_b: Vec<f64> = pass1.reads.iter().map(|r| r.roots[1][1]).collect();
    let rest_a = root_a[..60].iter().sum::<f64>() / 60.0;
    let rest_b = root_b[..60].iter().sum::<f64>() / 60.0;
    let dip = root_a[228..300]
        .iter()
        .fold(f64::INFINITY, |x, &y| x.min(y));
    let body_rest = pass1.body_a_y[..60].iter().sum::<f64>() / 60.0;
    let body_dip = pass1.body_a_y[120..240]
        .iter()
        .fold(f64::INFINITY, |x, &y| x.min(y));
    let converged_seam = pass1.reads.iter().filter(|r| r.converged).count();
    let converged_equiv = pass1
        .reads
        .iter()
        .zip(pass1.margins_no_hands.iter())
        .filter(|(r, m)| r.contacts.iter().all(|c| !c.engaged || c.slip <= 1e-6) && **m >= 0.0)
        .count();
    let hw_at = |step: usize| pass1.hand_waist[step];
    let hw_engaged_max = |hand: usize| {
        pass1.hand_waist[120..600]
            .iter()
            .map(|h| h[hand])
            .fold(0.0f64, f64::max)
    };
    json!({
        "roles": {"a_lifter": scene.actors[0].display, "b_lifted": scene.actors[1].display,
                  "height_a_units": bounds_a.height, "height_b_units": bounds_b.height,
                  "foot_y_a": bounds_a.foot_y, "foot_y_b": bounds_b.foot_y},
        "tune": {"b_x": tune.b_x, "target_row_y_b_local": tune.target_row_y,
                 "targets_b_local": {"hand_l": tune.target_left, "hand_r": tune.target_right},
                 "predicted_slip": tune.predicted_slip,
                 "pre_solve_slip_at_grasp": tune.pre_solve_slip,
                 "grasp_arm_l": tune.grasp_arm_l, "crouch_depth": tune.crouch_depth,
                 "turn_lift_compensation_units": tune.turn_lift,
                 "crouch_drop_units": tune.crouch_drop,
                 "hand_rise_grasp_to_lift_units": tune.hand_rise,
                 "predicted_clearance_units": tune.predicted_clearance,
                 "waist_band_b_local": tune.waist_band,
                 "contact_offsets": [offsets[0], offsets[1]]},
        "c1_slip_units": {
            "engage_2_4s": {"p95": engage_p95, "max": engage_max},
            "lift_4_8s": {"p95": lift_p95, "max": lift_max},
            "hold_8_10s": {"p95": hold_p95, "max": hold_max},
            "post_release_drift_max": release_max,
            "engaged_max_all": engaged_max,
            "engaged_max_pct_height_b": engaged_max / bounds_b.height * 100.0},
        "hand_to_waist_band_units": {
            "approach_step_60": hw_at(60),
            "grasp_step_120": hw_at(120),
            "crouch_step_180": hw_at(180),
            "lift_step_300": hw_at(300),
            "turn_step_390": hw_at(390),
            "setdown_step_570": hw_at(570),
            "release_step_660": hw_at(660),
            "engaged_max_2_10s": [hw_engaged_max(0), hw_engaged_max(1)]},
        "c2_min_margin_triangle_metric": {
            "all": {"value": margin_worst, "step": margin_worst_step},
            "approach_0_2s": {"value": approach_margin, "step": approach_margin_step},
            "engaged_2_10s": {"value": engaged_margin, "step": engaged_margin_step},
            "engaged_2_10s_no_hand_points": {"value": engaged_margin_nh, "step": engaged_margin_nh_step},
            "approach_grasp_window_0_3s": {"value": approach_worst, "step": approach_worst_step}},
        "c4_weight_transfer": {
            "a_root_rest_y": rest_a,
            "a_root_dip_y": dip,
            "a_root_dip_pct_height": (rest_a - dip) / bounds_a.height * 100.0,
            "a_body_centroid_rest_y": body_rest,
            "a_body_centroid_dip_y": body_dip,
            "a_root_y_at_turn_step_390": root_a[390],
            "b_root_rest_y": rest_b,
            "b_root_y_at_turn_step_390": root_b[390],
            "b_root_y_at_setdown_step_570": root_b[570],
            "b_foot_clearance_at_hold_step_300": pass1.b_clearance[300],
            "b_foot_clearance_min_hold_ramp_300_390": pass1.b_clearance[300..390].iter().fold(f64::INFINITY, |x, &y| x.min(y)),
            "b_foot_clearance_min_turn_390_480": pass1.b_clearance[390..480].iter().fold(f64::INFINITY, |x, &y| x.min(y)),
            "foot_min_y_all_steps": pass1.foot_min,
            "foot_min_y_step": pass1.foot_min_step,
            "foot_min_y_rig_a": pass1.foot_min_a,
            "foot_min_y_rig_b": pass1.foot_min_b,
            "b_foot_clearance_at_crouch_step_180": pass1.b_clearance[180]},
        "worst_frame": {"step": worst_step, "by_slip": by_slip.0,
                        "time_seconds": worst_step as f64 / f64::from(scene.rate)},
        "converged_frames_solver_seam": converged_seam,
        "converged_frames_with_c2_no_hands": converged_equiv,
        "total_frames": pass1.reads.len()
    })
}
