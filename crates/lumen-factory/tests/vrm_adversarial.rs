//! Adversarial tests: the VRM reader and wrapper fail closed on
//! hostile or malformed input — never a panic, never a partial wrap.

use std::path::PathBuf;

use lumen_factory::FactoryError;
use lumen_factory::gltf::{build_glb, parse_glb_parts};
use lumen_factory::vrm::{WrapMeta, inspect_vrm, wrap_as_vrm};

fn asset(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join(name);
    std::fs::read(&path).expect("asset readable")
}

fn wrapped() -> Vec<u8> {
    wrap_as_vrm(&asset("base_blended.glb"), &WrapMeta::new("Base")).expect("wrap")
}

/// Mutate the JSON document of a GLB and rebuild it (binary chunk
/// carried over byte-for-byte).
fn mutate_json(bytes: &[u8], mutate: impl FnOnce(&mut serde_json::Value)) -> Vec<u8> {
    let mut parts = parse_glb_parts(bytes).expect("parts");
    mutate(&mut parts.json);
    build_glb(&parts.json, &parts.bin).expect("rebuild")
}

#[test]
fn non_glb_bytes_are_rejected() {
    let err = inspect_vrm(b"this is not a glb file at all").expect_err("must fail");
    assert!(matches!(err, FactoryError::InvalidGlb(_)), "got {err:?}");
    let err = inspect_vrm(&[]).expect_err("empty input must fail");
    assert!(matches!(err, FactoryError::InvalidGlb(_)), "got {err:?}");
    let err = wrap_as_vrm(b"\x00\x01\x02\x03", &WrapMeta::new("X")).expect_err("wrap must fail");
    assert!(matches!(err, FactoryError::InvalidGlb(_)), "got {err:?}");
}

#[test]
fn glb_without_vrm_extension_is_not_vrm() {
    let err = inspect_vrm(&asset("base_blended.glb")).expect_err("plain GLB is not a VRM");
    assert!(matches!(err, FactoryError::NotVrm(_)), "got {err:?}");
    let err = inspect_vrm(&asset("seraphine.glb")).expect_err("plain GLB is not a VRM");
    assert!(matches!(err, FactoryError::NotVrm(_)), "got {err:?}");
}

#[test]
fn truncated_vrm_is_rejected() {
    let vrm = wrapped();
    let half = &vrm[..vrm.len() / 2];
    assert!(inspect_vrm(half).is_err(), "half-truncated file must fail");
    let almost = &vrm[..vrm.len() - 3];
    assert!(
        inspect_vrm(almost).is_err(),
        "tail-truncated file must fail"
    );
    let header_only = &vrm[..20];
    assert!(
        inspect_vrm(header_only).is_err(),
        "header-only file must fail"
    );
}

#[test]
fn vrmc_without_humanoid_is_rejected() {
    let broken = mutate_json(&wrapped(), |root| {
        root["extensions"]["VRMC_vrm"]
            .as_object_mut()
            .expect("vrmc object")
            .remove("humanoid");
    });
    let err = inspect_vrm(&broken).expect_err("missing humanoid must fail");
    assert!(matches!(err, FactoryError::InvalidGlb(_)), "got {err:?}");
}

#[test]
fn vrmc_with_empty_human_bones_is_rejected() {
    let broken = mutate_json(&wrapped(), |root| {
        root["extensions"]["VRMC_vrm"]["humanoid"]["humanBones"] = serde_json::json!({});
    });
    assert!(inspect_vrm(&broken).is_err(), "empty humanBones must fail");
}

#[test]
fn humanoid_node_out_of_range_is_rejected() {
    let broken = mutate_json(&wrapped(), |root| {
        root["extensions"]["VRMC_vrm"]["humanoid"]["humanBones"]["hips"]["node"] =
            serde_json::json!(9999);
    });
    let err = inspect_vrm(&broken).expect_err("out-of-range bone node must fail");
    assert!(matches!(err, FactoryError::InvalidGlb(_)), "got {err:?}");
}

#[test]
fn expression_bind_node_out_of_range_is_rejected() {
    let broken = mutate_json(&wrapped(), |root| {
        root["extensions"]["VRMC_vrm"]["expressions"]["preset"]["happy"]["morphTargetBinds"][0]["node"] =
            serde_json::json!(4242);
    });
    assert!(
        inspect_vrm(&broken).is_err(),
        "out-of-range bind node must fail"
    );
}

#[test]
fn vrm0_without_humanoid_is_rejected() {
    let broken = mutate_json(&wrapped(), |root| {
        let root_obj = root.as_object_mut().expect("root object");
        root_obj["extensions"] = serde_json::json!({
            "VRM": { "meta": { "title": "no bones" } }
        });
    });
    assert!(
        inspect_vrm(&broken).is_err(),
        "0.x without humanoid must fail"
    );
}

#[test]
fn wrap_requires_morph_targets() {
    // base.glb carries no morph targets, so there is nothing to bind
    // the VRM expressions to: the wrap must fail closed.
    let err = wrap_as_vrm(&asset("base.glb"), &WrapMeta::new("Base"))
        .expect_err("wrap without targets must fail");
    assert!(matches!(err, FactoryError::BadParam(_)), "got {err:?}");
}

#[test]
fn wrap_requires_a_name() {
    let err = wrap_as_vrm(&asset("base_blended.glb"), &WrapMeta::new(""))
        .expect_err("empty name must fail");
    assert!(matches!(err, FactoryError::BadParam(_)), "got {err:?}");
}
