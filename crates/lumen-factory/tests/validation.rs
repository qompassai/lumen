//! Validation tests: skeleton, mesh invariants, GLB round-trip.

use lumen_factory::base::{build_base_mesh, joint_table, validate_joints};
use lumen_factory::gltf::{MorphTarget, parse_glb, write_glb};

#[test]
fn joint_count_is_65() {
    assert_eq!(joint_table().len(), 65);
}

#[test]
fn joint_parents_exist_and_acyclic() {
    validate_joints(&joint_table()).expect("joint table must validate");
}

#[test]
fn joint_positions_finite_and_in_range() {
    for joint in joint_table() {
        assert!(
            joint.position.iter().all(|v| v.is_finite()),
            "{}",
            joint.name
        );
        assert!(
            joint.position[1] >= 0.0 && joint.position[1] <= 1.80,
            "{}",
            joint.name
        );
    }
}

#[test]
fn left_right_mirror_in_x() {
    let joints = joint_table();
    let get = |name: &str| {
        joints
            .iter()
            .find(|joint| joint.name == name)
            .unwrap()
            .position
    };
    for (left, right) in [
        ("LeftArm", "RightArm"),
        ("LeftUpLeg", "RightUpLeg"),
        ("LeftHandThumb1", "RightHandThumb1"),
    ] {
        let l = get(left);
        let r = get(right);
        assert!((l[0] + r[0]).abs() < 1e-6, "{left}/{right} x mirror");
        assert!((l[1] - r[1]).abs() < 1e-6);
        assert!((l[2] - r[2]).abs() < 1e-6);
    }
}

#[test]
fn mesh_parts_nonempty_and_indices_in_range() {
    let mesh = build_base_mesh().expect("base builds");
    assert_eq!(mesh.parts.len(), 9);
    for part in &mesh.parts {
        assert!(!part.vertices.is_empty(), "{}", part.name);
        assert!(!part.indices.is_empty(), "{}", part.name);
        assert_eq!(part.indices.len() % 3, 0, "{}", part.name);
        for &idx in &part.indices {
            assert!(
                (idx as usize) < part.vertices.len(),
                "{} index range",
                part.name
            );
        }
    }
}

#[test]
fn mesh_normals_are_unit_length() {
    let mesh = build_base_mesh().expect("base builds");
    for part in &mesh.parts {
        for vertex in &part.vertices {
            let len =
                (vertex.normal[0].powi(2) + vertex.normal[1].powi(2) + vertex.normal[2].powi(2))
                    .sqrt();
            assert!(
                (len - 1.0).abs() < 1e-3,
                "{} normal length {len}",
                part.name
            );
        }
    }
}

#[test]
fn mesh_weights_sum_to_one_and_at_most_four_influences() {
    let mesh = build_base_mesh().expect("base builds");
    for part in &mesh.parts {
        for vertex in &part.vertices {
            let sum: f32 = vertex.weights.iter().sum();
            assert!((sum - 1.0).abs() <= 1e-3, "{} weight sum {sum}", part.name);
            let influences = vertex.weights.iter().filter(|w| **w > 0.0).count();
            assert!((1..=4).contains(&influences), "{} influences", part.name);
            assert!(vertex.weights.iter().all(|w| w.is_finite() && *w >= 0.0));
        }
    }
}

#[test]
fn mesh_uvs_inside_atlas_and_face_reserved() {
    let mesh = build_base_mesh().expect("base builds");
    for part in &mesh.parts {
        for vertex in &part.vertices {
            assert!((0.0..=1.0).contains(&vertex.uv[0]), "{} u", part.name);
            assert!((0.0..=1.0).contains(&vertex.uv[1]), "{} v", part.name);
            if part.name == "head" {
                assert!(
                    vertex.uv[0] <= 0.25 && vertex.uv[1] <= 0.25,
                    "head uv in face rect"
                );
            }
        }
    }
}

#[test]
fn head_masks_aligned_and_bounded() {
    let mesh = build_base_mesh().expect("base builds");
    let head = mesh.parts.iter().find(|p| p.name == "head").unwrap();
    assert_eq!(mesh.head_masks.len(), head.vertices.len());
    for mask in &mesh.head_masks {
        for value in [
            mask.brow,
            mask.cheek,
            mask.eye_corner,
            mask.jaw,
            mask.lips,
            mask.lower_lid,
            mask.mouth,
            mask.upper_lid,
        ] {
            assert!((0.0..=1.0).contains(&value), "mask value {value}");
        }
    }
    // At least some vertices must carry meaningful mouth / brow signal.
    assert!(mesh.head_masks.iter().any(|m| m.mouth > 0.5));
    assert!(mesh.head_masks.iter().any(|m| m.brow > 0.5));
}

#[test]
fn glb_round_trip_counts_match() {
    let mesh = build_base_mesh().expect("base builds");
    let bytes = write_glb(&mesh, &[]).expect("write glb");
    let summary = parse_glb(&bytes).expect("parse glb");
    let vertices: usize = mesh.parts.iter().map(|p| p.vertices.len()).sum();
    let indices: usize = mesh.parts.iter().map(|p| p.indices.len()).sum();
    assert_eq!(summary.joint_count, 65);
    assert_eq!(summary.primitive_count, mesh.parts.len());
    assert_eq!(summary.mesh_count, 1);
    assert_eq!(summary.node_count, 65 + 2);
    assert_eq!(summary.vertex_count, vertices);
    assert_eq!(summary.index_count, indices);
    assert!(
        (summary.weight_sum_min - 1.0).abs() <= 1e-3,
        "min {}",
        summary.weight_sum_min
    );
    assert!(
        (summary.weight_sum_max - 1.0).abs() <= 1e-3,
        "max {}",
        summary.weight_sum_max
    );
}

#[test]
fn glb_determinism_byte_identical() {
    let a = write_glb(&build_base_mesh().unwrap(), &[]).unwrap();
    let b = write_glb(&build_base_mesh().unwrap(), &[]).unwrap();
    assert_eq!(a, b);
    assert_eq!(lumen_factory::sha256_hex(&a), lumen_factory::sha256_hex(&b));
}

#[test]
fn glb_morph_target_round_trip() {
    let mesh = build_base_mesh().expect("base builds");
    let head_len = mesh.head_masks.len();
    let target = MorphTarget {
        deltas: vec![[0.0, 0.001, 0.0]; head_len],
        name: "test_smile".to_string(),
    };
    let bytes = write_glb(&mesh, &[target]).expect("write with target");
    let summary = parse_glb(&bytes).expect("parse with target");
    assert_eq!(summary.target_names, vec!["test_smile".to_string()]);
}
