//! Bounded PMX 2.0 / 2.1 record parser (pmx-import/1).
//!
//! Written from the published PMX file-format specification (see
//! `~/workspace/mmd-port-20261102/SPEC.md` for the source list). No
//! code from any existing PMX implementation was consulted.
//!
//! Contract:
//! - Input is an untrusted byte slice. Every read is bounds-checked,
//!   every count is range-checked against a named cap *before* any
//!   loop or allocation sized by it, and malformed input returns a
//!   typed [`CensusError`]. The parser never panics on input data.
//! - Unlike the Stage 0 census walk, this parser *retains* the full
//!   geometry payload (vertices, indices, materials, morph offsets,
//!   bone detail, rigid bodies, joints) so `glb.rs` can convert
//!   without re-reading the file.
//! - Trailing bytes after the final section are a typed error.

use std::fmt;

/// Maximum accepted input size, in bytes.
pub const INPUT_BYTES_MAX: usize = 256 * 1024 * 1024;
/// Maximum vertices in one model.
pub const VERTEX_COUNT_MAX: i64 = 10_000_000;
/// Maximum surface indices (vertex-index entries) in one model.
pub const SURFACE_COUNT_MAX: i64 = 30_000_000;
/// Maximum textures in one model.
pub const TEXTURE_COUNT_MAX: i64 = 4_096;
/// Maximum materials in one model.
pub const MATERIAL_COUNT_MAX: i64 = 4_096;
/// Maximum bones in one model.
pub const BONE_COUNT_MAX: i64 = 4_096;
/// Maximum morphs in one model.
pub const MORPH_COUNT_MAX: i64 = 4_096;
/// Maximum offsets inside one morph.
pub const MORPH_OFFSET_COUNT_MAX: i64 = 20_000_000;
/// Maximum display frames in one model.
pub const DISPLAY_FRAME_COUNT_MAX: i64 = 1_024;
/// Maximum elements inside one display frame.
pub const DISPLAY_ELEMENT_COUNT_MAX: i64 = 100_000;
/// Maximum rigid bodies in one model.
pub const RIGID_BODY_COUNT_MAX: i64 = 8_192;
/// Maximum joints in one model.
pub const JOINT_COUNT_MAX: i64 = 8_192;
/// Maximum soft bodies in one PMX 2.1 model.
pub const SOFT_BODY_COUNT_MAX: i64 = 1_024;
/// Maximum links inside one bone IK block.
pub const IK_LINK_COUNT_MAX: i64 = 1_024;
/// Maximum byte length of one `text` field.
pub const TEXT_BYTES_MAX: i64 = 1_048_576;

/// A typed parse failure. Every variant is reachable from untrusted
/// input alone; none of them is a panic path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CensusError {
    /// The first four bytes are not the PMX signature `PMX `.
    BadMagic,
    /// The version float is neither 2.0 nor 2.1. Carries its bits.
    UnsupportedVersion(u32),
    /// A header globals byte is outside its legal set.
    BadGlobals(&'static str),
    /// The input ended inside the named section.
    Truncated(&'static str),
    /// A count field is negative or above its named cap.
    CountOutOfRange(&'static str, i64),
    /// A `text` field is not valid in the declared encoding.
    InvalidText(&'static str),
    /// A single-value field (enum-like byte) is outside its legal set.
    InvalidValue(&'static str, i64),
    /// Bytes remain after the final section.
    TrailingBytes(usize),
}

impl fmt::Display for CensusError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CensusError::BadMagic => write!(f, "bad magic: not a PMX file"),
            CensusError::UnsupportedVersion(bits) => {
                write!(f, "unsupported PMX version (f32 bits {bits:#010x})")
            }
            CensusError::BadGlobals(field) => write!(f, "bad header globals value: {field}"),
            CensusError::Truncated(section) => write!(f, "truncated input in section: {section}"),
            CensusError::CountOutOfRange(field, value) => {
                write!(f, "count out of range: {field} = {value}")
            }
            CensusError::InvalidText(field) => write!(f, "invalid text encoding in: {field}"),
            CensusError::InvalidValue(field, value) => {
                write!(f, "invalid value: {field} = {value}")
            }
            CensusError::TrailingBytes(n) => write!(f, "{n} trailing bytes after final section"),
        }
    }
}

impl std::error::Error for CensusError {}

/// Result alias for PMX parsing.
pub type ParseResult<T> = Result<T, CensusError>;

/// Which index family a sized index belongs to; controls signedness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IndexKind {
    /// Vertex indices are unsigned.
    Vertex,
    /// Every other index family is signed, with -1 as the nil value.
    Signed,
}

/// Header globals that steer every later record's size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Globals {
    /// True for UTF-8 text, false for UTF-16LE.
    pub encoding_utf8: bool,
    /// Additional vec4 count per vertex (0..=4).
    pub additional_vec4_count: u8,
    /// Byte width of vertex indices.
    pub vertex_index_size: u8,
    /// Byte width of texture indices.
    pub texture_index_size: u8,
    /// Byte width of material indices.
    pub material_index_size: u8,
    /// Byte width of bone indices.
    pub bone_index_size: u8,
    /// Byte width of morph indices.
    pub morph_index_size: u8,
    /// Byte width of rigid-body indices.
    pub rigid_index_size: u8,
}

/// Vertex skinning payload, retained in full.
#[derive(Debug, Clone, PartialEq)]
pub enum Deform {
    /// BDEF1: one bone.
    Bdef1 {
        /// Bone index.
        bone: i64,
    },
    /// BDEF2: two bones, first-bone weight.
    Bdef2 {
        /// First bone index.
        bone1: i64,
        /// Second bone index.
        bone2: i64,
        /// Weight of the first bone.
        weight: f32,
    },
    /// BDEF4: four bones and four weights (not guaranteed to sum to 1).
    Bdef4 {
        /// Bone indices.
        bones: [i64; 4],
        /// Bone weights as authored.
        weights: [f32; 4],
    },
    /// SDEF: spherical deform; degrades to BDEF2 at conversion, with
    /// the C / R0 / R1 vectors preserved in provenance.
    Sdef {
        /// First bone index.
        bone1: i64,
        /// Second bone index.
        bone2: i64,
        /// Weight of the first bone.
        weight: f32,
        /// SDEF C vector, verbatim.
        c: [f32; 3],
        /// SDEF R0 vector, verbatim.
        r0: [f32; 3],
        /// SDEF R1 vector, verbatim.
        r1: [f32; 3],
    },
    /// QDEF (2.1): dual-quaternion deform; degrades to linear blend.
    Qdef {
        /// Bone indices.
        bones: [i64; 4],
        /// Bone weights as authored.
        weights: [f32; 4],
    },
}

impl Deform {
    /// Census deform class slot: 0 BDEF1, 1 BDEF2, 2 BDEF4, 3 SDEF, 4 QDEF.
    pub fn class_slot(&self) -> usize {
        match self {
            Deform::Bdef1 { .. } => 0,
            Deform::Bdef2 { .. } => 1,
            Deform::Bdef4 { .. } => 2,
            Deform::Sdef { .. } => 3,
            Deform::Qdef { .. } => 4,
        }
    }
}

/// One retained vertex.
#[derive(Debug, Clone, PartialEq)]
pub struct Vertex {
    /// Position (PMX space: left-handed, Y-up, PMX units).
    pub position: [f32; 3],
    /// Normal (PMX space).
    pub normal: [f32; 3],
    /// UV coordinates.
    pub uv: [f32; 2],
    /// Additional vec4 payloads (0..=4 entries).
    pub additional: Vec<[f32; 4]>,
    /// Skinning payload.
    pub deform: Deform,
    /// Edge scale.
    pub edge_scale: f32,
}

/// One retained texture-table entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Texture {
    /// Decoded path string, verbatim.
    pub path: String,
    /// Raw stored path bytes (hashed by the census).
    pub raw_path: Vec<u8>,
}

/// Toon reference of a material.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToonRef {
    /// Reference is a texture-table index.
    Texture(i64),
    /// Reference is an internal ramp number (0..=10 in practice).
    Internal(u8),
}

/// One retained material, with its surface span size.
#[derive(Debug, Clone, PartialEq)]
pub struct Material {
    /// Local name, verbatim.
    pub name_local: String,
    /// Universal name, verbatim.
    pub name_universal: String,
    /// Diffuse colour RGBA.
    pub diffuse: [f32; 4],
    /// Specular colour RGB.
    pub specular: [f32; 3],
    /// Specular strength.
    pub specular_strength: f32,
    /// Ambient colour RGB.
    pub ambient: [f32; 3],
    /// Drawing flags byte.
    pub flags: u8,
    /// Edge colour RGBA.
    pub edge_color: [f32; 4],
    /// Edge scale.
    pub edge_scale: f32,
    /// Diffuse texture-table index, or -1.
    pub texture_index: i64,
    /// Environment (sphere) texture-table index, or -1.
    pub environment_index: i64,
    /// Environment blend mode byte.
    pub environment_blend: u8,
    /// Toon reference.
    pub toon: ToonRef,
    /// Meta text, verbatim.
    pub meta: String,
    /// Surface-index count covered by this material (multiple of 3
    /// in well-formed files; validated by the census sum check).
    pub surface_count: i64,
}

/// Bone tail: an indexed child bone or a direction offset.
#[derive(Debug, Clone, PartialEq)]
pub enum BoneTail {
    /// Tail is a bone index (flag bit 0 set).
    Indexed(i64),
    /// Tail is a position offset (flag bit 0 clear).
    Offset([f32; 3]),
}

/// Inherit (grant) block of a bone.
#[derive(Debug, Clone, PartialEq)]
pub struct Inherit {
    /// Source bone index.
    pub parent_bone: i64,
    /// Influence ratio.
    pub influence: f32,
}

/// One IK link inside a bone IK block.
#[derive(Debug, Clone, PartialEq)]
pub struct IkLink {
    /// Link bone index.
    pub bone: i64,
    /// Angle limits, when present: (min, max) radians per axis.
    pub limits: Option<([f32; 3], [f32; 3])>,
}

/// IK block of a bone.
#[derive(Debug, Clone, PartialEq)]
pub struct Ik {
    /// IK target bone index.
    pub target: i64,
    /// Loop (iteration) count.
    pub loop_count: i32,
    /// Per-iteration angle limit, radians.
    pub limit_radian: f32,
    /// IK links in file order.
    pub links: Vec<IkLink>,
}

/// One retained bone, with all flag-conditional blocks.
#[derive(Debug, Clone, PartialEq)]
pub struct Bone {
    /// Local name, verbatim.
    pub name_local: String,
    /// Universal name, verbatim.
    pub name_universal: String,
    /// Rest position (PMX space).
    pub position: [f32; 3],
    /// Parent bone index, or -1.
    pub parent: i64,
    /// Deformation layer.
    pub layer: i32,
    /// Bone flags bit field, verbatim.
    pub flags: u16,
    /// Tail (indexed or offset).
    pub tail: BoneTail,
    /// Inherit block (flags bits 8/9).
    pub inherit: Option<Inherit>,
    /// Fixed-axis direction (flag bit 10).
    pub fixed_axis: Option<[f32; 3]>,
    /// Local coordinate axes X/Z (flag bit 11).
    pub local_axes: Option<([f32; 3], [f32; 3])>,
    /// External parent index (flag bit 13).
    pub external_parent: Option<i64>,
    /// IK block (flag bit 5).
    pub ik: Option<Ik>,
}

/// One morph offset record, retained in full per class.
#[derive(Debug, Clone, PartialEq)]
pub enum MorphOffset {
    /// Group / flip offset: target morph + influence.
    Group {
        /// Target morph index.
        morph: i64,
        /// Influence.
        influence: f32,
    },
    /// Vertex offset: vertex + translation.
    Vertex {
        /// Vertex index.
        vertex: i64,
        /// Translation (PMX space).
        translation: [f32; 3],
    },
    /// Bone offset: bone + translation + quaternion rotation.
    Bone {
        /// Bone index.
        bone: i64,
        /// Translation (PMX space).
        translation: [f32; 3],
        /// Rotation quaternion (x, y, z, w).
        rotation: [f32; 4],
    },
    /// UV offset (classes 3..=7): vertex + vec4 delta.
    Uv {
        /// Vertex index.
        vertex: i64,
        /// Vec4 delta; the first two components drive TEXCOORD_0.
        delta: [f32; 4],
    },
    /// Material offset: full tint block.
    Material {
        /// Material index (-1 = all materials in PMX practice).
        material: i64,
        /// Operation byte (0 = multiply, 1 = add in the spec).
        op: u8,
        /// Diffuse tint.
        diffuse: [f32; 4],
        /// Specular tint.
        specular: [f32; 3],
        /// Specularity tint.
        specular_strength: f32,
        /// Ambient tint.
        ambient: [f32; 3],
        /// Edge colour tint.
        edge_color: [f32; 4],
        /// Edge size tint.
        edge_size: f32,
        /// Texture tint.
        texture_tint: [f32; 4],
        /// Environment tint.
        environment_tint: [f32; 4],
        /// Toon tint.
        toon_tint: [f32; 4],
    },
    /// Impulse offset: rigid body + velocity kick.
    Impulse {
        /// Rigid-body index.
        rigid: i64,
        /// Local-flag byte.
        local_flag: u8,
        /// Velocity.
        velocity: [f32; 3],
        /// Torque.
        torque: [f32; 3],
    },
}

/// One retained morph.
#[derive(Debug, Clone, PartialEq)]
pub struct Morph {
    /// Local name, verbatim.
    pub name_local: String,
    /// Universal name, verbatim.
    pub name_universal: String,
    /// Panel type byte.
    pub panel: u8,
    /// Morph class byte (0 group, 1 vertex, 2 bone, 3..=7 uv,
    /// 8 material, 9 flip, 10 impulse).
    pub morph_type: u8,
    /// Offset records in file order.
    pub offsets: Vec<MorphOffset>,
}

impl Morph {
    /// Census class name for this morph.
    pub fn type_name(&self) -> &'static str {
        match self.morph_type {
            0 => "group",
            1 => "vertex",
            2 => "bone",
            3..=7 => "uv",
            8 => "material",
            9 => "flip",
            10 => "impulse",
            _ => "unknown",
        }
    }

    /// Census tally slot: [group, vertex, bone, uv, material, flip, impulse].
    pub fn tally_slot(&self) -> usize {
        match self.morph_type {
            0 => 0,
            1 => 1,
            2 => 2,
            3..=7 => 3,
            8 => 4,
            9 => 5,
            10 => 6,
            _ => 0,
        }
    }
}

/// One display-frame element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayElement {
    /// Element kind: 0 bone, 1 morph.
    pub kind: u8,
    /// Bone or morph index.
    pub index: i64,
}

/// One retained display frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisplayFrame {
    /// Local name, verbatim.
    pub name_local: String,
    /// Universal name, verbatim.
    pub name_universal: String,
    /// Special-frame flag.
    pub special_flag: u8,
    /// Elements in file order.
    pub elements: Vec<DisplayElement>,
}

/// One retained rigid body.
#[derive(Debug, Clone, PartialEq)]
pub struct RigidBody {
    /// Local name, verbatim.
    pub name_local: String,
    /// Universal name, verbatim.
    pub name_universal: String,
    /// Related bone index, or -1.
    pub bone: i64,
    /// Collision group id.
    pub group_id: u8,
    /// Non-collision mask.
    pub non_collision_mask: i16,
    /// Shape byte: 0 sphere, 1 box, 2 capsule.
    pub shape: u8,
    /// Shape size.
    pub size: [f32; 3],
    /// Shape position.
    pub position: [f32; 3],
    /// Shape rotation.
    pub rotation: [f32; 3],
    /// Mass.
    pub mass: f32,
    /// Move attenuation.
    pub move_attenuation: f32,
    /// Rotation damping.
    pub rotation_damping: f32,
    /// Repulsion.
    pub repulsion: f32,
    /// Friction.
    pub friction: f32,
    /// Physics mode byte: 0 follow_bone, 1 physics, 2 physics_plus_bone.
    pub physics_mode: u8,
}

impl RigidBody {
    /// Census shape class name.
    pub fn shape_name(&self) -> &'static str {
        match self.shape {
            0 => "sphere",
            1 => "box",
            2 => "capsule",
            _ => "unknown",
        }
    }

    /// Census physics-mode name.
    pub fn physics_mode_name(&self) -> &'static str {
        match self.physics_mode {
            0 => "follow_bone",
            1 => "physics",
            2 => "physics_plus_bone",
            _ => "unknown",
        }
    }
}

/// One retained joint (spring 6DOF and friends).
#[derive(Debug, Clone, PartialEq)]
pub struct Joint {
    /// Local name, verbatim.
    pub name_local: String,
    /// Universal name, verbatim.
    pub name_universal: String,
    /// Joint type byte.
    pub joint_type: u8,
    /// First rigid-body index.
    pub rigid_a: i64,
    /// Second rigid-body index.
    pub rigid_b: i64,
    /// Position.
    pub position: [f32; 3],
    /// Rotation.
    pub rotation: [f32; 3],
    /// Position minimum.
    pub position_min: [f32; 3],
    /// Position maximum.
    pub position_max: [f32; 3],
    /// Rotation minimum.
    pub rotation_min: [f32; 3],
    /// Rotation maximum.
    pub rotation_max: [f32; 3],
    /// Position spring constants.
    pub position_spring: [f32; 3],
    /// Rotation spring constants.
    pub rotation_spring: [f32; 3],
}

/// One soft-body anchor rigid.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SoftAnchor {
    /// Rigid-body index.
    pub rigid: i64,
    /// Vertex index.
    pub vertex: i64,
    /// Near-mode flag.
    pub near_mode: u8,
}

/// One retained PMX 2.1 soft body.
#[derive(Debug, Clone, PartialEq)]
pub struct SoftBody {
    /// Local name, verbatim.
    pub name_local: String,
    /// Universal name, verbatim.
    pub name_universal: String,
    /// Shape byte.
    pub shape: u8,
    /// Material index.
    pub material_index: i64,
    /// Collision group id.
    pub group_id: u8,
    /// Non-collision mask.
    pub non_collision_mask: i16,
    /// Flags byte.
    pub flags: u8,
    /// B-link distance.
    pub b_link_distance: i32,
    /// Cluster count.
    pub cluster_count: i32,
    /// The 21 f32 config/material scalars, in file order.
    pub config_scalars: Vec<f32>,
    /// The 7 i32 iteration/stiffness fields, in file order.
    pub iteration_fields: Vec<i32>,
    /// Anchor rigids.
    pub anchors: Vec<SoftAnchor>,
    /// Pinned vertex indices.
    pub pins: Vec<i64>,
}

/// A fully retained PMX model: the conversion input contract.
#[derive(Debug, Clone, PartialEq)]
pub struct PmxModel {
    /// PMX version (2.0 or 2.1).
    pub pmx_version: f32,
    /// Header globals.
    pub globals: Globals,
    /// Model local name, verbatim.
    pub model_name_local: String,
    /// Model universal name, verbatim.
    pub model_name_universal: String,
    /// Model local comment, verbatim.
    pub model_comment_local: String,
    /// Model universal comment, verbatim.
    pub model_comment_universal: String,
    /// Vertices in file order.
    pub vertices: Vec<Vertex>,
    /// Surface indices in file order (vertex indices).
    pub surface_indices: Vec<i64>,
    /// Texture table.
    pub textures: Vec<Texture>,
    /// Materials in file order; spans partition `surface_indices`.
    pub materials: Vec<Material>,
    /// Bones in file order.
    pub bones: Vec<Bone>,
    /// Morphs in file order.
    pub morphs: Vec<Morph>,
    /// Display frames in file order.
    pub display_frames: Vec<DisplayFrame>,
    /// Rigid bodies in file order.
    pub rigid_bodies: Vec<RigidBody>,
    /// Joints in file order.
    pub joints: Vec<Joint>,
    /// Soft bodies (2.1 only).
    pub soft_bodies: Vec<SoftBody>,
}

// ---------------------------------------------------------------------
// Cursor + section parsers (appended to parse.rs at install time)
// ---------------------------------------------------------------------

/// A bounds-checked little-endian cursor over the input.
struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
    section: &'static str,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Cursor {
            bytes,
            pos: 0,
            section: "header",
        }
    }

    fn set_section(&mut self, section: &'static str) {
        self.section = section;
    }

    fn remaining(&self) -> usize {
        self.bytes.len() - self.pos
    }

    fn take(&mut self, count: usize) -> ParseResult<&'a [u8]> {
        let end = self
            .pos
            .checked_add(count)
            .ok_or(CensusError::Truncated(self.section))?;
        if end > self.bytes.len() {
            return Err(CensusError::Truncated(self.section));
        }
        let slice = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(slice)
    }

    fn u8(&mut self) -> ParseResult<u8> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> ParseResult<u16> {
        let b = self.take(2)?;
        Ok(u16::from_le_bytes([b[0], b[1]]))
    }

    fn i16(&mut self) -> ParseResult<i16> {
        Ok(self.u16()? as i16)
    }

    fn i32(&mut self) -> ParseResult<i32> {
        let b = self.take(4)?;
        Ok(i32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    }

    fn f32(&mut self) -> ParseResult<f32> {
        Ok(f32::from_bits(self.i32()? as u32))
    }

    fn vec2(&mut self) -> ParseResult<[f32; 2]> {
        Ok([self.f32()?, self.f32()?])
    }

    fn vec3(&mut self) -> ParseResult<[f32; 3]> {
        Ok([self.f32()?, self.f32()?, self.f32()?])
    }

    fn vec4(&mut self) -> ParseResult<[f32; 4]> {
        Ok([self.f32()?, self.f32()?, self.f32()?, self.f32()?])
    }

    /// Read a count field and validate it against its cap before
    /// any caller loops or allocates on it.
    fn count(&mut self, field: &'static str, cap: i64) -> ParseResult<i64> {
        let value = i64::from(self.i32()?);
        if value < 0 || value > cap {
            return Err(CensusError::CountOutOfRange(field, value));
        }
        Ok(value)
    }

    /// Read a sized index. Size comes from the header globals and
    /// was validated to be 1, 2, or 4 at header parse time.
    fn index(&mut self, size: u8, kind: IndexKind) -> ParseResult<i64> {
        let value = match (size, kind) {
            (1, IndexKind::Vertex) => i64::from(self.u8()?),
            (1, IndexKind::Signed) => i64::from(self.u8()? as i8),
            (2, IndexKind::Vertex) => i64::from(self.u16()?),
            (2, IndexKind::Signed) => i64::from(self.i16()?),
            (4, _) => i64::from(self.i32()?),
            _ => return Err(CensusError::BadGlobals("index size")),
        };
        Ok(value)
    }

    /// Read one `text` value: an i32 byte length followed by bytes
    /// in the header's declared encoding. The raw bytes are also
    /// returned because the census hashes texture paths as stored.
    fn text(&mut self, utf8: bool, field: &'static str) -> ParseResult<(String, Vec<u8>)> {
        let len = i64::from(self.i32()?);
        if !(0..=TEXT_BYTES_MAX).contains(&len) {
            return Err(CensusError::CountOutOfRange(field, len));
        }
        let raw = self.take(len as usize)?.to_vec();
        let decoded = if utf8 {
            String::from_utf8(raw.clone()).map_err(|_| CensusError::InvalidText(field))?
        } else {
            if raw.len() % 2 != 0 {
                return Err(CensusError::InvalidText(field));
            }
            let units: Vec<u16> = raw
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| u16::from_le_bytes(*pair))
                .collect();
            String::from_utf16(&units).map_err(|_| CensusError::InvalidText(field))?
        };
        Ok((decoded, raw))
    }
}

fn check_index_size(value: u8) -> ParseResult<u8> {
    match value {
        1 | 2 | 4 => Ok(value),
        _ => Err(CensusError::BadGlobals("index size")),
    }
}

/// Parse a PMX 2.0 / 2.1 byte slice into a fully retained model.
///
/// Bounded by the `*_MAX` constants; returns a typed
/// [`CensusError`] for every malformed-input case.
pub fn parse_model(bytes: &[u8]) -> ParseResult<PmxModel> {
    if bytes.len() > INPUT_BYTES_MAX {
        return Err(CensusError::CountOutOfRange(
            "input bytes",
            bytes.len() as i64,
        ));
    }
    let mut cur = Cursor::new(bytes);

    // ---- Header ---------------------------------------------------
    let magic = cur.take(4)?;
    if magic != b"PMX " {
        return Err(CensusError::BadMagic);
    }
    let version_bits = cur.i32()? as u32;
    let pmx_version = f32::from_bits(version_bits);
    if version_bits != 2.0f32.to_bits() && version_bits != 2.1f32.to_bits() {
        return Err(CensusError::UnsupportedVersion(version_bits));
    }
    let globals_count = cur.u8()?;
    if globals_count != 8 {
        return Err(CensusError::BadGlobals("globals count"));
    }
    let g = cur.take(8)?;
    let globals = Globals {
        encoding_utf8: match g[0] {
            0 => false,
            1 => true,
            _ => return Err(CensusError::BadGlobals("text encoding")),
        },
        additional_vec4_count: match g[1] {
            0..=4 => g[1],
            _ => return Err(CensusError::BadGlobals("additional vec4 count")),
        },
        vertex_index_size: check_index_size(g[2])?,
        texture_index_size: check_index_size(g[3])?,
        material_index_size: check_index_size(g[4])?,
        bone_index_size: check_index_size(g[5])?,
        morph_index_size: check_index_size(g[6])?,
        rigid_index_size: check_index_size(g[7])?,
    };
    let utf8 = globals.encoding_utf8;

    // ---- Model information ----------------------------------------
    cur.set_section("model info");
    let (model_name_local, _) = cur.text(utf8, "model name local")?;
    let (model_name_universal, _) = cur.text(utf8, "model name universal")?;
    let (model_comment_local, _) = cur.text(utf8, "model comment local")?;
    let (model_comment_universal, _) = cur.text(utf8, "model comment universal")?;

    // ---- Vertices ---------------------------------------------------
    cur.set_section("vertices");
    let vertex_count = cur.count("vertex count", VERTEX_COUNT_MAX)?;
    let mut vertices = Vec::with_capacity(vertex_count as usize);
    for _ in 0..vertex_count {
        let position = cur.vec3()?;
        let normal = cur.vec3()?;
        let uv = cur.vec2()?;
        let mut additional = Vec::with_capacity(usize::from(globals.additional_vec4_count));
        for _ in 0..globals.additional_vec4_count {
            additional.push(cur.vec4()?);
        }
        let deform_type = cur.u8()?;
        let deform = parse_deform(&mut cur, deform_type, globals.bone_index_size)?;
        let edge_scale = cur.f32()?;
        vertices.push(Vertex {
            position,
            normal,
            uv,
            additional,
            deform,
            edge_scale,
        });
    }

    // ---- Surfaces ---------------------------------------------------
    cur.set_section("surfaces");
    let surface_count = cur.count("surface count", SURFACE_COUNT_MAX)?;
    if surface_count % 3 != 0 {
        return Err(CensusError::InvalidValue("surface count", surface_count));
    }
    let mut surface_indices = Vec::with_capacity(surface_count as usize);
    for _ in 0..surface_count {
        surface_indices.push(cur.index(globals.vertex_index_size, IndexKind::Vertex)?);
    }

    // ---- Textures ---------------------------------------------------
    cur.set_section("textures");
    let texture_count = cur.count("texture count", TEXTURE_COUNT_MAX)?;
    let mut textures = Vec::with_capacity(texture_count as usize);
    for _ in 0..texture_count {
        let (path, raw_path) = cur.text(utf8, "texture path")?;
        textures.push(Texture { path, raw_path });
    }

    // ---- Materials --------------------------------------------------
    cur.set_section("materials");
    let material_count = cur.count("material count", MATERIAL_COUNT_MAX)?;
    let mut materials = Vec::with_capacity(material_count as usize);
    for _ in 0..material_count {
        materials.push(parse_material(&mut cur, &globals)?);
    }

    // ---- Bones --------------------------------------------------------
    cur.set_section("bones");
    let bone_count = cur.count("bone count", BONE_COUNT_MAX)?;
    let mut bones = Vec::with_capacity(bone_count as usize);
    for _ in 0..bone_count {
        bones.push(parse_bone(&mut cur, &globals)?);
    }

    // ---- Morphs -------------------------------------------------------
    cur.set_section("morphs");
    let morph_count = cur.count("morph count", MORPH_COUNT_MAX)?;
    let mut morphs = Vec::with_capacity(morph_count as usize);
    for _ in 0..morph_count {
        morphs.push(parse_morph(&mut cur, &globals)?);
    }

    // ---- Display frames -------------------------------------------------
    cur.set_section("display frames");
    let frame_count = cur.count("display frame count", DISPLAY_FRAME_COUNT_MAX)?;
    let mut display_frames = Vec::with_capacity(frame_count as usize);
    for _ in 0..frame_count {
        let (name_local, _) = cur.text(utf8, "display name local")?;
        let (name_universal, _) = cur.text(utf8, "display name universal")?;
        let special_flag = cur.u8()?;
        let element_count = cur.count("display element count", DISPLAY_ELEMENT_COUNT_MAX)?;
        let mut elements = Vec::with_capacity(element_count as usize);
        for _ in 0..element_count {
            let kind = cur.u8()?;
            let index = match kind {
                0 => cur.index(globals.bone_index_size, IndexKind::Signed)?,
                1 => cur.index(globals.morph_index_size, IndexKind::Signed)?,
                other => {
                    return Err(CensusError::InvalidValue(
                        "display element type",
                        i64::from(other),
                    ));
                }
            };
            elements.push(DisplayElement { kind, index });
        }
        display_frames.push(DisplayFrame {
            name_local,
            name_universal,
            special_flag,
            elements,
        });
    }

    // ---- Rigid bodies -----------------------------------------------------
    cur.set_section("rigid bodies");
    let rigid_count = cur.count("rigid body count", RIGID_BODY_COUNT_MAX)?;
    let mut rigid_bodies = Vec::with_capacity(rigid_count as usize);
    for _ in 0..rigid_count {
        rigid_bodies.push(parse_rigid(&mut cur, &globals)?);
    }

    // ---- Joints ------------------------------------------------------------
    cur.set_section("joints");
    let joint_count = cur.count("joint count", JOINT_COUNT_MAX)?;
    let mut joints = Vec::with_capacity(joint_count as usize);
    for _ in 0..joint_count {
        joints.push(parse_joint(&mut cur, &globals)?);
    }

    // ---- Soft bodies (PMX 2.1 only) -----------------------------------------
    let mut soft_bodies = Vec::new();
    if version_bits == 2.1f32.to_bits() {
        cur.set_section("soft bodies");
        let soft_count = cur.count("soft body count", SOFT_BODY_COUNT_MAX)?;
        soft_bodies.reserve(soft_count as usize);
        for _ in 0..soft_count {
            soft_bodies.push(parse_soft_body(&mut cur, &globals)?);
        }
    }

    if cur.remaining() != 0 {
        return Err(CensusError::TrailingBytes(cur.remaining()));
    }

    Ok(PmxModel {
        pmx_version,
        globals,
        model_name_local,
        model_name_universal,
        model_comment_local,
        model_comment_universal,
        vertices,
        surface_indices,
        textures,
        materials,
        bones,
        morphs,
        display_frames,
        rigid_bodies,
        joints,
        soft_bodies,
    })
}

fn parse_deform(cur: &mut Cursor<'_>, deform_type: u8, bone_size: u8) -> ParseResult<Deform> {
    let bone = |c: &mut Cursor<'_>| c.index(bone_size, IndexKind::Signed);
    match deform_type {
        0 => Ok(Deform::Bdef1 { bone: bone(cur)? }),
        1 => {
            let bone1 = bone(cur)?;
            let bone2 = bone(cur)?;
            let weight = cur.f32()?;
            Ok(Deform::Bdef2 {
                bone1,
                bone2,
                weight,
            })
        }
        2 | 4 => {
            let mut bones = [0i64; 4];
            for slot in &mut bones {
                *slot = bone(cur)?;
            }
            let mut weights = [0f32; 4];
            for slot in &mut weights {
                *slot = cur.f32()?;
            }
            if deform_type == 2 {
                Ok(Deform::Bdef4 { bones, weights })
            } else {
                Ok(Deform::Qdef { bones, weights })
            }
        }
        3 => {
            let bone1 = bone(cur)?;
            let bone2 = bone(cur)?;
            let weight = cur.f32()?;
            let c = cur.vec3()?;
            let r0 = cur.vec3()?;
            let r1 = cur.vec3()?;
            Ok(Deform::Sdef {
                bone1,
                bone2,
                weight,
                c,
                r0,
                r1,
            })
        }
        other => Err(CensusError::InvalidValue("deform type", i64::from(other))),
    }
}

fn parse_material(cur: &mut Cursor<'_>, globals: &Globals) -> ParseResult<Material> {
    let utf8 = globals.encoding_utf8;
    let (name_local, _) = cur.text(utf8, "material name local")?;
    let (name_universal, _) = cur.text(utf8, "material name universal")?;
    let diffuse = cur.vec4()?;
    let specular = cur.vec3()?;
    let specular_strength = cur.f32()?;
    let ambient = cur.vec3()?;
    let flags = cur.u8()?;
    let edge_color = cur.vec4()?;
    let edge_scale = cur.f32()?;
    let texture_index = cur.index(globals.texture_index_size, IndexKind::Signed)?;
    let environment_index = cur.index(globals.texture_index_size, IndexKind::Signed)?;
    let environment_blend = cur.u8()?;
    let toon_reference = cur.u8()?;
    let toon = match toon_reference {
        0 => ToonRef::Texture(cur.index(globals.texture_index_size, IndexKind::Signed)?),
        1 => ToonRef::Internal(cur.u8()?),
        other => {
            return Err(CensusError::InvalidValue(
                "toon reference",
                i64::from(other),
            ));
        }
    };
    let (meta, _) = cur.text(utf8, "material meta")?;
    let surface_count = i64::from(cur.i32()?);
    if surface_count < 0 {
        return Err(CensusError::CountOutOfRange(
            "material surface count",
            surface_count,
        ));
    }
    Ok(Material {
        name_local,
        name_universal,
        diffuse,
        specular,
        specular_strength,
        ambient,
        flags,
        edge_color,
        edge_scale,
        texture_index,
        environment_index,
        environment_blend,
        toon,
        meta,
        surface_count,
    })
}

fn parse_bone(cur: &mut Cursor<'_>, globals: &Globals) -> ParseResult<Bone> {
    let utf8 = globals.encoding_utf8;
    let (name_local, _) = cur.text(utf8, "bone name local")?;
    let (name_universal, _) = cur.text(utf8, "bone name universal")?;
    let position = cur.vec3()?;
    let parent = cur.index(globals.bone_index_size, IndexKind::Signed)?;
    let layer = cur.i32()?;
    let flags = cur.u16()?;
    let tail = if flags & 0x0001 != 0 {
        BoneTail::Indexed(cur.index(globals.bone_index_size, IndexKind::Signed)?)
    } else {
        BoneTail::Offset(cur.vec3()?)
    };
    let inherit = if flags & 0x0300 != 0 {
        let parent_bone = cur.index(globals.bone_index_size, IndexKind::Signed)?;
        let influence = cur.f32()?;
        Some(Inherit {
            parent_bone,
            influence,
        })
    } else {
        None
    };
    let fixed_axis = if flags & 0x0400 != 0 {
        Some(cur.vec3()?)
    } else {
        None
    };
    let local_axes = if flags & 0x0800 != 0 {
        let x = cur.vec3()?;
        let z = cur.vec3()?;
        Some((x, z))
    } else {
        None
    };
    let external_parent = if flags & 0x2000 != 0 {
        Some(cur.index(globals.bone_index_size, IndexKind::Signed)?)
    } else {
        None
    };
    let ik = if flags & 0x0020 != 0 {
        let target = cur.index(globals.bone_index_size, IndexKind::Signed)?;
        let loop_count = cur.i32()?;
        let limit_radian = cur.f32()?;
        let link_count = cur.count("ik link count", IK_LINK_COUNT_MAX)?;
        let mut links = Vec::with_capacity(link_count as usize);
        for _ in 0..link_count {
            let bone = cur.index(globals.bone_index_size, IndexKind::Signed)?;
            let has_limits = cur.u8()?;
            let limits = if has_limits == 1 {
                let min = cur.vec3()?;
                let max = cur.vec3()?;
                Some((min, max))
            } else {
                None
            };
            links.push(IkLink { bone, limits });
        }
        Some(Ik {
            target,
            loop_count,
            limit_radian,
            links,
        })
    } else {
        None
    };
    Ok(Bone {
        name_local,
        name_universal,
        position,
        parent,
        layer,
        flags,
        tail,
        inherit,
        fixed_axis,
        local_axes,
        external_parent,
        ik,
    })
}

fn parse_morph(cur: &mut Cursor<'_>, globals: &Globals) -> ParseResult<Morph> {
    let utf8 = globals.encoding_utf8;
    let (name_local, _) = cur.text(utf8, "morph name local")?;
    let (name_universal, _) = cur.text(utf8, "morph name universal")?;
    let panel = cur.u8()?;
    let morph_type = cur.u8()?;
    match morph_type {
        0..=10 => {}
        other => {
            return Err(CensusError::InvalidValue("morph type", i64::from(other)));
        }
    }
    let offset_count = cur.count("morph offset count", MORPH_OFFSET_COUNT_MAX)?;
    let mut offsets = Vec::with_capacity(offset_count as usize);
    for _ in 0..offset_count {
        offsets.push(parse_morph_offset(cur, globals, morph_type)?);
    }
    Ok(Morph {
        name_local,
        name_universal,
        panel,
        morph_type,
        offsets,
    })
}

fn parse_morph_offset(
    cur: &mut Cursor<'_>,
    globals: &Globals,
    morph_type: u8,
) -> ParseResult<MorphOffset> {
    match morph_type {
        0 | 9 => {
            let morph = cur.index(globals.morph_index_size, IndexKind::Signed)?;
            let influence = cur.f32()?;
            Ok(MorphOffset::Group { morph, influence })
        }
        1 => {
            let vertex = cur.index(globals.vertex_index_size, IndexKind::Vertex)?;
            let translation = cur.vec3()?;
            Ok(MorphOffset::Vertex {
                vertex,
                translation,
            })
        }
        2 => {
            let bone = cur.index(globals.bone_index_size, IndexKind::Signed)?;
            let translation = cur.vec3()?;
            let rotation = cur.vec4()?;
            Ok(MorphOffset::Bone {
                bone,
                translation,
                rotation,
            })
        }
        3..=7 => {
            let vertex = cur.index(globals.vertex_index_size, IndexKind::Vertex)?;
            let delta = cur.vec4()?;
            Ok(MorphOffset::Uv { vertex, delta })
        }
        8 => {
            let material = cur.index(globals.material_index_size, IndexKind::Signed)?;
            let op = cur.u8()?;
            let diffuse = cur.vec4()?;
            let specular = cur.vec3()?;
            let specular_strength = cur.f32()?;
            let ambient = cur.vec3()?;
            let edge_color = cur.vec4()?;
            let edge_size = cur.f32()?;
            let texture_tint = cur.vec4()?;
            let environment_tint = cur.vec4()?;
            let toon_tint = cur.vec4()?;
            Ok(MorphOffset::Material {
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
            })
        }
        10 => {
            let rigid = cur.index(globals.rigid_index_size, IndexKind::Signed)?;
            let local_flag = cur.u8()?;
            let velocity = cur.vec3()?;
            let torque = cur.vec3()?;
            Ok(MorphOffset::Impulse {
                rigid,
                local_flag,
                velocity,
                torque,
            })
        }
        other => Err(CensusError::InvalidValue("morph type", i64::from(other))),
    }
}

fn parse_rigid(cur: &mut Cursor<'_>, globals: &Globals) -> ParseResult<RigidBody> {
    let utf8 = globals.encoding_utf8;
    let (name_local, _) = cur.text(utf8, "rigid name local")?;
    let (name_universal, _) = cur.text(utf8, "rigid name universal")?;
    let bone = cur.index(globals.bone_index_size, IndexKind::Signed)?;
    let group_id = cur.u8()?;
    let non_collision_mask = cur.i16()?;
    let shape = cur.u8()?;
    match shape {
        0..=2 => {}
        other => {
            return Err(CensusError::InvalidValue("rigid shape", i64::from(other)));
        }
    }
    let size = cur.vec3()?;
    let position = cur.vec3()?;
    let rotation = cur.vec3()?;
    let mass = cur.f32()?;
    let move_attenuation = cur.f32()?;
    let rotation_damping = cur.f32()?;
    let repulsion = cur.f32()?;
    let friction = cur.f32()?;
    let physics_mode = cur.u8()?;
    match physics_mode {
        0..=2 => {}
        other => {
            return Err(CensusError::InvalidValue("physics mode", i64::from(other)));
        }
    }
    Ok(RigidBody {
        name_local,
        name_universal,
        bone,
        group_id,
        non_collision_mask,
        shape,
        size,
        position,
        rotation,
        mass,
        move_attenuation,
        rotation_damping,
        repulsion,
        friction,
        physics_mode,
    })
}

fn parse_joint(cur: &mut Cursor<'_>, globals: &Globals) -> ParseResult<Joint> {
    let utf8 = globals.encoding_utf8;
    let (name_local, _) = cur.text(utf8, "joint name local")?;
    let (name_universal, _) = cur.text(utf8, "joint name universal")?;
    let joint_type = cur.u8()?;
    if joint_type > 5 {
        return Err(CensusError::InvalidValue(
            "joint type",
            i64::from(joint_type),
        ));
    }
    let rigid_a = cur.index(globals.rigid_index_size, IndexKind::Signed)?;
    let rigid_b = cur.index(globals.rigid_index_size, IndexKind::Signed)?;
    // position, rotation, position min/max, rotation min/max,
    // position spring, rotation spring: eight vec3 fields.
    let position = cur.vec3()?;
    let rotation = cur.vec3()?;
    let position_min = cur.vec3()?;
    let position_max = cur.vec3()?;
    let rotation_min = cur.vec3()?;
    let rotation_max = cur.vec3()?;
    let position_spring = cur.vec3()?;
    let rotation_spring = cur.vec3()?;
    Ok(Joint {
        name_local,
        name_universal,
        joint_type,
        rigid_a,
        rigid_b,
        position,
        rotation,
        position_min,
        position_max,
        rotation_min,
        rotation_max,
        position_spring,
        rotation_spring,
    })
}

fn parse_soft_body(cur: &mut Cursor<'_>, globals: &Globals) -> ParseResult<SoftBody> {
    let utf8 = globals.encoding_utf8;
    let (name_local, _) = cur.text(utf8, "soft body name local")?;
    let (name_universal, _) = cur.text(utf8, "soft body name universal")?;
    let shape = cur.u8()?;
    let material_index = cur.index(globals.material_index_size, IndexKind::Signed)?;
    let group_id = cur.u8()?;
    let non_collision_mask = cur.i16()?;
    let flags = cur.u8()?;
    let b_link_distance = cur.i32()?;
    let cluster_count = cur.i32()?;
    let mut config_scalars = Vec::with_capacity(21);
    for _ in 0..21 {
        config_scalars.push(cur.f32()?);
    }
    let mut iteration_fields = Vec::with_capacity(7);
    for _ in 0..7 {
        iteration_fields.push(cur.i32()?);
    }
    let anchor_count = cur.count("soft body anchor count", DISPLAY_ELEMENT_COUNT_MAX)?;
    let mut anchors = Vec::with_capacity(anchor_count as usize);
    for _ in 0..anchor_count {
        let rigid = cur.index(globals.rigid_index_size, IndexKind::Signed)?;
        let vertex = cur.index(globals.vertex_index_size, IndexKind::Vertex)?;
        let near_mode = cur.u8()?;
        anchors.push(SoftAnchor {
            rigid,
            vertex,
            near_mode,
        });
    }
    let pin_count = cur.count("soft body pin count", VERTEX_COUNT_MAX)?;
    let mut pins = Vec::with_capacity(pin_count as usize);
    for _ in 0..pin_count {
        pins.push(cur.index(globals.vertex_index_size, IndexKind::Vertex)?);
    }
    Ok(SoftBody {
        name_local,
        name_universal,
        shape,
        material_index,
        group_id,
        non_collision_mask,
        flags,
        b_link_distance,
        cluster_count,
        config_scalars,
        iteration_fields,
        anchors,
        pins,
    })
}
