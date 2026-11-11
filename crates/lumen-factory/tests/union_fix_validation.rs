//! Union-fix validation (D-1290): the union skeleton's two
//! geometry shortfalls stay fixed — eyeballs are rigid geometry
//! on the eye joints, and every articulation joint carries skin
//! weight because the hands are articulated, not mittens.

use lumen_factory::base::{build_base_mesh, joint_index, skin_joint_indices};

fn part<'m>(
    mesh: &'m lumen_factory::base::BaseMesh,
    name: &str,
) -> &'m lumen_factory::base::MeshPart {
    mesh.parts
        .iter()
        .find(|p| p.name == name)
        .unwrap_or_else(|| panic!("part {name} missing"))
}

#[test]
fn eyeballs_are_rigid_on_the_eye_joints() {
    let mesh = build_base_mesh().expect("base builds");
    for (part_name, joint_name) in [("eye_left", "LeftEye"), ("eye_right", "RightEye")] {
        let eye = part(&mesh, part_name);
        // The cast's count (prima.glb census): 396 verts per eye.
        assert_eq!(eye.vertices.len(), 396, "{part_name}");
        let eye_idx = joint_index(&mesh.joints, joint_name).expect("eye joint") as u16;
        let centre = mesh.joints[eye_idx as usize].position;
        for vertex in &eye.vertices {
            // Exactly one influence: the eye joint at weight 1.
            assert_eq!(vertex.joints[0], eye_idx, "{part_name}");
            assert_eq!(vertex.weights, [1.0, 0.0, 0.0, 0.0], "{part_name}");
            // On the eyeball sphere around the joint (the pivot).
            let d = ((vertex.position[0] - centre[0]).powi(2)
                + (vertex.position[1] - centre[1]).powi(2)
                + (vertex.position[2] - centre[2]).powi(2))
            .sqrt();
            assert!((d - 0.0172).abs() < 0.0002, "{part_name} radius {d}");
        }
    }
}

#[test]
fn skull_carries_no_eye_weight() {
    let mesh = build_base_mesh().expect("base builds");
    let eyes: Vec<u16> = ["LeftEye", "RightEye"]
        .iter()
        .map(|name| joint_index(&mesh.joints, name).expect("eye joint") as u16)
        .collect();
    for part_name in ["head", "hair"] {
        for vertex in &part(&mesh, part_name).vertices {
            for slot in 0..4 {
                assert!(
                    !(eyes.contains(&vertex.joints[slot]) && vertex.weights[slot] > 0.0),
                    "{part_name} vertex follows gaze"
                );
            }
        }
    }
}

#[test]
fn every_articulation_joint_carries_weight() {
    let mesh = build_base_mesh().expect("base builds");
    let skin = skin_joint_indices(&mesh.joints);
    assert_eq!(skin.len(), 54);
    let mut weighted = vec![false; mesh.joints.len()];
    for part in &mesh.parts {
        for vertex in &part.vertices {
            for slot in 0..4 {
                if vertex.weights[slot] > 0.0 {
                    weighted[vertex.joints[slot] as usize] = true;
                }
            }
        }
    }
    for &idx in &skin {
        assert!(
            weighted[idx],
            "joint {} carries no weight",
            mesh.joints[idx].name
        );
    }
}

#[test]
fn distal_finger_joints_own_their_segments() {
    let mesh = build_base_mesh().expect("base builds");
    // The 14 joints the mitten left unweighted (D-1290 census):
    // each must own at least one vertex outright (weight 1).
    for name in [
        "LeftHandThumb2",
        "LeftHandThumb3",
        "LeftHandIndex3",
        "LeftHandMiddle3",
        "LeftHandRing3",
        "LeftHandPinky2",
        "LeftHandPinky3",
        "RightHandThumb2",
        "RightHandThumb3",
        "RightHandIndex3",
        "RightHandMiddle3",
        "RightHandRing3",
        "RightHandPinky2",
        "RightHandPinky3",
    ] {
        let idx = joint_index(&mesh.joints, name).expect("joint") as u16;
        let owned = mesh
            .parts
            .iter()
            .flat_map(|p| &p.vertices)
            .filter(|v| v.joints[0] == idx && v.weights[0] == 1.0)
            .count();
        assert!(owned > 0, "{name} owns no vertex outright");
    }
}
