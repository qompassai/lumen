//! Scene staging for the two-body host: load puppet rigs
//! through the evaluator of record, bridge them to
//! section-3 models, measure bounds, run the grasp tune
//! the record declares, and simulate the scene through
//! the engine's contact seam. The staging procedure —
//! settle discipline, tune objective and bars, placement
//! conventions — is the D-859/D-875 procedure, ported
//! from the evaluator seam's scene driver and driven by
//! the scene record instead of compiled-in constants.

use std::collections::BTreeMap;
use std::path::Path;

use lumen_engine_core::contact::{
    endpoint_world, evaluate_frame_with_geometry, ContactDef, ContactEndpoint, EvaluatedMesh,
    FrameInput, FrameRead, Scene,
};
use lumen_engine_core::record::{SceneRecord, TuneExpected, TuneRecord};
use lumen_engine_core::{
    resolve_pose_track, sample_pivot_turn, sample_placement, sample_pose_track, ResolvedPoseKey,
    ValueExpr,
};
use lumen_rig_core::model::{BlendMode, Mesh, Model, Part};
use lumen_rig_core::puppet::{PuppetNodeKind, PuppetStage};
use lumen_rig_core::{parse_model, parse_puppet, PuppetEval, RgbaImage as TexImage};

use crate::instruments::{box_distance, is_leg_part, margin_of, points_of, world_meshes};

type HostResult<T> = Result<T, String>;

fn err<T>(message: impl Into<String>) -> HostResult<T> {
    Err(message.into())
}

/// Loaded rigs and their bridges.
pub struct Staging {
    /// Puppet stages per actor.
    pub stages: Vec<PuppetStage>,
    /// Decoded textures per actor.
    pub textures: Vec<Vec<TexImage>>,
    /// Bridge models per actor.
    pub models: Vec<Model>,
    /// Measured bounds per actor.
    pub bounds: Vec<Bounds>,
}

/// Measured rig bounds (puppet space).
#[derive(Clone, Copy, Debug)]
pub struct Bounds {
    /// Total height (max y - min y over all meshes, rest
    /// pose, y-down puppet space as the seam measures it).
    pub height: f64,
    /// Y of the lowest leg vertex (the ground lift).
    pub foot_y: f64,
    /// Median x of leg vertices near the feet.
    pub foot_centre_x: f64,
}

/// Load one puppet import directory (model.json +
/// puppet.json + textures) through the evaluator's
/// parsers.
pub fn load_puppet(dir: &Path) -> HostResult<(PuppetStage, Vec<TexImage>)> {
    let model_bytes =
        std::fs::read(dir.join("model.json")).map_err(|e| format!("model.json read: {e}"))?;
    let model = parse_model(&model_bytes).map_err(|e| format!("model parses: {e}"))?;
    let stage_bytes =
        std::fs::read(dir.join("puppet.json")).map_err(|e| format!("puppet.json read: {e}"))?;
    let stage = parse_puppet(&stage_bytes, &model).map_err(|e| format!("stage parses: {e}"))?;
    let mut textures = Vec::with_capacity(stage.textures.len());
    for tex in &stage.textures {
        let img = image::open(dir.join(&tex.source))
            .map_err(|e| format!("texture decodes: {e}"))?
            .to_rgba8();
        textures.push(TexImage {
            width: img.width() as usize,
            height: img.height() as usize,
            rgba: img.into_raw(),
        });
    }
    Ok((stage, textures))
}

/// A section-3 model whose meshes mirror the stage's part
/// nodes 1:1 (the seam's bridge: the model carries counts
/// and ids only; geometry arrives per frame).
pub fn bridge_model(stage: &PuppetStage, id: &str) -> Model {
    let mut parts = Vec::new();
    let mut meshes = Vec::new();
    for node in &stage.nodes {
        if !matches!(node.kind, PuppetNodeKind::Part) {
            continue;
        }
        let Some(mesh) = &node.mesh else { continue };
        parts.push(Part {
            id: node.id.clone(),
            parent: None,
            opacity: 1.0,
        });
        meshes.push(Mesh {
            id: node.id.clone(),
            part: node.id.clone(),
            texture: String::new(),
            deformer: None,
            vertices: mesh.vertices.iter().map(|v| [v[0], -v[1]]).collect(),
            uvs: mesh.uvs.clone(),
            indices: mesh.indices.clone(),
            opacity: 1.0,
            blend_mode: BlendMode::Normal,
            draw_order: (node.zsort * 1000.0) as i32,
            rendered: true,
            clipping_mask_ids: Vec::new(),
            keyforms: None,
        });
    }
    Model {
        id: id.to_string(),
        generator: Some("lumen-engine host bridge (after the D-875 driver bridge)".to_string()),
        parameters: Vec::new(),
        parts,
        textures: Vec::new(),
        deformers: Vec::new(),
        meshes,
        physics: Vec::new(),
    }
}

/// Harvest one frame of evaluated puppet geometry as
/// solver geometry: model space, y negated into the
/// solver's y-UP scene plane.
pub fn harvest(eval: &PuppetEval, model: &Model) -> Vec<EvaluatedMesh> {
    model
        .meshes
        .iter()
        .map(|m| {
            let verts = eval
                .node_world_vertices(&m.id)
                .unwrap_or_else(|| panic!("part {} yields vertices", m.id));
            EvaluatedMesh {
                draw_order: m.draw_order,
                id: m.id.clone(),
                part: m.part.clone(),
                vertices: verts
                    .iter()
                    .map(|v| [f64::from(v[0]), -f64::from(v[1])])
                    .collect(),
            }
        })
        .collect()
}

/// Vertex-mean centroid of an evaluated mesh.
pub fn centroid_of(mesh: &EvaluatedMesh) -> [f64; 2] {
    let n = mesh.vertices.len() as f64;
    let (mut x, mut y) = (0.0, 0.0);
    for v in &mesh.vertices {
        x += v[0];
        y += v[1];
    }
    [x / n, y / n]
}

/// An evaluated mesh by id (panics when absent, as the
/// seam's driver did — staging data errors are host bugs
/// against a validated record).
pub fn mesh_named<'m>(meshes: &'m [EvaluatedMesh], id: &str) -> &'m EvaluatedMesh {
    meshes
        .iter()
        .find(|m| m.id == id)
        .unwrap_or_else(|| panic!("mesh {id} harvested"))
}

/// Silhouette x-extent of the named parts' triangles at
/// height y (scene space).
pub fn silhouette_at(
    model: &Model,
    geo: &[EvaluatedMesh],
    parts: &[&str],
    y: f64,
) -> Option<(f64, f64)> {
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    for declared in &model.meshes {
        if !parts.contains(&declared.part.as_str()) {
            continue;
        }
        let eval = mesh_named(geo, &declared.id);
        for tri in declared.indices.as_chunks::<3>().0 {
            let pts = [
                eval.vertices[tri[0] as usize],
                eval.vertices[tri[1] as usize],
                eval.vertices[tri[2] as usize],
            ];
            for (a, b) in [(pts[0], pts[1]), (pts[1], pts[2]), (pts[2], pts[0])] {
                if (a[1] - y) * (b[1] - y) <= 0.0 && (a[1] - b[1]).abs() > 1e-9 {
                    let f = (y - a[1]) / (b[1] - a[1]);
                    if (0.0..=1.0).contains(&f) {
                        let x = a[0] + f * (b[0] - a[0]);
                        lo = lo.min(x);
                        hi = hi.max(x);
                    }
                }
            }
        }
    }
    if lo <= hi {
        Some((lo, hi))
    } else {
        None
    }
}

/// Apply a pose (begin frame + set each parameter).
pub fn set_pose(eval: &mut PuppetEval, sets: &[(String, [f32; 2])]) {
    eval.begin_frame();
    for (id, value) in sets {
        eval.set_param(id, *value).expect("pose parameter exists");
    }
}

/// Run empty frames so spring state settles.
pub fn settle(eval: &mut PuppetEval, frames: u32, dt: f32) {
    for _ in 0..frames {
        set_pose(eval, &[]);
        eval.end_frame(dt).expect("settle frame evaluates");
    }
}

/// Measure a rig's bounds on a settled evaluator.
pub fn measure_bounds(eval: &mut PuppetEval, model: &Model, dt: f32) -> Bounds {
    set_pose(eval, &[]);
    eval.end_frame(dt).expect("bounds frame evaluates");
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    let mut foot_y = f64::NEG_INFINITY;
    for mesh in &model.meshes {
        let verts = eval.node_world_vertices(&mesh.id).expect("bounds vertices");
        for v in &verts {
            lo = lo.min(f64::from(v[1]));
            hi = hi.max(f64::from(v[1]));
        }
        if is_leg_part(&mesh.id) {
            for v in &verts {
                foot_y = foot_y.max(f64::from(v[1]));
            }
        }
    }
    let mut foot_xs: Vec<f64> = Vec::new();
    for mesh in &model.meshes {
        if !is_leg_part(&mesh.id) {
            continue;
        }
        let verts = eval.node_world_vertices(&mesh.id).expect("bounds vertices");
        for v in &verts {
            if f64::from(v[1]) > foot_y - 60.0 {
                foot_xs.push(f64::from(v[0]));
            }
        }
    }
    foot_xs.sort_by(f64::total_cmp);
    let foot_centre_x = if foot_xs.is_empty() {
        0.0
    } else {
        foot_xs[foot_xs.len() / 2]
    };
    Bounds {
        height: hi - lo,
        foot_y,
        foot_centre_x,
    }
}

/// Load every actor's rig and measure bounds.
pub fn load_staging(scene: &SceneRecord, base: &Path) -> HostResult<Staging> {
    let dt = 1.0 / scene.rate as f32;
    let mut stages = Vec::new();
    let mut textures = Vec::new();
    let mut models = Vec::new();
    let mut bounds = Vec::new();
    for actor in &scene.actors {
        let dir = base.join(&actor.rig);
        let (stage, tex) = load_puppet(&dir)?;
        let model = bridge_model(&stage, &format!("bridge-{}", actor.id));
        let mut scratch = PuppetEval::new(&stage, &tex).map_err(|e| format!("eval builds: {e}"))?;
        settle(&mut scratch, scene.settle_steps, dt);
        bounds.push(measure_bounds(&mut scratch, &model, dt));
        stages.push(stage);
        textures.push(tex);
        models.push(model);
    }
    Ok(Staging {
        stages,
        textures,
        models,
        bounds,
    })
}

/* ---------------------------------------------------------------- */
/* Grasp tune                                                        */
/* ---------------------------------------------------------------- */

/// The tune's outputs (the staging the scan selected).
#[derive(Clone, Debug)]
pub struct TuneOutcome {
    /// The driven actor's held x.
    pub b_x: f64,
    /// Hand target for the first hand part (local).
    pub target_left: [f64; 2],
    /// Hand target for the second hand part (local).
    pub target_right: [f64; 2],
    /// The chosen band row (local y).
    pub target_row_y: f64,
    /// Predicted rigid residual of the chosen candidate.
    pub predicted_slip: f64,
    /// Per-contact pre-solve slip at the grasp.
    pub pre_solve_slip: [f64; 2],
    /// Chosen grasp parameter.
    pub grasp_arm_l: f32,
    /// Measured turn compensation lift.
    pub turn_lift: f64,
    /// Chosen crouch depth.
    pub crouch_depth: f32,
    /// Measured crouch hand drop.
    pub crouch_drop: f64,
    /// Measured hand rise from grasp to lift.
    pub hand_rise: f64,
    /// Predicted hold clearance of the chosen candidate.
    pub predicted_clearance: f64,
    /// The band box: [min x, min y, max x, max y].
    pub waist_band: [f64; 4],
}

impl TuneOutcome {
    /// The tune names the scene's tracks resolve against.
    pub fn names(&self) -> BTreeMap<String, f64> {
        BTreeMap::from([
            ("g".to_string(), f64::from(self.grasp_arm_l)),
            ("cd".to_string(), f64::from(self.crouch_depth)),
            ("turn_lift".to_string(), self.turn_lift),
            ("b_x".to_string(), self.b_x),
        ])
    }

    /// Field-level comparison against the record's
    /// expected outputs: the names of fields that differ
    /// (exact f64 comparison — the reads of record are
    /// bit-level gate values).
    pub fn diff_expected(
        &self,
        expected: &TuneExpected,
        contact_offsets: [[f64; 2]; 2],
    ) -> Vec<String> {
        let mut diffs = Vec::new();
        let mut check = |name: &str, got: f64, want: f64| {
            if got != want {
                diffs.push(format!("{name}: got {got}, expected {want}"));
            }
        };
        check("b_x", self.b_x, expected.b_x);
        check("target_row_y", self.target_row_y, expected.target_row_y);
        check(
            "target_left_x",
            self.target_left[0],
            expected.target_left[0],
        );
        check(
            "target_left_y",
            self.target_left[1],
            expected.target_left[1],
        );
        check(
            "target_right_x",
            self.target_right[0],
            expected.target_right[0],
        );
        check(
            "target_right_y",
            self.target_right[1],
            expected.target_right[1],
        );
        check(
            "predicted_slip",
            self.predicted_slip,
            expected.predicted_slip,
        );
        check(
            "pre_solve_slip_0",
            self.pre_solve_slip[0],
            expected.pre_solve_slip[0],
        );
        check(
            "pre_solve_slip_1",
            self.pre_solve_slip[1],
            expected.pre_solve_slip[1],
        );
        check(
            "grasp_arm_l",
            f64::from(self.grasp_arm_l),
            expected.grasp_arm_l,
        );
        check(
            "crouch_depth",
            f64::from(self.crouch_depth),
            expected.crouch_depth,
        );
        check("turn_lift", self.turn_lift, expected.turn_lift);
        check("crouch_drop", self.crouch_drop, expected.crouch_drop);
        check("hand_rise", self.hand_rise, expected.hand_rise);
        check(
            "predicted_clearance",
            self.predicted_clearance,
            expected.predicted_clearance,
        );
        for i in 0..4 {
            check(
                &format!("waist_band_{i}"),
                self.waist_band[i],
                expected.waist_band[i],
            );
        }
        for c in 0..2 {
            for a in 0..2 {
                check(
                    &format!("contact_offset_{c}_{a}"),
                    contact_offsets[c][a],
                    expected.contact_offsets[c][a],
                );
            }
        }
        diffs
    }
}

/// A pose map resolved to plain f32 sets.
pub type PoseSets = Vec<(String, [f32; 2])>;

/// Resolve one family/record pose map against names.
pub fn resolve_map(
    params: &BTreeMap<String, ValueExpr>,
    names: &BTreeMap<String, f64>,
) -> HostResult<Vec<(String, f32)>> {
    let mut out = Vec::with_capacity(params.len());
    for (name, form) in params {
        let value = form.eval(names).map_err(|e| e.to_string())?;
        out.push((name.clone(), value as f32));
    }
    Ok(out)
}

fn as_sets(map: &[(String, f32)]) -> PoseSets {
    map.iter().map(|(n, v)| (n.clone(), [*v, 0.0])).collect()
}

/// The four family poses resolved at (g, cd).
pub struct FamilyPoses {
    /// Grasp pose.
    pub grasp: Vec<(String, f32)>,
    /// Crouch pose.
    pub crouch: Vec<(String, f32)>,
    /// Lift pose.
    pub lift: Vec<(String, f32)>,
    /// Setdown pose.
    pub setdown: Vec<(String, f32)>,
}

/// Resolve the record's pose family at (g, cd).
pub fn resolve_family(tune: &TuneRecord, g: f64, cd: f64) -> HostResult<FamilyPoses> {
    let names = BTreeMap::from([("g".to_string(), g), ("cd".to_string(), cd)]);
    Ok(FamilyPoses {
        grasp: resolve_map(&tune.family.grasp, &names)?,
        crouch: resolve_map(&tune.family.crouch, &names)?,
        lift: resolve_map(&tune.family.lift, &names)?,
        setdown: resolve_map(&tune.family.setdown, &names)?,
    })
}

fn hand_pair(
    eval_a: &mut PuppetEval,
    model_a: &Model,
    map: &[(String, f32)],
    hand_parts: &(String, String),
    dt: f32,
) -> ([f64; 2], [f64; 2]) {
    set_pose(eval_a, &as_sets(map));
    eval_a.end_frame(dt).expect("pose frame A");
    let geo = harvest(eval_a, model_a);
    (
        centroid_of(mesh_named(&geo, &hand_parts.0)),
        centroid_of(mesh_named(&geo, &hand_parts.1)),
    )
}

/// Run the record's grasp tune. The procedure and its
/// objective are the D-859/D-875 ones: minimum true
/// Kabsch residual (separation mismatch / 2) over the
/// band rows, feasible candidates first (clearance bar
/// and engage floor from the record), the minimum-
/// residual candidate overall as fallback; then the
/// crouch depth under its drop cap, and the turn-lift
/// compensation measured on the rest geometry at the
/// placement track's peak key.
#[allow(clippy::too_many_arguments)]
pub fn run_tune(
    scene: &SceneRecord,
    tune: &TuneRecord,
    staging: &Staging,
) -> HostResult<TuneOutcome> {
    let dt = 1.0 / scene.rate as f32;
    let hand = tune.hand_actor;
    let band_actor = tune.band_actor;
    let mut eval_a = PuppetEval::new(&staging.stages[hand], &staging.textures[hand])
        .map_err(|e| e.to_string())?;
    let mut eval_b = PuppetEval::new(&staging.stages[band_actor], &staging.textures[band_actor])
        .map_err(|e| e.to_string())?;
    settle(&mut eval_a, scene.settle_steps, dt);
    settle(&mut eval_b, scene.settle_steps, dt);
    // Band part at rest: AABB and the band rows.
    set_pose(&mut eval_b, &[]);
    eval_b.end_frame(dt).expect("frame B");
    let geo_b = harvest(&eval_b, &staging.models[band_actor]);
    let body = mesh_named(&geo_b, &tune.band_part);
    let (mut bx0, mut bx1, mut by0, mut by1) = (
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::INFINITY,
        f64::NEG_INFINITY,
    );
    for v in &body.vertices {
        bx0 = bx0.min(v[0]);
        bx1 = bx1.max(v[0]);
        by0 = by0.min(v[1]);
        by1 = by1.max(v[1]);
    }
    let bh = by1 - by0;
    let band_lo = by1 - tune.band_far * bh;
    let band_hi = by1 - tune.band_near * bh;
    let waist_band = [bx0, band_lo, bx1, band_hi];

    #[derive(Clone, Copy)]
    struct Cand {
        slip: f64,
        engage: f64,
        clearance: f64,
        g: f32,
        row: f64,
        target_hand_l: [f64; 2],
        target_hand_r: [f64; 2],
        hand_mid_x: f64,
        target_mid_x: f64,
        drop08: f64,
        hands_g: ([f64; 2], [f64; 2]),
    }
    let mut best: Option<Cand> = None;
    let mut best_any: Option<Cand> = None;
    for &g in &tune.g_candidates {
        let family = resolve_family(tune, g, tune.default_crouch)?;
        let hands_g = hand_pair(
            &mut eval_a,
            &staging.models[hand],
            &family.grasp,
            &tune.hand_parts,
            dt,
        );
        let hands_c = hand_pair(
            &mut eval_a,
            &staging.models[hand],
            &family.crouch,
            &tune.hand_parts,
            dt,
        );
        let hands_l = hand_pair(
            &mut eval_a,
            &staging.models[hand],
            &family.lift,
            &tune.hand_parts,
            dt,
        );
        let mid = |h: &([f64; 2], [f64; 2])| [(h.0[0] + h.1[0]) / 2.0, (h.0[1] + h.1[1]) / 2.0];
        let mid_g = mid(&hands_g);
        let mid_c = mid(&hands_c);
        let mid_l = mid(&hands_l);
        let drop08 = mid_g[1] - mid_c[1];
        let hand_sep = (hands_g.0[0] - hands_g.1[0]).hypot(hands_g.0[1] - hands_g.1[1]);
        // Hand targets are assigned by measured side,
        // never by assumption (the seam's rule).
        let hand_l_is_neg = hands_g.0[0] <= hands_g.1[0];
        let mut row = band_lo;
        while row <= band_hi {
            if let Some((lo, hi)) = silhouette_at(
                &staging.models[band_actor],
                &geo_b,
                &[tune.band_part.as_str()],
                row,
            ) {
                let target_neg = [lo, row];
                let target_pos = [hi, row];
                let target_sep = target_pos[0] - target_neg[0];
                let slip = (hand_sep - target_sep).abs() / 2.0;
                let engage = (mid_g[1] + staging.bounds[hand].foot_y)
                    - (row + staging.bounds[band_actor].foot_y);
                let clearance = (mid_l[1] + staging.bounds[hand].foot_y)
                    - (row + staging.bounds[band_actor].foot_y);
                let (target_hand_l, target_hand_r) = if hand_l_is_neg {
                    (target_neg, target_pos)
                } else {
                    (target_pos, target_neg)
                };
                let cand = Cand {
                    slip,
                    engage,
                    clearance,
                    g: g as f32,
                    row,
                    target_hand_l,
                    target_hand_r,
                    hand_mid_x: mid_g[0],
                    target_mid_x: (target_neg[0] + target_pos[0]) / 2.0,
                    drop08,
                    hands_g,
                };
                if best_any.as_ref().is_none_or(|b| cand.slip < b.slip) {
                    best_any = Some(Cand { ..cand });
                }
                let feasible =
                    cand.clearance >= tune.clearance_bar && cand.engage >= tune.engage_min;
                if feasible
                    && best.as_ref().is_none_or(|b| {
                        cand.slip < b.slip
                            || (cand.slip == b.slip && cand.engage.abs() < b.engage.abs())
                    })
                {
                    best = Some(cand);
                }
            }
            row += 1.0;
        }
    }
    let chosen = best.or(best_any).expect("waist band row found");
    // Crouch depth: largest of the family keeping the
    // crouch-window dip inside the cap.
    let mut cd = tune.default_crouch as f32;
    if chosen.drop08 > 0.0 {
        let dip_at_default = chosen.engage - chosen.drop08;
        if dip_at_default < -tune.crouch_drop_cap {
            let allowed = (chosen.engage + tune.crouch_drop_cap) / chosen.drop08;
            cd = (tune.default_crouch * allowed).clamp(0.3, tune.default_crouch) as f32;
        }
    }
    let family_cd = resolve_family(tune, f64::from(chosen.g), f64::from(cd))?;
    let hands_c = hand_pair(
        &mut eval_a,
        &staging.models[hand],
        &family_cd.crouch,
        &tune.hand_parts,
        dt,
    );
    let hands_l = hand_pair(
        &mut eval_a,
        &staging.models[hand],
        &family_cd.lift,
        &tune.hand_parts,
        dt,
    );
    let mid = |h: &([f64; 2], [f64; 2])| [(h.0[0] + h.1[0]) / 2.0, (h.0[1] + h.1[1]) / 2.0];
    let mid_g = mid(&chosen.hands_g);
    let mid_c = mid(&hands_c);
    let mid_l = mid(&hands_l);
    let crouch_drop = mid_g[1] - mid_c[1];
    let hand_rise = mid_l[1] - mid_g[1];
    let predicted_clearance =
        (mid_l[1] + staging.bounds[hand].foot_y) - (chosen.row + staging.bounds[band_actor].foot_y);
    // Turn compensation measured on the rest geometry at
    // the placement track's peak-rotation key.
    let turn_lift = {
        set_pose(&mut eval_a, &[]);
        eval_a.end_frame(dt).expect("turn probe frame A");
        let geo = harvest(&eval_a, &staging.models[hand]);
        let bounds_a = &staging.bounds[hand];
        let pivot = [bounds_a.foot_centre_x, -bounds_a.foot_y];
        let probe = match &scene.actors[hand].placement {
            lumen_engine_core::PlacementTrack::PivotTurn { keys, .. } => {
                let peak_t = keys
                    .iter()
                    .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
                    .map(|k| k.0)
                    .unwrap_or(0.0);
                sample_pivot_turn(keys, 0.0, bounds_a.foot_y, pivot, peak_t)
            }
            _ => return err("turn-lift probe needs a pivot-turn placement on the hand actor"),
        };
        let mut min_y = f64::INFINITY;
        for mesh in &geo {
            if !is_leg_part(&mesh.id) {
                continue;
            }
            for v in &mesh.vertices {
                min_y = min_y.min(probe.apply(*v)[1]);
            }
        }
        if min_y < 0.0 {
            -min_y
        } else {
            0.0
        }
    };
    let b_x = chosen.hand_mid_x - chosen.target_mid_x;
    let slip = |hand_pt: [f64; 2], target: [f64; 2]| {
        let hw = [hand_pt[0], hand_pt[1] + staging.bounds[hand].foot_y];
        let tw = [
            target[0] + b_x,
            target[1] + staging.bounds[band_actor].foot_y,
        ];
        (hw[0] - tw[0]).hypot(hw[1] - tw[1])
    };
    Ok(TuneOutcome {
        b_x,
        target_left: chosen.target_hand_l,
        target_right: chosen.target_hand_r,
        target_row_y: chosen.row,
        predicted_slip: chosen.slip,
        pre_solve_slip: [
            slip(chosen.hands_g.0, chosen.target_hand_l),
            slip(chosen.hands_g.1, chosen.target_hand_r),
        ],
        grasp_arm_l: chosen.g,
        turn_lift,
        crouch_depth: cd,
        crouch_drop,
        hand_rise,
        predicted_clearance,
        waist_band,
    })
}

/// Verify the hand actor's pose-track keys at the family
/// key times resolve to exactly the family maps (one
/// source of truth in the record).
pub fn verify_family_consistency(
    scene: &SceneRecord,
    tune: &TuneRecord,
    outcome: &TuneOutcome,
) -> HostResult<()> {
    let names = outcome.names();
    let track = resolve_pose_track(&scene.actors[tune.hand_actor].pose_track, &names)
        .map_err(|e| e.to_string())?;
    let family = resolve_family(
        tune,
        f64::from(outcome.grasp_arm_l),
        f64::from(outcome.crouch_depth),
    )?;
    let checks = [
        (tune.family.grasp_at, &family.grasp),
        (tune.family.crouch_at, &family.crouch),
        (tune.family.lift_at, &family.lift),
        (tune.family.setdown_at, &family.setdown),
    ];
    for (at, map) in checks {
        let key = track
            .iter()
            .find(|k| k.t == at as f32)
            .ok_or_else(|| format!("no pose key at family time {at}"))?;
        let mut key_params = key.params.clone();
        let mut map_params = map.clone();
        key_params.sort_by(|a, b| a.0.cmp(&b.0));
        map_params.sort_by(|a, b| a.0.cmp(&b.0));
        if key_params != map_params {
            return err(format!(
                "pose key at {at} diverges from the tune family: track {key_params:?} vs family {map_params:?}"
            ));
        }
    }
    Ok(())
}

/// Build the contact seam's scene: contacts anchored on
/// the driven actor's band-part rest centroid, roots from
/// the record. Returns the scene plus the contact defs
/// and their tune-derived offsets.
pub fn build_contact_scene(
    scene: &SceneRecord,
    tune: &TuneOutcome,
    staging: &Staging,
) -> HostResult<(Scene, Vec<ContactDef>, [[f64; 2]; 2])> {
    let dt = 1.0 / scene.rate as f32;
    let b_actor = tune_band_actor(scene)?;
    let band_part = scene
        .tune
        .as_ref()
        .map(|t| t.band_part.clone())
        .unwrap_or_else(|| "Body".to_string());
    let body_default = {
        let mut scratch = PuppetEval::new(&staging.stages[b_actor], &staging.textures[b_actor])
            .map_err(|e| e.to_string())?;
        settle(&mut scratch, scene.settle_steps, dt);
        set_pose(&mut scratch, &[]);
        scratch.end_frame(dt).expect("scratch frame");
        let geo = harvest(&scratch, &staging.models[b_actor]);
        centroid_of(mesh_named(&geo, &band_part))
    };
    let targets = [tune.target_left, tune.target_right];
    if scene.contacts.len() != targets.len() {
        return err(format!(
            "tuned staging anchors exactly {} contacts, scene declares {}",
            targets.len(),
            scene.contacts.len()
        ));
    }
    let mut contacts = Vec::new();
    let mut offsets = [[0.0; 2]; 2];
    for (i, record) in scene.contacts.iter().enumerate() {
        let offset = [
            targets[i][0] - body_default[0],
            targets[i][1] - body_default[1],
        ];
        offsets[i] = offset;
        contacts.push(ContactDef {
            id: record.id.clone(),
            a_rig: record.a_actor,
            a: ContactEndpoint::PartCentroid {
                part: record.a_part.clone(),
            },
            b_rig: record.b_actor,
            b: ContactEndpoint::PartCentroid {
                part: record.b_part.clone(),
            },
            driven_rig: record.driven_actor,
            offset,
        });
    }
    let roots = scene
        .roots
        .iter()
        .map(|part| ContactEndpoint::PartCentroid { part: part.clone() })
        .collect();
    let contact_scene = Scene::new(staging.models.clone(), contacts.clone(), Vec::new(), roots)
        .map_err(|e| e.to_string())?;
    Ok((contact_scene, contacts, offsets))
}

fn tune_band_actor(scene: &SceneRecord) -> HostResult<usize> {
    scene
        .tune
        .as_ref()
        .map(|t| t.band_actor)
        .ok_or_else(|| "scene carries no tune block".to_string())
}

/* ---------------------------------------------------------------- */
/* Simulation pass                                                   */
/* ---------------------------------------------------------------- */

/// One pass's measurements (the seam driver's PassOut).
pub struct PassOut {
    /// Per-step seam reads.
    pub reads: Vec<FrameRead>,
    /// C2 margins per step.
    pub margins: Vec<f64>,
    /// C2 margins without hand points per step.
    pub margins_no_hands: Vec<f64>,
    /// Actor A's Body centroid world y per step.
    pub body_a_y: Vec<f64>,
    /// Actor B's minimum leg-vertex world y per step.
    pub b_clearance: Vec<f64>,
    /// Hand-to-waist-band distances per step.
    pub hand_waist: Vec<[f64; 2]>,
    /// Minimum leg-vertex y over all steps and rigs.
    pub foot_min: f64,
    /// Step of `foot_min`.
    pub foot_min_step: usize,
    /// Minimum leg-vertex y, rig A.
    pub foot_min_a: f64,
    /// Minimum leg-vertex y, rig B.
    pub foot_min_b: f64,
}

/// Everything one simulation pass needs, borrowed.
pub struct SimCfg<'x> {
    /// The scene record.
    pub record: &'x SceneRecord,
    /// Loaded staging.
    pub staging: &'x Staging,
    /// The contact seam's scene.
    pub contact_scene: &'x Scene,
    /// The tune outcome (band + names source).
    pub tune: &'x TuneOutcome,
    /// Resolved pose tracks per actor.
    pub tracks: &'x [Vec<ResolvedPoseKey>],
    /// Hand part names for the hand-to-band instrument.
    pub hand_parts: (String, String),
}

/// Simulate the scene end to end, calling `on_step` with
/// each solved frame (rendering hooks live there) and
/// `on_tick` at each step boundary before evaluation
/// (the core runtime steps there, in lockstep).
pub fn simulate(
    cfg: &SimCfg,
    mut on_step: impl FnMut(
        u64,
        &mut PuppetEval,
        &mut PuppetEval,
        &FrameRead,
        &[EvaluatedMesh],
        &[EvaluatedMesh],
    ),
    mut on_tick: impl FnMut(u64),
) -> HostResult<PassOut> {
    let record = cfg.record;
    let dt = 1.0 / record.rate as f32;
    let names = cfg.tune.names();
    let mut eval_a = PuppetEval::new(&cfg.staging.stages[0], &cfg.staging.textures[0])
        .map_err(|e| e.to_string())?;
    let mut eval_b = PuppetEval::new(&cfg.staging.stages[1], &cfg.staging.textures[1])
        .map_err(|e| e.to_string())?;
    settle(&mut eval_a, record.settle_steps, dt);
    settle(&mut eval_b, record.settle_steps, dt);
    let pivot_a = [
        cfg.staging.bounds[0].foot_centre_x,
        -cfg.staging.bounds[0].foot_y,
    ];
    let band = cfg.tune.waist_band;
    let steps = record.steps as usize;
    let mut reads = Vec::with_capacity(steps);
    let mut margins = Vec::with_capacity(steps);
    let mut margins_no_hands = Vec::with_capacity(steps);
    let mut body_a_y = Vec::with_capacity(steps);
    let mut b_clearance = Vec::with_capacity(steps);
    let mut hand_waist = Vec::with_capacity(steps);
    let mut foot_min = f64::INFINITY;
    let mut foot_min_step = 0usize;
    let mut foot_min_a = f64::INFINITY;
    let mut foot_min_b = f64::INFINITY;
    for step in 0..record.steps {
        on_tick(step);
        let t = step as f64 / f64::from(record.rate);
        let tf = t as f32;
        set_pose(&mut eval_a, &sample_pose_track(&cfg.tracks[0], tf));
        eval_a.end_frame(dt).expect("frame A evaluates");
        set_pose(&mut eval_b, &sample_pose_track(&cfg.tracks[1], tf));
        eval_b.end_frame(dt).expect("frame B evaluates");
        let geo_a = harvest(&eval_a, &cfg.staging.models[0]);
        let geo_b = harvest(&eval_b, &cfg.staging.models[1]);
        let placements = vec![
            sample_placement(
                &record.actors[0].placement,
                &names,
                cfg.staging.bounds[0].foot_y,
                pivot_a,
                t,
            )
            .map_err(|e| e.to_string())?,
            sample_placement(
                &record.actors[1].placement,
                &names,
                cfg.staging.bounds[1].foot_y,
                [0.0, 0.0],
                t,
            )
            .map_err(|e| e.to_string())?,
        ];
        let input = FrameInput {
            step,
            time_seconds: t,
            driven: vec![BTreeMap::new(), BTreeMap::new()],
            placements,
            active: record
                .contacts
                .iter()
                .map(|c| (c.active_from..c.active_to).contains(&t))
                .collect(),
        };
        let read = evaluate_frame_with_geometry(
            cfg.contact_scene,
            &input,
            &[geo_a.clone(), geo_b.clone()],
        )
        .map_err(|e| e.to_string())?;
        let world_a = world_meshes(&cfg.staging.models[0], &geo_a, &read.placements[0]);
        let world_b = world_meshes(&cfg.staging.models[1], &geo_b, &read.placements[1]);
        let points_a = points_of(&geo_a, &read.placements[0], false);
        let points_b = points_of(&geo_b, &read.placements[1], false);
        margins.push(margin_of(&points_a, &world_b).min(margin_of(&points_b, &world_a)));
        let points_a_nh = points_of(&geo_a, &read.placements[0], true);
        let points_b_nh = points_of(&geo_b, &read.placements[1], true);
        margins_no_hands
            .push(margin_of(&points_a_nh, &world_b).min(margin_of(&points_b_nh, &world_a)));
        body_a_y
            .push(centroid_of(mesh_named(&geo_a, "Body"))[1] + read.placements[0].translation[1]);
        // Hand-to-waist-band reads (the D-859 acceptance
        // measure, in the shared scene frame).
        let corners = [
            [band[0], band[1]],
            [band[0], band[3]],
            [band[2], band[1]],
            [band[2], band[3]],
        ];
        let mut blo = [f64::INFINITY; 2];
        let mut bhi = [f64::NEG_INFINITY; 2];
        for c in corners {
            let w = read.placements[1].apply(c);
            blo = [blo[0].min(w[0]), blo[1].min(w[1])];
            bhi = [bhi[0].max(w[0]), bhi[1].max(w[1])];
        }
        let hand_l = endpoint_world(
            &geo_a,
            &read.placements[0],
            &ContactEndpoint::PartCentroid {
                part: cfg.hand_parts.0.clone(),
            },
        );
        let hand_r = endpoint_world(
            &geo_a,
            &read.placements[0],
            &ContactEndpoint::PartCentroid {
                part: cfg.hand_parts.1.clone(),
            },
        );
        hand_waist.push([
            box_distance(hand_l, blo, bhi),
            box_distance(hand_r, blo, bhi),
        ]);
        let mut step_b_clearance = f64::INFINITY;
        for (geo, read_rig) in [(&geo_a, 0usize), (&geo_b, 1usize)] {
            for mesh in geo.iter() {
                if !is_leg_part(&mesh.id) {
                    continue;
                }
                for v in &mesh.vertices {
                    let world = read.placements[read_rig].apply(*v);
                    if world[1] < foot_min {
                        foot_min = world[1];
                        foot_min_step = step as usize;
                    }
                    if read_rig == 0 {
                        foot_min_a = foot_min_a.min(world[1]);
                    } else {
                        foot_min_b = foot_min_b.min(world[1]);
                    }
                    if read_rig == 1 {
                        step_b_clearance = step_b_clearance.min(world[1]);
                    }
                }
            }
        }
        b_clearance.push(step_b_clearance);
        on_step(step, &mut eval_a, &mut eval_b, &read, &geo_a, &geo_b);
        reads.push(read);
    }
    Ok(PassOut {
        reads,
        margins,
        margins_no_hands,
        body_a_y,
        b_clearance,
        hand_waist,
        foot_min,
        foot_min_step,
        foot_min_a,
        foot_min_b,
    })
}
