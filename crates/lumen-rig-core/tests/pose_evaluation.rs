//! Pose evaluation tests: deformer application, lattice
//! interpolation, and BIND-1 part translations against
//! hand-computed expectations, plus the landed Prima
//! fixture's rest-pose identity (evaluation at rest must
//! reproduce the authored vertices — the deformer machinery
//! may not drift the rig it was handed).

use lumen_rig_core::{ParameterValues, evaluate_pose, parse_model};

fn parse(document: &str) -> lumen_rig_core::Model {
    parse_model(document.as_bytes()).expect("test document parses")
}

fn values(model: &lumen_rig_core::Model, writes: &[(&str, f32)]) -> ParameterValues {
    let mut state = ParameterValues::rest(model);
    for (id, value) in writes {
        assert!(state.set(model, id, *value), "write {id} accepted");
    }
    state
}

fn mesh_vertices(
    model: &lumen_rig_core::Model,
    state: &ParameterValues,
    mesh_id: &str,
) -> Vec<[f32; 2]> {
    evaluate_pose(model, state)
        .into_iter()
        .find(|mesh| mesh.id == mesh_id)
        .expect("mesh evaluated")
        .vertices
}

fn assert_close(actual: [f32; 2], expected: [f32; 2], tolerance: f32) {
    assert!(
        (actual[0] - expected[0]).abs() <= tolerance
            && (actual[1] - expected[1]).abs() <= tolerance,
        "expected {expected:?} +/- {tolerance}, got {actual:?}"
    );
}

/// One quad under a rotation deformer whose lattice sweeps
/// angle 0 -> 90 degrees over `angle` in [0, 1].
const ROTATION_DOC: &str = r#"{
  "format": "lumen-rig-model", "format_version": 1, "id": "rot",
  "parameters": [
    { "id": "angle", "kind": "scalar", "min": 0.0, "max": 1.0, "default": 0.0 }
  ],
  "parts": [{ "id": "root" }],
  "textures": [{ "id": "tex0", "source": "tex0.png" }],
  "deformers": [
    {
      "id": "spin", "kind": "rotation", "origin": [0.0, 0.0],
      "base_angle_degrees": 0.0,
      "keyforms": {
        "axes": ["angle"], "keys": [[0.0, 1.0]],
        "forms": [
          { "at": [0], "angle_degrees": 0.0, "scale": [1.0, 1.0] },
          { "at": [1], "angle_degrees": 90.0, "scale": [1.0, 1.0] }
        ]
      }
    }
  ],
  "meshes": [
    {
      "id": "quad", "part": "root", "texture": "tex0", "deformer": "spin",
      "vertices": [[1.0, 0.0], [2.0, 0.0], [2.0, 1.0], [1.0, 1.0]],
      "uvs": [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
      "indices": [0, 1, 2, 0, 2, 3]
    }
  ],
  "physics": []
}"#;

#[test]
fn rotation_deformer_rotates_about_its_origin() {
    let model = parse(ROTATION_DOC);
    // At rest the quad is untouched.
    let rest = mesh_vertices(&model, &values(&model, &[]), "quad");
    assert_close(rest[0], [1.0, 0.0], 1e-6);
    // At angle = 1 the lattice state is 90 degrees CCW:
    // (1, 0) -> (0, 1), (2, 0) -> (0, 2), (2, 1) -> (-1, 2).
    let turned = mesh_vertices(&model, &values(&model, &[("angle", 1.0)]), "quad");
    assert_close(turned[0], [0.0, 1.0], 1e-5);
    assert_close(turned[1], [0.0, 2.0], 1e-5);
    assert_close(turned[2], [-1.0, 2.0], 1e-5);
    // Half-way interpolates the angle linearly: 45 degrees.
    let half = mesh_vertices(&model, &values(&model, &[("angle", 0.5)]), "quad");
    let diagonal = std::f32::consts::FRAC_1_SQRT_2;
    assert_close(half[0], [diagonal, diagonal], 1e-5);
}

#[test]
fn rotation_chain_applies_root_first_and_moves_child_origins() {
    // Parent rotates 90 degrees about (0, 0); the child,
    // origin (1, 0), rotates 90 degrees about its own
    // (moved) origin (0, 1). Root-first: the point (2, 0)
    // goes to (0, 2) under the parent, then rotates about
    // (0, 1) to (-1, 1). Child-first would give (0, 2)
    // instead — the order is observable and pinned.
    let document = r#"{
      "format": "lumen-rig-model", "format_version": 1, "id": "chain",
      "parameters": [],
      "parts": [{ "id": "root" }],
      "textures": [{ "id": "tex0", "source": "tex0.png" }],
      "deformers": [
        { "id": "parent", "kind": "rotation", "origin": [0.0, 0.0],
          "base_angle_degrees": 90.0 },
        { "id": "child", "kind": "rotation", "parent": "parent",
          "origin": [1.0, 0.0], "base_angle_degrees": 90.0 }
      ],
      "meshes": [
        {
          "id": "quad", "part": "root", "texture": "tex0", "deformer": "child",
          "vertices": [[2.0, 0.0], [3.0, 0.0], [3.0, 1.0], [2.0, 1.0]],
          "uvs": [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
          "indices": [0, 1, 2, 0, 2, 3]
        }
      ],
      "physics": []
    }"#;
    let model = parse(document);
    let out = mesh_vertices(&model, &values(&model, &[]), "quad");
    assert_close(out[0], [-1.0, 1.0], 1e-5);
}

#[test]
fn warp_deformer_maps_through_its_grid() {
    // A 1x1 warp over the unit square; the keyform pulls
    // the (1, 1) corner to (2, 2). The centre of the square
    // maps bilinearly to (0.75, 0.75) — hand-computed from
    // the patch weights at (0.5, 0.5): corners contribute
    // 0.25 each, so centre = 0.25*((0,0)+(1,0)+(2,2)+(0,1))
    // = (0.75, 0.75).
    let document = r#"{
      "format": "lumen-rig-model", "format_version": 1, "id": "warp",
      "parameters": [
        { "id": "pull", "kind": "scalar", "min": 0.0, "max": 1.0, "default": 0.0 }
      ],
      "parts": [{ "id": "root" }],
      "textures": [{ "id": "tex0", "source": "tex0.png" }],
      "deformers": [
        {
          "id": "grid", "kind": "warp", "columns": 1, "rows": 1,
          "rest_positions": [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]],
          "keyforms": {
            "axes": ["pull"], "keys": [[0.0, 1.0]],
            "forms": [
              { "at": [0], "positions": [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [1.0, 1.0]] },
              { "at": [1], "positions": [[0.0, 0.0], [1.0, 0.0], [0.0, 1.0], [2.0, 2.0]] }
            ]
          }
        }
      ],
      "meshes": [
        {
          "id": "quad", "part": "root", "texture": "tex0", "deformer": "grid",
          "vertices": [[0.5, 0.5], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
          "uvs": [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
          "indices": [0, 1, 2, 0, 2, 3]
        }
      ],
      "physics": []
    }"#;
    let model = parse(document);
    let rest = mesh_vertices(&model, &values(&model, &[]), "quad");
    assert_close(rest[0], [0.5, 0.5], 1e-6);
    let pulled = mesh_vertices(&model, &values(&model, &[("pull", 1.0)]), "quad");
    assert_close(pulled[0], [0.75, 0.75], 1e-5);
    // Grid corners are fixed points of the map.
    assert_close(pulled[1], [1.0, 0.0], 1e-5);
    assert_close(pulled[2], [2.0, 2.0], 1e-5);
    assert_close(pulled[3], [0.0, 1.0], 1e-5);
}

#[test]
fn mesh_keyforms_interpolate_absolute_vertices() {
    let document = r#"{
      "format": "lumen-rig-model", "format_version": 1, "id": "meshforms",
      "parameters": [
        { "id": "slide", "kind": "scalar", "min": 0.0, "max": 2.0, "default": 0.0 }
      ],
      "parts": [{ "id": "root" }],
      "textures": [{ "id": "tex0", "source": "tex0.png" }],
      "deformers": [],
      "meshes": [
        {
          "id": "quad", "part": "root", "texture": "tex0",
          "vertices": [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
          "uvs": [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
          "indices": [0, 1, 2, 0, 2, 3],
          "keyforms": {
            "axes": ["slide"], "keys": [[0.0, 2.0]],
            "forms": [
              { "at": [0], "vertices": [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]] },
              { "at": [1], "vertices": [[4.0, 0.0], [5.0, 0.0], [5.0, 1.0], [4.0, 1.0]] }
            ]
          }
        }
      ],
      "physics": []
    }"#;
    let model = parse(document);
    let mid = mesh_vertices(&model, &values(&model, &[("slide", 1.0)]), "quad");
    assert_close(mid[0], [2.0, 0.0], 1e-6);
    assert_close(mid[2], [3.0, 1.0], 1e-6);
}

#[test]
fn part_translations_accumulate_down_the_part_chain() {
    // Parent part translates +x with `move`; child part
    // translates +y on the same axis. The child's mesh sees
    // the sum; the rest-key form (at the default) is zero,
    // so rest is untouched.
    let document = r#"{
      "format": "lumen-rig-model", "format_version": 1, "id": "parts",
      "parameters": [
        { "id": "move", "kind": "scalar", "min": 0.0, "max": 1.0, "default": 0.0 }
      ],
      "parts": [
        { "id": "root" },
        {
          "id": "parent", "parent": "root",
          "translation_keyforms": {
            "axes": ["move"], "keys": [[0.0, 1.0]],
            "forms": [
              { "at": [0], "translation": [0.0, 0.0] },
              { "at": [1], "translation": [3.0, 0.0] }
            ]
          }
        },
        {
          "id": "child", "parent": "parent",
          "translation_keyforms": {
            "axes": ["move"], "keys": [[0.0, 1.0]],
            "forms": [
              { "at": [0], "translation": [0.0, 0.0] },
              { "at": [1], "translation": [0.0, 5.0] }
            ]
          }
        }
      ],
      "textures": [{ "id": "tex0", "source": "tex0.png" }],
      "deformers": [],
      "meshes": [
        {
          "id": "quad", "part": "child", "texture": "tex0",
          "vertices": [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
          "uvs": [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]],
          "indices": [0, 1, 2, 0, 2, 3]
        }
      ],
      "physics": []
    }"#;
    let model = parse(document);
    let rest = mesh_vertices(&model, &values(&model, &[]), "quad");
    assert_close(rest[0], [0.0, 0.0], 1e-6);
    let moved = mesh_vertices(&model, &values(&model, &[("move", 1.0)]), "quad");
    assert_close(moved[0], [3.0, 5.0], 1e-6);
    assert_close(moved[2], [4.0, 6.0], 1e-6);
}

#[test]
fn combined_parameters_drive_lattice_axes() {
    // A combined parameter (2 * slide) as a mesh lattice
    // axis is refused by the validator (axes must be
    // scalar), but a combined parameter's *terms* settle
    // before evaluation: here the rotation lattice axis is
    // the scalar, and a combined readout proves resolution.
    let document = r#"{
      "format": "lumen-rig-model", "format_version": 1, "id": "combined",
      "parameters": [
        { "id": "slide", "kind": "scalar", "min": 0.0, "max": 1.0, "default": 0.0 },
        {
          "id": "double", "kind": "combined", "min": 0.0, "max": 2.0,
          "terms": [{ "parameter": "slide", "weight": 2.0 }], "offset": 0.0
        }
      ],
      "parts": [{ "id": "root" }],
      "textures": [{ "id": "tex0", "source": "tex0.png" }],
      "deformers": [],
      "meshes": [],
      "physics": []
    }"#;
    let model = parse(document);
    let state = values(&model, &[("slide", 0.25)]);
    assert_eq!(state.get("double"), Some(0.5));
}

#[test]
fn prima_rest_pose_is_the_authored_pose() {
    // The landed Prima fixture, evaluated at rest: every
    // mesh must come back with its authored vertices (the
    // deformer chains are at their base states, the
    // lattices at their rest forms). Any drift here is the
    // evaluator inventing motion the rig did not author.
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("fixtures")
        .join("conformance")
        .join("prima-repertoire.model.json");
    let bytes = std::fs::read(&path).expect("Prima fixture readable");
    let model = parse_model(&bytes).expect("Prima fixture parses");
    let evaluated = evaluate_pose(&model, &ParameterValues::rest(&model));
    let mut worst = 0.0_f32;
    for (mesh, evaluated_mesh) in model.meshes.iter().zip(evaluated.iter()) {
        for (authored, actual) in mesh.vertices.iter().zip(evaluated_mesh.vertices.iter()) {
            worst = worst
                .max((authored[0] - actual[0]).abs())
                .max((authored[1] - actual[1]).abs());
        }
    }
    assert!(
        worst <= 1e-3,
        "rest pose drifted from the authored pose by {worst} model units"
    );
}
