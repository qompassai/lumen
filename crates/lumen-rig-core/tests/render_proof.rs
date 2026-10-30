//! RENDER-1 proof: the landed Prima rig, augmented with
//! the six BIND-1 part-translation lattices the torogao
//! bridge dropped (BrowL/R t.y, HeadAnchor t.x/t.y,
//! BodyRoot t.x/t.y — the B0 census list), rendered at rest
//! and at the F-03 state vector.
//!
//! The augmentation is test-side on purpose: the landed
//! fixture predates BIND-1, and this suite is the
//! rig-core half of the round trip (INP bindings -> lumen
//! model -> evaluation moves the channel -> pixels move).
//! The bridge half lives in torogao's rigexport, which now
//! emits exactly these lattices. Translation gains are the
//! test's own (±3 model units across each channel's range,
//! zero at its default) — the census established that the
//! bindings exist and were dropped, not their magnitudes,
//! so the proof claims movement, not INP magnitudes.
//!
//! Textures are synthetic (the fixture's PNG files are not
//! part of the repo): a fine checkerboard per slot, so any
//! geometric movement shows up as pixel change.

use std::collections::HashMap;

use lumen_rig_core::{
    FrameMapping, ParameterValues, RgbaImage, encode_png, evaluate_pose, parse_model, render_frame,
};

const CANVAS: u32 = 512;

/// The six dropped bindings, as (part id, parameter id,
/// model-space direction per unit of parameter travel).
/// Directions follow the bridge's rest-frame conversion of
/// INP local deltas (delta_to_world: y flips).
const DROPPED_BINDINGS: [(&str, &str, [f64; 2]); 6] = [
    ("HeadAnchor_1020", "ParamAngleX_2001_x", [1.0, 0.0]),
    ("HeadAnchor_1020", "ParamAngleY_2002_x", [0.0, -1.0]),
    ("BodyRoot_1010", "ParamBodyAngleX_2004_x", [1.0, 0.0]),
    ("BodyRoot_1010", "ParamBodyAngleY_2005_x", [0.0, -1.0]),
    ("BrowL_1024", "ParamBrowLY_2012_x", [0.0, -1.0]),
    ("BrowR_1025", "ParamBrowRY_2013_x", [0.0, -1.0]),
];

/// Prima with the six BIND-1 lattices injected. Each
/// lattice has keys at the parameter's [min, max] and
/// absolute translations `direction * (key - default) *
/// gain`, with the gain set so the full-range travel is 3
/// model units — zero at the default by construction.
fn augmented_prima() -> lumen_rig_core::Model {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("fixtures")
        .join("conformance")
        .join("prima-repertoire.model.json");
    let bytes = std::fs::read(&path).expect("Prima fixture readable");
    let mut document: serde_json::Value =
        serde_json::from_slice(&bytes).expect("Prima fixture is JSON");
    let parameters: HashMap<String, (f64, f64, f64)> = document["parameters"]
        .as_array()
        .expect("parameters array")
        .iter()
        .map(|param| {
            (
                param["id"].as_str().expect("parameter id").to_string(),
                (
                    param["min"].as_f64().expect("min"),
                    param["max"].as_f64().expect("max"),
                    param["default"].as_f64().expect("default"),
                ),
            )
        })
        .collect();
    for (part_id, parameter_id, direction) in DROPPED_BINDINGS {
        let (min, max, default) = parameters[parameter_id];
        let gain = 3.0 / (max - default).max(default - min);
        let forms: Vec<serde_json::Value> = [min, max]
            .iter()
            .enumerate()
            .map(|(at, key)| {
                let travel = (key - default) * gain;
                serde_json::json!({
                    "at": [at],
                    "translation": [direction[0] * travel, direction[1] * travel],
                })
            })
            .collect();
        let lattice = serde_json::json!({
            "axes": [parameter_id],
            "keys": [[min, max]],
            "forms": forms,
        });
        let parts = document["parts"].as_array_mut().expect("parts array");
        let part = parts
            .iter_mut()
            .find(|part| part["id"].as_str() == Some(part_id))
            .unwrap_or_else(|| panic!("part {part_id} present"));
        match part
            .as_object_mut()
            .expect("part object")
            .entry("translation_keyforms")
        {
            serde_json::map::Entry::Vacant(entry) => {
                entry.insert(lattice);
            }
            serde_json::map::Entry::Occupied(mut entry) => {
                // Two bindings share a part (HeadAnchor,
                // BodyRoot): a part carries ONE lattice, so
                // merge onto a 2-axis lattice over both
                // parameters. The existing entry is axis 1;
                // rebuild as the cross product.
                let existing = entry.get().clone();
                let (min1, max1, default1) =
                    parameters[existing["axes"][0].as_str().expect("existing axis")];
                let gain1 = 3.0 / (max1 - default1).max(default1 - min1);
                let dir1 = DROPPED_BINDINGS
                    .iter()
                    .find(|(_, id, _)| Some(*id) == existing["axes"][0].as_str())
                    .expect("existing binding known")
                    .2;
                let mut forms = Vec::new();
                for (i, key1) in [min1, max1].iter().enumerate() {
                    for (j, key2) in [min, max].iter().enumerate() {
                        let t1 = (key1 - default1) * gain1;
                        let t2 = (key2 - default) * gain;
                        forms.push(serde_json::json!({
                            "at": [i, j],
                            "translation": [
                                dir1[0] * t1 + direction[0] * t2,
                                dir1[1] * t1 + direction[1] * t2,
                            ],
                        }));
                    }
                }
                entry.insert(serde_json::json!({
                    "axes": [existing["axes"][0].clone(), parameter_id],
                    "keys": [[min1, max1], [min, max]],
                    "forms": forms,
                }));
            }
        }
    }
    let text = serde_json::to_string(&document).expect("augmented document serialises");
    parse_model(text.as_bytes()).expect("augmented Prima parses under BIND-1")
}

/// One synthetic texture: a 64x64 checkerboard of 4px cells
/// in a per-slot colour pair, fully opaque.
fn synthetic_texture(slot: u8) -> RgbaImage {
    let (width, height) = (64_u32, 64_u32);
    let dark = [40 + slot * 30, 60, 90];
    let light = [200, 190 - slot * 20, 120 + slot * 10];
    let mut pixels = Vec::with_capacity((width * height) as usize);
    for y in 0..height {
        for x in 0..width {
            let cell = ((x / 4) + (y / 4)) % 2 == 0;
            let colour = if cell { light } else { dark };
            pixels.push([colour[0], colour[1], colour[2], 255]);
        }
    }
    RgbaImage {
        width,
        height,
        pixels,
    }
}

fn textures() -> HashMap<String, RgbaImage> {
    (0..3)
        .map(|slot| (format!("tex_{slot}"), synthetic_texture(slot)))
        .collect()
}

/// The F-03 state vector (operationalised from
/// FACE-STATES.md's F-03 "Iki-kake, even": brows lowered +
/// knit, lids 0.35–0.50, mouth parted gasp, head engaged):
/// brow channels at their lowered extreme, eye-open at
/// 0.42, mouth open at 0.6, head angles off rest.
fn f03_values(model: &lumen_rig_core::Model) -> ParameterValues {
    let mut values = ParameterValues::rest(model);
    for (id, value) in [
        ("ParamBrowLY_2012_x", 1.0),
        ("ParamBrowRY_2013_x", 1.0),
        ("ParamEyeLOpen_2008_x", 0.42),
        ("ParamEyeROpen_2009_x", 0.42),
        ("ParamMouthOpenY_2014_x", 0.6),
        ("ParamAngleX_2001_x", 6.0),
        ("ParamAngleY_2002_x", -8.0),
        ("ParamAngleZ_2003_x", 4.0),
        ("ParamBodyAngleY_2005_x", 5.0),
    ] {
        assert!(values.set(model, id, value), "F-03 write {id}");
    }
    values
}

/// One fixed camera for every frame in this suite: the
/// mapping is fit at the rest pose and held. (Fitting per
/// frame would normalise whole-model translations — the
/// BodyRoot channels — out of the picture entirely; a
/// batch renderer shoots a sequence through one camera.)
fn rest_mapping(model: &lumen_rig_core::Model) -> FrameMapping {
    let meshes = evaluate_pose(model, &ParameterValues::rest(model));
    FrameMapping::fit(&meshes, CANVAS, CANVAS, 0.02).expect("frame mapping")
}

fn render(model: &lumen_rig_core::Model, values: &ParameterValues) -> RgbaImage {
    let meshes = evaluate_pose(model, values);
    render_frame(&meshes, &textures(), &rest_mapping(model)).expect("frame renders")
}

fn changed_pixels(a: &RgbaImage, b: &RgbaImage) -> Vec<(u32, u32)> {
    let mut changed = Vec::new();
    for y in 0..a.height {
        for x in 0..a.width {
            if a.texel(x, y) != b.texel(x, y) {
                changed.push((x, y));
            }
        }
    }
    changed
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
    }
    hash
}

#[test]
fn bind1_channels_move_geometry_by_the_lattice_amount() {
    // Geometry level: with ParamBrowLY at its max, the left
    // brow mesh sits exactly the lattice's max-form
    // translation below rest (0, -3); at rest it is
    // unmoved. This is the BIND-1 round trip at the
    // evaluator: bindings -> model -> evaluation moves the
    // channel.
    let model = augmented_prima();
    let rest = evaluate_pose(&model, &ParameterValues::rest(&model));
    let mut raised = ParameterValues::rest(&model);
    assert!(raised.set(&model, "ParamBrowLY_2012_x", 1.0));
    let moved = evaluate_pose(&model, &raised);
    let brow_rest = rest
        .iter()
        .find(|mesh| mesh.id == "mesh_BrowL_1024")
        .expect("brow mesh");
    let brow_moved = moved
        .iter()
        .find(|mesh| mesh.id == "mesh_BrowL_1024")
        .expect("brow mesh");
    for (before, after) in brow_rest.vertices.iter().zip(brow_moved.vertices.iter()) {
        assert!(
            (after[0] - before[0]).abs() <= 1e-4,
            "x holds: {before:?} -> {after:?}"
        );
        assert!(
            (after[1] - before[1] + 3.0).abs() <= 1e-4,
            "y drops by the lattice amount: {before:?} -> {after:?}"
        );
    }
    // And the right brow did not move on the left channel.
    let right_rest = rest
        .iter()
        .find(|mesh| mesh.id == "mesh_BrowR_1025")
        .expect("right brow");
    let right_moved = moved
        .iter()
        .find(|mesh| mesh.id == "mesh_BrowR_1025")
        .expect("right brow");
    assert_eq!(right_rest.vertices, right_moved.vertices);
}

#[test]
fn f03_frame_differs_from_rest_and_brow_channel_moves_brow_pixels() {
    let model = augmented_prima();
    let rest_values = ParameterValues::rest(&model);
    let rest = render(&model, &rest_values);
    let f03 = render(&model, &f03_values(&model));
    let all_changed = changed_pixels(&rest, &f03);
    assert!(
        all_changed.len() > 500,
        "F-03 frame must differ materially from rest: {} changed pixels",
        all_changed.len()
    );

    // Brow-only move: everything else at rest.
    let mut brow_values = ParameterValues::rest(&model);
    assert!(brow_values.set(&model, "ParamBrowLY_2012_x", 1.0));
    assert!(brow_values.set(&model, "ParamBrowRY_2013_x", 1.0));
    let brow_frame = render(&model, &brow_values);
    let brow_changed = changed_pixels(&rest, &brow_frame);
    assert!(
        !brow_changed.is_empty(),
        "the brow channels — inert before BIND-1 — now move pixels"
    );

    // Localisation: every changed pixel lies in the brow
    // region — the union of the two brow meshes' evaluated
    // bounding boxes at rest and at the brow state, mapped
    // to pixels and padded by 2.
    let rest_meshes = evaluate_pose(&model, &rest_values);
    let brow_meshes = evaluate_pose(&model, &brow_values);
    let mapping = FrameMapping::fit(&rest_meshes, CANVAS, CANVAS, 0.02).expect("mapping");
    let (mut min_x, mut min_y) = (f64::INFINITY, f64::INFINITY);
    let (mut max_x, mut max_y) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
    for meshes in [&rest_meshes, &brow_meshes] {
        for mesh in meshes
            .iter()
            .filter(|mesh| mesh.id == "mesh_BrowL_1024" || mesh.id == "mesh_BrowR_1025")
        {
            for vertex in &mesh.vertices {
                let pixel = mapping.model_to_pixel(*vertex);
                min_x = min_x.min(pixel[0]);
                max_x = max_x.max(pixel[0]);
                min_y = min_y.min(pixel[1]);
                max_y = max_y.max(pixel[1]);
            }
        }
    }
    let outside = brow_changed
        .iter()
        .filter(|(x, y)| {
            f64::from(*x) < min_x - 2.0
                || f64::from(*x) > max_x + 2.0
                || f64::from(*y) < min_y - 2.0
                || f64::from(*y) > max_y + 2.0
        })
        .count();
    assert_eq!(
        outside,
        0,
        "brow-channel changes stay inside the brow region ({} of {} escaped)",
        outside,
        brow_changed.len()
    );
    eprintln!(
        "render proof @512: rest-vs-F03 {} px changed; brow-only {} px changed, 0 outside the brow region",
        all_changed.len(),
        brow_changed.len()
    );
}

#[test]
fn previously_inert_head_and_body_channels_move_pixels() {
    // ParamAngleY, ParamBodyAngleX, ParamBodyAngleY were
    // deformation-inert in the landed export (B0 census);
    // each alone must now move pixels.
    let model = augmented_prima();
    let rest = render(&model, &ParameterValues::rest(&model));
    for (id, value) in [
        ("ParamAngleY_2002_x", -10.0),
        ("ParamBodyAngleX_2004_x", 10.0),
        ("ParamBodyAngleY_2005_x", 10.0),
        ("ParamAngleX_2001_x", 10.0),
    ] {
        let mut values = ParameterValues::rest(&model);
        assert!(values.set(&model, id, value));
        let frame = render(&model, &values);
        let changed = changed_pixels(&rest, &frame).len();
        assert!(changed > 0, "{id} moves pixels (got {changed})");
        eprintln!("render proof @512: {id} alone moves {changed} px");
    }
}

#[test]
fn raster_output_is_deterministic() {
    // Same input -> same PNG hash, across repeated renders
    // and across the two states.
    let model = augmented_prima();
    let rest_a = render(&model, &ParameterValues::rest(&model));
    let rest_b = render(&model, &ParameterValues::rest(&model));
    assert_eq!(fnv1a(&encode_png(&rest_a)), fnv1a(&encode_png(&rest_b)));
    let f03_a = render(&model, &f03_values(&model));
    let f03_b = render(&model, &f03_values(&model));
    assert_eq!(fnv1a(&encode_png(&f03_a)), fnv1a(&encode_png(&f03_b)));
    assert_ne!(fnv1a(&encode_png(&rest_a)), fnv1a(&encode_png(&f03_a)));
}
