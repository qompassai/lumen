//! INP (.inx) -> native conversion tests (exporter phase
//! E2's upstream stage).
//!
//! The validation side runs a synthetic puppet that exercises
//! every construct class the stage maps — vec2/scalar
//! parameters, a deform lattice, a driven node (warp), a
//! physics group with a folded physics-driven deform binding,
//! and an unbound parameter — plus the fixture-level tests:
//! the vendored Aka puppet, which converts in-tree since the
//! native texture bound rose to 256 (SPEC section 11, amended
//! 2026-10-26; a synthetic 257-slot puppet pins the bound
//! from above with LRF-107), and the E2-emitted Aka.moc3
//! read back through the R5a parser. (As landed in lumen,
//! 2026-10-29: the Aka fixtures live in sibling workspaces,
//! so those two tests run when the fixtures are present and
//! skip otherwise; the Ayagami second-reader layer does not
//! run here — ayagami is not a dependency of this crate.)
//! The adversarial side feeds malformed containers and
//! payloads and expects typed refusals, never panics.

#![recursion_limit = "256"]

use std::path::PathBuf;

use lumen_rig_import::convert_inp;
use lumen_rig_import::error::ConvertError;
use lumen_rig_import::moc3::Moc3File;

/// One texture slot: an 18-byte TGA header (width 4, height 4,
/// 32 bpp) followed by filler pixel bytes the stage must
/// never read.
fn texture_slot() -> Vec<u8> {
    let mut slot = vec![0u8, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 4, 0, 4, 0, 32, 0];
    slot.extend_from_slice(&[7u8; 64]);
    slot
}

/// Assemble an INP1 container around a JSON payload.
fn container(payload: &[u8], slots: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"TRNSRTS\0");
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
    out.extend_from_slice(b"TEX_SECT");
    out.extend_from_slice(&(slots.len() as u32).to_be_bytes());
    for slot in slots {
        out.extend_from_slice(&(slot.len() as u32).to_be_bytes());
        out.push(1);
        out.extend_from_slice(slot);
    }
    out
}

fn identity_transform() -> serde_json::Value {
    serde_json::json!({"trans": [0.0, 0.0, 0.0], "rot": [0.0, 0.0, 0.0], "scale": [1.0, 1.0]})
}

/// The synthetic puppet payload. Structure:
/// Root (Node)
/// `- Panel (Part, 1 triangle, texture slot 0)
///    `- Hair:: Physics Node (SimplePhysics)
/// Parameters: Head:: Tilt (vec2, deform on Panel),
/// Blink (scalar, transform.t.x on Panel), Hair:: Physics
/// (vec2, deform on Panel, driven by the physics node),
/// Unused (scalar, no bindings).
fn mini_payload() -> serde_json::Value {
    let zero_point = || serde_json::json!([[0.0, 0.0], [0.0, 0.0], [0.0, 0.0]]);
    let tilt_values = serde_json::json!([
        [zero_point(), zero_point()],
        [zero_point(), zero_point()],
        [
            [[30.0, 0.0], [0.0, 0.0], [0.0, 0.0]],
            [[30.0, 20.0], [0.0, 0.0], [0.0, 0.0]]
        ]
    ]);
    let hair_values = serde_json::json!([
        [zero_point(), zero_point()],
        [zero_point(), [[5.0, 5.0], [0.0, 0.0], [0.0, 0.0]]]
    ]);
    serde_json::json!({
        "meta": {"name": "Mini"},
        "physics": {"pixelsPerMeter": 1000.0, "gravity": 9.8},
        "nodes": {
            "uuid": 1, "name": "Root", "type": "Node", "enabled": true,
            "zsort": 0.0, "transform": identity_transform(),
            "children": [{
                "uuid": 2, "name": "Panel", "type": "Part", "enabled": true,
                "zsort": 0.0, "transform": identity_transform(),
                "mesh": {
                    "verts": [0.0, 0.0, 100.0, 0.0, 0.0, 100.0],
                    "uvs": [0.0, 0.0, 1.0, 0.0, 0.0, 1.0],
                    "indices": [0, 1, 2],
                    "origin": [0.0, 0.0]
                },
                "opacity": 1.0, "blend_mode": "Normal", "textures": [0],
                "masks": [],
                "children": [{
                    "uuid": 3, "name": "Hair:: Physics Node", "type": "SimplePhysics",
                    "enabled": true, "zsort": 0.0, "transform": identity_transform(),
                    "param": 13, "model_type": "SpringPendulum", "map_mode": "XY",
                    "gravity": 1.0, "length": 120.0, "frequency": 2.0,
                    "angle_damping": 0.5, "length_damping": 0.5,
                    "output_scale": [0.25, 0.5]
                }]
            }]
        },
        "param": [
            {
                "uuid": 11, "name": "Head:: Tilt", "is_vec2": true,
                "min": [-1.0, -1.0], "max": [1.0, 1.0], "defaults": [0.0, 0.0],
                "axis_points": [[0.0, 0.5, 1.0], [0.0, 1.0]],
                "bindings": [{
                    "node": 2, "param_name": "deform",
                    "values": tilt_values,
                    "isSet": [[true, true], [true, true], [true, true]],
                    "interpolate_mode": "Linear"
                }]
            },
            {
                "uuid": 12, "name": "Blink", "is_vec2": false,
                "min": [0.0, 0.0], "max": [1.0, 1.0], "defaults": [0.0, 0.0],
                "axis_points": [[0.0, 1.0], [0.0]],
                "bindings": [{
                    "node": 2, "param_name": "transform.t.x",
                    "values": [[0.0], [25.0]],
                    "isSet": [[true], [true]],
                    "interpolate_mode": "Linear"
                }]
            },
            {
                "uuid": 13, "name": "Hair:: Physics", "is_vec2": true,
                "min": [-1.0, -1.0], "max": [1.0, 1.0], "defaults": [0.0, 0.0],
                "axis_points": [[0.0, 1.0], [0.0, 1.0]],
                "bindings": [{
                    "node": 2, "param_name": "deform",
                    "values": hair_values,
                    "isSet": [[true, true], [true, true]],
                    "interpolate_mode": "Linear"
                }]
            },
            {
                "uuid": 14, "name": "Unused", "is_vec2": false,
                "min": [0.0, 0.0], "max": [1.0, 1.0], "defaults": [0.0, 0.0],
                "axis_points": [[0.0, 1.0], [0.0]],
                "bindings": []
            }
        ]
    })
}

fn mini_inx() -> Vec<u8> {
    let payload = serde_json::to_vec(&mini_payload()).expect("payload serializes");
    container(&payload, &[texture_slot()])
}

fn convert_mini() -> lumen_rig_import::InpOutcome {
    convert_inp(&mini_inx(), "Mini").expect("the synthetic puppet converts")
}

fn assert_approx(actual: f32, expected: f32, what: &str) {
    assert!(
        (actual - expected).abs() < 1e-4,
        "{what}: expected {expected}, got {actual}"
    );
}

#[test]
fn mini_puppet_converts_with_expected_shape() {
    let outcome = convert_mini();
    let model = &outcome.model;
    let counts = &outcome.report.counts;
    assert_eq!(counts.inp_parameters, 4);
    assert_eq!(counts.native_parameters, 6);
    assert_eq!(counts.inp_nodes, 3);
    assert_eq!(counts.native_parts, 1);
    assert_eq!(counts.native_meshes, 1);
    assert_eq!(counts.native_deformers, 1);
    assert_eq!(counts.physics_groups, 1);
    assert_eq!(counts.physics_strands, 2);
    assert_eq!(counts.bindings_total, 3);
    assert_eq!(counts.deform_bindings, 2);
    assert_eq!(counts.value_bindings, 1);

    // Parameters: vec2 splits per axis; the unbound parameter
    // is carried and referenced by nothing.
    let ids: Vec<&str> = model
        .parameters
        .iter()
        .map(|parameter| match parameter {
            lumen_rig_core::Parameter::Scalar(scalar) => scalar.id.as_str(),
            _ => panic!("the INP stage mints scalar parameters only"),
        })
        .collect();
    assert_eq!(
        ids,
        [
            "Head___Tilt_x",
            "Head___Tilt_y",
            "Blink",
            "Hair___Physics_x",
            "Hair___Physics_y",
            "Unused"
        ]
    );
    let mut referenced: Vec<&str> = Vec::new();
    for deformer in &model.deformers {
        let lumen_rig_core::Deformer::Warp(warp) = deformer else {
            continue;
        };
        if let Some(lattice) = &warp.keyforms {
            referenced.extend(lattice.axes.iter().map(String::as_str));
        }
    }
    for mesh in &model.meshes {
        if let Some(lattice) = &mesh.keyforms {
            referenced.extend(lattice.axes.iter().map(String::as_str));
        }
    }
    assert!(
        !referenced.contains(&"Unused"),
        "the unbound parameter must stay unbound"
    );

    // The mesh: rest verts carry the physics binding's
    // default-pose offset (bilinear center of the [1][1]
    // corner: 0.25 * [5, 5] = [1.25, 1.25] on vertex 0).
    let mesh = &model.meshes[0];
    assert_eq!(mesh.id, "Panel");
    assert_approx(mesh.vertices[0][0], 1.25, "rest vert 0 x");
    assert_approx(mesh.vertices[0][1], 1.25, "rest vert 0 y");
    assert_approx(mesh.vertices[1][0], 100.0, "rest vert 1 x");
    assert_approx(mesh.vertices[2][1], 100.0, "rest vert 2 y");
    assert_eq!(mesh.texture, "texture_00");
    assert_eq!(mesh.deformer.as_deref(), Some("Panel__warp"));
    let lattice = mesh.keyforms.as_ref().expect("mesh lattice");
    assert_eq!(lattice.axes.len(), 2);
    assert_eq!(lattice.axes[0], "Head___Tilt_x");
    assert_eq!(lattice.axes[1], "Head___Tilt_y");
    assert_eq!(lattice.keys, vec![vec![-1.0, 0.0, 1.0], vec![-1.0, 1.0]]);
    assert_eq!(lattice.forms.len(), 6);
    let extreme = lattice
        .forms
        .iter()
        .find(|form| form.at == [2, 0])
        .expect("the (1, -1) form exists");
    assert_approx(extreme.vertices[0][0], 31.25, "form vert 0 x");
    assert_approx(extreme.vertices[0][1], 1.25, "form vert 0 y");

    // The warp from the Blink transform binding.
    let lumen_rig_core::Deformer::Warp(warp) = &model.deformers[0] else {
        panic!("the driven node becomes a warp");
    };
    assert_eq!(warp.id, "Panel__warp");
    assert_eq!(warp.parent, None);
    let warp_lattice = warp.keyforms.as_ref().expect("warp lattice");
    assert_eq!(warp_lattice.axes.len(), 1);
    assert_eq!(warp_lattice.axes[0], "Blink");
    assert_eq!(warp_lattice.keys, vec![vec![0.0, 1.0]]);
    let shifted = warp_lattice
        .forms
        .iter()
        .find(|form| form.at == [1])
        .expect("the Blink=1 form exists");
    assert_approx(shifted.positions[0][0], 19.0, "warp corner x at Blink=1");
    assert_approx(shifted.positions[0][1], -6.0, "warp corner y at Blink=1");

    // Physics: one group, two strands, source scales carried.
    let group = &model.physics[0];
    assert_eq!(group.id, "Hair___Physics_Node");
    assert_eq!(group.gravity, [0.0, 9.8]);
    assert_eq!(group.strands.len(), 2);
    let out_x = group.strands[0]
        .output
        .as_ref()
        .expect("strand output exists");
    assert_eq!(out_x.parameter, "Hair___Physics_x");
    assert_approx(out_x.scale, 0.25, "strand x scale");
    let out_y = group.strands[1]
        .output
        .as_ref()
        .expect("strand output exists");
    assert_eq!(out_y.parameter, "Hair___Physics_y");
    assert_approx(out_y.scale, 0.5, "strand y scale");
    assert_approx(
        group.strands[0].particles[0].length,
        120.0,
        "pendulum length",
    );
    assert_eq!(group.strands[0].anchor.id, "Panel");

    // The report names the physics fold and the anchor loss.
    assert!(
        outcome
            .report
            .lossy
            .iter()
            .any(|entry| entry.item.contains("physics parameter")),
        "the folded physics deform binding is reported"
    );
    assert!(
        outcome
            .report
            .lossy
            .iter()
            .any(|entry| entry.item.contains("anchor driver")),
        "the map_mode driver loss is reported"
    );
    // Texture inventory: header dimensions only.
    let inventory = &outcome.report.source.texture_inventory;
    assert_eq!(inventory.len(), 1);
    assert_eq!(inventory[0].width, Some(4));
    assert_eq!(inventory[0].height, Some(4));
    assert_eq!(model.textures[0].source, "texture_00.tga");
}

#[test]
fn aka_fixture_converts_under_the_raised_texture_bound() {
    // The vendored Aka puppet (Torogao fixtures, CC-BY 4.0)
    // references 76 texture slots. Rig-core v1 originally
    // capped a model's textures at 64 (SPEC section 11), so
    // E2's in-tree conversion was refused by that bound alone
    // (LRF-107) and the round-trip proof ran in a disclosed
    // scratch copy. The bound rose to 256 (SPEC section 11,
    // amended 2026-10-26), so conversion now passes
    // self-validation in-tree. If the bound ever drops below
    // 76 again, this test fails loudly.
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../torogao-rp0-20261026/torogao/fixtures/example-models/aka/Aka.inx");
    // The fixture lives in a sibling workspace; as landed in
    // lumen it is not part of this repo, so the test runs
    // where the fixture is present and skips where it is not
    // (the official-sample pattern in validation.rs).
    let Ok(bytes) = std::fs::read(&path) else {
        return;
    };
    let outcome = convert_inp(&bytes, "Aka").expect("Aka converts under the 256 texture bound");
    assert_eq!(outcome.model.textures.len(), 76);
    let counts = &outcome.report.counts;
    assert_eq!(counts.inp_parameters, 34);
    assert_eq!(counts.native_parameters, 56);
    assert_eq!(counts.inp_nodes, 100);
    assert_eq!(counts.native_parts, 76);
    assert_eq!(counts.native_meshes, 76);
    assert_eq!(counts.native_deformers, 63);
    assert_eq!(counts.physics_groups, 19);
    assert_eq!(counts.physics_strands, 38);
    assert_eq!(counts.bindings_total, 259);
}

#[test]
fn texture_bound_refuses_above_256_slots() {
    // The pin for the amended bound (SPEC section 11,
    // 2026-10-26): 256 texture slots is the rig-core v1
    // maximum, so a puppet carrying 257 slots — one part, one
    // reference, nothing else near a limit — must refuse at
    // self-validation with exactly LRF-107 naming the 256
    // bound. If TEXTURES_MAX moves silently in either
    // direction, this test (or the Aka test above) fails.
    let payload = serde_json::to_vec(&mini_payload()).expect("payload serializes");
    let slots: Vec<Vec<u8>> = (0..257).map(|_| texture_slot()).collect();
    let bytes = container(&payload, &slots);
    let error = convert_inp(&bytes, "Mini").expect_err("257 slots exceed the texture bound");
    let ConvertError::NativeValidation { code, message } = &error else {
        panic!("expected the native validation refusal, got {error}");
    };
    assert_eq!(*code, "LRF-107");
    assert!(
        message.contains("textures count 257 exceeds max 256"),
        "the refusal names the texture bound: {message}"
    );
}

#[test]
fn e2_emitted_aka_moc3_parses_in_own_reader() {
    // The E2 deliverable artifact: Aka native -> moc3 V3_0
    // (canvas: 1 px/unit, origin at the model's rest-bounds
    // center, size = the bounds span). Layer 1 (our R5a
    // parser) evidence; layer 2 (Ayagami as a library) ran
    // only in the converter's original workspace — ayagami
    // is not a dependency of this crate as landed in lumen;
    // layer 3 (the importer round trip) is exercised by the
    // E2 harness and reconciled in E2-REPORT.md. The artifact
    // lives in a sibling workspace, so the test runs when it
    // is present and skips otherwise.
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../moc3-export-20261026/e2-output/Aka.moc3");
    let Ok(bytes) = std::fs::read(&path) else {
        return;
    };
    let parsed = Moc3File::parse(&bytes).expect("the R5a parser reads the emitted moc3");
    assert_eq!(parsed.counts.parameters, 56);
    assert_eq!(parsed.counts.parts, 76);
    assert_eq!(parsed.counts.warp_deformers, 63);
    assert_eq!(parsed.counts.rotation_deformers, 0);
    assert_eq!(parsed.counts.art_meshes, 76);
    assert_eq!(parsed.counts.drawable_masks, 26);
    assert_eq!(parsed.counts.glue, 0);
    assert_eq!(parsed.canvas.pixels_per_unit, 1.0);
    assert_eq!(parsed.canvas.origin_x, 310.0216);
    assert_eq!(parsed.canvas.origin_y, -68.30815);
    assert_eq!(parsed.canvas.width, 2784.0432);
    assert_eq!(parsed.canvas.height, 5095.9844);
}

// ----------------------------------------------------------------
// Adversarial: malformed inputs refuse typed, never panic.

fn expect_inp_error(bytes: &[u8], fragment: &str) {
    let error = convert_inp(bytes, "Mini").expect_err("the input must be refused");
    let ConvertError::Inp { message } = &error else {
        panic!("expected an INP refusal, got {error}");
    };
    assert!(
        message.contains(fragment),
        "refusal should mention {fragment:?}: {message}"
    );
}

fn mutated_payload(mutate: impl FnOnce(&mut serde_json::Value)) -> Vec<u8> {
    let mut payload = mini_payload();
    mutate(&mut payload);
    let bytes = serde_json::to_vec(&payload).expect("payload serializes");
    container(&bytes, &[texture_slot()])
}

#[test]
fn bad_magic_is_refused() {
    let mut bytes = mini_inx();
    bytes[0] = b'X';
    expect_inp_error(&bytes, "bad magic");
}

#[test]
fn truncated_payload_is_refused() {
    let payload = serde_json::to_vec(&mini_payload()).expect("payload serializes");
    let mut out = Vec::new();
    out.extend_from_slice(b"TRNSRTS\0");
    out.extend_from_slice(&((payload.len() + 500) as u32).to_be_bytes());
    out.extend_from_slice(&payload);
    expect_inp_error(&out, "truncated container");
}

#[test]
fn texture_slot_overrun_is_refused() {
    let payload = serde_json::to_vec(&mini_payload()).expect("payload serializes");
    let mut out = Vec::new();
    out.extend_from_slice(b"TRNSRTS\0");
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(&payload);
    out.extend_from_slice(b"TEX_SECT");
    out.extend_from_slice(&1u32.to_be_bytes());
    out.extend_from_slice(&999_999u32.to_be_bytes());
    out.push(1);
    out.extend_from_slice(&texture_slot());
    expect_inp_error(&out, "truncated container");
}

#[test]
fn unknown_binding_target_is_refused() {
    let bytes = mutated_payload(|payload| {
        payload["param"][0]["bindings"][0]["node"] = serde_json::json!(999);
    });
    expect_inp_error(&bytes, "targets unknown node uuid 999");
}

#[test]
fn duplicate_node_uuid_is_refused() {
    let bytes = mutated_payload(|payload| {
        payload["nodes"]["children"][0]["uuid"] = serde_json::json!(1);
    });
    expect_inp_error(&bytes, "duplicate node uuid 1");
}

#[test]
fn unknown_node_type_is_refused() {
    let bytes = mutated_payload(|payload| {
        payload["nodes"]["children"][0]["type"] = serde_json::json!("Widget");
    });
    expect_inp_error(&bytes, "unknown type");
}

#[test]
fn unknown_blend_mode_is_refused() {
    let bytes = mutated_payload(|payload| {
        payload["nodes"]["children"][0]["blend_mode"] = serde_json::json!("Wacky");
    });
    expect_inp_error(&bytes, "has no native counterpart");
}

#[test]
fn deform_grid_shape_mismatch_is_refused() {
    let bytes = mutated_payload(|payload| {
        payload["param"][0]["axis_points"][0] = serde_json::json!([0.0, 1.0]);
        payload["param"][0]["bindings"][0]["isSet"] =
            serde_json::json!([[true, true], [true, true]]);
    });
    expect_inp_error(&bytes, "axis 0 has 2 points");
}

#[test]
fn deform_vertex_count_mismatch_is_refused() {
    let bytes = mutated_payload(|payload| {
        payload["param"][0]["bindings"][0]["values"][0][0] =
            serde_json::json!([[0.0, 0.0], [0.0, 0.0]]);
    });
    expect_inp_error(&bytes, "target mesh has 3 vertices");
}

#[test]
fn mesh_index_out_of_range_is_refused() {
    let bytes = mutated_payload(|payload| {
        payload["nodes"]["children"][0]["mesh"]["indices"] = serde_json::json!([0, 1, 7]);
    });
    expect_inp_error(&bytes, "not a valid triangle list");
}

#[test]
fn unknown_binding_kind_is_refused() {
    let bytes = mutated_payload(|payload| {
        payload["param"][1]["bindings"][0]["param_name"] = serde_json::json!("sparkle");
    });
    expect_inp_error(&bytes, "unknown binding kind");
}

#[test]
fn default_outside_range_is_refused() {
    let bytes = mutated_payload(|payload| {
        payload["param"][1]["defaults"] = serde_json::json!([2.0, 0.0]);
    });
    expect_inp_error(&bytes, "lies outside");
}

#[test]
fn non_finite_grid_value_is_refused() {
    // Written as raw payload text: JSON cannot carry NaN, but
    // an out-of-range exponent parses to a non-finite f64 and
    // must refuse rather than convert.
    let text = serde_json::to_string(&mini_payload()).expect("payload serializes");
    let text = text.replace("25.0", "1e999");
    let out = container(text.as_bytes(), &[texture_slot()]);
    let error = convert_inp(&out, "Mini").expect_err("the input must be refused");
    assert!(
        matches!(error, ConvertError::Inp { .. }),
        "expected an INP refusal, got {error}"
    );
}
