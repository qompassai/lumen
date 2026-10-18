//! GLB writer and fail-closed GLB reader.
//!
//! Contract summary:
//! - Writer: one mesh whose primitives are the base mesh parts, one skin
//!   whose joints mirror the canonical hierarchy, PBR materials (one per
//!   part), inverse bind matrices computed with real 4x4 matrix math
//!   (hierarchy composition, then a general inverse) — no shortcuts.
//! - Morph targets are supported now: targets carry position deltas for
//!   the `head` primitive, names ride in the primitive mesh's
//!   `extras.targetNames`, and `mesh.weights` is emitted as zeros.
//! - Output is deterministic: fixed construction order, JSON chunk
//!   space-padded to 4 bytes, BIN chunk zero-padded to 4 bytes.
//! - Reader: parse-back validation only. It checks the GLB header, chunk
//!   bounds, and JSON structure with checked arithmetic throughout and
//!   returns a structural summary; any truncation, bad magic, or
//!   oversized declared length is a typed error, never a panic.
//! - [`parse_glb_parts`] / [`build_glb`] expose the raw JSON document
//!   and binary chunk (and their deterministic re-serialization) so
//!   the VRM layer in `vrm.rs` reuses this parser instead of forking
//!   it; the summary reader is built on the same parts parser.

#![forbid(unsafe_code)]

use serde::Serialize;
use serde_json::{Map, Value};

use crate::base::{BaseMesh, Joint};
use crate::{FactoryError, MAX_GLB_BYTES, MAX_MORPH_TARGETS};

const GLB_MAGIC: u32 = 0x4654_6C67; // "glTF"
const GLB_VERSION: u32 = 2;
const CHUNK_JSON: u32 = 0x4E4F_534A; // "JSON"
const CHUNK_BIN: u32 = 0x004E_4942; // "BIN\0"

/// One morph target: position deltas for the `head` primitive.
#[derive(Clone, Debug, PartialEq)]
pub struct MorphTarget {
    /// Deltas, one per head vertex, in construction order.
    pub deltas: Vec<[f32; 3]>,
    /// Target name (emitted in `extras.targetNames`).
    pub name: String,
}

/// Structural summary returned by the GLB reader.
#[derive(Clone, Debug, PartialEq)]
pub struct GlbSummary {
    /// Total triangle index count across primitives.
    pub index_count: usize,
    /// Number of joints in the first skin (0 when no skin).
    pub joint_count: usize,
    /// Number of meshes.
    pub mesh_count: usize,
    /// Number of nodes.
    pub node_count: usize,
    /// Number of primitives across all meshes.
    pub primitive_count: usize,
    /// Morph target names from `extras.targetNames`.
    pub target_names: Vec<String>,
    /// Total vertex count across primitives.
    pub vertex_count: usize,
    /// Maximum per-vertex skin weight sum observed (0 when unskinned).
    pub weight_sum_max: f32,
    /// Minimum per-vertex skin weight sum observed (0 when unskinned).
    pub weight_sum_min: f32,
}

type Mat4 = [[f32; 4]; 4]; // column-major: m[col][row]

fn mat4_identity() -> Mat4 {
    let mut m = [[0.0; 4]; 4];
    m[0][0] = 1.0;
    m[1][1] = 1.0;
    m[2][2] = 1.0;
    m[3][3] = 1.0;
    m
}

fn mat4_translation(t: [f32; 3]) -> Mat4 {
    let mut m = mat4_identity();
    m[3][0] = t[0];
    m[3][1] = t[1];
    m[3][2] = t[2];
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

/// General 4x4 inverse (adjugate method). Fails closed on singularity.
fn mat4_inverse(m: &Mat4) -> Result<Mat4, FactoryError> {
    // Convert to row-major working array a[row][col] = m[col][row].
    let mut a = [[0.0f32; 4]; 4];
    for (row, a_row) in a.iter_mut().enumerate() {
        for (col, value) in a_row.iter_mut().enumerate() {
            *value = m[col][row];
        }
    }
    let det = det4(&a);
    if !det.is_finite() || det.abs() < 1e-12 {
        return Err(FactoryError::MeshInvalid(
            "joint global transform is singular".to_string(),
        ));
    }
    let mut inv_row = [[0.0f32; 4]; 4];
    for (row, inv_row_values) in inv_row.iter_mut().enumerate() {
        for (col, value) in inv_row_values.iter_mut().enumerate() {
            // Cofactor of a[col][row] gives the transposed adjugate directly.
            let minor = minor3(&a, col, row);
            let sign = if (row + col) % 2 == 0 { 1.0 } else { -1.0 };
            *value = sign * det3(&minor) / det;
        }
    }
    let mut out = [[0.0f32; 4]; 4];
    for (col, out_column) in out.iter_mut().enumerate() {
        for (row, value) in out_column.iter_mut().enumerate() {
            *value = inv_row[row][col];
        }
    }
    Ok(out)
}

fn minor3(a: &[[f32; 4]; 4], skip_row: usize, skip_col: usize) -> [[f32; 3]; 3] {
    let mut out = [[0.0f32; 3]; 3];
    for (r, (_, row_values)) in a
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != skip_row)
        .enumerate()
    {
        for (c, (_, value)) in row_values
            .iter()
            .enumerate()
            .filter(|(index, _)| *index != skip_col)
            .enumerate()
        {
            out[r][c] = *value;
        }
    }
    out
}

fn det3(m: &[[f32; 3]; 3]) -> f32 {
    m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
}

fn det4(m: &[[f32; 4]; 4]) -> f32 {
    let mut det = 0.0f32;
    for col in 0..4 {
        let sign = if col % 2 == 0 { 1.0 } else { -1.0 };
        det += sign * m[0][col] * det3(&minor3(m, 0, col));
    }
    det
}

/// Global bind transform per joint, composed through the hierarchy from
/// local translations (world position minus parent world position).
fn global_bind_matrices(joints: &[Joint]) -> Result<Vec<Mat4>, FactoryError> {
    let mut globals: Vec<Option<Mat4>> = vec![None; joints.len()];
    // Table order is parent-before-child, so a single pass composes.
    for (idx, joint) in joints.iter().enumerate() {
        let local_pos = match joint.parent {
            Some(parent_name) => {
                let parent_idx = joints
                    .iter()
                    .position(|candidate| candidate.name == parent_name)
                    .ok_or_else(|| {
                        FactoryError::BadParam(format!("parent {parent_name} missing"))
                    })?;
                let parent_pos = joints[parent_idx].position;
                [
                    joint.position[0] - parent_pos[0],
                    joint.position[1] - parent_pos[1],
                    joint.position[2] - parent_pos[2],
                ]
            }
            None => joint.position,
        };
        let local = mat4_translation(local_pos);
        let global = match joint.parent {
            Some(parent_name) => {
                let parent_idx = joints
                    .iter()
                    .position(|candidate| candidate.name == parent_name)
                    .ok_or_else(|| {
                        FactoryError::BadParam(format!("parent {parent_name} missing"))
                    })?;
                let parent_global = globals[parent_idx].ok_or_else(|| {
                    FactoryError::MeshInvalid("parent composed after child".to_string())
                })?;
                mat4_mul(&parent_global, &local)
            }
            None => local,
        };
        globals[idx] = Some(global);
    }
    globals
        .into_iter()
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| FactoryError::MeshInvalid("uncomposed joint transform".to_string()))
}

// --- glTF JSON model (field order is the emitted order) ---

#[derive(Serialize)]
struct GltfJson {
    accessors: Vec<Accessor>,
    asset: Asset,
    #[serde(rename = "bufferViews")]
    buffer_views: Vec<BufferView>,
    buffers: Vec<BufferDef>,
    materials: Vec<Material>,
    meshes: Vec<MeshDef>,
    nodes: Vec<NodeDef>,
    scenes: Vec<SceneDef>,
    skins: Vec<SkinDef>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    images: Vec<ImageDef>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    samplers: Vec<SamplerDef>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    textures: Vec<TextureDef>,
}

#[derive(Serialize)]
struct ImageDef {
    #[serde(rename = "bufferView")]
    buffer_view: usize,
    #[serde(rename = "mimeType")]
    mime_type: String,
    name: String,
}

#[derive(Serialize)]
struct SamplerDef {
    #[serde(rename = "magFilter")]
    mag_filter: u32,
    #[serde(rename = "minFilter")]
    min_filter: u32,
    #[serde(rename = "wrapS")]
    wrap_s: u32,
    #[serde(rename = "wrapT")]
    wrap_t: u32,
}

#[derive(Serialize)]
struct TextureDef {
    sampler: usize,
    source: usize,
}

#[derive(Serialize)]
struct TexRef {
    index: usize,
    #[serde(rename = "texCoord")]
    tex_coord: usize,
}

#[derive(Serialize)]
struct Accessor {
    #[serde(rename = "bufferView")]
    buffer_view: usize,
    #[serde(rename = "byteOffset", skip_serializing_if = "is_zero_usize")]
    byte_offset: usize,
    #[serde(rename = "componentType")]
    component_type: u32,
    count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    max: Option<Vec<f32>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    min: Option<Vec<f32>>,
    #[serde(rename = "type")]
    accessor_type: String,
}

fn is_zero_usize(v: &usize) -> bool {
    *v == 0
}

#[derive(Serialize)]
struct Asset {
    generator: String,
    version: String,
}

#[derive(Serialize)]
struct BufferDef {
    #[serde(rename = "byteLength")]
    byte_length: usize,
}

#[derive(Serialize)]
struct BufferView {
    buffer: usize,
    #[serde(rename = "byteLength")]
    byte_length: usize,
    #[serde(rename = "byteOffset")]
    byte_offset: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    target: Option<u32>,
}

#[derive(Serialize)]
struct Material {
    #[serde(rename = "doubleSided")]
    double_sided: bool,
    name: String,
    #[serde(rename = "pbrMetallicRoughness")]
    pbr: Pbr,
}

#[derive(Serialize)]
struct Pbr {
    #[serde(rename = "baseColorFactor")]
    base_color_factor: [f32; 4],
    #[serde(rename = "baseColorTexture", skip_serializing_if = "Option::is_none")]
    base_color_texture: Option<TexRef>,
    #[serde(rename = "metallicFactor")]
    metallic_factor: f32,
    #[serde(rename = "roughnessFactor")]
    roughness_factor: f32,
}

#[derive(Serialize)]
struct MeshDef {
    #[serde(skip_serializing_if = "Option::is_none")]
    extras: Option<Value>,
    name: String,
    primitives: Vec<PrimitiveDef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    weights: Option<Vec<f32>>,
}

#[derive(Serialize)]
struct PrimitiveDef {
    attributes: Map<String, Value>,
    indices: usize,
    material: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    targets: Option<Vec<Map<String, Value>>>,
}

#[derive(Serialize)]
struct NodeDef {
    #[serde(skip_serializing_if = "Option::is_none")]
    children: Option<Vec<usize>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    mesh: Option<usize>,
    name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    skin: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    translation: Option<[f32; 3]>,
}

#[derive(Serialize)]
struct SceneDef {
    name: String,
    nodes: Vec<usize>,
}

#[derive(Serialize)]
struct SkinDef {
    #[serde(rename = "inverseBindMatrices")]
    inverse_bind_matrices: usize,
    joints: Vec<usize>,
    name: String,
    skeleton: usize,
}

struct BinBuilder {
    data: Vec<u8>,
    views: Vec<BufferView>,
}

impl BinBuilder {
    fn new() -> Self {
        Self {
            data: Vec::new(),
            views: Vec::new(),
        }
    }
    fn align(&mut self) {
        while !self.data.len().is_multiple_of(4) {
            self.data.push(0);
        }
    }
    fn push(&mut self, bytes: &[u8], target: Option<u32>) -> usize {
        self.align();
        let offset = self.data.len();
        self.data.extend_from_slice(bytes);
        self.views.push(BufferView {
            buffer: 0,
            byte_length: bytes.len(),
            byte_offset: offset,
            target,
        });
        self.views.len() - 1
    }
}

fn f32_bytes(values: &[[f32; 3]]) -> Vec<u8> {
    let mut out = Vec::with_capacity(values.len() * 12);
    for v in values {
        for component in v {
            out.extend_from_slice(&component.to_le_bytes());
        }
    }
    out
}

fn min_max_3(values: &[[f32; 3]]) -> (Vec<f32>, Vec<f32>) {
    let mut min = [f32::INFINITY; 3];
    let mut max = [f32::NEG_INFINITY; 3];
    for v in values {
        for axis in 0..3 {
            min[axis] = min[axis].min(v[axis]);
            max[axis] = max[axis].max(v[axis]);
        }
    }
    (min.to_vec(), max.to_vec())
}

/// Serialize the base mesh (plus optional morph targets for the head
/// primitive) into a deterministic GLB byte vector.
pub fn write_glb(mesh: &BaseMesh, morph_targets: &[MorphTarget]) -> Result<Vec<u8>, FactoryError> {
    write_glb_impl(mesh, morph_targets, None, None)
}

/// Serialize like [`write_glb`], additionally embedding a PNG texture
/// atlas as the shared `baseColorTexture` of every material. When a
/// texture is present the per-material `baseColorFactor` is white so
/// the painted atlas colors render unmodified. With `None` this is
/// exactly [`write_glb`].
pub fn write_glb_with_texture(
    mesh: &BaseMesh,
    morph_targets: &[MorphTarget],
    texture_png: Option<&[u8]>,
) -> Result<Vec<u8>, FactoryError> {
    write_glb_impl(mesh, morph_targets, texture_png, None)
}

/// Serialize like [`write_glb_with_texture`], additionally writing a
/// ramp/toon configuration into the mesh `extras` under the key
/// [`crate::toon::TOON_EXTRAS_KEY`] when `toon` is `Some`. The config
/// is validated before anything is written. With `None` this is
/// exactly [`write_glb_with_texture`], byte-for-byte.
pub fn write_glb_with_toon(
    mesh: &BaseMesh,
    morph_targets: &[MorphTarget],
    texture_png: Option<&[u8]>,
    toon: Option<&crate::toon::ToonConfig>,
) -> Result<Vec<u8>, FactoryError> {
    write_glb_impl(mesh, morph_targets, texture_png, toon)
}

fn write_glb_impl(
    mesh: &BaseMesh,
    morph_targets: &[MorphTarget],
    texture_png: Option<&[u8]>,
    toon: Option<&crate::toon::ToonConfig>,
) -> Result<Vec<u8>, FactoryError> {
    if let Some(config) = toon {
        config.validate()?;
    }
    if morph_targets.len() > MAX_MORPH_TARGETS {
        return Err(FactoryError::BadParam(format!(
            "morph target count {} exceeds bound",
            morph_targets.len()
        )));
    }
    let head_len = mesh
        .parts
        .iter()
        .find(|part| part.name == "head")
        .map(|part| part.vertices.len())
        .ok_or_else(|| FactoryError::MeshInvalid("head part missing".to_string()))?;
    for target in morph_targets {
        if target.deltas.len() != head_len {
            return Err(FactoryError::BadParam(format!(
                "morph target {} has {} deltas, head has {head_len} vertices",
                target.name,
                target.deltas.len()
            )));
        }
        if !target
            .deltas
            .iter()
            .all(|d| d.iter().all(|v| v.is_finite()))
        {
            return Err(FactoryError::BadParam(format!(
                "morph target {} contains non-finite deltas",
                target.name
            )));
        }
    }

    let mut bin = BinBuilder::new();
    let mut accessors: Vec<Accessor> = Vec::new();
    let mut add_accessor = |bin: &mut BinBuilder,
                            bytes: &[u8],
                            component_type: u32,
                            count: usize,
                            accessor_type: &str,
                            min: Option<Vec<f32>>,
                            max: Option<Vec<f32>>,
                            target: Option<u32>|
     -> usize {
        let view = bin.push(bytes, target);
        accessors.push(Accessor {
            buffer_view: view,
            byte_offset: 0,
            component_type,
            count,
            max,
            min,
            accessor_type: accessor_type.to_string(),
        });
        accessors.len() - 1
    };

    const ARRAY_BUFFER: u32 = 34962;
    const ELEMENT_ARRAY_BUFFER: u32 = 34963;
    const CT_FLOAT: u32 = 5126;
    const CT_UNSIGNED_SHORT: u32 = 5123;
    const CT_UNSIGNED_INT: u32 = 5125;

    // Per-part accessors, in part construction order.
    let mut primitives: Vec<PrimitiveDef> = Vec::new();
    let mut materials: Vec<Material> = Vec::new();
    let part_colors: [(&str, [f32; 4]); 9] = [
        ("arm_left", [0.96, 0.80, 0.70, 1.0]),
        ("arm_right", [0.96, 0.80, 0.70, 1.0]),
        ("body", [0.96, 0.80, 0.70, 1.0]),
        ("hair", [0.25, 0.16, 0.12, 1.0]),
        ("head", [0.97, 0.82, 0.72, 1.0]),
        ("leg_left", [0.96, 0.80, 0.70, 1.0]),
        ("leg_right", [0.96, 0.80, 0.70, 1.0]),
        ("outfit_legs", [0.30, 0.32, 0.45, 1.0]),
        ("outfit_torso", [0.55, 0.25, 0.35, 1.0]),
    ];
    for part in &mesh.parts {
        let positions: Vec<[f32; 3]> = part.vertices.iter().map(|v| v.position).collect();
        let normals: Vec<[f32; 3]> = part.vertices.iter().map(|v| v.normal).collect();
        let (pmin, pmax) = min_max_3(&positions);
        let pos_acc = add_accessor(
            &mut bin,
            &f32_bytes(&positions),
            CT_FLOAT,
            positions.len(),
            "VEC3",
            Some(pmin),
            Some(pmax),
            Some(ARRAY_BUFFER),
        );
        let nor_acc = add_accessor(
            &mut bin,
            &f32_bytes(&normals),
            CT_FLOAT,
            normals.len(),
            "VEC3",
            None,
            None,
            Some(ARRAY_BUFFER),
        );
        // TEXCOORD_0 as flat f32 pairs.
        let mut uv_bytes = Vec::with_capacity(part.vertices.len() * 8);
        for vertex in &part.vertices {
            uv_bytes.extend_from_slice(&vertex.uv[0].to_le_bytes());
            uv_bytes.extend_from_slice(&vertex.uv[1].to_le_bytes());
        }
        let uv_acc = add_accessor(
            &mut bin,
            &uv_bytes,
            CT_FLOAT,
            part.vertices.len(),
            "VEC2",
            None,
            None,
            Some(ARRAY_BUFFER),
        );
        // JOINTS_0 as vec4 u16, WEIGHTS_0 as vec4 f32.
        let mut joint_bytes = Vec::with_capacity(part.vertices.len() * 8);
        for vertex in &part.vertices {
            for joint in vertex.joints {
                joint_bytes.extend_from_slice(&joint.to_le_bytes());
            }
        }
        let joint_acc = add_accessor(
            &mut bin,
            &joint_bytes,
            CT_UNSIGNED_SHORT,
            part.vertices.len(),
            "VEC4",
            None,
            None,
            Some(ARRAY_BUFFER),
        );
        let mut weight_bytes = Vec::with_capacity(part.vertices.len() * 16);
        for vertex in &part.vertices {
            for w in vertex.weights {
                weight_bytes.extend_from_slice(&w.to_le_bytes());
            }
        }
        let weight_acc = add_accessor(
            &mut bin,
            &weight_bytes,
            CT_FLOAT,
            part.vertices.len(),
            "VEC4",
            None,
            None,
            Some(ARRAY_BUFFER),
        );
        let mut index_bytes = Vec::with_capacity(part.indices.len() * 4);
        for idx in &part.indices {
            index_bytes.extend_from_slice(&idx.to_le_bytes());
        }
        let idx_acc = add_accessor(
            &mut bin,
            &index_bytes,
            CT_UNSIGNED_INT,
            part.indices.len(),
            "SCALAR",
            None,
            None,
            Some(ELEMENT_ARRAY_BUFFER),
        );

        let mut attributes = Map::new();
        attributes.insert("JOINTS_0".to_string(), Value::from(joint_acc));
        attributes.insert("NORMAL".to_string(), Value::from(nor_acc));
        attributes.insert("POSITION".to_string(), Value::from(pos_acc));
        attributes.insert("TEXCOORD_0".to_string(), Value::from(uv_acc));
        attributes.insert("WEIGHTS_0".to_string(), Value::from(weight_acc));

        // Morph targets only on the head primitive.
        let targets = if part.name == "head" && !morph_targets.is_empty() {
            let mut list: Vec<Map<String, Value>> = Vec::new();
            for target in morph_targets {
                let (dmin, dmax) = min_max_3(&target.deltas);
                let delta_acc = add_accessor(
                    &mut bin,
                    &f32_bytes(&target.deltas),
                    CT_FLOAT,
                    target.deltas.len(),
                    "VEC3",
                    Some(dmin),
                    Some(dmax),
                    Some(ARRAY_BUFFER),
                );
                let mut map = Map::new();
                map.insert("POSITION".to_string(), Value::from(delta_acc));
                list.push(map);
            }
            Some(list)
        } else {
            None
        };

        let color = part_colors
            .iter()
            .find(|(name, _)| *name == part.name)
            .map(|(_, color)| *color)
            .unwrap_or([0.8, 0.8, 0.8, 1.0]);
        let (factor, tex_ref) = if texture_png.is_some() {
            (
                [1.0, 1.0, 1.0, 1.0],
                Some(TexRef {
                    index: 0,
                    tex_coord: 0,
                }),
            )
        } else {
            (color, None)
        };
        materials.push(Material {
            double_sided: true,
            name: part.name.clone(),
            pbr: Pbr {
                base_color_factor: factor,
                base_color_texture: tex_ref,
                metallic_factor: 0.0,
                roughness_factor: 0.85,
            },
        });
        primitives.push(PrimitiveDef {
            attributes,
            indices: idx_acc,
            material: materials.len() - 1,
            targets,
        });
    }

    // Inverse bind matrices via real matrix math.
    let globals = global_bind_matrices(&mesh.joints)?;
    let mut ibm_bytes = Vec::with_capacity(globals.len() * 64);
    for global in &globals {
        let inverse = mat4_inverse(global)?;
        // glTF stores matrices column-major; the array is already
        // laid out [column][row], so plain iteration is that order.
        for column in &inverse {
            for value in column {
                ibm_bytes.extend_from_slice(&value.to_le_bytes());
            }
        }
    }
    let ibm_acc = add_accessor(
        &mut bin,
        &ibm_bytes,
        CT_FLOAT,
        globals.len(),
        "MAT4",
        None,
        None,
        None,
    );

    // Nodes: joint nodes first (index = joint index), then the mesh node,
    // then the scene root.
    let joint_count = mesh.joints.len();
    let mesh_node_idx = joint_count;
    let root_idx = joint_count + 1;
    let mut nodes: Vec<NodeDef> = Vec::new();
    for (idx, joint) in mesh.joints.iter().enumerate() {
        let children: Vec<usize> = mesh
            .joints
            .iter()
            .enumerate()
            .filter(|(_, candidate)| candidate.parent == Some(joint.name))
            .map(|(child_idx, _)| child_idx)
            .collect();
        let local = match joint.parent {
            Some(parent_name) => {
                let parent = mesh
                    .joints
                    .iter()
                    .find(|candidate| candidate.name == parent_name)
                    .ok_or_else(|| {
                        FactoryError::BadParam(format!("parent {parent_name} missing"))
                    })?;
                [
                    joint.position[0] - parent.position[0],
                    joint.position[1] - parent.position[1],
                    joint.position[2] - parent.position[2],
                ]
            }
            None => joint.position,
        };
        let _ = idx;
        nodes.push(NodeDef {
            children: if children.is_empty() {
                None
            } else {
                Some(children)
            },
            mesh: None,
            name: joint.name.to_string(),
            skin: None,
            translation: Some(local),
        });
    }
    nodes.push(NodeDef {
        children: None,
        mesh: Some(0),
        name: "BaseMesh".to_string(),
        skin: Some(0),
        translation: None,
    });
    nodes.push(NodeDef {
        children: Some(vec![0, mesh_node_idx]),
        mesh: None,
        name: "Root".to_string(),
        skin: None,
        translation: None,
    });

    let target_names: Vec<String> = morph_targets.iter().map(|t| t.name.clone()).collect();
    // Mesh extras: morph target names (when any) and the ramp/toon
    // configuration (when a ramp build was requested). With neither,
    // extras stays absent — the pre-Phase-5 byte layout.
    let extras = {
        let mut map = Map::new();
        if !target_names.is_empty() {
            map.insert(
                "targetNames".to_string(),
                Value::Array(target_names.iter().cloned().map(Value::from).collect()),
            );
        }
        if let Some(config) = toon {
            map.insert(crate::toon::TOON_EXTRAS_KEY.to_string(), config.to_json());
        }
        if map.is_empty() {
            None
        } else {
            Some(Value::Object(map))
        }
    };
    let mesh_weights = if target_names.is_empty() {
        None
    } else {
        Some(vec![0.0; target_names.len()])
    };

    // Optional embedded texture atlas (PNG bytes ride in the BIN chunk).
    let (images, samplers, textures) = match texture_png {
        Some(png) => {
            let view = bin.push(png, None);
            (
                vec![ImageDef {
                    buffer_view: view,
                    mime_type: "image/png".to_string(),
                    name: "atlas".to_string(),
                }],
                vec![SamplerDef {
                    mag_filter: 9729,
                    min_filter: 9729,
                    wrap_s: 10497,
                    wrap_t: 10497,
                }],
                vec![TextureDef {
                    sampler: 0,
                    source: 0,
                }],
            )
        }
        None => (Vec::new(), Vec::new(), Vec::new()),
    };

    let gltf = GltfJson {
        accessors,
        asset: Asset {
            generator: "lumen-factory 0.1.0".to_string(),
            version: "2.0".to_string(),
        },
        buffer_views: bin.views,
        buffers: vec![BufferDef {
            byte_length: bin.data.len(),
        }],
        images,
        samplers,
        textures,
        materials,
        meshes: vec![MeshDef {
            extras,
            name: "BaseMesh".to_string(),
            primitives,
            weights: mesh_weights,
        }],
        nodes,
        scenes: vec![SceneDef {
            name: "Scene".to_string(),
            nodes: vec![root_idx],
        }],
        skins: vec![SkinDef {
            inverse_bind_matrices: ibm_acc,
            joints: (0..joint_count).collect(),
            name: "Skin".to_string(),
            skeleton: 0,
        }],
    };

    let mut json_bytes = serde_json::to_vec(&gltf)?;
    while !json_bytes.len().is_multiple_of(4) {
        json_bytes.push(b' ');
    }
    let mut bin_data = bin.data;
    while !bin_data.len().is_multiple_of(4) {
        bin_data.push(0);
    }
    let total_len = 12 + 8 + json_bytes.len() + 8 + bin_data.len();
    let mut out = Vec::with_capacity(total_len);
    out.extend_from_slice(&GLB_MAGIC.to_le_bytes());
    out.extend_from_slice(&GLB_VERSION.to_le_bytes());
    out.extend_from_slice(&(total_len as u32).to_le_bytes());
    out.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(&CHUNK_JSON.to_le_bytes());
    out.extend_from_slice(&json_bytes);
    out.extend_from_slice(&(bin_data.len() as u32).to_le_bytes());
    out.extend_from_slice(&CHUNK_BIN.to_le_bytes());
    out.extend_from_slice(&bin_data);
    Ok(out)
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, FactoryError> {
    let slice = bytes
        .get(offset..offset + 4)
        .ok_or_else(|| FactoryError::InvalidGlb("truncated u32 read".to_string()))?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

/// Raw GLB parts: the parsed glTF JSON document plus the binary
/// chunk bytes (empty when the GLB carries no BIN chunk).
///
/// This is the minimal surface extension that lets sibling modules
/// (the VRM reader/wrapper in `vrm.rs`) work on the document without
/// forking the GLB parser: [`parse_glb_parts`] is the one parser and
/// [`parse_glb`] is built on it.
#[derive(Clone, Debug, PartialEq)]
pub struct GlbParts {
    /// Binary chunk bytes (the buffer data), verbatim.
    pub bin: Vec<u8>,
    /// Parsed glTF JSON document.
    pub json: Value,
}

/// Parse GLB bytes into their raw parts. Fail-closed on any
/// truncation, bad magic, version mismatch, or oversized declared
/// length — exactly the checks [`parse_glb`] has always applied.
pub fn parse_glb_parts(bytes: &[u8]) -> Result<GlbParts, FactoryError> {
    if bytes.len() > MAX_GLB_BYTES {
        return Err(FactoryError::InvalidGlb(format!(
            "glb size {} exceeds bound {MAX_GLB_BYTES}",
            bytes.len()
        )));
    }
    if bytes.len() < 12 {
        return Err(FactoryError::InvalidGlb(
            "glb shorter than header".to_string(),
        ));
    }
    if read_u32(bytes, 0)? != GLB_MAGIC {
        return Err(FactoryError::InvalidGlb("bad glb magic".to_string()));
    }
    if read_u32(bytes, 4)? != GLB_VERSION {
        return Err(FactoryError::InvalidGlb(
            "unsupported glb version".to_string(),
        ));
    }
    let declared_len = read_u32(bytes, 8)? as usize;
    if declared_len != bytes.len() {
        return Err(FactoryError::InvalidGlb(format!(
            "declared length {declared_len} != actual {}",
            bytes.len()
        )));
    }
    // JSON chunk header.
    if bytes.len() < 20 {
        return Err(FactoryError::InvalidGlb(
            "missing JSON chunk header".to_string(),
        ));
    }
    let json_len = read_u32(bytes, 12)? as usize;
    if read_u32(bytes, 16)? != CHUNK_JSON {
        return Err(FactoryError::InvalidGlb(
            "first chunk is not JSON".to_string(),
        ));
    }
    let json_start = 20usize;
    let json_end = json_start
        .checked_add(json_len)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| FactoryError::InvalidGlb("JSON chunk overruns file".to_string()))?;
    let json_slice = &bytes[json_start..json_end];
    let root: Value = serde_json::from_slice(json_slice)
        .map_err(|e| FactoryError::InvalidGlb(format!("bad glTF JSON: {e}")))?;

    // BIN chunk (optional at this layer, bounds-checked when present).
    let mut bin: Vec<u8> = Vec::new();
    if json_end < bytes.len() {
        if json_end + 8 > bytes.len() {
            return Err(FactoryError::InvalidGlb(
                "truncated BIN chunk header".to_string(),
            ));
        }
        let bin_len = read_u32(bytes, json_end)? as usize;
        if read_u32(bytes, json_end + 4)? != CHUNK_BIN {
            return Err(FactoryError::InvalidGlb(
                "second chunk is not BIN".to_string(),
            ));
        }
        let bin_start = json_end + 8;
        let bin_end = bin_start
            .checked_add(bin_len)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| FactoryError::InvalidGlb("BIN chunk overruns file".to_string()))?;
        bin = bytes[bin_start..bin_end].to_vec();
    }
    Ok(GlbParts { bin, json: root })
}

/// Serialize a glTF JSON document plus a binary chunk into GLB
/// bytes: the inverse of [`parse_glb_parts`], using the writer's
/// deterministic framing (JSON space-padded to 4 bytes, BIN
/// zero-padded to 4 bytes). The `bin` bytes are emitted verbatim,
/// so a parse -> modify-JSON-only -> build round trip leaves every
/// buffer byte untouched.
pub fn build_glb(json: &Value, bin: &[u8]) -> Result<Vec<u8>, FactoryError> {
    let mut json_bytes = serde_json::to_vec(json)?;
    while !json_bytes.len().is_multiple_of(4) {
        json_bytes.push(b' ');
    }
    let mut bin_data = bin.to_vec();
    while !bin_data.len().is_multiple_of(4) {
        bin_data.push(0);
    }
    let total_len = 12 + 8 + json_bytes.len() + 8 + bin_data.len();
    let mut out = Vec::with_capacity(total_len);
    out.extend_from_slice(&GLB_MAGIC.to_le_bytes());
    out.extend_from_slice(&GLB_VERSION.to_le_bytes());
    out.extend_from_slice(&(total_len as u32).to_le_bytes());
    out.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    out.extend_from_slice(&CHUNK_JSON.to_le_bytes());
    out.extend_from_slice(&json_bytes);
    out.extend_from_slice(&(bin_data.len() as u32).to_le_bytes());
    out.extend_from_slice(&CHUNK_BIN.to_le_bytes());
    out.extend_from_slice(&bin_data);
    Ok(out)
}

/// Parse GLB bytes and return a structural summary. Fail-closed on any
/// truncation, bad magic, version mismatch, or oversized declared length.
pub fn parse_glb(bytes: &[u8]) -> Result<GlbSummary, FactoryError> {
    let parts = parse_glb_parts(bytes)?;
    let root: &Value = &parts.json;
    let bin_slice: &[u8] = &parts.bin;

    let get_array = |key: &str| -> Vec<Value> {
        root.get(key)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    };
    let nodes = get_array("nodes");
    let meshes = get_array("meshes");
    let skins = get_array("skins");
    let accessors = get_array("accessors");
    let buffer_views = get_array("bufferViews");

    let mut primitive_count = 0usize;
    let mut vertex_count = 0usize;
    let mut index_count = 0usize;
    let mut target_names: Vec<String> = Vec::new();
    let mut weight_sum_min = f32::INFINITY;
    let mut weight_sum_max = 0.0f32;
    let mut saw_weights = false;

    // Helper: read an accessor's f32/u16 data from the BIN chunk.
    let accessor_f32 = |accessor_idx: usize| -> Result<Vec<f32>, FactoryError> {
        let accessor = accessors
            .get(accessor_idx)
            .ok_or_else(|| FactoryError::InvalidGlb("accessor index out of range".to_string()))?;
        let view_idx = accessor
            .get("bufferView")
            .and_then(Value::as_u64)
            .ok_or_else(|| FactoryError::InvalidGlb("accessor missing bufferView".to_string()))?
            as usize;
        let view = buffer_views
            .get(view_idx)
            .ok_or_else(|| FactoryError::InvalidGlb("bufferView index out of range".to_string()))?;
        let byte_offset = view.get("byteOffset").and_then(Value::as_u64).unwrap_or(0) as usize;
        let byte_length = view
            .get("byteLength")
            .and_then(Value::as_u64)
            .ok_or_else(|| FactoryError::InvalidGlb("bufferView missing byteLength".to_string()))?
            as usize;
        let component_type = accessor
            .get("componentType")
            .and_then(Value::as_u64)
            .unwrap_or(0);
        let count = accessor.get("count").and_then(Value::as_u64).unwrap_or(0) as usize;
        let components = match accessor.get("type").and_then(Value::as_str) {
            Some("SCALAR") => 1usize,
            Some("VEC2") => 2,
            Some("VEC3") => 3,
            Some("VEC4") => 4,
            Some("MAT4") => 16,
            _ => 1,
        };
        let end = byte_offset
            .checked_add(byte_length)
            .filter(|end| *end <= bin_slice.len())
            .ok_or_else(|| FactoryError::InvalidGlb("bufferView overruns BIN".to_string()))?;
        let slice = &bin_slice[byte_offset..end];
        let mut out: Vec<f32> = Vec::new();
        match component_type {
            5126 => {
                if !slice.len().is_multiple_of(4) {
                    return Err(FactoryError::InvalidGlb("f32 view misaligned".to_string()));
                }
                for chunk in slice.as_chunks::<4>().0 {
                    out.push(f32::from_le_bytes(*chunk));
                }
            }
            5123 => {
                // JOINTS data: expand to f32 for uniform handling.
                for chunk in slice.as_chunks::<2>().0 {
                    out.push(f32::from(u16::from_le_bytes(*chunk)));
                }
            }
            5125 => {
                for chunk in slice.as_chunks::<4>().0 {
                    out.push(u32::from_le_bytes(*chunk) as f32);
                }
            }
            _ => {
                return Err(FactoryError::InvalidGlb(format!(
                    "unsupported componentType {component_type}"
                )));
            }
        }
        let expected = count.saturating_mul(components);
        if out.len() < expected {
            return Err(FactoryError::InvalidGlb(
                "accessor data shorter than declared count".to_string(),
            ));
        }
        out.truncate(expected);
        Ok(out)
    };

    for mesh in &meshes {
        if let Some(names) = mesh
            .get("extras")
            .and_then(|e| e.get("targetNames"))
            .and_then(Value::as_array)
        {
            for name in names {
                if let Some(text) = name.as_str() {
                    target_names.push(text.to_string());
                }
            }
        }
        let primitives = mesh
            .get("primitives")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        primitive_count += primitives.len();
        for primitive in &primitives {
            let attributes = primitive.get("attributes");
            if let Some(pos_idx) = attributes
                .and_then(|a| a.get("POSITION"))
                .and_then(Value::as_u64)
                && let Some(accessor) = accessors.get(pos_idx as usize)
            {
                vertex_count += accessor.get("count").and_then(Value::as_u64).unwrap_or(0) as usize;
            }
            if let Some(idx) = primitive.get("indices").and_then(Value::as_u64)
                && let Some(accessor) = accessors.get(idx as usize)
            {
                index_count += accessor.get("count").and_then(Value::as_u64).unwrap_or(0) as usize;
            }
            if let Some(w_idx) = attributes
                .and_then(|a| a.get("WEIGHTS_0"))
                .and_then(Value::as_u64)
            {
                let weights = accessor_f32(w_idx as usize)?;
                for vertex_weights in weights.as_chunks::<4>().0 {
                    let sum: f32 = vertex_weights.iter().sum();
                    saw_weights = true;
                    weight_sum_min = weight_sum_min.min(sum);
                    weight_sum_max = weight_sum_max.max(sum);
                }
            }
        }
    }

    let joint_count = skins
        .first()
        .and_then(|skin| skin.get("joints"))
        .and_then(Value::as_array)
        .map_or(0, Vec::len);

    Ok(GlbSummary {
        index_count,
        joint_count,
        mesh_count: meshes.len(),
        node_count: nodes.len(),
        primitive_count,
        target_names,
        vertex_count,
        weight_sum_max: if saw_weights { weight_sum_max } else { 0.0 },
        weight_sum_min: if saw_weights { weight_sum_min } else { 0.0 },
    })
}
