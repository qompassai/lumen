//! ops — the operation core of the TUI surface.
//!
//! SPEC section 5.3's hard rule: every TUI verb exists
//! as a CLI call with identical semantics, because the
//! operation core is the single implementation and both
//! front-ends (the ratatui view, the headless CLI) are
//! views over it. Every verb here returns an [`Outcome`]
//! — provenance-first readout lines in which every
//! number names the artifact it came from (D-713) —
//! or a typed [`Refusal`] that fired *before* any
//! mutation, in the SFW pipes' refusal vocabulary.
//!
//! Lane law (pipes section 1): authoring verbs stamp
//! `lane: qompass`, `origin_lane: qompass`,
//! `content_label: sfw` on every run/freeze record,
//! and intake refuses anything not labelled sfw —
//! `lane_mismatch` for nsfw, `lane_unclassified` for
//! anything else or nothing (unclassified defaults to
//! the NSFW side, fail closed).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use lumen_engine_bevy::staging::{load_puppet, load_staging, run_tune};
use lumen_engine_bevy::{run as host_run, HostJob};
use lumen_engine_core::record::SceneRecord;
use lumen_engine_core::runtime::Runtime;
use lumen_engine_core::state::StateRecord;
use lumen_engine_core::track::{resolve_pose_track, sample_pose_track};
use lumen_engine_core::FLASH_MAX_PER_SECOND;
use lumen_rig_core::puppet::{PuppetBindingValues, PuppetNodeKind};
use lumen_rig_core::PuppetEval;
use serde_json::{json, Map, Value};

use crate::termcap;

/// What a verb produced: readout lines (each number
/// traceable to an artifact), the artifacts it wrote,
/// and whether the verb's own verdict is a pass.
#[derive(Clone, Debug)]
pub struct Outcome {
    /// Verb title for panel/CLI headers.
    pub title: String,
    /// Provenance-first readout lines.
    pub lines: Vec<String>,
    /// Files the verb wrote.
    pub artifacts: Vec<PathBuf>,
    /// The verb's verdict (gates: the gate's; other
    /// verbs: success of the operation itself).
    pub ok: bool,
}

impl Outcome {
    fn new(title: &str) -> Self {
        Outcome {
            title: title.to_string(),
            lines: Vec::new(),
            artifacts: Vec::new(),
            ok: true,
        }
    }
}

/// A typed refusal. Refusals fire before mutation: a
/// refused verb leaves every input byte-identical.
#[derive(Clone, Debug)]
pub struct Refusal {
    /// Stable refusal code (pipes vocabulary where one
    /// exists, engine codes otherwise).
    pub code: &'static str,
    /// What was refused, and why.
    pub message: String,
}

impl Refusal {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Refusal {
            code,
            message: message.into(),
        }
    }
}

/// Every verb's result shape.
pub type OpResult = Result<Outcome, Refusal>;

fn io_err(context: &str, e: std::io::Error) -> Refusal {
    Refusal::new("io_error", format!("{context}: {e}"))
}

fn record_invalid(context: &str, e: impl std::fmt::Display) -> Refusal {
    Refusal::new("record_invalid", format!("{context}: {e}"))
}

/// SHA-256 of a file's bytes (the pack crate's helper;
/// one hash implementation across the engine).
pub fn file_sha256(path: &Path) -> Result<String, Refusal> {
    let bytes = std::fs::read(path).map_err(|e| io_err(&format!("read {}", path.display()), e))?;
    Ok(lumen_engine_pack::sha256_hex(&bytes))
}

fn read_json(path: &Path) -> Result<Value, Refusal> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| io_err(&format!("read {}", path.display()), e))?;
    serde_json::from_str(&text).map_err(|e| record_invalid(&format!("parse {}", path.display()), e))
}

fn write_json(path: &Path, value: &Value) -> Result<(), Refusal> {
    let text =
        serde_json::to_string_pretty(value).map_err(|e| record_invalid("serialize json", e))?;
    std::fs::write(path, text + "\n").map_err(|e| io_err(&format!("write {}", path.display()), e))
}

/// Load and validate a scene record from its RON text.
pub fn load_scene(path: &Path) -> Result<SceneRecord, Refusal> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| io_err(&format!("read {}", path.display()), e))?;
    SceneRecord::load_ron(&text)
        .map_err(|e| record_invalid(&format!("scene record {}", path.display()), e))
}

/// The scenes a project carries (`<root>/scenes/*.scene.ron`,
/// sorted by file name).
pub fn project_scenes(root: &Path) -> Result<Vec<PathBuf>, Refusal> {
    let dir = root.join("scenes");
    let mut scenes: Vec<PathBuf> = Vec::new();
    let entries =
        std::fs::read_dir(&dir).map_err(|e| io_err(&format!("read_dir {}", dir.display()), e))?;
    for entry in entries {
        let entry = entry.map_err(|e| io_err("read_dir entry", e))?;
        let path = entry.path();
        if path.extension().is_some_and(|x| x == "ron")
            && path
                .file_name()
                .is_some_and(|n| n.to_string_lossy().ends_with(".scene.ron"))
        {
            scenes.push(path);
        }
    }
    scenes.sort();
    Ok(scenes)
}

fn placement_kind(scene: &SceneRecord, actor: usize) -> &'static str {
    use lumen_engine_core::record::PlacementTrack;
    match &scene.actors[actor].placement {
        PlacementTrack::Fixed { .. } => "Fixed",
        PlacementTrack::PivotTurn { .. } => "PivotTurn",
        PlacementTrack::StepInOut { .. } => "StepInOut",
    }
}

/* ------------------------------------------------------------------ */
/* Project                                                             */
/* ------------------------------------------------------------------ */

/// Open a project: enumerate and validate its scene
/// records, and check every actor's rig import exists.
/// Read-only.
pub fn project_open(root: &Path, imports: &Path) -> OpResult {
    let mut out = Outcome::new("project open");
    out.lines.push(format!("project: {}", root.display()));
    out.lines.push(format!("imports: {}", imports.display()));
    let scenes = project_scenes(root)?;
    out.lines.push(format!(
        "scenes: {} (source: {})",
        scenes.len(),
        root.join("scenes").display()
    ));
    let mut rig_dirs: Vec<String> = Vec::new();
    for scene_path in &scenes {
        let scene = load_scene(scene_path)?;
        let actors: Vec<String> = scene.actors.iter().map(|a| a.id.clone()).collect();
        out.lines.push(format!(
            "scene {}: steps={} rate={} settle={} canvas={}x{}@{} actors=[{}] contacts={} effects={} audio={} dialogue={}",
            scene.id,
            scene.steps,
            scene.rate,
            scene.settle_steps,
            scene.canvas.width,
            scene.canvas.height,
            scene.canvas.scale,
            actors.join(", "),
            scene.contacts.len(),
            scene.effects.len(),
            scene.audio.len(),
            if scene.dialogue.is_some() { "yes" } else { "no" },
        ));
        for actor in &scene.actors {
            if !rig_dirs.contains(&actor.rig) {
                rig_dirs.push(actor.rig.clone());
            }
        }
        out.artifacts.push(scene_path.clone());
    }
    for rig in &rig_dirs {
        let dir = imports.join(rig);
        let present = dir.join("model.json").is_file() && dir.join("puppet.json").is_file();
        out.lines.push(format!(
            "import {rig}: {} (source: {})",
            if present {
                "present (model.json + puppet.json)"
            } else {
                "MISSING"
            },
            dir.display()
        ));
        if !present {
            out.ok = false;
        }
    }
    Ok(out)
}

/* ------------------------------------------------------------------ */
/* Authoring — canon handles, keeper chains, the pipe spine            */
/* ------------------------------------------------------------------ */

/// Find canon-record/1 artifacts under a project root
/// (bounded walk; the canon machinery is consumed from
/// lumen-canon, never forked — SPEC section 4.2).
fn find_canon_records(root: &Path) -> Result<Vec<(PathBuf, canon_record::CanonRecord)>, Refusal> {
    let mut found = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    let mut visited = 0usize;
    while let Some((dir, depth)) = stack.pop() {
        if depth > 6 || visited > 4096 {
            break;
        }
        let entries = match std::fs::read_dir(&dir) {
            Ok(e) => e,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            visited += 1;
            let path = entry.path();
            if path.is_dir() {
                stack.push((path, depth + 1));
            } else if path.extension().is_some_and(|x| x == "json") {
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                let Ok(value) = serde_json::from_str::<Value>(&text) else {
                    continue;
                };
                if value.get("format").and_then(Value::as_str) == Some(canon_record::FORMAT) {
                    let record: canon_record::CanonRecord =
                        serde_json::from_value(value).map_err(|e| {
                            record_invalid(&format!("canon record {}", path.display()), e)
                        })?;
                    found.push((path, record));
                }
            }
        }
    }
    found.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(found)
}

/// Canon handles panel: every canon record the project
/// anchors to, with its lane, label, anchors, and
/// validation state. A project that anchors to rig
/// imports instead reports zero, plainly.
pub fn author_handles(root: &Path) -> OpResult {
    let mut out = Outcome::new("author handles");
    let records = find_canon_records(root)?;
    out.lines.push(format!(
        "canon records under {}: {}",
        root.display(),
        records.len()
    ));
    if records.is_empty() {
        out.lines.push(
            "this project anchors to rig imports, not canon records; no handles to list"
                .to_string(),
        );
    }
    for (path, record) in &records {
        let issues = canon_record::validate_record(record);
        out.lines.push(format!(
            "handle canon://{}/{}@{}: lane={} label={} anchors={} invariants={} validation_issues={} (source: {})",
            record.origin_lane.as_str(),
            record.character().unwrap_or("?"),
            record.version,
            record.origin_lane.as_str(),
            match record.effective_label() {
                canon_record::ContentLabel::Sfw => "sfw",
                canon_record::ContentLabel::Nsfw => "nsfw",
            },
            record.anchors.len(),
            record.identity_invariants.len(),
            issues.len(),
            path.display(),
        ));
        out.artifacts.push(path.clone());
    }
    Ok(out)
}

/// Keeper chains panel: per canon record, the length
/// and adopted state of its append-only keeper chain.
pub fn author_chains(root: &Path) -> OpResult {
    let mut out = Outcome::new("author chains");
    let records = find_canon_records(root)?;
    out.lines.push(format!(
        "keeper chains under {}: {}",
        root.display(),
        records.len()
    ));
    if records.is_empty() {
        out.lines
            .push("no canon records, so no keeper chains exist in this project".to_string());
    }
    for (path, record) in &records {
        let adopted = record.keeper_chain.iter().filter(|e| e.adopted).count();
        out.lines.push(format!(
            "chain {}: entries={} adopted={} (source: {})",
            record.id,
            record.keeper_chain.len(),
            adopted,
            path.display()
        ));
        out.artifacts.push(path.clone());
    }
    Ok(out)
}

fn run_record_path(root: &Path, id: &str) -> PathBuf {
    root.join("runs").join(format!("{id}.run.json"))
}

fn check_run_id(id: &str) -> Result<(), Refusal> {
    if id.is_empty()
        || !id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
    {
        return Err(Refusal::new(
            "record_invalid",
            format!("run id {id:?} is not a plain slug ([a-z0-9-])"),
        ));
    }
    Ok(())
}

/// Describe (spine step 1): open a pipe run record.
/// The lane gate fires first, before any write: only
/// `sfw` material may be described into a Qompass tree.
pub fn author_describe(root: &Path, id: &str, label: Option<&str>, text: &str) -> OpResult {
    check_run_id(id)?;
    match label {
        Some("sfw") => {}
        Some("nsfw") => {
            return Err(Refusal::new(
                "lane_mismatch",
                "content_label nsfw is refused intake into the qompass lane",
            ));
        }
        _ => {
            return Err(Refusal::new(
                "lane_unclassified",
                "content_label missing or unrecognised; unclassified defaults to the NSFW side (fail closed)",
            ));
        }
    }
    let path = run_record_path(root, id);
    if path.exists() {
        return Err(Refusal::new(
            "stage_order",
            format!("run {id} already exists"),
        ));
    }
    let record = json!({
        "format": "lumen-engine-run/1",
        "id": id,
        "stage": "describe",
        "lane": "qompass",
        "origin_lane": "qompass",
        "content_label": "sfw",
        "description": text,
    });
    std::fs::create_dir_all(root.join("runs")).map_err(|e| io_err("create runs dir", e))?;
    write_json(&path, &record)?;
    let mut out = Outcome::new("author describe");
    out.lines.push(format!(
        "run {id}: stage=describe (record: {})",
        path.display()
    ));
    out.lines
        .push("stamps: lane=qompass origin_lane=qompass content_label=sfw".to_string());
    out.lines.push(format!("description: {text}"));
    out.artifacts.push(path);
    Ok(out)
}

fn load_run(root: &Path, id: &str) -> Result<Value, Refusal> {
    check_run_id(id)?;
    let path = run_record_path(root, id);
    if !path.is_file() {
        return Err(Refusal::new(
            "record_missing",
            format!("run {id} has no record"),
        ));
    }
    read_json(&path)
}

/// Propose (spine step 2): stage one textual edit of a
/// scene record as a proposal. The edit must match
/// exactly once and the result must still validate as
/// a scene record — both checked before anything is
/// written, so a refused proposal leaves no artifact.
pub fn author_propose(root: &Path, id: &str, scene_rel: &str, from: &str, to: &str) -> OpResult {
    let mut run = load_run(root, id)?;
    let stage = run.get("stage").and_then(Value::as_str).unwrap_or("");
    if stage != "describe" && stage != "propose" {
        return Err(Refusal::new(
            "stage_order",
            format!("run {id} is at stage {stage:?}; propose needs describe or propose"),
        ));
    }
    let scene_path = root.join(scene_rel);
    let base_text = std::fs::read_to_string(&scene_path)
        .map_err(|e| io_err(&format!("read {}", scene_path.display()), e))?;
    let matches = base_text.matches(from).count();
    if matches != 1 {
        return Err(Refusal::new(
            "edit_not_unique",
            format!("edit source occurs {matches} times in {scene_rel}; a proposal edit must match exactly once"),
        ));
    }
    let candidate = base_text.replacen(from, to, 1);
    SceneRecord::load_ron(&candidate)
        .map_err(|e| record_invalid("proposed scene does not validate; nothing written", e))?;
    let proposal_path = root.join("runs").join(format!("{id}.proposed.scene.ron"));
    std::fs::write(&proposal_path, &candidate)
        .map_err(|e| io_err(&format!("write {}", proposal_path.display()), e))?;
    let base_sha = lumen_engine_pack::sha256_hex(base_text.as_bytes());
    let proposal_sha = lumen_engine_pack::sha256_hex(candidate.as_bytes());
    run["stage"] = json!("propose");
    run["scene"] = json!(scene_rel);
    run["base_sha256"] = json!(base_sha);
    run["proposal_sha256"] = json!(proposal_sha);
    run["edit"] = json!({ "from": from, "to": to });
    write_json(&run_record_path(root, id), &run)?;
    let mut out = Outcome::new("author propose");
    out.lines.push(format!("run {id}: stage=propose"));
    out.lines.push(format!(
        "base sha256: {base_sha} (source: {})",
        scene_path.display()
    ));
    out.lines.push(format!(
        "proposal sha256: {proposal_sha} (artifact: {})",
        proposal_path.display()
    ));
    out.lines
        .push("proposed scene re-validated as a scene record before writing".to_string());
    out.artifacts.push(proposal_path);
    Ok(out)
}

/// Freeze (spine step 3): pin the proposal as a frozen
/// record — the only form the engine's play/gate verbs
/// treat as intake. The freeze record carries the
/// hashes and the lane stamps; no wall clock is read,
/// a freeze is identified by its content hash.
pub fn author_freeze(root: &Path, id: &str) -> OpResult {
    let mut run = load_run(root, id)?;
    let stage = run.get("stage").and_then(Value::as_str).unwrap_or("");
    if stage != "propose" {
        return Err(Refusal::new(
            "stage_order",
            format!("run {id} is at stage {stage:?}; freeze needs propose"),
        ));
    }
    let proposal_path = root.join("runs").join(format!("{id}.proposed.scene.ron"));
    let bytes = std::fs::read(&proposal_path)
        .map_err(|e| io_err(&format!("read {}", proposal_path.display()), e))?;
    let frozen_dir = root.join("frozen");
    std::fs::create_dir_all(&frozen_dir).map_err(|e| io_err("create frozen dir", e))?;
    let frozen_scene = frozen_dir.join(format!("{id}.scene.ron"));
    std::fs::write(&frozen_scene, &bytes)
        .map_err(|e| io_err(&format!("write {}", frozen_scene.display()), e))?;
    let scene_sha = lumen_engine_pack::sha256_hex(&bytes);
    let freeze_record = json!({
        "format": "lumen-engine-frozen/1",
        "run": id,
        "scene": format!("frozen/{id}.scene.ron"),
        "scene_sha256": scene_sha,
        "base_sha256": run.get("base_sha256").cloned().unwrap_or(Value::Null),
        "lane": "qompass",
        "origin_lane": "qompass",
        "content_label": "sfw",
    });
    let freeze_path = frozen_dir.join(format!("{id}.frozen.json"));
    write_json(&freeze_path, &freeze_record)?;
    run["stage"] = json!("freeze");
    run["frozen_sha256"] = json!(scene_sha);
    write_json(&run_record_path(root, id), &run)?;
    let mut out = Outcome::new("author freeze");
    out.lines.push(format!("run {id}: stage=freeze"));
    out.lines.push(format!(
        "frozen scene sha256: {scene_sha} (artifact: {})",
        frozen_scene.display()
    ));
    out.lines
        .push(format!("freeze record: {}", freeze_path.display()));
    out.artifacts.push(frozen_scene);
    out.artifacts.push(freeze_path);
    Ok(out)
}

/* ------------------------------------------------------------------ */
/* Scene staging                                                       */
/* ------------------------------------------------------------------ */

/// The scene record as a tree: actors, pose keys,
/// placements, contacts, and the runtime sections.
/// Read-only.
pub fn scene_tree(scene_path: &Path) -> OpResult {
    let scene = load_scene(scene_path)?;
    let mut out = Outcome::new("scene tree");
    out.lines.push(format!(
        "scene {} (version {}): steps={} rate={} settle={} canvas={}x{}@{} (source: {})",
        scene.id,
        scene.version,
        scene.steps,
        scene.rate,
        scene.settle_steps,
        scene.canvas.width,
        scene.canvas.height,
        scene.canvas.scale,
        scene_path.display()
    ));
    out.lines.push("actors:".to_string());
    for (i, actor) in scene.actors.iter().enumerate() {
        out.lines.push(format!(
            "  [{i}] {} ({}) rig={} placement={} pose_keys={}",
            actor.id,
            actor.display,
            actor.rig,
            placement_kind(&scene, i),
            actor.pose_track.len()
        ));
    }
    out.lines
        .push(format!("contacts: {}", scene.contacts.len()));
    for c in &scene.contacts {
        out.lines.push(format!(
            "  {}: {}:{} <-> {}:{} driven={} window=[{}s, {}s)",
            c.id,
            scene.actors[c.a_actor].id,
            c.a_part,
            scene.actors[c.b_actor].id,
            c.b_part,
            c.driven_actor,
            c.active_from,
            c.active_to
        ));
    }
    out.lines
        .push(format!("roots: [{}]", scene.roots.join(", ")));
    out.lines.push(format!(
        "key_frames: {}",
        scene
            .key_frames
            .iter()
            .map(|k| k.step.to_string())
            .collect::<Vec<_>>()
            .join(", ")
    ));
    match &scene.dialogue {
        Some(d) => out.lines.push(format!(
            "dialogue: start_step={} lines={} chars_per_step={} hold_steps={}",
            d.start_step,
            d.lines.len(),
            d.chars_per_step,
            d.hold_steps
        )),
        None => out.lines.push("dialogue: none".to_string()),
    }
    for fx in &scene.effects {
        let strobes = fx.keys.iter().filter(|k| k.strobe).count();
        out.lines.push(format!(
            "effect {}: start_step={} life_steps={} keys={} strobe_keys={}",
            fx.id,
            fx.start_step,
            fx.life_steps,
            fx.keys.len(),
            strobes
        ));
    }
    for cue in &scene.audio {
        out.lines.push(format!(
            "audio {}: cue={} bus={} gain_db={}",
            cue.id, cue.cue, cue.bus, cue.gain_db
        ));
    }
    Ok(out)
}

/// Stage a preview: render one actor's pose at scene
/// time `t` through the actual evaluator (rig-core's
/// puppet stage, the evaluator of record), and write
/// the frame as a PNG file plus the termcap halfblock
/// and kitty renderings of it. The preview is a staging
/// instrument, not a verdict surface (SPEC 5.2).
pub fn scene_preview(
    scene_path: &Path,
    imports: &Path,
    actor_id: &str,
    t: f64,
    out_png: &Path,
    out_cells: &Path,
    out_kitty: &Path,
) -> OpResult {
    let scene = load_scene(scene_path)?;
    let actor_idx = scene
        .actors
        .iter()
        .position(|a| a.id == actor_id)
        .ok_or_else(|| Refusal::new("record_missing", format!("no actor {actor_id} in scene")))?;
    let staging =
        load_staging(&scene, imports).map_err(|e| Refusal::new("host_staging_failed", e))?;
    let names: BTreeMap<String, f64> = match &scene.tune {
        Some(tune) => run_tune(&scene, tune, &staging)
            .map_err(|e| Refusal::new("host_staging_failed", e))?
            .names(),
        None => BTreeMap::new(),
    };
    let resolved = resolve_pose_track(&scene.actors[actor_idx].pose_track, &names)
        .map_err(|e| record_invalid("pose track resolution", e))?;
    let sets = sample_pose_track(&resolved, t as f32);
    let dt = 1.0 / scene.rate as f32;
    let mut eval = PuppetEval::new(&staging.stages[actor_idx], &staging.textures[actor_idx])
        .map_err(|e| record_invalid("evaluator build", e))?;
    // Hold the sampled pose through the settle window,
    // the staging discipline (springs settle under the
    // pose being inspected).
    for _ in 0..scene.settle_steps {
        eval.begin_frame();
        for (id, value) in &sets {
            eval.set_param(id, *value)
                .map_err(|e| record_invalid("pose set", e))?;
        }
        eval.end_frame(dt)
            .map_err(|e| record_invalid("pose frame", e))?;
    }
    let (w, h) = (scene.canvas.width as usize, scene.canvas.height as usize);
    let rgba = eval.render(w, h, scene.canvas.scale);
    let img = image::RgbaImage::from_raw(scene.canvas.width, scene.canvas.height, rgba)
        .ok_or_else(|| Refusal::new("record_invalid", "render size mismatch"))?;
    img.save(out_png)
        .map_err(|e| Refusal::new("stage_io", format!("write {}: {e}", out_png.display())))?;
    let png_bytes =
        std::fs::read(out_png).map_err(|e| io_err(&format!("read {}", out_png.display()), e))?;
    let grid = termcap::halfblocks(&img, 100);
    let ansi = termcap::grid_to_ansi(&grid);
    std::fs::write(out_cells, &ansi)
        .map_err(|e| io_err(&format!("write {}", out_cells.display()), e))?;
    std::fs::write(out_kitty, termcap::kitty_payload(&png_bytes))
        .map_err(|e| io_err(&format!("write {}", out_kitty.display()), e))?;
    let nontransparent = img.pixels().filter(|p| p.0[3] > 8).count();
    let caps = termcap::detect(
        std::env::var("TERM").ok().as_deref(),
        std::env::var("TERM_PROGRAM").ok().as_deref(),
    );
    let mut out = Outcome::new("scene preview");
    out.lines.push(format!(
        "actor {actor_id} at t={t}s: {} pose parameters set (source: {})",
        sets.len(),
        scene_path.display()
    ));
    out.lines.push(format!(
        "frame: {}x{} px, nontransparent {} px (artifact: {})",
        w,
        h,
        nontransparent,
        out_png.display()
    ));
    out.lines.push(format!(
        "frame sha256: {}",
        lumen_engine_pack::sha256_hex(&png_bytes)
    ));
    out.lines.push(format!(
        "termcap: this terminal presents as {} ({}); halfblock grid {}x{} cells (artifact: {}), kitty payload (artifact: {})",
        caps.protocol.name(),
        caps.evidence,
        grid.cols,
        grid.rows,
        out_cells.display(),
        out_kitty.display()
    ));
    out.lines
        .push("preview is a staging instrument, not a verdict surface (SPEC 5.2)".to_string());
    out.artifacts.push(out_png.to_path_buf());
    out.artifacts.push(out_cells.to_path_buf());
    out.artifacts.push(out_kitty.to_path_buf());
    Ok(out)
}

/* ------------------------------------------------------------------ */
/* Rig / pose inspection                                               */
/* ------------------------------------------------------------------ */

/// Rig census: parameters, nodes, bindings, textures
/// of one rig import, from the evaluator's own stage
/// structures. Read-only.
pub fn rig_census(rig_dir: &Path) -> OpResult {
    let (stage, textures) = load_puppet(rig_dir)
        .map_err(|e| Refusal::new("record_invalid", format!("rig load: {e}")))?;
    let mut out = Outcome::new("rig census");
    out.lines.push(format!(
        "rig {}: model={} textures={} (source: {})",
        rig_dir
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        stage.model,
        textures.len(),
        rig_dir.display()
    ));
    let mut kind_counts: BTreeMap<String, usize> = BTreeMap::new();
    let mut parts = 0usize;
    for node in &stage.nodes {
        *kind_counts.entry(format!("{:?}", node.kind)).or_default() += 1;
        if node.kind == PuppetNodeKind::Part {
            parts += 1;
        }
    }
    out.lines.push(format!(
        "nodes: {} (parts {}) — {}",
        stage.nodes.len(),
        parts,
        kind_counts
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    let mut binding_counts: BTreeMap<String, usize> = BTreeMap::new();
    for b in &stage.bindings {
        *binding_counts.entry(format!("{:?}", b.kind)).or_default() += 1;
    }
    out.lines.push(format!(
        "bindings: {} — {}",
        stage.bindings.len(),
        binding_counts
            .iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    out.lines
        .push(format!("parameters: {}", stage.parameters.len()));
    for p in &stage.parameters {
        let bound = stage
            .bindings
            .iter()
            .filter(|b| b.parameter == p.id)
            .count();
        out.lines.push(format!(
            "  {}: x range [{}, {}] default {} axis_points={} bindings={}",
            p.id,
            p.min[0],
            p.max[0],
            p.default[0],
            p.axis_points[0].len(),
            bound
        ));
    }
    Ok(out)
}

/// One rendered frame under a parameter assignment,
/// the sweep's render primitive: fresh evaluator, the
/// assignment held through the settle window, then the
/// evaluator's own raster. Also returns the bound
/// region measured from the POSED evaluator — the
/// union of the given part nodes' world-vertex AABBs,
/// dilated. The region must come from the posed frame,
/// as sweep.py of record measures it: set-style
/// parameters scale their parts from zero at rest, so
/// a rest-frame region collapses to a point and every
/// appeared pixel reads as outside.
fn sweep_render(
    stage: &lumen_rig_core::puppet::PuppetStage,
    textures: &[lumen_rig_core::RgbaImage],
    sets: &[(String, [f32; 2])],
    region_nodes: &[String],
    dil: f32,
) -> Result<(Vec<u8>, Option<[f32; 4]>), Refusal> {
    let mut eval =
        PuppetEval::new(stage, textures).map_err(|e| record_invalid("evaluator build", e))?;
    for _ in 0..30 {
        eval.begin_frame();
        for (id, value) in sets {
            eval.set_param(id, *value)
                .map_err(|e| record_invalid("sweep set", e))?;
        }
        eval.end_frame(1.0 / 60.0)
            .map_err(|e| record_invalid("sweep frame", e))?;
    }
    let frame = eval.render(SWEEP_W, SWEEP_H, 1.0);
    let mut region: Option<[f32; 4]> = None;
    for id in region_nodes {
        if let Some(verts) = eval.node_world_vertices(id) {
            for v in verts {
                match &mut region {
                    None => region = Some([v[0], v[1], v[0], v[1]]),
                    Some(r) => {
                        r[0] = r[0].min(v[0]);
                        r[1] = r[1].min(v[1]);
                        r[2] = r[2].max(v[0]);
                        r[3] = r[3].max(v[1]);
                    }
                }
            }
        }
    }
    if let Some(r) = &mut region {
        r[0] -= dil;
        r[1] -= dil;
        r[2] += dil;
        r[3] += dil;
    }
    Ok((frame, region))
}

/// Sweep canvas (the standing protocol's, sweep.py of
/// record): 600x800 at scale 1.0, model origin centred.
const SWEEP_W: usize = 600;
/// Sweep canvas height.
const SWEEP_H: usize = 800;

/// Pixel-difference read between two RGBA frames: the
/// count of pixels whose largest per-channel delta
/// exceeds the puppet-proof threshold (8), and their
/// bounding box.
fn diff_read(rest: &[u8], frame: &[u8]) -> (usize, Option<[usize; 4]>, Vec<bool>) {
    let mut count = 0usize;
    let mut bbox: Option<[usize; 4]> = None;
    let mut mask = vec![false; SWEEP_W * SWEEP_H];
    for i in 0..SWEEP_W * SWEEP_H {
        let mut delta = 0u16;
        for c in 0..4 {
            let d = (i16::from(rest[i * 4 + c]) - i16::from(frame[i * 4 + c])).abs();
            delta = delta.max(d as u16);
        }
        if delta > 8 {
            count += 1;
            mask[i] = true;
            let (x, y) = (i % SWEEP_W, i / SWEEP_W);
            match &mut bbox {
                None => bbox = Some([x, y, x + 1, y + 1]),
                Some(b) => {
                    b[0] = b[0].min(x);
                    b[1] = b[1].min(y);
                    b[2] = b[2].max(x + 1);
                    b[3] = b[3].max(y + 1);
                }
            }
        }
    }
    (count, bbox, mask)
}

/// The standing sweep protocol as a verb (RENDER-1
/// pattern, D-859 gate a): render the rest frame, then
/// for EVERY parameter render at min, at max, and at
/// the peak axis point where one exists (the axis point
/// of largest binding deviation from the default point,
/// for cycle-shaped channels whose extremes equal the
/// rest pose). A parameter passes iff some extreme
/// changes >0 pixels AND >=98% of changed pixels fall
/// inside its bound parts' dilated region.
pub fn rig_sweep(rig_dir: &Path, out_json: &Path) -> OpResult {
    let (stage, textures) = load_puppet(rig_dir)
        .map_err(|e| Refusal::new("record_invalid", format!("rig load: {e}")))?;
    // Rest frame (defaults held through the settle).
    let (rest, _) = sweep_render(&stage, &textures, &[], &[], 0.0)?;
    // Descendants per node (inclusive), from parent links.
    let mut children: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for node in &stage.nodes {
        if let Some(parent) = &node.parent {
            children
                .entry(parent.clone())
                .or_default()
                .push(node.id.clone());
        }
    }
    let descendants = |start: &str| -> Vec<String> {
        let mut out = vec![start.to_string()];
        let mut i = 0;
        while i < out.len() {
            if let Some(kids) = children.get(&out[i]) {
                for kid in kids {
                    if !out.contains(kid) {
                        out.push(kid.clone());
                    }
                }
            }
            i += 1;
        }
        out
    };
    let mut results = Map::new();
    let mut fail: Vec<String> = Vec::new();
    let mut pass_count = 0usize;
    for param in &stage.parameters {
        let bindings: Vec<_> = stage
            .bindings
            .iter()
            .filter(|b| b.parameter == param.id)
            .collect();
        // Bound region nodes: the Part nodes among the
        // descendants of every bound node. The region
        // itself is measured per render from the posed
        // evaluator (see sweep_render); here only the
        // dilation is derived, from the binding values.
        let mut region_nodes: Vec<String> = Vec::new();
        for b in &bindings {
            for desc in descendants(&b.node) {
                let is_part = stage
                    .nodes
                    .iter()
                    .find(|n| n.id == desc)
                    .is_some_and(|n| n.kind == PuppetNodeKind::Part);
                if is_part && !region_nodes.contains(&desc) {
                    region_nodes.push(desc);
                }
            }
        }
        let mut dil = 14.0f32;
        for b in &bindings {
            match &b.values {
                PuppetBindingValues::Deform(grid) => {
                    for row in grid {
                        for cell in row {
                            for pt in cell {
                                dil = dil.max(pt[0].abs() + 14.0).max(pt[1].abs() + 14.0);
                            }
                        }
                    }
                }
                PuppetBindingValues::Scalar(grid) => {
                    if matches!(
                        b.kind,
                        lumen_rig_core::puppet::PuppetBindingKind::TranslationX
                            | lumen_rig_core::puppet::PuppetBindingKind::TranslationY
                    ) {
                        for row in grid {
                            for v in row {
                                dil = dil.max(v.abs() + 14.0);
                            }
                        }
                    }
                }
            }
        }
        // Test points: min, max, and the peak axis point.
        let (mn, mx, default) = (param.min[0], param.max[0], param.default[0]);
        let mut tests: Vec<(String, f32)> =
            vec![("at_min".to_string(), mn), ("at_max".to_string(), mx)];
        let aps = &param.axis_points[0];
        let mut peak_value: Option<f32> = None;
        if aps.len() > 2 {
            let di = (0..aps.len())
                .min_by(|&a, &b| {
                    (aps[a] - default)
                        .abs()
                        .partial_cmp(&(aps[b] - default).abs())
                        .unwrap()
                })
                .unwrap_or(0);
            let mut dev = vec![0.0f32; aps.len()];
            for b in &bindings {
                match &b.values {
                    PuppetBindingValues::Scalar(grid) => {
                        if grid.len() == aps.len() {
                            for (i, row) in grid.iter().enumerate() {
                                for (j, v) in row.iter().enumerate() {
                                    if let Some(base) = grid[di].get(j) {
                                        dev[i] = dev[i].max((v - base).abs());
                                    }
                                }
                            }
                        }
                    }
                    PuppetBindingValues::Deform(grid) => {
                        if grid.len() == aps.len() {
                            for (i, row) in grid.iter().enumerate() {
                                for (j, cell) in row.iter().enumerate() {
                                    if let Some(base) = grid[di].get(j) {
                                        for (k, pt) in cell.iter().enumerate() {
                                            if let Some(bp) = base.get(k) {
                                                dev[i] = dev[i]
                                                    .max((pt[0] - bp[0]).abs())
                                                    .max((pt[1] - bp[1]).abs());
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            let peak_i = (0..aps.len())
                .max_by(|&a, &b| dev[a].partial_cmp(&dev[b]).unwrap())
                .unwrap_or(0);
            let pv = aps[peak_i];
            if pv != mn && pv != mx && pv != default {
                peak_value = Some(pv);
                tests.push(("at_peak".to_string(), pv));
            }
        }
        let mut entry = Map::new();
        entry.insert("min".into(), json!(mn));
        entry.insert("max".into(), json!(mx));
        entry.insert("default".into(), json!(default));
        entry.insert("bindings".into(), json!(bindings.len()));
        if let Some(pv) = peak_value {
            entry.insert("peak_value".into(), json!(pv));
        }
        let mut max_changed = 0usize;
        let mut fracs: Vec<f64> = Vec::new();
        for (label, value) in &tests {
            if *value == default {
                entry.insert(
                    label.clone(),
                    json!({ "changed": 0, "note": "extreme==default" }),
                );
                continue;
            }
            let (frame, region) = sweep_render(
                &stage,
                &textures,
                &[(param.id.clone(), [*value, param.default[1]])],
                &region_nodes,
                dil,
            )?;
            let (changed, bbox, mask) = diff_read(&rest, &frame);
            max_changed = max_changed.max(changed);
            let mut test = Map::new();
            test.insert("changed".into(), json!(changed));
            test.insert(
                "bbox".into(),
                match bbox {
                    Some(b) => json!(b),
                    None => Value::Null,
                },
            );
            if changed > 0 {
                if let Some(r) = &region {
                    // The region arrives already dilated
                    // (sweep_render, posed frame).
                    let (cx, cy) = (SWEEP_W as f32 / 2.0, SWEEP_H as f32 / 2.0);
                    let px0 = ((cx + r[0]).floor() as isize).max(0) as usize;
                    let py0 = ((cy + r[1]).floor() as isize).max(0) as usize;
                    let px1 = ((cx + r[2]).ceil() as usize).min(SWEEP_W);
                    let py1 = ((cy + r[3]).ceil() as usize).min(SWEEP_H);
                    let mut inside = 0usize;
                    for y in py0..py1 {
                        for x in px0..px1 {
                            if mask[y * SWEEP_W + x] {
                                inside += 1;
                            }
                        }
                    }
                    let frac = inside as f64 / changed as f64;
                    fracs.push(frac);
                    test.insert("region_frac".into(), json!(frac));
                }
            }
            entry.insert(label.clone(), Value::Object(test));
        }
        let passed = max_changed > 0 && fracs.iter().all(|f| *f >= 0.98);
        entry.insert("pass".into(), json!(passed));
        entry.insert("max_changed".into(), json!(max_changed));
        if passed {
            pass_count += 1;
        } else {
            fail.push(param.id.clone());
        }
        results.insert(param.id.clone(), Value::Object(entry));
    }
    let artifact = json!({
        "format": "lumen-engine-sweep/1",
        "rig": rig_dir.display().to_string(),
        "canvas": [SWEEP_W, SWEEP_H],
        "summary": {
            "params": stage.parameters.len(),
            "pass": pass_count,
            "fail": fail,
        },
        "results": results,
    });
    write_json(out_json, &artifact)?;
    let fail_list = artifact["summary"]["fail"].clone();
    let mut out = Outcome::new("rig sweep");
    out.lines.push(format!(
        "sweep: {} parameters, {} pass, {} fail (artifact: {})",
        stage.parameters.len(),
        pass_count,
        stage.parameters.len() - pass_count,
        out_json.display()
    ));
    out.lines.push(format!(
        "fail list: {}",
        fail_list
            .as_array()
            .map(|a| a
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", "))
            .unwrap_or_default()
    ));
    out.lines.push(
        "protocol: rest vs min/max/peak per parameter, >8 per-channel delta, >=98% of changed pixels inside the bound region (RENDER-1, D-859 gate a)"
            .to_string(),
    );
    out.ok = pass_count == stage.parameters.len();
    out.artifacts.push(out_json.to_path_buf());
    Ok(out)
}

/// Binding inspection for a scene: what drives what —
/// contacts, roots, tune bindings, and every parameter
/// the pose tracks bind, with its expression form.
pub fn rig_bindings(scene_path: &Path) -> OpResult {
    use lumen_engine_core::expr::ValueExpr;
    let scene = load_scene(scene_path)?;
    let mut out = Outcome::new("rig bindings");
    out.lines.push(format!(
        "scene {} (source: {})",
        scene.id,
        scene_path.display()
    ));
    for (i, actor) in scene.actors.iter().enumerate() {
        let mut driven: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for key in &actor.pose_track {
            for (name, expr) in &key.params {
                let kind = match expr {
                    ValueExpr::Const(_) => "Const",
                    ValueExpr::Ref(_) => "Ref",
                    ValueExpr::Add(..) => "Add",
                    ValueExpr::Mul(..) => "Mul",
                };
                driven
                    .entry(name.clone())
                    .or_default()
                    .push(kind.to_string());
            }
        }
        out.lines.push(format!(
            "actor [{i}] {}: {} driven parameters across {} pose keys",
            actor.id,
            driven.len(),
            actor.pose_track.len()
        ));
        for (name, kinds) in &driven {
            out.lines.push(format!("  {name}: {}", kinds.join(", ")));
        }
    }
    for c in &scene.contacts {
        out.lines.push(format!(
            "contact {}: {}.{} -> {}.{} (driven actor {}, window [{}s, {}s))",
            c.id,
            scene.actors[c.a_actor].id,
            c.a_part,
            scene.actors[c.b_actor].id,
            c.b_part,
            c.driven_actor,
            c.active_from,
            c.active_to
        ));
    }
    out.lines.push(format!(
        "root bindings: [{}]",
        scene
            .actors
            .iter()
            .zip(&scene.roots)
            .map(|(a, r)| format!("{}:{}", a.id, r))
            .collect::<Vec<_>>()
            .join(", ")
    ));
    if let Some(tune) = &scene.tune {
        out.lines.push(format!(
            "tune bindings: hands={} ({}, {}) band={}.{}",
            scene.actors[tune.hand_actor].id,
            tune.hand_parts.0,
            tune.hand_parts.1,
            scene.actors[tune.band_actor].id,
            tune.band_part
        ));
    }
    Ok(out)
}

/* ------------------------------------------------------------------ */
/* Playtest                                                            */
/* ------------------------------------------------------------------ */

/// The playtest control surface at core grain: advance
/// the fixed-step runtime `steps` times, count what it
/// produced, and run the snapshot/restore probe — the
/// state at `snapshot_at` must reproduce the
/// uninterrupted event tail exactly.
pub fn play_step(scene_path: &Path, steps: u64, snapshot_at: u64) -> OpResult {
    let scene = load_scene(scene_path)?;
    if snapshot_at >= steps {
        return Err(Refusal::new(
            "record_invalid",
            format!("snapshot step {snapshot_at} must be inside the {steps}-step run"),
        ));
    }
    let mut runtime = Runtime::load(&scene).map_err(|e| record_invalid("runtime load", e))?;
    let (mut n_dialogue, mut n_effects, mut n_audio) = (0usize, 0usize, 0usize);
    let mut snapshot: Option<StateRecord> = None;
    let mut tail: Vec<String> = Vec::new();
    for step in 0..steps {
        if step == snapshot_at {
            snapshot = Some(runtime.snapshot());
        }
        let events = runtime.step();
        n_dialogue += events.dialogue.len();
        n_effects += events.effects.len();
        n_audio += events.audio.len();
        if step >= snapshot_at {
            tail.push(serde_json::to_string(&events).unwrap_or_default());
        }
    }
    let snapshot = snapshot.expect("snapshot taken inside the loop");
    let final_hash = runtime.snapshot().hash();
    // Restore probe: rebuild from the snapshot and
    // re-step the same span; the event tail and the
    // final state hash must match the uninterrupted run.
    let mut restored =
        Runtime::restore(&scene, &snapshot).map_err(|e| record_invalid("runtime restore", e))?;
    let mut tail2: Vec<String> = Vec::new();
    for _ in snapshot_at..steps {
        let events = restored.step();
        tail2.push(serde_json::to_string(&events).unwrap_or_default());
    }
    let restored_hash = restored.snapshot().hash();
    let tail_matches = tail == tail2;
    let mut out = Outcome::new("play step");
    out.lines.push(format!(
        "runtime {}: {} steps at {} steps/s (source: {})",
        scene.id,
        steps,
        scene.rate,
        scene_path.display()
    ));
    out.lines.push(format!(
        "events: dialogue={} effects={} audio={}",
        n_dialogue, n_effects, n_audio
    ));
    out.lines.push(format!(
        "snapshot at step {}: state hash {}",
        snapshot_at,
        snapshot.hash()
    ));
    out.lines.push(format!(
        "restore probe: event tail matches uninterrupted run = {} ({} events re-stepped)",
        tail_matches,
        tail.len()
    ));
    out.lines.push(format!(
        "final state hash: uninterrupted={} restored={}",
        final_hash, restored_hash
    ));
    out.ok = tail_matches && final_hash == restored_hash;
    Ok(out)
}

/// State inspection: read a state record file and show
/// its identity and hash.
pub fn play_inspect(state_path: &Path) -> OpResult {
    let text = std::fs::read_to_string(state_path)
        .map_err(|e| io_err(&format!("read {}", state_path.display()), e))?;
    let state = StateRecord::load_ron(&text)
        .map_err(|e| record_invalid(&format!("state record {}", state_path.display()), e))?;
    let mut out = Outcome::new("play inspect");
    out.lines
        .push(format!("state record: {}", state_path.display()));
    out.lines.push(format!(
        "canonical ron: {}",
        state.to_ron().replace('\n', " ")
    ));
    out.lines.push(format!("state hash: {}", state.hash()));
    Ok(out)
}

/// Run the host over a scene into an output directory
/// (the playtest surface's coarse grain: the full
/// staging/tune/simulation protocol, frames included).
pub fn play_run(
    scene_path: &Path,
    imports: &Path,
    out_dir: &Path,
    reference: Option<&Path>,
) -> OpResult {
    let scene = load_scene(scene_path)?;
    let report = host_run(HostJob {
        scene: scene.clone(),
        imports_base: imports.to_path_buf(),
        out_dir: out_dir.to_path_buf(),
        reference_dir: reference.map(Path::to_path_buf),
        snapshot_step: 360,
    })
    .map_err(|e| Refusal::new("host_run_failed", e))?;
    let summary = &report.summary;
    let get = |path: &[&str]| -> Value {
        let mut v = summary;
        for key in path {
            v = &v[key];
        }
        v.clone()
    };
    let mut out = Outcome::new("play run");
    out.lines.push(format!(
        "host run of {} into {} (source: {})",
        scene.id,
        out_dir.display(),
        scene_path.display()
    ));
    out.lines.push(format!(
        "roles: lifter={} height={} lifted={} height={} (source: {}/summary.json)",
        get(&["roles", "a_lifter"]),
        get(&["roles", "height_a_units"]),
        get(&["roles", "b_lifted"]),
        get(&["roles", "height_b_units"]),
        out_dir.display()
    ));
    out.lines.push(format!(
        "contact engaged max (2-10s): {} (source: {}/summary.json)",
        get(&["hand_to_waist_band_units", "engaged_max_2_10s"]),
        out_dir.display()
    ));
    out.lines.push(format!(
        "solver converged: {}/{} frames (source: {}/summary.json)",
        get(&["converged_frames_solver_seam"]),
        get(&["total_frames"]),
        out_dir.display()
    ));
    out.lines.push(format!(
        "worst frame: step {} (source: {}/summary.json)",
        get(&["worst_frame", "step"]),
        out_dir.display()
    ));
    let state_path = out_dir.join("state-at-360.ron");
    if state_path.is_file() {
        let text =
            std::fs::read_to_string(&state_path).map_err(|e| io_err("read state snapshot", e))?;
        let state =
            StateRecord::load_ron(&text).map_err(|e| record_invalid("state snapshot", e))?;
        out.lines.push(format!(
            "runtime snapshot at step 360: state hash {} (source: {})",
            state.hash(),
            state_path.display()
        ));
    }
    let files = std::fs::read_dir(out_dir)
        .map_err(|e| io_err("read_dir run output", e))?
        .flatten()
        .count();
    out.lines.push(format!(
        "run artifacts: {} files in {}",
        files,
        out_dir.display()
    ));
    out.artifacts.push(out_dir.to_path_buf());
    Ok(out)
}

fn dir_manifest(dir: &Path) -> Result<BTreeMap<String, String>, Refusal> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for entry in std::fs::read_dir(&d).map_err(|e| io_err("read_dir", e))? {
            let entry = entry.map_err(|e| io_err("read_dir entry", e))?;
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.is_file() {
                let rel = path
                    .strip_prefix(dir)
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default();
                out.insert(rel, file_sha256(&path)?);
            }
        }
    }
    Ok(out)
}

/// Deterministic replay with hash readout: run the
/// host twice from the same frozen records and compare
/// every output byte.
pub fn play_replay(scene_path: &Path, imports: &Path, out_a: &Path, out_b: &Path) -> OpResult {
    let scene = load_scene(scene_path)?;
    for dir in [out_a, out_b] {
        host_run(HostJob {
            scene: scene.clone(),
            imports_base: imports.to_path_buf(),
            out_dir: dir.to_path_buf(),
            reference_dir: None,
            snapshot_step: 360,
        })
        .map_err(|e| Refusal::new("host_run_failed", e))?;
    }
    let a = dir_manifest(out_a)?;
    let b = dir_manifest(out_b)?;
    let mut differing: Vec<String> = Vec::new();
    for (name, hash) in &a {
        match b.get(name) {
            Some(other) if other == hash => {}
            Some(_) => differing.push(format!("{name} (hash differs)")),
            None => differing.push(format!("{name} (missing from run B)")),
        }
    }
    for name in b.keys() {
        if !a.contains_key(name) {
            differing.push(format!("{name} (missing from run A)"));
        }
    }
    let mut out = Outcome::new("play replay");
    out.lines.push(format!(
        "replay of {}: {} files per run (sources: {}, {})",
        scene.id,
        a.len(),
        out_a.display(),
        out_b.display()
    ));
    out.lines.push(format!(
        "reads.json sha256: run A {} run B {}",
        a.get("reads.json").cloned().unwrap_or_default(),
        b.get("reads.json").cloned().unwrap_or_default()
    ));
    out.lines.push(format!(
        "byte-identical: {} ({} differing)",
        differing.is_empty(),
        differing.len()
    ));
    for d in &differing {
        out.lines.push(format!("  differs: {d}"));
    }
    out.ok = differing.is_empty();
    out.artifacts.push(out_a.to_path_buf());
    out.artifacts.push(out_b.to_path_buf());
    Ok(out)
}

/* ------------------------------------------------------------------ */
/* Gate runs                                                           */
/* ------------------------------------------------------------------ */

/// The sweep gate over a sweep artifact: every
/// parameter swept, none failing.
pub fn gate_sweep(sweep_json: &Path) -> OpResult {
    let artifact = read_json(sweep_json)?;
    let params = artifact["summary"]["params"].as_u64().unwrap_or(0);
    let pass = artifact["summary"]["pass"].as_u64().unwrap_or(0);
    let fail = artifact["summary"]["fail"].clone();
    let mut out = Outcome::new("gate sweep");
    out.lines.push(format!(
        "sweep gate: {pass}/{params} parameters pass (source: {})",
        sweep_json.display()
    ));
    out.lines.push(format!(
        "failing parameters: {}",
        fail.as_array()
            .map(|a| a
                .iter()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>()
                .join(", "))
            .unwrap_or_else(|| "none".to_string())
    ));
    out.ok = params > 0 && pass == params;
    out.lines.push(format!(
        "gate verdict: {}",
        if out.ok { "PASS" } else { "FAIL" }
    ));
    Ok(out)
}

/// The flash-rate gate: count strobe keys per rolling
/// one-second window straight from the scene record
/// (the same definition the runtime enforces at load,
/// LE-106) and read the maximum against the cap.
pub fn gate_flash(scene_path: &Path) -> OpResult {
    let scene = load_scene(scene_path)?;
    let mut strobes: Vec<u64> = Vec::new();
    for fx in &scene.effects {
        for key in &fx.keys {
            if key.strobe {
                strobes.push(fx.start_step + key.at_step);
            }
        }
    }
    strobes.sort_unstable();
    let window = u64::from(scene.rate.max(1));
    let mut max_in_window = 0usize;
    for (i, &start) in strobes.iter().enumerate() {
        let n = strobes[i..]
            .iter()
            .take_while(|&&s| s < start + window)
            .count();
        max_in_window = max_in_window.max(n);
    }
    let mut out = Outcome::new("gate flash");
    out.lines.push(format!(
        "flash gate on {}: {} timelines, {} strobe keys (source: {})",
        scene.id,
        scene.effects.len(),
        strobes.len(),
        scene_path.display()
    ));
    out.lines.push(format!(
        "max strobes in any 1 s window: {} (cap {}) — {} flashes/s worst case",
        max_in_window, FLASH_MAX_PER_SECOND, max_in_window
    ));
    out.ok = max_in_window <= FLASH_MAX_PER_SECOND;
    out.lines.push(format!(
        "gate verdict: {}",
        if out.ok { "PASS" } else { "FAIL" }
    ));
    Ok(out)
}

fn diff_json(a: &Value, b: &Value, path: &str, out: &mut Vec<String>) {
    if out.len() > 200 {
        return;
    }
    match (a, b) {
        (Value::Object(ma), Value::Object(mb)) => {
            for (k, va) in ma {
                match mb.get(k) {
                    Some(vb) => diff_json(va, vb, &format!("{path}.{k}"), out),
                    None => out.push(format!("{path}.{k}: only in run")),
                }
            }
            for k in mb.keys() {
                if !ma.contains_key(k) {
                    out.push(format!("{path}.{k}: only in reference"));
                }
            }
        }
        (Value::Array(aa), Value::Array(ab)) => {
            if aa.len() != ab.len() {
                out.push(format!("{path}: array length {} vs {}", aa.len(), ab.len()));
            }
            for (i, (va, vb)) in aa.iter().zip(ab).enumerate() {
                diff_json(va, vb, &format!("{path}[{i}]"), out);
            }
        }
        _ => {
            if a != b {
                out.push(format!("{path}: {a} vs {b}"));
            }
        }
    }
}

/// The transcript-comparison gate: a run's summary
/// against the gate record of record, field by field,
/// plus byte identity of reads.json and of every frame
/// the reference carries.
pub fn gate_compare(run_dir: &Path, reference_dir: &Path) -> OpResult {
    let run_summary = read_json(&run_dir.join("summary.json"))?;
    let ref_summary = read_json(&reference_dir.join("summary.json"))?;
    let mut diffs = Vec::new();
    diff_json(&run_summary, &ref_summary, "summary", &mut diffs);
    let run_reads = file_sha256(&run_dir.join("reads.json"))?;
    let ref_reads = file_sha256(&reference_dir.join("reads.json"))?;
    let reads_equal = run_reads == ref_reads;
    let mut frame_diffs = 0usize;
    let mut frame_count = 0usize;
    for entry in std::fs::read_dir(reference_dir).map_err(|e| io_err("read_dir reference", e))? {
        let entry = entry.map_err(|e| io_err("read_dir entry", e))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("frame-") && name.ends_with(".png") {
            frame_count += 1;
            let mine = run_dir.join(&name);
            if !mine.is_file() || file_sha256(&mine)? != file_sha256(&entry.path())? {
                frame_diffs += 1;
            }
        }
    }
    let mut out = Outcome::new("gate compare");
    out.lines.push(format!(
        "summary diff vs gate record: {} differing fields (sources: {}/summary.json, {}/summary.json)",
        diffs.len(),
        run_dir.display(),
        reference_dir.display()
    ));
    for d in diffs.iter().take(10) {
        out.lines.push(format!("  {d}"));
    }
    out.lines.push(format!(
        "reads.json sha256: run {run_reads} reference {ref_reads} identical={reads_equal}"
    ));
    out.lines.push(format!(
        "frames: {frame_count} reference frames, {frame_diffs} differing or missing"
    ));
    out.ok = diffs.is_empty() && reads_equal && frame_diffs == 0;
    out.lines.push(format!(
        "gate verdict: {}",
        if out.ok { "PASS" } else { "FAIL" }
    ));
    Ok(out)
}

/* ------------------------------------------------------------------ */
/* Verdict packs (production, as files)                                */
/* ------------------------------------------------------------------ */

fn flatten_json(value: &Value, path: &str, out: &mut Vec<(String, String)>) {
    match value {
        Value::Object(map) => {
            for (k, v) in map {
                flatten_json(v, &format!("{path}.{k}"), out);
            }
        }
        Value::Array(items) => {
            for (i, v) in items.iter().enumerate() {
                flatten_json(v, &format!("{path}[{i}]"), out);
            }
        }
        other => out.push((path.to_string(), other.to_string())),
    }
}

/// Produce a verdict pack from a host run: the frames,
/// a SHOULD-BE-SEEN statement, the measurements, and a
/// manifest with hashes. The pack takes no verdict —
/// measurements are floors and the Owner's eye rules
/// (D-221/D-629, SPEC 5.2).
pub fn verdict_pack(run_dir: &Path, scene_path: &Path, out_dir: &Path) -> OpResult {
    let scene = load_scene(scene_path)?;
    let summary = read_json(&run_dir.join("summary.json"))?;
    std::fs::create_dir_all(out_dir).map_err(|e| io_err("create pack dir", e))?;
    let frames_dir = out_dir.join("frames");
    std::fs::create_dir_all(&frames_dir).map_err(|e| io_err("create frames dir", e))?;
    let mut frames: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(run_dir).map_err(|e| io_err("read_dir run", e))? {
        let entry = entry.map_err(|e| io_err("read_dir entry", e))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("frame-") && name.ends_with(".png") {
            std::fs::copy(entry.path(), frames_dir.join(&name))
                .map_err(|e| io_err("copy frame", e))?;
            frames.push(name);
        }
    }
    frames.sort();
    // MEASUREMENTS.md — every leaf of the summary, as a
    // table. Floors, not verdicts.
    let mut leaves = Vec::new();
    flatten_json(&summary, "summary", &mut leaves);
    let mut measurements = String::from("# Measurements\n\n| Read | Value |\n|---|---|\n");
    for (path, value) in &leaves {
        measurements.push_str(&format!("| `{path}` | {value} |\n"));
    }
    measurements.push_str(
        "\nMeasurements are floors for the Owner's verdict, not verdicts (D-221/D-629).\n",
    );
    std::fs::write(out_dir.join("MEASUREMENTS.md"), &measurements)
        .map_err(|e| io_err("write MEASUREMENTS.md", e))?;
    // SHOULD-BE-SEEN.md — what each frame must show,
    // stated from the scene record and the run's own
    // reads, so the eye compares seen against stated.
    let band = &summary["hand_to_waist_band_units"];
    let lifter = summary["roles"]["a_lifter"]
        .as_str()
        .unwrap_or("the lifter");
    let lifted = summary["roles"]["b_lifted"]
        .as_str()
        .unwrap_or("the lifted");
    let mut should = String::new();
    should.push_str(&format!("# Should be seen — {}\n\n", scene.id));
    should.push_str(
        "Stakes: this pack is the two-body lift's staging evidence. If the hands are not on the waist band at the engaged steps, the lift is not staged — everything else is decoration.\n\n",
    );
    should.push_str(&format!(
        "Scene: {lifter} lifts {lifted} over {} steps at {} steps/s. Contact engaged max (2-10 s) reads {} — the hands should sit exactly on the band at grasp/crouch/lift/turn/setdown (per-step band reads: grasp {}, crouch {}, lift {}, turn {}, setdown {}).\n\n",
        scene.steps,
        scene.rate,
        band["engaged_max_2_10s"],
        band["grasp_step_120"],
        band["crouch_step_180"],
        band["lift_step_300"],
        band["turn_step_390"],
        band["setdown_step_570"],
    ));
    should.push_str("Frames in this pack:\n\n");
    for frame in &frames {
        should.push_str(&format!("- `frames/{frame}`\n"));
    }
    should.push_str(
        "\nThe worst frame (by slip) is the one to judge first; approach and release frames should show clear hand-to-band distance (the approach read is in MEASUREMENTS.md).\n",
    );
    std::fs::write(out_dir.join("SHOULD-BE-SEEN.md"), &should)
        .map_err(|e| io_err("write SHOULD-BE-SEEN.md", e))?;
    // README.md — the manifest with hashes.
    let scene_sha = file_sha256(scene_path)?;
    let mut readme = String::new();
    readme.push_str(&format!("# Verdict pack — {}\n\n", scene.id));
    readme.push_str(&format!(
        "Produced by lumen-engine-cli `verdict pack` from the run at {}.\nScene record {} sha256 {}.\n\nThis pack takes no verdict: it states what should be seen and what was measured. The Owner's eye rules (D-221/D-629).\n\n## Files\n\n",
        run_dir.display(),
        scene_path.display(),
        scene_sha
    ));
    for name in ["SHOULD-BE-SEEN.md", "MEASUREMENTS.md"] {
        let p = out_dir.join(name);
        readme.push_str(&format!("- {name} sha256 {}\n", file_sha256(&p)?));
    }
    for frame in &frames {
        let p = frames_dir.join(frame);
        readme.push_str(&format!("- frames/{frame} sha256 {}\n", file_sha256(&p)?));
    }
    std::fs::write(out_dir.join("README.md"), &readme).map_err(|e| io_err("write README.md", e))?;
    let mut out = Outcome::new("verdict pack");
    out.lines.push(format!(
        "verdict pack for {}: {} frames (artifacts in {})",
        scene.id,
        frames.len(),
        out_dir.display()
    ));
    out.lines.push(
        "pack contents: frames/, SHOULD-BE-SEEN.md, MEASUREMENTS.md, README.md (manifest with sha256 per file)"
            .to_string(),
    );
    out.lines
        .push("the pack takes no verdict; the Owner's eye rules (D-221/D-629)".to_string());
    out.artifacts.push(out_dir.to_path_buf());
    Ok(out)
}

/* ------------------------------------------------------------------ */
/* Packaging                                                           */
/* ------------------------------------------------------------------ */

/// Build (packaging step 1): assemble the release tree
/// — host binary, scene records, frozen intake — with
/// a build manifest.
pub fn pack_build(root: &Path, host_bin: &Path, out_dir: &Path) -> OpResult {
    if !host_bin.is_file() {
        return Err(Refusal::new(
            "record_missing",
            format!("host binary not found: {}", host_bin.display()),
        ));
    }
    std::fs::create_dir_all(out_dir.join("bin")).map_err(|e| io_err("create bin dir", e))?;
    std::fs::create_dir_all(out_dir.join("scenes")).map_err(|e| io_err("create scenes dir", e))?;
    let bin_dest = out_dir.join("bin").join("lumen-engine-host");
    std::fs::copy(host_bin, &bin_dest).map_err(|e| io_err("copy host binary", e))?;
    let mut copied: Vec<String> = vec![format!(
        "bin/lumen-engine-host ({} bytes, sha256 {})",
        std::fs::metadata(&bin_dest).map(|m| m.len()).unwrap_or(0),
        file_sha256(&bin_dest)?
    )];
    for scene in project_scenes(root)? {
        let name = scene.file_name().expect("scene file name").to_os_string();
        let dest = out_dir.join("scenes").join(&name);
        std::fs::copy(&scene, &dest).map_err(|e| io_err("copy scene", e))?;
        copied.push(format!("scenes/{}", name.to_string_lossy()));
    }
    let frozen_dir = root.join("frozen");
    if frozen_dir.is_dir() {
        std::fs::create_dir_all(out_dir.join("frozen"))
            .map_err(|e| io_err("create frozen dir", e))?;
        for entry in std::fs::read_dir(&frozen_dir).map_err(|e| io_err("read_dir frozen", e))? {
            let entry = entry.map_err(|e| io_err("read_dir entry", e))?;
            let name = entry.file_name();
            if name.to_string_lossy().ends_with(".scene.ron") {
                std::fs::copy(entry.path(), out_dir.join("frozen").join(&name))
                    .map_err(|e| io_err("copy frozen scene", e))?;
                copied.push(format!("frozen/{}", name.to_string_lossy()));
            }
        }
    }
    let members =
        lumen_engine_pack::collect_tree(out_dir).map_err(|e| Refusal::new("pack_failed", e.0))?;
    let manifest = json!({
        "format": "lumen-engine-build/1",
        "project": root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        "files": lumen_engine_pack::manifest_of(&members)
            .iter()
            .map(|m| json!({ "path": m.path, "size": m.size, "sha256": m.sha256 }))
            .collect::<Vec<_>>(),
    });
    let manifest_path = out_dir.join("BUILD.json");
    write_json(&manifest_path, &manifest)?;
    let mut out = Outcome::new("pack build");
    out.lines.push(format!(
        "release tree assembled at {} ({} files before manifest)",
        out_dir.display(),
        members.len()
    ));
    for c in &copied {
        out.lines.push(format!("  {c}"));
    }
    out.lines
        .push(format!("build manifest: {}", manifest_path.display()));
    out.artifacts.push(out_dir.to_path_buf());
    Ok(out)
}

/// Package (step 2): the deterministic archive.
pub fn pack_package(dir: &Path, out_archive: &Path) -> OpResult {
    let project = dir
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "release".to_string());
    let archive_file = out_archive
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let (archive, record) = lumen_engine_pack::package_tree(dir, &project, &archive_file)
        .map_err(|e| Refusal::new("pack_failed", e.0))?;
    std::fs::write(out_archive, &archive)
        .map_err(|e| io_err(&format!("write {}", out_archive.display()), e))?;
    let mut out = Outcome::new("pack package");
    out.lines.push(format!(
        "archive: {} ({} bytes, sha256 {})",
        out_archive.display(),
        record.archive_size,
        record.archive_sha256
    ));
    out.lines.push(format!("members: {}", record.members.len()));
    out.lines.push("deterministic: same tree packages to the same bytes (pinned tar fields, zeroed gzip mtime)".to_string());
    out.artifacts.push(out_archive.to_path_buf());
    Ok(out)
}

/// Release record (step 3): emit the record for an
/// existing archive by reading the archive itself.
pub fn pack_record(archive_path: &Path, project: &str, out_json: &Path) -> OpResult {
    let bytes = std::fs::read(archive_path)
        .map_err(|e| io_err(&format!("read {}", archive_path.display()), e))?;
    let members =
        lumen_engine_pack::archive_members(&bytes).map_err(|e| Refusal::new("pack_failed", e.0))?;
    let record = lumen_engine_pack::ReleaseRecord {
        format: lumen_engine_pack::RELEASE_FORMAT.to_string(),
        project: project.to_string(),
        archive_file: archive_path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        archive_size: bytes.len() as u64,
        archive_sha256: lumen_engine_pack::sha256_hex(&bytes),
        members: lumen_engine_pack::manifest_of(&members),
    };
    let value = serde_json::to_value(&record).map_err(|e| record_invalid("serialize record", e))?;
    write_json(out_json, &value)?;
    let mut out = Outcome::new("pack record");
    out.lines.push(format!(
        "release record for {}: {} members, archive sha256 {} (artifact: {})",
        record.archive_file,
        record.members.len(),
        record.archive_sha256,
        out_json.display()
    ));
    out.artifacts.push(out_json.to_path_buf());
    Ok(out)
}

/// Verify (step 4): read an archive back against its
/// release record, in place.
pub fn pack_verify(archive_path: &Path, record_path: &Path) -> OpResult {
    let bytes = std::fs::read(archive_path)
        .map_err(|e| io_err(&format!("read {}", archive_path.display()), e))?;
    let text = std::fs::read_to_string(record_path)
        .map_err(|e| io_err(&format!("read {}", record_path.display()), e))?;
    let record: lumen_engine_pack::ReleaseRecord = serde_json::from_str(&text)
        .map_err(|e| record_invalid(&format!("parse {}", record_path.display()), e))?;
    let problems = lumen_engine_pack::verify_archive(&bytes, &record)
        .map_err(|e| Refusal::new("pack_failed", e.0))?;
    let mut out = Outcome::new("pack verify");
    out.lines.push(format!(
        "verify {} against {}: {} members in record",
        archive_path.display(),
        record_path.display(),
        record.members.len()
    ));
    if problems.is_empty() {
        out.lines.push(format!(
            "archive matches its record exactly (sha256 {})",
            record.archive_sha256
        ));
    } else {
        for p in &problems {
            out.lines.push(format!("  discrepancy: {p}"));
        }
    }
    out.ok = problems.is_empty();
    out.lines.push(format!(
        "verify verdict: {}",
        if out.ok { "PASS" } else { "FAIL" }
    ));
    Ok(out)
}
