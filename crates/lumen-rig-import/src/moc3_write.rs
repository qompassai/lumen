//! From-scratch `.moc3` binary writer — the outbound mirror of
//! [`crate::moc3`] (exporter phase E1, lumen R5c).
//!
//! The writer emits the V3_0 section layout: a 64-byte header,
//! a 160-entry section offset table of absolute `i32` offsets at
//! 0x40, the 32-field count info table, then the section arrays
//! packed contiguously in slot order with zero padding only.
//!
//! The layout is anchored to three independent sources that
//! agree slot for slot (the full derived table is recorded in
//! the E1 report):
//! - the extracted byte-level spec (AYAGAMI-REVIEW section 2:
//!   the Ayagami parser's sequential pass order over its class
//!   field lists — 101 sections at V3_0, sections 0/1 being the
//!   count table and canvas),
//! - the R5a parser's verified slot map ([`crate::moc3`]), which
//!   names every slot it reads,
//! - two official sample files read byte by byte (Haru, version
//!   byte 1; Ren, version byte 6), which fix the values of the
//!   sections neither reader consumes: object header pairs are
//!   (0, 0), parameter snap types are 3, the mesh unknown flag
//!   and both deformer flags are 1, part visibility flags are 1,
//!   draw-group totals are the group's art-mesh count, and a
//!   draw item's group reference is -1 for art meshes.
//!
//! Sections the writer does not emit (the V3_0 glue sections,
//! slots 89-100) get offset 0 and count 0: the native format has
//! no glue construct (MAPPING.md section 8).
//!
//! Version parameterization: [`Moc3Version`] names every
//! revision the layout tables know about, and the section
//! builders are per-version functions; only V3_0 is implemented
//! in this slice and any other version fails with a typed
//! [`ConvertError::Export`] instead of emitting an unverified
//! layout. Later versions only add sections and counts
//! (AYAGAMI-REVIEW section 2.2), so the V5 follow-on extends
//! these tables rather than rewriting them.
//!
//! Determinism: the output is a pure function of the input
//! data — parallel arrays in caller order, no timestamps, no
//! hash-map iteration anywhere in the emission path.

use crate::error::ConvertError;

/// Header size in bytes.
const HEADER_BYTES: usize = 64;
/// Section offset table size in entries (the table is 160 wide
/// at every version; readers consume the prefix their version
/// defines — 101 sections at V3_0).
const SOT_ENTRIES: usize = 160;
/// Byte offset of the section offset table.
const SOT_OFFSET: usize = 0x40;
/// Size of one id block in bytes (`MOC3Id`).
const ID_BLOCK_BYTES: usize = 64;
/// Count info table size in `i32` fields.
const CIT_FIELDS: usize = 32;
/// Draw depth range maximum; native draw orders rescale into
/// [0, DRAW_DEPTH_MAX] (MAPPING.md section 7).
pub const DRAW_DEPTH_MAX: f32 = 1000.0;

/// Section offset table slots, V3_0 (101 sections). Names follow
/// the Ayagami field lists; the R5a parser's slot names are noted
/// where they differ.
mod slot {
    pub const COUNT_INFO_TABLE: usize = 0;
    pub const CANVAS_INFO: usize = 1;
    pub const PART_HEADERS: usize = 2;
    pub const PART_IDS: usize = 3;
    pub const PART_BINDING_SETS: usize = 4;
    pub const PART_FORM_BEGINS: usize = 5;
    pub const PART_FORM_COUNTS: usize = 6;
    pub const PART_VISIBLE_ART_MESHES: usize = 7;
    pub const PART_VISIBLE_DEFORMERS: usize = 8;
    pub const PART_PARENT_INDICES: usize = 9;
    pub const DEFORMER_HEADERS: usize = 10;
    pub const DEFORMER_IDS: usize = 11;
    pub const DEFORMER_BINDING_SETS: usize = 12;
    pub const DEFORMER_UNKNOWN_FLAGS: usize = 13;
    pub const DEFORMER_VISIBLE: usize = 14;
    pub const DEFORMER_PARENT_PART_INDICES: usize = 15;
    pub const DEFORMER_PARENT_INDICES: usize = 16;
    pub const DEFORMER_TYPES: usize = 17;
    pub const DEFORMER_SPECIFIC_INDICES: usize = 18;
    pub const WARP_BINDING_SETS: usize = 19;
    pub const WARP_FORM_BEGINS: usize = 20;
    pub const WARP_FORM_COUNTS: usize = 21;
    pub const WARP_VERTEX_COUNTS: usize = 22;
    pub const WARP_ROWS: usize = 23;
    pub const WARP_COLUMNS: usize = 24;
    pub const ROTATION_BINDING_SETS: usize = 25;
    pub const ROTATION_FORM_BEGINS: usize = 26;
    pub const ROTATION_FORM_COUNTS: usize = 27;
    pub const ROTATION_BASE_ANGLES: usize = 28;
    pub const MESH_HEADERS: usize = 29;
    pub const MESH_UNKNOWN_A: usize = 30;
    pub const MESH_UNKNOWN_B: usize = 31;
    pub const MESH_UNKNOWN_C: usize = 32;
    pub const MESH_IDS: usize = 33;
    pub const MESH_BINDING_SETS: usize = 34;
    pub const MESH_FORM_BEGINS: usize = 35;
    pub const MESH_FORM_COUNTS: usize = 36;
    pub const MESH_UNKNOWN_FLAGS: usize = 37;
    pub const MESH_VISIBLE: usize = 38;
    pub const MESH_PARENT_PART_INDICES: usize = 39;
    pub const MESH_PARENT_DEFORMER_INDICES: usize = 40;
    pub const MESH_TEXTURE_NUMBERS: usize = 41;
    pub const MESH_RENDER_CONFIGS: usize = 42;
    pub const MESH_VERTEX_COUNTS: usize = 43;
    pub const MESH_UV_BEGINS: usize = 44;
    pub const MESH_INDEX_BEGINS: usize = 45;
    pub const MESH_INDEX_COUNTS: usize = 46;
    pub const MESH_MASK_BEGINS: usize = 47;
    pub const MESH_MASK_COUNTS: usize = 48;
    pub const PARAMETER_HEADERS: usize = 49;
    pub const PARAMETER_IDS: usize = 50;
    pub const PARAMETER_MAX_VALUES: usize = 51;
    pub const PARAMETER_MIN_VALUES: usize = 52;
    pub const PARAMETER_DEFAULT_VALUES: usize = 53;
    pub const PARAMETER_REPEAT_TOGGLES: usize = 54;
    pub const PARAMETER_SNAP_TYPES: usize = 55;
    pub const PARAMETER_MAP_BEGINS: usize = 56;
    pub const PARAMETER_MAP_COUNTS: usize = 57;
    pub const PART_FORM_DEPTHS: usize = 58;
    pub const WARP_FORM_OPACITIES: usize = 59;
    pub const WARP_FORM_POSITION_BEGINS: usize = 60;
    pub const ROTATION_FORM_OPACITIES: usize = 61;
    pub const ROTATION_FORM_ANGLES: usize = 62;
    pub const ROTATION_FORM_ORIGIN_X: usize = 63;
    pub const ROTATION_FORM_ORIGIN_Y: usize = 64;
    pub const ROTATION_FORM_SCALES: usize = 65;
    pub const ROTATION_FORM_REFLECT_X: usize = 66;
    pub const ROTATION_FORM_REFLECT_Y: usize = 67;
    pub const MESH_FORM_OPACITIES: usize = 68;
    pub const MESH_FORM_DEPTHS: usize = 69;
    pub const MESH_FORM_POSITION_BEGINS: usize = 70;
    pub const KEYFORM_POSITIONS: usize = 71;
    pub const PARAMETER_BINDING_INDEX: usize = 72;
    pub const KEYFORM_BINDING_BEGINS: usize = 73;
    pub const KEYFORM_BINDING_COUNTS: usize = 74;
    pub const PARAMETER_BINDING_KEY_BEGINS: usize = 75;
    pub const PARAMETER_BINDING_KEY_COUNTS: usize = 76;
    pub const KEYS: usize = 77;
    pub const UVS: usize = 78;
    pub const POSITION_INDICES: usize = 79;
    pub const DRAWABLE_MASKS: usize = 80;
    pub const DRAW_ORDER_GROUP_ITEM_BEGINS: usize = 81;
    pub const DRAW_ORDER_GROUP_ITEM_COUNTS: usize = 82;
    pub const DRAW_ORDER_GROUP_TOTALS: usize = 83;
    pub const DRAW_ORDER_GROUP_MAXIMUMS: usize = 84;
    pub const DRAW_ORDER_GROUP_MINIMUMS: usize = 85;
    pub const DRAW_ORDER_OBJECT_TYPES: usize = 86;
    pub const DRAW_ORDER_OBJECT_INDICES: usize = 87;
    pub const DRAW_ORDER_OBJECT_GROUP_REFS: usize = 88;
}

/// moc3 format revisions. The writer's layout tables are
/// per-version; only [`Moc3Version::V3_0`] is implemented.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Moc3Version {
    /// SDK 4-era moc3 (version word 1). The E1 target.
    V3_0,
    /// Version word 4. Layout known, writer not implemented.
    V4_2,
    /// Version word 5. Layout known, writer not implemented.
    V5_0,
    /// Version word 6. Layout known, writer not implemented.
    V5_3,
}

impl Moc3Version {
    /// The label used in reports and error messages.
    pub fn label(&self) -> &'static str {
        match self {
            Moc3Version::V3_0 => "V3_0",
            Moc3Version::V4_2 => "V4_2",
            Moc3Version::V5_0 => "V5_0",
            Moc3Version::V5_3 => "V5_3",
        }
    }

    /// The number of section slots this version defines
    /// (V3_0: 101, slots 0-100). Slots past this in the
    /// 160-entry table are padding and stay 0.
    fn section_count(&self) -> usize {
        match self {
            Moc3Version::V3_0 => 101,
            Moc3Version::V4_2 => 101,
            Moc3Version::V5_0 => 101,
            Moc3Version::V5_3 => 101,
        }
    }

    /// The u32 version word stored at header offset 4.
    fn version_word(&self) -> Result<u32, ConvertError> {
        match self {
            Moc3Version::V3_0 => Ok(1),
            other => Err(export_error(format!(
                "moc3 version {} is not implemented by the writer (V3_0 only in this slice)",
                other.label()
            ))),
        }
    }
}

/// The moc3 canvas block: an exporter input, never derived
/// (MAPPING.md section 2, question Q1 resolved as option (a) —
/// canvas is a `CanvasSpec` argument, mirroring the editor
/// side, where canvas size and pixels-per-unit are export
/// settings).
#[derive(Debug, Clone, Copy, Default)]
pub struct CanvasSpec {
    pub pixels_per_unit: f32,
    pub origin_x: f32,
    pub origin_y: f32,
    pub width: f32,
    pub height: f32,
}

/// One parameter's writer record. `map_begin`/`map_count` slice
/// the parameter-map pool; ownership ranges over that pool must
/// partition it per parameter (the reader's battery enforces
/// this, the writer builds it so).
#[derive(Debug, Clone)]
pub struct ParamData {
    pub id: String,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub repeat: bool,
    /// 0 = IntegerFloor, 1 = IntegerSnap, 3 = Normal; 2 does not
    /// exist in the format.
    pub snap_type: u32,
    pub map_begin: usize,
    pub map_count: usize,
}

/// One parameter map: an axis key list, as a window into the
/// global keys pool.
#[derive(Debug, Clone, Copy)]
pub struct ParamMapData {
    pub key_begin: usize,
    pub key_count: usize,
}

/// One keyform binding set: a window into the binding-reference
/// pool (one reference per axis, in axis order).
#[derive(Debug, Clone, Copy)]
pub struct BindingSetData {
    pub ref_begin: usize,
    pub ref_count: usize,
}

/// One part. Parts carry exactly one keyform in this writer
/// (MAPPING.md section 4: native parts are unbound), so the
/// part's form index is its own index into the depth pool.
#[derive(Debug, Clone)]
pub struct PartData {
    pub id: String,
    /// Parent part index, or -1.
    pub parent: i32,
    /// Index into the binding-set pool (the shared empty set
    /// for unbound parts, as in the sample files).
    pub binding_set: usize,
    pub visible_art_meshes: bool,
    pub visible_deformers: bool,
}

/// One deformer (unified table entry).
#[derive(Debug, Clone)]
pub struct DeformerData {
    pub id: String,
    pub binding_set: usize,
    /// The format's unnamed per-deformer flag; 1 in the sample
    /// files. Carried verbatim from the caller's data.
    pub unknown_flag: bool,
    pub visible: bool,
    /// Parent part index, or -1 (MAPPING.md section 5: always
    /// -1 from native — deformers form their own forest).
    pub parent_part: i32,
    /// Parent deformer index, or -1.
    pub parent: i32,
    /// 0 = warp, 1 = rotation.
    pub kind: u32,
    /// Index into the kind's own table.
    pub typed_index: usize,
}

/// One warp deformer (specific table entry).
#[derive(Debug, Clone, Copy)]
pub struct WarpData {
    pub binding_set: usize,
    pub form_begin: usize,
    pub form_count: usize,
    pub vertex_count: usize,
    pub rows: usize,
    pub columns: usize,
}

/// One rotation deformer (specific table entry).
#[derive(Debug, Clone, Copy)]
pub struct RotationData {
    pub binding_set: usize,
    pub form_begin: usize,
    pub form_count: usize,
    pub base_angle: f32,
}

/// One rotation keyform.
#[derive(Debug, Clone, Copy)]
pub struct RotationFormData {
    pub opacity: f32,
    pub angle: f32,
    pub origin: [f32; 2],
    pub scale: f32,
    pub reflect_x: bool,
    pub reflect_y: bool,
}

/// One art mesh.
#[derive(Debug, Clone)]
pub struct MeshData {
    pub id: String,
    pub binding_set: usize,
    pub form_begin: usize,
    pub form_count: usize,
    /// The format's unnamed per-mesh flag (the R5a parser's
    /// "enabled" slot); 1 in the sample files.
    pub unknown_flag: bool,
    pub visible: bool,
    pub parent_part: i32,
    /// Parent deformer index, or -1.
    pub parent_deformer: i32,
    pub texture_number: usize,
    /// The render-config byte: blend bits 0-1, double-sided
    /// bit 0x4; the high nibble must be 0.
    pub render_config: u8,
    pub vertex_count: usize,
    /// Begin index into the global UV pool, in f32 units
    /// (2 per vertex).
    pub uv_begin: usize,
    pub index_begin: usize,
    pub index_count: usize,
    pub mask_begin: usize,
    pub mask_count: usize,
}

/// One draw order group.
#[derive(Debug, Clone, Copy, Default)]
pub struct DrawGroupData {
    pub item_begin: usize,
    pub item_count: usize,
    /// Total art meshes reachable through the group.
    pub total_art_meshes: usize,
    pub max_depth: f32,
    pub min_depth: f32,
}

/// One draw order group item.
#[derive(Debug, Clone, Copy)]
pub struct DrawItemData {
    /// 0 = art mesh, 1 = part.
    pub item_type: u32,
    pub child: usize,
    /// The item's child draw group, or -1 (always -1 for art
    /// meshes; sample-verified).
    pub draw_group: i32,
}

/// Everything the writer emits, as parallel arrays in file
/// order. The mapping layer ([`crate::convert::outbound`])
/// builds this from a native model; the writer owns layout,
/// counts, and structural validation only.
#[derive(Debug, Clone)]
pub struct Moc3Data {
    pub version: Moc3Version,
    pub canvas: CanvasSpec,
    pub parts: Vec<PartData>,
    /// One depth per part (part keyforms), in part order.
    pub part_form_depths: Vec<f32>,
    pub deformers: Vec<DeformerData>,
    pub warps: Vec<WarpData>,
    pub warp_form_opacities: Vec<f32>,
    pub warp_form_position_begins: Vec<usize>,
    pub rotations: Vec<RotationData>,
    pub rotation_forms: Vec<RotationFormData>,
    pub meshes: Vec<MeshData>,
    pub mesh_form_opacities: Vec<f32>,
    pub mesh_form_depths: Vec<f32>,
    pub mesh_form_position_begins: Vec<usize>,
    /// The shared keyform position pool (f32 x/y pairs for warp
    /// grids and mesh vertices).
    pub keyform_positions: Vec<f32>,
    pub parameters: Vec<ParamData>,
    pub parameter_maps: Vec<ParamMapData>,
    /// The binding-reference pool: one parameter-map index per
    /// entry; binding sets slice it.
    pub binding_refs: Vec<usize>,
    pub binding_sets: Vec<BindingSetData>,
    pub keys: Vec<f32>,
    pub uvs: Vec<f32>,
    pub position_indices: Vec<u16>,
    /// Clipping-mask pool: art-mesh indices.
    pub drawable_masks: Vec<i32>,
    pub draw_groups: Vec<DrawGroupData>,
    pub draw_items: Vec<DrawItemData>,
}

impl Default for Moc3Data {
    fn default() -> Self {
        Self {
            version: Moc3Version::V3_0,
            canvas: CanvasSpec::default(),
            parts: Vec::new(),
            part_form_depths: Vec::new(),
            deformers: Vec::new(),
            warps: Vec::new(),
            warp_form_opacities: Vec::new(),
            warp_form_position_begins: Vec::new(),
            rotations: Vec::new(),
            rotation_forms: Vec::new(),
            meshes: Vec::new(),
            mesh_form_opacities: Vec::new(),
            mesh_form_depths: Vec::new(),
            mesh_form_position_begins: Vec::new(),
            keyform_positions: Vec::new(),
            parameters: Vec::new(),
            parameter_maps: Vec::new(),
            binding_refs: Vec::new(),
            binding_sets: Vec::new(),
            keys: Vec::new(),
            uvs: Vec::new(),
            position_indices: Vec::new(),
            drawable_masks: Vec::new(),
            draw_groups: Vec::new(),
            draw_items: Vec::new(),
        }
    }
}

/// The count info table values the writer computed for a file,
/// in CIT order 0-19 (fields 20-22 are glue, always 0 here;
/// 23-31 are padding). The mapping layer mirrors these into
/// the export report.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Moc3Counts {
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
}

fn export_error(message: String) -> ConvertError {
    ConvertError::Export { message }
}

fn i32_of(value: usize, what: &str) -> Result<i32, ConvertError> {
    i32::try_from(value).map_err(|_| {
        export_error(format!(
            "{what}: value {value} does not fit the i32 fields of the format"
        ))
    })
}

fn u32_of(value: usize, what: &str) -> Result<u32, ConvertError> {
    u32::try_from(value).map_err(|_| {
        export_error(format!(
            "{what}: value {value} does not fit the u32 fields of the format"
        ))
    })
}

fn check_finite(value: f32, what: &str) -> Result<(), ConvertError> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(export_error(format!("{what}: value {value} is not finite")))
    }
}

fn check_window(begin: usize, count: usize, pool: usize, what: &str) -> Result<(), ConvertError> {
    let end = begin
        .checked_add(count)
        .ok_or_else(|| export_error(format!("{what}: window [{begin}, +{count}) overflows")))?;
    if end > pool {
        return Err(export_error(format!(
            "{what}: window [{begin}, {end}) exceeds its pool of {pool}"
        )));
    }
    Ok(())
}

fn check_parent(value: i32, count: usize, what: &str) -> Result<(), ConvertError> {
    if value < -1 || (value >= 0 && value as usize >= count) {
        return Err(export_error(format!("{what}: index {value} out of range")));
    }
    Ok(())
}

/// Validate the writer input structurally: parallel-array
/// lengths, index ranges, finite floats, strictly increasing
/// key lists, and the format's hard identities. Every failure
/// is a typed error; the writer never emits a partial file.
fn validate(data: &Moc3Data) -> Result<(), ConvertError> {
    for (name, value) in [
        ("canvas pixels_per_unit", data.canvas.pixels_per_unit),
        ("canvas origin_x", data.canvas.origin_x),
        ("canvas origin_y", data.canvas.origin_y),
        ("canvas width", data.canvas.width),
        ("canvas height", data.canvas.height),
    ] {
        check_finite(value, name)?;
    }
    if data.part_form_depths.len() != data.parts.len() {
        return Err(export_error(format!(
            "part form depth pool has {} entries for {} parts (exactly one form per part)",
            data.part_form_depths.len(),
            data.parts.len()
        )));
    }
    for (index, part) in data.parts.iter().enumerate() {
        check_parent(
            part.parent,
            data.parts.len(),
            &format!("part {index} parent"),
        )?;
        if part.binding_set >= data.binding_sets.len() {
            return Err(export_error(format!(
                "part {:?}: binding set {} out of range",
                part.id, part.binding_set
            )));
        }
        check_finite(data.part_form_depths[index], "part form depth")?;
    }
    if data.warp_form_opacities.len() != data.warp_form_position_begins.len() {
        return Err(export_error(format!(
            "warp form pools disagree: {} opacities vs {} position begins",
            data.warp_form_opacities.len(),
            data.warp_form_position_begins.len()
        )));
    }
    if data.mesh_form_opacities.len() != data.mesh_form_depths.len()
        || data.mesh_form_opacities.len() != data.mesh_form_position_begins.len()
    {
        return Err(export_error(format!(
            "art mesh form pools disagree: {} opacities, {} depths, {} position begins",
            data.mesh_form_opacities.len(),
            data.mesh_form_depths.len(),
            data.mesh_form_position_begins.len()
        )));
    }
    for pool in [
        &data.warp_form_opacities,
        &data.mesh_form_opacities,
        &data.mesh_form_depths,
        &data.keyform_positions,
        &data.keys,
        &data.uvs,
    ] {
        for value in pool {
            check_finite(*value, "float pool")?;
        }
    }
    let mut typed_seen = [0usize; 2];
    for (index, deformer) in data.deformers.iter().enumerate() {
        if deformer.binding_set >= data.binding_sets.len() {
            return Err(export_error(format!(
                "deformer {:?}: binding set {} out of range",
                deformer.id, deformer.binding_set
            )));
        }
        check_parent(
            deformer.parent_part,
            data.parts.len(),
            &format!("deformer {index} parent part"),
        )?;
        check_parent(
            deformer.parent,
            data.deformers.len(),
            &format!("deformer {index} parent"),
        )?;
        let typed_pool = match deformer.kind {
            0 => data.warps.len(),
            1 => data.rotations.len(),
            other => {
                return Err(export_error(format!(
                    "deformer {:?}: unknown type {other}",
                    deformer.id
                )));
            }
        };
        if deformer.typed_index >= typed_pool {
            return Err(export_error(format!(
                "deformer {:?}: typed index {} out of range for its kind",
                deformer.id, deformer.typed_index
            )));
        }
        typed_seen[deformer.kind as usize] += 1;
    }
    if typed_seen[0] != data.warps.len() || typed_seen[1] != data.rotations.len() {
        return Err(export_error(format!(
            "deformer typed tables disagree with the unified table: {} warps / {} rotations referenced, {} / {} present",
            typed_seen[0],
            typed_seen[1],
            data.warps.len(),
            data.rotations.len()
        )));
    }
    for (index, warp) in data.warps.iter().enumerate() {
        if warp.binding_set >= data.binding_sets.len() {
            return Err(export_error(format!(
                "warp deformer {index}: binding set {} out of range",
                warp.binding_set
            )));
        }
        check_window(
            warp.form_begin,
            warp.form_count,
            data.warp_form_opacities.len(),
            "warp deformer forms",
        )?;
        let grid = (warp.rows as u64 + 1) * (warp.columns as u64 + 1);
        if grid != warp.vertex_count as u64 {
            return Err(export_error(format!(
                "warp deformer {index}: grid {}x{} does not match vertex count {}",
                warp.rows, warp.columns, warp.vertex_count
            )));
        }
    }
    for (index, rotation) in data.rotations.iter().enumerate() {
        if rotation.binding_set >= data.binding_sets.len() {
            return Err(export_error(format!(
                "rotation deformer {index}: binding set {} out of range",
                rotation.binding_set
            )));
        }
        check_window(
            rotation.form_begin,
            rotation.form_count,
            data.rotation_forms.len(),
            "rotation deformer forms",
        )?;
        check_finite(rotation.base_angle, "rotation base angle")?;
    }
    for form in &data.rotation_forms {
        check_finite(form.opacity, "rotation form opacity")?;
        check_finite(form.angle, "rotation form angle")?;
        check_finite(form.origin[0], "rotation form origin x")?;
        check_finite(form.origin[1], "rotation form origin y")?;
        check_finite(form.scale, "rotation form scale")?;
    }
    for (index, mesh) in data.meshes.iter().enumerate() {
        if mesh.binding_set >= data.binding_sets.len() {
            return Err(export_error(format!(
                "art mesh {:?}: binding set {} out of range",
                mesh.id, mesh.binding_set
            )));
        }
        if mesh.parent_part < 0 || mesh.parent_part as usize >= data.parts.len() {
            return Err(export_error(format!(
                "art mesh {:?}: parent part {} out of range",
                mesh.id, mesh.parent_part
            )));
        }
        check_parent(
            mesh.parent_deformer,
            data.deformers.len(),
            &format!("art mesh {index} parent deformer"),
        )?;
        if mesh.render_config & 0xF0 != 0 {
            return Err(export_error(format!(
                "art mesh {:?}: render config {:#04x} has high-nibble bits set",
                mesh.id, mesh.render_config
            )));
        }
        check_window(
            mesh.form_begin,
            mesh.form_count,
            data.mesh_form_opacities.len(),
            "art mesh forms",
        )?;
        check_window(
            mesh.uv_begin,
            mesh.vertex_count * 2,
            data.uvs.len(),
            "art mesh uvs",
        )?;
        check_window(
            mesh.index_begin,
            mesh.index_count,
            data.position_indices.len(),
            "art mesh position indices",
        )?;
        check_window(
            mesh.mask_begin,
            mesh.mask_count,
            data.drawable_masks.len(),
            "art mesh masks",
        )?;
    }
    for mask in &data.drawable_masks {
        if *mask < 0 || *mask as usize >= data.meshes.len() {
            return Err(export_error(format!(
                "drawable mask index {mask} out of range"
            )));
        }
    }
    for (index, parameter) in data.parameters.iter().enumerate() {
        check_window(
            parameter.map_begin,
            parameter.map_count,
            data.parameter_maps.len(),
            &format!("parameter {index} maps"),
        )?;
        if !matches!(parameter.snap_type, 0 | 1 | 3) {
            return Err(export_error(format!(
                "parameter {:?}: snap type {} does not exist (0, 1, or 3)",
                parameter.id, parameter.snap_type
            )));
        }
        check_finite(parameter.min, "parameter min")?;
        check_finite(parameter.max, "parameter max")?;
        check_finite(parameter.default, "parameter default")?;
        if parameter.min > parameter.max {
            return Err(export_error(format!(
                "parameter {:?}: min {} exceeds max {}",
                parameter.id, parameter.min, parameter.max
            )));
        }
    }
    // Parameter map ownership must partition the map pool.
    {
        let mut owned = vec![false; data.parameter_maps.len()];
        for parameter in &data.parameters {
            for slot in &mut owned[parameter.map_begin..parameter.map_begin + parameter.map_count] {
                if *slot {
                    return Err(export_error(
                        "parameter map ownership ranges overlap".to_string(),
                    ));
                }
                *slot = true;
            }
        }
        if owned.contains(&false) {
            return Err(export_error(
                "some parameter maps have no owning parameter".to_string(),
            ));
        }
    }
    for (index, map) in data.parameter_maps.iter().enumerate() {
        check_window(
            map.key_begin,
            map.key_count,
            data.keys.len(),
            "parameter map keys",
        )?;
        let keys = &data.keys[map.key_begin..map.key_begin + map.key_count];
        if keys.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(export_error(format!(
                "parameter map {index}: keys are not strictly increasing"
            )));
        }
    }
    for (index, set) in data.binding_sets.iter().enumerate() {
        check_window(
            set.ref_begin,
            set.ref_count,
            data.binding_refs.len(),
            &format!("binding set {index}"),
        )?;
    }
    for reference in &data.binding_refs {
        if *reference >= data.parameter_maps.len() {
            return Err(export_error(format!(
                "binding reference {reference} out of range"
            )));
        }
    }
    for (index, group) in data.draw_groups.iter().enumerate() {
        check_window(
            group.item_begin,
            group.item_count,
            data.draw_items.len(),
            &format!("draw group {index} items"),
        )?;
        check_finite(group.max_depth, "draw group max depth")?;
        check_finite(group.min_depth, "draw group min depth")?;
    }
    for item in &data.draw_items {
        let bound = match item.item_type {
            0 => data.meshes.len(),
            1 => data.parts.len(),
            other => {
                return Err(export_error(format!("draw item has unknown type {other}")));
            }
        };
        if item.child >= bound {
            return Err(export_error(format!(
                "draw item child {} out of range for its type",
                item.child
            )));
        }
        check_parent(item.draw_group, data.draw_groups.len(), "draw item group")?;
    }
    Ok(())
}

/// The counts the writer will stamp into the count info table.
fn counts_of(data: &Moc3Data) -> Moc3Counts {
    Moc3Counts {
        parts: data.parts.len(),
        deformers: data.deformers.len(),
        warp_deformers: data.warps.len(),
        rotation_deformers: data.rotations.len(),
        art_meshes: data.meshes.len(),
        parameters: data.parameters.len(),
        part_keyforms: data.part_form_depths.len(),
        warp_keyforms: data.warp_form_opacities.len(),
        rotation_keyforms: data.rotation_forms.len(),
        art_mesh_keyforms: data.mesh_form_opacities.len(),
        keyform_positions: data.keyform_positions.len(),
        parameter_binding_index: data.binding_refs.len(),
        keyform_bindings: data.binding_sets.len(),
        parameter_bindings: data.parameter_maps.len(),
        keys: data.keys.len(),
        uvs: data.uvs.len(),
        position_indices: data.position_indices.len(),
        drawable_masks: data.drawable_masks.len(),
        draw_order_groups: data.draw_groups.len(),
        draw_order_group_objects: data.draw_items.len(),
    }
}

fn enc_i32s(values: &[i32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

fn enc_u32s(values: &[u32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

fn enc_f32s(values: &[f32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

fn enc_bools(values: &[bool]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| (*value as i32).to_le_bytes())
        .collect()
}

fn enc_ids(ids: &[&str], what: &str) -> Result<Vec<u8>, ConvertError> {
    let mut out = Vec::with_capacity(ids.len() * ID_BLOCK_BYTES);
    for id in ids {
        if id.is_empty() || id.len() > ID_BLOCK_BYTES || id.as_bytes().contains(&0) {
            return Err(export_error(format!(
                "{what}: id {id:?} does not fit the 64-byte id block (empty, overlong, or NUL-bearing)"
            )));
        }
        let mut block = [0u8; ID_BLOCK_BYTES];
        block[..id.len()].copy_from_slice(id.as_bytes());
        out.extend_from_slice(&block);
    }
    Ok(out)
}

/// Write `data` as a complete `.moc3` byte stream. Fails with a
/// typed [`ConvertError::Export`] on any structural problem —
/// no partial output exists on the error path.
pub fn write_moc3(data: &Moc3Data) -> Result<Vec<u8>, ConvertError> {
    let version_word = data.version.version_word()?;
    validate(data)?;
    let counts = counts_of(data);

    let mut sections: Vec<(usize, Vec<u8>)> = Vec::with_capacity(89);
    let mut push = |slot_index: usize, bytes: Vec<u8>| sections.push((slot_index, bytes));

    // Count info table (slot 0): 23 used fields, 9 zero padding.
    {
        let mut cit = [0i32; CIT_FIELDS];
        let fields = [
            counts.parts,
            counts.deformers,
            counts.warp_deformers,
            counts.rotation_deformers,
            counts.art_meshes,
            counts.parameters,
            counts.part_keyforms,
            counts.warp_keyforms,
            counts.rotation_keyforms,
            counts.art_mesh_keyforms,
            counts.keyform_positions,
            counts.parameter_binding_index,
            counts.keyform_bindings,
            counts.parameter_bindings,
            counts.keys,
            counts.uvs,
            counts.position_indices,
            counts.drawable_masks,
            counts.draw_order_groups,
            counts.draw_order_group_objects,
            0, // glue
            0, // glue info
            0, // glue keyforms
        ];
        for (at, value) in fields.iter().enumerate() {
            cit[at] = i32_of(*value, "count info table")?;
        }
        push(slot::COUNT_INFO_TABLE, enc_i32s(&cit));
    }
    // Canvas info (slot 1): one 64-byte record, five f32 used.
    {
        let mut canvas = vec![0u8; 64];
        for (at, value) in [
            data.canvas.pixels_per_unit,
            data.canvas.origin_x,
            data.canvas.origin_y,
            data.canvas.width,
            data.canvas.height,
        ]
        .iter()
        .enumerate()
        {
            canvas[at * 4..at * 4 + 4].copy_from_slice(&value.to_le_bytes());
        }
        push(slot::CANVAS_INFO, canvas);
    }

    let zero_pairs = |count: usize| vec![0u8; count * 8];

    // Parts (slots 2-9).
    push(slot::PART_HEADERS, zero_pairs(data.parts.len()));
    push(
        slot::PART_IDS,
        enc_ids(
            &data
                .parts
                .iter()
                .map(|part| part.id.as_str())
                .collect::<Vec<_>>(),
            "part ids",
        )?,
    );
    push(
        slot::PART_BINDING_SETS,
        enc_u32s(
            &data
                .parts
                .iter()
                .map(|part| u32_of(part.binding_set, "part binding set"))
                .collect::<Result<Vec<_>, _>>()?,
        ),
    );
    push(
        slot::PART_FORM_BEGINS,
        enc_u32s(&(0..data.parts.len() as u32).collect::<Vec<_>>()),
    );
    push(
        slot::PART_FORM_COUNTS,
        enc_u32s(&vec![1u32; data.parts.len()]),
    );
    push(
        slot::PART_VISIBLE_ART_MESHES,
        enc_bools(
            &data
                .parts
                .iter()
                .map(|part| part.visible_art_meshes)
                .collect::<Vec<_>>(),
        ),
    );
    push(
        slot::PART_VISIBLE_DEFORMERS,
        enc_bools(
            &data
                .parts
                .iter()
                .map(|part| part.visible_deformers)
                .collect::<Vec<_>>(),
        ),
    );
    push(
        slot::PART_PARENT_INDICES,
        enc_i32s(
            &data
                .parts
                .iter()
                .map(|part| part.parent)
                .collect::<Vec<_>>(),
        ),
    );

    // Deformers, unified (slots 10-18).
    push(slot::DEFORMER_HEADERS, zero_pairs(data.deformers.len()));
    push(
        slot::DEFORMER_IDS,
        enc_ids(
            &data
                .deformers
                .iter()
                .map(|deformer| deformer.id.as_str())
                .collect::<Vec<_>>(),
            "deformer ids",
        )?,
    );
    push(
        slot::DEFORMER_BINDING_SETS,
        enc_i32s(
            &data
                .deformers
                .iter()
                .map(|deformer| i32_of(deformer.binding_set, "deformer binding set"))
                .collect::<Result<Vec<_>, _>>()?,
        ),
    );
    push(
        slot::DEFORMER_UNKNOWN_FLAGS,
        enc_bools(
            &data
                .deformers
                .iter()
                .map(|deformer| deformer.unknown_flag)
                .collect::<Vec<_>>(),
        ),
    );
    push(
        slot::DEFORMER_VISIBLE,
        enc_bools(
            &data
                .deformers
                .iter()
                .map(|deformer| deformer.visible)
                .collect::<Vec<_>>(),
        ),
    );
    push(
        slot::DEFORMER_PARENT_PART_INDICES,
        enc_i32s(
            &data
                .deformers
                .iter()
                .map(|deformer| deformer.parent_part)
                .collect::<Vec<_>>(),
        ),
    );
    push(
        slot::DEFORMER_PARENT_INDICES,
        enc_i32s(
            &data
                .deformers
                .iter()
                .map(|deformer| deformer.parent)
                .collect::<Vec<_>>(),
        ),
    );
    push(
        slot::DEFORMER_TYPES,
        enc_u32s(
            &data
                .deformers
                .iter()
                .map(|deformer| deformer.kind)
                .collect::<Vec<_>>(),
        ),
    );
    push(
        slot::DEFORMER_SPECIFIC_INDICES,
        enc_u32s(
            &data
                .deformers
                .iter()
                .map(|deformer| u32_of(deformer.typed_index, "deformer typed index"))
                .collect::<Result<Vec<_>, _>>()?,
        ),
    );

    // Warp deformers (slots 19-24).
    let warp_u32s = |pick: &dyn Fn(&WarpData) -> usize, what: &str| {
        data.warps
            .iter()
            .map(|warp| u32_of(pick(warp), what))
            .collect::<Result<Vec<u32>, _>>()
    };
    push(
        slot::WARP_BINDING_SETS,
        enc_i32s(
            &data
                .warps
                .iter()
                .map(|warp| i32_of(warp.binding_set, "warp binding set"))
                .collect::<Result<Vec<_>, _>>()?,
        ),
    );
    push(
        slot::WARP_FORM_BEGINS,
        enc_u32s(&warp_u32s(&|warp| warp.form_begin, "warp form begin")?),
    );
    push(
        slot::WARP_FORM_COUNTS,
        enc_u32s(&warp_u32s(&|warp| warp.form_count, "warp form count")?),
    );
    push(
        slot::WARP_VERTEX_COUNTS,
        enc_u32s(&warp_u32s(&|warp| warp.vertex_count, "warp vertex count")?),
    );
    push(
        slot::WARP_ROWS,
        enc_u32s(&warp_u32s(&|warp| warp.rows, "warp rows")?),
    );
    push(
        slot::WARP_COLUMNS,
        enc_u32s(&warp_u32s(&|warp| warp.columns, "warp columns")?),
    );

    // Rotation deformers (slots 25-28).
    push(
        slot::ROTATION_BINDING_SETS,
        enc_i32s(
            &data
                .rotations
                .iter()
                .map(|rotation| i32_of(rotation.binding_set, "rotation binding set"))
                .collect::<Result<Vec<_>, _>>()?,
        ),
    );
    push(
        slot::ROTATION_FORM_BEGINS,
        enc_u32s(
            &data
                .rotations
                .iter()
                .map(|rotation| u32_of(rotation.form_begin, "rotation form begin"))
                .collect::<Result<Vec<_>, _>>()?,
        ),
    );
    push(
        slot::ROTATION_FORM_COUNTS,
        enc_u32s(
            &data
                .rotations
                .iter()
                .map(|rotation| u32_of(rotation.form_count, "rotation form count"))
                .collect::<Result<Vec<_>, _>>()?,
        ),
    );
    push(
        slot::ROTATION_BASE_ANGLES,
        enc_f32s(
            &data
                .rotations
                .iter()
                .map(|rotation| rotation.base_angle)
                .collect::<Vec<_>>(),
        ),
    );

    // Art meshes (slots 29-48).
    push(slot::MESH_HEADERS, zero_pairs(data.meshes.len()));
    push(slot::MESH_UNKNOWN_A, zero_pairs(data.meshes.len()));
    push(slot::MESH_UNKNOWN_B, zero_pairs(data.meshes.len()));
    push(slot::MESH_UNKNOWN_C, zero_pairs(data.meshes.len()));
    push(
        slot::MESH_IDS,
        enc_ids(
            &data
                .meshes
                .iter()
                .map(|mesh| mesh.id.as_str())
                .collect::<Vec<_>>(),
            "art mesh ids",
        )?,
    );
    push(
        slot::MESH_BINDING_SETS,
        enc_i32s(
            &data
                .meshes
                .iter()
                .map(|mesh| i32_of(mesh.binding_set, "art mesh binding set"))
                .collect::<Result<Vec<_>, _>>()?,
        ),
    );
    let mesh_u32s = |pick: &dyn Fn(&MeshData) -> usize, what: &str| {
        data.meshes
            .iter()
            .map(|mesh| u32_of(pick(mesh), what))
            .collect::<Result<Vec<u32>, _>>()
    };
    push(
        slot::MESH_FORM_BEGINS,
        enc_u32s(&mesh_u32s(&|mesh| mesh.form_begin, "art mesh form begin")?),
    );
    push(
        slot::MESH_FORM_COUNTS,
        enc_u32s(&mesh_u32s(&|mesh| mesh.form_count, "art mesh form count")?),
    );
    push(
        slot::MESH_UNKNOWN_FLAGS,
        enc_bools(
            &data
                .meshes
                .iter()
                .map(|mesh| mesh.unknown_flag)
                .collect::<Vec<_>>(),
        ),
    );
    push(
        slot::MESH_VISIBLE,
        enc_bools(
            &data
                .meshes
                .iter()
                .map(|mesh| mesh.visible)
                .collect::<Vec<_>>(),
        ),
    );
    push(
        slot::MESH_PARENT_PART_INDICES,
        enc_i32s(
            &data
                .meshes
                .iter()
                .map(|mesh| mesh.parent_part)
                .collect::<Vec<_>>(),
        ),
    );
    push(
        slot::MESH_PARENT_DEFORMER_INDICES,
        enc_i32s(
            &data
                .meshes
                .iter()
                .map(|mesh| mesh.parent_deformer)
                .collect::<Vec<_>>(),
        ),
    );
    push(
        slot::MESH_TEXTURE_NUMBERS,
        enc_u32s(&mesh_u32s(
            &|mesh| mesh.texture_number,
            "art mesh texture number",
        )?),
    );
    push(
        slot::MESH_RENDER_CONFIGS,
        data.meshes
            .iter()
            .map(|mesh| mesh.render_config)
            .collect::<Vec<u8>>(),
    );
    push(
        slot::MESH_VERTEX_COUNTS,
        enc_u32s(&mesh_u32s(
            &|mesh| mesh.vertex_count,
            "art mesh vertex count",
        )?),
    );
    push(
        slot::MESH_UV_BEGINS,
        enc_u32s(&mesh_u32s(&|mesh| mesh.uv_begin, "art mesh uv begin")?),
    );
    push(
        slot::MESH_INDEX_BEGINS,
        enc_u32s(&mesh_u32s(
            &|mesh| mesh.index_begin,
            "art mesh index begin",
        )?),
    );
    push(
        slot::MESH_INDEX_COUNTS,
        enc_u32s(&mesh_u32s(
            &|mesh| mesh.index_count,
            "art mesh index count",
        )?),
    );
    push(
        slot::MESH_MASK_BEGINS,
        enc_u32s(&mesh_u32s(&|mesh| mesh.mask_begin, "art mesh mask begin")?),
    );
    push(
        slot::MESH_MASK_COUNTS,
        enc_u32s(&mesh_u32s(&|mesh| mesh.mask_count, "art mesh mask count")?),
    );

    // Parameters (slots 49-57).
    push(slot::PARAMETER_HEADERS, zero_pairs(data.parameters.len()));
    push(
        slot::PARAMETER_IDS,
        enc_ids(
            &data
                .parameters
                .iter()
                .map(|parameter| parameter.id.as_str())
                .collect::<Vec<_>>(),
            "parameter ids",
        )?,
    );
    push(
        slot::PARAMETER_MAX_VALUES,
        enc_f32s(
            &data
                .parameters
                .iter()
                .map(|parameter| parameter.max)
                .collect::<Vec<_>>(),
        ),
    );
    push(
        slot::PARAMETER_MIN_VALUES,
        enc_f32s(
            &data
                .parameters
                .iter()
                .map(|parameter| parameter.min)
                .collect::<Vec<_>>(),
        ),
    );
    push(
        slot::PARAMETER_DEFAULT_VALUES,
        enc_f32s(
            &data
                .parameters
                .iter()
                .map(|parameter| parameter.default)
                .collect::<Vec<_>>(),
        ),
    );
    push(
        slot::PARAMETER_REPEAT_TOGGLES,
        enc_bools(
            &data
                .parameters
                .iter()
                .map(|parameter| parameter.repeat)
                .collect::<Vec<_>>(),
        ),
    );
    push(
        slot::PARAMETER_SNAP_TYPES,
        enc_u32s(
            &data
                .parameters
                .iter()
                .map(|parameter| parameter.snap_type)
                .collect::<Vec<_>>(),
        ),
    );
    push(
        slot::PARAMETER_MAP_BEGINS,
        enc_u32s(
            &data
                .parameters
                .iter()
                .map(|parameter| u32_of(parameter.map_begin, "parameter map begin"))
                .collect::<Result<Vec<_>, _>>()?,
        ),
    );
    push(
        slot::PARAMETER_MAP_COUNTS,
        enc_u32s(
            &data
                .parameters
                .iter()
                .map(|parameter| u32_of(parameter.map_count, "parameter map count"))
                .collect::<Result<Vec<_>, _>>()?,
        ),
    );

    // Keyform scalar pools (slots 58-70).
    push(slot::PART_FORM_DEPTHS, enc_f32s(&data.part_form_depths));
    push(
        slot::WARP_FORM_OPACITIES,
        enc_f32s(&data.warp_form_opacities),
    );
    push(
        slot::WARP_FORM_POSITION_BEGINS,
        enc_u32s(
            &data
                .warp_form_position_begins
                .iter()
                .map(|begin| u32_of(*begin, "warp form position begin"))
                .collect::<Result<Vec<_>, _>>()?,
        ),
    );
    push(
        slot::ROTATION_FORM_OPACITIES,
        enc_f32s(
            &data
                .rotation_forms
                .iter()
                .map(|form| form.opacity)
                .collect::<Vec<_>>(),
        ),
    );
    push(
        slot::ROTATION_FORM_ANGLES,
        enc_f32s(
            &data
                .rotation_forms
                .iter()
                .map(|form| form.angle)
                .collect::<Vec<_>>(),
        ),
    );
    push(
        slot::ROTATION_FORM_ORIGIN_X,
        enc_f32s(
            &data
                .rotation_forms
                .iter()
                .map(|form| form.origin[0])
                .collect::<Vec<_>>(),
        ),
    );
    push(
        slot::ROTATION_FORM_ORIGIN_Y,
        enc_f32s(
            &data
                .rotation_forms
                .iter()
                .map(|form| form.origin[1])
                .collect::<Vec<_>>(),
        ),
    );
    push(
        slot::ROTATION_FORM_SCALES,
        enc_f32s(
            &data
                .rotation_forms
                .iter()
                .map(|form| form.scale)
                .collect::<Vec<_>>(),
        ),
    );
    push(
        slot::ROTATION_FORM_REFLECT_X,
        enc_bools(
            &data
                .rotation_forms
                .iter()
                .map(|form| form.reflect_x)
                .collect::<Vec<_>>(),
        ),
    );
    push(
        slot::ROTATION_FORM_REFLECT_Y,
        enc_bools(
            &data
                .rotation_forms
                .iter()
                .map(|form| form.reflect_y)
                .collect::<Vec<_>>(),
        ),
    );
    push(
        slot::MESH_FORM_OPACITIES,
        enc_f32s(&data.mesh_form_opacities),
    );
    push(slot::MESH_FORM_DEPTHS, enc_f32s(&data.mesh_form_depths));
    push(
        slot::MESH_FORM_POSITION_BEGINS,
        enc_u32s(
            &data
                .mesh_form_position_begins
                .iter()
                .map(|begin| u32_of(*begin, "art mesh form position begin"))
                .collect::<Result<Vec<_>, _>>()?,
        ),
    );

    // Shared pools (slots 71-80).
    push(slot::KEYFORM_POSITIONS, enc_f32s(&data.keyform_positions));
    push(
        slot::PARAMETER_BINDING_INDEX,
        enc_u32s(
            &data
                .binding_refs
                .iter()
                .map(|reference| u32_of(*reference, "binding reference"))
                .collect::<Result<Vec<_>, _>>()?,
        ),
    );
    push(
        slot::KEYFORM_BINDING_BEGINS,
        enc_u32s(
            &data
                .binding_sets
                .iter()
                .map(|set| u32_of(set.ref_begin, "binding set begin"))
                .collect::<Result<Vec<_>, _>>()?,
        ),
    );
    push(
        slot::KEYFORM_BINDING_COUNTS,
        enc_u32s(
            &data
                .binding_sets
                .iter()
                .map(|set| u32_of(set.ref_count, "binding set count"))
                .collect::<Result<Vec<_>, _>>()?,
        ),
    );
    push(
        slot::PARAMETER_BINDING_KEY_BEGINS,
        enc_u32s(
            &data
                .parameter_maps
                .iter()
                .map(|map| u32_of(map.key_begin, "parameter map key begin"))
                .collect::<Result<Vec<_>, _>>()?,
        ),
    );
    push(
        slot::PARAMETER_BINDING_KEY_COUNTS,
        enc_u32s(
            &data
                .parameter_maps
                .iter()
                .map(|map| u32_of(map.key_count, "parameter map key count"))
                .collect::<Result<Vec<_>, _>>()?,
        ),
    );
    push(slot::KEYS, enc_f32s(&data.keys));
    push(slot::UVS, enc_f32s(&data.uvs));
    push(
        slot::POSITION_INDICES,
        data.position_indices
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect::<Vec<u8>>(),
    );
    push(slot::DRAWABLE_MASKS, enc_i32s(&data.drawable_masks));

    // Draw order (slots 81-88).
    push(
        slot::DRAW_ORDER_GROUP_ITEM_BEGINS,
        enc_u32s(
            &data
                .draw_groups
                .iter()
                .map(|group| u32_of(group.item_begin, "draw group item begin"))
                .collect::<Result<Vec<_>, _>>()?,
        ),
    );
    push(
        slot::DRAW_ORDER_GROUP_ITEM_COUNTS,
        enc_u32s(
            &data
                .draw_groups
                .iter()
                .map(|group| u32_of(group.item_count, "draw group item count"))
                .collect::<Result<Vec<_>, _>>()?,
        ),
    );
    push(
        slot::DRAW_ORDER_GROUP_TOTALS,
        enc_u32s(
            &data
                .draw_groups
                .iter()
                .map(|group| u32_of(group.total_art_meshes, "draw group total"))
                .collect::<Result<Vec<_>, _>>()?,
        ),
    );
    push(
        slot::DRAW_ORDER_GROUP_MAXIMUMS,
        enc_f32s(
            &data
                .draw_groups
                .iter()
                .map(|group| group.max_depth)
                .collect::<Vec<_>>(),
        ),
    );
    push(
        slot::DRAW_ORDER_GROUP_MINIMUMS,
        enc_f32s(
            &data
                .draw_groups
                .iter()
                .map(|group| group.min_depth)
                .collect::<Vec<_>>(),
        ),
    );
    push(
        slot::DRAW_ORDER_OBJECT_TYPES,
        enc_u32s(
            &data
                .draw_items
                .iter()
                .map(|item| item.item_type)
                .collect::<Vec<_>>(),
        ),
    );
    push(
        slot::DRAW_ORDER_OBJECT_INDICES,
        enc_u32s(
            &data
                .draw_items
                .iter()
                .map(|item| u32_of(item.child, "draw item child"))
                .collect::<Result<Vec<_>, _>>()?,
        ),
    );
    push(
        slot::DRAW_ORDER_OBJECT_GROUP_REFS,
        enc_i32s(
            &data
                .draw_items
                .iter()
                .map(|item| item.draw_group)
                .collect::<Vec<_>>(),
        ),
    );

    // Assemble: header, offset table, sections in slot order.
    // Sections the writer does not emit (the glue family,
    // slots 89-100 at V3_0) still need offsets: sequential
    // readers advance to every slot's offset unconditionally,
    // and the editor's own files point an empty section at
    // the end of file (verified on the official samples: in
    // Haru, every empty glue slot holds exactly the file
    // length). Slots past the version's section count are
    // table padding and stay 0, as in the samples.
    let mut sot = [0i32; SOT_ENTRIES];
    let mut cursor = HEADER_BYTES + SOT_ENTRIES * 4;
    let mut previous_slot: Option<usize> = None;
    for (slot_index, bytes) in &sections {
        if previous_slot.is_some_and(|previous| *slot_index <= previous) {
            return Err(export_error(format!(
                "internal error: sections emitted out of slot order at slot {slot_index}"
            )));
        }
        previous_slot = Some(*slot_index);
        sot[*slot_index] = i32_of(cursor, "section offset")?;
        cursor += bytes.len();
    }
    let end = i32_of(cursor, "section offset")?;
    for entry in sot.iter_mut().take(data.version.section_count()) {
        if *entry == 0 {
            *entry = end;
        }
    }
    let mut out = Vec::with_capacity(cursor);
    out.extend_from_slice(b"MOC3");
    out.extend_from_slice(&version_word.to_le_bytes());
    out.resize(SOT_OFFSET, 0);
    // The R5a parser's header check reads byte 5 as an endianness
    // flag and requires 0; the version word's little-endian
    // encoding already guarantees that (word 1 -> byte 5 = 0).
    debug_assert_eq!(out[5], 0);
    out.extend_from_slice(&enc_i32s(&sot));
    for (_, bytes) in &sections {
        out.extend_from_slice(bytes);
    }
    debug_assert_eq!(out.len(), cursor);
    Ok(out)
}
