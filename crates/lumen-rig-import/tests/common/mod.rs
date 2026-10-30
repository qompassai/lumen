//! Shared fixtures for the converter test suites.
//!
//! The moc3 fixture is assembled byte by byte from the same
//! community-spec layout the parser implements (and that the
//! converter's layout battery cross-checks): a 0x40-byte header,
//! a 160-entry section offset table of absolute i32 offsets, the
//! 32-field count info table, then the section arrays. If the
//! fixture and the parser ever disagree about the layout, the
//! battery fails loudly in these tests — that is the point of
//! building the fixture at the byte level instead of mocking the
//! parser's data structures.
//!
//! The synthetic model, small but complete:
//! - 2 parts (root + child), 3 parameters (two angles, one blink)
//! - 1 rotation deformer (bound to ParamAngleX, 3 keyforms)
//! - 1 warp deformer, 2x2 cells (bound to [ParamAngleY,
//!   ParamAngleX], 9 keyforms), parent of both meshes
//! - 2 art meshes: a clipped quad (9 keyforms) and its mask (3)
//! - 1 draw order group spanning both meshes
//! - physics3 with one 3-vertex setting, motion3 with a bezier
//!   and a stepped curve, one exp3 expression

// Each test target includes this module separately; helpers used
// by only one suite would otherwise read as dead code in the
// other target.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

// Section slots used by the fixture (the parser's slot map).
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
    pub const PART_KEYFORM_DRAW_ORDERS: usize = 58;
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
    pub const DRAW_ORDER_GROUP_OBJECT_TOTALS: usize = 83;
    pub const DRAW_ORDER_GROUP_MAXIMUMS: usize = 84;
    pub const DRAW_ORDER_GROUP_MINIMUMS: usize = 85;
    pub const DRAW_ORDER_OBJECT_TYPES: usize = 86;
    pub const DRAW_ORDER_OBJECT_INDICES: usize = 87;
    pub const DRAW_ORDER_OBJECT_SELF_INDICES: usize = 88;
}

/// Knobs for the adversarial variants of the fixture.
#[derive(Debug, Clone, Default)]
pub struct FixtureOptions {
    /// Rotation keyform 1 (the default form) reflects X.
    pub rotation_reflect: bool,
    /// The warp deformer is its own parent deformer.
    pub deformer_parent_cycle: bool,
    /// The mask mesh masks the quad back (a two-mesh mask cycle).
    pub mask_cycle: bool,
    /// The quad masks itself.
    pub mesh_self_mask: bool,
    /// The quad inverts its clipping mask (flag 0x8, with masks).
    pub invert_mask: bool,
    /// Override the art mesh count in the count info table.
    pub art_mesh_count_override: Option<i32>,
    /// The moc3 version byte to stamp.
    pub version: Option<u8>,
}

fn i32_bytes(values: &[i32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

fn f32_bytes(values: &[f32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

fn id_bytes(ids: &[&str]) -> Vec<u8> {
    let mut out = Vec::new();
    for id in ids {
        let mut block = [0u8; 64];
        block[..id.len()].copy_from_slice(id.as_bytes());
        out.extend_from_slice(&block);
    }
    out
}

/// Assemble the fixture moc3 byte stream.
pub fn fixture_moc3(options: &FixtureOptions) -> Vec<u8> {
    let mut sections: BTreeMap<usize, Vec<u8>> = BTreeMap::new();

    // Count info table: [parts, deformers, warp, rotation,
    // artMeshes, parameters, partKeyforms, warpKeyforms,
    // rotationKeyforms, artMeshKeyforms, keyformPositions,
    // parameterBindingIndex, keyformBindings, parameterBindings,
    // keys, uvs, positionIndices, drawableMasks, drawOrderGroups,
    // drawOrderGroupObjects, glue, glueInfo, glueKeyforms, ...]
    let drawable_mask_count: i32 = if options.mask_cycle { 2 } else { 1 };
    let mut cit = [0i32; 32];
    cit[0] = 2;
    cit[1] = 2;
    cit[2] = 1;
    cit[3] = 1;
    cit[4] = options.art_mesh_count_override.unwrap_or(2);
    cit[5] = 3;
    cit[6] = 2;
    cit[7] = 9;
    cit[8] = 3;
    cit[9] = 12;
    cit[10] = 258;
    cit[11] = 5;
    cit[12] = 4;
    cit[13] = 3;
    cit[14] = 8;
    cit[15] = 16;
    cit[16] = 12;
    cit[17] = drawable_mask_count;
    cit[18] = 1;
    cit[19] = 2;
    sections.insert(slot::COUNT_INFO_TABLE, i32_bytes(&cit));

    // Canvas info: one 64-byte record, five f32 fields.
    let mut canvas = vec![0u8; 64];
    for (at, value) in [
        (0usize, 100.0f32),
        (4, 50.0),
        (8, 60.0),
        (12, 200.0),
        (16, 300.0),
    ] {
        canvas[at..at + 4].copy_from_slice(&value.to_le_bytes());
    }
    sections.insert(slot::CANVAS_INFO, canvas);

    // Parts.
    sections.insert(slot::PART_IDS, id_bytes(&["PartRoot", "PartChild"]));
    sections.insert(slot::PART_PARENT_INDICES, i32_bytes(&[-1, 0]));
    sections.insert(slot::PART_KEYFORM_DRAW_ORDERS, f32_bytes(&[0.0, 0.0]));

    // Parameters and their bindings. Binding key blocks in the
    // flat keys table: [AngleX keys][AngleY keys][Blink keys].
    sections.insert(
        slot::PARAMETER_IDS,
        id_bytes(&["ParamAngleX", "ParamAngleY", "ParamBlink"]),
    );
    sections.insert(slot::PARAMETER_MAX_VALUES, f32_bytes(&[30.0, 30.0, 1.0]));
    sections.insert(slot::PARAMETER_MIN_VALUES, f32_bytes(&[-30.0, -30.0, 0.0]));
    sections.insert(slot::PARAMETER_DEFAULT_VALUES, f32_bytes(&[0.0, 0.0, 1.0]));
    sections.insert(slot::PARAMETER_REPEAT_TOGGLES, i32_bytes(&[0, 0, 0]));
    sections.insert(slot::PARAMETER_BINDING_BEGIN_INDICES, i32_bytes(&[0, 1, 2]));
    sections.insert(slot::PARAMETER_BINDING_COUNTS, i32_bytes(&[1, 1, 1]));
    sections.insert(slot::PARAMETER_BINDING_KEY_BEGINS, i32_bytes(&[0, 3, 6]));
    sections.insert(slot::PARAMETER_BINDING_KEY_COUNTS, i32_bytes(&[3, 3, 2]));
    sections.insert(
        slot::KEYS,
        f32_bytes(&[-30.0, 0.0, 30.0, -30.0, 0.0, 30.0, 0.0, 1.0]),
    );
    // Global binding indices: rotation uses entry 0 (binding 0 =
    // ParamAngleX); warp uses entries 1..3 (bindings 1, 0 =
    // ParamAngleY, ParamAngleX); the quad uses entries 3..5
    // (bindings 1, 0); the mask uses entry 0 (binding 0).
    sections.insert(slot::PARAMETER_BINDING_INDEX, i32_bytes(&[0, 1, 0, 1, 0]));
    sections.insert(
        slot::KEYFORM_BINDING_BEGIN_INDICES,
        i32_bytes(&[0, 1, 3, 0]),
    );
    sections.insert(slot::KEYFORM_BINDING_COUNTS, i32_bytes(&[1, 2, 2, 1]));

    // Deformers (unified order: rotation, warp).
    sections.insert(
        slot::DEFORMER_IDS,
        id_bytes(&["DeformerRot", "DeformerWarp"]),
    );
    sections.insert(slot::DEFORMER_BINDING_INDICES, i32_bytes(&[0, 1]));
    sections.insert(slot::DEFORMER_PARENT_PART_INDICES, i32_bytes(&[0, 0]));
    let warp_parent = if options.deformer_parent_cycle { 1 } else { -1 };
    sections.insert(slot::DEFORMER_PARENT_INDICES, i32_bytes(&[-1, warp_parent]));
    sections.insert(slot::DEFORMER_TYPES, i32_bytes(&[1, 0]));
    sections.insert(slot::DEFORMER_SPECIFIC_INDICES, i32_bytes(&[0, 0]));

    // Rotation deformer: 3 keyforms over ParamAngleX.
    sections.insert(slot::ROTATION_BINDING_INDICES, i32_bytes(&[0]));
    sections.insert(slot::ROTATION_BEGIN_INDICES, i32_bytes(&[0]));
    sections.insert(slot::ROTATION_COUNTS, i32_bytes(&[3]));
    sections.insert(slot::ROTATION_BASE_ANGLES, f32_bytes(&[0.0]));
    sections.insert(
        slot::ROTATION_KEYFORM_OPACITIES,
        f32_bytes(&[1.0, 1.0, 1.0]),
    );
    sections.insert(
        slot::ROTATION_KEYFORM_ANGLES,
        f32_bytes(&[-10.0, 0.0, 10.0]),
    );
    sections.insert(slot::ROTATION_KEYFORM_ORIGIN_X, f32_bytes(&[0.0, 0.0, 0.0]));
    sections.insert(slot::ROTATION_KEYFORM_ORIGIN_Y, f32_bytes(&[0.0, 0.0, 0.0]));
    sections.insert(slot::ROTATION_KEYFORM_SCALES, f32_bytes(&[1.0, 1.0, 1.0]));
    let reflect_x: [i32; 3] = if options.rotation_reflect {
        [0, 1, 0]
    } else {
        [0, 0, 0]
    };
    sections.insert(slot::ROTATION_KEYFORM_REFLECT_X, i32_bytes(&reflect_x));
    sections.insert(slot::ROTATION_KEYFORM_REFLECT_Y, i32_bytes(&[0, 0, 0]));

    // Warp deformer: 2x2 cells = 3x3 grid, 9 keyforms.
    sections.insert(slot::WARP_BINDING_INDICES, i32_bytes(&[1]));
    sections.insert(slot::WARP_BEGIN_INDICES, i32_bytes(&[0]));
    sections.insert(slot::WARP_COUNTS, i32_bytes(&[9]));
    sections.insert(slot::WARP_VERTEX_COUNTS, i32_bytes(&[9]));
    sections.insert(slot::WARP_ROWS, i32_bytes(&[2]));
    sections.insert(slot::WARP_COLUMNS, i32_bytes(&[2]));
    sections.insert(slot::WARP_KEYFORM_OPACITIES, f32_bytes(&[1.0; 9]));
    let warp_begins: Vec<i32> = (0..9).map(|form| form * 18).collect();
    sections.insert(slot::WARP_KEYFORM_POSITION_BEGINS, i32_bytes(&warp_begins));

    // Art meshes.
    sections.insert(slot::MESH_IDS, id_bytes(&["MeshQuad", "MeshMask"]));
    sections.insert(slot::MESH_BINDING_INDICES, i32_bytes(&[2, 3]));
    sections.insert(slot::MESH_BEGIN_INDICES, i32_bytes(&[0, 9]));
    sections.insert(slot::MESH_COUNTS, i32_bytes(&[9, 3]));
    sections.insert(slot::MESH_VISIBLE, i32_bytes(&[1, 1]));
    sections.insert(slot::MESH_ENABLED, i32_bytes(&[1, 1]));
    sections.insert(slot::MESH_PARENT_PART_INDICES, i32_bytes(&[1, 1]));
    sections.insert(slot::MESH_PARENT_DEFORMER_INDICES, i32_bytes(&[1, 1]));
    sections.insert(slot::MESH_TEXTURE_NUMBERS, i32_bytes(&[0, 0]));
    let quad_flag: u8 = if options.invert_mask { 0x08 } else { 0x00 };
    sections.insert(slot::MESH_DRAWABLE_FLAGS, vec![quad_flag, 0]);
    sections.insert(slot::MESH_VERTEX_COUNTS, i32_bytes(&[4, 4]));
    sections.insert(slot::MESH_UV_BEGIN_INDICES, i32_bytes(&[0, 8]));
    sections.insert(slot::MESH_POS_INDEX_BEGIN_INDICES, i32_bytes(&[0, 6]));
    sections.insert(slot::MESH_POS_INDEX_COUNTS, i32_bytes(&[6, 6]));
    sections.insert(slot::MESH_MASK_BEGIN_INDICES, i32_bytes(&[0, 0]));
    let (mask_counts, masks): ([i32; 2], Vec<i32>) = if options.mesh_self_mask {
        ([1, 0], vec![0])
    } else if options.mask_cycle {
        ([1, 1], vec![1, 0])
    } else {
        ([1, 0], vec![1])
    };
    sections.insert(slot::MESH_MASK_COUNTS, i32_bytes(&mask_counts));
    sections.insert(slot::DRAWABLE_MASKS, i32_bytes(&masks));
    sections.insert(slot::MESH_KEYFORM_OPACITIES, f32_bytes(&[1.0; 12]));
    let mut mesh_draw_orders = vec![500.0f32; 9];
    mesh_draw_orders.extend_from_slice(&[400.0; 3]);
    sections.insert(slot::MESH_KEYFORM_DRAW_ORDERS, f32_bytes(&mesh_draw_orders));
    let mut mesh_begins: Vec<i32> = (0..9).map(|form| 162 + form * 8).collect();
    mesh_begins.extend((0..3).map(|form| 234 + form * 8));
    sections.insert(slot::MESH_KEYFORM_POSITION_BEGINS, i32_bytes(&mesh_begins));

    // Keyform positions: warp forms (9 * 18), quad forms
    // (9 * 8), mask forms (3 * 8). Form f shifts x by f * 0.01 so
    // every form is distinct and the default form (ordinal 4 for
    // the two-axis objects, 1 for the mask) is identifiable.
    let mut positions: Vec<f32> = Vec::new();
    for form in 0..9 {
        for gy in 0..3 {
            for gx in 0..3 {
                positions.push(gx as f32 - 1.0 + form as f32 * 0.01);
                positions.push(gy as f32 - 1.0);
            }
        }
    }
    for form in 0..9 {
        for (x, y) in [(-0.5f32, -0.5f32), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)] {
            positions.push(x + form as f32 * 0.01);
            positions.push(y);
        }
    }
    for form in 0..3 {
        for (x, y) in [(0.6f32, 0.6f32), (1.0, 0.6), (1.0, 1.0), (0.6, 1.0)] {
            positions.push(x + form as f32 * 0.01);
            positions.push(y);
        }
    }
    assert_eq!(positions.len(), 258);
    sections.insert(slot::KEYFORM_POSITIONS, f32_bytes(&positions));

    // UVs and position indices (stored as u16 bit patterns).
    let mut uvs: Vec<f32> = Vec::new();
    for _ in 0..2 {
        uvs.extend_from_slice(&[0.0, 0.0, 1.0, 0.0, 1.0, 1.0, 0.0, 1.0]);
    }
    sections.insert(slot::UVS, f32_bytes(&uvs));
    let indices: [u16; 12] = [0, 1, 2, 0, 2, 3, 0, 1, 2, 0, 2, 3];
    let index_bytes: Vec<u8> = indices
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect();
    sections.insert(slot::POSITION_INDICES, index_bytes);

    // Draw order: one group over both meshes.
    sections.insert(slot::DRAW_ORDER_GROUP_OBJECT_BEGINS, i32_bytes(&[0]));
    sections.insert(slot::DRAW_ORDER_GROUP_OBJECT_COUNTS, i32_bytes(&[2]));
    sections.insert(slot::DRAW_ORDER_GROUP_OBJECT_TOTALS, i32_bytes(&[2]));
    sections.insert(slot::DRAW_ORDER_GROUP_MAXIMUMS, f32_bytes(&[1000.0]));
    sections.insert(slot::DRAW_ORDER_GROUP_MINIMUMS, f32_bytes(&[0.0]));
    sections.insert(slot::DRAW_ORDER_OBJECT_TYPES, i32_bytes(&[0, 0]));
    sections.insert(slot::DRAW_ORDER_OBJECT_INDICES, i32_bytes(&[0, 1]));
    sections.insert(slot::DRAW_ORDER_OBJECT_SELF_INDICES, i32_bytes(&[0, 1]));

    // Assemble: header, offset table, sections in slot order.
    let header_len = 0x40 + 160 * 4;
    let mut sot = [0i32; 160];
    let mut cursor = header_len;
    for (slot_index, bytes) in &sections {
        sot[*slot_index] = cursor as i32;
        cursor += bytes.len();
    }
    let mut out = Vec::with_capacity(cursor);
    out.extend_from_slice(b"MOC3");
    out.push(options.version.unwrap_or(1));
    out.push(0); // little-endian marker
    out.resize(0x40, 0);
    out.extend_from_slice(&i32_bytes(&sot));
    for bytes in sections.values() {
        out.extend_from_slice(bytes);
    }
    assert_eq!(out.len(), cursor);
    out
}

/// The model3 manifest for the fixture. With `duplicate_motion`,
/// the same motion file is referenced from two groups (a duplicate
/// native motion id).
pub fn fixture_model3(duplicate_motion: bool) -> String {
    let tap = if duplicate_motion {
        r#", "Tap": [{ "File": "motions/idle.motion3.json" }]"#
    } else {
        ""
    };
    format!(
        r#"{{
  "Version": 3,
  "FileReferences": {{
    "Moc": "fixture.moc3",
    "Textures": ["fixture.1024/texture_00.png"],
    "Physics": "fixture.physics3.json",
    "DisplayInfo": "fixture.cdi3.json",
    "Expressions": [{{ "Name": "Smile", "File": "expressions/smile.exp3.json" }}],
    "Motions": {{ "Idle": [{{ "File": "motions/idle.motion3.json", "FadeInTime": 0.25, "FadeOutTime": 0.5 }}]{tap} }}
  }},
  "Groups": [{{ "Target": "Parameter", "Name": "EyeBlink", "Ids": ["ParamBlink"] }}],
  "HitAreas": [{{ "Id": "Head", "Name": "Head" }}]
}}"#
    )
}

pub const FIXTURE_CDI3: &str = r#"{
  "Version": 3,
  "Parameters": [
    { "Id": "ParamAngleX", "GroupId": "Body", "Name": "Angle X" },
    { "Id": "ParamAngleY", "GroupId": "Body", "Name": "Angle Y" },
    { "Id": "ParamBlink", "GroupId": "Face", "Name": "Blink" }
  ],
  "ParameterGroups": [
    { "Id": "Body", "Name": "Body" },
    { "Id": "Face", "Name": "Face" }
  ],
  "Parts": [
    { "Id": "PartRoot", "Name": "Root" },
    { "Id": "PartChild", "Name": "Child" }
  ]
}"#;

pub const FIXTURE_PHYSICS3: &str = r#"{
  "Version": 3,
  "Meta": {
    "PhysicsSettingCount": 1,
    "TotalInputCount": 1,
    "TotalOutputCount": 1,
    "VertexCount": 3,
    "EffectiveForces": {
      "Gravity": { "X": 0, "Y": -1 },
      "Wind": { "X": 0, "Y": 0 }
    },
    "PhysicsDictionary": [{ "Id": "PhysicsSetting1", "Name": "Hair" }]
  },
  "PhysicsSettings": [
    {
      "Id": "PhysicsSetting1",
      "Input": [
        { "Source": { "Target": "Parameter", "Id": "ParamAngleX" }, "Weight": 50, "Type": "X", "Reflect": false }
      ],
      "Output": [
        { "Destination": { "Target": "Parameter", "Id": "ParamAngleY" }, "VertexIndex": 2, "Scale": 10, "Weight": 100, "Type": "Angle", "Reflect": false }
      ],
      "Vertices": [
        { "Position": { "X": 0, "Y": 0 }, "Mobility": 1.0, "Delay": 1.0, "Acceleration": 1.0, "Radius": 0 },
        { "Position": { "X": 0, "Y": 10 }, "Mobility": 0.9, "Delay": 0.8, "Acceleration": 1.0, "Radius": 5 },
        { "Position": { "X": 0, "Y": 20 }, "Mobility": 0.8, "Delay": 0.7, "Acceleration": 1.0, "Radius": 5 }
      ],
      "Normalization": {
        "Position": { "Minimum": -30, "Default": 0, "Maximum": 30 },
        "Angle": { "Minimum": -10, "Default": 0, "Maximum": 10 }
      }
    }
  ]
}"#;

/// The fixture motion. `end_angle` is the bezier curve's final
/// ParamAngleX value (30 is in range; anything beyond is the
/// adversarial out-of-range case).
pub fn fixture_motion3(end_angle: f32) -> String {
    format!(
        r#"{{
  "Version": 3,
  "Meta": {{
    "Duration": 1.0,
    "Fps": 30.0,
    "Loop": true,
    "AreBeziersRestricted": true,
    "CurveCount": 2,
    "TotalSegmentCount": 2,
    "TotalPointCount": 4,
    "UserDataCount": 0,
    "TotalUserDataSize": 0
  }},
  "Curves": [
    {{
      "Target": "Parameter",
      "Id": "ParamAngleX",
      "Segments": [0.0, 0.0, 1.0, 0.25, 5.0, 0.75, 25.0, 1.0, {end_angle}]
    }},
    {{
      "Target": "Parameter",
      "Id": "ParamBlink",
      "Segments": [0.0, 1.0, 2.0, 1.0, 0.0]
    }}
  ],
  "UserData": []
}}"#
    )
}

pub const FIXTURE_EXP3: &str = r#"{
  "Type": "Live2D Expression",
  "Parameters": [
    { "Id": "ParamAngleX", "Value": 15.0, "Blend": "Add" },
    { "Id": "ParamBlink", "Value": 0.0, "Blend": "Overwrite" }
  ]
}"#;

pub const LICENSE_OK: &str = r#"{
  "source": "synthetic test fixture",
  "license_name": "CC0-1.0",
  "license_url": "https://creativecommons.org/publicdomain/zero/1.0/",
  "game_use": true,
  "conversion_allowed": true,
  "recorded_by": "lumen-rig-import tests",
  "date": "2026-10-26"
}"#;

/// A unique temporary directory, removed on drop.
pub struct TestDir {
    pub root: PathBuf,
}

static COUNTER: AtomicUsize = AtomicUsize::new(0);

impl TestDir {
    pub fn new(tag: &str) -> TestDir {
        let serial = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!(
            "lumen-rig-import-test-{}-{tag}-{serial}",
            std::process::id()
        ));
        fs::create_dir_all(&root).expect("create temp dir");
        TestDir { root }
    }

    /// Write a complete fixture model directory plus a license
    /// record; returns (model_dir, license_path, out_dir).
    pub fn write_model(
        &self,
        moc3: &[u8],
        model3: &str,
        motion3: &str,
        license: &str,
    ) -> (PathBuf, PathBuf, PathBuf) {
        let model_dir = self.root.join("model");
        fs::create_dir_all(model_dir.join("motions")).expect("motions dir");
        fs::create_dir_all(model_dir.join("expressions")).expect("expressions dir");
        fs::write(model_dir.join("fixture.moc3"), moc3).expect("moc3");
        fs::write(model_dir.join("fixture.model3.json"), model3).expect("model3");
        fs::write(model_dir.join("fixture.cdi3.json"), FIXTURE_CDI3).expect("cdi3");
        fs::write(model_dir.join("fixture.physics3.json"), FIXTURE_PHYSICS3).expect("physics3");
        fs::write(model_dir.join("motions/idle.motion3.json"), motion3).expect("motion3");
        fs::write(model_dir.join("expressions/smile.exp3.json"), FIXTURE_EXP3).expect("exp3");
        let license_path = self.root.join("license.json");
        fs::write(&license_path, license).expect("license");
        (model_dir, license_path, self.root.join("out"))
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

/// Read every file under `dir` as (relative path, bytes), sorted.
pub fn read_tree(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    collect_tree(dir, dir, &mut out);
    out.sort();
    out
}

fn collect_tree(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_tree(root, &path, out);
        } else if let Ok(relative) = path.strip_prefix(root) {
            let bytes = fs::read(&path).expect("read tree file");
            out.push((relative.to_string_lossy().replace('\\', "/"), bytes));
        }
    }
}
