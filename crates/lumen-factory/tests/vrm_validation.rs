//! Validation tests: VRM 1.0 wrap -> inspect round trip on the real
//! factory assets, geometry preservation, determinism, proportions
//! against the identity record, and a synthetic VRM 0.x summary.

use std::path::PathBuf;

use lumen_factory::base::base_proportions;
use lumen_factory::gltf::{parse_glb, parse_glb_parts};
use lumen_factory::identity::load_record;
use lumen_factory::vrm::{
    CUSTOM_EXPRESSIONS, PRESET_EXPRESSIONS, REQUIRED_BONES, VRM_LICENSE_URL, WrapMeta, inspect_vrm,
    wrap_as_vrm,
};

fn asset(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join(name);
    std::fs::read(&path).expect("asset readable")
}

fn wrapped_seraphine() -> Vec<u8> {
    wrap_as_vrm(&asset("seraphine.glb"), &WrapMeta::new("Séraphine")).expect("wrap seraphine glb")
}

#[test]
fn wrap_inspect_round_trip_bones_expressions_meta() {
    let vrm = wrapped_seraphine();
    let summary = inspect_vrm(&vrm).expect("wrapped file inspects as vrm");
    assert_eq!(summary.flavor, "vrm-1.0");
    assert_eq!(summary.spec_version.as_deref(), Some("1.0"));
    assert_eq!(summary.meta.name.as_deref(), Some("Séraphine"));
    assert_eq!(summary.meta.version.as_deref(), Some("1.0.0"));
    assert_eq!(summary.meta.license_url.as_deref(), Some(VRM_LICENSE_URL));
    assert_eq!(summary.meta.authors, vec!["Lumen Factory".to_string()]);
    // 15 required bones plus chest / upperChest / neck optionals.
    assert_eq!(summary.humanoid_bones.len(), 18);
    for bone in REQUIRED_BONES {
        assert!(
            summary.humanoid_bones.contains_key(*bone),
            "required bone {bone} mapped"
        );
    }
    for optional in ["chest", "upperChest", "neck"] {
        assert!(
            summary.humanoid_bones.contains_key(optional),
            "optional bone {optional} mapped from factory joints"
        );
    }
    assert!(
        !summary.humanoid_bones.contains_key("leftEye"),
        "factory has no eye joints; eyes must be omitted"
    );
    // Bone map points at the nodes carrying the Mixamo joint names.
    let parts = parse_glb_parts(&vrm).expect("parts");
    let node_name = |idx: usize| {
        parts.json["nodes"][idx]["name"]
            .as_str()
            .unwrap_or("")
            .to_string()
    };
    assert_eq!(
        node_name(summary.humanoid_bones["hips"]),
        "Hips",
        "hips maps to the Hips node"
    );
    assert_eq!(node_name(summary.humanoid_bones["head"]), "Head");
    assert_eq!(node_name(summary.humanoid_bones["leftUpperArm"]), "LeftArm");
    assert_eq!(
        node_name(summary.humanoid_bones["rightLowerLeg"]),
        "RightLeg"
    );
    assert_eq!(node_name(summary.humanoid_bones["upperChest"]), "Spine2");
    // Expressions: presets and customs, each with exactly one bind.
    let preset_names: Vec<&str> = summary
        .preset_expressions
        .iter()
        .map(|e| e.name.as_str())
        .collect();
    for name in PRESET_EXPRESSIONS {
        assert!(preset_names.contains(name), "preset {name} present");
    }
    let custom_names: Vec<&str> = summary
        .custom_expressions
        .iter()
        .map(|e| e.name.as_str())
        .collect();
    for name in CUSTOM_EXPRESSIONS {
        assert!(custom_names.contains(name), "custom {name} present");
    }
    for expression in summary
        .preset_expressions
        .iter()
        .chain(summary.custom_expressions.iter())
    {
        assert_eq!(expression.binds.len(), 1, "{} bind count", expression.name);
        assert_eq!(expression.binds[0].weight, 1.0);
    }
}

#[test]
fn expression_binds_resolve_through_target_names() {
    let glb = asset("seraphine.glb");
    let glb_summary = parse_glb(&glb).expect("seraphine glb parses");
    let vrm = wrap_as_vrm(&glb, &WrapMeta::new("Séraphine")).expect("wrap");
    let summary = inspect_vrm(&vrm).expect("inspect");
    let head_node = summary
        .preset_expressions
        .iter()
        .find(|e| e.name == "happy")
        .expect("happy preset")
        .binds[0]
        .node;
    // The bound node carries the morph-target mesh.
    let parts = parse_glb_parts(&vrm).expect("parts");
    assert!(
        parts.json["nodes"][head_node].get("mesh").is_some(),
        "expression binds point at the mesh node"
    );
    for expression in summary
        .preset_expressions
        .iter()
        .chain(summary.custom_expressions.iter())
    {
        let bind = &expression.binds[0];
        assert_eq!(bind.node, head_node, "all binds share the head node");
        assert_eq!(
            glb_summary.target_names[bind.index], expression.name,
            "bind index resolves via extras.targetNames to the same-named target"
        );
    }
}

#[test]
fn wrap_preserves_geometry_byte_identically() {
    let glb = asset("seraphine.glb");
    let vrm = wrap_as_vrm(&glb, &WrapMeta::new("Séraphine")).expect("wrap");
    let before = parse_glb_parts(&glb).expect("parts before");
    let after = parse_glb_parts(&vrm).expect("parts after");
    assert_eq!(before.bin, after.bin, "binary chunk copied byte-for-byte");
    for key in [
        "meshes",
        "skins",
        "buffers",
        "bufferViews",
        "accessors",
        "nodes",
    ] {
        assert_eq!(
            before.json[key], after.json[key],
            "JSON region {key} unchanged by wrap"
        );
    }
    let before_summary = parse_glb(&glb).expect("summary before");
    let after_summary = parse_glb(&vrm).expect("summary after");
    assert_eq!(
        before_summary, after_summary,
        "structural summary unchanged"
    );
    // extensionsUsed gains exactly the VRMC_vrm entry.
    let used: Vec<&str> = after.json["extensionsUsed"]
        .as_array()
        .expect("extensionsUsed")
        .iter()
        .filter_map(|v| v.as_str())
        .collect();
    assert_eq!(used, vec!["VRMC_vrm"]);
}

#[test]
fn wrap_is_deterministic() {
    let glb = asset("base_blended.glb");
    let a = wrap_as_vrm(&glb, &WrapMeta::new("Base")).expect("wrap a");
    let b = wrap_as_vrm(&glb, &WrapMeta::new("Base")).expect("wrap b");
    assert_eq!(a, b, "same GLB + same meta must wrap byte-identically");
}

#[test]
fn proportions_match_base_for_canonical_skeleton() {
    let vrm = wrap_as_vrm(&asset("base_blended.glb"), &WrapMeta::new("Base")).expect("wrap");
    let summary = inspect_vrm(&vrm).expect("inspect");
    let proportions = summary.proportions.expect("proportions derived");
    let (head, torso, leg) = base_proportions();
    assert!((proportions.total_height - 1.78).abs() < 1e-4);
    assert!(
        (proportions.head_ratio - head).abs() < 1e-4,
        "head {} vs base {head}",
        proportions.head_ratio
    );
    assert!((proportions.torso_ratio - torso).abs() < 1e-4);
    assert!((proportions.leg_ratio - leg).abs() < 1e-4);
    let sum = proportions.head_ratio + proportions.torso_ratio + proportions.leg_ratio;
    assert!((sum - 1.0).abs() < 1e-4, "ratios sum to 1, got {sum}");
}

#[test]
fn seraphine_proportions_match_identity_record() {
    let identity_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("assets")
        .join("seraphine.identity.json");
    let record = load_record(&identity_path).expect("identity record loads");
    let summary = inspect_vrm(&wrapped_seraphine()).expect("inspect");
    let proportions = summary.proportions.expect("proportions derived");
    assert!(
        (proportions.head_ratio - record.proportions.head_ratio as f32).abs() < 2e-3,
        "vrm head {} vs identity {}",
        proportions.head_ratio,
        record.proportions.head_ratio
    );
    assert!(
        (proportions.torso_ratio - record.proportions.torso_ratio as f32).abs() < 2e-3,
        "vrm torso {} vs identity {}",
        proportions.torso_ratio,
        record.proportions.torso_ratio
    );
    assert!(
        (proportions.leg_ratio - record.proportions.leg_ratio as f32).abs() < 2e-3,
        "vrm leg {} vs identity {}",
        proportions.leg_ratio,
        record.proportions.leg_ratio
    );
}

#[test]
fn vrm_0x_summary_shape_matches() {
    // Rebuild the wrapped file's document into the 0.x shape:
    // extensions.VRM with a humanoidBone list and blendShapeGroups.
    let vrm = wrapped_seraphine();
    let mut parts = parse_glb_parts(&vrm).expect("parts");
    let vrmc = parts.json["extensions"]["VRMC_vrm"].clone();
    let mut humanoid_bone: Vec<serde_json::Value> = Vec::new();
    for (bone, entry) in vrmc["humanoid"]["humanBones"]
        .as_object()
        .expect("humanBones")
    {
        humanoid_bone.push(serde_json::json!({ "bone": bone, "node": entry["node"] }));
    }
    let mut groups: Vec<serde_json::Value> = Vec::new();
    for (name, entry) in vrmc["expressions"]["preset"].as_object().expect("preset") {
        let bind = &entry["morphTargetBinds"][0];
        groups.push(serde_json::json!({
            "binds": [{ "index": bind["index"], "mesh": 0, "weight": 100.0 }],
            "name": name,
            "presetName": name,
        }));
    }
    let vrm0 = serde_json::json!({
        "blendShapeMaster": { "blendShapeGroups": groups },
        "exporterVersion": "UniVRM-0.128.0",
        "humanoidBone": humanoid_bone,
        "meta": {
            "author": "Lumen Factory",
            "licenseUrl": "https://vrm.dev/licenses/1.0/",
            "title": "Séraphine 0.x",
            "version": "2",
        },
        "specVersion": "0.0",
    });
    let root = parts.json.as_object_mut().expect("root object");
    root["extensions"] = serde_json::json!({ "VRM": vrm0 });
    root["extensionsUsed"] = serde_json::json!(["VRM"]);
    let bytes = lumen_factory::gltf::build_glb(&parts.json, &parts.bin).expect("rebuild");

    let summary = inspect_vrm(&bytes).expect("0.x file inspects");
    assert_eq!(summary.flavor, "vrm-0.x");
    assert_eq!(summary.meta.name.as_deref(), Some("Séraphine 0.x"));
    assert_eq!(summary.meta.authors, vec!["Lumen Factory".to_string()]);
    assert_eq!(summary.spec_version.as_deref(), Some("UniVRM-0.128.0"));
    assert_eq!(summary.humanoid_bones.len(), 18);
    assert!(summary.humanoid_bones.contains_key("hips"));
    let preset_names: Vec<&str> = summary
        .preset_expressions
        .iter()
        .map(|e| e.name.as_str())
        .collect();
    assert!(preset_names.contains(&"happy"), "0.x preset happy present");
    let happy = summary
        .preset_expressions
        .iter()
        .find(|e| e.name == "happy")
        .expect("happy group");
    assert_eq!(happy.binds.len(), 1);
    assert!(
        (happy.binds[0].weight - 1.0).abs() < 1e-9,
        "0.x weight 100 normalizes to 1.0"
    );
    assert!(summary.proportions.is_some(), "0.x proportions derived");
}
