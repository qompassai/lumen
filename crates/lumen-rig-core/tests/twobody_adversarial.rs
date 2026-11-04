//! Two-body contact model: adversarial tests — bad scenes
//! are refused with named errors, unsatisfiable constraints
//! surface as data, and nothing panics.

use std::collections::BTreeMap;
use std::path::PathBuf;

use lumen_rig_core::twobody::{
    ContactDef, ContactEndpoint, FrameInput, Placement, ProbeDef, Scene, evaluate_frame,
};
use lumen_rig_core::{Model, parse_model};

fn fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../fixtures/twobody")
        .join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn mannequin(name: &str) -> Model {
    parse_model(&fixture(name)).expect("mannequin parses")
}

/// A minimal one-quad model whose single parameter id is
/// caller-chosen, for disjoint-parameter-set scenes.
fn tiny_model(id: &str, param: &str) -> Model {
    let doc = serde_json::json!({
        "format": "lumen-rig-model", "format_version": 1, "id": id,
        "parameters": [
            {"kind": "scalar", "id": param, "min": -10.0, "max": 10.0, "default": 0.0}
        ],
        "parts": [{"id": "root"}],
        "textures": [{"id": "tex", "source": "t.png"}],
        "deformers": [],
        "meshes": [{
            "id": "quad", "part": "root", "texture": "tex",
            "vertices": [[0.0, 0.0], [4.0, 0.0], [4.0, 4.0], [0.0, 4.0]],
            "uvs": [[0.0, 0.0], [0.0, 0.0], [0.0, 0.0], [0.0, 0.0]],
            "indices": [0, 1, 2, 0, 2, 3]
        }],
        "physics": []
    });
    parse_model(doc.to_string().as_bytes()).expect("tiny model parses")
}

fn vertex(mesh: &str, vertex: u32) -> ContactEndpoint {
    ContactEndpoint::Vertex {
        mesh: mesh.into(),
        vertex,
    }
}

fn contact(
    id: &str,
    a_rig: usize,
    a: ContactEndpoint,
    b_rig: usize,
    b: ContactEndpoint,
    driven_rig: usize,
    offset: [f64; 2],
) -> ContactDef {
    ContactDef {
        id: id.into(),
        a_rig,
        a,
        b_rig,
        b,
        driven_rig,
        offset,
    }
}

fn centroid(part: &str) -> ContactEndpoint {
    ContactEndpoint::PartCentroid { part: part.into() }
}

fn two_mannequins() -> Vec<Model> {
    vec![
        mannequin("mannequin_a.model.json"),
        mannequin("mannequin_b.model.json"),
    ]
}

fn idle_input(rigs: usize) -> FrameInput {
    FrameInput {
        step: 0,
        time_seconds: 0.0,
        driven: vec![BTreeMap::new(); rigs],
        placements: vec![Placement::IDENTITY; rigs],
        active: vec![],
    }
}

#[test]
fn missing_mesh_endpoint_is_refused() {
    let err = Scene::new(
        two_mannequins(),
        vec![contact(
            "c",
            0,
            vertex("no_such_mesh", 0),
            1,
            vertex("torso", 2),
            1,
            [0.0, 0.0],
        )],
        vec![],
        vec![centroid("pelvis"), centroid("pelvis")],
    )
    .expect_err("missing mesh must be refused");
    assert_eq!(err.code, "LTB-101", "{err}");
}

#[test]
fn vertex_out_of_range_is_refused() {
    let err = Scene::new(
        two_mannequins(),
        vec![contact(
            "c",
            0,
            vertex("torso", 999),
            1,
            vertex("torso", 2),
            1,
            [0.0, 0.0],
        )],
        vec![],
        vec![centroid("pelvis"), centroid("pelvis")],
    )
    .expect_err("bad vertex must be refused");
    assert_eq!(err.code, "LTB-101", "{err}");
}

#[test]
fn probe_with_missing_mesh_is_refused() {
    let err = Scene::new(
        two_mannequins(),
        vec![],
        vec![ProbeDef {
            id: "p".into(),
            rig: 0,
            mesh: "ghost".into(),
            vertex: 0,
            against_rig: 1,
        }],
        vec![centroid("pelvis"), centroid("pelvis")],
    )
    .expect_err("bad probe must be refused");
    assert_eq!(err.code, "LTB-101", "{err}");
}

#[test]
fn self_contact_is_refused() {
    let err = Scene::new(
        two_mannequins(),
        vec![contact(
            "c",
            0,
            vertex("torso", 2),
            0,
            vertex("torso", 5),
            0,
            [0.0, 0.0],
        )],
        vec![],
        vec![centroid("pelvis"), centroid("pelvis")],
    )
    .expect_err("self-contact must be refused");
    assert_eq!(err.code, "LTB-102", "{err}");
}

#[test]
fn driven_cycle_is_refused() {
    let err = Scene::new(
        two_mannequins(),
        vec![
            contact(
                "c1",
                0,
                vertex("torso", 2),
                1,
                vertex("torso", 2),
                1,
                [0.0, 0.0],
            ),
            contact(
                "c2",
                1,
                vertex("torso", 5),
                0,
                vertex("torso", 5),
                0,
                [0.0, 0.0],
            ),
        ],
        vec![],
        vec![centroid("pelvis"), centroid("pelvis")],
    )
    .expect_err("a driven cycle must be refused");
    assert_eq!(err.code, "LTB-103", "{err}");
}

#[test]
fn driven_rig_must_be_a_contact_party() {
    let models = vec![
        tiny_model("m0", "p0"),
        tiny_model("m1", "p1"),
        tiny_model("m2", "p2"),
    ];
    let err = Scene::new(
        models,
        vec![contact(
            "c",
            0,
            vertex("quad", 0),
            1,
            vertex("quad", 0),
            2,
            [0.0, 0.0],
        )],
        vec![],
        vec![centroid("root"), centroid("root"), centroid("root")],
    )
    .expect_err("third-party driven rig must be refused");
    assert_eq!(err.code, "LTB-101", "{err}");
}

#[test]
fn nan_driven_value_is_refused() {
    let scene = Scene::new(
        two_mannequins(),
        vec![contact(
            "c",
            0,
            vertex("arm_near_fore", 3),
            1,
            vertex("torso", 2),
            1,
            [0.0, 0.0],
        )],
        vec![],
        vec![centroid("pelvis"), centroid("pelvis")],
    )
    .expect("scene constructs");
    let mut input = idle_input(2);
    input.active = vec![true];
    input.driven[0].insert("spine_lean".into(), f64::NAN);
    let err = evaluate_frame(&scene, &input).expect_err("NaN driven value must be refused");
    assert_eq!(err.code, "LTB-104", "{err}");
}

#[test]
fn nan_placement_is_refused() {
    let scene = Scene::new(
        two_mannequins(),
        vec![],
        vec![],
        vec![centroid("pelvis"), centroid("pelvis")],
    )
    .expect("scene constructs");
    let mut input = idle_input(2);
    input.placements[1].rotation_degrees = f64::INFINITY;
    let err = evaluate_frame(&scene, &input).expect_err("non-finite placement must be refused");
    assert_eq!(err.code, "LTB-104", "{err}");
}

#[test]
fn unknown_driven_parameter_is_refused() {
    let scene = Scene::new(
        two_mannequins(),
        vec![],
        vec![],
        vec![centroid("pelvis"), centroid("pelvis")],
    )
    .expect("scene constructs");
    let mut input = idle_input(2);
    input.driven[1].insert("no_such_param".into(), 1.0);
    let err = evaluate_frame(&scene, &input).expect_err("unknown parameter must be refused");
    assert_eq!(err.code, "LTB-105", "{err}");
}

#[test]
fn contradictory_contacts_surface_as_data_not_panic() {
    // Two satisfiable grasp contacts plus a third demanding
    // B's head sit 80 units from A's head: the solver cap is
    // hit, the frame still evaluates, and the violation is
    // reported as data.
    let mut contacts = vec![
        contact(
            "c_near",
            0,
            vertex("arm_near_fore", 3),
            1,
            vertex("torso", 2),
            1,
            [0.0, -1.664_567_278],
        ),
        contact(
            "c_far",
            0,
            vertex("arm_far_fore", 3),
            1,
            vertex("torso", 5),
            1,
            [0.0, 0.063_570_144],
        ),
    ];
    contacts.push(contact(
        "c_impossible",
        0,
        centroid("head"),
        1,
        centroid("head"),
        1,
        [80.0, 0.0],
    ));
    let scene = Scene::new(
        two_mannequins(),
        contacts,
        vec![],
        vec![centroid("pelvis"), centroid("pelvis")],
    )
    .expect("scene constructs");
    let mut input = idle_input(2);
    input.placements[1] = Placement {
        translation: [54.778_421_414, 0.0],
        rotation_degrees: 0.0,
    };
    input.active = vec![true, true, true];
    let read = evaluate_frame(&scene, &input).expect("frame evaluates despite contradiction");
    assert!(!read.converged, "contradiction must not report convergence");
    let impossible = read
        .contacts
        .iter()
        .find(|c| c.id == "c_impossible")
        .expect("impossible contact is reported");
    assert!(
        impossible.slip > 1.0,
        "impossible contact slip {} reported as data",
        impossible.slip
    );
    for value in read.roots.iter().flatten() {
        assert!(value.is_finite(), "roots stay finite");
    }
}

#[test]
fn disjoint_parameter_sets_evaluate_cleanly() {
    let models = vec![
        tiny_model("left", "alpha_wave"),
        tiny_model("right", "beta_wave"),
    ];
    let scene = Scene::new(
        models,
        vec![contact(
            "c",
            0,
            vertex("quad", 1),
            1,
            vertex("quad", 0),
            1,
            [0.0, 0.0],
        )],
        vec![],
        vec![centroid("root"), centroid("root")],
    )
    .expect("scene constructs");
    let mut input = idle_input(2);
    input.placements[1] = Placement {
        translation: [10.0, 0.0],
        rotation_degrees: 0.0,
    };
    input.active = vec![true];
    input.driven[0].insert("alpha_wave".into(), 3.0);
    input.driven[1].insert("beta_wave".into(), -2.0);
    let read = evaluate_frame(&scene, &input).expect("frame evaluates");
    assert!(read.converged, "single contact solves exactly: {read:?}");
    assert!(read.contacts[0].slip <= 1e-6);
}
