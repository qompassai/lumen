//! Physics evaluation tests (SPEC.md section 5): the
//! Verlet strand model, its output writes, and step
//! determinism. The integrator under test is section 5's
//! own — not the INP reference's RK4 pendulum, whose
//! trajectories the format deliberately does not reproduce
//! (see the physics module docs for the named difference).

use lumen_rig_core::{ParameterValues, PhysicsSim, Rig, parse_model};

/// One strand: a single free particle on a 1-unit segment,
/// anchored to the `root_anchor` rotation deformer at the
/// origin, writing `sway` = 2 * tip angle + 10.
const PENDULUM_DOC: &str = r#"{
  "format": "lumen-rig-model", "format_version": 1, "id": "pendulum",
  "parameters": [
    { "id": "sway", "kind": "scalar", "min": -1000.0, "max": 1000.0, "default": 0.0 },
    { "id": "tilt", "kind": "scalar", "min": 0.0, "max": 1.0, "default": 0.0 }
  ],
  "parts": [{ "id": "root" }],
  "textures": [{ "id": "tex0", "source": "tex0.png" }],
  "deformers": [
    { "id": "root_anchor", "kind": "rotation", "origin": [0.0, 0.0],
      "base_angle_degrees": 0.0 }
  ],
  "meshes": [
    {
      "id": "quad", "part": "root", "texture": "tex0", "deformer": "root_anchor",
      "vertices": [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
      "uvs": [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
      "indices": [0, 1, 2, 0, 2, 3]
    }
  ],
  "physics": [
    {
      "id": "strand_group",
      "gravity": [0.0, -9.8],
      "inputs": [{ "parameter": "tilt", "scale_degrees": 90.0 }],
      "strands": [
        {
          "id": "strand",
          "anchor": { "kind": "deformer", "id": "root_anchor", "offset": [0.0, 0.0] },
          "particles": [
            { "length": 1.0, "radius": 0.0, "mobility": 0.0, "delay": 0.0, "damping": 0.0 }
          ],
          "output": { "parameter": "sway", "scale": 2.0, "offset": 10.0 }
        }
      ]
    }
  ]
}"#;

fn pendulum() -> lumen_rig_core::Model {
    parse_model(PENDULUM_DOC.as_bytes()).expect("pendulum document parses")
}

#[test]
fn pendulum_falls_and_writes_its_output() {
    // Rest construction lays the particle along +x from the
    // anchor (section 5.1): the tip starts at (1, 0) and
    // the output reads 10 + 2 * 0 degrees = 10. Under
    // gravity the tip falls: its y drops, the segment keeps
    // its length, and the output angle goes negative.
    let model = pendulum();
    let mut rig = Rig::new(model, 0);
    let positions = rig
        .physics()
        .strand_positions(rig.model(), 0, 0)
        .expect("strand state")
        .to_vec();
    assert_eq!(positions, vec![[1.0, 0.0]]);
    // Quarter period (0.5 s at 120 steps/s): the tip is at
    // the bottom of its swing.
    for _ in 0..60 {
        rig.step(1.0 / 120.0);
    }
    let positions = rig
        .physics()
        .strand_positions(rig.model(), 0, 0)
        .expect("strand state")
        .to_vec();
    let tip = positions[0];
    assert!(
        tip[1] < -0.9,
        "tip fell to the bottom of its swing: {tip:?}"
    );
    // Half period: swung through to the far side, length
    // still exact, output angle now far negative.
    for _ in 0..60 {
        rig.step(1.0 / 120.0);
    }
    let positions = rig
        .physics()
        .strand_positions(rig.model(), 0, 0)
        .expect("strand state")
        .to_vec();
    let tip = positions[0];
    let length = (tip[0] * tip[0] + tip[1] * tip[1]).sqrt();
    assert!(
        (length - 1.0).abs() <= 1e-6,
        "segment length held by the constraint solve: {length}"
    );
    let sway = rig.values().get("sway").expect("sway written");
    assert!(sway < 10.0, "output angle went negative: sway = {sway}");
}

#[test]
fn physics_steps_are_deterministic() {
    // Two fresh rigs, the same drive (the anchor's group
    // input parameter swept mid-run), the same step count:
    // strand states and written outputs agree bit for bit.
    let run = || {
        let model = pendulum();
        let mut rig = Rig::new(model, 42);
        for step in 0..240 {
            if step == 60 {
                rig.set_parameter("tilt", 0.5);
            }
            rig.step(1.0 / 120.0);
        }
        let positions = rig
            .physics()
            .strand_positions(rig.model(), 0, 0)
            .expect("strand state")
            .to_vec();
        (positions, rig.values().get("sway"))
    };
    assert_eq!(run(), run());
}

#[test]
fn turbulence_is_seeded_and_deterministic() {
    // With turbulence on, the run seed is a simulation
    // input: same seed, same trajectory; another seed, a
    // different one.
    let document = PENDULUM_DOC.replace(
        "\"gravity\": [0.0, -9.8],",
        "\"gravity\": [0.0, -9.8], \"turbulence_amplitude\": 3.0,",
    );
    let run = |seed: u64| {
        let model = parse_model(document.as_bytes()).expect("turbulent document parses");
        let values = ParameterValues::rest(&model);
        let mut sim = PhysicsSim::new(&model, &values, seed);
        let mut values = values;
        let anchors = vec![[0.0, 0.0]];
        for _ in 0..60 {
            sim.step(&model, &mut values, &anchors, 1.0 / 120.0);
        }
        sim.strand_positions(&model, 0, 0).expect("strand").to_vec()
    };
    assert_eq!(run(7), run(7));
    assert_ne!(run(7), run(8));
}

#[test]
fn prima_physics_outputs_move_under_a_swinging_anchor() {
    // The landed Prima rig: step the simulation while the
    // head anchor swings (ParamAngleZ drives rot_HeadAnchor,
    // the hair strands' anchor). The hair-sway outputs must
    // leave their defaults, and a replay must reproduce the
    // run exactly.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("fixtures")
        .join("conformance")
        .join("prima-repertoire.model.json");
    let bytes = std::fs::read(&path).expect("Prima fixture readable");
    let run = || {
        let model = parse_model(&bytes).expect("Prima fixture parses");
        let mut rig = Rig::new(model, 0);
        for step in 0..240 {
            let angle = ((step as f32) / 240.0 * std::f32::consts::TAU).sin() * 20.0;
            rig.set_parameter("ParamAngleZ_2003_x", angle);
            rig.step(1.0 / 120.0);
        }
        [
            rig.values().get("ParamHairSway1_9011_x"),
            rig.values().get("ParamHairSway2_9012_x"),
            rig.values().get("ParamHairSway3_9013_x"),
        ]
    };
    let first = run();
    let second = run();
    assert_eq!(first, second, "physics replay is deterministic");
    assert!(
        first.iter().any(|value| value.is_some_and(|v| v != 0.0)),
        "at least one hair-sway output left its default: {first:?}"
    );
}
