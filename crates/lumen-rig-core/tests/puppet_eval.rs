//! Puppet evaluation stage: behavior tests. Covers the
//! (0,0) reference semantics, opacity snapshot behavior,
//! deform composition, physics state evolution, masking, and
//! determinism.

use lumen_rig_core::puppet::{PuppetStage, parse_puppet};
use lumen_rig_core::{PuppetEval, RgbaImage, parse_model};
use serde_json::{Value, json};

const W: usize = 200;
const H: usize = 200;
const SCALE: f32 = 1.0;

fn model_json() -> Value {
    json!({
        "format": "lumen-rig-model",
        "format_version": 1,
        "id": "evaltiny",
        "parameters": [
            {"kind": "scalar", "id": "pan_x", "min": -1.0, "max": 1.0, "default": 0.0},
            {"kind": "scalar", "id": "pan_y", "min": -1.0, "max": 1.0, "default": 0.0},
            {"kind": "scalar", "id": "fade_x", "min": 0.0, "max": 1.0, "default": 0.0},
            {"kind": "scalar", "id": "phys_x", "min": -1.0, "max": 1.0, "default": 0.0}
        ],
        "parts": [
            {"id": "root"},
            {"id": "quad", "parent": "root"},
            {"id": "veil", "parent": "root"},
            {"id": "anchor", "parent": "root"},
            {"id": "pend", "parent": "anchor"}
        ],
        "textures": [],
        "deformers": [],
        "meshes": [],
        "physics": []
    })
}

/// A 40x40 quad centered on the node origin, solid texture.
fn quad_mesh(id: &str) -> Value {
    json!({
        "id": id, "texture": "tex_a", "origin": [0.0, 0.0],
        "vertices": [[-20.0, -20.0], [20.0, -20.0], [20.0, 20.0], [-20.0, 20.0]],
        "uvs": [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
        "indices": [0, 1, 2, 0, 2, 3]
    })
}

fn transform(x: f32, y: f32) -> Value {
    json!({
        "translation": [x, y, 0.0],
        "rotation_degrees": [0.0, 0.0, 0.0],
        "scale": [1.0, 1.0]
    })
}

fn stage_json() -> Value {
    json!({
        "format": "lumen-rig-puppet",
        "format_version": 1,
        "model": "evaltiny",
        "parameters": [
            {
                "id": "Pan", "x": "pan_x", "y": "pan_y",
                "min": [-1.0, -1.0], "max": [1.0, 1.0], "default": [0.0, 0.0],
                "axis_points": [[-1.0, 0.0, 1.0], [-1.0, 0.0, 1.0]]
            },
            {
                "id": "Fade", "x": "fade_x",
                "min": [0.0, 0.0], "max": [1.0, 1.0], "default": [0.0, 0.0],
                "axis_points": [[0.0, 1.0], []]
            },
            {
                "id": "Phys", "x": "phys_x",
                "min": [-1.0, 0.0], "max": [1.0, 1.0], "default": [0.0, 0.0],
                "axis_points": [[-1.0, 0.0, 1.0], []]
            }
        ],
        "textures": [{"id": "tex_a", "source": "textures/tex_a.png"}],
        "nodes": [
            {"id": "root", "kind": "group", "transform": transform(0.0, 0.0)},
            {
                "id": "quad", "parent": "root", "kind": "part",
                "transform": transform(0.0, 0.0),
                "mesh": quad_mesh("quad_mesh"),
                "drawable": {"blend_mode": "normal", "opacity": 1.0}
            },
            {
                "id": "veil", "parent": "root", "kind": "part",
                "transform": transform(30.0, 20.0),
                "mesh": quad_mesh("veil_mesh"),
                "drawable": {"blend_mode": "normal", "opacity": 1.0},
                "masks": [{"source": "quad", "mode": "mask"}],
                "mask_threshold": 0.5
            },
            {"id": "anchor", "parent": "root", "kind": "group", "transform": transform(0.0, -50.0)},
            {
                "id": "pend", "parent": "anchor", "kind": "physics",
                "transform": transform(0.0, 0.0),
                "physics": {
                    "parameter": "Phys", "model": "spring", "map_mode": "xy",
                    "gravity": 1.0, "length": 100.0, "frequency": 1.0,
                    "angle_damping": 0.5, "length_damping": 0.5,
                    "output_scale": [1.0, 1.0]
                }
            }
        ],
        "bindings": [
            {
                // Non-identity AT the (0,0) grid point: x
                // translation +30 when Pan is at its center.
                "parameter": "Pan", "node": "quad", "kind": "translation_x",
                "interpolate": "linear",
                "values": [[0.0, 0.0, 0.0], [0.0, 30.0, 0.0], [0.0, 60.0, 0.0]]
            },
            {
                "parameter": "Fade", "node": "quad", "kind": "opacity",
                "interpolate": "linear",
                "values": [[1.0], [0.5]]
            },
            {
                "parameter": "Phys", "node": "quad", "kind": "translation_x",
                "interpolate": "linear",
                "values": [[-40.0], [0.0], [40.0]]
            }
        ],
        "physics": {"pixels_per_meter": 1000.0, "gravity": 9.8}
    })
}

fn build(stage: &Value) -> (PuppetStage, Vec<RgbaImage>) {
    let model = parse_model(&serde_json::to_vec(&model_json()).unwrap()).unwrap();
    let stage = parse_puppet(&serde_json::to_vec(stage).unwrap(), &model).unwrap();
    // 4x4 texture: left half opaque red, right half clear.
    let mut rgba = Vec::new();
    for _y in 0..4 {
        for x in 0..4 {
            if x < 2 {
                rgba.extend_from_slice(&[255, 0, 0, 255]);
            } else {
                rgba.extend_from_slice(&[0, 0, 0, 0]);
            }
        }
    }
    (
        stage,
        vec![RgbaImage {
            width: 4,
            height: 4,
            rgba,
        }],
    )
}

fn pixel(frame: &[u8], x: usize, y: usize) -> [u8; 4] {
    let i = (y * W + x) * 4;
    [frame[i], frame[i + 1], frame[i + 2], frame[i + 3]]
}

#[test]
fn rest_frame_renders_the_quad() {
    let (stage, textures) = build(&stage_json());
    let mut eval = PuppetEval::new(&stage, &textures).unwrap();
    eval.begin_frame();
    eval.end_frame(0.0).unwrap();
    let frame = eval.render(W, H, SCALE);
    // Quad is offset +30 by the (0,0) binding (see next test);
    // its opaque left half spans x in [110, 130).
    assert_eq!(pixel(&frame, 115, 100), [255, 0, 0, 255]);
}

#[test]
fn zero_zero_bindings_apply_reference_semantics() {
    let (stage, textures) = build(&stage_json());
    let mut eval = PuppetEval::new(&stage, &textures).unwrap();
    eval.begin_frame();
    eval.end_frame(0.0).unwrap();
    let frame = eval.render(W, H, SCALE);
    // Pan sits at (0,0); its grid center carries +30 x. With
    // reference semantics the quad's left edge is at
    // 100 + 30 - 20 = 110 and its opaque half spans
    // x in [110, 130): pixel 105 is clear, 115 is red.
    assert_eq!(pixel(&frame, 105, 100), [0, 0, 0, 0]);
    assert_eq!(pixel(&frame, 115, 100), [255, 0, 0, 255]);

    // The proof-only skip reproduces the shipped runtime:
    // no offset at (0,0), quad spans [80, 100) in its opaque
    // half; 95 red, 115 clear.
    let mut eval = PuppetEval::new(&stage, &textures).unwrap();
    eval.set_zero_skip_for_proof(true);
    eval.begin_frame();
    eval.end_frame(0.0).unwrap();
    let frame = eval.render(W, H, SCALE);
    assert_eq!(pixel(&frame, 85, 100), [255, 0, 0, 255]);
    assert_eq!(pixel(&frame, 115, 100), [0, 0, 0, 0]);
}

#[test]
fn opacity_factor_does_not_compound_across_frames() {
    let (stage, textures) = build(&stage_json());
    let mut eval = PuppetEval::new(&stage, &textures).unwrap();
    let render_at_fade = |eval: &mut PuppetEval| -> Vec<u8> {
        eval.begin_frame();
        eval.set_param("Fade", [1.0, 0.0]).unwrap();
        eval.end_frame(0.0).unwrap();
        eval.render(W, H, SCALE)
    };
    let first = render_at_fade(&mut eval);
    let second = render_at_fade(&mut eval);
    assert_eq!(first, second, "opacity must re-derive from base each frame");
    // Factor 0.5 over transparent black: alpha halves.
    // (115, 90) is quad-only territory (the veil starts at
    // y = 100).
    assert_eq!(pixel(&first, 115, 90)[3], 128);
}

#[test]
fn physics_output_evolves_and_settles() {
    let (stage, textures) = build(&stage_json());
    let mut eval = PuppetEval::new(&stage, &textures).unwrap();
    // The spring bob starts at the world origin, far from
    // its anchor: frame 3 is mid-transient, so the physics
    // parameter must be well off its rest value.
    for _ in 0..3 {
        eval.begin_frame();
        eval.end_frame(1.0 / 60.0).unwrap();
    }
    let early = eval.param_value("Phys").unwrap();
    assert!(early[1].abs() > 0.05, "expected a transient, got {early:?}");
    // After ten seconds the pendulum hangs at rest: the XY
    // output of a hanging bob is ~(0, 0).
    for _ in 0..600 {
        eval.begin_frame();
        eval.end_frame(1.0 / 60.0).unwrap();
    }
    let settled = eval.param_value("Phys").unwrap();
    assert!(
        settled[0].abs() < 0.05 && settled[1].abs() < 0.05,
        "expected rest near (0, 0), got {settled:?}"
    );
}

#[test]
fn physics_blowup_inputs_stay_finite() {
    let (stage, textures) = build(&stage_json());
    let mut eval = PuppetEval::new(&stage, &textures).unwrap();
    for dt in [5.0f32, 10.0, 0.5] {
        eval.begin_frame();
        eval.end_frame(dt).unwrap();
        let v = eval.param_value("Phys").unwrap();
        assert!(
            v[0].is_finite() && v[1].is_finite(),
            "dt {dt} produced {v:?}"
        );
    }
    let err = eval.end_frame(-1.0).unwrap_err();
    assert_eq!(err.code(), "LRP-306");
}

#[test]
fn mask_clips_the_veil_to_the_quad() {
    let (stage, textures) = build(&stage_json());
    let mut eval = PuppetEval::new(&stage, &textures).unwrap();
    eval.begin_frame();
    eval.end_frame(0.0).unwrap();
    let frame = eval.render(W, H, SCALE);
    // The quad (after its +30 binding) spans [110, 150) x
    // [80, 120); the veil spans [110, 150) x [100, 140).
    // Inside the overlap the veil shows through its mask;
    // below the quad (y >= 120) the mask clips it away.
    assert_eq!(pixel(&frame, 10, 10), [0, 0, 0, 0]);
    assert_eq!(pixel(&frame, 115, 110), [255, 0, 0, 255]);
    assert_eq!(pixel(&frame, 125, 130), [0, 0, 0, 0]);
}

#[test]
fn evaluation_is_deterministic_across_sessions() {
    let scenario = |stage: &PuppetStage, textures: &[RgbaImage]| -> Vec<u8> {
        let mut eval = PuppetEval::new(stage, textures).unwrap();
        for frame in 0..30 {
            eval.begin_frame();
            if frame >= 10 {
                eval.set_param("Pan", [0.75, -0.5]).unwrap();
            }
            eval.end_frame(1.0 / 60.0).unwrap();
        }
        eval.render(W, H, SCALE)
    };
    let (stage, textures) = build(&stage_json());
    let a = scenario(&stage, &textures);
    let b = scenario(&stage, &textures);
    assert_eq!(a, b);
}

#[test]
fn parameter_values_clamp_to_the_grid_edge() {
    let (stage, textures) = build(&stage_json());
    let mut eval = PuppetEval::new(&stage, &textures).unwrap();
    eval.begin_frame();
    eval.set_param("Pan", [5.0, 0.0]).unwrap();
    eval.end_frame(0.0).unwrap();
    let frame = eval.render(W, H, SCALE);
    // Clamped to x = 1: grid value +60. Quad center =
    // 100 + 60 = 160; opaque half spans [140, 160).
    assert_eq!(pixel(&frame, 145, 100), [255, 0, 0, 255]);
    assert_eq!(pixel(&frame, 135, 100), [0, 0, 0, 0]);
}

/// Oracle accessor (D-870): the snapshot reports the state
/// `render` draws — the (0,0) binding's +30 shift in world
/// space, the opacity factor, and the root draw order.
#[cfg(feature = "oracle")]
#[test]
fn oracle_state_reports_the_evaluated_frame() {
    let (stage, textures) = build(&stage_json());
    let mut eval = PuppetEval::new(&stage, &textures).unwrap();
    eval.begin_frame();
    eval.set_param("Fade", [1.0, 0.0]).unwrap();
    eval.end_frame(0.0).unwrap();
    let state = eval.oracle_state();
    assert_eq!(state.nodes.len(), stage.nodes.len());
    let quad = &state.nodes[1];
    assert_eq!(quad.world_verts[0], [10.0, -20.0]);
    assert_eq!(quad.opacity, 0.5);
    // Equal zsorts: the stable sort keeps pre-order.
    assert_eq!(state.root_draw_order, vec![1, 2]);
    // Non-parts carry no geometry; groups are not drawables.
    assert!(state.nodes[0].world_verts.is_empty());
    assert_eq!(state.nodes[0].opacity, 1.0);
}

/// Adversarial: taking snapshots must not perturb evaluation —
/// a session snapshotted every frame renders byte-identically
/// to one never snapshotted, and repeated snapshots agree.
#[cfg(feature = "oracle")]
#[test]
fn oracle_state_is_read_only() {
    let (stage, textures) = build(&stage_json());
    let run = |snapshot: bool| -> (Vec<u8>, Option<lumen_rig_core::OracleState>) {
        let mut eval = PuppetEval::new(&stage, &textures).unwrap();
        let mut last = None;
        for _ in 0..30 {
            eval.begin_frame();
            eval.set_param("Pan", [0.75, -0.5]).unwrap();
            eval.end_frame(1.0 / 60.0).unwrap();
            if snapshot {
                let first = eval.oracle_state();
                assert_eq!(first, eval.oracle_state());
                last = Some(first);
            }
        }
        (eval.render(W, H, SCALE), last)
    };
    let (plain, _) = run(false);
    let (observed, state) = run(true);
    assert_eq!(plain, observed);
    assert!(
        state.unwrap().nodes[1]
            .world_verts
            .iter()
            .all(|v| v[0].is_finite())
    );
}
