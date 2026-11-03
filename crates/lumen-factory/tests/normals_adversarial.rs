//! Adversarial tests for `normals-from-proxy/1` (T-12): the
//! direct mode refuses skinned targets, the rig-safe mode proves
//! skin preservation, and malformed scopes/parts refuse by name.

use std::collections::BTreeMap;

use lumen_factory::base::{BaseMesh, Joint, MeshPart, Vertex};
use lumen_factory::convert::normals::{NormalsMode, run_normals_from_proxy};

fn vertex(position: [f32; 3], normal: [f32; 3], weight: f32) -> Vertex {
    Vertex {
        joints: [0; 4],
        normal,
        position,
        uv: [0.0; 2],
        weights: [weight, 0.0, 0.0, 0.0],
    }
}

fn proxy() -> BaseMesh {
    BaseMesh {
        head_masks: Vec::new(),
        joints: Vec::new(),
        parts: vec![MeshPart {
            indices: vec![0, 1, 2],
            name: "head".to_string(),
            vertices: vec![
                vertex([0.0, 0.0, 0.0], [0.0, 0.0, 1.0], 0.0),
                vertex([2.0, 0.0, 0.0], [1.0, 0.0, 0.0], 0.0),
                vertex([0.0, 2.0, 0.0], [0.0, 1.0, 0.0], 0.0),
            ],
        }],
    }
}

fn skinned_target() -> BaseMesh {
    BaseMesh {
        head_masks: Vec::new(),
        joints: vec![Joint {
            name: "head",
            parent: None,
            position: [0.0, 1.6, 0.0],
        }],
        parts: vec![MeshPart {
            indices: vec![0, 1, 2],
            name: "head".to_string(),
            vertices: vec![
                vertex([0.5, 0.5, 0.25], [0.0, 0.0, 1.0], 1.0),
                vertex([1.0, 0.0, 0.25], [0.0, 0.0, 1.0], 1.0),
                vertex([0.0, 1.0, 0.25], [0.0, 0.0, 1.0], 1.0),
            ],
        }],
    }
}

fn scopes(weights: Vec<f32>) -> BTreeMap<String, Vec<f32>> {
    BTreeMap::from([("head".to_string(), weights)])
}

#[test]
fn direct_mode_refuses_a_skinned_target() {
    let error = run_normals_from_proxy(
        &skinned_target(),
        &proxy(),
        &scopes(vec![1.0, 1.0, 1.0]),
        NormalsMode::Direct,
    )
    .expect_err("direct mode must refuse skin");
    assert!(error.to_string().contains("rig-safe"), "{error}");
}

#[test]
fn rig_safe_mode_transfers_and_preserves_the_skin_exactly() {
    let target = skinned_target();
    let (output, report) = run_normals_from_proxy(
        &target,
        &proxy(),
        &scopes(vec![1.0, 1.0, 1.0]),
        NormalsMode::RigSafeStaticDuplicate,
    )
    .expect("rig-safe transfer runs");
    assert!(report.skin_preserved);
    assert_eq!(report.mode, NormalsMode::RigSafeStaticDuplicate);
    for (out, original) in output.parts[0]
        .vertices
        .iter()
        .zip(target.parts[0].vertices.iter())
    {
        assert_eq!(out.joints, original.joints);
        assert_eq!(out.weights, original.weights);
        assert_eq!(out.position, original.position);
    }
    assert_eq!(output.joints.len(), target.joints.len());
    // And the normals actually moved.
    assert_ne!(output.parts[0].vertices[0].normal, [0.0, 0.0, 1.0]);
}

#[test]
fn scope_length_mismatch_and_out_of_range_weights_refuse() {
    let target = skinned_target();
    let rig = NormalsMode::RigSafeStaticDuplicate;
    assert!(run_normals_from_proxy(&target, &proxy(), &scopes(vec![1.0]), rig).is_err());
    assert!(run_normals_from_proxy(&target, &proxy(), &scopes(vec![1.0, 1.5, 1.0]), rig).is_err());
    assert!(
        run_normals_from_proxy(&target, &proxy(), &scopes(vec![1.0, f32::NAN, 1.0]), rig).is_err()
    );
}

#[test]
fn unmatched_parts_and_scopes_refuse_by_name() {
    let target = skinned_target();
    let rig = NormalsMode::RigSafeStaticDuplicate;
    // Proxy without the target's part.
    let mut bare_proxy = proxy();
    bare_proxy.parts[0].name = "other".to_string();
    let error = run_normals_from_proxy(&target, &bare_proxy, &scopes(vec![1.0; 3]), rig)
        .expect_err("missing proxy part must refuse");
    assert!(error.to_string().contains("head"), "{error}");
    // Scope for a part the target does not have.
    let mut wide = scopes(vec![1.0; 3]);
    wide.insert("ghost".to_string(), vec![1.0]);
    assert!(run_normals_from_proxy(&target, &proxy(), &wide, rig).is_err());
    // No scope at all for the part.
    assert!(run_normals_from_proxy(&target, &proxy(), &BTreeMap::new(), rig).is_err());
}

#[test]
fn degenerate_only_proxy_refuses() {
    let mut flat_proxy = proxy();
    flat_proxy.parts[0].vertices[2].position = [2.0, 0.0, 0.0];
    let error = run_normals_from_proxy(
        &skinned_target(),
        &flat_proxy,
        &scopes(vec![1.0; 3]),
        NormalsMode::RigSafeStaticDuplicate,
    )
    .expect_err("a proxy with no real face cannot transfer");
    assert!(
        error.to_string().contains("no non-degenerate triangle"),
        "{error}"
    );
}

#[test]
fn inputs_are_never_mutated() {
    let target = skinned_target();
    let before = target.parts[0].vertices[0].normal;
    let _ = run_normals_from_proxy(
        &target,
        &proxy(),
        &scopes(vec![1.0; 3]),
        NormalsMode::RigSafeStaticDuplicate,
    )
    .expect("transfer runs");
    assert_eq!(target.parts[0].vertices[0].normal, before);
}
