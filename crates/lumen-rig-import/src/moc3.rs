//! From-scratch `.moc3` binary parser.
//!
//! The layout implemented here is the community OpenL2D
//! specification of the MOC3 format (no official public spec
//! exists and no Live2D code is used), verified byte-for-byte
//! against official free sample models spanning version bytes
//! 1, 3, 5, and 6: every count identity and every keyform-binding
//! product in those files checks out under the single section
//! map below. Version bytes 2 and 4 share the map but have no
//! local sample; the layout self-verification battery in
//! [`Moc3File::parse`] is the guard that keeps any wrong layout
//! from degrading silently — it fails loudly instead.
//!
//! Format summary (little-endian throughout):
//! - 64-byte header: magic `MOC3`, version byte, endianness flag.
//! - Section offset table at 0x40: 160 `i32` entries holding
//!   **absolute byte offsets** from the start of the file.
//! - Count info table: 32 `i32` counters (23 used, rest padding).
//! - Sections are parallel arrays, one per attribute, indexed by
//!   the count info table.
//!
//! Keyform bindings resolve as: an object's *keyform binding*
//! slices the parameter-binding-index table; each entry names a
//! global parameter binding; a parameter binding is owned by
//! exactly one parameter (ownership ranges are stored per
//! parameter) and carries that parameter's key values for the
//! binding. Stored keyforms for an object are ordered with the
//! **first-listed binding axis varying fastest** (verified on
//! real models by centroid displacement: for a head warp bound
//! to [AngleY, AngleX], consecutive keyforms sweep pitch while
//! the slow index sweeps yaw).

use crate::error::{ConvertError, bounded_quote};

/// Header size in bytes.
const HEADER_BYTES: usize = 64;
/// Section offset table size in entries.
const SOT_ENTRIES: usize = 160;
/// Byte offset of the section offset table.
const SOT_OFFSET: usize = 0x40;
/// Size of one id block in bytes (`MOC3Id`).
const ID_BLOCK_BYTES: usize = 64;
/// Count info table size in `i32` fields.
const CIT_FIELDS: usize = 32;

// Section offset table slots (verified layout; see module docs).
mod slot {
    pub const COUNT_INFO_TABLE: usize = 0;
    pub const CANVAS_INFO: usize = 1;
    pub const PART_IDS: usize = 3;
    pub const PART_PARENT_INDICES: usize = 9;
    pub const DEFORMER_IDS: usize = 11;
    pub const DEFORMER_BINDING_INDICES: usize = 12;
    pub const DEFORMER_PARENT_PART_INDICES: usize = 15;
    pub const DEFORMER_PARENT_INDICES: usize = 16;
    pub const DEFORMER_TYPES: usize = 17;
    pub const DEFORMER_SPECIFIC_INDICES: usize = 18;
    pub const WARP_BINDING_INDICES: usize = 19;
    pub const WARP_BEGIN_INDICES: usize = 20;
    pub const WARP_COUNTS: usize = 21;
    pub const WARP_VERTEX_COUNTS: usize = 22;
    pub const WARP_ROWS: usize = 23;
    pub const WARP_COLUMNS: usize = 24;
    pub const ROTATION_BINDING_INDICES: usize = 25;
    pub const ROTATION_BEGIN_INDICES: usize = 26;
    pub const ROTATION_COUNTS: usize = 27;
    pub const ROTATION_BASE_ANGLES: usize = 28;
    pub const MESH_IDS: usize = 33;
    pub const MESH_BINDING_INDICES: usize = 34;
    pub const MESH_BEGIN_INDICES: usize = 35;
    pub const MESH_COUNTS: usize = 36;
    pub const MESH_VISIBLE: usize = 37;
    pub const MESH_ENABLED: usize = 38;
    pub const MESH_PARENT_PART_INDICES: usize = 39;
    pub const MESH_PARENT_DEFORMER_INDICES: usize = 40;
    pub const MESH_TEXTURE_NUMBERS: usize = 41;
    pub const MESH_DRAWABLE_FLAGS: usize = 42;
    pub const MESH_VERTEX_COUNTS: usize = 43;
    pub const MESH_UV_BEGIN_INDICES: usize = 44;
    pub const MESH_POS_INDEX_BEGIN_INDICES: usize = 45;
    pub const MESH_POS_INDEX_COUNTS: usize = 46;
    pub const MESH_MASK_BEGIN_INDICES: usize = 47;
    pub const MESH_MASK_COUNTS: usize = 48;
    pub const PARAMETER_IDS: usize = 50;
    pub const PARAMETER_MAX_VALUES: usize = 51;
    pub const PARAMETER_MIN_VALUES: usize = 52;
    pub const PARAMETER_DEFAULT_VALUES: usize = 53;
    pub const PARAMETER_REPEAT_TOGGLES: usize = 54;
    pub const PARAMETER_BINDING_BEGIN_INDICES: usize = 56;
    pub const PARAMETER_BINDING_COUNTS: usize = 57;
    pub const WARP_KEYFORM_OPACITIES: usize = 59;
    pub const WARP_KEYFORM_POSITION_BEGINS: usize = 60;
    pub const ROTATION_KEYFORM_OPACITIES: usize = 61;
    pub const ROTATION_KEYFORM_ANGLES: usize = 62;
    pub const ROTATION_KEYFORM_ORIGIN_X: usize = 63;
    pub const ROTATION_KEYFORM_ORIGIN_Y: usize = 64;
    pub const ROTATION_KEYFORM_SCALES: usize = 65;
    pub const ROTATION_KEYFORM_REFLECT_X: usize = 66;
    pub const ROTATION_KEYFORM_REFLECT_Y: usize = 67;
    pub const MESH_KEYFORM_OPACITIES: usize = 68;
    pub const MESH_KEYFORM_DRAW_ORDERS: usize = 69;
    pub const MESH_KEYFORM_POSITION_BEGINS: usize = 70;
    pub const KEYFORM_POSITIONS: usize = 71;
    pub const PARAMETER_BINDING_INDEX: usize = 72;
    pub const KEYFORM_BINDING_BEGIN_INDICES: usize = 73;
    pub const KEYFORM_BINDING_COUNTS: usize = 74;
    pub const PARAMETER_BINDING_KEY_BEGINS: usize = 75;
    pub const PARAMETER_BINDING_KEY_COUNTS: usize = 76;
    pub const KEYS: usize = 77;
    pub const UVS: usize = 78;
    pub const POSITION_INDICES: usize = 79;
    pub const DRAWABLE_MASKS: usize = 80;
    pub const DRAW_ORDER_GROUP_OBJECT_BEGINS: usize = 81;
    pub const DRAW_ORDER_GROUP_OBJECT_COUNTS: usize = 82;
    pub const DRAW_ORDER_GROUP_MAXIMUMS: usize = 84;
    pub const DRAW_ORDER_GROUP_MINIMUMS: usize = 85;
    pub const DRAW_ORDER_OBJECT_TYPES: usize = 86;
    pub const DRAW_ORDER_OBJECT_INDICES: usize = 87;
}

/// The 23 used counters of the count info table.
#[derive(Debug, Clone, Copy, Default)]
pub struct Counts {
    pub parts: usize,
    pub deformers: usize,
    pub warp_deformers: usize,
    pub rotation_deformers: usize,
    pub art_meshes: usize,
    pub parameters: usize,
    pub part_keyforms: usize,
    pub warp_keyforms: usize,
    pub rotation_keyforms: usize,
    pub art_mesh_keyforms: usize,
    pub keyform_positions: usize,
    pub parameter_binding_index: usize,
    pub keyform_bindings: usize,
    pub parameter_bindings: usize,
    pub keys: usize,
    pub uvs: usize,
    pub position_indices: usize,
    pub drawable_masks: usize,
    pub draw_order_groups: usize,
    pub draw_order_group_objects: usize,
    pub glue: usize,
    pub glue_info: usize,
    pub glue_keyforms: usize,
}

/// Canvas info: the model's authored pixel canvas.
#[derive(Debug, Clone, Copy)]
pub struct CanvasInfo {
    pub pixels_per_unit: f32,
    pub origin_x: f32,
    pub origin_y: f32,
    pub width: f32,
    pub height: f32,
}

/// One axis of a resolved keyform binding.
#[derive(Debug, Clone)]
pub struct BindingAxis {
    /// Index into the model's parameter table.
    pub parameter: usize,
    /// The key values for this axis, strictly increasing.
    pub keys: Vec<f32>,
}

/// A part, as stored.
#[derive(Debug, Clone)]
pub struct RawPart {
    pub id: String,
    pub parent: i32,
}

/// A parameter, as stored.
#[derive(Debug, Clone)]
pub struct RawParameter {
    pub id: String,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub is_repeat: bool,
}

/// Which specific-deformer table a deformer belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeformerKind {
    Warp(usize),
    Rotation(usize),
}

/// A deformer, as stored (unified table entry).
#[derive(Debug, Clone)]
pub struct RawDeformer {
    pub id: String,
    /// The keyform binding index as stored in the unified table;
    /// the battery cross-checks it against the specific table.
    pub binding: i32,
    pub parent_part: i32,
    pub parent_deformer: i32,
    pub kind: DeformerKind,
}

/// A warp deformer's own data plus its keyforms' scalars.
#[derive(Debug, Clone)]
pub struct RawWarp {
    pub binding: i32,
    pub begin: i32,
    pub count: i32,
    pub vertex_count: i32,
    /// Grid cell counts (vertex_count == (rows+1) * (columns+1),
    /// verified against real files).
    pub rows: i32,
    pub columns: i32,
    pub keyform_opacities: Vec<f32>,
    pub keyform_position_begins: Vec<i32>,
}

/// A rotation deformer's own data plus its keyforms.
#[derive(Debug, Clone)]
pub struct RawRotation {
    pub binding: i32,
    pub begin: i32,
    pub count: i32,
    pub base_angle: f32,
    pub keyforms: Vec<RawRotationKeyform>,
}

/// One rotation keyform, as stored.
#[derive(Debug, Clone, Copy)]
pub struct RawRotationKeyform {
    pub opacity: f32,
    pub angle: f32,
    pub origin: [f32; 2],
    pub scale: f32,
    pub reflect_x: bool,
    pub reflect_y: bool,
}

/// An art mesh, as stored.
#[derive(Debug, Clone)]
pub struct RawMesh {
    pub id: String,
    pub binding: i32,
    pub begin: i32,
    pub count: i32,
    pub is_visible: bool,
    pub is_enabled: bool,
    pub parent_part: i32,
    pub parent_deformer: i32,
    pub texture_number: i32,
    pub drawable_flag: u8,
    pub vertex_count: i32,
    pub uv_begin: i32,
    pub position_index_begin: i32,
    pub position_index_count: i32,
    pub mask_begin: i32,
    pub mask_count: i32,
    pub keyform_opacities: Vec<f32>,
    pub keyform_draw_orders: Vec<f32>,
    pub keyform_position_begins: Vec<i32>,
}

/// One draw order group: the objects it orders and its range.
#[derive(Debug, Clone)]
pub struct RawDrawOrderGroup {
    pub minimum: i32,
    pub maximum: i32,
    /// `(object_type, object_index)`; type 0 = art mesh, 1 = part.
    pub objects: Vec<(i32, i32)>,
}

/// A fully parsed moc3 file, layout-verified.
#[derive(Debug, Clone)]
pub struct Moc3File {
    pub version: u8,
    pub canvas: CanvasInfo,
    pub counts: Counts,
    pub parts: Vec<RawPart>,
    pub parameters: Vec<RawParameter>,
    pub deformers: Vec<RawDeformer>,
    pub warps: Vec<RawWarp>,
    pub rotations: Vec<RawRotation>,
    pub meshes: Vec<RawMesh>,
    /// Resolved keyform bindings, indexed by binding index.
    pub bindings: Vec<Vec<BindingAxis>>,
    pub keyform_positions: Vec<f32>,
    pub uvs: Vec<f32>,
    pub position_indices: Vec<i16>,
    pub drawable_masks: Vec<i32>,
    pub draw_order_groups: Vec<RawDrawOrderGroup>,
    /// Highest end offset into the keys array that any parameter
    /// binding references; the array may carry an unreferenced
    /// tail on newer versions (observed on SDK 5 exports).
    pub keys_referenced_end: usize,
}

/// Bounds-checked section reader over the file bytes.
struct Sections<'a> {
    bytes: &'a [u8],
    sot: Vec<i32>,
}

impl<'a> Sections<'a> {
    fn new(bytes: &'a [u8]) -> Result<Sections<'a>, ConvertError> {
        let sot_bytes = bytes
            .get(SOT_OFFSET..SOT_OFFSET + SOT_ENTRIES * 4)
            .ok_or_else(|| ConvertError::Moc3 {
                message: "file too small for the section offset table".to_string(),
            })?;
        let mut sot = Vec::with_capacity(SOT_ENTRIES);
        for chunk in sot_bytes.as_chunks::<4>().0 {
            sot.push(i32::from_le_bytes(*chunk));
        }
        Ok(Sections { bytes, sot })
    }

    /// The byte slice of one section: `count` elements of
    /// `element_bytes` each, starting at the section's absolute
    /// offset. All arithmetic is checked; a count that overruns
    /// the file is a structural error, never a panic.
    fn slice(
        &self,
        slot_index: usize,
        element_bytes: usize,
        count: usize,
        what: &str,
    ) -> Result<&'a [u8], ConvertError> {
        let raw_offset = self.sot[slot_index];
        if raw_offset < 0 {
            return Err(ConvertError::Moc3 {
                message: format!("{what}: negative section offset {raw_offset}"),
            });
        }
        let offset = raw_offset as usize;
        let length = element_bytes
            .checked_mul(count)
            .ok_or_else(|| ConvertError::Moc3 {
                message: format!("{what}: section size overflows"),
            })?;
        let end = offset
            .checked_add(length)
            .ok_or_else(|| ConvertError::Moc3 {
                message: format!("{what}: section end overflows"),
            })?;
        self.bytes
            .get(offset..end)
            .ok_or_else(|| ConvertError::Moc3 {
                message: format!(
                    "{what}: section [{offset}, {end}) exceeds file size {}",
                    self.bytes.len()
                ),
            })
    }

    fn i32s(&self, slot_index: usize, count: usize, what: &str) -> Result<Vec<i32>, ConvertError> {
        let raw = self.slice(slot_index, 4, count, what)?;
        let mut out = Vec::with_capacity(count);
        for chunk in raw.as_chunks::<4>().0 {
            out.push(i32::from_le_bytes(*chunk));
        }
        Ok(out)
    }

    fn f32s(&self, slot_index: usize, count: usize, what: &str) -> Result<Vec<f32>, ConvertError> {
        let raw = self.slice(slot_index, 4, count, what)?;
        let mut out = Vec::with_capacity(count);
        for chunk in raw.as_chunks::<4>().0 {
            let value = f32::from_le_bytes(*chunk);
            if !value.is_finite() {
                return Err(ConvertError::Moc3 {
                    message: format!("{what}: non-finite float in section"),
                });
            }
            out.push(value);
        }
        Ok(out)
    }

    fn i16s(&self, slot_index: usize, count: usize, what: &str) -> Result<Vec<i16>, ConvertError> {
        let raw = self.slice(slot_index, 2, count, what)?;
        let mut out = Vec::with_capacity(count);
        for chunk in raw.as_chunks::<2>().0 {
            out.push(i16::from_le_bytes(*chunk));
        }
        Ok(out)
    }

    fn u8s(&self, slot_index: usize, count: usize, what: &str) -> Result<Vec<u8>, ConvertError> {
        Ok(self.slice(slot_index, 1, count, what)?.to_vec())
    }

    fn bools(
        &self,
        slot_index: usize,
        count: usize,
        what: &str,
    ) -> Result<Vec<bool>, ConvertError> {
        let raw = self.i32s(slot_index, count, what)?;
        let mut out = Vec::with_capacity(count);
        for value in raw {
            match value {
                0 => out.push(false),
                1 => out.push(true),
                other => {
                    return Err(ConvertError::Moc3 {
                        message: format!("{what}: bool32 value {other} is not 0 or 1"),
                    });
                }
            }
        }
        Ok(out)
    }

    fn ids(
        &self,
        slot_index: usize,
        count: usize,
        what: &str,
    ) -> Result<Vec<String>, ConvertError> {
        let raw = self.slice(slot_index, ID_BLOCK_BYTES, count, what)?;
        let mut out = Vec::with_capacity(count);
        for block in raw.as_chunks::<ID_BLOCK_BYTES>().0 {
            let end = block
                .iter()
                .position(|byte| *byte == 0)
                .unwrap_or(ID_BLOCK_BYTES);
            let text = std::str::from_utf8(&block[..end]).map_err(|_| ConvertError::Moc3 {
                message: format!("{what}: id is not valid UTF-8"),
            })?;
            if text.is_empty() {
                return Err(ConvertError::Moc3 {
                    message: format!("{what}: empty id"),
                });
            }
            out.push(text.to_string());
        }
        Ok(out)
    }
}

/// Read a count from the count info table as a checked `usize`.
fn count(cit: &[i32], index: usize, what: &str) -> Result<usize, ConvertError> {
    let value = cit[index];
    if value < 0 {
        return Err(ConvertError::Moc3 {
            message: format!("count info table: {what} count is negative ({value})"),
        });
    }
    Ok(value as usize)
}

impl Moc3File {
    /// Parse and layout-verify a moc3 file from its bytes.
    pub fn parse(bytes: &[u8]) -> Result<Moc3File, ConvertError> {
        if bytes.len() < HEADER_BYTES {
            return Err(ConvertError::Moc3 {
                message: "file too small for the moc3 header".to_string(),
            });
        }
        if &bytes[0..4] != b"MOC3" {
            return Err(ConvertError::Moc3 {
                message: format!(
                    "bad magic: expected MOC3, found {:?}",
                    bounded_quote(&String::from_utf8_lossy(&bytes[0..4]))
                ),
            });
        }
        let version = bytes[4];
        if !(1..=6).contains(&version) {
            return Err(ConvertError::UnsupportedMoc3Version { found: version });
        }
        if bytes[5] != 0 {
            return Err(ConvertError::Moc3 {
                message: "big-endian moc3 files are not a known format variant".to_string(),
            });
        }
        let sections = Sections::new(bytes)?;
        let cit = sections.i32s(slot::COUNT_INFO_TABLE, CIT_FIELDS, "count info table")?;
        let counts = Counts {
            parts: count(&cit, 0, "parts")?,
            deformers: count(&cit, 1, "deformers")?,
            warp_deformers: count(&cit, 2, "warp deformers")?,
            rotation_deformers: count(&cit, 3, "rotation deformers")?,
            art_meshes: count(&cit, 4, "art meshes")?,
            parameters: count(&cit, 5, "parameters")?,
            part_keyforms: count(&cit, 6, "part keyforms")?,
            warp_keyforms: count(&cit, 7, "warp keyforms")?,
            rotation_keyforms: count(&cit, 8, "rotation keyforms")?,
            art_mesh_keyforms: count(&cit, 9, "art mesh keyforms")?,
            keyform_positions: count(&cit, 10, "keyform positions")?,
            parameter_binding_index: count(&cit, 11, "parameter binding index")?,
            keyform_bindings: count(&cit, 12, "keyform bindings")?,
            parameter_bindings: count(&cit, 13, "parameter bindings")?,
            keys: count(&cit, 14, "keys")?,
            uvs: count(&cit, 15, "uvs")?,
            position_indices: count(&cit, 16, "position indices")?,
            drawable_masks: count(&cit, 17, "drawable masks")?,
            draw_order_groups: count(&cit, 18, "draw order groups")?,
            draw_order_group_objects: count(&cit, 19, "draw order group objects")?,
            glue: count(&cit, 20, "glue")?,
            glue_info: count(&cit, 21, "glue info")?,
            glue_keyforms: count(&cit, 22, "glue keyforms")?,
        };
        let canvas = {
            let raw = sections.slice(slot::CANVAS_INFO, 1, 64, "canvas info")?;
            let read_f32 =
                |at: usize| f32::from_le_bytes([raw[at], raw[at + 1], raw[at + 2], raw[at + 3]]);
            let canvas = CanvasInfo {
                pixels_per_unit: read_f32(0),
                origin_x: read_f32(4),
                origin_y: read_f32(8),
                width: read_f32(12),
                height: read_f32(16),
            };
            if ![
                canvas.pixels_per_unit,
                canvas.origin_x,
                canvas.origin_y,
                canvas.width,
                canvas.height,
            ]
            .iter()
            .all(|value| value.is_finite())
            {
                return Err(ConvertError::Moc3 {
                    message: "canvas info contains a non-finite value".to_string(),
                });
            }
            canvas
        };
        let parsed = parse_sections(&sections, counts, version, canvas)?;
        verify_layout(&parsed)?;
        Ok(parsed)
    }
}

/// Read every section the converter consumes into raw structures.
fn parse_sections(
    sections: &Sections,
    counts: Counts,
    version: u8,
    canvas: CanvasInfo,
) -> Result<Moc3File, ConvertError> {
    let part_ids = sections.ids(slot::PART_IDS, counts.parts, "part ids")?;
    let part_parents = sections.i32s(slot::PART_PARENT_INDICES, counts.parts, "part parents")?;
    let parts: Vec<RawPart> = part_ids
        .into_iter()
        .zip(part_parents)
        .map(|(id, parent)| RawPart { id, parent })
        .collect();

    let parameter_ids = sections.ids(slot::PARAMETER_IDS, counts.parameters, "parameter ids")?;
    let parameter_max = sections.f32s(
        slot::PARAMETER_MAX_VALUES,
        counts.parameters,
        "parameter max",
    )?;
    let parameter_min = sections.f32s(
        slot::PARAMETER_MIN_VALUES,
        counts.parameters,
        "parameter min",
    )?;
    let parameter_default = sections.f32s(
        slot::PARAMETER_DEFAULT_VALUES,
        counts.parameters,
        "parameter defaults",
    )?;
    let parameter_repeat = sections.bools(
        slot::PARAMETER_REPEAT_TOGGLES,
        counts.parameters,
        "parameter repeats",
    )?;
    let parameters: Vec<RawParameter> = parameter_ids
        .into_iter()
        .enumerate()
        .map(|(index, id)| RawParameter {
            id,
            min: parameter_min[index],
            max: parameter_max[index],
            default: parameter_default[index],
            is_repeat: parameter_repeat[index],
        })
        .collect();

    // Keyform bindings: resolve before objects so the battery can
    // check every object's keyform count against its binding.
    let keys = sections.f32s(slot::KEYS, counts.keys, "keys")?;
    let binding_key_begins = sections.i32s(
        slot::PARAMETER_BINDING_KEY_BEGINS,
        counts.parameter_bindings,
        "binding key begins",
    )?;
    let binding_key_counts = sections.i32s(
        slot::PARAMETER_BINDING_KEY_COUNTS,
        counts.parameter_bindings,
        "binding key counts",
    )?;
    let binding_owner = binding_owners(sections, &counts)?;
    let mut binding_key_ranges: Vec<(usize, usize)> = Vec::with_capacity(counts.parameter_bindings);
    let mut keys_referenced_end = 0usize;
    for index in 0..counts.parameter_bindings {
        let begin = checked_index(binding_key_begins[index], "parameter binding key begin")?;
        let length = checked_index(binding_key_counts[index], "parameter binding key count")?;
        let end = begin
            .checked_add(length)
            .ok_or_else(|| ConvertError::Moc3 {
                message: "parameter binding key range overflows".to_string(),
            })?;
        if end > keys.len() {
            return Err(ConvertError::Moc3 {
                message: "parameter binding key range exceeds the keys array".to_string(),
            });
        }
        let slice = &keys[begin..end];
        if slice.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(ConvertError::Moc3 {
                message: format!("parameter binding {index} keys are not strictly increasing"),
            });
        }
        keys_referenced_end = keys_referenced_end.max(end);
        binding_key_ranges.push((begin, length));
    }
    let binding_index_table = sections.i32s(
        slot::PARAMETER_BINDING_INDEX,
        counts.parameter_binding_index,
        "parameter binding index",
    )?;
    let keyform_binding_begins = sections.i32s(
        slot::KEYFORM_BINDING_BEGIN_INDICES,
        counts.keyform_bindings,
        "keyform binding begins",
    )?;
    let keyform_binding_counts = sections.i32s(
        slot::KEYFORM_BINDING_COUNTS,
        counts.keyform_bindings,
        "keyform binding counts",
    )?;
    let mut bindings: Vec<Vec<BindingAxis>> = Vec::with_capacity(counts.keyform_bindings);
    for binding in 0..counts.keyform_bindings {
        let begin = checked_index(keyform_binding_begins[binding], "keyform binding begin")?;
        let length = checked_index(keyform_binding_counts[binding], "keyform binding count")?;
        let end = begin
            .checked_add(length)
            .ok_or_else(|| ConvertError::Moc3 {
                message: "keyform binding range overflows".to_string(),
            })?;
        if end > binding_index_table.len() {
            return Err(ConvertError::Moc3 {
                message: "keyform binding range exceeds the parameter binding index table"
                    .to_string(),
            });
        }
        let mut axes = Vec::with_capacity(length);
        for entry in &binding_index_table[begin..end] {
            let global_binding = checked_index(*entry, "parameter binding index entry")?;
            if global_binding >= counts.parameter_bindings {
                return Err(ConvertError::Moc3 {
                    message: "parameter binding index entry out of range".to_string(),
                });
            }
            let (keys_begin, keys_length) = binding_key_ranges[global_binding];
            axes.push(BindingAxis {
                parameter: binding_owner[global_binding],
                keys: keys[keys_begin..keys_begin + keys_length].to_vec(),
            });
        }
        bindings.push(axes);
    }

    let deformer_ids = sections.ids(slot::DEFORMER_IDS, counts.deformers, "deformer ids")?;
    let deformer_bindings = sections.i32s(
        slot::DEFORMER_BINDING_INDICES,
        counts.deformers,
        "deformer bindings",
    )?;
    let deformer_parent_parts = sections.i32s(
        slot::DEFORMER_PARENT_PART_INDICES,
        counts.deformers,
        "deformer parent parts",
    )?;
    let deformer_parents = sections.i32s(
        slot::DEFORMER_PARENT_INDICES,
        counts.deformers,
        "deformer parents",
    )?;
    let deformer_types = sections.i32s(slot::DEFORMER_TYPES, counts.deformers, "deformer types")?;
    let deformer_specifics = sections.i32s(
        slot::DEFORMER_SPECIFIC_INDICES,
        counts.deformers,
        "deformer specifics",
    )?;
    // The unified deformer table and the specific tables must
    // agree about bindings; both store it, and the battery
    // cross-checks the two copies (parse_sections keeps the
    // unified value on RawDeformer for exactly that check).
    let mut deformers = Vec::with_capacity(counts.deformers);
    for index in 0..counts.deformers {
        let specific = checked_index(deformer_specifics[index], "deformer specific index")?;
        let kind = match deformer_types[index] {
            0 => {
                if specific >= counts.warp_deformers {
                    return Err(ConvertError::Moc3 {
                        message: "warp deformer specific index out of range".to_string(),
                    });
                }
                DeformerKind::Warp(specific)
            }
            1 => {
                if specific >= counts.rotation_deformers {
                    return Err(ConvertError::Moc3 {
                        message: "rotation deformer specific index out of range".to_string(),
                    });
                }
                DeformerKind::Rotation(specific)
            }
            other => {
                return Err(ConvertError::Moc3 {
                    message: format!("deformer {} has unknown type {other}", deformer_ids[index]),
                });
            }
        };
        deformers.push(RawDeformer {
            id: deformer_ids[index].clone(),
            binding: deformer_bindings[index],
            parent_part: deformer_parent_parts[index],
            parent_deformer: deformer_parents[index],
            kind,
        });
    }

    let warp_bindings = sections.i32s(
        slot::WARP_BINDING_INDICES,
        counts.warp_deformers,
        "warp bindings",
    )?;
    let warp_begins = sections.i32s(
        slot::WARP_BEGIN_INDICES,
        counts.warp_deformers,
        "warp begins",
    )?;
    let warp_counts = sections.i32s(slot::WARP_COUNTS, counts.warp_deformers, "warp counts")?;
    let warp_vertex_counts = sections.i32s(
        slot::WARP_VERTEX_COUNTS,
        counts.warp_deformers,
        "warp vertex counts",
    )?;
    let warp_rows = sections.i32s(slot::WARP_ROWS, counts.warp_deformers, "warp rows")?;
    let warp_columns = sections.i32s(slot::WARP_COLUMNS, counts.warp_deformers, "warp columns")?;
    let warp_keyform_opacities = sections.f32s(
        slot::WARP_KEYFORM_OPACITIES,
        counts.warp_keyforms,
        "warp keyform opacities",
    )?;
    let warp_keyform_position_begins = sections.i32s(
        slot::WARP_KEYFORM_POSITION_BEGINS,
        counts.warp_keyforms,
        "warp keyform position begins",
    )?;
    let mut warps = Vec::with_capacity(counts.warp_deformers);
    for index in 0..counts.warp_deformers {
        let (opacities, begins) = keyform_window(
            warp_begins[index],
            warp_counts[index],
            &warp_keyform_opacities,
            &warp_keyform_position_begins,
            "warp deformer",
        )?;
        warps.push(RawWarp {
            binding: warp_bindings[index],
            begin: warp_begins[index],
            count: warp_counts[index],
            vertex_count: warp_vertex_counts[index],
            rows: warp_rows[index],
            columns: warp_columns[index],
            keyform_opacities: opacities,
            keyform_position_begins: begins,
        });
    }

    let rotation_bindings = sections.i32s(
        slot::ROTATION_BINDING_INDICES,
        counts.rotation_deformers,
        "rotation bindings",
    )?;
    let rotation_begins = sections.i32s(
        slot::ROTATION_BEGIN_INDICES,
        counts.rotation_deformers,
        "rotation begins",
    )?;
    let rotation_counts = sections.i32s(
        slot::ROTATION_COUNTS,
        counts.rotation_deformers,
        "rotation counts",
    )?;
    let rotation_base_angles = sections.f32s(
        slot::ROTATION_BASE_ANGLES,
        counts.rotation_deformers,
        "rotation base angles",
    )?;
    let rotation_opacities = sections.f32s(
        slot::ROTATION_KEYFORM_OPACITIES,
        counts.rotation_keyforms,
        "rotation keyform opacities",
    )?;
    let rotation_angles = sections.f32s(
        slot::ROTATION_KEYFORM_ANGLES,
        counts.rotation_keyforms,
        "rotation keyform angles",
    )?;
    let rotation_origin_x = sections.f32s(
        slot::ROTATION_KEYFORM_ORIGIN_X,
        counts.rotation_keyforms,
        "rotation keyform origin x",
    )?;
    let rotation_origin_y = sections.f32s(
        slot::ROTATION_KEYFORM_ORIGIN_Y,
        counts.rotation_keyforms,
        "rotation keyform origin y",
    )?;
    let rotation_scales = sections.f32s(
        slot::ROTATION_KEYFORM_SCALES,
        counts.rotation_keyforms,
        "rotation keyform scales",
    )?;
    let rotation_reflect_x = sections.bools(
        slot::ROTATION_KEYFORM_REFLECT_X,
        counts.rotation_keyforms,
        "rotation keyform reflect x",
    )?;
    let rotation_reflect_y = sections.bools(
        slot::ROTATION_KEYFORM_REFLECT_Y,
        counts.rotation_keyforms,
        "rotation keyform reflect y",
    )?;
    let mut rotations = Vec::with_capacity(counts.rotation_deformers);
    for index in 0..counts.rotation_deformers {
        let begin = checked_index(rotation_begins[index], "rotation keyform begin")?;
        let length = checked_index(rotation_counts[index], "rotation keyform count")?;
        let end = begin
            .checked_add(length)
            .ok_or_else(|| ConvertError::Moc3 {
                message: "rotation keyform range overflows".to_string(),
            })?;
        if end > counts.rotation_keyforms {
            return Err(ConvertError::Moc3 {
                message: "rotation keyform range exceeds the keyform table".to_string(),
            });
        }
        let mut keyforms = Vec::with_capacity(length);
        for at in begin..end {
            keyforms.push(RawRotationKeyform {
                opacity: rotation_opacities[at],
                angle: rotation_angles[at],
                origin: [rotation_origin_x[at], rotation_origin_y[at]],
                scale: rotation_scales[at],
                reflect_x: rotation_reflect_x[at],
                reflect_y: rotation_reflect_y[at],
            });
        }
        rotations.push(RawRotation {
            binding: rotation_bindings[index],
            begin: rotation_begins[index],
            count: rotation_counts[index],
            base_angle: rotation_base_angles[index],
            keyforms,
        });
    }

    let meshes = parse_meshes(sections, &counts)?;
    let keyform_positions = sections.f32s(
        slot::KEYFORM_POSITIONS,
        counts.keyform_positions,
        "keyform positions",
    )?;
    let uvs = sections.f32s(slot::UVS, counts.uvs, "uvs")?;
    let position_indices = sections.i16s(
        slot::POSITION_INDICES,
        counts.position_indices,
        "position indices",
    )?;
    let drawable_masks = sections.i32s(
        slot::DRAWABLE_MASKS,
        counts.drawable_masks,
        "drawable masks",
    )?;
    let draw_order_groups = parse_draw_order_groups(sections, &counts)?;

    Ok(Moc3File {
        version,
        canvas,
        counts,
        parts,
        parameters,
        deformers,
        warps,
        rotations,
        meshes,
        bindings,
        keyform_positions,
        uvs,
        position_indices,
        drawable_masks,
        draw_order_groups,
        keys_referenced_end,
    })
}

/// Per-parameter ownership of the global parameter bindings:
/// parameter `p` owns bindings `[begin[p], begin[p] + count[p])`
/// and those ranges must partition the binding table exactly.
fn binding_owners(sections: &Sections, counts: &Counts) -> Result<Vec<usize>, ConvertError> {
    let begins = sections.i32s(
        slot::PARAMETER_BINDING_BEGIN_INDICES,
        counts.parameters,
        "parameter binding begins",
    )?;
    let lengths = sections.i32s(
        slot::PARAMETER_BINDING_COUNTS,
        counts.parameters,
        "parameter binding counts",
    )?;
    let mut owners = vec![usize::MAX; counts.parameter_bindings];
    for parameter in 0..counts.parameters {
        let begin = checked_index(begins[parameter], "parameter binding begin")?;
        let length = checked_index(lengths[parameter], "parameter binding count")?;
        let end = begin
            .checked_add(length)
            .ok_or_else(|| ConvertError::Moc3 {
                message: "parameter binding range overflows".to_string(),
            })?;
        if end > counts.parameter_bindings {
            return Err(ConvertError::Moc3 {
                message: "parameter binding range exceeds the binding table".to_string(),
            });
        }
        for slot in &mut owners[begin..end] {
            if *slot != usize::MAX {
                return Err(ConvertError::Moc3 {
                    message: "parameter binding ownership ranges overlap".to_string(),
                });
            }
            *slot = parameter;
        }
    }
    if owners.contains(&usize::MAX) {
        return Err(ConvertError::Moc3 {
            message: "some parameter bindings have no owning parameter".to_string(),
        });
    }
    Ok(owners)
}

/// Slice the per-keyform scalar windows for one object out of the
/// global keyform tables.
fn keyform_window(
    begin_raw: i32,
    count_raw: i32,
    opacities: &[f32],
    position_begins: &[i32],
    what: &str,
) -> Result<(Vec<f32>, Vec<i32>), ConvertError> {
    let begin = checked_index(begin_raw, "keyform begin")?;
    let length = checked_index(count_raw, "keyform count")?;
    let end = begin
        .checked_add(length)
        .ok_or_else(|| ConvertError::Moc3 {
            message: format!("{what}: keyform range overflows"),
        })?;
    if end > opacities.len() || end > position_begins.len() {
        return Err(ConvertError::Moc3 {
            message: format!("{what}: keyform range exceeds the keyform tables"),
        });
    }
    Ok((
        opacities[begin..end].to_vec(),
        position_begins[begin..end].to_vec(),
    ))
}

/// Read the art mesh table.
fn parse_meshes(sections: &Sections, counts: &Counts) -> Result<Vec<RawMesh>, ConvertError> {
    let ids = sections.ids(slot::MESH_IDS, counts.art_meshes, "art mesh ids")?;
    let bindings = sections.i32s(
        slot::MESH_BINDING_INDICES,
        counts.art_meshes,
        "art mesh bindings",
    )?;
    let begins = sections.i32s(
        slot::MESH_BEGIN_INDICES,
        counts.art_meshes,
        "art mesh begins",
    )?;
    let keyform_counts = sections.i32s(slot::MESH_COUNTS, counts.art_meshes, "art mesh counts")?;
    let visible = sections.bools(slot::MESH_VISIBLE, counts.art_meshes, "art mesh visibility")?;
    let enabled = sections.bools(slot::MESH_ENABLED, counts.art_meshes, "art mesh enabled")?;
    let parent_parts = sections.i32s(
        slot::MESH_PARENT_PART_INDICES,
        counts.art_meshes,
        "art mesh parent parts",
    )?;
    let parent_deformers = sections.i32s(
        slot::MESH_PARENT_DEFORMER_INDICES,
        counts.art_meshes,
        "art mesh parent deformers",
    )?;
    let texture_numbers = sections.i32s(
        slot::MESH_TEXTURE_NUMBERS,
        counts.art_meshes,
        "art mesh texture numbers",
    )?;
    let drawable_flags = sections.u8s(
        slot::MESH_DRAWABLE_FLAGS,
        counts.art_meshes,
        "art mesh drawable flags",
    )?;
    let vertex_counts = sections.i32s(
        slot::MESH_VERTEX_COUNTS,
        counts.art_meshes,
        "art mesh vertex counts",
    )?;
    let uv_begins = sections.i32s(
        slot::MESH_UV_BEGIN_INDICES,
        counts.art_meshes,
        "art mesh uv begins",
    )?;
    let position_index_begins = sections.i32s(
        slot::MESH_POS_INDEX_BEGIN_INDICES,
        counts.art_meshes,
        "art mesh position index begins",
    )?;
    let position_index_counts = sections.i32s(
        slot::MESH_POS_INDEX_COUNTS,
        counts.art_meshes,
        "art mesh position index counts",
    )?;
    let mask_begins = sections.i32s(
        slot::MESH_MASK_BEGIN_INDICES,
        counts.art_meshes,
        "art mesh mask begins",
    )?;
    let mask_counts = sections.i32s(
        slot::MESH_MASK_COUNTS,
        counts.art_meshes,
        "art mesh mask counts",
    )?;
    let keyform_opacities = sections.f32s(
        slot::MESH_KEYFORM_OPACITIES,
        counts.art_mesh_keyforms,
        "art mesh keyform opacities",
    )?;
    let keyform_draw_orders = sections.f32s(
        slot::MESH_KEYFORM_DRAW_ORDERS,
        counts.art_mesh_keyforms,
        "art mesh keyform draw orders",
    )?;
    let keyform_position_begins = sections.i32s(
        slot::MESH_KEYFORM_POSITION_BEGINS,
        counts.art_mesh_keyforms,
        "art mesh keyform position begins",
    )?;
    let mut meshes = Vec::with_capacity(counts.art_meshes);
    for index in 0..counts.art_meshes {
        let (opacities, position_begins) = keyform_window(
            begins[index],
            keyform_counts[index],
            &keyform_opacities,
            &keyform_position_begins,
            "art mesh",
        )?;
        let begin = checked_index(begins[index], "art mesh keyform begin")?;
        let length = checked_index(keyform_counts[index], "art mesh keyform count")?;
        meshes.push(RawMesh {
            id: ids[index].clone(),
            binding: bindings[index],
            begin: begins[index],
            count: keyform_counts[index],
            is_visible: visible[index],
            is_enabled: enabled[index],
            parent_part: parent_parts[index],
            parent_deformer: parent_deformers[index],
            texture_number: texture_numbers[index],
            drawable_flag: drawable_flags[index],
            vertex_count: vertex_counts[index],
            uv_begin: uv_begins[index],
            position_index_begin: position_index_begins[index],
            position_index_count: position_index_counts[index],
            mask_begin: mask_begins[index],
            mask_count: mask_counts[index],
            keyform_opacities: opacities,
            keyform_draw_orders: keyform_draw_orders[begin..begin + length].to_vec(),
            keyform_position_begins: position_begins,
        });
    }
    Ok(meshes)
}

/// Read the draw order groups and their object lists.
fn parse_draw_order_groups(
    sections: &Sections,
    counts: &Counts,
) -> Result<Vec<RawDrawOrderGroup>, ConvertError> {
    let begins = sections.i32s(
        slot::DRAW_ORDER_GROUP_OBJECT_BEGINS,
        counts.draw_order_groups,
        "draw order group begins",
    )?;
    let lengths = sections.i32s(
        slot::DRAW_ORDER_GROUP_OBJECT_COUNTS,
        counts.draw_order_groups,
        "draw order group counts",
    )?;
    let maximums = sections.i32s(
        slot::DRAW_ORDER_GROUP_MAXIMUMS,
        counts.draw_order_groups,
        "draw order maximums",
    )?;
    let minimums = sections.i32s(
        slot::DRAW_ORDER_GROUP_MINIMUMS,
        counts.draw_order_groups,
        "draw order minimums",
    )?;
    let types = sections.i32s(
        slot::DRAW_ORDER_OBJECT_TYPES,
        counts.draw_order_group_objects,
        "draw order object types",
    )?;
    let indices = sections.i32s(
        slot::DRAW_ORDER_OBJECT_INDICES,
        counts.draw_order_group_objects,
        "draw order object indices",
    )?;
    let mut groups = Vec::with_capacity(counts.draw_order_groups);
    for group in 0..counts.draw_order_groups {
        let begin = checked_index(begins[group], "draw order group begin")?;
        let length = checked_index(lengths[group], "draw order group count")?;
        let end = begin
            .checked_add(length)
            .ok_or_else(|| ConvertError::Moc3 {
                message: "draw order group range overflows".to_string(),
            })?;
        if end > counts.draw_order_group_objects {
            return Err(ConvertError::Moc3 {
                message: "draw order group range exceeds the object table".to_string(),
            });
        }
        let mut objects = Vec::with_capacity(length);
        for at in begin..end {
            objects.push((types[at], indices[at]));
        }
        groups.push(RawDrawOrderGroup {
            minimum: minimums[group],
            maximum: maximums[group],
            objects,
        });
    }
    Ok(groups)
}

/// Convert a stored `i32` index/count to `usize`, rejecting
/// negatives loudly.
pub(crate) fn checked_index(value: i32, what: &str) -> Result<usize, ConvertError> {
    if value < 0 {
        return Err(ConvertError::Moc3 {
            message: format!("{what} is negative ({value})"),
        });
    }
    Ok(value as usize)
}

/// The number of keyforms an object with this binding must have:
/// the product of its axes' key counts (1 for no axes). Checked
/// against a hard cap so a corrupt binding cannot overflow.
pub(crate) fn binding_form_count(axes: &[BindingAxis]) -> Result<usize, ConvertError> {
    let mut total: usize = 1;
    for axis in axes {
        total = total
            .checked_mul(axis.keys.len())
            .ok_or_else(|| ConvertError::Moc3 {
                message: "keyform binding product overflows".to_string(),
            })?;
        if total > 1_000_000 {
            return Err(ConvertError::Moc3 {
                message: "keyform binding product exceeds the sanity cap".to_string(),
            });
        }
    }
    Ok(total)
}

/// Look up the axes for a stored binding index (-1 or an empty
/// binding means no axes: a single rest form).
pub(crate) fn axes_for(
    bindings: &[Vec<BindingAxis>],
    binding: i32,
) -> Result<&[BindingAxis], ConvertError> {
    if binding < 0 {
        return Ok(&[]);
    }
    let index = binding as usize;
    bindings
        .get(index)
        .map(|axes| axes.as_slice())
        .ok_or_else(|| ConvertError::Moc3 {
            message: format!("keyform binding index {binding} out of range"),
        })
}

/// The layout self-verification battery. Every identity here was
/// verified against real sample files across four format
/// versions; a file that fails one was not written in the layout
/// this parser implements, and conversion refuses it.
fn verify_layout(file: &Moc3File) -> Result<(), ConvertError> {
    let counts = &file.counts;
    let layout = |message: String| ConvertError::LayoutCheck { message };

    // UV and index totals must reconcile with the mesh table.
    let uv_total: usize = file
        .meshes
        .iter()
        .map(|mesh| (mesh.vertex_count.max(0) as usize) * 2)
        .sum();
    if uv_total != counts.uvs {
        return Err(layout(format!(
            "uv total {uv_total} != count info table {0}",
            counts.uvs
        )));
    }
    let index_total: usize = file
        .meshes
        .iter()
        .map(|mesh| mesh.position_index_count.max(0) as usize)
        .sum();
    if index_total != counts.position_indices {
        return Err(layout(format!(
            "position index total {index_total} != count info table {}",
            counts.position_indices
        )));
    }

    // Reference ranges.
    for (index, part) in file.parts.iter().enumerate() {
        check_parent(part.parent, counts.parts, &format!("part {index} parent"))?;
    }
    for (index, deformer) in file.deformers.iter().enumerate() {
        check_parent(
            deformer.parent_deformer,
            counts.deformers,
            &format!("deformer {index} parent"),
        )?;
        check_parent(
            deformer.parent_part,
            counts.parts,
            &format!("deformer {index} parent part"),
        )?;
    }
    for mesh in &file.meshes {
        if mesh.parent_part < 0 || mesh.parent_part as usize >= counts.parts {
            return Err(layout(format!(
                "art mesh {:?} parent part out of range",
                mesh.id
            )));
        }
        check_parent(
            mesh.parent_deformer,
            counts.deformers,
            "art mesh parent deformer",
        )?;
        if mesh.texture_number < 0 {
            return Err(layout(format!(
                "art mesh {:?} texture number negative",
                mesh.id
            )));
        }
        let mask_begin = checked_index(mesh.mask_begin, "mask begin")?;
        let mask_count = checked_index(mesh.mask_count, "mask count")?;
        let mask_end = mask_begin
            .checked_add(mask_count)
            .ok_or_else(|| layout("drawable mask range overflows".to_string()))?;
        if mask_end > file.drawable_masks.len() {
            return Err(layout(format!(
                "art mesh {:?} mask range out of bounds",
                mesh.id
            )));
        }
        for mask in &file.drawable_masks[mask_begin..mask_end] {
            if *mask < 0 || *mask as usize >= counts.art_meshes {
                return Err(layout(format!(
                    "art mesh {:?} mask index out of range",
                    mesh.id
                )));
            }
        }
        let uv_begin = checked_index(mesh.uv_begin, "uv begin")?;
        let uv_end = uv_begin
            .checked_add((mesh.vertex_count.max(0) as usize) * 2)
            .ok_or_else(|| layout("uv range overflows".to_string()))?;
        if uv_end > file.uvs.len() {
            return Err(layout(format!(
                "art mesh {:?} uv range out of bounds",
                mesh.id
            )));
        }
        let index_begin = checked_index(mesh.position_index_begin, "position index begin")?;
        let index_count = checked_index(mesh.position_index_count, "position index count")?;
        let index_end = index_begin
            .checked_add(index_count)
            .ok_or_else(|| layout("position index range overflows".to_string()))?;
        if index_end > file.position_indices.len() {
            return Err(layout(format!(
                "art mesh {:?} position index range out of bounds",
                mesh.id
            )));
        }
    }

    // The unified deformer table's binding index must agree with
    // the specific table's for every deformer.
    for deformer in &file.deformers {
        let specific_binding = match deformer.kind {
            DeformerKind::Warp(index) => file.warps[index].binding,
            DeformerKind::Rotation(index) => file.rotations[index].binding,
        };
        if specific_binding != deformer.binding {
            return Err(layout(format!(
                "deformer {:?} binding {} in the unified table != {} in its specific table",
                deformer.id, deformer.binding, specific_binding
            )));
        }
    }

    // Keyform counts must equal their bindings' products.
    for warp in &file.warps {
        let axes = axes_for(&file.bindings, warp.binding)?;
        let expected = binding_form_count(axes)?;
        if warp.count < 0 || warp.count as usize != expected {
            return Err(layout(format!(
                "warp deformer keyform count {} != binding product {expected}",
                warp.count
            )));
        }
        check_keyform_positions(
            file,
            warp.vertex_count,
            &warp.keyform_position_begins,
            "warp deformer",
        )?;
        let grid = (warp.rows as i64 + 1) * (warp.columns as i64 + 1);
        if warp.rows < 0 || warp.columns < 0 || grid != warp.vertex_count as i64 {
            return Err(layout(format!(
                "warp deformer grid {}x{} does not match vertex count {}",
                warp.rows, warp.columns, warp.vertex_count
            )));
        }
    }
    for rotation in &file.rotations {
        let axes = axes_for(&file.bindings, rotation.binding)?;
        let expected = binding_form_count(axes)?;
        if rotation.count < 0 || rotation.count as usize != expected {
            return Err(layout(format!(
                "rotation deformer keyform count {} != binding product {expected}",
                rotation.count
            )));
        }
    }
    for mesh in &file.meshes {
        let axes = axes_for(&file.bindings, mesh.binding)?;
        let expected = binding_form_count(axes)?;
        if mesh.count < 0 || mesh.count as usize != expected {
            return Err(layout(format!(
                "art mesh {:?} keyform count {} != binding product {expected}",
                mesh.id, mesh.count
            )));
        }
        if mesh.count == 0 {
            return Err(layout(format!(
                "art mesh {:?} has no keyforms; its geometry is unrecoverable",
                mesh.id
            )));
        }
        check_keyform_positions(
            file,
            mesh.vertex_count,
            &mesh.keyform_position_begins,
            "art mesh",
        )?;
    }
    Ok(())
}

/// A parent reference must be -1 (none) or a valid index.
fn check_parent(value: i32, count: usize, what: &str) -> Result<(), ConvertError> {
    if value < -1 || (value >= 0 && value as usize >= count) {
        return Err(ConvertError::LayoutCheck {
            message: format!("{what} index {value} out of range"),
        });
    }
    Ok(())
}

/// Every keyform's position window must fit the positions array.
fn check_keyform_positions(
    file: &Moc3File,
    vertex_count: i32,
    begins: &[i32],
    what: &str,
) -> Result<(), ConvertError> {
    if vertex_count < 0 {
        return Err(ConvertError::LayoutCheck {
            message: format!("{what}: negative vertex count"),
        });
    }
    let span = (vertex_count as usize) * 2;
    for begin_raw in begins {
        let begin = checked_index(*begin_raw, "keyform position begin")?;
        let end = begin
            .checked_add(span)
            .ok_or_else(|| ConvertError::LayoutCheck {
                message: format!("{what}: keyform position range overflows"),
            })?;
        if end > file.keyform_positions.len() {
            return Err(ConvertError::LayoutCheck {
                message: format!("{what}: keyform position range out of bounds"),
            });
        }
    }
    Ok(())
}
