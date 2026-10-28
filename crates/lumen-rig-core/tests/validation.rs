//! Validation tests: well-formed models parse, defaults
//! resolve as SPEC.md section 3 names them, and canonical
//! emission round-trips to an identical model.

use std::fs;
use std::path::Path;

use lumen_rig_core::{BlendMode, Deformer, Model, Parameter, parse_model};

fn fixture(relative: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("fixtures")
        .join(relative);
    fs::read(&path).unwrap_or_else(|error| panic!("fixture {relative}: {error}"))
}

fn parse_fixture(relative: &str) -> Model {
    parse_model(&fixture(relative))
        .unwrap_or_else(|error| panic!("fixture {relative} should parse: {error}"))
}

#[test]
fn minimal_model_parses() {
    let model = parse_fixture("minimal.model.json");
    assert_eq!(model.id, "minimal");
    assert_eq!(model.parameters.len(), 1);
    let Parameter::Scalar(param) = &model.parameters[0] else {
        panic!("parameter 0 should be scalar");
    };
    assert_eq!(param.id, "angle_x");
    assert_eq!((param.min, param.max, param.default), (-30.0, 30.0, 0.0));
    assert_eq!(model.parts.len(), 1);
    assert_eq!(model.parts[0].opacity, 1.0);
    assert_eq!(model.textures.len(), 1);
    assert_eq!(model.meshes.len(), 1);
    assert_eq!(model.meshes[0].indices, vec![0, 1, 2]);
    assert!(model.deformers.is_empty());
    assert!(model.physics.is_empty());
}

#[test]
fn omitted_mesh_fields_resolve_to_spec_defaults() {
    let model = parse_fixture("minimal.model.json");
    let mesh = &model.meshes[0];
    assert_eq!(mesh.opacity, 1.0);
    assert_eq!(mesh.blend_mode, BlendMode::Normal);
    assert_eq!(mesh.draw_order, 0);
    assert!(mesh.rendered);
    assert!(mesh.clipping_mask_ids.is_empty());
    assert!(mesh.deformer.is_none());
    assert!(mesh.keyforms.is_none());
}

#[test]
fn full_model_parses_all_collections() {
    let model = parse_fixture("full.model.json");
    assert_eq!(model.id, "full");
    assert_eq!(model.parameters.len(), 5);
    assert_eq!(model.parts.len(), 3);
    assert_eq!(model.textures.len(), 2);
    assert_eq!(model.deformers.len(), 2);
    assert_eq!(model.meshes.len(), 4);
    assert_eq!(model.physics.len(), 1);

    let Parameter::Combined(param) = &model.parameters[4] else {
        panic!("parameter 4 should be combined");
    };
    assert_eq!(param.id, "gaze");
    assert_eq!(param.terms.len(), 2);
    assert_eq!(param.terms[0].parameter, "angle_x");
    assert_eq!(param.terms[0].weight, 0.5);

    let hair = model.parts.iter().find(|part| part.id == "hair");
    assert_eq!(hair.map(|part| part.opacity), Some(0.9));
    assert_eq!(hair.and_then(|part| part.parent.as_deref()), Some("root"));
}

#[test]
fn full_model_deformers_and_lattices_parse() {
    let model = parse_fixture("full.model.json");
    let Deformer::Rotation(head) = &model.deformers[0] else {
        panic!("deformer 0 should be rotation");
    };
    assert_eq!(head.id, "head_turn");
    assert_eq!(head.origin, [0.0, 100.0]);
    let head_lattice = head.keyforms.as_ref().expect("head lattice");
    assert_eq!(head_lattice.axes, vec!["angle_x".to_string()]);
    assert_eq!(head_lattice.forms.len(), 3);
    assert_eq!(head_lattice.forms[0].angle_degrees, -30.0);

    let Deformer::Warp(hair) = &model.deformers[1] else {
        panic!("deformer 1 should be warp");
    };
    assert_eq!(hair.parent.as_deref(), Some("head_turn"));
    assert_eq!((hair.columns, hair.rows), (2, 1));
    assert_eq!(hair.rest_positions.len(), 6);
    let warp_lattice = hair.keyforms.as_ref().expect("warp lattice");
    assert_eq!(warp_lattice.axes.len(), 2);
    assert_eq!(warp_lattice.forms.len(), 6);
    assert_eq!(warp_lattice.forms[5].at, vec![2, 1]);
}

#[test]
fn full_model_meshes_carry_masks_blends_and_order() {
    let model = parse_fixture("full.model.json");
    let by_id = |id: &str| model.meshes.iter().find(|mesh| mesh.id == id);
    let mask = by_id("mask_mesh").expect("mask mesh");
    assert!(!mask.rendered);
    let mouth = by_id("mouth_mesh").expect("mouth mesh");
    assert_eq!(mouth.clipping_mask_ids, vec!["mask_mesh".to_string()]);
    let hair = by_id("hair_mesh").expect("hair mesh");
    assert_eq!(hair.blend_mode, BlendMode::Multiply);
    assert_eq!(hair.draw_order, 20);
    assert_eq!(
        hair.keyforms.as_ref().map(|lattice| lattice.forms.len()),
        Some(4)
    );

    // SPEC 3.5: draw order is (draw_order, id) ascending.
    let mut keyed: Vec<(i32, &str)> = model
        .meshes
        .iter()
        .map(|mesh| (mesh.draw_order, mesh.id.as_str()))
        .collect();
    keyed.sort();
    let sorted: Vec<&str> = keyed.iter().map(|entry| entry.1).collect();
    assert_eq!(
        sorted,
        vec!["mask_mesh", "face_mesh", "mouth_mesh", "hair_mesh"]
    );
}

#[test]
fn full_model_physics_defaults_resolve() {
    let model = parse_fixture("full.model.json");
    let group = &model.physics[0];
    assert_eq!(group.gravity, [0.0, -9.8]);
    assert_eq!(group.wind, [0.0, 0.0]);
    assert_eq!(group.turbulence_amplitude, 0.0);
    assert_eq!(group.inputs.len(), 1);
    assert_eq!(group.inputs[0].scale_degrees, 12.0);
    let strand = &group.strands[0];
    assert_eq!(strand.particles.len(), 2);
    let output = strand.output.as_ref().expect("strand output");
    assert_eq!(output.parameter, "sway");
}

#[test]
fn round_trip_minimal_is_identical() {
    let model = parse_fixture("minimal.model.json");
    let emitted = model.to_canonical_json();
    let reparsed = parse_model(emitted.as_bytes()).expect("emitted model re-parses");
    assert_eq!(model, reparsed);
    assert_eq!(emitted, reparsed.to_canonical_json());
}

#[test]
fn round_trip_full_is_identical() {
    let model = parse_fixture("full.model.json");
    let emitted = model.to_canonical_json();
    let reparsed = parse_model(emitted.as_bytes()).expect("emitted model re-parses");
    assert_eq!(model, reparsed);
    assert_eq!(emitted, reparsed.to_canonical_json());
}

#[test]
fn canonical_emission_orders_header_first() {
    let model = parse_fixture("minimal.model.json");
    let emitted = model.to_canonical_json();
    let prefix =
        "{\n  \"format\": \"lumen-rig-model\",\n  \"format_version\": 1,\n  \"id\": \"minimal\"";
    assert!(
        emitted.starts_with(prefix),
        "emission should start with the header fields: {emitted}"
    );
    assert!(emitted.ends_with("}\n"));
    // Resolved defaults are emitted, not omitted (SPEC 1.2).
    assert!(emitted.contains("\"blend_mode\": \"normal\""));
    assert!(emitted.contains("\"opacity\": 1.0"));
}

#[test]
fn all_eighteen_blend_modes_parse() {
    // SPEC 3.5: the blend set is exactly these 18 strings.
    let modes = [
        "normal",
        "multiply",
        "screen",
        "overlay",
        "darken",
        "lighten",
        "color_dodge",
        "color_burn",
        "hard_light",
        "soft_light",
        "difference",
        "exclusion",
        "hue",
        "saturation",
        "color",
        "luminosity",
        "linear_dodge",
        "linear_burn",
    ];
    for mode in modes {
        let text = format!(
            "{{\"format\":\"lumen-rig-model\",\"format_version\":1,\"id\":\"m\",\
             \"parameters\":[],\"parts\":[{{\"id\":\"root\"}}],\
             \"textures\":[{{\"id\":\"t\",\"source\":\"t.png\"}}],\"deformers\":[],\
             \"meshes\":[{{\"id\":\"mesh\",\"part\":\"root\",\"texture\":\"t\",\
             \"vertices\":[[0.0,0.0],[1.0,0.0],[0.0,1.0]],\
             \"uvs\":[[0.0,0.0],[1.0,0.0],[0.0,1.0]],\"indices\":[0,1,2],\
             \"blend_mode\":\"{mode}\"}}],\"physics\":[]}}"
        );
        let model = parse_model(text.as_bytes())
            .unwrap_or_else(|error| panic!("blend mode {mode} should parse: {error}"));
        assert_eq!(
            model.meshes[0].blend_mode,
            BlendMode::from_format_str(mode).expect("mode is in the set")
        );
    }
}

#[test]
fn combined_parameter_chain_parses() {
    // A combined parameter may reference another combined
    // parameter; containment is proven through the chain.
    let text = concat!(
        "{\"format\":\"lumen-rig-model\",\"format_version\":1,\"id\":\"chain\",",
        "\"parameters\":[",
        "{\"kind\":\"scalar\",\"id\":\"a\",\"min\":0.0,\"max\":10.0,\"default\":0.0},",
        "{\"kind\":\"combined\",\"id\":\"b\",\"min\":0.0,\"max\":20.0,\"offset\":0.0,",
        "\"terms\":[{\"parameter\":\"a\",\"weight\":2.0}]},",
        "{\"kind\":\"combined\",\"id\":\"c\",\"min\":0.0,\"max\":11.0,\"offset\":1.0,",
        "\"terms\":[{\"parameter\":\"b\",\"weight\":0.5}]}],",
        "\"parts\":[],\"textures\":[],\"deformers\":[],\"meshes\":[],\"physics\":[]}"
    );
    let model = parse_model(text.as_bytes()).expect("combined chain should parse");
    assert_eq!(model.parameters.len(), 3);
    assert_eq!(model.parameters[2].id(), "c");
}

#[test]
fn version_error_precedes_unknown_field() {
    // SPEC 8: the header probe (step 3) runs before the
    // schema parse (step 5), so a wrong version wins over an
    // unknown field present in the same document.
    let bytes = fixture("adversarial/bad-version.json");
    let text = String::from_utf8(bytes).expect("fixture is utf-8");
    let text = text.replace("\"id\": \"base\"", "\"id\": \"base\", \"surprise\": true");
    let error = parse_model(text.as_bytes()).expect_err("must fail");
    assert_eq!(error.code(), "LRF-106");
}
