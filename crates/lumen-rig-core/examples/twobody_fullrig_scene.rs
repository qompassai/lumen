//! The two-body lift-and-hold scene (D-847 Stage 1 timeline)
//! replayed with the estate's own full-body rigs (D-875
//! carry-over): Prima lifts Alma. Both puppets are the D-859
//! full rigs (`prima-full.inx` / `alma-full.inx`), posed
//! through the puppet evaluator of record, their evaluated
//! geometry fed to the two-body solver through
//! `evaluate_frame_with_geometry`, placements solved exactly
//! as in Stage 1 and in the D-857 corpus-puppet run, frames
//! rendered back through the evaluator under the solved
//! placements.
//!
//! Why this cast (measured, see CHARS-REPORT carry-over
//! section): the D-858 census never tried these files — its
//! candidate set held only the older INP 0.8 bust fixtures,
//! which have no limb parts at all. The full rigs carry the
//! channels the corpus pair lacks: arm/elbow deform chains,
//! leg/knee chains, and a real `ParamCrouch`. Contacts bind
//! to Alma's WAIST (her `Body` silhouette inside the 62–78%
//! waist band the D-859 acceptance used), not to a skirt
//! flare.
//!
//! Staging: front-facing puppets, so Prima stands behind
//! Alma, near-concentric (the staging this art supports, as
//! in D-857). The crouch is Prima's `ParamCrouch`; the lift
//! is the crouch recovery plus the arm-pose change; the turn
//! is the solved placement rotation (Stage 1's keys, fully
//! unwound by set-down, as adapted in D-857). Alma's rise is
//! solver-driven through the contacts — her authored
//! placement never lifts her; her knee/arm pose only dresses
//! the carried shape.
//!
//! Usage: twobody_fullrig_scene <prima-import-dir> <alma-import-dir> <output-dir>
//!
//! Coordinate law: the puppet stage lives in the source
//! format's y-DOWN plane; the solver's scene is y-UP. The
//! bridge negates y on harvested geometry, and the render
//! transform for a solved placement (rotation theta,
//! translation T in scene space) is rotation -theta and
//! translation (T.x, -T.y) in puppet space, plus one shared
//! scene-centre offset so both puppets land in one canvas.

use std::collections::BTreeMap;
use std::path::PathBuf;

use image::codecs::gif::{GifEncoder, Repeat};
use image::{Delay, Frame, Rgb, RgbImage, RgbaImage};
use lumen_rig_core::model::{BlendMode, Mesh, Model, Part};
use lumen_rig_core::puppet::{PuppetNodeKind, PuppetStage};
use lumen_rig_core::twobody::{
    ContactDef, ContactEndpoint, EvaluatedMesh, FrameInput, FrameRead, Placement, Scene,
    endpoint_world, evaluate_frame_with_geometry,
};
use lumen_rig_core::{PuppetEval, RgbaImage as TexImage, parse_model, parse_puppet};

const STEPS: u64 = 720;
const DT: f32 = 1.0 / 60.0;
/// D-859 acceptance standard: the lift must clear Alma's
/// feet by at least the 70 units the Torogao-side
/// validation measured.
const CLEARANCE_BAR: f64 = 70.0;
/// Largest crouch hand-drop the staging accepts before the
/// crouch depth is reduced (the driven rig follows the
/// hands down; the drop is the crouch-window floor dip).
const CROUCH_DROP_CAP: f64 = 30.0;

/* ---------------------------------------------------------------- */
/* Loading                                                           */
/* ---------------------------------------------------------------- */

fn load_puppet(dir: &std::path::Path) -> (PuppetStage, Vec<TexImage>) {
    let model_bytes = std::fs::read(dir.join("model.json")).expect("model.json read");
    let model = parse_model(&model_bytes).expect("model parses");
    let stage_bytes = std::fs::read(dir.join("puppet.json")).expect("puppet.json read");
    let stage = parse_puppet(&stage_bytes, &model).expect("stage parses");
    let mut textures = Vec::with_capacity(stage.textures.len());
    for tex in &stage.textures {
        let img = image::open(dir.join(&tex.source))
            .expect("texture decodes")
            .to_rgba8();
        textures.push(TexImage {
            width: img.width() as usize,
            height: img.height() as usize,
            rgba: img.into_raw(),
        });
    }
    (stage, textures)
}

/* ---------------------------------------------------------------- */
/* Bridge: a section-3 model whose meshes mirror the stage's         */
/* part nodes 1:1. The model carries counts and ids only; all        */
/* geometry arrives per-frame through the solver seam.               */
/* ---------------------------------------------------------------- */

fn bridge_model(stage: &PuppetStage, id: &str) -> Model {
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
        generator: Some("twobody-fullrig bridge (D-875)".to_string()),
        parameters: Vec::new(),
        parts,
        textures: Vec::new(),
        deformers: Vec::new(),
        meshes,
        physics: Vec::new(),
    }
}

/// Harvest one frame of evaluated puppet geometry as solver
/// geometry: model space, y negated into the solver's y-UP
/// scene plane.
fn harvest(eval: &PuppetEval, model: &Model) -> Vec<EvaluatedMesh> {
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
                vertices: verts.iter().map(|v| [f64::from(v[0]), -f64::from(v[1])]).collect(),
            }
        })
        .collect()
}

fn centroid_of(mesh: &EvaluatedMesh) -> [f64; 2] {
    let n = mesh.vertices.len() as f64;
    let (mut x, mut y) = (0.0, 0.0);
    for v in &mesh.vertices {
        x += v[0];
        y += v[1];
    }
    [x / n, y / n]
}

fn mesh_named<'m>(meshes: &'m [EvaluatedMesh], id: &str) -> &'m EvaluatedMesh {
    meshes
        .iter()
        .find(|m| m.id == id)
        .unwrap_or_else(|| panic!("mesh {id} harvested"))
}

/// Silhouette x-extent of the named parts' triangles at
/// height y (scene space): intersect every triangle edge
/// with the horizontal line, take min/max crossing x.
fn silhouette_at(
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
    if lo <= hi { Some((lo, hi)) } else { None }
}

/* ---------------------------------------------------------------- */
/* Pose tracks: scalar keyframes over the full-rig parameter         */
/* vocabulary. Every posed parameter defaults to 0, so a key         */
/* lists only what it moves; interpolation is per-parameter          */
/* smoothstep between adjacent keys. The key TIMES and the           */
/* pose family are the D-859 acceptance's, retimed onto the          */
/* 12 s Stage 1 timeline.                                            */
/* ---------------------------------------------------------------- */

type PoseMap = Vec<(&'static str, f32)>;

fn smoothstep(t: f32, t0: f32, t1: f32) -> f32 {
    let f = ((t - t0) / (t1 - t0)).clamp(0.0, 1.0);
    f * f * (3.0 - 2.0 * f)
}

fn pose_as_sets(map: &PoseMap) -> Vec<(&'static str, [f32; 2])> {
    map.iter().map(|(n, v)| (*n, [*v, 0.0])).collect()
}

fn track_at(track: &[(f32, PoseMap)], t: f32) -> Vec<(&'static str, [f32; 2])> {
    if t <= track[0].0 {
        return pose_as_sets(&track[0].1);
    }
    for pair in track.windows(2) {
        let (t0, p0) = &pair[0];
        let (t1, p1) = &pair[1];
        if t <= *t1 {
            let f = if t1 > t0 {
                smoothstep(t, *t0, *t1)
            } else {
                0.0
            };
            let mut names: Vec<&'static str> = Vec::new();
            for (n, _) in p0.iter().chain(p1.iter()) {
                if !names.contains(n) {
                    names.push(n);
                }
            }
            return names
                .iter()
                .map(|n| {
                    let va = p0
                        .iter()
                        .find(|(k, _)| k == n)
                        .map(|(_, v)| *v)
                        .unwrap_or(0.0);
                    let vb = p1
                        .iter()
                        .find(|(k, _)| k == n)
                        .map(|(_, v)| *v)
                        .unwrap_or(0.0);
                    (*n, [va + (vb - va) * f, 0.0])
                })
                .collect();
        }
    }
    pose_as_sets(&track[track.len() - 1].1)
}

fn set_pose(eval: &mut PuppetEval, sets: &[(&str, [f32; 2])]) {
    eval.begin_frame();
    for (id, value) in sets {
        eval.set_param(id, *value).expect("pose parameter exists");
    }
}

fn settle(eval: &mut PuppetEval, frames: usize) {
    for _ in 0..frames {
        set_pose(eval, &[]);
        eval.end_frame(DT).expect("settle frame evaluates");
    }
}

/// The tuned staging: B's authored x, the waist targets on
/// B (local scene space), the predicted rigid residual, the
/// pose family the tune selected, and the waist band.
struct Tune {
    b_x: f64,
    target_left: [f64; 2],
    target_right: [f64; 2],
    target_row_y: f64,
    predicted_slip: f64,
    pre_solve_slip: [f64; 2],
    grasp_arm_l: f32,
    turn_lift: f64,
    crouch_depth: f32,
    crouch_drop: f64,
    hand_rise: f64,
    predicted_clearance: f64,
    waist_band: [f64; 4],
    prima_track: Vec<(f32, PoseMap)>,
    alma_track: Vec<(f32, PoseMap)>,
}

/// The pose family, parameterized by the tuned left-arm
/// grasp value `g` and crouch depth `cd`. Right-arm and
/// elbow values are the D-859 acceptance's; the lift pose
/// is its lift keyframe verbatim.
fn pose_family(g: f32, cd: f32) -> (PoseMap, PoseMap, PoseMap, PoseMap) {
    let grasp: PoseMap = vec![
        ("ParamArmR", 0.9),
        ("ParamElbowR", 0.2),
        ("ParamArmL", g),
        ("ParamElbowL", 0.5),
    ];
    let crouch: PoseMap = vec![
        ("ParamArmR", 1.0),
        ("ParamElbowR", 0.25),
        ("ParamArmL", g + 0.05),
        ("ParamElbowL", 0.55),
        ("ParamCrouch", cd),
    ];
    let lift: PoseMap = vec![
        ("ParamArmR", 1.0),
        ("ParamElbowR", 0.1),
        ("ParamArmL", 1.0),
        ("ParamElbowL", 0.1),
        ("ParamCrouch", cd * 0.3125),
    ];
    let setdown: PoseMap = vec![
        ("ParamArmR", 1.0),
        ("ParamElbowR", 0.25),
        ("ParamArmL", g + 0.05),
        ("ParamElbowL", 0.55),
        ("ParamCrouch", cd * 0.75),
    ];
    (grasp, crouch, lift, setdown)
}

fn prima_track(g: f32, cd: f32) -> Vec<(f32, PoseMap)> {
    let (grasp, crouch, lift, setdown) = pose_family(g, cd);
    vec![
        (0.0, vec![]),
        (2.0, grasp),
        (3.0, crouch.clone()),
        (3.9, crouch),
        (5.0, lift.clone()),
        (8.6, lift),
        (9.6, setdown.clone()),
        (10.0, setdown),
        (11.0, vec![]),
        (12.0, vec![]),
    ]
}

fn alma_track() -> Vec<(f32, PoseMap)> {
    let knees_light: PoseMap = vec![("ParamKneeL", 0.15), ("ParamKneeR", 0.15)];
    let carried: PoseMap = vec![
        ("ParamKneeL", 0.9),
        ("ParamKneeR", 0.9),
        ("ParamArmL", -0.2),
        ("ParamArmR", -0.2),
    ];
    let knees_down: PoseMap = vec![("ParamKneeL", 0.2), ("ParamKneeR", 0.2)];
    vec![
        (0.0, vec![]),
        (2.0, vec![]),
        (3.0, knees_light.clone()),
        (3.9, knees_light),
        (5.0, carried.clone()),
        (8.6, carried),
        (9.6, knees_down.clone()),
        (10.0, knees_down),
        (11.0, vec![]),
        (12.0, vec![]),
    ]
}

/* ---------------------------------------------------------------- */
/* Placements (scene space, y-UP)                                    */
/* ---------------------------------------------------------------- */

/// A's authored placement: ground lift only, plus Stage 1's
/// rotation keys pivoting about A's ground contact point
/// (fully unwound by set-down, the D-857 puppet-scale
/// adaptation).
fn a_placement(t: f64, ground_lift: f64, pivot: [f64; 2], turn_lift: f64) -> Placement {
    const KEYS: [(f64, f64); 7] = [
        (0.0, 0.0),
        (5.0, 0.0),
        (6.5, 26.0),
        (8.0, 26.0),
        (9.6, 0.0),
        (10.4, 0.0),
        (12.0, 0.0),
    ];
    let mut rotation = 0.0;
    for pair in KEYS.windows(2) {
        let (t0, r0) = pair[0];
        let (t1, r1) = pair[1];
        if t <= t1 {
            let f = if t1 > t0 { (t - t0) / (t1 - t0) } else { 0.0 };
            rotation = r0 + f * (r1 - r0);
            break;
        }
    }
    // Pivot fold: T = T_key + c - R(theta) * c, plus the
    // measured rotating-foot compensation pro-rata with
    // |sin(theta)| so the foot corners graze, not cross,
    // the floor through the turn.
    let (sin, cos) = rotation.to_radians().sin_cos();
    let rotated = [cos * pivot[0] - sin * pivot[1], sin * pivot[0] + cos * pivot[1]];
    let lift = turn_lift * sin.abs() / 26.0f64.to_radians().sin();
    Placement {
        translation: [
            pivot[0] - rotated[0],
            ground_lift + lift + pivot[1] - rotated[1],
        ],
        rotation_degrees: rotation,
    }
}

/// B's authored placement: she steps in over [0, 2] (the
/// D-859 acceptance's approach spacing, 90 units), holds the
/// tuned x while the contacts are live (her rise is
/// solver-driven), then steps back over [10, 11] (the
/// acceptance's release spacing, 70 units).
fn b_placement(t: f64, tune: &Tune, ground_lift: f64) -> Placement {
    let step_in = 90.0 * (1.0 - f64::from(smoothstep(t as f32, 0.0, 2.0)));
    let step_back = 70.0 * f64::from(smoothstep(t as f32, 10.0, 11.0));
    Placement {
        translation: [tune.b_x + step_in + step_back, ground_lift],
        rotation_degrees: 0.0,
    }
}

/* ---------------------------------------------------------------- */
/* C2: triangle-based signed margin over evaluated puppet            */
/* meshes. A point's margin is its distance to the closest           */
/* triangle of the other rig, negative when the point lies           */
/* inside any triangle. Per-mesh AABB culling keeps it exact.        */
/* (Instrument unchanged from the D-857 driver.)                     */
/* ---------------------------------------------------------------- */

struct WorldMesh {
    aabb: ([f64; 2], [f64; 2]),
    tris: Vec<[[f64; 2]; 3]>,
}

fn world_meshes(model: &Model, evaluated: &[EvaluatedMesh], placement: &Placement) -> Vec<WorldMesh> {
    model
        .meshes
        .iter()
        .zip(evaluated.iter())
        .map(|(declared, eval)| {
            let world: Vec<[f64; 2]> = eval.vertices.iter().map(|v| placement.apply(*v)).collect();
            let mut lo = [f64::INFINITY; 2];
            let mut hi = [f64::NEG_INFINITY; 2];
            for v in &world {
                lo = [lo[0].min(v[0]), lo[1].min(v[1])];
                hi = [hi[0].max(v[0]), hi[1].max(v[1])];
            }
            let tris = declared
                .indices
                .as_chunks::<3>()
                .0
                .iter()
                .map(|tri| {
                    [
                        world[tri[0] as usize],
                        world[tri[1] as usize],
                        world[tri[2] as usize],
                    ]
                })
                .collect();
            WorldMesh {
                aabb: (lo, hi),
                tris,
            }
        })
        .collect()
}

fn point_segment_distance(p: [f64; 2], a: [f64; 2], b: [f64; 2]) -> f64 {
    let ab = [b[0] - a[0], b[1] - a[1]];
    let len2 = ab[0] * ab[0] + ab[1] * ab[1];
    let f = if len2 > 0.0 {
        (((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1]) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let q = [a[0] + f * ab[0], a[1] + f * ab[1]];
    (p[0] - q[0]).hypot(p[1] - q[1])
}

fn point_triangle_signed(p: [f64; 2], tri: &[[f64; 2]; 3]) -> f64 {
    let cross = |a: [f64; 2], b: [f64; 2], q: [f64; 2]| {
        (b[0] - a[0]) * (q[1] - a[1]) - (b[1] - a[1]) * (q[0] - a[0])
    };
    let d0 = cross(tri[0], tri[1], p);
    let d1 = cross(tri[1], tri[2], p);
    let d2 = cross(tri[2], tri[0], p);
    let inside = (d0 >= 0.0 && d1 >= 0.0 && d2 >= 0.0) || (d0 <= 0.0 && d1 <= 0.0 && d2 <= 0.0);
    let dist = point_segment_distance(p, tri[0], tri[1])
        .min(point_segment_distance(p, tri[1], tri[2]))
        .min(point_segment_distance(p, tri[2], tri[0]));
    if inside { -dist } else { dist }
}

/// Minimum signed margin of `points` against the other
/// rig's triangle soup.
fn margin_of(points: &[[f64; 2]], others: &[WorldMesh]) -> f64 {
    let mut best = f64::INFINITY;
    for &p in points {
        for mesh in others {
            let (lo, hi) = mesh.aabb;
            let dx = (lo[0] - p[0]).max(p[0] - hi[0]).max(0.0);
            let dy = (lo[1] - p[1]).max(p[1] - hi[1]).max(0.0);
            if (dx * dx + dy * dy).sqrt() > best {
                continue;
            }
            for tri in &mesh.tris {
                let m = point_triangle_signed(p, tri);
                if m < best {
                    best = m;
                }
            }
        }
    }
    best
}

fn is_leg_part(id: &str) -> bool {
    id.starts_with("Foot") || id.starts_with("Shin") || id.starts_with("Thigh")
}

fn points_of(evaluated: &[EvaluatedMesh], placement: &Placement, exclude_hands: bool) -> Vec<[f64; 2]> {
    evaluated
        .iter()
        .filter(|m| !exclude_hands || !(m.id == "HandL" || m.id == "HandR"))
        .flat_map(|m| m.vertices.iter().map(|v| placement.apply(*v)))
        .collect()
}

/* ---------------------------------------------------------------- */
/* Rendering                                                         */
/* ---------------------------------------------------------------- */

const CANVAS_W: usize = 1100;
const CANVAS_H: usize = 1050;
const SCALE: f32 = 2.0;
const GIF_W: usize = 800;
const GIF_H: usize = 764;
const GIF_SCALE: f32 = 1.4545;

/// Puppet-space scene transform for a solved placement:
/// rotation -theta, translation (T.x - CX, -T.y + CY) for
/// scene centre (CX, CY) (see the module docs).
fn scene_transform(placement: &Placement, centre: [f64; 2]) -> ([f32; 2], f32) {
    (
        [
            (placement.translation[0] - centre[0]) as f32,
            (-placement.translation[1] + centre[1]) as f32,
        ],
        -placement.rotation_degrees as f32,
    )
}

fn composite(
    eval_a: &mut PuppetEval,
    eval_b: &mut PuppetEval,
    read: &FrameRead,
    centre: [f64; 2],
    width: usize,
    height: usize,
    scale: f32,
) -> RgbaImage {
    let (ta, ra) = scene_transform(&read.placements[0], centre);
    eval_a.set_scene_transform(ta, ra);
    let frame_a = eval_a.render(width, height, scale);
    let (tb, rb) = scene_transform(&read.placements[1], centre);
    eval_b.set_scene_transform(tb, rb);
    let frame_b = eval_b.render(width, height, scale);
    eval_a.clear_scene_transform();
    eval_b.clear_scene_transform();
    let mut out = RgbaImage::from_pixel(width as u32, height as u32, image::Rgba([24, 24, 28, 255]));
    for (i, px) in frame_a.as_chunks::<4>().0.iter().enumerate() {
        blend_over(&mut out, i, px);
    }
    for (i, px) in frame_b.as_chunks::<4>().0.iter().enumerate() {
        blend_over(&mut out, i, px);
    }
    out
}

fn blend_over(out: &mut RgbaImage, index: usize, src: &[u8; 4]) {
    let w = out.width() as usize;
    let (x, y) = ((index % w) as u32, (index / w) as u32);
    let dst = out.get_pixel(x, y);
    let sa = f32::from(src[3]) / 255.0;
    let da = f32::from(dst[3]) / 255.0;
    let oa = sa + da * (1.0 - sa);
    if oa <= 0.0 {
        return;
    }
    let mut px = [0u8; 4];
    for c in 0..3 {
        let v = (f32::from(src[c]) * sa + f32::from(dst[c]) * da * (1.0 - sa)) / oa;
        px[c] = v.round().clamp(0.0, 255.0) as u8;
    }
    px[3] = (oa * 255.0).round() as u8;
    out.put_pixel(x, y, image::Rgba(px));
}

/// Scene point -> canvas pixel (float), for annotations.
fn to_px(p: [f64; 2], centre: [f64; 2], width: usize, height: usize, scale: f32) -> (f64, f64) {
    (
        (p[0] - centre[0]) * f64::from(scale) + f64::from(width as u32) / 2.0,
        f64::from(height as u32) / 2.0 - (p[1] - centre[1]) * f64::from(scale),
    )
}

fn disc(
    img: &mut RgbImage,
    centre: [f64; 2],
    radius_units: f64,
    color: Rgb<u8>,
    scene_centre: [f64; 2],
    scale: f32,
) {
    let (cx, cy) = to_px(centre, scene_centre, CANVAS_W, CANVAS_H, scale);
    let r = radius_units * f64::from(scale);
    let x0 = (cx - r).floor().max(0.0) as u32;
    let x1 = (cx + r).ceil().min(f64::from(CANVAS_W as u32) - 1.0) as u32;
    let y0 = (cy - r).floor().max(0.0) as u32;
    let y1 = (cy + r).ceil().min(f64::from(CANVAS_H as u32) - 1.0) as u32;
    for y in y0..=y1 {
        for x in x0..=x1 {
            let dx = f64::from(x) - cx;
            let dy = f64::from(y) - cy;
            if dx * dx + dy * dy <= r * r {
                img.put_pixel(x, y, color);
            }
        }
    }
}

fn annotate(
    img: &mut RgbImage,
    read: &FrameRead,
    geo_a: &[EvaluatedMesh],
    geo_b: &[EvaluatedMesh],
    contacts: &[ContactDef],
    centre: [f64; 2],
    scale: f32,
) {
    // Ground line at scene y = 0.
    let (_, gy) = to_px([0.0, 0.0], centre, CANVAS_W, CANVAS_H, scale);
    let gy = gy.round() as u32;
    if gy < CANVAS_H as u32 {
        for x in 0..CANVAS_W as u32 {
            img.put_pixel(x, gy, Rgb([110, 110, 110]));
        }
    }
    for (index, contact) in contacts.iter().enumerate() {
        if !read.contacts[index].engaged {
            continue;
        }
        let a_point = endpoint_world(geo_a, &read.placements[0], &contact.a);
        disc(img, a_point, 26.0, Rgb([255, 60, 60]), centre, scale);
        let b_local = endpoint_world(geo_b, &Placement::IDENTITY, &contact.b);
        let fixed = read.placements[1].apply([
            b_local[0] + contact.offset[0],
            b_local[1] + contact.offset[1],
        ]);
        disc(img, fixed, 17.0, Rgb([255, 255, 255]), centre, scale);
    }
    for root in &read.roots {
        disc(img, *root, 20.0, Rgb([255, 220, 60]), centre, scale);
    }
}

/* ---------------------------------------------------------------- */
/* Bounds                                                            */
/* ---------------------------------------------------------------- */

struct Bounds {
    height: f64,
    /// Puppet-space y of the lowest leg vertex (feet), the
    /// ground lift that puts the feet on scene y = 0.
    foot_y: f64,
    /// Local scene-space x centre of the feet band.
    foot_centre_x: f64,
}

fn measure_bounds(eval: &mut PuppetEval, model: &Model) -> Bounds {
    set_pose(eval, &[]);
    eval.end_frame(DT).expect("bounds frame evaluates");
    let mut lo = f64::INFINITY;
    let mut hi = f64::NEG_INFINITY;
    let mut foot_y = f64::NEG_INFINITY;
    for mesh in &model.meshes {
        let verts = eval
            .node_world_vertices(&mesh.id)
            .expect("bounds vertices");
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
        let verts = eval
            .node_world_vertices(&mesh.id)
            .expect("bounds vertices");
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

/* ---------------------------------------------------------------- */
/* Grasp tune: scan the D-859 waist band on Alma's Body              */
/* silhouette against Prima's hand pair over the pose family.        */
/* The objective is the D-857 one — minimum true Kabsch              */
/* residual (separation mismatch / 2) — subject to the               */
/* D-859 clearance bar: the predicted hold clearance               */
/* (lift-pose hand height vs the target row, both grounded)          */
/* must reach 70 units, and the grasp itself must engage at          */
/* roughly ground level. The crouch depth is then the                */
/* largest of the family's whose hand drop keeps the driven          */
/* rig's crouch-window dip inside the cap.                           */
/* ---------------------------------------------------------------- */

fn hand_pair(
    eval_a: &mut PuppetEval,
    model_a: &Model,
    map: &PoseMap,
) -> ([f64; 2], [f64; 2]) {
    set_pose(eval_a, &pose_as_sets(map));
    eval_a.end_frame(DT).expect("pose frame A");
    let geo = harvest(eval_a, model_a);
    (
        centroid_of(mesh_named(&geo, "HandL")),
        centroid_of(mesh_named(&geo, "HandR")),
    )
}

fn tune(
    eval_a: &mut PuppetEval,
    eval_b: &mut PuppetEval,
    model_a: &Model,
    model_b: &Model,
    bounds_a: &Bounds,
    bounds_b: &Bounds,
) -> Tune {
    settle(eval_a, 30);
    settle(eval_b, 30);
    // Alma at rest: Body AABB and the waist band (62–78% of
    // the Body part measured from its top, the D-859
    // acceptance's band), in B-local scene coordinates.
    set_pose(eval_b, &[]);
    eval_b.end_frame(DT).expect("frame B");
    let geo_b = harvest(eval_b, model_b);
    let body = mesh_named(&geo_b, "Body");
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
    let band_lo = by1 - 0.78 * bh;
    let band_hi = by1 - 0.62 * bh;
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
    for g in [0.25f32, 0.4, 0.55, 0.7, 0.85] {
        let (grasp, crouch, lift, _) = pose_family(g, 0.8);
        let hands_g = hand_pair(eval_a, model_a, &grasp);
        let hands_c = hand_pair(eval_a, model_a, &crouch);
        let hands_l = hand_pair(eval_a, model_a, &lift);
        let mid_g = [
            (hands_g.0[0] + hands_g.1[0]) / 2.0,
            (hands_g.0[1] + hands_g.1[1]) / 2.0,
        ];
        let mid_c = [
            (hands_c.0[0] + hands_c.1[0]) / 2.0,
            (hands_c.0[1] + hands_c.1[1]) / 2.0,
        ];
        let mid_l = [
            (hands_l.0[0] + hands_l.1[0]) / 2.0,
            (hands_l.0[1] + hands_l.1[1]) / 2.0,
        ];
        let drop08 = mid_g[1] - mid_c[1];
        let hand_sep = (hands_g.0[0] - hands_g.1[0]).hypot(hands_g.0[1] - hands_g.1[1]);
        // HandL is the -x hand on these rigs (measured at
        // rest: HandL centroid x < HandR centroid x); assign
        // targets by measured side, never by assumption.
        let hand_l_is_neg = hands_g.0[0] <= hands_g.1[0];
        let mut row = band_lo;
        while row <= band_hi {
            if let Some((lo, hi)) = silhouette_at(model_b, &geo_b, &["Body"], row) {
                let target_neg = [lo, row];
                let target_pos = [hi, row];
                let target_sep = target_pos[0] - target_neg[0];
                let slip = (hand_sep - target_sep).abs() / 2.0;
                let engage = (mid_g[1] + bounds_a.foot_y) - (row + bounds_b.foot_y);
                let clearance = (mid_l[1] + bounds_a.foot_y) - (row + bounds_b.foot_y);
                let (target_hand_l, target_hand_r) = if hand_l_is_neg {
                    (target_neg, target_pos)
                } else {
                    (target_pos, target_neg)
                };
                let cand = Cand {
                    slip,
                    engage,
                    clearance,
                    g,
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
                let feasible = cand.clearance >= CLEARANCE_BAR && cand.engage >= -8.0;
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
    // crouch-window dip (engage shift minus crouch hand
    // drop, drop ~ linear in depth) inside the cap.
    let mut cd = 0.8f32;
    if chosen.drop08 > 0.0 {
        let dip_at_08 = chosen.engage - chosen.drop08;
        if dip_at_08 < -CROUCH_DROP_CAP {
            let allowed = (chosen.engage + CROUCH_DROP_CAP) / chosen.drop08;
            cd = (0.8 * allowed).clamp(0.3, 0.8) as f32;
        }
    }
    let (_, crouch_cd, lift_cd, _) = pose_family(chosen.g, cd);
    let hands_c = hand_pair(eval_a, model_a, &crouch_cd);
    let hands_l = hand_pair(eval_a, model_a, &lift_cd);
    let mid_g = [
        (chosen.hands_g.0[0] + chosen.hands_g.1[0]) / 2.0,
        (chosen.hands_g.0[1] + chosen.hands_g.1[1]) / 2.0,
    ];
    let mid_c = [
        (hands_c.0[0] + hands_c.1[0]) / 2.0,
        (hands_c.0[1] + hands_c.1[1]) / 2.0,
    ];
    let mid_l = [
        (hands_l.0[0] + hands_l.1[0]) / 2.0,
        (hands_l.0[1] + hands_l.1[1]) / 2.0,
    ];
    let crouch_drop = mid_g[1] - mid_c[1];
    let hand_rise = mid_l[1] - mid_g[1];
    let predicted_clearance = (mid_l[1] + bounds_a.foot_y) - (chosen.row + bounds_b.foot_y);
    // Turn compensation: rotating A about her foot-centre
    // pivot swings the foot plates' corners below the
    // floor (measured here on the rest geometry at the
    // peak 26-degree key); the placement lift that exactly
    // cancels the dip is applied pro-rata with |sin| of
    // the current rotation in a_placement.
    let turn_lift = {
        set_pose(eval_a, &[]);
        eval_a.end_frame(DT).expect("turn probe frame A");
        let geo = harvest(eval_a, model_a);
        let pivot = [bounds_a.foot_centre_x, -bounds_a.foot_y];
        let probe = a_placement(6.5, bounds_a.foot_y, pivot, 0.0);
        let mut min_y = f64::INFINITY;
        for mesh in &geo {
            if !is_leg_part(&mesh.id) {
                continue;
            }
            for v in &mesh.vertices {
                min_y = min_y.min(probe.apply(*v)[1]);
            }
        }
        if min_y < 0.0 { -min_y } else { 0.0 }
    };
    let b_x = chosen.hand_mid_x - chosen.target_mid_x;
    let slip = |hand: [f64; 2], target: [f64; 2]| {
        let hw = [hand[0], hand[1] + bounds_a.foot_y];
        let tw = [target[0] + b_x, target[1] + bounds_b.foot_y];
        (hw[0] - tw[0]).hypot(hw[1] - tw[1])
    };
    Tune {
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
        prima_track: prima_track(chosen.g, cd),
        alma_track: alma_track(),
    }
}

/* ---------------------------------------------------------------- */
/* Simulation pass                                                   */
/* ---------------------------------------------------------------- */

struct PassOut {
    reads: Vec<FrameRead>,
    margins: Vec<f64>,
    margins_no_hands: Vec<f64>,
    body_a_y: Vec<f64>,
    b_clearance: Vec<f64>,
    hand_waist: Vec<[f64; 2]>,
    foot_min: f64,
    foot_min_step: usize,
    foot_min_a: f64,
    foot_min_b: f64,
}

/// Everything one simulation pass needs, borrowed.
struct SimCfg<'x> {
    stage_a: &'x PuppetStage,
    tex_a: &'x [TexImage],
    stage_b: &'x PuppetStage,
    tex_b: &'x [TexImage],
    model_a: &'x Model,
    model_b: &'x Model,
    scene: &'x Scene,
    tune: &'x Tune,
    bounds_a: &'x Bounds,
    bounds_b: &'x Bounds,
}

/// Distance from a point to an axis-aligned box (0 inside).
fn box_distance(p: [f64; 2], lo: [f64; 2], hi: [f64; 2]) -> f64 {
    let dx = (lo[0] - p[0]).max(p[0] - hi[0]).max(0.0);
    let dy = (lo[1] - p[1]).max(p[1] - hi[1]).max(0.0);
    dx.hypot(dy)
}

fn simulate(
    cfg: &SimCfg,
    mut on_step: impl FnMut(
        u64,
        &mut PuppetEval,
        &mut PuppetEval,
        &FrameRead,
        &[EvaluatedMesh],
        &[EvaluatedMesh],
    ),
) -> PassOut {
    let mut eval_a = PuppetEval::new(cfg.stage_a, cfg.tex_a).expect("eval A builds");
    let mut eval_b = PuppetEval::new(cfg.stage_b, cfg.tex_b).expect("eval B builds");
    settle(&mut eval_a, 30);
    settle(&mut eval_b, 30);
    let pivot_a = [cfg.bounds_a.foot_centre_x, -cfg.bounds_a.foot_y];
    let band = cfg.tune.waist_band;
    let mut reads = Vec::with_capacity(STEPS as usize);
    let mut margins = Vec::with_capacity(STEPS as usize);
    let mut margins_no_hands = Vec::with_capacity(STEPS as usize);
    let mut body_a_y = Vec::with_capacity(STEPS as usize);
    let mut b_clearance = Vec::with_capacity(STEPS as usize);
    let mut hand_waist = Vec::with_capacity(STEPS as usize);
    let mut foot_min = f64::INFINITY;
    let mut foot_min_step = 0usize;
    let mut foot_min_a = f64::INFINITY;
    let mut foot_min_b = f64::INFINITY;
    for step in 0..STEPS {
        let t = step as f64 / 60.0;
        let tf = t as f32;
        set_pose(&mut eval_a, &track_at(&cfg.tune.prima_track, tf));
        eval_a.end_frame(DT).expect("frame A evaluates");
        set_pose(&mut eval_b, &track_at(&cfg.tune.alma_track, tf));
        eval_b.end_frame(DT).expect("frame B evaluates");
        let geo_a = harvest(&eval_a, cfg.model_a);
        let geo_b = harvest(&eval_b, cfg.model_b);
        let input = FrameInput {
            step,
            time_seconds: t,
            driven: vec![BTreeMap::new(), BTreeMap::new()],
            placements: vec![
                a_placement(t, cfg.bounds_a.foot_y, pivot_a, cfg.tune.turn_lift),
                b_placement(t, cfg.tune, cfg.bounds_b.foot_y),
            ],
            active: vec![(2.0..10.0).contains(&t), (2.0..10.0).contains(&t)],
        };
        let read = evaluate_frame_with_geometry(cfg.scene, &input, &[geo_a.clone(), geo_b.clone()])
            .expect("frame solves");
        // C2 (driver-side, triangle metric) + foot reads at
        // the SOLVED placements.
        let world_a = world_meshes(cfg.model_a, &geo_a, &read.placements[0]);
        let world_b = world_meshes(cfg.model_b, &geo_b, &read.placements[1]);
        let points_a = points_of(&geo_a, &read.placements[0], false);
        let points_b = points_of(&geo_b, &read.placements[1], false);
        margins.push(margin_of(&points_a, &world_b).min(margin_of(&points_b, &world_a)));
        let points_a_nh = points_of(&geo_a, &read.placements[0], true);
        let points_b_nh = points_of(&geo_b, &read.placements[1], true);
        margins_no_hands.push(
            margin_of(&points_a_nh, &world_b).min(margin_of(&points_b_nh, &world_a)),
        );
        body_a_y.push(centroid_of(mesh_named(&geo_a, "Body"))[1] + read.placements[0].translation[1]);
        // Hand-to-waist-band reads (the D-859 acceptance
        // measure, in the shared scene frame): Prima's hand
        // centroids at the solved A placement vs Alma's
        // waist band box at the solved B placement.
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
                part: "HandL".into(),
            },
        );
        let hand_r = endpoint_world(
            &geo_a,
            &read.placements[0],
            &ContactEndpoint::PartCentroid {
                part: "HandR".into(),
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
    PassOut {
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
    }
}

fn percentile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let index = ((sorted.len() - 1) as f64 * q).round() as usize;
    sorted[index]
}

fn main() {
    let mut args = std::env::args().skip(1);
    let a_dir = PathBuf::from(args.next().expect("usage: <prima-dir> <alma-dir> <out>"));
    let b_dir = PathBuf::from(args.next().expect("usage: <prima-dir> <alma-dir> <out>"));
    let out = PathBuf::from(args.next().expect("usage: <prima-dir> <alma-dir> <out>"));
    std::fs::create_dir_all(&out).expect("output dir");

    let (stage_a, tex_a) = load_puppet(&a_dir);
    let (stage_b, tex_b) = load_puppet(&b_dir);
    let model_a = bridge_model(&stage_a, "bridge-prima");
    let model_b = bridge_model(&stage_b, "bridge-alma");

    let mut scratch_a = PuppetEval::new(&stage_a, &tex_a).expect("eval A");
    settle(&mut scratch_a, 30);
    let bounds_a = measure_bounds(&mut scratch_a, &model_a);
    let mut scratch_b = PuppetEval::new(&stage_b, &tex_b).expect("eval B");
    settle(&mut scratch_b, 30);
    let bounds_b = measure_bounds(&mut scratch_b, &model_b);
    println!(
        "roles: A (lifter) = Prima H={:.1} foot_y={:.1}; B (lifted) = Alma H={:.1} foot_y={:.1}",
        bounds_a.height, bounds_a.foot_y, bounds_b.height, bounds_b.foot_y
    );

    let mut tune_a = PuppetEval::new(&stage_a, &tex_a).expect("tune eval A");
    let mut tune_b = PuppetEval::new(&stage_b, &tex_b).expect("tune eval B");
    let tune = tune(
        &mut tune_a, &mut tune_b, &model_a, &model_b, &bounds_a, &bounds_b,
    );
    println!(
        "tune: b_x={:.2} row_y={:.1} predicted_slip={:.2} pre_solve_slip=[{:.2}, {:.2}] grasp_arm_l={} crouch_depth={} crouch_drop={:.1} hand_rise={:.1} predicted_clearance={:.1}",
        tune.b_x,
        tune.target_row_y,
        tune.predicted_slip,
        tune.pre_solve_slip[0],
        tune.pre_solve_slip[1],
        tune.grasp_arm_l,
        tune.crouch_depth,
        tune.crouch_drop,
        tune.hand_rise,
        tune.predicted_clearance
    );
    drop(tune_a);
    drop(tune_b);

    // Contacts: Prima's hand centroids onto the tuned waist
    // targets, anchored on Alma's Body centroid frame.
    let body_b_default = {
        let mut scratch = PuppetEval::new(&stage_b, &tex_b).expect("scratch B");
        settle(&mut scratch, 30);
        set_pose(&mut scratch, &[]);
        scratch.end_frame(DT).expect("scratch frame");
        let geo = harvest(&scratch, &model_b);
        centroid_of(mesh_named(&geo, "Body"))
    };
    let contacts = vec![
        ContactDef {
            id: "c_hand_l".into(),
            a_rig: 0,
            a: ContactEndpoint::PartCentroid {
                part: "HandL".into(),
            },
            b_rig: 1,
            b: ContactEndpoint::PartCentroid {
                part: "Body".into(),
            },
            driven_rig: 1,
            offset: [
                tune.target_left[0] - body_b_default[0],
                tune.target_left[1] - body_b_default[1],
            ],
        },
        ContactDef {
            id: "c_hand_r".into(),
            a_rig: 0,
            a: ContactEndpoint::PartCentroid {
                part: "HandR".into(),
            },
            b_rig: 1,
            b: ContactEndpoint::PartCentroid {
                part: "Body".into(),
            },
            driven_rig: 1,
            offset: [
                tune.target_right[0] - body_b_default[0],
                tune.target_right[1] - body_b_default[1],
            ],
        },
    ];
    let scene = Scene::new(
        vec![model_a.clone(), model_b.clone()],
        contacts.clone(),
        Vec::new(),
        vec![
            ContactEndpoint::PartCentroid {
                part: "SkirtFull".into(),
            },
            ContactEndpoint::PartCentroid {
                part: "SkirtFull".into(),
            },
        ],
    )
    .expect("scene constructs");

    let centre = [tune.b_x / 2.0, bounds_b.height / 2.0];

    // Pass 1: measurements + annotated key frames.
    let key_frames: [(u64, &str); 6] = [
        (60, "approach-t1.0"),
        (120, "grasp-t2.0"),
        (240, "lift-t4.0"),
        (390, "turn-t6.5"),
        (570, "setdown-t9.5"),
        (660, "release-t11.0"),
    ];
    let cfg = SimCfg {
        stage_a: &stage_a,
        tex_a: &tex_a,
        stage_b: &stage_b,
        tex_b: &tex_b,
        model_a: &model_a,
        model_b: &model_b,
        scene: &scene,
        tune: &tune,
        bounds_a: &bounds_a,
        bounds_b: &bounds_b,
    };
    let pass1 = simulate(
        &cfg,
        |step, eval_a, eval_b, read, geo_a, geo_b| {
            if let Some((_, label)) = key_frames.iter().find(|(s, _)| *s == step) {
                let frame = composite(
                    eval_a, eval_b, read, centre, CANVAS_W, CANVAS_H, SCALE,
                );
                let mut img = RgbImage::from_fn(CANVAS_W as u32, CANVAS_H as u32, |x, y| {
                    let p = frame.get_pixel(x, y);
                    Rgb([p[0], p[1], p[2]])
                });
                annotate(&mut img, read, geo_a, geo_b, &contacts, centre, SCALE);
                img.save(out.join(format!("frame-{label}.png")))
                    .expect("key frame saves");
            }
        },
    );

    // Worst-frame selection, Stage 1's rule scaled to this
    // scene (largest engaged slip over 0.03% of A's height,
    // else smallest margin), plus the approach/grasp-window
    // margin worst as the honest FAIL exemplar.
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
    let (margin_worst, margin_worst_step) = pass1
        .margins
        .iter()
        .enumerate()
        .fold((f64::INFINITY, 0usize), |acc, (i, &m)| {
            if m < acc.0 { (m, i) } else { acc }
        });
    let (approach_worst, approach_worst_step) = pass1.margins[..180]
        .iter()
        .enumerate()
        .fold((f64::INFINITY, 0usize), |acc, (i, &m)| {
            if m < acc.0 { (m, i) } else { acc }
        });
    let worst_step = if by_slip.0 > 0.0003 * bounds_a.height {
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

    // Pass 2: GIF frames + worst-frame renders; reads must
    // reproduce pass 1 exactly (in-process determinism).
    let mut gif_frames: Vec<RgbaImage> = Vec::new();
    let pass2 = simulate(
        &cfg,
        |step, eval_a, eval_b, read, geo_a, geo_b| {
            if step % 4 == 0 {
                gif_frames.push(composite(
                    eval_a, eval_b, read, centre, GIF_W, GIF_H, GIF_SCALE,
                ));
            }
            if let Some((_, label)) = wanted.iter().find(|(s, _)| *s == step) {
                let frame = composite(
                    eval_a, eval_b, read, centre, CANVAS_W, CANVAS_H, SCALE,
                );
                let mut img = RgbImage::from_fn(CANVAS_W as u32, CANVAS_H as u32, |x, y| {
                    let p = frame.get_pixel(x, y);
                    Rgb([p[0], p[1], p[2]])
                });
                annotate(&mut img, read, geo_a, geo_b, &contacts, centre, SCALE);
                img.save(out.join(format!("frame-{label}.png")))
                    .expect("worst frame saves");
            }
        },
    );
    assert_eq!(
        serde_json::to_string(&pass1.reads).expect("reads serialize"),
        serde_json::to_string(&pass2.reads).expect("reads serialize"),
        "pass 2 reads reproduce pass 1"
    );

    let mut gif_file = std::fs::File::create(out.join("scene.gif")).expect("gif file");
    let mut encoder = GifEncoder::new(&mut gif_file);
    encoder.set_repeat(Repeat::Infinite).expect("gif repeat");
    encoder
        .encode_frames(gif_frames.into_iter().map(|img| {
            Frame::from_parts(img, 0, 0, Delay::from_numer_denom_ms(200, 3))
        }))
        .expect("gif encodes");

    std::fs::write(
        out.join("reads.json"),
        serde_json::to_string(&pass1.reads).expect("reads serialize"),
    )
    .expect("reads write");

    /* ---------------- measurements ---------------- */
    let phase = |lo: usize, hi: usize| -> (f64, f64) {
        let mut slips: Vec<f64> = pass1.reads[lo..hi]
            .iter()
            .flat_map(|r| r.contacts.iter().filter(|c| c.engaged).map(|c| c.slip))
            .collect();
        slips.sort_by(f64::total_cmp);
        (percentile(&slips, 0.95), slips.last().copied().unwrap_or(f64::NAN))
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
                if m < acc.0 { (m, i + lo) } else { acc }
            })
    };
    let (engaged_margin, engaged_margin_step) = window_min(&pass1.margins, 120, 600);
    let (approach_margin, approach_margin_step) = window_min(&pass1.margins, 0, 120);
    let (engaged_margin_nh, engaged_margin_nh_step) =
        window_min(&pass1.margins_no_hands, 120, 600);
    let root_a: Vec<f64> = pass1.reads.iter().map(|r| r.roots[0][1]).collect();
    let root_b: Vec<f64> = pass1.reads.iter().map(|r| r.roots[1][1]).collect();
    let rest_a = root_a[..60].iter().sum::<f64>() / 60.0;
    let rest_b = root_b[..60].iter().sum::<f64>() / 60.0;
    let dip = root_a[228..300].iter().fold(f64::INFINITY, |x, &y| x.min(y));
    let body_rest = pass1.body_a_y[..60].iter().sum::<f64>() / 60.0;
    let body_dip = pass1.body_a_y[120..240]
        .iter()
        .fold(f64::INFINITY, |x, &y| x.min(y));
    let converged_seam = pass1.reads.iter().filter(|r| r.converged).count();
    let converged_equiv = pass1
        .reads
        .iter()
        .zip(pass1.margins_no_hands.iter())
        .filter(|(r, m)| {
            r.contacts.iter().all(|c| !c.engaged || c.slip <= 1e-6) && **m >= 0.0
        })
        .count();
    let hw_at = |step: usize| pass1.hand_waist[step];
    let hw_engaged_max = |hand: usize| {
        pass1.hand_waist[120..600]
            .iter()
            .map(|h| h[hand])
            .fold(0.0f64, f64::max)
    };

    let summary = serde_json::json!({
        "roles": {"a_lifter": "Prima", "b_lifted": "Alma",
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
                 "contact_offsets": [contacts[0].offset, contacts[1].offset]},
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
                        "time_seconds": worst_step as f64 / 60.0},
        "converged_frames_solver_seam": converged_seam,
        "converged_frames_with_c2_no_hands": converged_equiv,
        "total_frames": pass1.reads.len()
    });
    std::fs::write(
        out.join("summary.json"),
        serde_json::to_string_pretty(&summary).expect("summary serializes"),
    )
    .expect("summary writes");
    println!("{}", serde_json::to_string_pretty(&summary).unwrap());
}
