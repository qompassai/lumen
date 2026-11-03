//! `pmx-census/1` record and its deterministic JSON emission.
//!
//! The census is derived from the retained model produced by
//! `parse.rs` (a single parse serves both passes, so census and
//! conversion can never disagree about a file's structure). The
//! JSON form is byte-identical to the Stage 0 prototype's output
//! for the same file — that parity is a Stage 1 gate, pinned by
//! `tests/pmx_golden.rs` against the Stage 0 fixture JSONs.
//!
//! Determinism: fixed field order, file-order arrays, no
//! timestamps, no hash-map iteration.

use super::CensusError;
use super::parse::{PmxModel, parse_model};
use super::sha256;

/// One texture-table entry in the census.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextureEntry {
    /// Position in the PMX texture table.
    pub index: usize,
    /// Decoded path string, verbatim from the file.
    pub path: String,
    /// SHA-256 of the raw stored path bytes.
    pub path_sha256: String,
}

/// One material entry in the census.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MaterialEntry {
    /// Position in the PMX material table.
    pub index: usize,
    /// Local material name, verbatim.
    pub name_local: String,
    /// Universal material name, verbatim.
    pub name_universal: String,
    /// Texture-table index this material samples, or -1.
    pub texture_index: i64,
    /// Surface-index count this material covers.
    pub surface_count: i64,
}

/// One bone entry in the census.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BoneEntry {
    /// Position in the PMX bone table.
    pub index: usize,
    /// Local bone name, verbatim.
    pub name_local: String,
    /// Universal bone name, verbatim.
    pub name_universal: String,
    /// Parent bone index, or -1 for a root bone.
    pub parent: i64,
    /// True when the bone carries an IK block.
    pub has_ik: bool,
}

/// One morph entry in the census.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MorphEntry {
    /// Position in the PMX morph table.
    pub index: usize,
    /// Local morph name, verbatim.
    pub name_local: String,
    /// Universal morph name, verbatim.
    pub name_universal: String,
    /// PMX panel type byte.
    pub panel: u8,
    /// Morph class name (group, vertex, bone, uv, material, flip, impulse).
    pub morph_type: &'static str,
    /// Number of offset records inside this morph.
    pub offset_count: i64,
}

/// One rigid-body entry in the census.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RigidEntry {
    /// Position in the PMX rigid-body table.
    pub index: usize,
    /// Local rigid-body name, verbatim.
    pub name_local: String,
    /// Universal rigid-body name, verbatim.
    pub name_universal: String,
    /// Related bone index, or -1.
    pub bone: i64,
    /// Shape class name (sphere, box, capsule).
    pub shape: &'static str,
    /// Physics mode name (follow_bone, physics, physics_plus_bone).
    pub physics_mode: &'static str,
}

/// The census record: everything `pmx-census/1` emits. (No `Eq`:
/// the record carries the header's f32 version verbatim.)
#[derive(Debug, Clone, PartialEq)]
pub struct Census {
    /// PMX version as written in the header (2.0 or 2.1).
    pub pmx_version: f32,
    /// Declared text encoding: "utf-16le" or "utf-8".
    pub encoding: &'static str,
    /// Additional vec4 count from the header globals (0..=4).
    pub additional_vec4_count: u8,
    /// Model local name, verbatim (may be empty).
    pub model_name_local: String,
    /// Model universal name, verbatim (may be empty).
    pub model_name_universal: String,
    /// Model local comment, verbatim (may be empty).
    pub model_comment_local: String,
    /// Model universal comment, verbatim (may be empty).
    pub model_comment_universal: String,
    /// Total input length in bytes.
    pub input_bytes: usize,
    /// SHA-256 of the whole input.
    pub file_sha256: String,
    /// Vertex count.
    pub vertex_count: i64,
    /// Surface index count.
    pub surface_count: i64,
    /// Triangle count (surface_count / 3).
    pub face_count: i64,
    /// Texture entries, in file order.
    pub textures: Vec<TextureEntry>,
    /// Material entries, in file order.
    pub materials: Vec<MaterialEntry>,
    /// Bone entries, in file order.
    pub bones: Vec<BoneEntry>,
    /// Morph entries, in file order.
    pub morphs: Vec<MorphEntry>,
    /// Display frame count.
    pub display_frame_count: i64,
    /// Rigid-body entries, in file order.
    pub rigid_bodies: Vec<RigidEntry>,
    /// Joint count.
    pub joint_count: i64,
    /// Soft-body count (PMX 2.1; always 0 for 2.0 files).
    pub soft_body_count: i64,
    /// Vertex deform tally: [BDEF1, BDEF2, BDEF4, SDEF, QDEF].
    pub deform_counts: [i64; 5],
    /// Morph tally by class: [group, vertex, bone, uv, material, flip, impulse].
    pub morph_type_counts: [i64; 7],
    /// True when the per-material surface counts sum to the total.
    pub material_surface_sum_matches: bool,
}

/// Derive the census record for an already-parsed model. `bytes`
/// is the original file (for size + whole-file hash).
pub fn census_of(model: &PmxModel, bytes: &[u8]) -> Census {
    let mut deform_counts = [0i64; 5];
    for vertex in &model.vertices {
        deform_counts[vertex.deform.class_slot()] += 1;
    }
    let mut morph_type_counts = [0i64; 7];
    for morph in &model.morphs {
        morph_type_counts[morph.tally_slot()] += 1;
    }
    let surface_count = model.surface_indices.len() as i64;
    let mut material_surface_sum: i64 = 0;
    let mut sum_ok = true;
    for material in &model.materials {
        match material_surface_sum.checked_add(material.surface_count) {
            Some(total) => material_surface_sum = total,
            None => sum_ok = false,
        }
    }
    Census {
        pmx_version: model.pmx_version,
        encoding: if model.globals.encoding_utf8 {
            "utf-8"
        } else {
            "utf-16le"
        },
        additional_vec4_count: model.globals.additional_vec4_count,
        model_name_local: model.model_name_local.clone(),
        model_name_universal: model.model_name_universal.clone(),
        model_comment_local: model.model_comment_local.clone(),
        model_comment_universal: model.model_comment_universal.clone(),
        input_bytes: bytes.len(),
        file_sha256: sha256::sha256_hex(bytes),
        vertex_count: model.vertices.len() as i64,
        surface_count,
        face_count: surface_count / 3,
        textures: model
            .textures
            .iter()
            .enumerate()
            .map(|(index, tex)| TextureEntry {
                index,
                path: tex.path.clone(),
                path_sha256: sha256::sha256_hex(&tex.raw_path),
            })
            .collect(),
        materials: model
            .materials
            .iter()
            .enumerate()
            .map(|(index, mat)| MaterialEntry {
                index,
                name_local: mat.name_local.clone(),
                name_universal: mat.name_universal.clone(),
                texture_index: mat.texture_index,
                surface_count: mat.surface_count,
            })
            .collect(),
        bones: model
            .bones
            .iter()
            .enumerate()
            .map(|(index, bone)| BoneEntry {
                index,
                name_local: bone.name_local.clone(),
                name_universal: bone.name_universal.clone(),
                parent: bone.parent,
                has_ik: bone.ik.is_some(),
            })
            .collect(),
        morphs: model
            .morphs
            .iter()
            .enumerate()
            .map(|(index, morph)| MorphEntry {
                index,
                name_local: morph.name_local.clone(),
                name_universal: morph.name_universal.clone(),
                panel: morph.panel,
                morph_type: morph.type_name(),
                offset_count: morph.offsets.len() as i64,
            })
            .collect(),
        display_frame_count: model.display_frames.len() as i64,
        rigid_bodies: model
            .rigid_bodies
            .iter()
            .enumerate()
            .map(|(index, rigid)| RigidEntry {
                index,
                name_local: rigid.name_local.clone(),
                name_universal: rigid.name_universal.clone(),
                bone: rigid.bone,
                shape: rigid.shape_name(),
                physics_mode: rigid.physics_mode_name(),
            })
            .collect(),
        joint_count: model.joints.len() as i64,
        soft_body_count: model.soft_bodies.len() as i64,
        deform_counts,
        morph_type_counts,
        material_surface_sum_matches: sum_ok && material_surface_sum == surface_count,
    }
}

/// Parse `bytes` and derive the census in one call (the Stage 0
/// prototype's entry point, kept for the CLI and the ported tests).
pub fn census(bytes: &[u8]) -> Result<Census, CensusError> {
    let model = parse_model(bytes)?;
    Ok(census_of(&model, bytes))
}

/// Escape a string for inclusion in the census JSON output.
fn json_escape(text: &str, out: &mut String) {
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            ch if (ch as u32) < 0x20 => {
                out.push_str(&format!("\\u{:04x}", ch as u32));
            }
            ch => out.push(ch),
        }
    }
    out.push('"');
}

impl Census {
    /// Serialize the census as `pmx-census/1` JSON. Deterministic:
    /// fixed field order, file-order arrays, no ambient data.
    /// Byte-identical to the Stage 0 prototype for the same input.
    pub fn to_json(&self) -> String {
        let mut s = String::with_capacity(4096);
        s.push_str("{\n  \"format\": \"pmx-census/1\",\n");
        s.push_str(&format!("  \"pmx_version\": {},\n", self.pmx_version));
        s.push_str(&format!("  \"encoding\": \"{}\",\n", self.encoding));
        s.push_str(&format!(
            "  \"additional_vec4_count\": {},\n",
            self.additional_vec4_count
        ));
        s.push_str(&format!("  \"input_bytes\": {},\n", self.input_bytes));
        s.push_str(&format!("  \"file_sha256\": \"{}\",\n", self.file_sha256));
        s.push_str("  \"model\": {");
        s.push_str("\n    \"name_local\": ");
        json_escape(&self.model_name_local, &mut s);
        s.push_str(",\n    \"name_universal\": ");
        json_escape(&self.model_name_universal, &mut s);
        s.push_str(",\n    \"comment_local\": ");
        json_escape(&self.model_comment_local, &mut s);
        s.push_str(",\n    \"comment_universal\": ");
        json_escape(&self.model_comment_universal, &mut s);
        s.push_str("\n  },\n  \"counts\": {");
        s.push_str(&format!("\n    \"vertices\": {},", self.vertex_count));
        s.push_str(&format!(
            "\n    \"surface_indices\": {},",
            self.surface_count
        ));
        s.push_str(&format!("\n    \"faces\": {},", self.face_count));
        s.push_str(&format!("\n    \"textures\": {},", self.textures.len()));
        s.push_str(&format!("\n    \"materials\": {},", self.materials.len()));
        s.push_str(&format!("\n    \"bones\": {},", self.bones.len()));
        s.push_str(&format!("\n    \"morphs\": {},", self.morphs.len()));
        s.push_str(&format!(
            "\n    \"display_frames\": {},",
            self.display_frame_count
        ));
        s.push_str(&format!(
            "\n    \"rigid_bodies\": {},",
            self.rigid_bodies.len()
        ));
        s.push_str(&format!("\n    \"joints\": {},", self.joint_count));
        s.push_str(&format!("\n    \"soft_bodies\": {}", self.soft_body_count));
        s.push_str("\n  },\n  \"deform_counts\": {");
        let deform_names = ["bdef1", "bdef2", "bdef4", "sdef", "qdef"];
        for (i, name) in deform_names.iter().enumerate() {
            let comma = if i + 1 < deform_names.len() { "," } else { "" };
            s.push_str(&format!(
                "\n    \"{name}\": {}{comma}",
                self.deform_counts[i]
            ));
        }
        s.push_str("\n  },\n  \"morphs_by_type\": {");
        let morph_names = [
            "group", "vertex", "bone", "uv", "material", "flip", "impulse",
        ];
        for (i, name) in morph_names.iter().enumerate() {
            let comma = if i + 1 < morph_names.len() { "," } else { "" };
            s.push_str(&format!(
                "\n    \"{name}\": {}{comma}",
                self.morph_type_counts[i]
            ));
        }
        s.push_str("\n  },\n  \"textures\": [");
        for (i, tex) in self.textures.iter().enumerate() {
            let comma = if i + 1 < self.textures.len() { "," } else { "" };
            s.push_str(&format!("\n    {{\"index\": {}, \"path\": ", tex.index));
            json_escape(&tex.path, &mut s);
            s.push_str(&format!(
                ", \"path_sha256\": \"{}\"}}{comma}",
                tex.path_sha256
            ));
        }
        s.push_str("\n  ],\n  \"materials\": [");
        for (i, mat) in self.materials.iter().enumerate() {
            let comma = if i + 1 < self.materials.len() {
                ","
            } else {
                ""
            };
            s.push_str(&format!(
                "\n    {{\"index\": {}, \"name_local\": ",
                mat.index
            ));
            json_escape(&mat.name_local, &mut s);
            s.push_str(", \"name_universal\": ");
            json_escape(&mat.name_universal, &mut s);
            s.push_str(&format!(
                ", \"texture_index\": {}, \"surface_count\": {}}}{comma}",
                mat.texture_index, mat.surface_count
            ));
        }
        s.push_str("\n  ],\n  \"bones\": [");
        for (i, bone) in self.bones.iter().enumerate() {
            let comma = if i + 1 < self.bones.len() { "," } else { "" };
            s.push_str(&format!(
                "\n    {{\"index\": {}, \"name_local\": ",
                bone.index
            ));
            json_escape(&bone.name_local, &mut s);
            s.push_str(", \"name_universal\": ");
            json_escape(&bone.name_universal, &mut s);
            s.push_str(&format!(
                ", \"parent\": {}, \"has_ik\": {}}}{comma}",
                bone.parent, bone.has_ik
            ));
        }
        s.push_str("\n  ],\n  \"morphs\": [");
        for (i, morph) in self.morphs.iter().enumerate() {
            let comma = if i + 1 < self.morphs.len() { "," } else { "" };
            s.push_str(&format!(
                "\n    {{\"index\": {}, \"name_local\": ",
                morph.index
            ));
            json_escape(&morph.name_local, &mut s);
            s.push_str(", \"name_universal\": ");
            json_escape(&morph.name_universal, &mut s);
            s.push_str(&format!(
                ", \"panel\": {}, \"type\": \"{}\", \"offset_count\": {}}}{comma}",
                morph.panel, morph.morph_type, morph.offset_count
            ));
        }
        s.push_str("\n  ],\n  \"rigid_bodies\": [");
        for (i, rigid) in self.rigid_bodies.iter().enumerate() {
            let comma = if i + 1 < self.rigid_bodies.len() {
                ","
            } else {
                ""
            };
            s.push_str(&format!(
                "\n    {{\"index\": {}, \"name_local\": ",
                rigid.index
            ));
            json_escape(&rigid.name_local, &mut s);
            s.push_str(", \"name_universal\": ");
            json_escape(&rigid.name_universal, &mut s);
            s.push_str(&format!(
                ", \"bone\": {}, \"shape\": \"{}\", \"physics_mode\": \"{}\"}}{comma}",
                rigid.bone, rigid.shape, rigid.physics_mode
            ));
        }
        s.push_str(&format!(
            "\n  ],\n  \"material_surface_sum_matches\": {}\n}}\n",
            self.material_surface_sum_matches
        ));
        s
    }
}
