//! Adversarial tests: hostile inputs must fail closed, never panic.

use lumen_factory::base::build_base_mesh;
use lumen_factory::base::build_base_mesh_with_profile;
use lumen_factory::gltf::{MorphTarget, parse_glb, write_glb};

#[test]
fn reader_rejects_empty_and_short_inputs() {
    assert!(parse_glb(&[]).is_err());
    assert!(parse_glb(&[0u8; 4]).is_err());
    assert!(parse_glb(b"glTF").is_err());
}

#[test]
fn reader_rejects_bad_magic() {
    let bytes = write_glb(&build_base_mesh().unwrap(), &[]).unwrap();
    let mut bad = bytes.clone();
    bad[0] ^= 0xFF;
    assert!(parse_glb(&bad).is_err());
}

#[test]
fn reader_rejects_every_truncation_without_panic() {
    let bytes = write_glb(&build_base_mesh().unwrap(), &[]).unwrap();
    for cut in [1usize, 8, 12, 20, 40, bytes.len() / 2, bytes.len() - 1] {
        assert!(parse_glb(&bytes[..cut]).is_err(), "cut {cut} must fail");
    }
}

#[test]
fn reader_rejects_oversized_declared_length() {
    let bytes = write_glb(&build_base_mesh().unwrap(), &[]).unwrap();
    let mut bad = bytes.clone();
    bad[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(parse_glb(&bad).is_err());
    let mut bad2 = bytes.clone();
    bad2[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
    assert!(parse_glb(&bad2).is_err());
}

#[test]
fn reader_rejects_random_bytes() {
    // Deterministic pseudo-random garbage (no rng dependency).
    let mut state = 0x1234_5678u32;
    let mut garbage = vec![0u8; 512];
    for byte in &mut garbage {
        state = state.wrapping_mul(1664525).wrapping_add(1013904223);
        *byte = (state >> 24) as u8;
    }
    assert!(parse_glb(&garbage).is_err());
}

#[test]
fn mesh_rejects_nan_profile() {
    let profile = vec![
        (0.92, 0.15, 0.10),
        (f32::NAN, 0.16, 0.11),
        (1.50, 0.05, 0.05),
    ];
    assert!(build_base_mesh_with_profile(&profile).is_err());
    let profile_nan_radius = vec![(0.92, f32::NAN, 0.10), (1.50, 0.05, 0.05)];
    assert!(build_base_mesh_with_profile(&profile_nan_radius).is_err());
}

#[test]
fn mesh_rejects_nonpositive_and_nonascending_profiles() {
    assert!(build_base_mesh_with_profile(&[(0.92, 0.0, 0.10), (1.50, 0.05, 0.05)]).is_err());
    assert!(build_base_mesh_with_profile(&[(0.92, -0.1, 0.10), (1.50, 0.05, 0.05)]).is_err());
    assert!(build_base_mesh_with_profile(&[(1.50, 0.15, 0.10), (0.92, 0.05, 0.05)]).is_err());
    assert!(build_base_mesh_with_profile(&[]).is_err());
    assert!(build_base_mesh_with_profile(&[(0.92, 0.15, 0.10)]).is_err());
    assert!(
        build_base_mesh_with_profile(&[(0.92, f32::INFINITY, 0.10), (1.50, 0.05, 0.05)]).is_err()
    );
}

#[test]
fn writer_rejects_mismatched_morph_deltas() {
    let mesh = build_base_mesh().unwrap();
    let bad = MorphTarget {
        deltas: vec![[0.0; 3]; 3],
        name: "bad".to_string(),
    };
    assert!(write_glb(&mesh, &[bad]).is_err());
    let nan = MorphTarget {
        deltas: vec![[f32::NAN, 0.0, 0.0]; mesh.head_masks.len()],
        name: "nan".to_string(),
    };
    assert!(write_glb(&mesh, &[nan]).is_err());
}
