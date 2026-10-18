//! VRM reader and wrapper: the VRoid input path (Phase 2b).
//!
//! Contract summary:
//! - [`inspect_vrm`] parses a `.vrm` file (a GLB 2.0 container, parsed
//!   through [`crate::gltf::parse_glb_parts`] — the factory's single
//!   GLB parser, never a fork) and extracts a [`VrmSummary`]: the
//!   flavor (`vrm-1.0` when `extensions.VRMC_vrm` is present,
//!   `vrm-0.x` when `extensions.VRM` is present, otherwise a
//!   fail-closed [`FactoryError::NotVrm`]), the meta block, the
//!   humanoid bone map (VRM bone name -> glTF node index), the
//!   expression names (preset and custom), and derived body
//!   proportions.
//! - [`wrap_as_vrm`] turns a factory GLB into a VRM 1.0 file by
//!   injecting (or replacing) `extensions.VRMC_vrm`: the humanoid
//!   mapping for the 15 required VRM 1.0 bones derived from the
//!   factory's Mixamo joint names (plus the optional `chest`,
//!   `upperChest`, and `neck` bones where the factory joints exist;
//!   the factory skeleton has no eye joints, so `leftEye`/`rightEye`
//!   are omitted), a meta block, and expressions whose
//!   `morphTargetBinds` point at the factory head mesh's morph
//!   targets, resolved by name through `extras.targetNames` (target
//!   order is alphabetical, per `blend.rs`). The binary chunk is
//!   copied byte-for-byte: geometry, skins, and buffers are provably
//!   untouched (asserted by the parse-back tests).
//! - Every structural problem (missing humanoid, a bone or bind
//!   referencing a node index out of range, a required factory joint
//!   or morph target missing at wrap time) is a typed
//!   [`FactoryError`], never a panic and never a partial output.
//!
//! Division of labor with lumen's `src/vrm.rs` (landed in 39cf296;
//! reconciled in the integration-update pass): that module owns the
//! texture layer of an existing VRM — its `vrm_inspect` reports
//! flavor / meta / texture inventory and `vrm_grade` re-encodes the
//! embedded textures with a style preset, geometry and extensions
//! untouched. This module owns the structure layer those ops do not
//! expose: the humanoid bone map, the expression binds, and derived
//! proportions (the factory's build inputs), plus the estate's only
//! GLB -> VRM wrap (lumen never wraps a bare GLB). The two readers
//! are independent implementations of the GLB / VRM contract by
//! design — this crate is standalone (see the factory plan) — and
//! are verified interoperable against each other: lumen's
//! `vrm_grade` output for `assets/seraphine.vrm` (preset
//! `demo_toon_portrait`) is committed as
//! `assets/seraphine_graded.vrm`, and `inspect_vrm` extracts from it
//! a summary identical to the ungraded file's (asserted in
//! `tests/vrm_interop.rs`).
//!
//! Derived proportions (definitions): node world translations are
//! composed through the node hierarchy (TRS / matrix, full 4x4
//! composition). Let `top` be the maximum world Y over all nodes,
//! `bottom` the minimum, and `total = top - bottom`.
//! - head span = `top - neck_y` when a `neck` bone is mapped, else
//!   `top - head_y` (comparable to `identity.rs`, whose head ratio is
//!   the head band from the top of the sprite to the neck row).
//! - torso span = `neck_y - hips_y` (or `head_y - hips_y` without a
//!   neck).
//! - leg span = `hips_y - bottom` (comparable to the identity leg
//!   ratio, hip row to the bottom of the sprite).
//!   Ratios are the spans divided by `total`; they sum to 1 by
//!   construction when a neck is present.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

use crate::gltf::{build_glb, parse_glb_parts};
use crate::{FactoryError, MAX_GLB_BYTES};

/// VRM spec version emitted by [`wrap_as_vrm`].
pub const VRM_SPEC_VERSION: &str = "1.0";
/// VRM Public License URL emitted in wrapped meta blocks.
pub const VRM_LICENSE_URL: &str = "https://vrm.dev/licenses/1.0/";
/// Default author list emitted in wrapped meta blocks.
pub const VRM_DEFAULT_AUTHORS: &[&str] = &["Lumen Factory"];

/// The 15 required VRM 1.0 humanoid bones, in spec order.
pub const REQUIRED_BONES: &[&str] = &[
    "hips",
    "spine",
    "head",
    "leftUpperArm",
    "leftLowerArm",
    "leftHand",
    "rightUpperArm",
    "rightLowerArm",
    "rightHand",
    "leftUpperLeg",
    "leftLowerLeg",
    "leftFoot",
    "rightUpperLeg",
    "rightLowerLeg",
    "rightFoot",
];

/// Required VRM bone -> factory (Mixamo) joint name. The optional
/// bones (`chest`, `upperChest`, `neck`) are appended by
/// [`bone_mapping`] when the corresponding factory joints exist.
fn required_bone_mapping() -> Vec<(&'static str, &'static str)> {
    vec![
        ("hips", "Hips"),
        ("spine", "Spine"),
        ("head", "Head"),
        ("leftUpperArm", "LeftArm"),
        ("leftLowerArm", "LeftForeArm"),
        ("leftHand", "LeftHand"),
        ("rightUpperArm", "RightArm"),
        ("rightLowerArm", "RightForeArm"),
        ("rightHand", "RightHand"),
        ("leftUpperLeg", "LeftUpLeg"),
        ("leftLowerLeg", "LeftLeg"),
        ("leftFoot", "LeftFoot"),
        ("rightUpperLeg", "RightUpLeg"),
        ("rightLowerLeg", "RightLeg"),
        ("rightFoot", "RightFoot"),
    ]
}

/// Full VRM bone -> factory joint mapping (required bones first, in
/// spec order, then the optional bones in spec order).
fn bone_mapping() -> Vec<(&'static str, &'static str)> {
    let mut out = required_bone_mapping();
    out.push(("chest", "Spine1"));
    out.push(("upperChest", "Spine2"));
    out.push(("neck", "Neck"));
    out
}

/// VRM 1.0 preset expressions bound by [`wrap_as_vrm`].
pub const PRESET_EXPRESSIONS: &[&str] = &["happy", "angry", "sad", "surprised", "neutral"];
/// Custom expressions bound by [`wrap_as_vrm`].
pub const CUSTOM_EXPRESSIONS: &[&str] = &[
    "fun",
    "annoyed",
    "worried",
    "embarrassed",
    "determined",
    "smug",
];

/// VRM meta block, normalized across the 1.0 and 0.x shapes.
#[derive(Clone, Debug, PartialEq)]
pub struct VrmMeta {
    /// Author names (1.0 `meta.authors`; 0.x `meta.author` as a
    /// single-element list).
    pub authors: Vec<String>,
    /// License URL when present (1.0 `meta.licenseUrl`; 0.x
    /// `meta.otherLicenseUrl` / `meta.licenseUrl`).
    pub license_url: Option<String>,
    /// Avatar display name (1.0 `meta.name`; 0.x `meta.title`).
    pub name: Option<String>,
    /// Model version string when present (1.0 `meta.version`;
    /// 0.x `meta.version`).
    pub version: Option<String>,
}

/// One morph-target bind inside a VRM expression.
#[derive(Clone, Debug, PartialEq)]
pub struct VrmMorphBind {
    /// Morph target index within the bound node's mesh primitive.
    pub index: usize,
    /// glTF node index carrying the morph targets.
    pub node: usize,
    /// Bind weight (finite; VRM uses 0.0..=1.0 in practice, the
    /// reader only requires finiteness).
    pub weight: f64,
}

/// One VRM expression: a name plus its morph-target binds.
#[derive(Clone, Debug, PartialEq)]
pub struct VrmExpression {
    /// Bind list (empty when the expression declares none).
    pub binds: Vec<VrmMorphBind>,
    /// Expression name (the preset key or the custom key).
    pub name: String,
}

/// Derived body proportions from node world translations (see the
/// module docs for the span definitions).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VrmProportions {
    /// Head span (meters).
    pub head_span: f32,
    /// Head span / total height.
    pub head_ratio: f32,
    /// Leg span (meters).
    pub leg_span: f32,
    /// Leg span / total height.
    pub leg_ratio: f32,
    /// Torso span (meters).
    pub torso_span: f32,
    /// Torso span / total height.
    pub torso_ratio: f32,
    /// Total height (meters): max node world Y minus min node world Y.
    pub total_height: f32,
}

/// The summary returned by [`inspect_vrm`].
#[derive(Clone, Debug, PartialEq)]
pub struct VrmSummary {
    /// Custom expressions, sorted by name.
    pub custom_expressions: Vec<VrmExpression>,
    /// `vrm-1.0` or `vrm-0.x`.
    pub flavor: String,
    /// Humanoid bone map: VRM bone name -> glTF node index.
    pub humanoid_bones: BTreeMap<String, usize>,
    /// Meta block (normalized across flavors).
    pub meta: VrmMeta,
    /// Number of glTF nodes in the document.
    pub node_count: usize,
    /// Preset expressions, sorted by name.
    pub preset_expressions: Vec<VrmExpression>,
    /// Derived proportions, when `hips` and `head` are both mapped.
    pub proportions: Option<VrmProportions>,
    /// Extension spec/exporter version string when present.
    pub spec_version: Option<String>,
}

/// Meta input for [`wrap_as_vrm`].
#[derive(Clone, Debug, PartialEq)]
pub struct WrapMeta {
    /// Author names for the meta block.
    pub authors: Vec<String>,
    /// License URL for the meta block.
    pub license_url: String,
    /// Model name for the meta block.
    pub name: String,
    /// Model version string for the meta block.
    pub version: String,
}

impl WrapMeta {
    /// Meta for a model `name`, with the factory defaults: authors
    /// [`VRM_DEFAULT_AUTHORS`], license [`VRM_LICENSE_URL`], version
    /// `1.0.0`.
    pub fn new(name: &str) -> Self {
        Self {
            authors: VRM_DEFAULT_AUTHORS.iter().map(|s| s.to_string()).collect(),
            license_url: VRM_LICENSE_URL.to_string(),
            name: name.to_string(),
            version: "1.0.0".to_string(),
        }
    }
}

// --- small 4x4 helpers (column-major, matching gltf.rs) ---

type Mat4 = [[f32; 4]; 4];

fn mat4_identity() -> Mat4 {
    let mut m = [[0.0; 4]; 4];
    m[0][0] = 1.0;
    m[1][1] = 1.0;
    m[2][2] = 1.0;
    m[3][3] = 1.0;
    m
}

fn mat4_mul(a: &Mat4, b: &Mat4) -> Mat4 {
    let mut out = [[0.0f32; 4]; 4];
    for col in 0..4 {
        for row in 0..4 {
            let mut sum = 0.0f32;
            for k in 0..4 {
                sum += a[k][row] * b[col][k];
            }
            out[col][row] = sum;
        }
    }
    out
}

fn node_local_matrix(node: &Value, index: usize) -> Result<Mat4, FactoryError> {
    if let Some(matrix) = node.get("matrix").and_then(Value::as_array) {
        if matrix.len() != 16 {
            return Err(FactoryError::InvalidGlb(format!(
                "node {index} matrix has {} elements, expected 16",
                matrix.len()
            )));
        }
        let mut m = [[0.0f32; 4]; 4];
        for (i, value) in matrix.iter().enumerate() {
            let v = value.as_f64().ok_or_else(|| {
                FactoryError::InvalidGlb(format!("node {index} matrix element {i} not a number"))
            })? as f32;
            if !v.is_finite() {
                return Err(FactoryError::InvalidGlb(format!(
                    "node {index} matrix element {i} not finite"
                )));
            }
            m[i / 4][i % 4] = v;
        }
        return Ok(m);
    }
    let read_vec = |key: &str, default: [f32; 4], len: usize| -> Result<Vec<f32>, FactoryError> {
        match node.get(key).and_then(Value::as_array) {
            Some(items) => {
                if items.len() != len {
                    return Err(FactoryError::InvalidGlb(format!(
                        "node {index} {key} has {} elements, expected {len}",
                        items.len()
                    )));
                }
                let mut out = Vec::with_capacity(len);
                for item in items {
                    let v = item.as_f64().ok_or_else(|| {
                        FactoryError::InvalidGlb(format!("node {index} {key} not numeric"))
                    })? as f32;
                    if !v.is_finite() {
                        return Err(FactoryError::InvalidGlb(format!(
                            "node {index} {key} not finite"
                        )));
                    }
                    out.push(v);
                }
                Ok(out)
            }
            None => Ok(default[..len].to_vec()),
        }
    };
    let t = read_vec("translation", [0.0, 0.0, 0.0, 0.0], 3)?;
    let q = read_vec("rotation", [0.0, 0.0, 0.0, 1.0], 4)?;
    let s = read_vec("scale", [1.0, 1.0, 1.0, 0.0], 3)?;
    let (x, y, z, w) = (q[0], q[1], q[2], q[3]);
    // Rotation matrix from the quaternion (row-major terms), then
    // compose T * R * S in column-major storage.
    let r = [
        [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y - z * w),
            2.0 * (x * z + y * w),
        ],
        [
            2.0 * (x * y + z * w),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z - x * w),
        ],
        [
            2.0 * (x * z - y * w),
            2.0 * (y * z + x * w),
            1.0 - 2.0 * (x * x + y * y),
        ],
    ];
    let mut m = mat4_identity();
    for col in 0..3 {
        for row in 0..3 {
            m[col][row] = r[row][col] * s[col];
        }
    }
    m[3][0] = t[0];
    m[3][1] = t[1];
    m[3][2] = t[2];
    Ok(m)
}

/// World translations for every node, composed through the
/// children hierarchy. Roots are nodes never listed as a child.
/// Fails closed on out-of-range children, cycles, or non-finite
/// transforms.
fn node_world_translations(root: &Value) -> Result<Vec<[f32; 3]>, FactoryError> {
    let nodes = root
        .get("nodes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let count = nodes.len();
    let mut parent: Vec<Option<usize>> = vec![None; count];
    for (idx, node) in nodes.iter().enumerate() {
        if let Some(children) = node.get("children").and_then(Value::as_array) {
            for child in children {
                let child_idx = child.as_u64().ok_or_else(|| {
                    FactoryError::InvalidGlb(format!("node {idx} child is not an index"))
                })? as usize;
                if child_idx >= count {
                    return Err(FactoryError::InvalidGlb(format!(
                        "node {idx} child {child_idx} out of range ({count} nodes)"
                    )));
                }
                parent[child_idx] = Some(idx);
            }
        }
    }
    let mut worlds: Vec<Option<Mat4>> = vec![None; count];
    for (idx, slot) in worlds.iter_mut().enumerate() {
        // Walk to the root, collecting the chain (bounded by count).
        let mut chain: Vec<usize> = Vec::new();
        let mut current = Some(idx);
        while let Some(node_idx) = current {
            if chain.len() > count {
                return Err(FactoryError::InvalidGlb(
                    "node hierarchy contains a cycle".to_string(),
                ));
            }
            chain.push(node_idx);
            current = parent[node_idx];
        }
        let mut world = mat4_identity();
        for node_idx in chain.iter().rev() {
            let local = node_local_matrix(&nodes[*node_idx], *node_idx)?;
            world = mat4_mul(&world, &local);
        }
        *slot = Some(world);
    }
    let mut out = Vec::with_capacity(count);
    for (idx, world) in worlds.into_iter().enumerate() {
        let world = world.ok_or_else(|| {
            FactoryError::InvalidGlb(format!("node {idx} world transform uncomposed"))
        })?;
        let translation = [world[3][0], world[3][1], world[3][2]];
        if !translation.iter().all(|v| v.is_finite()) {
            return Err(FactoryError::InvalidGlb(format!(
                "node {idx} world translation not finite"
            )));
        }
        out.push(translation);
    }
    Ok(out)
}

fn derive_proportions(
    bones: &BTreeMap<String, usize>,
    worlds: &[[f32; 3]],
) -> Option<VrmProportions> {
    let hips_idx = *bones.get("hips")?;
    let head_idx = *bones.get("head")?;
    let top = worlds
        .iter()
        .map(|p| p[1])
        .fold(f32::NEG_INFINITY, f32::max);
    let bottom = worlds.iter().map(|p| p[1]).fold(f32::INFINITY, f32::min);
    let total = top - bottom;
    if !total.is_finite() || total <= 0.0 {
        return None;
    }
    let hips_y = worlds.get(hips_idx)?[1];
    let head_y = worlds.get(head_idx)?[1];
    let neck_y = bones
        .get("neck")
        .and_then(|idx| worlds.get(*idx))
        .map(|p| p[1]);
    let head_top_ref = neck_y.unwrap_or(head_y);
    let head_span = top - head_top_ref;
    let torso_span = head_top_ref - hips_y;
    let leg_span = hips_y - bottom;
    Some(VrmProportions {
        head_ratio: head_span / total,
        head_span,
        leg_ratio: leg_span / total,
        leg_span,
        torso_ratio: torso_span / total,
        torso_span,
        total_height: total,
    })
}

fn check_node_index(index: usize, node_count: usize, what: &str) -> Result<(), FactoryError> {
    if index >= node_count {
        return Err(FactoryError::InvalidGlb(format!(
            "{what} references node {index}, out of range ({node_count} nodes)"
        )));
    }
    Ok(())
}

fn parse_binds(
    raw: Option<&Value>,
    node_count: usize,
    what: &str,
) -> Result<Vec<VrmMorphBind>, FactoryError> {
    let mut out = Vec::new();
    if let Some(binds) = raw.and_then(Value::as_array) {
        for bind in binds {
            let node = bind
                .get("node")
                .and_then(Value::as_u64)
                .ok_or_else(|| FactoryError::InvalidGlb(format!("{what} bind missing node")))?
                as usize;
            let index = bind
                .get("index")
                .and_then(Value::as_u64)
                .ok_or_else(|| FactoryError::InvalidGlb(format!("{what} bind missing index")))?
                as usize;
            let weight = bind.get("weight").and_then(Value::as_f64).unwrap_or(1.0);
            if !weight.is_finite() {
                return Err(FactoryError::InvalidGlb(format!(
                    "{what} bind weight not finite"
                )));
            }
            check_node_index(node, node_count, what)?;
            out.push(VrmMorphBind {
                index,
                node,
                weight,
            });
        }
    }
    Ok(out)
}

/// The flavor-specific pieces of a parsed VRM extension, normalized
/// into one shape by the 1.0 and 0.x parsers.
struct ParsedFlavor {
    /// Humanoid bone map: VRM bone name -> glTF node index.
    bones: BTreeMap<String, usize>,
    /// Custom expressions, sorted by name.
    custom: Vec<VrmExpression>,
    /// Meta block.
    meta: VrmMeta,
    /// Preset expressions, sorted by name.
    preset: Vec<VrmExpression>,
    /// Extension spec/exporter version string when present.
    spec_version: Option<String>,
}

/// Inspect a VRM file's bytes: flavor, meta, humanoid bone map,
/// expressions, and derived proportions. Fail-closed: non-GLB input
/// is an [`FactoryError::InvalidGlb`]; a GLB with no VRM extension is
/// [`FactoryError::NotVrm`]; a VRM whose humanoid block is missing,
/// or whose bones/binds reference out-of-range nodes, is an error.
pub fn inspect_vrm(bytes: &[u8]) -> Result<VrmSummary, FactoryError> {
    if bytes.len() > MAX_GLB_BYTES {
        return Err(FactoryError::InvalidGlb(format!(
            "vrm size {} exceeds bound {MAX_GLB_BYTES}",
            bytes.len()
        )));
    }
    let parts = parse_glb_parts(bytes)?;
    let root = &parts.json;
    if root
        .get("asset")
        .and_then(|a| a.get("version"))
        .and_then(Value::as_str)
        != Some("2.0")
    {
        return Err(FactoryError::InvalidGlb(
            "asset.version is not \"2.0\"".to_string(),
        ));
    }
    let extensions = root.get("extensions").and_then(Value::as_object);
    let (flavor, ext) = match extensions {
        Some(map) if map.get("VRMC_vrm").is_some_and(Value::is_object) => {
            ("vrm-1.0", map["VRMC_vrm"].clone())
        }
        Some(map) if map.get("VRM").is_some_and(Value::is_object) => {
            ("vrm-0.x", map["VRM"].clone())
        }
        _ => {
            return Err(FactoryError::NotVrm(
                "neither extensions.VRMC_vrm (1.0) nor extensions.VRM (0.x) is present".to_string(),
            ));
        }
    };
    let nodes = root
        .get("nodes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let node_count = nodes.len();

    let parsed = if flavor == "vrm-1.0" {
        parse_vrm_1_0(&ext, node_count)?
    } else {
        parse_vrm_0_x(&ext, node_count)?
    };

    let worlds = node_world_translations(root)?;
    let proportions = derive_proportions(&parsed.bones, &worlds);
    Ok(VrmSummary {
        custom_expressions: parsed.custom,
        flavor: flavor.to_string(),
        humanoid_bones: parsed.bones,
        meta: parsed.meta,
        node_count,
        preset_expressions: parsed.preset,
        proportions,
        spec_version: parsed.spec_version,
    })
}

fn parse_vrm_1_0(ext: &Value, node_count: usize) -> Result<ParsedFlavor, FactoryError> {
    let meta_obj = ext.get("meta").and_then(Value::as_object);
    let meta = VrmMeta {
        authors: meta_obj
            .and_then(|m| m.get("authors"))
            .and_then(Value::as_array)
            .map(|items| {
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_string)
                    .collect()
            })
            .unwrap_or_default(),
        license_url: meta_obj
            .and_then(|m| m.get("licenseUrl"))
            .and_then(Value::as_str)
            .map(str::to_string),
        name: meta_obj
            .and_then(|m| m.get("name"))
            .and_then(Value::as_str)
            .map(str::to_string),
        version: meta_obj
            .and_then(|m| m.get("version"))
            .and_then(Value::as_str)
            .map(str::to_string),
    };
    let spec_version = ext
        .get("specVersion")
        .and_then(Value::as_str)
        .map(str::to_string);

    let humanoid = ext
        .get("humanoid")
        .and_then(Value::as_object)
        .ok_or_else(|| FactoryError::InvalidGlb("VRMC_vrm missing humanoid object".to_string()))?;
    let human_bones = humanoid
        .get("humanBones")
        .and_then(Value::as_object)
        .ok_or_else(|| {
            FactoryError::InvalidGlb("VRMC_vrm humanoid missing humanBones".to_string())
        })?;
    let mut bones: BTreeMap<String, usize> = BTreeMap::new();
    for (bone, entry) in human_bones {
        let node = entry.get("node").and_then(Value::as_u64).ok_or_else(|| {
            FactoryError::InvalidGlb(format!("humanBones.{bone} missing node index"))
        })? as usize;
        check_node_index(node, node_count, &format!("humanBones.{bone}"))?;
        bones.insert(bone.clone(), node);
    }
    if bones.is_empty() {
        return Err(FactoryError::InvalidGlb(
            "VRMC_vrm humanBones is empty".to_string(),
        ));
    }

    let mut preset: Vec<VrmExpression> = Vec::new();
    let mut custom: Vec<VrmExpression> = Vec::new();
    if let Some(expressions) = ext.get("expressions").and_then(Value::as_object) {
        if let Some(presets) = expressions.get("preset").and_then(Value::as_object) {
            for (name, entry) in presets {
                let binds = parse_binds(
                    entry.get("morphTargetBinds"),
                    node_count,
                    &format!("expressions.preset.{name}"),
                )?;
                preset.push(VrmExpression {
                    binds,
                    name: name.clone(),
                });
            }
        }
        if let Some(customs) = expressions.get("custom").and_then(Value::as_object) {
            for (name, entry) in customs {
                let binds = parse_binds(
                    entry.get("morphTargetBinds"),
                    node_count,
                    &format!("expressions.custom.{name}"),
                )?;
                custom.push(VrmExpression {
                    binds,
                    name: name.clone(),
                });
            }
        }
    }
    preset.sort_by(|a, b| a.name.cmp(&b.name));
    custom.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(ParsedFlavor {
        bones,
        custom,
        meta,
        preset,
        spec_version,
    })
}

fn parse_vrm_0_x(ext: &Value, node_count: usize) -> Result<ParsedFlavor, FactoryError> {
    let meta_obj = ext.get("meta").and_then(Value::as_object);
    let meta = VrmMeta {
        authors: meta_obj
            .and_then(|m| m.get("author"))
            .and_then(Value::as_str)
            .map(|a| vec![a.to_string()])
            .unwrap_or_default(),
        license_url: meta_obj
            .and_then(|m| {
                m.get("otherLicenseUrl")
                    .and_then(Value::as_str)
                    .or_else(|| m.get("licenseUrl").and_then(Value::as_str))
            })
            .map(str::to_string),
        name: meta_obj
            .and_then(|m| m.get("title"))
            .and_then(Value::as_str)
            .map(str::to_string),
        version: meta_obj
            .and_then(|m| m.get("version"))
            .and_then(Value::as_str)
            .map(str::to_string),
    };
    let spec_version = ext
        .get("exporterVersion")
        .and_then(Value::as_str)
        .map(str::to_string);

    // 0.x humanoid: `humanoidBone` is a list of {bone, node}; some
    // exporters emit a `humanoidBones` map instead. Accept either.
    let mut bones: BTreeMap<String, usize> = BTreeMap::new();
    let mut found_humanoid = false;
    if let Some(list) = ext.get("humanoidBone").and_then(Value::as_array) {
        found_humanoid = true;
        for entry in list {
            let bone = entry
                .get("bone")
                .and_then(Value::as_str)
                .ok_or_else(|| {
                    FactoryError::InvalidGlb("VRM humanoidBone entry missing bone".to_string())
                })?
                .to_string();
            let node = entry.get("node").and_then(Value::as_u64).ok_or_else(|| {
                FactoryError::InvalidGlb(format!("VRM humanoidBone.{bone} missing node index"))
            })? as usize;
            check_node_index(node, node_count, &format!("humanoidBone.{bone}"))?;
            bones.insert(bone, node);
        }
    } else if let Some(map) = ext.get("humanoidBones").and_then(Value::as_object) {
        found_humanoid = true;
        for (bone, entry) in map {
            let node = entry.get("node").and_then(Value::as_u64).ok_or_else(|| {
                FactoryError::InvalidGlb(format!("VRM humanoidBones.{bone} missing node index"))
            })? as usize;
            check_node_index(node, node_count, &format!("humanoidBones.{bone}"))?;
            bones.insert(bone.clone(), node);
        }
    }
    if !found_humanoid {
        return Err(FactoryError::InvalidGlb(
            "VRM extension missing humanoidBone entries".to_string(),
        ));
    }
    if bones.is_empty() {
        return Err(FactoryError::InvalidGlb(
            "VRM humanoid bone map is empty".to_string(),
        ));
    }

    // 0.x expressions: blendShapeMaster.blendShapeGroups, each with a
    // `name`, an optional `presetName`, and `binds` shaped
    // {mesh, index, weight} (mesh index, not node index — validated
    // against the expression's own shape only).
    let mut preset: Vec<VrmExpression> = Vec::new();
    let mut custom: Vec<VrmExpression> = Vec::new();
    if let Some(groups) = ext
        .get("blendShapeMaster")
        .and_then(|m| m.get("blendShapeGroups"))
        .and_then(Value::as_array)
    {
        for group in groups {
            let name = group
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            if name.is_empty() {
                continue;
            }
            let preset_name = group
                .get("presetName")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_string();
            let mut binds: Vec<VrmMorphBind> = Vec::new();
            if let Some(raw_binds) = group.get("binds").and_then(Value::as_array) {
                for bind in raw_binds {
                    let mesh = bind.get("mesh").and_then(Value::as_u64).unwrap_or(0) as usize;
                    let index = bind.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                    let weight_raw = bind.get("weight").and_then(Value::as_f64).unwrap_or(0.0);
                    if !weight_raw.is_finite() {
                        return Err(FactoryError::InvalidGlb(format!(
                            "blendShapeGroup {name} bind weight not finite"
                        )));
                    }
                    // 0.x weights are 0..=100; normalize to 0..=1.
                    binds.push(VrmMorphBind {
                        index,
                        node: mesh,
                        weight: weight_raw / 100.0,
                    });
                }
            }
            let expression = VrmExpression { binds, name };
            if preset_name == "unknown" || preset_name.is_empty() {
                custom.push(expression);
            } else {
                preset.push(expression);
            }
        }
    }
    preset.sort_by(|a, b| a.name.cmp(&b.name));
    custom.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(ParsedFlavor {
        bones,
        custom,
        meta,
        preset,
        spec_version,
    })
}

/// Wrap a factory GLB as a VRM 1.0 file: inject (or replace)
/// `extensions.VRMC_vrm` with the humanoid mapping, meta, and
/// expressions described in the module docs. Deterministic; the
/// binary chunk is copied byte-for-byte, so geometry is untouched.
pub fn wrap_as_vrm(glb_bytes: &[u8], meta: &WrapMeta) -> Result<Vec<u8>, FactoryError> {
    if glb_bytes.len() > MAX_GLB_BYTES {
        return Err(FactoryError::InvalidGlb(format!(
            "glb size {} exceeds bound {MAX_GLB_BYTES}",
            glb_bytes.len()
        )));
    }
    if meta.name.is_empty() {
        return Err(FactoryError::BadParam(
            "vrm wrap requires a non-empty model name".to_string(),
        ));
    }
    let parts = parse_glb_parts(glb_bytes)?;
    let mut root = parts.json.clone();
    let root_obj = root
        .as_object_mut()
        .ok_or_else(|| FactoryError::InvalidGlb("glTF root is not an object".to_string()))?;
    if root_obj
        .get("asset")
        .and_then(|a| a.get("version"))
        .and_then(Value::as_str)
        != Some("2.0")
    {
        return Err(FactoryError::InvalidGlb(
            "asset.version is not \"2.0\"".to_string(),
        ));
    }

    // Node name -> index (factory node names are unique).
    let nodes = root_obj
        .get("nodes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut node_by_name: BTreeMap<String, usize> = BTreeMap::new();
    for (idx, node) in nodes.iter().enumerate() {
        if let Some(name) = node.get("name").and_then(Value::as_str) {
            node_by_name.entry(name.to_string()).or_insert(idx);
        }
    }

    // Humanoid mapping from the factory joint names.
    let mut human_bones = Map::new();
    for (vrm_bone, factory_joint) in bone_mapping() {
        match node_by_name.get(factory_joint) {
            Some(idx) => {
                human_bones.insert(vrm_bone.to_string(), json!({ "node": idx }));
            }
            None => {
                if REQUIRED_BONES.contains(&vrm_bone) {
                    return Err(FactoryError::BadParam(format!(
                        "factory joint {factory_joint} (VRM bone {vrm_bone}) missing from GLB nodes"
                    )));
                }
                // Optional bone with no factory joint: omit.
            }
        }
    }

    // Head mesh: the mesh carrying extras.targetNames, and the node
    // that references it.
    let meshes = root_obj
        .get("meshes")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    let mut target_names: Vec<String> = Vec::new();
    let mut head_mesh_idx: Option<usize> = None;
    for (idx, mesh) in meshes.iter().enumerate() {
        if let Some(names) = mesh
            .get("extras")
            .and_then(|e| e.get("targetNames"))
            .and_then(Value::as_array)
        {
            target_names = names
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect();
            head_mesh_idx = Some(idx);
            break;
        }
    }
    let head_mesh_idx = head_mesh_idx.ok_or_else(|| {
        FactoryError::BadParam("no mesh with extras.targetNames to bind expressions to".to_string())
    })?;
    let head_node = nodes
        .iter()
        .position(|node| node.get("mesh").and_then(Value::as_u64) == Some(head_mesh_idx as u64))
        .ok_or_else(|| {
            FactoryError::BadParam("no node references the morph-target mesh".to_string())
        })?;
    let target_index = |name: &str| -> Result<usize, FactoryError> {
        target_names
            .iter()
            .position(|candidate| candidate == name)
            .ok_or_else(|| {
                FactoryError::BadParam(format!(
                    "morph target {name} missing from extras.targetNames"
                ))
            })
    };
    let expression_entry = |name: &str| -> Result<Value, FactoryError> {
        let idx = target_index(name)?;
        Ok(json!({
            "morphTargetBinds": [
                { "index": idx, "node": head_node, "weight": 1.0 }
            ]
        }))
    };
    let mut preset = Map::new();
    for name in PRESET_EXPRESSIONS {
        preset.insert((*name).to_string(), expression_entry(name)?);
    }
    let mut custom = Map::new();
    for name in CUSTOM_EXPRESSIONS {
        custom.insert((*name).to_string(), expression_entry(name)?);
    }

    let vrmc = json!({
        "expressions": {
            "custom": Value::Object(custom),
            "preset": Value::Object(preset),
        },
        "humanoid": {
            "humanBones": Value::Object(human_bones),
        },
        "meta": {
            "authors": meta.authors,
            "licenseUrl": meta.license_url,
            "name": meta.name,
            "version": meta.version,
        },
        "specVersion": VRM_SPEC_VERSION,
    });

    let extensions = root_obj
        .entry("extensions")
        .or_insert_with(|| Value::Object(Map::new()));
    let extensions_obj = extensions
        .as_object_mut()
        .ok_or_else(|| FactoryError::InvalidGlb("extensions is not an object".to_string()))?;
    extensions_obj.insert("VRMC_vrm".to_string(), vrmc);

    let used = root_obj
        .entry("extensionsUsed")
        .or_insert_with(|| Value::Array(Vec::new()));
    let used_arr = used
        .as_array_mut()
        .ok_or_else(|| FactoryError::InvalidGlb("extensionsUsed is not an array".to_string()))?;
    if !used_arr
        .iter()
        .any(|entry| entry.as_str() == Some("VRMC_vrm"))
    {
        used_arr.push(Value::String("VRMC_vrm".to_string()));
    }

    build_glb(&root, &parts.bin)
}
