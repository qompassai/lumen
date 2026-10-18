//! Structural export validator (Phase 4): the "validated export" gate.
//!
//! Contract summary:
//! - [`validate_bytes`] inspects a GLB/VRM file purely from its bytes
//!   and returns a [`ValidationReport`]: a list of named checks, each
//!   with a pass flag and a human-readable detail, plus the structural
//!   counts and the scene bounding box. It never panics and never
//!   errors out early: a broken file produces a report whose failing
//!   checks name exactly what is wrong (fail-closed error list).
//! - POSITION `min`/`max` are recomputed from the decoded accessor
//!   data and compared exactly (bit-for-bit after the JSON round
//!   trip, which is exact for the f32 values this writer emits; no
//!   tolerance is applied, and none was needed on any factory asset).
//! - The scene height bound `[1.0, 2.5]` meters is a stylized
//!   factory bound: the canonical base is 1.70 m and identity
//!   projection scales bands within `[0.5, 2.0]`, so a factory
//!   character outside that band is structurally implausible, not a
//!   legitimate variant.
//! - When a VRM extension is present the report reuses
//!   [`crate::vrm::inspect_vrm`] and records the flavor and humanoid
//!   bone count.

#![forbid(unsafe_code)]

use serde::Serialize;
use serde_json::Value;

use crate::MAX_GLB_BYTES;
use crate::vrm::inspect_vrm;

/// Report schema identifier emitted in every validation report.
pub const VALIDATION_SCHEMA: &str = "lumen.factory-validation";
/// Report schema version.
pub const VALIDATION_VERSION: u32 = 1;
/// Skin weights must sum to 1.0 within this tolerance.
pub const WEIGHT_SUM_TOLERANCE: f32 = 1e-3;
/// Stylized scene-height bounds in meters (see module docs).
pub const HEIGHT_MIN_M: f32 = 1.0;
/// Stylized scene-height bounds in meters (see module docs).
pub const HEIGHT_MAX_M: f32 = 2.5;
/// Largest sane texture dimension in pixels (header check only).
pub const MAX_TEXTURE_DIM: u32 = 8192;

const GLB_MAGIC: &[u8; 4] = b"glTF";
const GLB_VERSION: u32 = 2;
const CHUNK_JSON: u32 = 0x4E4F_534A;
const CHUNK_BIN: u32 = 0x004E_4942;

/// One named validation check.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ValidationCheck {
    /// Human-readable detail (measurements on success, the exact
    /// defect on failure).
    pub detail: String,
    /// Stable rule name, e.g. `skin_weight_sums`.
    pub name: String,
    /// Whether the rule held.
    pub pass: bool,
}

/// Structural counts for a validated file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize)]
pub struct ValidationCounts {
    /// Number of embedded images.
    pub images: usize,
    /// Number of joints in the first skin (0 when unskinned).
    pub joints: usize,
    /// Number of meshes.
    pub meshes: usize,
    /// Number of nodes.
    pub nodes: usize,
    /// Number of primitives across all meshes.
    pub primitives: usize,
    /// Number of morph targets across all primitives.
    pub targets: usize,
}

/// Scene bounding box aggregated from primitive POSITION data.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct ValidationBBox {
    /// Box height in meters (`max[1] - min[1]`).
    pub height: f32,
    /// Maximum corner `[x, y, z]`.
    pub max: [f32; 3],
    /// Minimum corner `[x, y, z]`.
    pub min: [f32; 3],
}

/// VRM information reported when a VRM extension is present.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ValidationVrm {
    /// Humanoid bone count.
    pub bones: usize,
    /// `vrm-1.0` or `vrm-0.x`.
    pub flavor: String,
}

/// The full validation report. Serializes to the
/// `lumen.factory-validation` v1 JSON schema.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ValidationReport {
    /// Scene bounding box, when any POSITION data could be read.
    pub bbox: Option<ValidationBBox>,
    /// Every named check that ran, in rule order.
    pub checks: Vec<ValidationCheck>,
    /// Structural counts.
    pub counts: ValidationCounts,
    /// File identifier as supplied by the caller.
    pub file: String,
    /// Overall verdict: every check passed.
    pub pass: bool,
    /// Schema identifier, always [`VALIDATION_SCHEMA`].
    pub schema: String,
    /// File size in bytes.
    pub size: usize,
    /// Schema version, always [`VALIDATION_VERSION`].
    pub version: u32,
    /// VRM flavor/bone info when a VRM extension is present.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vrm: Option<ValidationVrm>,
}

fn read_u32(bytes: &[u8], offset: usize) -> Option<u32> {
    let slice = bytes.get(offset..offset + 4)?;
    Some(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

fn get_array<'a>(root: &'a Value, key: &str) -> &'a [Value] {
    root.get(key)
        .and_then(Value::as_array)
        .map_or(&[], Vec::as_slice)
}

fn get_u64(value: &Value, key: &str) -> Option<u64> {
    value.get(key).and_then(Value::as_u64)
}

fn component_size(component_type: u64) -> Option<usize> {
    match component_type {
        5120 | 5121 => Some(1),
        5122 | 5123 => Some(2),
        5125 | 5126 => Some(4),
        _ => None,
    }
}

fn component_count(accessor_type: &str) -> Option<usize> {
    match accessor_type {
        "SCALAR" => Some(1),
        "VEC2" => Some(2),
        "VEC3" => Some(3),
        "VEC4" => Some(4),
        "MAT2" => Some(4),
        "MAT3" => Some(9),
        "MAT4" => Some(16),
        _ => None,
    }
}

/// One accessor's data, assembled contiguously (byteStride honored).
struct AccessorData {
    component_type: u64,
    elements: Vec<u8>,
}

fn accessor_data(root: &Value, bin: &[u8], index: usize) -> Result<AccessorData, String> {
    let accessors = get_array(root, "accessors");
    let accessor = accessors
        .get(index)
        .ok_or_else(|| format!("accessor {index} out of range"))?;
    let component_type = get_u64(accessor, "componentType")
        .ok_or_else(|| format!("accessor {index} missing componentType"))?;
    let comp_size = component_size(component_type)
        .ok_or_else(|| format!("accessor {index} bad componentType {component_type}"))?;
    let accessor_type = accessor
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("accessor {index} missing type"))?;
    let num = component_count(accessor_type)
        .ok_or_else(|| format!("accessor {index} bad type {accessor_type}"))?;
    let count = get_u64(accessor, "count")
        .ok_or_else(|| format!("accessor {index} missing count"))? as usize;
    let view_index = get_u64(accessor, "bufferView")
        .ok_or_else(|| format!("accessor {index} missing bufferView"))?
        as usize;
    let views = get_array(root, "bufferViews");
    let view = views
        .get(view_index)
        .ok_or_else(|| format!("accessor {index} bufferView {view_index} out of range"))?;
    let view_offset = get_u64(view, "byteOffset").unwrap_or(0) as usize;
    let view_length = get_u64(view, "byteLength")
        .ok_or_else(|| format!("bufferView {view_index} missing byteLength"))?
        as usize;
    let element_size = comp_size * num;
    let accessor_offset = get_u64(accessor, "byteOffset").unwrap_or(0) as usize;
    let stride = get_u64(view, "byteStride").map(|s| s as usize);
    if let Some(stride) = stride
        && stride < element_size
    {
        return Err(format!(
            "bufferView {view_index} byteStride {stride} < element size {element_size}"
        ));
    }
    let step = stride.unwrap_or(element_size);
    // Bounds: the last element must fit inside the bufferView, and the
    // bufferView itself must fit inside the BIN chunk.
    if count > 0 {
        let needed = accessor_offset
            .checked_add((count - 1).checked_mul(step).ok_or("count overflow")?)
            .and_then(|v| v.checked_add(element_size))
            .ok_or("offset overflow")?;
        if needed > view_length {
            return Err(format!(
                "accessor {index} needs {needed} bytes, bufferView {view_index} has {view_length}"
            ));
        }
    }
    let bin_start = view_offset
        .checked_add(accessor_offset)
        .ok_or("offset overflow")?;
    if bin_start > bin.len() {
        return Err(format!("accessor {index} starts past BIN chunk"));
    }
    let mut elements = Vec::with_capacity(count.saturating_mul(element_size));
    for i in 0..count {
        let start = bin_start + i * step;
        let slice = bin
            .get(start..start + element_size)
            .ok_or_else(|| format!("accessor {index} element {i} overruns BIN"))?;
        elements.extend_from_slice(slice);
    }
    Ok(AccessorData {
        component_type,
        elements,
    })
}

fn accessor_f32(root: &Value, bin: &[u8], index: usize) -> Result<Vec<f32>, String> {
    let data = accessor_data(root, bin, index)?;
    if data.component_type != 5126 {
        return Err(format!(
            "accessor {index} componentType {} is not float",
            data.component_type
        ));
    }
    let mut out = Vec::with_capacity(data.elements.len() / 4);
    for chunk in data.elements.as_chunks::<4>().0 {
        out.push(f32::from_le_bytes(*chunk));
    }
    Ok(out)
}

fn accessor_ints(root: &Value, bin: &[u8], index: usize) -> Result<Vec<u64>, String> {
    let data = accessor_data(root, bin, index)?;
    let mut out = Vec::new();
    match data.component_type {
        5121 => {
            for byte in &data.elements {
                out.push(u64::from(*byte));
            }
        }
        5123 => {
            for chunk in data.elements.as_chunks::<2>().0 {
                out.push(u64::from(u16::from_le_bytes(*chunk)));
            }
        }
        5125 => {
            for chunk in data.elements.as_chunks::<4>().0 {
                out.push(u64::from(u32::from_le_bytes(*chunk)));
            }
        }
        other => {
            return Err(format!(
                "accessor {index} componentType {other} is not an integer type"
            ));
        }
    }
    Ok(out)
}

fn primitive_accessor(primitive: &Value, attribute: &str) -> Option<usize> {
    primitive
        .get("attributes")
        .and_then(|attrs| attrs.get(attribute))
        .and_then(Value::as_u64)
        .map(|v| v as usize)
}

struct CheckBuilder {
    checks: Vec<ValidationCheck>,
}

impl CheckBuilder {
    fn new() -> Self {
        Self { checks: Vec::new() }
    }
    fn push(&mut self, name: &str, pass: bool, detail: String) {
        self.checks.push(ValidationCheck {
            detail,
            name: name.to_string(),
            pass,
        });
    }
    fn push_result(&mut self, name: &str, result: Result<String, String>) {
        match result {
            Ok(detail) => self.push(name, true, detail),
            Err(detail) => self.push(name, false, detail),
        }
    }
}

/// Validate GLB/VRM `bytes` (identified as `file` in the report) and
/// return the full report. Never panics, never fails early: every
/// rule that can run does run, and rules that cannot run because an
/// earlier layer is broken are reported as failed with the reason.
pub fn validate_bytes(bytes: &[u8], file: &str) -> ValidationReport {
    let mut builder = CheckBuilder::new();
    let mut counts = ValidationCounts::default();
    let mut bbox: Option<ValidationBBox> = None;
    let mut vrm: Option<ValidationVrm> = None;

    // --- container layer -------------------------------------------------
    if bytes.len() > MAX_GLB_BYTES {
        builder.push(
            "container_chunk_layout",
            false,
            format!("file size {} exceeds bound {MAX_GLB_BYTES}", bytes.len()),
        );
    }
    let magic_ok = bytes.get(0..4) == Some(GLB_MAGIC.as_slice());
    builder.push(
        "container_magic",
        magic_ok,
        if magic_ok {
            "glTF magic present".to_string()
        } else {
            "missing or wrong glTF magic".to_string()
        },
    );
    let version = read_u32(bytes, 4);
    builder.push(
        "container_version",
        version == Some(GLB_VERSION),
        format!("version {version:?}, expected {GLB_VERSION}"),
    );

    let mut json_value: Option<Value> = None;
    let mut bin: Vec<u8> = Vec::new();
    let mut layout_detail: Result<String, String> = Err("file shorter than GLB header".to_string());
    let mut json_parse: Result<String, String> = Err("no JSON chunk located".to_string());
    if bytes.len() >= 20 {
        let declared = read_u32(bytes, 8).map(|v| v as usize);
        let json_len = read_u32(bytes, 12).map(|v| v as usize);
        let json_type = read_u32(bytes, 16);
        if declared != Some(bytes.len()) {
            layout_detail = Err(format!(
                "declared length {declared:?} != actual {}",
                bytes.len()
            ));
        } else if json_type != Some(CHUNK_JSON) {
            layout_detail = Err("first chunk is not JSON".to_string());
        } else if let Some(json_len) = json_len {
            if json_len % 4 != 0 {
                layout_detail = Err(format!("JSON chunk length {json_len} not 4-aligned"));
            } else if 20usize
                .checked_add(json_len)
                .is_none_or(|end| end > bytes.len())
            {
                layout_detail = Err("JSON chunk overruns file".to_string());
            } else {
                let json_end = 20 + json_len;
                let json_slice = &bytes[20..json_end];
                match serde_json::from_slice::<Value>(json_slice) {
                    Ok(value) => {
                        json_value = Some(value);
                        json_parse = Ok(format!("JSON chunk parses ({json_len} bytes)"));
                    }
                    Err(error) => {
                        json_parse = Err(format!("bad glTF JSON: {error}"));
                    }
                }
                // BIN chunk layout.
                if json_end == bytes.len() {
                    layout_detail = Ok("JSON chunk only, no BIN chunk".to_string());
                } else if json_end + 8 > bytes.len() {
                    layout_detail = Err("truncated BIN chunk header".to_string());
                } else {
                    let bin_len = read_u32(bytes, json_end).map(|v| v as usize);
                    let bin_type = read_u32(bytes, json_end + 4);
                    match (bin_len, bin_type) {
                        (Some(bin_len), Some(CHUNK_BIN)) => {
                            if bin_len % 4 != 0 {
                                layout_detail =
                                    Err(format!("BIN chunk length {bin_len} not 4-aligned"));
                            } else if json_end + 8 + bin_len != bytes.len() {
                                layout_detail = Err(format!(
                                    "BIN chunk ends at {}, file is {} bytes",
                                    json_end + 8 + bin_len,
                                    bytes.len()
                                ));
                            } else {
                                bin = bytes[json_end + 8..json_end + 8 + bin_len].to_vec();
                                layout_detail = Ok(format!(
                                    "JSON {json_len} bytes + BIN {bin_len} bytes fill the file"
                                ));
                            }
                        }
                        _ => {
                            layout_detail = Err("second chunk is not BIN".to_string());
                        }
                    }
                }
            }
        }
    }
    builder.push_result("container_chunk_layout", layout_detail);
    builder.push_result("container_json_parses", json_parse);

    let Some(root) = json_value else {
        // Without a document no deeper rule can run; report them all
        // as failed rather than silently omitting them.
        for name in [
            "container_single_buffer_no_uri",
            "buffer_views_in_bounds",
            "accessors_component_types",
            "accessors_counts_consistent",
            "position_min_max_exact",
            "skin_ibm_count_matches_joints",
            "skin_weight_sums",
            "skin_joint_indices_in_range",
            "morph_target_names_match",
            "morph_target_position_counts",
            "scene_bbox_height",
            "no_nan_float_accessors",
            "texture_png_mime_and_dims",
            "vrm_extension",
        ] {
            builder.push(name, false, "skipped: no parsed glTF JSON".to_string());
        }
        return finish(builder, counts, bbox, vrm, file, bytes.len());
    };

    // --- counts ------------------------------------------------------------
    let nodes = get_array(&root, "nodes");
    let meshes = get_array(&root, "meshes");
    let skins = get_array(&root, "skins");
    let accessors = get_array(&root, "accessors");
    let buffer_views = get_array(&root, "bufferViews");
    let buffers = get_array(&root, "buffers");
    let images = get_array(&root, "images");
    counts.nodes = nodes.len();
    counts.meshes = meshes.len();
    counts.images = images.len();
    counts.joints = skins
        .first()
        .and_then(|skin| skin.get("joints"))
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    for mesh in meshes {
        for primitive in get_array(mesh, "primitives") {
            counts.primitives += 1;
            if let Some(targets) = primitive.get("targets").and_then(Value::as_array) {
                counts.targets += targets.len();
            }
        }
    }

    // --- buffer ------------------------------------------------------------
    builder.push_result("container_single_buffer_no_uri", {
        if buffers.len() != 1 {
            Err(format!("{} buffers, expected exactly 1", buffers.len()))
        } else if let Some(uri) = buffers[0].get("uri") {
            Err(format!(
                "buffer carries a uri ({uri}); data-uri / external buffers are not factory exports"
            ))
        } else {
            match get_u64(&buffers[0], "byteLength") {
                Some(byte_length) => {
                    let byte_length = byte_length as usize;
                    // The BIN chunk may carry up to 3 padding bytes
                    // past the declared buffer length (4-alignment).
                    if byte_length <= bin.len() && bin.len() - byte_length < 4 {
                        Ok(format!(
                            "single unnamed buffer, byteLength {byte_length}, BIN {} bytes",
                            bin.len()
                        ))
                    } else {
                        Err(format!(
                            "buffer byteLength {byte_length} inconsistent with BIN {} bytes",
                            bin.len()
                        ))
                    }
                }
                None => Err("buffer missing byteLength".to_string()),
            }
        }
    });

    builder.push_result("buffer_views_in_bounds", {
        let mut result = Ok(format!(
            "{} bufferViews within BIN bounds",
            buffer_views.len()
        ));
        for (index, view) in buffer_views.iter().enumerate() {
            let buffer_index = get_u64(view, "buffer").unwrap_or(0) as usize;
            let offset = get_u64(view, "byteOffset").unwrap_or(0) as usize;
            let length = get_u64(view, "byteLength");
            match length {
                Some(length) => {
                    let length = length as usize;
                    if buffer_index >= buffers.len() {
                        result = Err(format!(
                            "bufferView {index} references buffer {buffer_index} out of range"
                        ));
                        break;
                    }
                    if offset.checked_add(length).is_none_or(|end| end > bin.len()) {
                        result = Err(format!(
                            "bufferView {index} [{offset}, {}) overruns BIN {} bytes",
                            offset + length,
                            bin.len()
                        ));
                        break;
                    }
                }
                None => {
                    result = Err(format!("bufferView {index} missing byteLength"));
                    break;
                }
            }
        }
        result
    });

    // --- accessors ---------------------------------------------------------
    builder.push_result("accessors_component_types", {
        let mut result = Ok(format!("{} accessors with valid types", accessors.len()));
        for (index, accessor) in accessors.iter().enumerate() {
            let ct = get_u64(accessor, "componentType");
            let ty = accessor.get("type").and_then(Value::as_str);
            if ct.is_none_or(|ct| component_size(ct).is_none())
                || ty.is_none_or(|ty| component_count(ty).is_none())
                || get_u64(accessor, "count").is_none()
            {
                result = Err(format!(
                    "accessor {index} invalid componentType {ct:?} / type {ty:?} / count"
                ));
                break;
            }
        }
        result
    });

    builder.push_result("accessors_counts_consistent", {
        let mut result = Ok("every accessor fits its bufferView".to_string());
        for index in 0..accessors.len() {
            if let Err(error) = accessor_data(&root, &bin, index) {
                result = Err(error);
                break;
            }
        }
        result
    });

    builder.push_result("position_min_max_exact", {
        let mut checked = 0usize;
        let mut result = Ok(String::new());
        for (index, accessor) in accessors.iter().enumerate() {
            let (Some(min), Some(max)) = (accessor.get("min"), accessor.get("max")) else {
                continue;
            };
            let ty = accessor.get("type").and_then(Value::as_str).unwrap_or("");
            if ty != "VEC3" || get_u64(accessor, "componentType") != Some(5126) {
                continue;
            }
            match accessor_f32(&root, &bin, index) {
                Ok(values) => {
                    let mut rmin = [f32::INFINITY; 3];
                    let mut rmax = [f32::NEG_INFINITY; 3];
                    for chunk in values.as_chunks::<3>().0 {
                        for axis in 0..3 {
                            // f32::min/max propagate NaN per IEEE
                            // minNum semantics in Rust: they return the
                            // non-NaN operand, so a NaN is caught by the
                            // dedicated no-NaN rule, not hidden here.
                            rmin[axis] = rmin[axis].min(chunk[axis]);
                            rmax[axis] = rmax[axis].max(chunk[axis]);
                        }
                    }
                    let declared_min: Vec<f32> = min
                        .as_array()
                        .map(|a| a.iter().filter_map(Value::as_f64).map(|v| v as f32).collect())
                        .unwrap_or_default();
                    let declared_max: Vec<f32> = max
                        .as_array()
                        .map(|a| a.iter().filter_map(Value::as_f64).map(|v| v as f32).collect())
                        .unwrap_or_default();
                    if declared_min.len() != 3
                        || declared_max.len() != 3
                        || declared_min[..] != rmin[..]
                        || declared_max[..] != rmax[..]
                    {
                        result = Err(format!(
                            "accessor {index} declared min/max {declared_min:?}/{declared_max:?} != recomputed {rmin:?}/{rmax:?} (exact compare)"
                        ));
                        break;
                    }
                    checked += 1;
                }
                Err(error) => {
                    result = Err(error);
                    break;
                }
            }
        }
        if result.is_ok() {
            result = Ok(format!(
                "{checked} VEC3 accessors with declared min/max exactly matching recomputed values"
            ));
        }
        result
    });

    // --- skin --------------------------------------------------------------
    let joint_count = counts.joints;
    builder.push_result("skin_ibm_count_matches_joints", {
        if skins.is_empty() {
            Err("no skin present".to_string())
        } else {
            let mut result = Ok(String::new());
            let mut total_joints = 0usize;
            for (index, skin) in skins.iter().enumerate() {
                let joints = skin
                    .get("joints")
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len);
                total_joints += joints;
                match skin
                    .get("inverseBindMatrices")
                    .and_then(Value::as_u64)
                    .and_then(|ibm| accessors.get(ibm as usize))
                    .and_then(|acc| get_u64(acc, "count"))
                {
                    Some(count) if count as usize == joints => {}
                    other => {
                        result = Err(format!(
                            "skin {index}: inverseBindMatrices count {other:?} != joints {joints}"
                        ));
                        break;
                    }
                }
            }
            if result.is_ok() {
                result = Ok(format!(
                    "{} skin(s), IBM count == joints count ({total_joints} total)",
                    skins.len()
                ));
            }
            result
        }
    });

    builder.push_result("skin_weight_sums", {
        let mut vertices = 0usize;
        let mut result = Ok(String::new());
        'outer: for mesh in meshes {
            for primitive in get_array(mesh, "primitives") {
                let Some(weights_index) = primitive_accessor(primitive, "WEIGHTS_0") else {
                    continue;
                };
                match accessor_f32(&root, &bin, weights_index) {
                    Ok(values) => {
                        for chunk in values.as_chunks::<4>().0 {
                            let sum: f32 = chunk.iter().sum();
                            vertices += 1;
                            if (sum - 1.0).abs() > WEIGHT_SUM_TOLERANCE {
                                result = Err(format!(
                                    "vertex {vertices} weight sum {sum} not within {WEIGHT_SUM_TOLERANCE} of 1.0"
                                ));
                                break 'outer;
                            }
                        }
                    }
                    Err(error) => {
                        result = Err(error);
                        break 'outer;
                    }
                }
            }
        }
        if result.is_ok() {
            if vertices == 0 {
                result = Err("no WEIGHTS_0 data found".to_string());
            } else {
                result = Ok(format!(
                    "{vertices} vertices with weight sums within {WEIGHT_SUM_TOLERANCE} of 1.0"
                ));
            }
        }
        result
    });

    builder.push_result("skin_joint_indices_in_range", {
        let mut indices = 0usize;
        let mut result = Ok(String::new());
        'outer: for mesh in meshes {
            for primitive in get_array(mesh, "primitives") {
                let Some(joints_index) = primitive_accessor(primitive, "JOINTS_0") else {
                    continue;
                };
                match accessor_ints(&root, &bin, joints_index) {
                    Ok(values) => {
                        for value in values {
                            indices += 1;
                            if value as usize >= joint_count {
                                result = Err(format!(
                                    "joint index {value} >= joint count {joint_count}"
                                ));
                                break 'outer;
                            }
                        }
                    }
                    Err(error) => {
                        result = Err(error);
                        break 'outer;
                    }
                }
            }
        }
        if result.is_ok() {
            if indices == 0 {
                result = Err("no JOINTS_0 data found".to_string());
            } else {
                result = Ok(format!(
                    "{indices} joint indices all < joint count {joint_count}"
                ));
            }
        }
        result
    });

    // --- morphs --------------------------------------------------------------
    builder.push_result("morph_target_names_match", {
        let mut with_targets = 0usize;
        let mut result = Ok(String::new());
        for mesh in meshes {
            let names_len = mesh
                .get("extras")
                .and_then(|extras| extras.get("targetNames"))
                .and_then(Value::as_array)
                .map(Vec::len);
            for primitive in get_array(mesh, "primitives") {
                let Some(targets) = primitive.get("targets").and_then(Value::as_array) else {
                    continue;
                };
                with_targets += 1;
                match names_len {
                    Some(len) if len == targets.len() => {}
                    other => {
                        result = Err(format!(
                            "mesh extras.targetNames length {other:?} != primitive targets length {}",
                            targets.len()
                        ));
                        break;
                    }
                }
            }
            if result.is_err() {
                break;
            }
        }
        if result.is_ok() {
            result = Ok(if with_targets == 0 {
                "no morph targets present".to_string()
            } else {
                format!(
                    "{with_targets} primitive(s) whose target count matches extras.targetNames"
                )
            });
        }
        result
    });

    builder.push_result("morph_target_position_counts", {
        let mut checked = 0usize;
        let mut result = Ok(String::new());
        'outer: for mesh in meshes {
            for primitive in get_array(mesh, "primitives") {
                let Some(targets) = primitive.get("targets").and_then(Value::as_array) else {
                    continue;
                };
                let base_count = primitive_accessor(primitive, "POSITION")
                    .and_then(|idx| accessors.get(idx))
                    .and_then(|acc| get_u64(acc, "count"));
                for target in targets {
                    let Some(target_pos) = target.get("POSITION").and_then(Value::as_u64) else {
                        result = Err("morph target without POSITION accessor".to_string());
                        break 'outer;
                    };
                    let target_count = accessors
                        .get(target_pos as usize)
                        .and_then(|acc| get_u64(acc, "count"));
                    if target_count != base_count {
                        result = Err(format!(
                            "morph target POSITION count {target_count:?} != base POSITION count {base_count:?}"
                        ));
                        break 'outer;
                    }
                    checked += 1;
                }
            }
        }
        if result.is_ok() {
            result = Ok(format!(
                "{checked} morph target POSITION accessors match base counts"
            ));
        }
        result
    });

    // --- scene sanity --------------------------------------------------------
    let mut scene_result: Result<String, String> = Ok(String::new());
    {
        let mut gmin = [f32::INFINITY; 3];
        let mut gmax = [f32::NEG_INFINITY; 3];
        let mut saw = false;
        for mesh in meshes {
            for primitive in get_array(mesh, "primitives") {
                let Some(pos_index) = primitive_accessor(primitive, "POSITION") else {
                    continue;
                };
                match accessor_f32(&root, &bin, pos_index) {
                    Ok(values) => {
                        saw = true;
                        for chunk in values.as_chunks::<3>().0 {
                            for axis in 0..3 {
                                gmin[axis] = gmin[axis].min(chunk[axis]);
                                gmax[axis] = gmax[axis].max(chunk[axis]);
                            }
                        }
                    }
                    Err(error) => {
                        scene_result = Err(error);
                        break;
                    }
                }
            }
            if scene_result.is_err() {
                break;
            }
        }
        if scene_result.is_ok() {
            if !saw {
                scene_result = Err("no POSITION data found".to_string());
            } else {
                let height = gmax[1] - gmin[1];
                bbox = Some(ValidationBBox {
                    height,
                    max: gmax,
                    min: gmin,
                });
                if (HEIGHT_MIN_M..=HEIGHT_MAX_M).contains(&height) {
                    scene_result = Ok(format!(
                        "scene height {height:.4} m within [{HEIGHT_MIN_M}, {HEIGHT_MAX_M}] (stylized bounds)"
                    ));
                } else {
                    scene_result = Err(format!(
                        "scene height {height:.4} m outside [{HEIGHT_MIN_M}, {HEIGHT_MAX_M}] (stylized bounds)"
                    ));
                }
            }
        }
    }
    builder.push_result("scene_bbox_height", scene_result);

    builder.push_result("no_nan_float_accessors", {
        let mut values_checked = 0usize;
        let mut result = Ok(String::new());
        for (index, accessor) in accessors.iter().enumerate() {
            if get_u64(accessor, "componentType") != Some(5126) {
                continue;
            }
            match accessor_f32(&root, &bin, index) {
                Ok(values) => {
                    values_checked += values.len();
                    if let Some(bad) = values.iter().position(|v| !v.is_finite()) {
                        result = Err(format!(
                            "accessor {index} value {bad} is not finite (NaN/Inf)"
                        ));
                        break;
                    }
                }
                Err(error) => {
                    result = Err(error);
                    break;
                }
            }
        }
        if result.is_ok() {
            result = Ok(format!("{values_checked} float values, none NaN/Inf"));
        }
        result
    });

    // --- textures --------------------------------------------------------------
    builder.push_result("texture_png_mime_and_dims", {
        if images.is_empty() {
            Ok("no images present".to_string())
        } else {
            let mut result = Ok(String::new());
            for (index, image) in images.iter().enumerate() {
                if image.get("mimeType").and_then(Value::as_str) != Some("image/png") {
                    result = Err(format!("image {index} mimeType is not image/png"));
                    break;
                }
                let Some(view_index) = get_u64(image, "bufferView").map(|v| v as usize) else {
                    result = Err(format!("image {index} missing bufferView"));
                    break;
                };
                let Some(view) = buffer_views.get(view_index) else {
                    result = Err(format!("image {index} bufferView out of range"));
                    break;
                };
                let offset = get_u64(view, "byteOffset").unwrap_or(0) as usize;
                let length = get_u64(view, "byteLength").unwrap_or(0) as usize;
                let Some(slice) = bin.get(offset..offset + length) else {
                    result = Err(format!("image {index} bytes overrun BIN"));
                    break;
                };
                match crate::png_dimensions(slice) {
                    Ok((width, height))
                        if width >= 1
                            && height >= 1
                            && width <= MAX_TEXTURE_DIM
                            && height <= MAX_TEXTURE_DIM =>
                    {
                        result = Ok(format!("image {index}: PNG {width}x{height}"));
                    }
                    Ok((width, height)) => {
                        result = Err(format!(
                            "image {index} PNG dims {width}x{height} not sane (1..={MAX_TEXTURE_DIM})"
                        ));
                        break;
                    }
                    Err(error) => {
                        result = Err(format!("image {index} does not decode as PNG: {error}"));
                        break;
                    }
                }
            }
            result
        }
    });

    // --- VRM ---------------------------------------------------------------------
    let has_vrm = root
        .get("extensions")
        .is_some_and(|ext| ext.get("VRMC_vrm").is_some() || ext.get("VRM").is_some());
    if has_vrm {
        match inspect_vrm(bytes) {
            Ok(summary) => {
                let bones = summary.humanoid_bones.len();
                builder.push(
                    "vrm_extension",
                    true,
                    format!("VRM flavor {}, {bones} humanoid bones", summary.flavor),
                );
                vrm = Some(ValidationVrm {
                    bones,
                    flavor: summary.flavor,
                });
            }
            Err(error) => {
                builder.push(
                    "vrm_extension",
                    false,
                    format!("VRM extension present but inspect failed: {error}"),
                );
            }
        }
    } else {
        builder.push(
            "vrm_extension",
            true,
            "no VRM extension (plain GLB)".to_string(),
        );
    }

    finish(builder, counts, bbox, vrm, file, bytes.len())
}

fn finish(
    builder: CheckBuilder,
    counts: ValidationCounts,
    bbox: Option<ValidationBBox>,
    vrm: Option<ValidationVrm>,
    file: &str,
    size: usize,
) -> ValidationReport {
    let pass = builder.checks.iter().all(|check| check.pass);
    ValidationReport {
        bbox,
        checks: builder.checks,
        counts,
        file: file.to_string(),
        pass,
        schema: VALIDATION_SCHEMA.to_string(),
        size,
        version: VALIDATION_VERSION,
        vrm,
    }
}

/// Validate a file on disk and return its report. A read failure is a
/// typed error; a structurally broken file is a report with
/// `pass == false`, not an error.
pub fn validate_file(path: &std::path::Path) -> Result<ValidationReport, crate::FactoryError> {
    let bytes = std::fs::read(path)?;
    Ok(validate_bytes(&bytes, &path.display().to_string()))
}
