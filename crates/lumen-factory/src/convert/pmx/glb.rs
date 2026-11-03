//! PMX → GLB emission (SPEC §5).
//!
//! Mapping summary (each rule is §5's, stated where it is applied):
//! - Left-handed PMX → right-handed glTF: Z negated on positions,
//!   normals and translation deltas; triangle winding reversed.
//! - Scale is profile data (`scale_to_meters`, default 0.08): it is
//!   applied from the profile and recorded in provenance extras,
//!   never silently hard-coded.
//! - One mesh per PMX material span; all span meshes share the
//!   model's attribute accessors (the full vertex set) and the
//!   global morph-target table, so every primitive carries the
//!   same targets in the same order (the glTF rule).
//! - BDEF1/2/4 → JOINTS_0/WEIGHTS_0; BDEF4/QDEF weights are
//!   normalised with the correction counted; SDEF and QDEF degrade
//!   to linear blend with ledgers, and SDEF C/R0/R1 vectors are
//!   preserved verbatim in provenance extras.
//! - Bones → skin joints; IK / inherit / fixed-axis /
//!   external-parent detail is preserved verbatim in a
//!   `pmx_provenance` extras block on the skin.
//! - Vertex morphs → POSITION targets, UV morphs → TEXCOORD_0
//!   targets; bone/material morphs are ledgered unconverted with
//!   offsets preserved; group/flip morphs become profile recipes.
//! - Textures embed only when resolved by the caller; unresolved
//!   paths stay as named external references plus a ledger.
//! - Determinism: struct-ordered JSON, file-order arrays, JSON
//!   chunk space-padded and BIN chunk zero-padded to 4 bytes (the
//!   estate GLB framing, matching `crate::gltf`), no timestamps.

use serde::Serialize;
use serde_json::{Map, Value, json};

use super::parse::{Deform, MorphOffset, PmxModel, ToonRef};
use super::profile::{
    MmdProfile, ProportionAssessment, assess_proportions, bone_target, fac_channel, morph_recipes,
};
use crate::FactoryError;

const GLB_MAGIC: u32 = 0x4654_6C67;
const GLB_VERSION: u32 = 2;
const CHUNK_JSON: u32 = 0x4E4F_534A;
const CHUNK_BIN: u32 = 0x004E_4942;
const CT_FLOAT: u32 = 5126;
const CT_UNSIGNED_SHORT: u32 = 5123;
const CT_UNSIGNED_INT: u32 = 5125;
const ARRAY_BUFFER: u32 = 34962;
const ELEMENT_ARRAY_BUFFER: u32 = 34963;
/// glTF client-support floor for simultaneous morphed attributes
/// (SPEC §5.4): conversions above it are flagged, not refused.
pub const MORPHED_ATTRIBUTE_FLOOR: usize = 8;

/// A texture file resolved by the caller at conversion time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTexture {
    /// PMX texture-table index this file belongs to.
    pub pmx_index: usize,
    /// File bytes (embedded verbatim when the mime is embeddable).
    pub bytes: Vec<u8>,
    /// MIME type ("image/png" / "image/jpeg" embed; others ledger).
    pub mime: String,
}

/// Conversion options: profile data, never hidden constants.
#[derive(Debug, Clone, PartialEq)]
pub struct ConvertOptions {
    /// PMX units → metres (profile `scale_to_meters`).
    pub scale_to_meters: f32,
    /// Textures that resolved at conversion time, in any order.
    pub textures: Vec<ResolvedTexture>,
    /// SHA-256 of the source PMX file (the census `file_sha256`),
    /// carried into provenance so Stage 2's `gltf-import/1` can
    /// anchor its ingest record to the censused source. `None`
    /// when the caller did not census first (ledgered as null).
    pub source_sha256: Option<String>,
}

impl Default for ConvertOptions {
    fn default() -> Self {
        ConvertOptions {
            scale_to_meters: MmdProfile::builtin().proportions.scale_to_meters,
            textures: Vec::new(),
            source_sha256: None,
        }
    }
}

/// One unconverted (ledgered) morph.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnconvertedMorph {
    /// Morph name (universal when present, else local).
    pub name: String,
    /// Census class name.
    pub morph_type: &'static str,
    /// Offset record count preserved in provenance.
    pub offset_count: usize,
}

/// One Fac_ mapping row for a morph target.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FacMapping {
    /// Morph name (universal when present, else local).
    pub morph: String,
    /// Canonical Fac_ channel, or `None` for explicit unmapped.
    pub channel: Option<String>,
}

/// The conversion report: every loss in the mapping, counted.
#[derive(Debug, Clone, PartialEq)]
pub struct ConversionReport {
    /// Vertex count (matches the census).
    pub vertex_count: usize,
    /// Triangle count (matches the census face count).
    pub triangle_count: usize,
    /// Mesh count (one per material span).
    pub mesh_count: usize,
    /// Skin joint count (matches the census bone count).
    pub bone_count: usize,
    /// Vertices whose BDEF4/QDEF weights needed normalisation.
    pub weight_corrections: usize,
    /// Vertices degraded from SDEF to BDEF2.
    pub sdef_vertices: usize,
    /// Vertices degraded from QDEF to linear blend.
    pub qdef_vertices: usize,
    /// Deform bone references outside the bone table (clamped to 0).
    pub invalid_bone_refs: usize,
    /// Morph targets emitted (vertex + UV morphs).
    pub morph_target_count: usize,
    /// Simultaneous morphed attributes (== morph target count here).
    pub morphed_attribute_count: usize,
    /// True when the morphed-attribute count exceeds the floor of 8.
    pub exceeds_morph_floor: bool,
    /// Bone/material/impulse morphs ledgered unconverted.
    pub unconverted_morphs: Vec<UnconvertedMorph>,
    /// Fac_ mapping for every morph target.
    pub fac_mapping: Vec<FacMapping>,
    /// Group/flip recipes (SPEC §6.3 sidecar).
    pub recipes: Vec<super::profile::MorphRecipe>,
    /// Texture paths that did not resolve (or did not embed).
    pub missing_textures: Vec<String>,
    /// Texture paths embedded into the GLB.
    pub embedded_textures: Vec<String>,
    /// Bone local names with no torogao-humanoid/1 target.
    pub unmapped_bones: Vec<String>,
    /// True when material spans did not exactly partition the
    /// surface run and were adjusted (see `convert` docs).
    pub spans_adjusted: bool,
    /// The scale applied, from profile data.
    pub scale_to_meters: f32,
    /// D-631 proportion assessment of the source model.
    pub proportions: ProportionAssessment,
}

/// Convert a retained PMX model to deterministic GLB bytes plus
/// the conversion report. Fails typed (`FactoryError`) on internal
/// inconsistencies; lossy steps never fail — they ledger.
pub fn convert(
    model: &PmxModel,
    options: &ConvertOptions,
) -> Result<(Vec<u8>, ConversionReport), FactoryError> {
    let profile = MmdProfile::builtin();
    let scale = options.scale_to_meters;
    if !scale.is_finite() || scale <= 0.0 {
        return Err(FactoryError::BadParam(
            "scale_to_meters must be positive and finite".to_string(),
        ));
    }
    let vertex_count = model.vertices.len();
    let bone_count = model.bones.len();

    // ---- Skinning weights (SPEC §5.2) ------------------------------
    let mut joints: Vec<[u16; 4]> = Vec::with_capacity(vertex_count);
    let mut weights: Vec<[f32; 4]> = Vec::with_capacity(vertex_count);
    let mut weight_corrections = 0usize;
    let mut invalid_bone_refs = 0usize;
    let mut sdef_records: Vec<(usize, &super::parse::Deform)> = Vec::new();
    let mut qdef_vertices = 0usize;
    for (index, vertex) in model.vertices.iter().enumerate() {
        let (j, w) = match &vertex.deform {
            Deform::Bdef1 { bone } => (
                [
                    sane_bone(*bone, bone_count, &mut invalid_bone_refs),
                    0,
                    0,
                    0,
                ],
                [1.0, 0.0, 0.0, 0.0],
            ),
            Deform::Bdef2 {
                bone1,
                bone2,
                weight,
            }
            | Deform::Sdef {
                bone1,
                bone2,
                weight,
                ..
            } => {
                if matches!(vertex.deform, Deform::Sdef { .. }) {
                    sdef_records.push((index, &vertex.deform));
                }
                (
                    [
                        sane_bone(*bone1, bone_count, &mut invalid_bone_refs),
                        sane_bone(*bone2, bone_count, &mut invalid_bone_refs),
                        0,
                        0,
                    ],
                    [*weight, 1.0 - *weight, 0.0, 0.0],
                )
            }
            Deform::Bdef4 {
                bones,
                weights: raw,
            }
            | Deform::Qdef {
                bones,
                weights: raw,
            } => {
                if matches!(vertex.deform, Deform::Qdef { .. }) {
                    qdef_vertices += 1;
                }
                let mut out = *raw;
                let sum: f32 = raw.iter().sum();
                if !sum.is_finite() || sum == 0.0 {
                    weight_corrections += 1;
                    out = [1.0, 0.0, 0.0, 0.0];
                } else if (sum - 1.0).abs() > 1e-6 {
                    weight_corrections += 1;
                    for slot in &mut out {
                        *slot /= sum;
                    }
                }
                (
                    [
                        sane_bone(bones[0], bone_count, &mut invalid_bone_refs),
                        sane_bone(bones[1], bone_count, &mut invalid_bone_refs),
                        sane_bone(bones[2], bone_count, &mut invalid_bone_refs),
                        sane_bone(bones[3], bone_count, &mut invalid_bone_refs),
                    ],
                    out,
                )
            }
        };
        joints.push(j);
        weights.push(w);
    }

    // ---- Converted geometry -----------------------------------------
    let positions: Vec<[f32; 3]> = model
        .vertices
        .iter()
        .map(|v| convert_point(v.position, scale))
        .collect();
    let normals: Vec<[f32; 3]> = model
        .vertices
        .iter()
        .map(|v| convert_direction(v.normal))
        .collect();
    let uvs: Vec<[f32; 2]> = model.vertices.iter().map(|v| v.uv).collect();

    // ---- Morph targets (global table, SPEC §5.4) --------------------
    let target_morphs: Vec<&super::parse::Morph> = model
        .morphs
        .iter()
        .filter(|m| m.morph_type == 1 || (3..=7).contains(&m.morph_type))
        .collect();
    let target_names: Vec<String> = target_morphs
        .iter()
        .map(|m| display_name(&m.name_local, &m.name_universal))
        .collect();
    let mut fac_mapping: Vec<FacMapping> = Vec::new();
    for morph in &target_morphs {
        fac_mapping.push(FacMapping {
            morph: display_name(&morph.name_local, &morph.name_universal),
            channel: fac_channel(&morph.name_local, &morph.name_universal).map(str::to_string),
        });
    }

    // ---- Material spans ----------------------------------------------
    let (spans, spans_adjusted) = material_spans(model);

    // ---- Builder: shared accessors -----------------------------------
    let mut builder = GlbBuilder::new();
    let pos_acc = add_vec3(&mut builder, &positions, true);
    let nor_acc = add_vec3(&mut builder, &normals, false);
    let uv_acc = {
        let mut bytes = Vec::with_capacity(uvs.len() * 8);
        for uv in &uvs {
            bytes.extend_from_slice(&uv[0].to_le_bytes());
            bytes.extend_from_slice(&uv[1].to_le_bytes());
        }
        builder.add_accessor(
            &bytes,
            CT_FLOAT,
            uvs.len(),
            "VEC2",
            None,
            Some(ARRAY_BUFFER),
        )
    };
    let joint_acc = {
        let mut bytes = Vec::with_capacity(joints.len() * 8);
        for joint_set in &joints {
            for joint in joint_set {
                bytes.extend_from_slice(&joint.to_le_bytes());
            }
        }
        builder.add_accessor(
            &bytes,
            CT_UNSIGNED_SHORT,
            joints.len(),
            "VEC4",
            None,
            Some(ARRAY_BUFFER),
        )
    };
    let weight_acc = {
        let mut bytes = Vec::with_capacity(weights.len() * 16);
        for weight_set in &weights {
            for w in weight_set {
                bytes.extend_from_slice(&w.to_le_bytes());
            }
        }
        builder.add_accessor(
            &bytes,
            CT_FLOAT,
            weights.len(),
            "VEC4",
            None,
            Some(ARRAY_BUFFER),
        )
    };
    let mut additional_accs: Vec<usize> = Vec::new();
    for k in 0..usize::from(model.globals.additional_vec4_count) {
        let values: Vec<[f32; 4]> = model
            .vertices
            .iter()
            .map(|v| v.additional.get(k).copied().unwrap_or([0.0; 4]))
            .collect();
        let mut bytes = Vec::with_capacity(values.len() * 16);
        for value in &values {
            for component in value {
                bytes.extend_from_slice(&component.to_le_bytes());
            }
        }
        additional_accs.push(builder.add_accessor(
            &bytes,
            CT_FLOAT,
            values.len(),
            "VEC4",
            None,
            Some(ARRAY_BUFFER),
        ));
    }
    // Morph target accessors, in global-table order.
    let mut target_accs: Vec<(bool, usize)> = Vec::new(); // (is_position, accessor)
    for morph in &target_morphs {
        if morph.morph_type == 1 {
            let mut deltas = vec![[0.0f32; 3]; vertex_count];
            for offset in &morph.offsets {
                if let MorphOffset::Vertex {
                    vertex,
                    translation,
                } = offset
                    && *vertex >= 0
                    && (*vertex as usize) < vertex_count
                {
                    deltas[*vertex as usize] = convert_point(*translation, scale);
                }
            }
            target_accs.push((true, add_vec3(&mut builder, &deltas, false)));
        } else {
            let mut deltas = vec![[0.0f32; 2]; vertex_count];
            for offset in &morph.offsets {
                if let MorphOffset::Uv { vertex, delta } = offset
                    && *vertex >= 0
                    && (*vertex as usize) < vertex_count
                {
                    deltas[*vertex as usize] = [delta[0], delta[1]];
                }
            }
            let mut bytes = Vec::with_capacity(deltas.len() * 8);
            for delta in &deltas {
                bytes.extend_from_slice(&delta[0].to_le_bytes());
                bytes.extend_from_slice(&delta[1].to_le_bytes());
            }
            let acc = builder.add_accessor(
                &bytes,
                CT_FLOAT,
                deltas.len(),
                "VEC2",
                None,
                Some(ARRAY_BUFFER),
            );
            target_accs.push((false, acc));
        }
    }
    // Index accessors per span, winding reversed (SPEC §5.1).
    let mut span_index_accs: Vec<usize> = Vec::new();
    for (start, count) in &spans {
        let mut indices: Vec<u32> = Vec::with_capacity(*count);
        let segment = &model.surface_indices[*start..*start + *count];
        for triangle in segment.as_chunks::<3>().0 {
            for raw in [triangle[0], triangle[2], triangle[1]] {
                if raw < 0 || (vertex_count > 0 && raw as usize >= vertex_count) {
                    return Err(FactoryError::MeshInvalid(format!(
                        "surface index {raw} outside vertex count {vertex_count}"
                    )));
                }
                indices.push(raw as u32);
            }
        }
        let mut bytes = Vec::with_capacity(indices.len() * 4);
        for index in &indices {
            bytes.extend_from_slice(&index.to_le_bytes());
        }
        span_index_accs.push(builder.add_accessor(
            &bytes,
            CT_UNSIGNED_INT,
            indices.len(),
            "SCALAR",
            None,
            Some(ELEMENT_ARRAY_BUFFER),
        ));
    }
    // Inverse bind matrices: bind pose is translation-only.
    let ibm_acc = if bone_count > 0 {
        let mut bytes = Vec::with_capacity(bone_count * 64);
        for bone in &model.bones {
            let global = convert_point(bone.position, scale);
            let matrix: [f32; 16] = [
                1.0, 0.0, 0.0, 0.0, //
                0.0, 1.0, 0.0, 0.0, //
                0.0, 0.0, 1.0, 0.0, //
                -global[0], -global[1], -global[2], 1.0,
            ];
            for component in matrix {
                bytes.extend_from_slice(&component.to_le_bytes());
            }
        }
        Some(builder.add_accessor(&bytes, CT_FLOAT, bone_count, "MAT4", None, None))
    } else {
        None
    };

    // ---- Textures ------------------------------------------------------
    let mut images: Vec<ImageDef> = Vec::new();
    let mut gltf_textures: Vec<TextureDef> = Vec::new();
    let mut image_of_pmx: Vec<Option<usize>> = vec![None; model.textures.len()];
    let mut texture_of_pmx: Vec<Option<usize>> = vec![None; model.textures.len()];
    let mut missing_textures: Vec<String> = Vec::new();
    let mut embedded_textures: Vec<String> = Vec::new();
    for (pmx_index, texture) in model.textures.iter().enumerate() {
        let resolved = options
            .textures
            .iter()
            .find(|entry| entry.pmx_index == pmx_index);
        match resolved {
            Some(entry) if entry.mime == "image/png" || entry.mime == "image/jpeg" => {
                let view = builder.push_bin(&entry.bytes, None);
                image_of_pmx[pmx_index] = Some(images.len());
                images.push(ImageDef {
                    buffer_view: view,
                    mime_type: entry.mime.clone(),
                    name: texture.path.clone(),
                });
                embedded_textures.push(texture.path.clone());
            }
            _ => {
                missing_textures.push(texture.path.clone());
            }
        }
    }

    // ---- Materials -----------------------------------------------------
    let mut materials: Vec<MaterialDef> = Vec::new();
    for material in &model.materials {
        let base_texture = if material.texture_index >= 0
            && (material.texture_index as usize) < model.textures.len()
        {
            let pmx_index = material.texture_index as usize;
            if let Some(image) = image_of_pmx[pmx_index] {
                if texture_of_pmx[pmx_index].is_none() {
                    texture_of_pmx[pmx_index] = Some(gltf_textures.len());
                    gltf_textures.push(TextureDef {
                        sampler: 0,
                        source: image,
                    });
                }
                texture_of_pmx[pmx_index].map(|tex| TexRef {
                    index: tex,
                    tex_coord: 0,
                })
            } else {
                None
            }
        } else {
            None
        };
        let toon_json = match &material.toon {
            ToonRef::Texture(tex) => json!({"kind": "texture", "index": tex}),
            ToonRef::Internal(value) => json!({"kind": "internal", "index": value}),
        };
        let sphere_json = if material.environment_index >= 0 {
            json!({
                "texture": {
                    "pmx_index": material.environment_index,
                    "path": model
                        .textures
                        .get(material.environment_index as usize)
                        .map(|t| t.path.clone())
                        .unwrap_or_default(),
                    "embedded": image_of_pmx
                        .get(material.environment_index as usize)
                        .copied()
                        .flatten()
                        .is_some(),
                },
                "blend_mode": material.environment_blend,
            })
        } else {
            Value::Null
        };
        let mut extras = Map::new();
        extras.insert(
            "pmx_toon".to_string(),
            json!({"reference": toon_json, "texture": sphere_json}),
        );
        extras.insert(
            "pmx_material".to_string(),
            json!({
                "specular": material.specular,
                "specular_strength": material.specular_strength,
                "ambient": material.ambient,
                "flags": material.flags,
                "edge": {"color": material.edge_color, "scale": material.edge_scale},
                "texture_pmx_index": material.texture_index,
                "meta": material.meta,
            }),
        );
        materials.push(MaterialDef {
            double_sided: material.flags & 0x01 != 0,
            extras: Some(Value::Object(extras)),
            name: display_name(&material.name_local, &material.name_universal),
            pbr: PbrDef {
                base_color_factor: material.diffuse,
                base_color_texture: base_texture,
                metallic_factor: 0.0,
                roughness_factor: 1.0,
            },
        });
    }
    if materials.is_empty() {
        materials.push(MaterialDef {
            double_sided: false,
            extras: None,
            name: "default".to_string(),
            pbr: PbrDef {
                base_color_factor: [1.0, 1.0, 1.0, 1.0],
                base_color_texture: None,
                metallic_factor: 0.0,
                roughness_factor: 1.0,
            },
        });
    }

    // ---- Meshes ---------------------------------------------------------
    let mut attributes = Map::new();
    attributes.insert("POSITION".to_string(), Value::from(pos_acc));
    attributes.insert("NORMAL".to_string(), Value::from(nor_acc));
    attributes.insert("TEXCOORD_0".to_string(), Value::from(uv_acc));
    if bone_count > 0 {
        attributes.insert("JOINTS_0".to_string(), Value::from(joint_acc));
        attributes.insert("WEIGHTS_0".to_string(), Value::from(weight_acc));
    }
    for (k, acc) in additional_accs.iter().enumerate() {
        attributes.insert(format!("_ADDITIONAL{k}"), Value::from(*acc));
    }
    let targets: Vec<Map<String, Value>> = target_accs
        .iter()
        .map(|(is_position, acc)| {
            let mut target = Map::new();
            if *is_position {
                target.insert("POSITION".to_string(), Value::from(*acc));
            } else {
                target.insert("TEXCOORD_0".to_string(), Value::from(*acc));
            }
            target
        })
        .collect();
    let mut mesh_extras = Map::new();
    mesh_extras.insert(
        "targetNames".to_string(),
        Value::Array(
            target_names
                .iter()
                .map(|n| Value::String(n.clone()))
                .collect(),
        ),
    );
    mesh_extras.insert(
        "pmx_fac_mapping".to_string(),
        Value::Array(
            fac_mapping
                .iter()
                .map(|row| {
                    json!({
                        "morph": row.morph,
                        "fac_channel": row.channel.clone().map_or(Value::Null, Value::String),
                        "unmapped": row.channel.is_none(),
                    })
                })
                .collect(),
        ),
    );
    let mut meshes: Vec<MeshDef> = Vec::new();
    for (span_index, _) in spans.iter().enumerate() {
        let material_index = if model.materials.is_empty() {
            0
        } else {
            span_index.min(materials.len() - 1)
        };
        meshes.push(MeshDef {
            extras: Some(Value::Object(mesh_extras.clone())),
            name: materials
                .get(material_index)
                .map(|m| m.name.clone())
                .unwrap_or_else(|| format!("span{span_index}")),
            primitives: vec![PrimitiveDef {
                attributes: attributes.clone(),
                indices: span_index_accs[span_index],
                material: material_index,
                targets: if targets.is_empty() {
                    None
                } else {
                    Some(targets.clone())
                },
            }],
            weights: if target_names.is_empty() {
                None
            } else {
                Some(vec![0.0; target_names.len()])
            },
        });
    }
    if meshes.is_empty() {
        // No material spans at all (no surfaces): emit one empty mesh
        // so the model still carries its skeleton and provenance.
        meshes.push(MeshDef {
            extras: Some(Value::Object(mesh_extras.clone())),
            name: "empty".to_string(),
            primitives: Vec::new(),
            weights: None,
        });
    }

    // ---- Nodes + skin -----------------------------------------------------
    let mut nodes: Vec<NodeDef> = Vec::new();
    let mut children_of: Vec<Vec<usize>> = vec![Vec::new(); bone_count];
    let mut roots: Vec<usize> = Vec::new();
    for (index, bone) in model.bones.iter().enumerate() {
        if bone.parent >= 0 && (bone.parent as usize) < bone_count && bone.parent as usize != index
        {
            children_of[bone.parent as usize].push(index);
        } else {
            roots.push(index);
        }
    }
    for (index, bone) in model.bones.iter().enumerate() {
        let local = if bone.parent >= 0 && (bone.parent as usize) < bone_count {
            let parent = model.bones[bone.parent as usize].position;
            [
                bone.position[0] - parent[0],
                bone.position[1] - parent[1],
                bone.position[2] - parent[2],
            ]
        } else {
            bone.position
        };
        nodes.push(NodeDef {
            children: if children_of[index].is_empty() {
                None
            } else {
                Some(children_of[index].clone())
            },
            mesh: None,
            name: display_name(&bone.name_local, &bone.name_universal),
            skin: None,
            translation: Some(convert_point(local, scale)),
        });
    }
    let mesh_node_start = nodes.len();
    for mesh_index in 0..meshes.len() {
        nodes.push(NodeDef {
            children: None,
            mesh: Some(mesh_index),
            name: format!("mesh{mesh_index}"),
            skin: if bone_count > 0 { Some(0) } else { None },
            translation: None,
        });
    }
    let mut scene_nodes = roots.clone();
    for mesh_index in 0..meshes.len() {
        scene_nodes.push(mesh_node_start + mesh_index);
    }
    if scene_nodes.is_empty() {
        scene_nodes.push(0);
    }

    // ---- Provenance -------------------------------------------------------
    let unconverted_morphs: Vec<UnconvertedMorph> = model
        .morphs
        .iter()
        .filter(|m| matches!(m.morph_type, 2 | 8 | 10))
        .map(|m| UnconvertedMorph {
            name: display_name(&m.name_local, &m.name_universal),
            morph_type: m.type_name(),
            offset_count: m.offsets.len(),
        })
        .collect();
    let recipes = morph_recipes(model);
    let unmapped_bones: Vec<String> = model
        .bones
        .iter()
        .filter(|bone| bone_target(&bone.name_local).is_none())
        .map(|bone| bone.name_local.clone())
        .collect();
    let proportions = assess_proportions(model, &profile.proportions);

    let skin_provenance = skin_provenance_json(model);
    let skins: Vec<SkinDef> = if bone_count > 0 {
        vec![SkinDef {
            extras: Some(json!({"pmx_provenance": skin_provenance})),
            inverse_bind_matrices: ibm_acc.unwrap_or(0),
            joints: (0..bone_count).collect(),
            name: "Skin".to_string(),
            skeleton: roots.first().copied().unwrap_or(0),
        }]
    } else {
        Vec::new()
    };

    // Stage 2 physics/morph provenance, built outside the root
    // `json!` (the macro has a recursion limit the root record
    // would otherwise exceed).
    let morphs_json: Vec<Value> = model
        .morphs
        .iter()
        .enumerate()
        .map(|(i, m)| {
            json!({
                "index": i,
                "name_local": m.name_local,
                "name_universal": m.name_universal,
                "panel": m.panel,
                "type": m.type_name(),
                "offset_count": m.offsets.len(),
            })
        })
        .collect();
    let rigid_bodies_json: Vec<Value> = model
        .rigid_bodies
        .iter()
        .enumerate()
        .map(|(i, r)| {
            json!({
                "index": i,
                "name_local": r.name_local,
                "name_universal": r.name_universal,
                "bone": r.bone,
                "group_id": r.group_id,
                "non_collision_mask": r.non_collision_mask,
                "shape": r.shape_name(),
                "size": r.size,
                "position_pmx": r.position,
                "rotation_pmx": r.rotation,
                "mass": r.mass,
                "move_attenuation": r.move_attenuation,
                "rotation_damping": r.rotation_damping,
                "repulsion": r.repulsion,
                "friction": r.friction,
                "physics_mode": r.physics_mode_name(),
            })
        })
        .collect();
    let joints_json: Vec<Value> = model
        .joints
        .iter()
        .enumerate()
        .map(|(i, j)| {
            json!({
                "index": i,
                "name_local": j.name_local,
                "name_universal": j.name_universal,
                "rigid_a": j.rigid_a,
                "rigid_b": j.rigid_b,
                "position_pmx": j.position,
                "rotation_pmx": j.rotation,
                "position_min": j.position_min,
                "position_max": j.position_max,
                "rotation_min": j.rotation_min,
                "rotation_max": j.rotation_max,
                "position_spring": j.position_spring,
                "rotation_spring": j.rotation_spring,
            })
        })
        .collect();

    let root_provenance = json!({
        "pmx_provenance": {
            "converter": "pmx-import/1",
            "scale_to_meters": scale,
            "scale_applied": true,
            "pmx_version": model.pmx_version,
            "model_name_local": model.model_name_local,
            "model_name_universal": model.model_name_universal,
            "vertex_count": vertex_count,
            "surface_count": model.surface_indices.len(),
            "weight_corrections": weight_corrections,
            "invalid_bone_refs": invalid_bone_refs,
            "sdef": {
                "count": sdef_records.len(),
                "vertices": sdef_records.iter().map(|(i, _)| *i).collect::<Vec<_>>(),
                "c": sdef_records.iter().map(|(_, d)| sdef_vectors(d).0).collect::<Vec<_>>(),
                "r0": sdef_records.iter().map(|(_, d)| sdef_vectors(d).1).collect::<Vec<_>>(),
                "r1": sdef_records.iter().map(|(_, d)| sdef_vectors(d).2).collect::<Vec<_>>(),
            },
            "qdef": {"count": qdef_vertices},
            "unconverted_morphs": model.morphs.iter()
                .filter(|m| matches!(m.morph_type, 2 | 8 | 10))
                .map(|m| json!({
                    "name": display_name(&m.name_local, &m.name_universal),
                    "type": m.type_name(),
                    "offsets": m.offsets.iter().map(offset_json).collect::<Vec<_>>(),
                }))
                .collect::<Vec<_>>(),
            "recipes": recipes.iter().map(|r| json!({
                "name": r.name,
                "complete": r.complete,
                "drives": r.drives.iter()
                    .map(|(name, influence)| json!({"morph": name, "influence": influence}))
                    .collect::<Vec<_>>(),
            })).collect::<Vec<_>>(),
            "fac_mapping": fac_mapping.iter().map(|row| json!({
                "morph": row.morph,
                "fac_channel": row.channel.clone().map_or(Value::Null, Value::String),
                "unmapped": row.channel.is_none(),
            })).collect::<Vec<_>>(),
            "missing_textures": missing_textures,
            "embedded_textures": embedded_textures,
            "textures": model.textures.iter().enumerate().map(|(i, t)| json!({
                "pmx_index": i,
                "path": t.path,
                "embedded": image_of_pmx.get(i).copied().flatten().is_some(),
                "image": image_of_pmx.get(i).copied().flatten().map_or(Value::Null, Value::from),
            })).collect::<Vec<_>>(),
            "morphed_attribute_count": target_names.len(),
            "exceeds_eight_attribute_floor": target_names.len() > MORPHED_ATTRIBUTE_FLOOR,
            "proportions": {
                "height_pmx": proportions.height_pmx,
                "head_fraction": proportions.head_fraction.map_or(Value::Null, |v| json!(v)),
                "height_in_band": proportions.height_in_band,
                "height_in_heroine_band": proportions.height_in_heroine_band,
                "head_fraction_in_band": proportions.head_fraction_in_band,
            },
            "rigid_body_count": model.rigid_bodies.len(),
            "joint_count": model.joints.len(),
            "soft_body_count": model.soft_bodies.len(),
            "spans_adjusted": spans_adjusted,
            // Stage 2 amendment (gltf-import/1, D-633): the GLB is
            // the auditable midpoint of Route A, so it must carry
            // everything ingest needs — the source census hash, the
            // census counts, the full morph table, and the physics
            // records verbatim (PMX-space values, named as such).
            // Without these, physics and zero-silent-drop accounting
            // could not be derived from the GLB alone.
            "source_file_sha256": options.source_sha256.clone()
                .map_or(Value::Null, Value::String),
            "census_counts": census_counts_json(model),
            "morphs": morphs_json,
            "rigid_bodies": rigid_bodies_json,
            "joints": joints_json,
        }
    });

    let doc = GltfDoc {
        accessors: builder.accessors,
        asset: AssetDef {
            generator: "lumen-factory pmx-import/1".to_string(),
            version: "2.0".to_string(),
        },
        buffer_views: builder.views,
        buffers: vec![BufferDef {
            byte_length: builder.bin.len(),
        }],
        extras: Some(root_provenance),
        images,
        materials,
        meshes,
        nodes,
        samplers: if gltf_textures.is_empty() {
            Vec::new()
        } else {
            vec![SamplerDef {
                mag_filter: 9729,
                min_filter: 9729,
                wrap_s: 10497,
                wrap_t: 10497,
            }]
        },
        scenes: vec![SceneDef {
            name: "Scene".to_string(),
            nodes: scene_nodes,
        }],
        skins,
        textures: gltf_textures,
    };

    let glb = assemble_glb(&doc, &builder.bin)?;
    let report = ConversionReport {
        vertex_count,
        triangle_count: model.surface_indices.len() / 3,
        mesh_count: doc.meshes.len(),
        bone_count,
        weight_corrections,
        sdef_vertices: sdef_records.len(),
        qdef_vertices,
        invalid_bone_refs,
        morph_target_count: target_names.len(),
        morphed_attribute_count: target_names.len(),
        exceeds_morph_floor: target_names.len() > MORPHED_ATTRIBUTE_FLOOR,
        unconverted_morphs,
        fac_mapping,
        recipes,
        missing_textures: model
            .textures
            .iter()
            .enumerate()
            .filter(|(i, _)| image_of_pmx.get(*i).copied().flatten().is_none())
            .map(|(_, t)| t.path.clone())
            .collect(),
        embedded_textures: model
            .textures
            .iter()
            .enumerate()
            .filter(|(i, _)| image_of_pmx.get(*i).copied().flatten().is_some())
            .map(|(_, t)| t.path.clone())
            .collect(),
        unmapped_bones,
        spans_adjusted,
        scale_to_meters: scale,
        proportions,
    };
    Ok((glb, report))
}

fn display_name(local: &str, universal: &str) -> String {
    if universal.is_empty() {
        local.to_string()
    } else {
        universal.to_string()
    }
}

/// PMX → glTF point conversion: scale, then negate Z (SPEC §5.1).
fn convert_point(point: [f32; 3], scale: f32) -> [f32; 3] {
    [point[0] * scale, point[1] * scale, -point[2] * scale]
}

/// PMX → glTF direction conversion: negate Z, no scale.
fn convert_direction(direction: [f32; 3]) -> [f32; 3] {
    [direction[0], direction[1], -direction[2]]
}

fn sane_bone(bone: i64, bone_count: usize, invalid: &mut usize) -> u16 {
    if bone >= 0 && (bone as usize) < bone_count {
        bone as u16
    } else {
        *invalid += 1;
        0
    }
}

fn sdef_vectors(deform: &Deform) -> ([f32; 3], [f32; 3], [f32; 3]) {
    match deform {
        Deform::Sdef { c, r0, r1, .. } => (*c, *r0, *r1),
        _ => ([0.0; 3], [0.0; 3], [0.0; 3]),
    }
}

/// Material spans over the surface run: (start, count) per material.
/// When the authored counts do not exactly partition the run, the
/// final span absorbs the remainder (or later spans go empty) and
/// the caller ledgers `spans_adjusted` — never a silent change.
fn material_spans(model: &PmxModel) -> (Vec<(usize, usize)>, bool) {
    let total = model.surface_indices.len();
    if model.materials.is_empty() {
        return (vec![(0, total)], total > 0);
    }
    let mut spans = Vec::with_capacity(model.materials.len());
    let mut cursor = 0usize;
    let mut adjusted = false;
    for (index, material) in model.materials.iter().enumerate() {
        let authored = material.surface_count.max(0) as usize;
        let remaining = total.saturating_sub(cursor);
        let mut count = authored.min(remaining);
        if index + 1 == model.materials.len() && count != remaining {
            count = remaining;
        }
        if count != authored {
            adjusted = true;
        }
        spans.push((cursor, count));
        cursor += count;
    }
    if cursor != total {
        adjusted = true;
    }
    (spans, adjusted)
}

/// Full offset payload as JSON (provenance preservation).
fn offset_json(offset: &MorphOffset) -> Value {
    match offset {
        MorphOffset::Group { morph, influence } => {
            json!({"kind": "group", "morph": morph, "influence": influence})
        }
        MorphOffset::Vertex {
            vertex,
            translation,
        } => json!({"kind": "vertex", "vertex": vertex, "translation": translation}),
        MorphOffset::Bone {
            bone,
            translation,
            rotation,
        } => json!({
            "kind": "bone", "bone": bone,
            "translation": translation, "rotation": rotation,
        }),
        MorphOffset::Uv { vertex, delta } => {
            json!({"kind": "uv", "vertex": vertex, "delta": delta})
        }
        MorphOffset::Material {
            material,
            op,
            diffuse,
            specular,
            specular_strength,
            ambient,
            edge_color,
            edge_size,
            texture_tint,
            environment_tint,
            toon_tint,
        } => json!({
            "kind": "material", "material": material, "op": op,
            "diffuse": diffuse, "specular": specular,
            "specular_strength": specular_strength, "ambient": ambient,
            "edge_color": edge_color, "edge_size": edge_size,
            "texture_tint": texture_tint,
            "environment_tint": environment_tint, "toon_tint": toon_tint,
        }),
        MorphOffset::Impulse {
            rigid,
            local_flag,
            velocity,
            torque,
        } => json!({
            "kind": "impulse", "rigid": rigid, "local_flag": local_flag,
            "velocity": velocity, "torque": torque,
        }),
    }
}

/// Census counts as provenance JSON (Stage 2 amendment): the
/// denominators of `gltf-import/1`'s zero-silent-drop accounting,
/// derived from the same retained parse the census derives from,
/// so the two can never disagree.
fn census_counts_json(model: &PmxModel) -> Value {
    let mut by_class = Map::new();
    for class in [
        "group", "vertex", "bone", "uv", "material", "flip", "impulse",
    ] {
        by_class.insert(class.to_string(), Value::from(0usize));
    }
    for morph in &model.morphs {
        let key = morph.type_name();
        if let Some(slot) = by_class.get_mut(key) {
            *slot = Value::from(slot.as_u64().unwrap_or(0) + 1);
        }
    }
    json!({
        "vertices": model.vertices.len(),
        "faces": model.surface_indices.len() / 3,
        "textures": model.textures.len(),
        "materials": model.materials.len(),
        "bones": model.bones.len(),
        "morphs_total": model.morphs.len(),
        "morphs_by_class": Value::Object(by_class),
        "rigid_bodies": model.rigid_bodies.len(),
        "joints": model.joints.len(),
        "soft_bodies": model.soft_bodies.len(),
        "display_frames": model.display_frames.len(),
    })
}

/// The skin's `pmx_provenance`: every bone's flag-conditional detail
/// verbatim (PMX-space values named as such), plus its §6.1 target.
fn skin_provenance_json(model: &PmxModel) -> Value {
    let bones: Vec<Value> = model
        .bones
        .iter()
        .enumerate()
        .map(|(index, bone)| {
            let tail = match &bone.tail {
                super::parse::BoneTail::Indexed(target) => {
                    json!({"kind": "bone", "index": target})
                }
                super::parse::BoneTail::Offset(offset) => {
                    json!({"kind": "offset", "offset_pmx": offset})
                }
            };
            let ik = bone.ik.as_ref().map_or(Value::Null, |ik| {
                json!({
                    "target": ik.target,
                    "loop_count": ik.loop_count,
                    "limit_radian": ik.limit_radian,
                    "links": ik.links.iter().map(|link| json!({
                        "bone": link.bone,
                        "limits": link.limits.map_or(Value::Null, |(min, max)| {
                            json!({"min": min, "max": max})
                        }),
                    })).collect::<Vec<_>>(),
                })
            });
            json!({
                "index": index,
                "name_local": bone.name_local,
                "name_universal": bone.name_universal,
                "parent": bone.parent,
                "layer": bone.layer,
                "flags": bone.flags,
                "position_pmx": bone.position,
                "tail": tail,
                "inherit": bone.inherit.as_ref().map_or(Value::Null, |inherit| {
                    json!({
                        "parent_bone": inherit.parent_bone,
                        "influence": inherit.influence,
                    })
                }),
                "fixed_axis": bone.fixed_axis.map_or(Value::Null, |axis| json!(axis)),
                "local_axes": bone.local_axes.map_or(Value::Null, |(x, z)| {
                    json!({"x": x, "z": z})
                }),
                "external_parent": bone
                    .external_parent
                    .map_or(Value::Null, Value::from),
                "ik": ik,
                "torogao": bone_target(&bone.name_local)
                    .map_or(Value::Null, Value::from),
            })
        })
        .collect();
    json!({"bones": bones})
}

// ---------------------------------------------------------------------
// Deterministic GLB assembly (estate framing: JSON space-padded,
// BIN zero-padded, both to 4 bytes; fixed struct field order).
// ---------------------------------------------------------------------

#[derive(Serialize)]
struct GltfDoc {
    accessors: Vec<AccessorDef>,
    asset: AssetDef,
    #[serde(rename = "bufferViews")]
    buffer_views: Vec<BufferViewDef>,
    buffers: Vec<BufferDef>,
    #[serde(skip_serializing_if = "Option::is_none")]
    extras: Option<Value>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    images: Vec<ImageDef>,
    materials: Vec<MaterialDef>,
    meshes: Vec<MeshDef>,
    nodes: Vec<NodeDef>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    samplers: Vec<SamplerDef>,
    scenes: Vec<SceneDef>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    skins: Vec<SkinDef>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    textures: Vec<TextureDef>,
}

#[derive(Serialize)]
struct AssetDef {
    generator: String,
    version: String,
}

#[derive(Serialize)]
struct BufferDef {
    #[serde(rename = "byteLength")]
    byte_length: usize,
}

#[derive(Serialize, Clone)]
struct BufferViewDef {
    buffer: usize,
    #[serde(rename = "byteLength")]
    byte_length: usize,
    #[serde(rename = "byteOffset")]
    byte_offset: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    target: Option<u32>,
}

#[derive(Serialize)]
struct AccessorDef {
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

fn is_zero_usize(value: &usize) -> bool {
    *value == 0
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
struct MaterialDef {
    #[serde(rename = "doubleSided")]
    double_sided: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    extras: Option<Value>,
    name: String,
    #[serde(rename = "pbrMetallicRoughness")]
    pbr: PbrDef,
}

#[derive(Serialize)]
struct PbrDef {
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
    #[serde(skip_serializing_if = "Option::is_none")]
    extras: Option<Value>,
    #[serde(rename = "inverseBindMatrices")]
    inverse_bind_matrices: usize,
    joints: Vec<usize>,
    name: String,
    skeleton: usize,
}

struct GlbBuilder {
    bin: Vec<u8>,
    views: Vec<BufferViewDef>,
    accessors: Vec<AccessorDef>,
}

impl GlbBuilder {
    fn new() -> Self {
        GlbBuilder {
            bin: Vec::new(),
            views: Vec::new(),
            accessors: Vec::new(),
        }
    }

    fn push_bin(&mut self, bytes: &[u8], target: Option<u32>) -> usize {
        while !self.bin.len().is_multiple_of(4) {
            self.bin.push(0);
        }
        let offset = self.bin.len();
        self.bin.extend_from_slice(bytes);
        self.views.push(BufferViewDef {
            buffer: 0,
            byte_length: bytes.len(),
            byte_offset: offset,
            target,
        });
        self.views.len() - 1
    }

    fn add_accessor(
        &mut self,
        bytes: &[u8],
        component_type: u32,
        count: usize,
        accessor_type: &str,
        min_max: Option<(Vec<f32>, Vec<f32>)>,
        target: Option<u32>,
    ) -> usize {
        let view = self.push_bin(bytes, target);
        self.accessors.push(AccessorDef {
            buffer_view: view,
            byte_offset: 0,
            component_type,
            count,
            max: min_max.as_ref().map(|bounds| bounds.1.clone()),
            min: min_max.map(|bounds| bounds.0),
            accessor_type: accessor_type.to_string(),
        });
        self.accessors.len() - 1
    }
}

fn add_vec3(builder: &mut GlbBuilder, values: &[[f32; 3]], with_min_max: bool) -> usize {
    let mut bytes = Vec::with_capacity(values.len() * 12);
    for value in values {
        for component in value {
            bytes.extend_from_slice(&component.to_le_bytes());
        }
    }
    let (min, max) = if with_min_max && !values.is_empty() {
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        for value in values {
            for axis in 0..3 {
                lo[axis] = lo[axis].min(value[axis]);
                hi[axis] = hi[axis].max(value[axis]);
            }
        }
        (Some(lo.to_vec()), Some(hi.to_vec()))
    } else {
        (None, None)
    };
    builder.add_accessor(
        &bytes,
        CT_FLOAT,
        values.len(),
        "VEC3",
        min.zip(max),
        Some(ARRAY_BUFFER),
    )
}

fn assemble_glb(doc: &GltfDoc, bin: &[u8]) -> Result<Vec<u8>, FactoryError> {
    let mut json_bytes = serde_json::to_vec(doc)?;
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
