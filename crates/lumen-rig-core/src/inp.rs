//! The INP1 importer (SPEC.md section 13.10): INP1 bytes in,
//! native rig-format data out — a validated [`Model`] (the
//! puppet's scalar parameters and part tree), a validated
//! [`PuppetStage`] (everything else), and the payload
//! textures as files-to-be. The runtime never parses INP1;
//! this is the only INP1 reader in the crate.
//!
//! The importer is loud by law: anything it cannot map is a
//! typed [`ImportError`], never a silent drop. Where the
//! reference runtime is lenient (unknown blend modes become
//! Normal, unparseable bindings vanish, extra texture slots
//! are ignored), the importer refuses instead, because a
//! dropped binding is a changed puppet. The two documented
//! exceptions are data the reference runtime itself never
//! evaluates: binding `isSet` grids (authoring metadata) and
//! EXT_SECT vendor payloads (application settings); both are
//! counted in the import report.

use std::collections::{HashMap, HashSet};
use std::fmt;

use serde_json::{Map, Value, json};

use crate::error::ParseError;
use crate::model::Model;
use crate::puppet::{PuppetError, PuppetStage, parse_puppet};
use crate::validate::parse_model;

/// Maximum INP1 input size (256 MiB). INP1 files embed their
/// textures, so the asset bound of the JSON formats does not
/// apply; this bound exists so a hostile length field cannot
/// demand unbounded work.
pub const INP_BYTES_MAX: usize = 268_435_456;

const MAGIC: &[u8] = b"TRNSRTS\0";
const TEX_SECT: &[u8] = b"TEX_SECT";
const EXT_SECT: &[u8] = b"EXT_SECT";
const RADIANS_TO_DEGREES: f64 = 180.0 / std::f64::consts::PI;

/// An import failure. Wrapped format errors keep their own
/// `LRF-`/`LRP-` codes; `LRI-` codes (SPEC 13.10) are stable.
#[derive(Debug, Clone, PartialEq)]
pub enum ImportError {
    /// LRI-001: the magic bytes do not match.
    BadMagic,
    /// LRI-002: the container ends early, or the input is
    /// over the byte bound.
    Truncated { context: String },
    /// LRI-003: the payload is not valid JSON / not an object.
    BadPayload { message: String },
    /// LRI-004: no TEX_SECT after the payload.
    MissingTextureSection,
    /// LRI-005: a texture uses an unsupported encoding byte.
    TextureEncoding { index: usize, byte: u8 },
    /// LRI-006: an enum string the importer does not know.
    UnknownEnum { context: String, value: String },
    /// LRI-007: malformed puppet data (missing fields, bad
    /// shapes, unknown uuid references).
    Malformed { message: String },
    /// LRI-008: a feature with no stage representation.
    Unsupported { message: String },
    /// The assembled model failed its own validation.
    ModelInvalid { source: ParseError },
    /// The assembled stage failed its own validation.
    StageInvalid { source: PuppetError },
}

impl ImportError {
    /// The stable code for this failure.
    pub fn code(&self) -> &'static str {
        match self {
            ImportError::BadMagic => "LRI-001",
            ImportError::Truncated { .. } => "LRI-002",
            ImportError::BadPayload { .. } => "LRI-003",
            ImportError::MissingTextureSection => "LRI-004",
            ImportError::TextureEncoding { .. } => "LRI-005",
            ImportError::UnknownEnum { .. } => "LRI-006",
            ImportError::Malformed { .. } => "LRI-007",
            ImportError::Unsupported { .. } => "LRI-008",
            ImportError::ModelInvalid { source } => source.code(),
            ImportError::StageInvalid { source } => source.code(),
        }
    }
}

impl fmt::Display for ImportError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let code = self.code();
        match self {
            ImportError::BadMagic => write!(f, "{code}: not an INP1 file (bad magic)"),
            ImportError::Truncated { context } => {
                write!(f, "{code}: container truncated at {context}")
            }
            ImportError::BadPayload { message } => {
                write!(f, "{code}: bad payload JSON: {message}")
            }
            ImportError::MissingTextureSection => {
                write!(f, "{code}: no TEX_SECT after the payload")
            }
            ImportError::TextureEncoding { index, byte } => {
                write!(
                    f,
                    "{code}: texture {index} has unsupported encoding byte {byte}"
                )
            }
            ImportError::UnknownEnum { context, value } => {
                write!(f, "{code}: unknown {context} {value:?}")
            }
            ImportError::Malformed { message } => write!(f, "{code}: {message}"),
            ImportError::Unsupported { message } => {
                write!(f, "{code}: unsupported: {message}")
            }
            ImportError::ModelInvalid { source } => {
                write!(f, "LRI-009: assembled model invalid: {source}")
            }
            ImportError::StageInvalid { source } => {
                write!(f, "LRI-010: assembled stage invalid: {source}")
            }
        }
    }
}

impl std::error::Error for ImportError {}

fn malformed(message: impl Into<String>) -> ImportError {
    ImportError::Malformed {
        message: message.into(),
    }
}

/// One payload texture, byte-preserved, with the stage
/// texture-table source path it belongs at.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportedTexture {
    /// Stage texture id (`tex_000`, ...).
    pub id: String,
    /// Relative source path (`textures/tex_000.png`).
    pub source: String,
    /// The encoded payload bytes, untouched.
    pub bytes: Vec<u8>,
}

/// What the importer saw, for the round-trip census: counts
/// of everything, including the carried-but-inert data.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ImportReport {
    /// Total nodes.
    pub nodes: usize,
    /// Group nodes.
    pub group_nodes: usize,
    /// Part nodes.
    pub part_nodes: usize,
    /// Composite nodes.
    pub composite_nodes: usize,
    /// Mesh-group nodes (their meshes ride along inert).
    pub mesh_group_nodes: usize,
    /// Physics nodes.
    pub physics_nodes: usize,
    /// Parameters (vec2 + 1D).
    pub parameters: usize,
    /// Vec2 parameters among them.
    pub vec2_parameters: usize,
    /// Bindings carried.
    pub bindings: usize,
    /// Bindings with cubic interpolation (carried, inert).
    pub cubic_bindings: usize,
    /// Mask references.
    pub masks: usize,
    /// Payload textures.
    pub textures: usize,
    /// EXT_SECT vendor entries skipped (counted, not read).
    pub ext_entries: usize,
    /// Extra per-part texture slots (emissive/bump) the
    /// reference runtime parses but never renders; the stage
    /// keeps slot 0 (albedo) only, and the drop is counted
    /// here — loud, never silent (SPEC.md section 13.12).
    pub dropped_texture_slots: usize,
}

/// The import product.
#[derive(Debug, Clone)]
pub struct ImportedPuppet {
    /// The validated model (parameters + part tree).
    pub model: Model,
    /// The validated puppet stage.
    pub stage: PuppetStage,
    /// Payload textures in stage texture-table order.
    pub textures: Vec<ImportedTexture>,
    /// The import census.
    pub report: ImportReport,
}

/* ------------------------------------------------------------------ */
/* Container reading                                                   */
/* ------------------------------------------------------------------ */

struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, n: usize, context: &str) -> Result<&'a [u8], ImportError> {
        if n > self.bytes.len() - self.pos.min(self.bytes.len()) {
            return Err(ImportError::Truncated {
                context: context.to_string(),
            });
        }
        let out = &self.bytes[self.pos..self.pos + n];
        self.pos += n;
        Ok(out)
    }
    fn be_u32(&mut self, context: &str) -> Result<usize, ImportError> {
        let b = self.take(4, context)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize)
    }
}

/* ------------------------------------------------------------------ */
/* JSON access helpers (all failures are LRI-007)                      */
/* ------------------------------------------------------------------ */

fn obj<'v>(value: &'v Value, context: &str) -> Result<&'v Map<String, Value>, ImportError> {
    value
        .as_object()
        .ok_or_else(|| malformed(format!("{context}: expected an object")))
}

fn get<'v>(o: &'v Map<String, Value>, key: &str) -> Option<&'v Value> {
    o.get(key)
}

fn req<'v>(o: &'v Map<String, Value>, key: &str, context: &str) -> Result<&'v Value, ImportError> {
    o.get(key)
        .ok_or_else(|| malformed(format!("{context}: missing field {key:?}")))
}

fn as_str<'v>(value: &'v Value, context: &str) -> Result<&'v str, ImportError> {
    value
        .as_str()
        .ok_or_else(|| malformed(format!("{context}: expected a string")))
}

/// INP1 uuids serialize as numbers; accept strings too and
/// normalize to the decimal string form.
fn as_uuid(value: &Value, context: &str) -> Result<String, ImportError> {
    match value {
        Value::Number(n) => Ok(n.to_string()),
        Value::String(s) => Ok(s.clone()),
        _ => Err(malformed(format!(
            "{context}: uuid is neither number nor string"
        ))),
    }
}

fn node_name(node: &Map<String, Value>) -> String {
    get(node, "name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string()
}

fn as_f64(value: &Value, context: &str) -> Result<f64, ImportError> {
    value
        .as_f64()
        .ok_or_else(|| malformed(format!("{context}: expected a number")))
}

fn as_arr<'v>(value: &'v Value, context: &str) -> Result<&'v Vec<Value>, ImportError> {
    value
        .as_array()
        .ok_or_else(|| malformed(format!("{context}: expected an array")))
}

fn opt_f64(
    o: &Map<String, Value>,
    key: &str,
    default: f64,
    context: &str,
) -> Result<f64, ImportError> {
    match get(o, key) {
        None => Ok(default),
        Some(v) => as_f64(v, context),
    }
}

fn vec_n<const N: usize>(
    o: &Map<String, Value>,
    key: &str,
    default: [f64; N],
    context: &str,
) -> Result<[f64; N], ImportError> {
    match get(o, key) {
        None => Ok(default),
        Some(v) => {
            let arr = as_arr(v, context)?;
            if arr.len() != N {
                return Err(malformed(format!(
                    "{context}: {key} has {} components, expected {N}",
                    arr.len()
                )));
            }
            let mut out = [0.0f64; N];
            for (i, item) in arr.iter().enumerate() {
                out[i] = as_f64(item, context)?;
            }
            Ok(out)
        }
    }
}

fn opt_bool(o: &Map<String, Value>, key: &str, default: bool) -> bool {
    get(o, key).and_then(Value::as_bool).unwrap_or(default)
}

/* ------------------------------------------------------------------ */
/* Id assignment                                                       */
/* ------------------------------------------------------------------ */

/// Map a source name to the shared id grammar: strip trailing
/// NUL padding (the exporter pads names), keep
/// `[A-Za-z0-9_-]`, and collapse every maximal run of other
/// characters into one `_` (so the source's `:: ` namespace
/// separator becomes a single underscore). Collisions the
/// collapse creates are resolved by the id allocator.
fn sanitize_id(name: &str) -> String {
    let trimmed = name.trim_end_matches('\0');
    let mut out = String::with_capacity(trimmed.len());
    let mut in_run = false;
    for ch in trimmed.chars() {
        if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
            out.push(ch);
            in_run = false;
        } else if !in_run {
            out.push('_');
            in_run = true;
        }
    }
    while out.len() > 56 {
        out.pop();
    }
    if out.is_empty() {
        out.push_str("unnamed");
    }
    out
}

/// A deterministic id allocator: first claim keeps the base
/// id, later collisions take `_2`, `_3`, ...
struct IdAllocator {
    used: HashSet<String>,
}

impl IdAllocator {
    fn new() -> Self {
        IdAllocator {
            used: HashSet::new(),
        }
    }
    fn claim(&mut self, base: &str) -> String {
        if self.used.insert(base.to_string()) {
            return base.to_string();
        }
        let mut n = 2u32;
        loop {
            let candidate = format!("{base}_{n}");
            if self.used.insert(candidate.clone()) {
                return candidate;
            }
            n += 1;
        }
    }
}

/* ------------------------------------------------------------------ */
/* The importer                                                        */
/* ------------------------------------------------------------------ */

/// Import an INP1 file. `fallback_id` names the model when
/// the puppet's own meta name is empty or unusable.
pub fn import_inp(bytes: &[u8], fallback_id: &str) -> Result<ImportedPuppet, ImportError> {
    if bytes.len() > INP_BYTES_MAX {
        return Err(ImportError::Truncated {
            context: format!(
                "input is {} bytes, over the {INP_BYTES_MAX}-byte bound",
                bytes.len()
            ),
        });
    }
    let mut cursor = Cursor { bytes, pos: 0 };
    if cursor.take(8, "magic")? != MAGIC {
        return Err(ImportError::BadMagic);
    }
    let json_len = cursor.be_u32("payload length")?;
    let payload_bytes = cursor.take(json_len, "payload")?;
    let payload_text = std::str::from_utf8(payload_bytes).map_err(|e| ImportError::BadPayload {
        message: e.to_string(),
    })?;
    let payload: Value =
        serde_json::from_str(payload_text).map_err(|e| ImportError::BadPayload {
            message: e.to_string(),
        })?;
    let root = obj(&payload, "payload")?;

    // Textures.
    if cursor.take(8, "TEX_SECT header")? != TEX_SECT {
        return Err(ImportError::MissingTextureSection);
    }
    let tex_count = cursor.be_u32("texture count")?;
    if tex_count > crate::limits::PUPPET_TEXTURES_MAX {
        return Err(ImportError::Unsupported {
            message: format!(
                "{tex_count} payload textures, over the stage bound of {}",
                crate::limits::PUPPET_TEXTURES_MAX
            ),
        });
    }
    let mut textures: Vec<ImportedTexture> = Vec::with_capacity(tex_count);
    for i in 0..tex_count {
        let len = cursor.be_u32("texture length")?;
        let encoding = cursor.take(1, "texture encoding")?[0];
        let ext = match encoding {
            0 => "png",
            1 => "tga",
            other => {
                return Err(ImportError::TextureEncoding {
                    index: i,
                    byte: other,
                });
            }
        };
        let data = cursor.take(len, "texture data")?;
        textures.push(ImportedTexture {
            id: format!("tex_{i:03}"),
            source: format!("textures/tex_{i:03}.{ext}"),
            bytes: data.to_vec(),
        });
    }

    // Optional vendor section: counted, not read (application
    // settings are not puppet semantics).
    let mut ext_entries = 0usize;
    if cursor.pos + 8 <= bytes.len() && &bytes[cursor.pos..cursor.pos + 8] == EXT_SECT {
        cursor.take(8, "EXT_SECT header")?;
        ext_entries = cursor.be_u32("EXT_SECT count")?;
    }

    build_assets(root, textures, ext_entries, fallback_id)
}

fn build_assets(
    root: &Map<String, Value>,
    textures: Vec<ImportedTexture>,
    ext_entries: usize,
    fallback_id: &str,
) -> Result<ImportedPuppet, ImportError> {
    let mut report = ImportReport {
        textures: textures.len(),
        ext_entries,
        ..Default::default()
    };

    // Meta + model id.
    let meta = get(root, "meta").and_then(|m| m.as_object());
    let meta_name = meta
        .and_then(|m| get(m, "name"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let meta_version = meta
        .and_then(|m| get(m, "version"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let base = sanitize_id(&meta_name);
    let model_id = if base == "unnamed" {
        sanitize_id(fallback_id)
    } else {
        base
    };

    // Physics globals (source defaults: 1000 px/m, 9.8 m/s²).
    let physics_obj = get(root, "physics").and_then(|p| p.as_object());
    let (pixels_per_meter, gravity) = match physics_obj {
        Some(p) => (
            opt_f64(p, "pixelsPerMeter", 1000.0, "physics")?,
            opt_f64(p, "gravity", 9.8, "physics")?,
        ),
        None => (1000.0, 9.8),
    };

    // ---- Parameters --------------------------------------------------
    let params_arr = as_arr(req(root, "param", "payload")?, "param")?;
    let mut param_ids = IdAllocator::new();
    let mut scalar_ids = IdAllocator::new();
    let mut param_uuid_to_id: HashMap<String, String> = HashMap::new();
    let mut seen_param_uuids: HashSet<String> = HashSet::new();
    let mut stage_params: Vec<Value> = Vec::new();
    let mut model_params: Vec<Value> = Vec::new();
    let mut provenance_names: Vec<[String; 2]> = Vec::new();
    let mut raw_params: Vec<&Map<String, Value>> = Vec::with_capacity(params_arr.len());
    for item in params_arr {
        let p = obj(item, "param entry")?;
        raw_params.push(p);
        let name = as_str(req(p, "name", "param")?, "param name")?;
        let uuid = as_uuid(req(p, "uuid", "param")?, "param uuid")?;
        if !seen_param_uuids.insert(uuid.clone()) {
            return Err(malformed(format!("duplicate param uuid {uuid:?}")));
        }
        let id = param_ids.claim(&sanitize_id(name));
        if id != name {
            provenance_names.push([id.clone(), name.to_string()]);
        }
        param_uuid_to_id.insert(uuid, id.clone());
        let is_vec2 = opt_bool(p, "is_vec2", false);
        let min = vec_n(p, "min", [0.0, 0.0], "param min")?;
        let max = vec_n(p, "max", [1.0, 1.0], "param max")?;
        let default = vec_n(p, "defaults", [0.0, 0.0], "param defaults")?;
        let axes_raw = as_arr(req(p, "axis_points", "param")?, "axis_points")?;
        if axes_raw.len() != 2 {
            return Err(malformed("param axis_points must have two channels"));
        }
        // INP1 axis points are NORMALIZED positions across
        // [min, max] (the reference runtime normalizes the
        // value, `(v - min) / (max - min)`, and looks the
        // axis up in that space). The stage stores axes in
        // value space, so the import maps t -> min + t*(max-min)
        // per channel (SPEC.md section 13.12). The mapping is
        // affine, so interpolation weights are unchanged.
        let denormalize = |t: f64, lo: f64, hi: f64| lo + t * (hi - lo);
        let axis_x: Vec<f64> = as_arr(&axes_raw[0], "axis_points x")?
            .iter()
            .map(|v| Ok(denormalize(as_f64(v, "axis point")?, min[0], max[0])))
            .collect::<Result<_, ImportError>>()?;
        let axis_y: Vec<f64> = if is_vec2 {
            as_arr(&axes_raw[1], "axis_points y")?
                .iter()
                .map(|v| Ok(denormalize(as_f64(v, "axis point")?, min[1], max[1])))
                .collect::<Result<_, ImportError>>()?
        } else {
            Vec::new()
        };
        let x_scalar = scalar_ids.claim(&format!("{id}_x"));
        let y_scalar = if is_vec2 {
            Some(scalar_ids.claim(&format!("{id}_y")))
        } else {
            None
        };
        model_params.push(json!({
            "kind": "scalar", "id": x_scalar,
            "min": min[0], "max": max[0], "default": default[0],
        }));
        if let Some(y_scalar) = &y_scalar {
            model_params.push(json!({
                "kind": "scalar", "id": y_scalar,
                "min": min[1], "max": max[1], "default": default[1],
            }));
        }
        stage_params.push(json!({
            "id": id, "x": x_scalar, "y": y_scalar,
            "min": min, "max": max, "default": default,
            "axis_points": [axis_x, axis_y],
        }));
        report.parameters += 1;
        if is_vec2 {
            report.vec2_parameters += 1;
        }
    }

    // ---- Nodes, pass A: assign every id first (mask sources
    // and physics references may point anywhere in the tree).
    let nodes_root = obj(req(root, "nodes", "payload")?, "nodes")?;
    let mut node_ids = IdAllocator::new();
    let mut uuid_to_id: HashMap<String, String> = HashMap::new();
    assign_node_ids(
        nodes_root,
        &mut node_ids,
        &mut uuid_to_id,
        &mut provenance_names,
        0,
    )?;

    // ---- Nodes, pass B: convert.
    let mut ctx = ConvertCtx {
        uuid_to_id: &uuid_to_id,
        param_uuid_to_id: &param_uuid_to_id,
        textures: &textures,
        report: ImportReport::default(),
    };
    let mut nodes_json: Vec<Value> = Vec::new();
    convert_node(nodes_root, None, &mut ctx, &mut nodes_json)?;
    report.nodes = ctx.report.nodes;
    report.group_nodes = ctx.report.group_nodes;
    report.part_nodes = ctx.report.part_nodes;
    report.composite_nodes = ctx.report.composite_nodes;
    report.mesh_group_nodes = ctx.report.mesh_group_nodes;
    report.physics_nodes = ctx.report.physics_nodes;
    report.masks = ctx.report.masks;
    report.dropped_texture_slots = ctx.report.dropped_texture_slots;

    // Model parts mirror the node forest 1:1.
    let model_parts: Vec<Value> = nodes_json
        .iter()
        .map(|n| json!({ "id": n["id"].clone(), "parent": n["parent"].clone(), "opacity": 1.0 }))
        .collect();

    // ---- Bindings ----------------------------------------------------
    let mut stage_bindings: Vec<Value> = Vec::new();
    for p in &raw_params {
        let uuid = as_uuid(req(p, "uuid", "param")?, "param uuid")?;
        let param_id = param_uuid_to_id
            .get(&uuid)
            .expect("param id recorded")
            .clone();
        let bindings = match get(p, "bindings") {
            None => continue,
            Some(b) => as_arr(b, "bindings")?,
        };
        for b in bindings {
            let b = obj(b, "binding")?;
            stage_bindings.push(convert_binding(b, &param_id, &uuid_to_id, &mut report)?);
        }
    }

    // ---- Assemble + validate (dogfooding the format gates) -----------
    let model_json = json!({
        "format": "lumen-rig-model",
        "format_version": 1,
        "id": model_id,
        "generator": "lumen-rig-core inp1 importer",
        "parameters": model_params,
        "parts": model_parts,
        "textures": [],
        "deformers": [],
        "meshes": [],
        "physics": [],
    });
    let model_bytes = serde_json::to_vec(&model_json).expect("model json");
    let model = parse_model(&model_bytes).map_err(|source| ImportError::ModelInvalid { source })?;

    let stage_json = json!({
        "format": "lumen-rig-puppet",
        "format_version": 1,
        "model": model_id,
        "generator": "lumen-rig-core inp1 importer",
        "provenance": {
            "source_name": meta_name,
            "source_version": meta_version,
            "names": provenance_names,
        },
        "parameters": stage_params,
        "textures": textures
            .iter()
            .map(|t| json!({"id": t.id, "source": t.source}))
            .collect::<Vec<_>>(),
        "nodes": nodes_json,
        "bindings": stage_bindings,
        "physics": {
            "pixels_per_meter": pixels_per_meter,
            "gravity": gravity,
        },
    });
    let stage_bytes = serde_json::to_vec(&stage_json).expect("stage json");
    let stage = parse_puppet(&stage_bytes, &model)
        .map_err(|source| ImportError::StageInvalid { source })?;

    Ok(ImportedPuppet {
        model,
        stage,
        textures,
        report,
    })
}

fn assign_node_ids(
    node: &Map<String, Value>,
    node_ids: &mut IdAllocator,
    uuid_to_id: &mut HashMap<String, String>,
    provenance_names: &mut Vec<[String; 2]>,
    depth: usize,
) -> Result<(), ImportError> {
    if depth > 64 {
        return Err(malformed("node tree deeper than 64"));
    }
    let name = node_name(node);
    let uuid = as_uuid(req(node, "uuid", "node")?, "node uuid")?;
    let id = node_ids.claim(&sanitize_id(&name));
    if id != name {
        provenance_names.push([id.clone(), name]);
    }
    if uuid_to_id.insert(uuid, id).is_some() {
        return Err(malformed("duplicate node uuid"));
    }
    if let Some(children) = get(node, "children") {
        for child in as_arr(children, "children")? {
            assign_node_ids(
                obj(child, "child node")?,
                node_ids,
                uuid_to_id,
                provenance_names,
                depth + 1,
            )?;
        }
    }
    Ok(())
}

struct ConvertCtx<'a> {
    uuid_to_id: &'a HashMap<String, String>,
    param_uuid_to_id: &'a HashMap<String, String>,
    textures: &'a [ImportedTexture],
    report: ImportReport,
}

fn convert_node(
    node: &Map<String, Value>,
    parent: Option<&str>,
    ctx: &mut ConvertCtx,
    out_nodes: &mut Vec<Value>,
) -> Result<(), ImportError> {
    let uuid = as_uuid(req(node, "uuid", "node")?, "node uuid")?;
    let id = ctx
        .uuid_to_id
        .get(&uuid)
        .expect("id assigned in pass A")
        .clone();
    let type_str = get(node, "type").and_then(Value::as_str);
    let kind = match type_str {
        None | Some("Node") => "group",
        Some("Part") => "part",
        Some("Composite") => "composite",
        Some("MeshGroup") => "mesh_group",
        Some("SimplePhysics") => "physics",
        Some(other) => {
            return Err(ImportError::UnknownEnum {
                context: "node type".to_string(),
                value: other.to_string(),
            });
        }
    };

    // Transform: INP rotations are radians; the format's are
    // degrees.
    let transform = get(node, "transform").and_then(|t| t.as_object());
    let (translation, rotation_deg, scale, pixel_snap) = match transform {
        Some(t) => (
            vec_n(t, "trans", [0.0, 0.0, 0.0], "transform trans")?,
            vec_n(t, "rot", [0.0, 0.0, 0.0], "transform rot")?.map(|r| r * RADIANS_TO_DEGREES),
            vec_n(t, "scale", [1.0, 1.0], "transform scale")?,
            opt_bool(t, "pixelSnap", false),
        ),
        None => ([0.0; 3], [0.0; 3], [1.0; 2], false),
    };

    let mut out = json!({
        "id": id,
        "parent": parent,
        "kind": kind,
        "enabled": opt_bool(node, "enabled", true),
        "lock_to_root": opt_bool(node, "lockToRoot", false),
        "zsort": opt_f64(node, "zsort", 0.0, "node zsort")?,
        "transform": {
            "translation": translation,
            "rotation_degrees": rotation_deg,
            "scale": scale,
            "pixel_snap": pixel_snap,
        },
    });

    match kind {
        "part" | "composite" => {
            out["drawable"] = convert_drawable(node)?;
            out["masks"] = convert_masks(node, ctx)?;
            out["mask_threshold"] = json!(opt_f64(node, "mask_threshold", 0.5, "mask_threshold")?);
            if kind == "part" {
                out["mesh"] = convert_mesh(node, &id, ctx.textures, &mut ctx.report, true)?;
                ctx.report.part_nodes += 1;
            } else {
                ctx.report.composite_nodes += 1;
            }
        }
        "mesh_group" => {
            if get(node, "mesh").is_some() {
                out["group_mesh"] = convert_mesh(node, &id, ctx.textures, &mut ctx.report, false)?;
            }
            ctx.report.mesh_group_nodes += 1;
        }
        "physics" => {
            out["physics"] = convert_physics(node, ctx.param_uuid_to_id)?;
            ctx.report.physics_nodes += 1;
        }
        _ => {
            ctx.report.group_nodes += 1;
        }
    }
    ctx.report.nodes += 1;
    out_nodes.push(out);

    if let Some(children) = get(node, "children") {
        for child in as_arr(children, "children")? {
            convert_node(obj(child, "child node")?, Some(&id), ctx, out_nodes)?;
        }
    }
    Ok(())
}

fn convert_drawable(node: &Map<String, Value>) -> Result<Value, ImportError> {
    let blend_raw = get(node, "blend_mode")
        .and_then(Value::as_str)
        .unwrap_or("Normal");
    let blend = match blend_raw {
        "Normal" => "normal",
        "Multiply" => "multiply",
        "ColorDodge" => "color_dodge",
        "LinearDodge" => "linear_dodge",
        "Screen" => "screen",
        "ClipToLower" => "clip_to_lower",
        "SliceFromLower" => "slice_from_lower",
        other => {
            return Err(ImportError::UnknownEnum {
                context: "blend mode".to_string(),
                value: other.to_string(),
            });
        }
    };
    Ok(json!({
        "blend_mode": blend,
        "opacity": opt_f64(node, "opacity", 1.0, "opacity")?,
        "tint": vec_n(node, "tint", [1.0, 1.0, 1.0], "tint")?,
        "screen_tint": vec_n(node, "screenTint", [0.0, 0.0, 0.0], "screenTint")?,
    }))
}

fn convert_masks(node: &Map<String, Value>, ctx: &mut ConvertCtx) -> Result<Value, ImportError> {
    let mut out: Vec<Value> = Vec::new();
    if let Some(masks) = get(node, "masks") {
        for m in as_arr(masks, "masks")? {
            let m = obj(m, "mask")?;
            let source_uuid = as_uuid(req(m, "source", "mask")?, "mask source")?;
            let source = ctx
                .uuid_to_id
                .get(&source_uuid)
                .cloned()
                .ok_or_else(|| malformed(format!("mask source uuid {source_uuid} unknown")))?;
            let mode_raw = get(m, "mode").and_then(Value::as_str).unwrap_or("Mask");
            let mode = match mode_raw {
                "Mask" => "mask",
                "DodgeMask" => "dodge_mask",
                other => {
                    return Err(ImportError::UnknownEnum {
                        context: "mask mode".to_string(),
                        value: other.to_string(),
                    });
                }
            };
            out.push(json!({"source": source, "mode": mode}));
            ctx.report.masks += 1;
        }
    }
    Ok(Value::Array(out))
}

fn convert_mesh(
    node: &Map<String, Value>,
    node_id: &str,
    textures: &[ImportedTexture],
    report: &mut ImportReport,
    textured: bool,
) -> Result<Value, ImportError> {
    let mesh = obj(req(node, "mesh", "part")?, "mesh")?;
    let verts = as_arr(req(mesh, "verts", "mesh")?, "verts")?;
    let uvs = as_arr(req(mesh, "uvs", "mesh")?, "uvs")?;
    if verts.len() % 2 != 0 || uvs.len() % 2 != 0 {
        return Err(malformed(format!(
            "node {node_id}: flat vertex/uv arrays must be even"
        )));
    }
    let pairs = |flat: &Vec<Value>| -> Result<Vec<Value>, ImportError> {
        let mut out = Vec::with_capacity(flat.len() / 2);
        for pair in flat.as_chunks::<2>().0 {
            out.push(json!([
                as_f64(&pair[0], "vertex")?,
                as_f64(&pair[1], "vertex")?
            ]));
        }
        Ok(out)
    };
    let origin = vec_n(mesh, "origin", [0.0, 0.0], "mesh origin")?;
    let mut out = json!({
        "id": format!("{node_id}_mesh"),
        "origin": origin,
        "vertices": pairs(verts)?,
        "uvs": pairs(uvs)?,
        "indices": req(mesh, "indices", "mesh")?.clone(),
    });
    if textured {
        let tex_arr = as_arr(req(node, "textures", "part")?, "textures")?;
        if tex_arr.is_empty() {
            return Err(malformed(format!("part {node_id} has no texture")));
        }
        if tex_arr.len() > 1 {
            // The reference runtime parses the extra slots
            // (emissive/bump) but its renderer never samples
            // them; the stage mirrors that behavior and the
            // import report counts the drop.
            report.dropped_texture_slots += tex_arr.len() - 1;
        }
        let index = tex_arr[0]
            .as_u64()
            .ok_or_else(|| malformed(format!("part {node_id}: texture index not a number")))?
            as usize;
        let texture = textures.get(index).ok_or_else(|| {
            malformed(format!(
                "part {node_id}: texture index {index} out of range"
            ))
        })?;
        out["texture"] = json!(texture.id);
    }
    Ok(out)
}

fn convert_physics(
    node: &Map<String, Value>,
    param_uuid_to_id: &HashMap<String, String>,
) -> Result<Value, ImportError> {
    let param_uuid = as_uuid(req(node, "param", "physics node")?, "physics param")?;
    let param_id = param_uuid_to_id
        .get(&param_uuid)
        .cloned()
        .ok_or_else(|| malformed(format!("physics param uuid {param_uuid} unknown")))?;
    let model_raw = get(node, "model_type")
        .and_then(Value::as_str)
        .unwrap_or("");
    let model = match model_raw {
        "Pendulum" => "rigid",
        "SpringPendulum" => "spring",
        other => {
            return Err(ImportError::UnknownEnum {
                context: "physics model_type".to_string(),
                value: other.to_string(),
            });
        }
    };
    let map_raw = get(node, "map_mode").and_then(Value::as_str).unwrap_or("");
    let map_mode = match map_raw {
        "XY" => "xy",
        "YX" => "yx",
        "AngleLength" => "angle_length",
        "LengthAngle" => "length_angle",
        other => {
            return Err(ImportError::UnknownEnum {
                context: "physics map_mode".to_string(),
                value: other.to_string(),
            });
        }
    };
    Ok(json!({
        "parameter": param_id,
        "model": model,
        "map_mode": map_mode,
        "local_only": opt_bool(node, "local_only", false),
        "gravity": opt_f64(node, "gravity", 1.0, "physics gravity")?,
        "length": opt_f64(node, "length", 1.0, "physics length")?,
        "frequency": opt_f64(node, "frequency", 1.0, "physics frequency")?,
        "angle_damping": opt_f64(node, "angle_damping", 0.5, "angle_damping")?,
        "length_damping": opt_f64(node, "length_damping", 0.5, "length_damping")?,
        "output_scale": vec_n(node, "output_scale", [1.0, 1.0], "output_scale")?,
    }))
}

fn convert_binding(
    b: &Map<String, Value>,
    param_id: &str,
    uuid_to_id: &HashMap<String, String>,
    report: &mut ImportReport,
) -> Result<Value, ImportError> {
    let node_uuid = as_uuid(req(b, "node", "binding")?, "binding node")?;
    let node_id = uuid_to_id
        .get(&node_uuid)
        .cloned()
        .ok_or_else(|| malformed(format!("binding node uuid {node_uuid} unknown")))?;
    let param_name = as_str(req(b, "param_name", "binding")?, "binding param_name")?;
    let (kind, is_rotation) = match param_name {
        "zSort" => ("z_sort", false),
        "transform.t.x" => ("translation_x", false),
        "transform.t.y" => ("translation_y", false),
        "transform.s.x" => ("scale_x", false),
        "transform.s.y" => ("scale_y", false),
        "transform.r.x" => ("rotation_x", true),
        "transform.r.y" => ("rotation_y", true),
        "transform.r.z" => ("rotation_z", true),
        "deform" => ("deform", false),
        "opacity" => ("opacity", false),
        other => {
            return Err(ImportError::UnknownEnum {
                context: "binding param_name".to_string(),
                value: other.to_string(),
            });
        }
    };
    let interp_raw = as_str(req(b, "interpolate_mode", "binding")?, "interpolate_mode")?;
    let interpolate = match interp_raw {
        "Linear" => "linear",
        "Nearest" => "nearest",
        "Cubic" => "cubic",
        other => {
            return Err(ImportError::UnknownEnum {
                context: "interpolate_mode".to_string(),
                value: other.to_string(),
            });
        }
    };
    if interpolate == "cubic" {
        report.cubic_bindings += 1;
    }
    report.bindings += 1;
    let mut values = req(b, "values", "binding")?.clone();
    if is_rotation {
        // INP rotation offsets are radians; the format's are
        // degrees.
        scale_grid_numbers(&mut values);
    }
    Ok(json!({
        "parameter": param_id,
        "node": node_id,
        "kind": kind,
        "interpolate": interpolate,
        "values": values,
    }))
}

/// Multiply every number in a scalar grid by 180/π.
fn scale_grid_numbers(values: &mut Value) {
    match values {
        Value::Number(n) => {
            if let Some(v) = n.as_f64() {
                *n = serde_json::Number::from_f64(v * RADIANS_TO_DEGREES)
                    .unwrap_or_else(|| serde_json::Number::from(0));
            }
        }
        Value::Array(items) => {
            for item in items {
                scale_grid_numbers(item);
            }
        }
        _ => {}
    }
}
