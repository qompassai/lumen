//! Two-body contact model: validation tests (the
//! adversarial half lives in `twobody_adversarial.rs`).

use std::collections::BTreeMap;
use std::path::PathBuf;

use lumen_rig_core::twobody::{
    ContactDef, ContactEndpoint, FrameInput, Placement, ProbeDef, Scene, endpoint_world,
    evaluate_frame, evaluate_model, resolve_parameters,
};
use lumen_rig_core::{Model, Motion, curve_value_at, parse_model, parse_motion};

fn fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../fixtures/twobody")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn mannequins() -> (Model, Model, Motion, Motion) {
    let a = parse_model(&fixture("mannequin_a.model.json")).expect("model A parses");
    let b = parse_model(&fixture("mannequin_b.model.json")).expect("model B parses");
    let ma = parse_motion(&fixture("lift_a.motion.json"), &a).expect("motion A parses");
    let mb = parse_motion(&fixture("lift_b.motion.json"), &b).expect("motion B parses");
    (a, b, ma, mb)
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

const B_X: f64 = 54.778_421_414;

fn grasp_contacts() -> Vec<ContactDef> {
    vec![
        ContactDef {
            id: "c_near".into(),
            a_rig: 0,
            a: ContactEndpoint::Vertex {
                mesh: "arm_near_fore".into(),
                vertex: 3,
            },
            b_rig: 1,
            b: ContactEndpoint::Vertex {
                mesh: "torso".into(),
                vertex: 2,
            },
            driven_rig: 1,
            offset: [0.0, -1.664_567_278],
        },
        ContactDef {
            id: "c_far".into(),
            a_rig: 0,
            a: ContactEndpoint::Vertex {
                mesh: "arm_far_fore".into(),
                vertex: 3,
            },
            b_rig: 1,
            b: ContactEndpoint::Vertex {
                mesh: "torso".into(),
                vertex: 5,
            },
            driven_rig: 1,
            offset: [0.0, 0.063_570_144],
        },
    ]
}

fn probes() -> Vec<ProbeDef> {
    vec![
        // Probes target NON-contact points: a contact
        // endpoint sits at (or, by its grip offset, just
        // inside) the grasped surface by design, so probing
        // it would measure the authored grip, not a
        // violation. The forearm base stands in for the arm.
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
        // B's pelvis corner: a body-core point. (B's torso
        // vertex 2 is contact c_near's endpoint - probing it
        // would measure the authored grasp, not a violation.)
        ProbeDef {
            id: "p_b_pelvis".into(),
            rig: 1,
            mesh: "pelvis".into(),
            vertex: 0,
            against_rig: 0,
        },
    ]
}

fn roots() -> Vec<ContactEndpoint> {
    vec![
        ContactEndpoint::PartCentroid {
            part: "pelvis".into(),
        },
        ContactEndpoint::PartCentroid {
            part: "pelvis".into(),
        },
    ]
}

fn lift_scene() -> (Scene, Motion, Motion) {
    let (a, b, ma, mb) = mannequins();
    let scene =
        Scene::new(vec![a, b], grasp_contacts(), probes(), roots()).expect("scene constructs");
    (scene, ma, mb)
}

/// A's authored placement track (piecewise linear): crouch,
/// lift, turn to 26 degrees, set down, release.
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
    Placement {
        translation: [0.0, 0.0],
        rotation_degrees: 0.0,
    }
}

/// B's authored placement: static while the contacts are
/// live (the lift is purely solver-driven), then B steps
/// back after set-down.
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
        active: grasp_contacts()
            .iter()
            .map(|_| (2.0..10.0).contains(&time))
            .collect(),
    }
}

#[test]
fn fixtures_parse_and_bind() {
    let (a, b, ma, mb) = mannequins();
    assert_eq!(a.id, "mannequin_a");
    assert_eq!(b.id, "mannequin_b");
    assert_eq!(ma.model, "mannequin_a");
    assert_eq!(mb.model, "mannequin_b");
    assert_eq!(a.meshes.len(), 14);
    assert_eq!(b.meshes.len(), 14);
}

#[test]
fn rotation_deformer_matches_hand_computed() {
    // One rotation deformer, identity lattice over
    // [-90, 90]; vertex (20, 10) about origin (10, 10).
    let doc = serde_json::json!({
        "format": "lumen-rig-model", "format_version": 1, "id": "tiny",
        "parameters": [
            {"kind": "scalar", "id": "tilt", "min": -90.0, "max": 90.0, "default": 0.0}
        ],
        "parts": [{"id": "root"}],
        "textures": [{"id": "tex", "source": "t.png"}],
        "deformers": [{
            "kind": "rotation", "id": "tilt_d", "origin": [10.0, 10.0],
            "base_angle_degrees": 0.0,
            "keyforms": {"axes": ["tilt"], "keys": [[-90.0, 90.0]], "forms": [
                {"at": [0], "angle_degrees": -90.0, "scale": [1.0, 1.0]},
                {"at": [1], "angle_degrees": 90.0, "scale": [1.0, 1.0]}
            ]}
        }],
        "meshes": [{
            "id": "quad", "part": "root", "texture": "tex", "deformer": "tilt_d",
            "vertices": [[20.0, 10.0], [20.0, 12.0], [18.0, 12.0]],
            "uvs": [[0.0, 0.0], [0.0, 0.0], [0.0, 0.0]],
            "indices": [0, 1, 2]
        }],
        "physics": []
    });
    let model = parse_model(doc.to_string().as_bytes()).expect("tiny model parses");
    for (value, expected) in [
        (90.0, [10.0, 20.0]),
        (-90.0, [10.0, 0.0]),
        (0.0, [20.0, 10.0]),
    ] {
        let driven = BTreeMap::from([("tilt".to_string(), value)]);
        let params = resolve_parameters(&model, &driven).expect("params resolve");
        let meshes = evaluate_model(&model, &params);
        let got = meshes[0].vertices[0];
        assert!(
            (got[0] - expected[0]).abs() < 1e-12 && (got[1] - expected[1]).abs() < 1e-12,
            "tilt {value}: got {got:?}, want {expected:?}"
        );
    }
}

#[test]
fn warp_cell_centre_maps_to_bilinear_centre() {
    let doc = serde_json::json!({
        "format": "lumen-rig-model", "format_version": 1, "id": "warpy",
        "parameters": [
            {"kind": "scalar", "id": "bend", "min": 0.0, "max": 10.0, "default": 0.0}
        ],
        "parts": [{"id": "root"}],
        "textures": [{"id": "tex", "source": "t.png"}],
        "deformers": [{
            "kind": "warp", "id": "bend_d", "columns": 1, "rows": 1,
            "rest_positions": [[0.0, 0.0], [2.0, 0.0], [0.0, 2.0], [2.0, 2.0]],
            "keyforms": {"axes": ["bend"], "keys": [[0.0, 10.0]], "forms": [
                {"at": [0], "positions": [[0.0, 0.0], [2.0, 0.0], [0.0, 2.0], [2.0, 2.0]]},
                {"at": [1], "positions": [[0.0, 0.0], [2.0, 0.0], [0.0, 2.0], [6.0, 4.0]]}
            ]}
        }],
        "meshes": [{
            "id": "quad", "part": "root", "texture": "tex", "deformer": "bend_d",
            "vertices": [[1.0, 1.0], [0.5, 0.5], [1.5, 1.5]],
            "uvs": [[0.0, 0.0], [0.0, 0.0], [0.0, 0.0]],
            "indices": [0, 1, 2]
        }],
        "physics": []
    });
    let model = parse_model(doc.to_string().as_bytes()).expect("warp model parses");
    let driven = BTreeMap::from([("bend".to_string(), 10.0)]);
    let params = resolve_parameters(&model, &driven).expect("params resolve");
    let meshes = evaluate_model(&model, &params);
    // Centre of the single cell -> mean of the four deformed
    // control points: ((0+2+0+6)/4, (0+0+2+4)/4) = (2, 1.5).
    let got = meshes[0].vertices[0];
    assert!(
        (got[0] - 2.0).abs() < 1e-9 && (got[1] - 1.5).abs() < 1e-9,
        "warp centre: got {got:?}, want [2.0, 1.5]"
    );
}

#[test]
fn mannequin_palm_matches_generator_fk() {
    // The fixture generator (build_fixtures.py) solved the
    // grasp at shoulder 40.5 / elbow 17.5 by forward
    // kinematics; the Rust evaluator must reproduce its palm.
    let (a, _b, _ma, _mb) = mannequins();
    let driven = BTreeMap::from([
        ("shoulder_swing".to_string(), 40.5),
        ("elbow_bend".to_string(), 17.5),
    ]);
    let params = resolve_parameters(&a, &driven).expect("params resolve");
    let meshes = evaluate_model(&a, &params);
    let palm = endpoint_world(
        &meshes,
        &Placement::IDENTITY,
        &ContactEndpoint::Vertex {
            mesh: "arm_near_fore".into(),
            vertex: 3,
        },
    );
    // Generator FK: near palm lands at (B_X - 6.65, 110.2 -
    // 1.665) = (48.128, 108.535).
    assert!(
        (palm[0] - 48.128421).abs() < 0.01 && (palm[1] - 108.535).abs() < 0.01,
        "palm {palm:?} vs generator FK (48.128, 108.535)"
    );
}

#[test]
fn grasp_solve_from_perturbed_start() {
    let (scene, ma, mb) = lift_scene();
    let mut input = frame_at(&ma, &mb, 120);
    // Perturb B's authored placement: the solver must recover
    // the grasp from a wrong start, in-step.
    input.placements[1] = Placement {
        translation: [B_X + 3.0, -2.0],
        rotation_degrees: 5.0,
    };
    let read = evaluate_frame(&scene, &input).expect("frame evaluates");
    assert!(read.converged, "grasp should converge: {read:?}");
    for contact in &read.contacts {
        assert!(
            contact.slip <= 1e-6,
            "contact {} slip {}",
            contact.id,
            contact.slip
        );
        assert!(contact.pre_solve_slip > 1.0, "perturbation was real");
    }
    let moved = (read.placements[1].translation[0] - (B_X + 3.0))
        .hypot(read.placements[1].translation[1] - (-2.0));
    assert!(moved > 1.0, "solver moved B by {moved}");
}

#[test]
fn full_scene_determinism_and_reads() {
    let (scene, ma, mb) = lift_scene();
    let run = |scene: &Scene| -> Vec<lumen_rig_core::twobody::FrameRead> {
        (0..720)
            .map(|step| evaluate_frame(scene, &frame_at(&ma, &mb, step)).expect("frame evaluates"))
            .collect()
    };
    let first = run(&scene);
    let second = run(&scene);
    let first_json = serde_json::to_string(&first).expect("reads serialize");
    let second_json = serde_json::to_string(&second).expect("reads serialize");
    assert_eq!(
        first_json, second_json,
        "double-bake must be byte-identical"
    );
    assert_eq!(first_json.len(), second_json.len());

    let root_a = |step: usize| first[step].roots[0][1];
    let root_b = |step: usize| first[step].roots[1][1];
    // Weight transfer: A's root dips during the crouch window
    // (3.8-5.0 s) and recovers by the hold.
    let dip = (228..300).map(root_a).fold(f64::INFINITY, f64::min);
    assert!(dip < 96.0, "A root dip {dip} (rest 97)");
    assert!(
        (root_a(300) - 97.0).abs() < 0.01,
        "A root recovered to rest by hold: {}",
        root_a(300)
    );
    // B is airborne through the hold: root well above rest.
    assert!(
        root_b(390) > 92.15 + 12.0,
        "B root at turn {} (rest 92.15)",
        root_b(390)
    );
    // Hold slip stays at solver precision through the turn
    // (steps 300..480): the two-contact grasp on a rigid body
    // is exactly solvable under placement rotation.
    let hold_slip = (300..480)
        .flat_map(|step| first[step].contacts.iter().map(|c| c.slip))
        .fold(0.0f64, f64::max);
    assert!(hold_slip <= 1e-4, "hold slip {hold_slip}");
}
