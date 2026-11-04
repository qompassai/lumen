//! Puppet stage asset: validation tests (half) and
//! adversarial tests (half), per the house split.

use lumen_rig_core::parse_model;
use lumen_rig_core::puppet::{PuppetError, parse_puppet};
use serde_json::{Value, json};

fn model_json() -> Value {
    json!({
        "format": "lumen-rig-model",
        "format_version": 1,
        "id": "tiny",
        "parameters": [
            {"kind": "scalar", "id": "head_x", "min": -1.0, "max": 1.0, "default": 0.0},
            {"kind": "scalar", "id": "head_y", "min": -1.0, "max": 1.0, "default": 0.0},
            {"kind": "scalar", "id": "spin_x", "min": -1.0, "max": 1.0, "default": 0.0}
        ],
        "parts": [
            {"id": "root"},
            {"id": "face", "parent": "root"},
            {"id": "skirt", "parent": "root"},
            {"id": "hair_phys", "parent": "root"}
        ],
        "textures": [],
        "deformers": [],
        "meshes": [],
        "physics": []
    })
}

fn quad_mesh(id: &str) -> Value {
    json!({
        "id": id,
        "texture": "tex_a",
        "origin": [0.0, 0.0],
        "vertices": [[-10.0, -10.0], [10.0, -10.0], [10.0, 10.0], [-10.0, 10.0]],
        "uvs": [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
        "indices": [0, 1, 2, 0, 2, 3]
    })
}

fn node(id: &str, kind: &str) -> Value {
    json!({
        "id": id,
        "parent": "root",
        "kind": kind,
        "transform": {
            "translation": [0.0, 0.0, 0.0],
            "rotation_degrees": [0.0, 0.0, 0.0],
            "scale": [1.0, 1.0]
        }
    })
}

fn stage_json() -> Value {
    let mut face = node("face", "part");
    face["mesh"] = quad_mesh("face_mesh");
    face["drawable"] = json!({"blend_mode": "normal", "opacity": 1.0});
    let mut skirt = node("skirt", "part");
    skirt["mesh"] = quad_mesh("skirt_mesh");
    skirt["drawable"] = json!({"blend_mode": "multiply", "opacity": 0.9});
    skirt["masks"] = json!([{"source": "face", "mode": "mask"}]);
    let mut phys = node("hair_phys", "physics");
    phys["physics"] = json!({
        "parameter": "Spin",
        "model": "spring",
        "map_mode": "xy",
        "gravity": 1.0,
        "length": 100.0,
        "frequency": 1.0,
        "angle_damping": 0.5,
        "length_damping": 0.5,
        "output_scale": [1.0, 1.0]
    });
    json!({
        "format": "lumen-rig-puppet",
        "format_version": 1,
        "model": "tiny",
        "parameters": [
            {
                "id": "Head", "x": "head_x", "y": "head_y",
                "min": [-1.0, -1.0], "max": [1.0, 1.0], "default": [0.0, 0.0],
                "axis_points": [[-1.0, 0.0, 1.0], [-1.0, 0.0, 1.0]]
            },
            {
                "id": "Spin", "x": "spin_x",
                "min": [-1.0, 0.0], "max": [1.0, 1.0], "default": [0.0, 0.0],
                "axis_points": [[-1.0, 0.0, 1.0], []]
            }
        ],
        "textures": [{"id": "tex_a", "source": "textures/tex_a.png"}],
        "nodes": [
            {
                "id": "root", "kind": "group",
                "transform": {
                    "translation": [0.0, 0.0, 0.0],
                    "rotation_degrees": [0.0, 0.0, 0.0],
                    "scale": [1.0, 1.0]
                }
            },
            face, skirt, phys
        ],
        "bindings": [
            {
                "parameter": "Head", "node": "face", "kind": "translation_x",
                "interpolate": "linear",
                "values": [[0.0, 0.0, 0.0], [0.0, 5.0, 0.0], [0.0, 0.0, 0.0]]
            },
            {
                "parameter": "Head", "node": "face", "kind": "deform",
                "interpolate": "linear",
                "values": [
                    [[[0.0, 0.0], [0.0, 0.0], [0.0, 0.0], [0.0, 0.0]],
                     [[0.0, 0.0], [0.0, 0.0], [0.0, 0.0], [0.0, 0.0]],
                     [[0.0, 0.0], [0.0, 0.0], [0.0, 0.0], [0.0, 0.0]]],
                    [[[0.0, 0.0], [0.0, 0.0], [0.0, 0.0], [0.0, 0.0]],
                     [[1.0, 2.0], [1.0, 2.0], [1.0, 2.0], [1.0, 2.0]],
                     [[0.0, 0.0], [0.0, 0.0], [0.0, 0.0], [0.0, 0.0]]],
                    [[[0.0, 0.0], [0.0, 0.0], [0.0, 0.0], [0.0, 0.0]],
                     [[0.0, 0.0], [0.0, 0.0], [0.0, 0.0], [0.0, 0.0]],
                     [[0.0, 0.0], [0.0, 0.0], [0.0, 0.0], [0.0, 0.0]]]
                ]
            }
        ],
        "physics": {"pixels_per_meter": 1000.0, "gravity": 9.8}
    })
}

fn parse(stage: &Value) -> Result<lumen_rig_core::puppet::PuppetStage, PuppetError> {
    let model = parse_model(&serde_json::to_vec(&model_json()).unwrap()).unwrap();
    parse_puppet(&serde_json::to_vec(stage).unwrap(), &model)
}

fn code(stage: &Value) -> &'static str {
    parse(stage).unwrap_err().code()
}

/* ---------------- validation ---------------- */

#[test]
fn valid_stage_parses() {
    let stage = parse(&stage_json()).unwrap();
    assert_eq!(stage.parameters.len(), 2);
    assert_eq!(stage.nodes.len(), 4);
    assert_eq!(stage.bindings.len(), 2);
}

#[test]
fn canonical_round_trip_is_identity() {
    let stage = parse(&stage_json()).unwrap();
    let text = stage.to_canonical_json();
    let model = parse_model(&serde_json::to_vec(&model_json()).unwrap()).unwrap();
    let again = parse_puppet(text.as_bytes(), &model).unwrap();
    assert_eq!(stage, again);
    assert_eq!(text, again.to_canonical_json());
}

#[test]
fn cubic_bindings_are_carried() {
    let mut stage = stage_json();
    stage["bindings"][0]["interpolate"] = json!("cubic");
    let stage = parse(&stage).unwrap();
    assert_eq!(
        stage.bindings[0].interpolate,
        lumen_rig_core::puppet::PuppetInterpolate::Cubic
    );
}

/* ---------------- adversarial ---------------- */

#[test]
fn wrong_format_is_lrp_105() {
    let mut stage = stage_json();
    stage["format"] = json!("lumen-rig-model");
    assert_eq!(code(&stage), "LRP-105");
}

#[test]
fn model_mismatch_is_lrp_206() {
    let mut stage = stage_json();
    stage["model"] = json!("other");
    assert_eq!(code(&stage), "LRP-206");
}

#[test]
fn channel_range_mismatch_is_lrp_206() {
    let mut stage = stage_json();
    stage["parameters"][0]["max"] = json!([2.0, 1.0]);
    assert_eq!(code(&stage), "LRP-206");
}

#[test]
fn unknown_field_is_lrp_103() {
    let mut stage = stage_json();
    stage["nodes"][0]["surprise"] = json!(true);
    assert_eq!(code(&stage), "LRP-103");
}

#[test]
fn duplicate_node_id_is_lrp_202() {
    let mut stage = stage_json();
    stage["nodes"][2]["id"] = json!("face");
    assert_eq!(code(&stage), "LRP-202");
}

#[test]
fn dangling_mask_source_is_lrp_203() {
    let mut stage = stage_json();
    stage["nodes"][2]["masks"][0]["source"] = json!("ghost");
    assert_eq!(code(&stage), "LRP-203");
}

#[test]
fn mask_cycle_is_lrp_204() {
    let mut stage = stage_json();
    stage["nodes"][1]["masks"] = json!([{"source": "skirt", "mode": "mask"}]);
    assert_eq!(code(&stage), "LRP-204");
}

#[test]
fn two_roots_is_lrp_204() {
    let mut stage = stage_json();
    stage["nodes"][1]["parent"] = Value::Null;
    // The model disagrees first (LRP-206 parent mirror); make
    // the model agree so the forest rule is what fires.
    let mut model = model_json();
    model["parts"][1]["parent"] = Value::Null;
    let model = parse_model(&serde_json::to_vec(&model).unwrap()).unwrap();
    let err = parse_puppet(&serde_json::to_vec(&stage).unwrap(), &model).unwrap_err();
    assert_eq!(err.code(), "LRP-204");
}

#[test]
fn grid_shape_mismatch_is_lrp_303() {
    let mut stage = stage_json();
    stage["bindings"][0]["values"] = json!([[0.0, 0.0], [0.0, 5.0]]);
    assert_eq!(code(&stage), "LRP-303");
}

#[test]
fn deform_cell_vertex_mismatch_is_lrp_303() {
    let mut stage = stage_json();
    stage["bindings"][1]["values"][1][1] = json!([[1.0, 2.0]]);
    assert_eq!(code(&stage), "LRP-303");
}

#[test]
fn axis_not_increasing_is_lrp_304() {
    let mut stage = stage_json();
    stage["parameters"][0]["axis_points"][0] = json!([-1.0, 0.5, 0.25]);
    assert_eq!(code(&stage), "LRP-304");
}

#[test]
fn second_physics_writer_is_lrp_205() {
    let mut stage = stage_json();
    let mut model = model_json();
    model["parts"]
        .as_array_mut()
        .unwrap()
        .push(json!({"id": "hair_phys2", "parent": "root"}));
    let mut phys2 = node("hair_phys2", "physics");
    phys2["physics"] = stage["nodes"][3]["physics"].clone();
    stage["nodes"].as_array_mut().unwrap().push(phys2);
    let model = parse_model(&serde_json::to_vec(&model).unwrap()).unwrap();
    let err = parse_puppet(&serde_json::to_vec(&stage).unwrap(), &model).unwrap_err();
    assert_eq!(err.code(), "LRP-205");
}

#[test]
fn opacity_binding_on_group_is_lrp_207() {
    let mut stage = stage_json();
    stage["bindings"].as_array_mut().unwrap().push(json!({
        "parameter": "Head", "node": "root", "kind": "opacity",
        "interpolate": "linear",
        "values": [[1.0, 1.0, 1.0], [1.0, 1.0, 1.0], [1.0, 1.0, 1.0]]
    }));
    assert_eq!(code(&stage), "LRP-207");
}

#[test]
fn duplicate_binding_is_lrp_202() {
    let mut stage = stage_json();
    let dup = stage["bindings"][0].clone();
    stage["bindings"].as_array_mut().unwrap().push(dup);
    assert_eq!(code(&stage), "LRP-202");
}
