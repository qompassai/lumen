//! The two-body lift-and-hold scene (D-847 Stage 1):
//! evaluate all 720 steps, export the canonical frame
//! reads, compute the per-criterion measurements from
//! TWOBODY-CRITERIA.md, and render annotated frames.
//!
//! Usage: twobody_scene <output-dir>
//!
//! Outputs in <output-dir>:
//! - reads.json: canonical FrameRead for every step
//!   (the determinism double-bake compares this file
//!   byte-for-byte across runs).
//! - summary.json: per-criterion aggregates (C1..C6).
//! - frame-<label>.png: annotated renders at the contact
//!   moments and at the honest worst frame.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use image::{Rgb, RgbImage};
use lumen_rig_core::twobody::{
    ContactDef, ContactEndpoint, EvaluatedMesh, FrameInput, FrameRead, Placement, ProbeDef, Scene,
    endpoint_world, evaluate_frame, evaluate_model, resolve_parameters,
};
use lumen_rig_core::{Motion, curve_value_at, parse_model, parse_motion};

const B_X: f64 = 54.778_421_414;
const FIGURE_HEIGHT: f64 = 170.0;

fn fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../fixtures/twobody")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn driven_at(motion: &Motion, time: f64) -> BTreeMap<String, f64> {
    motion
        .curves
        .iter()
        .map(|curve| {
            (
                curve.parameter.clone(),
                curve_value_at(&curve.keyframes, time),
            )
        })
        .collect()
}

fn vertex(mesh: &str, vertex: u32) -> ContactEndpoint {
    ContactEndpoint::Vertex {
        mesh: mesh.into(),
        vertex,
    }
}

fn centroid(part: &str) -> ContactEndpoint {
    ContactEndpoint::PartCentroid { part: part.into() }
}

fn grasp_contacts() -> Vec<ContactDef> {
    vec![
        ContactDef {
            id: "c_near".into(),
            a_rig: 0,
            a: vertex("arm_near_fore", 3),
            b_rig: 1,
            b: vertex("torso", 2),
            driven_rig: 1,
            offset: [0.0, -1.664_567_278],
        },
        ContactDef {
            id: "c_far".into(),
            a_rig: 0,
            a: vertex("arm_far_fore", 3),
            b_rig: 1,
            b: vertex("torso", 5),
            driven_rig: 1,
            offset: [0.0, 0.063_570_144],
        },
    ]
}

fn probes() -> Vec<ProbeDef> {
    vec![
        ProbeDef {
            id: "p_a_forearm".into(),
            rig: 0,
            mesh: "arm_near_fore".into(),
            vertex: 0,
            against_rig: 1,
        },
        ProbeDef {
            id: "p_a_torso".into(),
            rig: 0,
            mesh: "torso".into(),
            vertex: 2,
            against_rig: 1,
        },
        ProbeDef {
            id: "p_b_palm".into(),
            rig: 1,
            mesh: "arm_near_fore".into(),
            vertex: 3,
            against_rig: 0,
        },
        ProbeDef {
            id: "p_b_pelvis".into(),
            rig: 1,
            mesh: "pelvis".into(),
            vertex: 0,
            against_rig: 0,
        },
    ]
}

fn a_placement(time: f64) -> Placement {
    const KEYS: [(f64, [f64; 2], f64); 9] = [
        (0.0, [0.0, 0.0], 0.0),
        (3.0, [0.0, 0.0], 0.0),
        (3.9, [0.5, -2.2], 0.0),
        (5.0, [0.5, 0.0], 0.0),
        (6.5, [0.5, 0.0], 26.0),
        (8.0, [0.5, 0.0], 26.0),
        (9.6, [0.5, -1.2], 8.0),
        (10.4, [0.5, 0.0], 0.0),
        (12.0, [0.0, 0.0], 0.0),
    ];
    for pair in KEYS.windows(2) {
        let (t0, p0, r0) = pair[0];
        let (t1, p1, r1) = pair[1];
        if time <= t1 {
            let f = if t1 > t0 {
                (time - t0) / (t1 - t0)
            } else {
                0.0
            };
            return Placement {
                translation: [p0[0] + f * (p1[0] - p0[0]), p0[1] + f * (p1[1] - p0[1])],
                rotation_degrees: r0 + f * (r1 - r0),
            };
        }
    }
    Placement::IDENTITY
}

/// B's authored placement: the solver's starting point each
/// frame. Static while the contacts are live (the lift is
/// purely solver-driven), then B steps back after set-down.
fn b_placement(time: f64) -> Placement {
    let x = if time < 10.0 {
        B_X
    } else if time < 11.0 {
        B_X + 7.0 * (time - 10.0)
    } else {
        B_X + 7.0
    };
    Placement {
        translation: [x, 0.0],
        rotation_degrees: 0.0,
    }
}

fn frame_at(ma: &Motion, mb: &Motion, step: u64) -> FrameInput {
    let time = step as f64 / 60.0;
    FrameInput {
        step,
        time_seconds: time,
        driven: vec![driven_at(ma, time), driven_at(mb, time)],
        placements: vec![a_placement(time), b_placement(time)],
        active: vec![(2.0..10.0).contains(&time), (2.0..10.0).contains(&time)],
    }
}

// --- rendering ---------------------------------------------------

const CANVAS_W: u32 = 960;
const CANVAS_H: u32 = 720;
// The window must contain the whole turn: at 26 degrees
// the carried figure swings up-left of the lifter's heel
// pivot (world x to about -60, y to about 200).
const WORLD_MIN: [f64; 2] = [-100.0, -12.0];
const WORLD_MAX: [f64; 2] = [130.0, 205.0];

fn to_px(p: [f64; 2]) -> (f64, f64) {
    let sx = f64::from(CANVAS_W) / (WORLD_MAX[0] - WORLD_MIN[0]);
    let sy = f64::from(CANVAS_H) / (WORLD_MAX[1] - WORLD_MIN[1]);
    let s = sx.min(sy);
    (
        (p[0] - WORLD_MIN[0]) * s,
        f64::from(CANVAS_H) - (p[1] - WORLD_MIN[1]) * s,
    )
}

fn fill_triangle(img: &mut RgbImage, a: [f64; 2], b: [f64; 2], c: [f64; 2], color: Rgb<u8>) {
    let (ax, ay) = to_px(a);
    let (bx, by) = to_px(b);
    let (cx, cy) = to_px(c);
    let min_x = ax.min(bx).min(cx).floor().max(0.0) as u32;
    let max_x = ax.max(bx).max(cx).ceil().min(f64::from(CANVAS_W) - 1.0) as u32;
    let min_y = ay.min(by).min(cy).floor().max(0.0) as u32;
    let max_y = ay.max(by).max(cy).ceil().min(f64::from(CANVAS_H) - 1.0) as u32;
    let denom = (by - cy) * (ax - cx) + (cx - bx) * (ay - cy);
    if denom == 0.0 {
        return;
    }
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let px = f64::from(x) + 0.5;
            let py = f64::from(y) + 0.5;
            let w0 = ((by - cy) * (px - cx) + (cx - bx) * (py - cy)) / denom;
            let w1 = ((cy - ay) * (px - cx) + (ax - cx) * (py - cy)) / denom;
            let w2 = 1.0 - w0 - w1;
            if w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0 {
                img.put_pixel(x, y, color);
            }
        }
    }
}

fn disc(img: &mut RgbImage, centre: [f64; 2], radius: f64, color: Rgb<u8>) {
    let (cx, cy) = to_px(centre);
    let r = radius * f64::from(CANVAS_W) / (WORLD_MAX[0] - WORLD_MIN[0]);
    let x0 = (cx - r).floor().max(0.0) as u32;
    let x1 = (cx + r).ceil().min(f64::from(CANVAS_W) - 1.0) as u32;
    let y0 = (cy - r).floor().max(0.0) as u32;
    let y1 = (cy + r).ceil().min(f64::from(CANVAS_H) - 1.0) as u32;
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

fn part_color(rig: usize, part: &str) -> Rgb<u8> {
    match (rig, part) {
        (0, "torso") => Rgb([70, 110, 180]),
        (0, "pelvis") => Rgb([55, 85, 145]),
        (0, "head") => Rgb([225, 190, 130]),
        (0, "arm_near") => Rgb([90, 140, 210]),
        (0, "arm_far") => Rgb([45, 70, 115]),
        (0, "leg_near") => Rgb([60, 95, 160]),
        (0, "leg_far") => Rgb([40, 60, 100]),
        (1, "torso") => Rgb([200, 110, 60]),
        (1, "pelvis") => Rgb([165, 88, 48]),
        (1, "head") => Rgb([235, 200, 140]),
        (1, "arm_near") => Rgb([220, 130, 75]),
        (1, "arm_far") => Rgb([140, 75, 40]),
        (1, "leg_near") => Rgb([180, 100, 55]),
        (1, "leg_far") => Rgb([120, 65, 35]),
        _ => Rgb([128, 128, 128]),
    }
}

fn render_frame(
    out: &Path,
    label: &str,
    scene: &Scene,
    ma: &Motion,
    mb: &Motion,
    step: u64,
    read: &FrameRead,
) {
    let time = step as f64 / 60.0;
    let mut img = RgbImage::from_pixel(CANVAS_W, CANVAS_H, Rgb([24, 24, 28]));
    // Ground line at y = 0.
    let (_, gy) = to_px([0.0, 0.0]);
    for x in 0..CANVAS_W {
        let y = gy.round() as u32;
        if y < CANVAS_H {
            img.put_pixel(x, y, Rgb([110, 110, 110]));
        }
    }
    // Geometry re-evaluated at the SOLVED placements.
    let input = frame_at(ma, mb, step);
    let mut draw: Vec<(i32, usize, EvaluatedMesh)> = Vec::new();
    for (rig, model) in scene.models().iter().enumerate() {
        let params = resolve_parameters(model, &input.driven[rig]).expect("params resolve");
        let meshes = evaluate_model(model, &params);
        for mesh in meshes {
            let world = EvaluatedMesh {
                id: mesh.id.clone(),
                part: mesh.part.clone(),
                draw_order: mesh.draw_order,
                vertices: mesh
                    .vertices
                    .iter()
                    .map(|v| read.placements[rig].apply(*v))
                    .collect(),
            };
            draw.push((mesh.draw_order, rig, world));
        }
    }
    draw.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.2.id.cmp(&b.2.id)));
    for (_, rig, mesh) in &draw {
        let model_mesh = scene.models()[*rig]
            .meshes
            .iter()
            .find(|m| m.id == mesh.id)
            .expect("mesh exists");
        let color = part_color(*rig, &mesh.part);
        for tri in model_mesh.indices.as_chunks::<3>().0 {
            fill_triangle(
                &mut img,
                mesh.vertices[tri[0] as usize],
                mesh.vertices[tri[1] as usize],
                mesh.vertices[tri[2] as usize],
                color,
            );
        }
    }
    // Contact markers: red ring at the a endpoint, white
    // dot at the fixed point on B; a line between them is
    // the slip made visible.
    let input2 = frame_at(ma, mb, step);
    for (index, contact) in scene.contacts().iter().enumerate() {
        if !input2.active[index] {
            continue;
        }
        let params_a =
            resolve_parameters(&scene.models()[0], &input2.driven[0]).expect("params resolve");
        let meshes_a = evaluate_model(&scene.models()[0], &params_a);
        let a_point = endpoint_world(&meshes_a, &read.placements[0], &contact.a);
        disc(&mut img, a_point, 1.6, Rgb([255, 60, 60]));
        let params_b =
            resolve_parameters(&scene.models()[1], &input2.driven[1]).expect("params resolve");
        let meshes_b = evaluate_model(&scene.models()[1], &params_b);
        let b_local = endpoint_world(&meshes_b, &Placement::IDENTITY, &contact.b);
        let fixed = read.placements[1].apply([
            b_local[0] + contact.offset[0],
            b_local[1] + contact.offset[1],
        ]);
        disc(&mut img, fixed, 1.0, Rgb([255, 255, 255]));
    }
    // Root markers (yellow).
    for root in &read.roots {
        disc(&mut img, *root, 1.1, Rgb([255, 220, 60]));
    }
    let _ = time;
    img.save(out.join(format!("frame-{label}.png")))
        .expect("frame saves");
}

// --- measurements --------------------------------------------------

fn percentile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let index = ((sorted.len() - 1) as f64 * q).round() as usize;
    sorted[index]
}

fn main() {
    let out = PathBuf::from(std::env::args().nth(1).expect("usage: twobody_scene <dir>"));
    std::fs::create_dir_all(&out).expect("output dir");
    let a = parse_model(&fixture("mannequin_a.model.json")).expect("model A");
    let b = parse_model(&fixture("mannequin_b.model.json")).expect("model B");
    let ma = parse_motion(&fixture("lift_a.motion.json"), &a).expect("motion A");
    let mb = parse_motion(&fixture("lift_b.motion.json"), &b).expect("motion B");
    let scene = Scene::new(
        vec![a, b],
        grasp_contacts(),
        probes(),
        vec![centroid("pelvis"), centroid("pelvis")],
    )
    .expect("scene constructs");

    let reads: Vec<FrameRead> = (0..720)
        .map(|step| evaluate_frame(&scene, &frame_at(&ma, &mb, step)).expect("frame evaluates"))
        .collect();
    std::fs::write(
        out.join("reads.json"),
        serde_json::to_string(&reads).expect("reads serialize"),
    )
    .expect("reads write");

    // Per-phase slip aggregates (C1).
    let phase = |lo: u64, hi: u64| -> (f64, f64) {
        let mut slips: Vec<f64> = reads[(lo as usize)..(hi as usize)]
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
    // Post-release drift: how far the (now inactive)
    // contact pairs sit apart once the grip lets go.
    let release_max = reads[600..720]
        .iter()
        .flat_map(|r| r.contacts.iter().map(|c| c.slip))
        .fold(0.0f64, f64::max);
    let engaged_max = reads
        .iter()
        .flat_map(|r| r.contacts.iter().filter(|c| c.engaged).map(|c| c.slip))
        .fold(0.0f64, f64::max);

    // C2: worst margin over the whole scene, and over the
    // engaged window alone.
    let (min_margin, min_margin_step) = reads
        .iter()
        .enumerate()
        .filter_map(|(i, r)| r.min_margin.map(|m| (m, i)))
        .fold(
            (f64::INFINITY, 0usize),
            |acc, x| if x.0 < acc.0 { x } else { acc },
        );
    let (engaged_margin, engaged_margin_step) = reads[120..600]
        .iter()
        .enumerate()
        .filter_map(|(i, r)| r.min_margin.map(|m| (m, i + 120)))
        .fold(
            (f64::INFINITY, 0usize),
            |acc, x| if x.0 < acc.0 { x } else { acc },
        );
    let engaged_margin_probe = reads[engaged_margin_step]
        .min_margin_probe
        .clone()
        .unwrap_or_default();

    // C4: A root dip + recovery; B airborne window + clearance.
    let root_a: Vec<f64> = reads.iter().map(|r| r.roots[0][1]).collect();
    let root_b: Vec<f64> = reads.iter().map(|r| r.roots[1][1]).collect();
    let dip = root_a[228..300]
        .iter()
        .fold(f64::INFINITY, |x, &y| x.min(y));
    let dip_pct = (97.0 - dip) / FIGURE_HEIGHT * 100.0;
    let recovered = root_a[390];
    // B foot clearance: re-evaluate foot vertices at solved
    // placements for the hold window.
    let mut clearance_min = f64::INFINITY;
    for step in 300..480 {
        let input = frame_at(&ma, &mb, step);
        let params =
            resolve_parameters(&scene.models()[1], &input.driven[1]).expect("params resolve");
        let meshes = evaluate_model(&scene.models()[1], &params);
        for mesh in &meshes {
            if mesh.id.contains("foot") {
                for v in &mesh.vertices {
                    let world = reads[step as usize].placements[1].apply(*v);
                    clearance_min = clearance_min.min(world[1]);
                }
            }
        }
    }

    // Foot-vs-ground across the WHOLE scene (the ground
    // is not a rig, so this is a read, not a constraint):
    // the lowest foot vertex of either figure, any step.
    let mut foot_min = f64::INFINITY;
    let mut foot_min_step = 0usize;
    for (step, read) in reads.iter().enumerate() {
        let input = frame_at(&ma, &mb, step as u64);
        for rig in 0..2 {
            let params = resolve_parameters(&scene.models()[rig], &input.driven[rig])
                .expect("params resolve");
            let meshes = evaluate_model(&scene.models()[rig], &params);
            for mesh in &meshes {
                if mesh.id.contains("foot") {
                    for v in &mesh.vertices {
                        let world = read.placements[rig].apply(*v);
                        if world[1] < foot_min {
                            foot_min = world[1];
                            foot_min_step = step;
                        }
                    }
                }
            }
        }
    }

    // C5: gaze error — each figure's facing (head centroid
    // -> nose vertex) vs the direction to the partner's
    // head centroid, sampled every 6 steps while engaged.
    let gaze_at = |step: usize, who: usize| -> f64 {
        let input = frame_at(&ma, &mb, step as u64);
        let mut heads = [[0.0; 2]; 2];
        let mut noses = [[0.0; 2]; 2];
        for rig in 0..2 {
            let params = resolve_parameters(&scene.models()[rig], &input.driven[rig])
                .expect("params resolve");
            let meshes = evaluate_model(&scene.models()[rig], &params);
            heads[rig] = endpoint_world(&meshes, &reads[step].placements[rig], &centroid("head"));
            noses[rig] = endpoint_world(&meshes, &reads[step].placements[rig], &vertex("head", 0));
        }
        let other = 1 - who;
        let facing = [noses[who][0] - heads[who][0], noses[who][1] - heads[who][1]];
        let toward = [
            heads[other][0] - heads[who][0],
            heads[other][1] - heads[who][1],
        ];
        let cross = facing[0] * toward[1] - facing[1] * toward[0];
        let dot = facing[0] * toward[0] + facing[1] * toward[1];
        cross.atan2(dot).to_degrees().abs()
    };
    let (mut gaze_a_max, mut gaze_a_step) = (0.0f64, 0usize);
    let (mut gaze_b_max, mut gaze_b_step) = (0.0f64, 0usize);
    for step in (120..600).step_by(6) {
        let ga = gaze_at(step, 0);
        if ga > gaze_a_max {
            gaze_a_max = ga;
            gaze_a_step = step;
        }
        let gb = gaze_at(step, 1);
        if gb > gaze_b_max {
            gaze_b_max = gb;
            gaze_b_step = step;
        }
    }

    // Worst frame (C1/C2 indictment): largest engaged slip,
    // else smallest margin.
    let (worst_step, worst_kind) = {
        let by_slip = reads
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
        if by_slip.0 > 0.05 {
            (by_slip.1, "largest engaged slip")
        } else {
            (min_margin_step, "smallest margin")
        }
    };

    let summary = serde_json::json!({
        "c1_slip_units": {
            "engage_2_4s": {"p95": engage_p95, "max": engage_max},
            "lift_4_8s": {"p95": lift_p95, "max": lift_max},
            "hold_8_10s": {"p95": hold_p95, "max": hold_max},
            "post_release_drift_max": release_max,
            "engaged_max_all": engaged_max,
            "figure_height_units": FIGURE_HEIGHT,
            "engaged_max_pct_height": engaged_max / FIGURE_HEIGHT * 100.0
        },
        "c2_min_margin": {"value": min_margin, "step": min_margin_step,
                          "time_seconds": min_margin_step as f64 / 60.0},
        "c2_engaged_min_margin": {"value": engaged_margin,
                          "step": engaged_margin_step,
                          "probe": engaged_margin_probe,
                          "time_seconds": engaged_margin_step as f64 / 60.0},
        "c3_solver_lag_steps": 0,
        "c4_weight_transfer": {
            "a_root_rest_y": 97.0,
            "a_root_dip_y": dip,
            "a_root_dip_pct_height": dip_pct,
            "a_root_y_at_turn_step_390": recovered,
            "b_root_rest_y": 92.15,
            "b_root_y_at_turn_step_390": root_b[390],
            "b_foot_clearance_min_hold_units": clearance_min,
            "foot_min_y_all_steps": foot_min,
            "foot_min_y_step": foot_min_step
        },
        "c5_gaze_error_max_degrees_engaged_sampled": {
            "a": gaze_a_max, "a_step": gaze_a_step,
            "b": gaze_b_max, "b_step": gaze_b_step},
        "worst_frame": {"step": worst_step, "kind": worst_kind,
                        "time_seconds": worst_step as f64 / 60.0},
        "converged_frames": reads.iter().filter(|r| r.converged).count(),
        "total_frames": reads.len()
    });
    std::fs::write(
        out.join("summary.json"),
        serde_json::to_string_pretty(&summary).expect("summary serializes"),
    )
    .expect("summary writes");
    println!("{}", serde_json::to_string_pretty(&summary).unwrap());

    for (label, step) in [
        ("approach-t1.0", 60u64),
        ("grasp-t2.0", 120),
        ("lift-t4.0", 240),
        ("turn-t6.5", 390),
        ("setdown-t9.5", 570),
        ("release-t11.0", 660),
    ] {
        render_frame(&out, label, &scene, &ma, &mb, step, &reads[step as usize]);
    }
    render_frame(
        &out,
        "worst",
        &scene,
        &ma,
        &mb,
        worst_step as u64,
        &reads[worst_step],
    );
    render_frame(
        &out,
        "margin-worst",
        &scene,
        &ma,
        &mb,
        min_margin_step as u64,
        &reads[min_margin_step],
    );
}
