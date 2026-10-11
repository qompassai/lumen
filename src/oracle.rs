//! Oracle intake (sprite-to-3D factory, oracle arm): how generative
//! image-to-3D GLB results enter the factory.
//!
//! Generative output is stochastic and unrigged; it enters lumen only as
//! an *oracle* whose output the parametric stages measure and fit (see
//! `TODO.md`, "Standing design boundary"). This module is the intake
//! door: it validates one GLB, normalizes its orientation to the
//! trellis.cpp convention, and emits a manifest recording exactly what
//! was done. Nothing downstream ever guesses where a model came from.
//!
//! Contract summary:
//! - The source is EXPLICIT. `OracleSource` is parsed from a caller
//!   supplied token (`pixal3d`, `trellis`, `trellis-cpp`, `trellis.cpp`);
//!   any other token is rejected. The filename is never consulted.
//! - Pixal3D GLBs export facing AWAY from the trellis.cpp convention
//!   (trial record, 2026-10-07). Intake rotates them 180 degrees about Y
//!   by default. The rotation is a single new scene-graph wrapper node
//!   carrying the quaternion `[0, 1, 0, 0]`; the binary chunk is copied
//!   byte for byte and no vertex, index, or accessor is touched, so the
//!   transform is a pure rigid rotation and exactly reversible.
//! - The trellis.cpp path is passed through BYTE-IDENTICAL: no wrapper,
//!   no re-serialization, no orientation change of any kind.
//! - Fail closed: truncated or corrupt GLB structure, non-2.0 glTF,
//!   out-of-bounds references, external buffer URIs, and required
//!   extensions outside the reviewed allowlist are all typed errors.
//!   The only allowlisted required extension is `EXT_texture_webp`
//!   (texture encoding; it cannot change geometry), which is what both
//!   oracle generators emit. A required extension that can change
//!   geometry (mesh compression, quantization) must be reviewed before
//!   it is allowlisted — until then intake refuses the file.
//! - Bounded work: input at most `GLB_BYTES_MAX`; array sizes, node
//!   tree depth, and reported position counts all carry named caps.
//! - Hashes are 64-bit FNV-1a, rendered as 16 lowercase hex digits —
//!   change detectors, not security primitives (same convention as
//!   `crate::identity`).

#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

use serde::Serialize;
use serde_json::{Map, Value, json};

use crate::{LumenError, resolve_write_path};

/// Upper bound on an intake GLB. The heaviest oracle output on record is
/// the 40.5 MB Pixal3D trial GLB; this leaves generous headroom while
/// keeping a hostile file from exhausting memory before validation.
pub const GLB_BYTES_MAX: u64 = 256 * 1024 * 1024;
/// Schema tag written into every intake manifest.
pub const INTAKE_SCHEMA: &str = "lumen.oracle.intake";
/// Bumped whenever the intake transform or manifest shape changes.
pub const INTAKE_VERSION: u32 = 1;
/// Deepest node chain `world_positions` will walk. Oracle GLBs are flat;
/// this bounds recursion over hostile nesting.
pub const NODE_TREE_DEPTH_MAX: usize = 64;
/// Name of the wrapper node intake adds around Pixal3D scene roots.
pub const ROTATE_Y180_NODE_NAME: &str = "lumen_oracle_y180";

const ACCESSORS_MAX: usize = 65_536;
const BUFFER_VIEWS_MAX: usize = 16_384;
const BUFFERS_MAX: usize = 64;
const CHUNK_HEADER_BYTES: usize = 8;
const CHUNK_TYPE_BIN: u32 = 0x004E_4942;
const CHUNK_TYPE_JSON: u32 = 0x4E4F_534A;
const COMPONENT_FLOAT: u64 = 5126;
const COMPONENT_UNSIGNED_BYTE: u64 = 5121;
const COMPONENT_UNSIGNED_INT: u64 = 5125;
const COMPONENT_UNSIGNED_SHORT: u64 = 5123;
const FNV1A64_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV1A64_PRIME: u64 = 0x0000_0100_0000_01b3;
const GLB_HEADER_BYTES: usize = 12;
const GLB_MAGIC: u32 = 0x4654_6c67;
const GLB_VERSION: u32 = 2;
const MESHES_MAX: usize = 4096;
const NODES_MAX: usize = 8192;
/// Required glTF extensions intake accepts without further review. Each
/// entry must be provably geometry-neutral; see the module contract.
const REQUIRED_EXTENSIONS_ALLOWED: [&str; 1] = ["EXT_texture_webp"];
const SCENES_MAX: usize = 64;
/// Cap on positions returned by `world_positions` (8M * 12 B = 96 MB
/// worst case), so a valid-but-huge oracle mesh cannot blow up callers.
const WORLD_POSITIONS_MAX: usize = 8_000_000;

/// Which oracle produced a GLB. The source decides the orientation
/// normalization; it is part of the intake contract, never inferred.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OracleSource {
    /// Tencent ARC Pixal3D (via the pixal3d.cpp GGUF port). Exports
    /// facing away from the trellis.cpp convention; intake rotates
    /// 180 degrees about Y.
    Pixal3d,
    /// trellis.cpp (TRELLIS GGUF route). Defines the shared
    /// orientation convention; intake passes its GLBs through
    /// byte-identical.
    TrellisCpp,
}

impl OracleSource {
    /// Canonical token for this source, as recorded in manifests.
    pub fn as_str(&self) -> &'static str {
        match self {
            OracleSource::Pixal3d => "pixal3d",
            OracleSource::TrellisCpp => "trellis-cpp",
        }
    }

    /// Parse a caller-supplied source token, fail closed: only the
    /// documented tokens match (case-insensitively, no trimming).
    pub fn parse(raw: &str) -> Result<Self, LumenError> {
        if raw.eq_ignore_ascii_case("pixal3d") {
            return Ok(OracleSource::Pixal3d);
        }
        if raw.eq_ignore_ascii_case("trellis")
            || raw.eq_ignore_ascii_case("trellis-cpp")
            || raw.eq_ignore_ascii_case("trellis.cpp")
        {
            return Ok(OracleSource::TrellisCpp);
        }
        Err(LumenError::BadParam(format!(
            "unknown oracle source \"{raw}\": expected \"pixal3d\" or \"trellis-cpp\""
        )))
    }

    /// Y-axis rotation intake applies for this source, in degrees.
    pub fn rotation_degrees_y(&self) -> u32 {
        match self {
            OracleSource::Pixal3d => 180,
            OracleSource::TrellisCpp => 0,
        }
    }
}

/// Geometry and structure counts for one validated GLB.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct GlbStats {
    pub accessors: u64,
    pub buffer_views: u64,
    pub buffers: u64,
    pub meshes: u64,
    pub nodes: u64,
    pub primitives: u64,
    pub scenes: u64,
    pub triangles: u64,
    pub vertices: u64,
}

/// The intake manifest: a versioned record of one intake run. Geometry
/// counts are taken before and after the transform and must agree; the
/// report is only produced when they do.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct IntakeReport {
    /// True when the output binary chunk is byte-identical to the
    /// input's (always true by construction; verified, not assumed).
    pub bin_chunk_preserved: bool,
    pub input_bytes: u64,
    pub input_fnv1a64: String,
    pub meshes: u64,
    pub output_bytes: u64,
    pub output_fnv1a64: String,
    pub primitives: u64,
    pub rotation_degrees_y: u32,
    pub schema: String,
    pub source: String,
    pub triangles: u64,
    pub version: u32,
    pub vertices: u64,
    /// Wrapper nodes added (one per non-empty scene for Pixal3D, else 0).
    pub wrapper_nodes_added: u32,
}

/// A parsed, validated GLB: JSON document plus the exact binary-chunk
/// bytes as stored in the file (including any chunk padding).
struct ParsedGlb {
    bin_chunk: Option<Vec<u8>>,
    json: Value,
    stats: GlbStats,
}

/// Intake one GLB held in memory. Returns the normalized GLB bytes and
/// the manifest. For `TrellisCpp` the output is the input, byte for
/// byte. For `Pixal3d` the output differs only in the JSON chunk: one
/// wrapper node per non-empty scene, carrying the 180-degree Y
/// rotation; the binary chunk is preserved byte for byte.
pub fn intake_glb_bytes(
    input: &[u8],
    source: OracleSource,
) -> Result<(Vec<u8>, IntakeReport), LumenError> {
    if input.len() as u64 > GLB_BYTES_MAX {
        return Err(LumenError::DocInvalid(format!(
            "glb is {} bytes; at most {GLB_BYTES_MAX} are accepted",
            input.len()
        )));
    }
    let parsed = parse_glb(input)?;
    let (output, wrapper_nodes_added) = match source {
        OracleSource::TrellisCpp => (input.to_vec(), 0),
        OracleSource::Pixal3d => {
            let mut document = parsed.json.clone();
            let wrappers = rotate_document(&mut document)?;
            let bytes = build_glb(&document, parsed.bin_chunk.as_deref())?;
            (bytes, wrappers)
        }
    };
    // Lossless proof, enforced on every run: the output must re-validate
    // with identical geometry counts, and its binary chunk must equal
    // the input's byte for byte.
    let out_parsed = parse_glb(&output)?;
    if out_parsed.stats.vertices != parsed.stats.vertices
        || out_parsed.stats.triangles != parsed.stats.triangles
        || out_parsed.stats.meshes != parsed.stats.meshes
        || out_parsed.stats.primitives != parsed.stats.primitives
    {
        return Err(LumenError::DocInvalid(
            "internal: intake changed geometry counts".to_string(),
        ));
    }
    let bin_chunk_preserved = out_parsed.bin_chunk == parsed.bin_chunk;
    if !bin_chunk_preserved {
        return Err(LumenError::DocInvalid(
            "internal: intake changed the binary chunk".to_string(),
        ));
    }
    let report = IntakeReport {
        bin_chunk_preserved,
        input_bytes: input.len() as u64,
        input_fnv1a64: fnv1a64_hex(input),
        meshes: parsed.stats.meshes,
        output_bytes: output.len() as u64,
        output_fnv1a64: fnv1a64_hex(&output),
        primitives: parsed.stats.primitives,
        rotation_degrees_y: source.rotation_degrees_y(),
        schema: INTAKE_SCHEMA.to_string(),
        source: source.as_str().to_string(),
        triangles: parsed.stats.triangles,
        version: INTAKE_VERSION,
        vertices: parsed.stats.vertices,
        wrapper_nodes_added,
    };
    Ok((output, report))
}

/// Intake one GLB file inside a project root and write the normalized
/// GLB (plus an optional JSON manifest) back inside the same root.
/// Paths are project-relative and fail closed under the same rules as
/// `crate::resolve_sprite_path`; only `.glb` inputs are accepted.
pub fn intake_glb_file(
    root: &Path,
    input_rel: &str,
    output_rel: &str,
    manifest_rel: Option<&str>,
    source: OracleSource,
) -> Result<IntakeReport, LumenError> {
    let input_path = resolve_glb_path(input_rel, root)?;
    let metadata = std::fs::metadata(&input_path).map_err(LumenError::from)?;
    if metadata.len() > GLB_BYTES_MAX {
        return Err(LumenError::DocInvalid(format!(
            "glb is {} bytes; at most {GLB_BYTES_MAX} are accepted",
            metadata.len()
        )));
    }
    let input = std::fs::read(&input_path).map_err(LumenError::from)?;
    let (output, report) = intake_glb_bytes(&input, source)?;
    let output_path = resolve_write_path(output_rel, root, &["glb"])?;
    write_atomic(&output_path, &output)?;
    if let Some(manifest) = manifest_rel {
        let manifest_path = resolve_write_path(manifest, root, &["json"])?;
        let mut text = serde_json::to_string_pretty(&report)
            .map_err(|e| LumenError::BadParam(format!("manifest serialize failed: {e}")))?;
        text.push('\n');
        write_atomic(&manifest_path, text.as_bytes())?;
    }
    Ok(report)
}

/// Validate one GLB held in memory and report its structure and
/// geometry counts. No normalization is applied.
pub fn inspect_glb_bytes(input: &[u8]) -> Result<GlbStats, LumenError> {
    if input.len() as u64 > GLB_BYTES_MAX {
        return Err(LumenError::DocInvalid(format!(
            "glb is {} bytes; at most {GLB_BYTES_MAX} are accepted",
            input.len()
        )));
    }
    Ok(parse_glb(input)?.stats)
}

/// World-space vertex positions of the default scene, in traversal
/// order. This is the geometry-level check on intake: after a Pixal3D
/// intake, every position `(x, y, z)` of the raw model must appear as
/// `(-x, y, -z)`.
///
/// Only TRS node transforms are supported; a node carrying a `matrix`
/// is rejected explicitly rather than silently mis-measured. Scale in
/// an ancestor chain is applied component-wise, which is exact for the
/// shear-free chains oracle GLBs and the intake wrapper use.
pub fn world_positions(input: &[u8]) -> Result<Vec<[f32; 3]>, LumenError> {
    let parsed = parse_glb(input)?;
    let document = parsed
        .json
        .as_object()
        .ok_or_else(|| LumenError::DocInvalid("glTF root is not an object".to_string()))?;
    let scene_index = match document.get("scene") {
        Some(value) => json_usize(value, "scene")?,
        None => 0,
    };
    let scenes = document
        .get("scenes")
        .and_then(Value::as_array)
        .ok_or_else(|| LumenError::DocInvalid("glTF has no scenes array".to_string()))?;
    let scene = scenes
        .get(scene_index)
        .ok_or_else(|| LumenError::DocInvalid("default scene index out of bounds".to_string()))?;
    let roots = match scene.get("nodes") {
        Some(value) => json_index_list(value, "scene nodes")?,
        None => Vec::new(),
    };
    let identity = Trs::identity();
    let mut out: Vec<[f32; 3]> = Vec::new();
    let mut visited: usize = 0;
    for root_index in roots {
        collect_node_positions(&parsed, root_index, &identity, 0, &mut visited, &mut out)?;
    }
    Ok(out)
}

/// One node's TRS transform (glTF defaults applied).
#[derive(Debug, Clone, Copy)]
struct Trs {
    rotation: [f64; 4],
    scale: [f64; 3],
    translation: [f64; 3],
}

impl Trs {
    fn identity() -> Self {
        Self {
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
            translation: [0.0, 0.0, 0.0],
        }
    }

    /// `self` applied after `child`: world = self ∘ child.
    fn compose(&self, child: &Trs) -> Trs {
        let scaled_child_t = [
            child.translation[0] * self.scale[0],
            child.translation[1] * self.scale[1],
            child.translation[2] * self.scale[2],
        ];
        let rotated = quat_rotate(self.rotation, scaled_child_t);
        Trs {
            rotation: quat_multiply(self.rotation, child.rotation),
            scale: [
                self.scale[0] * child.scale[0],
                self.scale[1] * child.scale[1],
                self.scale[2] * child.scale[2],
            ],
            translation: [
                self.translation[0] + rotated[0],
                self.translation[1] + rotated[1],
                self.translation[2] + rotated[2],
            ],
        }
    }

    fn apply(&self, point: [f64; 3]) -> [f64; 3] {
        let scaled = [
            point[0] * self.scale[0],
            point[1] * self.scale[1],
            point[2] * self.scale[2],
        ];
        let rotated = quat_rotate(self.rotation, scaled);
        [
            self.translation[0] + rotated[0],
            self.translation[1] + rotated[1],
            self.translation[2] + rotated[2],
        ]
    }
}

fn quat_multiply(a: [f64; 4], b: [f64; 4]) -> [f64; 4] {
    let (ax, ay, az, aw) = (a[0], a[1], a[2], a[3]);
    let (bx, by, bz, bw) = (b[0], b[1], b[2], b[3]);
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

fn quat_rotate(q: [f64; 4], v: [f64; 3]) -> [f64; 3] {
    // v' = v + 2 * cross(q.xyz, cross(q.xyz, v) + q.w * v)
    let (qx, qy, qz, qw) = (q[0], q[1], q[2], q[3]);
    let cx = qy * v[2] - qz * v[1] + qw * v[0];
    let cy = qz * v[0] - qx * v[2] + qw * v[1];
    let cz = qx * v[1] - qy * v[0] + qw * v[2];
    [
        v[0] + 2.0 * (qy * cz - qz * cy),
        v[1] + 2.0 * (qz * cx - qx * cz),
        v[2] + 2.0 * (qx * cy - qy * cx),
    ]
}

fn collect_node_positions(
    parsed: &ParsedGlb,
    node_index: usize,
    parent: &Trs,
    depth: usize,
    visited: &mut usize,
    out: &mut Vec<[f32; 3]>,
) -> Result<(), LumenError> {
    if depth > NODE_TREE_DEPTH_MAX {
        return Err(LumenError::DocInvalid(format!(
            "node tree deeper than {NODE_TREE_DEPTH_MAX}"
        )));
    }
    *visited = visited
        .checked_add(1)
        .ok_or_else(|| LumenError::DocInvalid("node visit count overflow".to_string()))?;
    if *visited > NODES_MAX {
        return Err(LumenError::DocInvalid(format!(
            "node traversal exceeds {NODES_MAX} visits"
        )));
    }
    let nodes = parsed
        .json
        .get("nodes")
        .and_then(Value::as_array)
        .ok_or_else(|| LumenError::DocInvalid("glTF has no nodes array".to_string()))?;
    let node = nodes
        .get(node_index)
        .ok_or_else(|| LumenError::DocInvalid("node index out of bounds".to_string()))?;
    let world = parent.compose(&node_trs(node)?);
    if let Some(mesh_value) = node.get("mesh") {
        let mesh_index = json_usize(mesh_value, "node mesh")?;
        for position in mesh_positions(parsed, mesh_index)? {
            if out.len() >= WORLD_POSITIONS_MAX {
                return Err(LumenError::DocInvalid(format!(
                    "world positions exceed {WORLD_POSITIONS_MAX}"
                )));
            }
            let p = world.apply([
                f64::from(position[0]),
                f64::from(position[1]),
                f64::from(position[2]),
            ]);
            out.push([p[0] as f32, p[1] as f32, p[2] as f32]);
        }
    }
    if let Some(children) = node.get("children") {
        for child in json_index_list(children, "node children")? {
            collect_node_positions(parsed, child, &world, depth + 1, visited, out)?;
        }
    }
    Ok(())
}

/// A node's local TRS. `matrix` nodes are rejected: this measurement
/// path does not decompose matrices, and guessing would corrupt it.
fn node_trs(node: &Value) -> Result<Trs, LumenError> {
    if node.get("matrix").is_some() {
        return Err(LumenError::DocInvalid(
            "world_positions does not support nodes with a matrix transform".to_string(),
        ));
    }
    let mut trs = Trs::identity();
    if let Some(value) = node.get("translation") {
        trs.translation = json_vec_f64(value, "node translation")?;
    }
    if let Some(value) = node.get("rotation") {
        trs.rotation = json_vec_f64(value, "node rotation")?;
    }
    if let Some(value) = node.get("scale") {
        trs.scale = json_vec_f64(value, "node scale")?;
    }
    Ok(trs)
}

/// Raw POSITION values of one mesh's primitives, in primitive order.
fn mesh_positions(parsed: &ParsedGlb, mesh_index: usize) -> Result<Vec<[f32; 3]>, LumenError> {
    let meshes = parsed
        .json
        .get("meshes")
        .and_then(Value::as_array)
        .ok_or_else(|| LumenError::DocInvalid("glTF has no meshes array".to_string()))?;
    let mesh = meshes
        .get(mesh_index)
        .ok_or_else(|| LumenError::DocInvalid("mesh index out of bounds".to_string()))?;
    let primitives = mesh
        .get("primitives")
        .and_then(Value::as_array)
        .ok_or_else(|| LumenError::DocInvalid("mesh has no primitives array".to_string()))?;
    let mut out = Vec::new();
    for primitive in primitives {
        let accessor_index = primitive
            .get("attributes")
            .and_then(|attrs| attrs.get("POSITION"))
            .ok_or_else(|| {
                LumenError::DocInvalid("mesh primitive has no POSITION attribute".to_string())
            })?;
        let accessor_index = json_usize(accessor_index, "POSITION accessor")?;
        out.extend(read_position_accessor(parsed, accessor_index)?);
    }
    Ok(out)
}

/// Decode one float VEC3 accessor from the binary chunk.
fn read_position_accessor(
    parsed: &ParsedGlb,
    accessor_index: usize,
) -> Result<Vec<[f32; 3]>, LumenError> {
    let bin = parsed.bin_chunk.as_deref().ok_or_else(|| {
        LumenError::DocInvalid("glb has no binary chunk to read positions from".to_string())
    })?;
    let accessors = parsed
        .json
        .get("accessors")
        .and_then(Value::as_array)
        .ok_or_else(|| LumenError::DocInvalid("glTF has no accessors array".to_string()))?;
    let accessor = accessors
        .get(accessor_index)
        .ok_or_else(|| LumenError::DocInvalid("accessor index out of bounds".to_string()))?;
    // Validation already established: float VEC3 with a bufferView.
    let view_index = json_usize(
        accessor
            .get("bufferView")
            .ok_or_else(|| LumenError::DocInvalid("accessor has no bufferView".to_string()))?,
        "accessor bufferView",
    )?;
    let count = json_usize(
        accessor
            .get("count")
            .ok_or_else(|| LumenError::DocInvalid("accessor has no count".to_string()))?,
        "accessor count",
    )?;
    let accessor_offset = match accessor.get("byteOffset") {
        Some(value) => json_usize(value, "accessor byteOffset")?,
        None => 0,
    };
    let views = parsed
        .json
        .get("bufferViews")
        .and_then(Value::as_array)
        .ok_or_else(|| LumenError::DocInvalid("glTF has no bufferViews array".to_string()))?;
    let view = views
        .get(view_index)
        .ok_or_else(|| LumenError::DocInvalid("bufferView index out of bounds".to_string()))?;
    let view_offset = match view.get("byteOffset") {
        Some(value) => json_usize(value, "bufferView byteOffset")?,
        None => 0,
    };
    let stride = match view.get("byteStride") {
        Some(value) => json_usize(value, "bufferView byteStride")?,
        None => 12,
    };
    if stride < 12 {
        return Err(LumenError::DocInvalid(format!(
            "bufferView byteStride {stride} is smaller than a VEC3 float (12)"
        )));
    }
    let base = view_offset
        .checked_add(accessor_offset)
        .ok_or_else(|| LumenError::DocInvalid("accessor offset overflow".to_string()))?;
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let start = base
            .checked_add(i.checked_mul(stride).ok_or_else(|| {
                LumenError::DocInvalid("accessor element offset overflow".to_string())
            })?)
            .ok_or_else(|| {
                LumenError::DocInvalid("accessor element offset overflow".to_string())
            })?;
        let bytes = bin.get(start..start + 12).ok_or_else(|| {
            LumenError::DocInvalid("accessor data runs past the binary chunk".to_string())
        })?;
        let mut point = [0.0f32; 3];
        for (axis, chunk) in bytes.as_chunks::<4>().0.iter().enumerate() {
            point[axis] = f32::from_le_bytes(*chunk);
        }
        out.push(point);
    }
    Ok(out)
}

/// Parse and fully validate the GLB container and glTF document.
fn parse_glb(input: &[u8]) -> Result<ParsedGlb, LumenError> {
    if input.len() < GLB_HEADER_BYTES {
        return Err(LumenError::DocInvalid(format!(
            "glb is {} bytes; the header alone is {GLB_HEADER_BYTES}",
            input.len()
        )));
    }
    let magic = read_u32_le(input, 0)?;
    if magic != GLB_MAGIC {
        return Err(LumenError::DocInvalid(
            "not a GLB file (bad magic)".to_string(),
        ));
    }
    let version = read_u32_le(input, 4)?;
    if version != GLB_VERSION {
        return Err(LumenError::DocInvalid(format!(
            "glb container version {version}; only version {GLB_VERSION} is accepted"
        )));
    }
    let declared_len = read_u32_le(input, 8)? as usize;
    if declared_len != input.len() {
        return Err(LumenError::DocInvalid(format!(
            "glb declares {declared_len} bytes but {} were supplied (truncated or trailing data)",
            input.len()
        )));
    }
    let mut offset = GLB_HEADER_BYTES;
    let mut json_chunk: Option<Value> = None;
    let mut bin_chunk: Option<Vec<u8>> = None;
    while offset < input.len() {
        let header = input
            .get(offset..offset + CHUNK_HEADER_BYTES)
            .ok_or_else(|| {
                LumenError::DocInvalid("glb chunk header runs past the file".to_string())
            })?;
        let chunk_len = read_u32_le(header, 0)? as usize;
        let chunk_type = read_u32_le(header, 4)?;
        if !chunk_len.is_multiple_of(4) {
            return Err(LumenError::DocInvalid(format!(
                "glb chunk length {chunk_len} is not 4-byte aligned"
            )));
        }
        let data_start = offset + CHUNK_HEADER_BYTES;
        let data_end = data_start
            .checked_add(chunk_len)
            .filter(|end| *end <= input.len())
            .ok_or_else(|| {
                LumenError::DocInvalid("glb chunk data runs past the file (truncated)".to_string())
            })?;
        let data = &input[data_start..data_end];
        match chunk_type {
            CHUNK_TYPE_JSON => {
                if json_chunk.is_some() || offset != GLB_HEADER_BYTES {
                    return Err(LumenError::DocInvalid(
                        "glb JSON chunk must appear exactly once, first".to_string(),
                    ));
                }
                let parsed: Value = serde_json::from_slice(data).map_err(|e| {
                    LumenError::DocInvalid(format!("glTF JSON does not parse: {e}"))
                })?;
                json_chunk = Some(parsed);
            }
            CHUNK_TYPE_BIN => {
                if bin_chunk.is_some() {
                    return Err(LumenError::DocInvalid(
                        "glb has more than one binary chunk".to_string(),
                    ));
                }
                bin_chunk = Some(data.to_vec());
            }
            other => {
                return Err(LumenError::DocInvalid(format!(
                    "glb chunk type {other:#010x} is not accepted at intake"
                )));
            }
        }
        offset = data_end;
    }
    let json =
        json_chunk.ok_or_else(|| LumenError::DocInvalid("glb has no JSON chunk".to_string()))?;
    let stats = validate_document(&json, bin_chunk.as_deref())?;
    Ok(ParsedGlb {
        bin_chunk,
        json,
        stats,
    })
}

fn read_u32_le(bytes: &[u8], offset: usize) -> Result<u32, LumenError> {
    let slice = bytes.get(offset..offset + 4).ok_or_else(|| {
        LumenError::DocInvalid("unexpected end of glb while reading a u32".to_string())
    })?;
    Ok(u32::from_le_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

/// Structural validation of the glTF document against the binary chunk
/// that carries it. Returns geometry counts. Every index, offset, and
/// length is range-checked with checked arithmetic before it is used.
fn validate_document(json: &Value, bin_chunk: Option<&[u8]>) -> Result<GlbStats, LumenError> {
    let root = json
        .as_object()
        .ok_or_else(|| LumenError::DocInvalid("glTF root is not an object".to_string()))?;
    let version = root
        .get("asset")
        .and_then(|asset| asset.get("version"))
        .and_then(Value::as_str)
        .ok_or_else(|| LumenError::DocInvalid("glTF asset.version is missing".to_string()))?;
    if version != "2.0" && !version.starts_with("2.") {
        return Err(LumenError::DocInvalid(format!(
            "glTF version \"{version}\"; only 2.x is accepted"
        )));
    }
    if let Some(required) = root.get("extensionsRequired") {
        let list = required.as_array().ok_or_else(|| {
            LumenError::DocInvalid("extensionsRequired is not an array".to_string())
        })?;
        for entry in list {
            let name = entry.as_str().ok_or_else(|| {
                LumenError::DocInvalid("extensionsRequired entry is not a string".to_string())
            })?;
            if !REQUIRED_EXTENSIONS_ALLOWED.contains(&name) {
                return Err(LumenError::DocInvalid(format!(
                    "required extension \"{name}\" is not reviewed for intake; \
                     refusing rather than risking a geometry-changing transform"
                )));
            }
        }
    }
    let buffers = optional_array(root, "buffers")?;
    let buffer_views = optional_array(root, "bufferViews")?;
    let accessors = optional_array(root, "accessors")?;
    let meshes = optional_array(root, "meshes")?;
    let nodes = optional_array(root, "nodes")?;
    let scenes = optional_array(root, "scenes")?;
    check_len("buffers", buffers.len(), BUFFERS_MAX)?;
    check_len("bufferViews", buffer_views.len(), BUFFER_VIEWS_MAX)?;
    check_len("accessors", accessors.len(), ACCESSORS_MAX)?;
    check_len("meshes", meshes.len(), MESHES_MAX)?;
    check_len("nodes", nodes.len(), NODES_MAX)?;
    check_len("scenes", scenes.len(), SCENES_MAX)?;
    if scenes.is_empty() {
        return Err(LumenError::DocInvalid(
            "glTF has no scenes; an oracle model must carry a scene".to_string(),
        ));
    }
    // Buffers: a GLB is self-contained. Buffer 0 is the binary chunk;
    // any buffer with a URI is an external reference and is refused.
    let mut buffer_lengths: Vec<u64> = Vec::with_capacity(buffers.len());
    for (index, buffer) in buffers.iter().enumerate() {
        if buffer.get("uri").is_some() {
            return Err(LumenError::DocInvalid(format!(
                "buffer {index} has a URI; intake accepts self-contained GLBs only"
            )));
        }
        let byte_length = buffer
            .get("byteLength")
            .and_then(Value::as_u64)
            .ok_or_else(|| LumenError::DocInvalid(format!("buffer {index} has no byteLength")))?;
        buffer_lengths.push(byte_length);
    }
    if let Some(first) = buffer_lengths.first() {
        match bin_chunk {
            Some(chunk) if *first <= chunk.len() as u64 => {}
            Some(_) => {
                return Err(LumenError::DocInvalid(
                    "buffer 0 byteLength exceeds the binary chunk (truncated GLB)".to_string(),
                ));
            }
            None if *first > 0 => {
                return Err(LumenError::DocInvalid(
                    "glTF declares buffer data but the GLB has no binary chunk".to_string(),
                ));
            }
            None => {}
        }
    }
    // Buffer views must land inside their buffer.
    for (index, view) in buffer_views.iter().enumerate() {
        let buffer_index = json_usize(
            view.get("buffer").ok_or_else(|| {
                LumenError::DocInvalid(format!("bufferView {index} has no buffer"))
            })?,
            "bufferView buffer",
        )?;
        let buffer_len = *buffer_lengths.get(buffer_index).ok_or_else(|| {
            LumenError::DocInvalid(format!("bufferView {index} buffer out of bounds"))
        })?;
        let offset = value_or_zero(view, "byteOffset")?;
        let length = view
            .get("byteLength")
            .and_then(Value::as_u64)
            .ok_or_else(|| {
                LumenError::DocInvalid(format!("bufferView {index} has no byteLength"))
            })?;
        let end = offset
            .checked_add(length)
            .ok_or_else(|| LumenError::DocInvalid(format!("bufferView {index} range overflows")))?;
        if end > buffer_len {
            return Err(LumenError::DocInvalid(format!(
                "bufferView {index} range {offset}..{end} exceeds its buffer ({buffer_len})"
            )));
        }
    }
    // Accessors must land inside their buffer view.
    for (index, accessor) in accessors.iter().enumerate() {
        validate_accessor(index, accessor, buffer_views)?;
    }
    // Meshes: count vertices and triangles from POSITION + indices.
    let mut vertices: u64 = 0;
    let mut triangles: u64 = 0;
    let mut primitives_total: u64 = 0;
    for (mesh_index, mesh) in meshes.iter().enumerate() {
        let primitives = mesh
            .get("primitives")
            .and_then(Value::as_array)
            .filter(|list| !list.is_empty())
            .ok_or_else(|| {
                LumenError::DocInvalid(format!("mesh {mesh_index} has no primitives"))
            })?;
        for primitive in primitives {
            primitives_total += 1;
            let position_index = primitive
                .get("attributes")
                .and_then(|attrs| attrs.get("POSITION"))
                .ok_or_else(|| {
                    LumenError::DocInvalid(format!(
                        "mesh {mesh_index} primitive has no POSITION attribute"
                    ))
                })?;
            let position_index = json_usize(position_index, "POSITION accessor")?;
            let position = accessors.get(position_index).ok_or_else(|| {
                LumenError::DocInvalid("POSITION accessor out of bounds".to_string())
            })?;
            if position.get("type").and_then(Value::as_str) != Some("VEC3")
                || position.get("componentType").and_then(Value::as_u64) != Some(COMPONENT_FLOAT)
            {
                return Err(LumenError::DocInvalid(
                    "POSITION accessor must be float VEC3 at intake".to_string(),
                ));
            }
            let position_count = accessor_count(position)?;
            vertices = vertices
                .checked_add(position_count)
                .ok_or_else(|| LumenError::DocInvalid("vertex count overflow".to_string()))?;
            let tri_count = match primitive.get("indices") {
                Some(indices_value) => {
                    let indices_index = json_usize(indices_value, "indices accessor")?;
                    let indices = accessors.get(indices_index).ok_or_else(|| {
                        LumenError::DocInvalid("indices accessor out of bounds".to_string())
                    })?;
                    if indices.get("type").and_then(Value::as_str) != Some("SCALAR") {
                        return Err(LumenError::DocInvalid(
                            "indices accessor must be SCALAR".to_string(),
                        ));
                    }
                    match indices.get("componentType").and_then(Value::as_u64) {
                        Some(ct)
                            if ct == COMPONENT_UNSIGNED_BYTE
                                || ct == COMPONENT_UNSIGNED_SHORT
                                || ct == COMPONENT_UNSIGNED_INT => {}
                        _ => {
                            return Err(LumenError::DocInvalid(
                                "indices accessor must be an unsigned integer type".to_string(),
                            ));
                        }
                    }
                    let count = accessor_count(indices)?;
                    if !count.is_multiple_of(3) {
                        return Err(LumenError::DocInvalid(
                            "indices count is not a multiple of 3".to_string(),
                        ));
                    }
                    count / 3
                }
                None => {
                    if !position_count.is_multiple_of(3) {
                        return Err(LumenError::DocInvalid(
                            "unindexed primitive position count is not a multiple of 3".to_string(),
                        ));
                    }
                    position_count / 3
                }
            };
            triangles = triangles
                .checked_add(tri_count)
                .ok_or_else(|| LumenError::DocInvalid("triangle count overflow".to_string()))?;
        }
    }
    // Nodes and scenes: every reference must resolve.
    for (index, node) in nodes.iter().enumerate() {
        if let Some(mesh_value) = node.get("mesh") {
            let mesh_index = json_usize(mesh_value, "node mesh")?;
            if mesh_index >= meshes.len() {
                return Err(LumenError::DocInvalid(format!(
                    "node {index} mesh {mesh_index} out of bounds"
                )));
            }
        }
        if let Some(children) = node.get("children") {
            for child in json_index_list(children, "node children")? {
                if child >= nodes.len() {
                    return Err(LumenError::DocInvalid(format!(
                        "node {index} child {child} out of bounds"
                    )));
                }
            }
        }
        if let Some(skin_value) = node.get("skin") {
            let skin_index = json_usize(skin_value, "node skin")?;
            let skins = optional_array(root, "skins")?;
            if skin_index >= skins.len() {
                return Err(LumenError::DocInvalid(format!(
                    "node {index} skin {skin_index} out of bounds"
                )));
            }
        }
    }
    for (index, scene) in scenes.iter().enumerate() {
        if let Some(roots) = scene.get("nodes") {
            for root_index in json_index_list(roots, "scene nodes")? {
                if root_index >= nodes.len() {
                    return Err(LumenError::DocInvalid(format!(
                        "scene {index} node {root_index} out of bounds"
                    )));
                }
            }
        }
    }
    if let Some(scene_value) = root.get("scene") {
        let scene_index = json_usize(scene_value, "scene")?;
        if scene_index >= scenes.len() {
            return Err(LumenError::DocInvalid(format!(
                "default scene {scene_index} out of bounds"
            )));
        }
    }
    Ok(GlbStats {
        accessors: accessors.len() as u64,
        buffer_views: buffer_views.len() as u64,
        buffers: buffers.len() as u64,
        meshes: meshes.len() as u64,
        nodes: nodes.len() as u64,
        primitives: primitives_total,
        scenes: scenes.len() as u64,
        triangles,
        vertices,
    })
}

fn validate_accessor(
    index: usize,
    accessor: &Value,
    buffer_views: &[Value],
) -> Result<(), LumenError> {
    let component_type = accessor
        .get("componentType")
        .and_then(Value::as_u64)
        .ok_or_else(|| LumenError::DocInvalid(format!("accessor {index} has no componentType")))?;
    let component_size: u64 = match component_type {
        5120 | 5121 => 1,
        5122 | 5123 => 2,
        5125 | 5126 => 4,
        other => {
            return Err(LumenError::DocInvalid(format!(
                "accessor {index} componentType {other} is not a glTF component type"
            )));
        }
    };
    let type_name = accessor
        .get("type")
        .and_then(Value::as_str)
        .ok_or_else(|| LumenError::DocInvalid(format!("accessor {index} has no type")))?;
    let components: u64 = match type_name {
        "SCALAR" => 1,
        "VEC2" => 2,
        "VEC3" => 3,
        "VEC4" | "MAT2" => 4,
        "MAT3" => 9,
        "MAT4" => 16,
        other => {
            return Err(LumenError::DocInvalid(format!(
                "accessor {index} type \"{other}\" is not a glTF accessor type"
            )));
        }
    };
    let count = accessor_count(accessor)?;
    if let Some(view_value) = accessor.get("bufferView") {
        let view_index = json_usize(view_value, "accessor bufferView")?;
        let view = buffer_views.get(view_index).ok_or_else(|| {
            LumenError::DocInvalid(format!("accessor {index} bufferView out of bounds"))
        })?;
        let view_len = view
            .get("byteLength")
            .and_then(Value::as_u64)
            .ok_or_else(|| {
                LumenError::DocInvalid(format!("bufferView {view_index} has no byteLength"))
            })?;
        let offset = value_or_zero(accessor, "byteOffset")?;
        let element_bytes = component_size * components;
        let span = match view.get("byteStride").and_then(Value::as_u64) {
            Some(stride) if stride > element_bytes => {
                if count == 0 {
                    0
                } else {
                    stride
                        .checked_mul(count - 1)
                        .and_then(|v| v.checked_add(element_bytes))
                        .ok_or_else(|| {
                            LumenError::DocInvalid(format!("accessor {index} span overflows"))
                        })?
                }
            }
            _ => element_bytes.checked_mul(count).ok_or_else(|| {
                LumenError::DocInvalid(format!("accessor {index} span overflows"))
            })?,
        };
        let end = offset
            .checked_add(span)
            .ok_or_else(|| LumenError::DocInvalid(format!("accessor {index} range overflows")))?;
        if end > view_len {
            return Err(LumenError::DocInvalid(format!(
                "accessor {index} range {offset}..{end} exceeds its bufferView ({view_len})"
            )));
        }
    }
    Ok(())
}

fn accessor_count(accessor: &Value) -> Result<u64, LumenError> {
    accessor
        .get("count")
        .and_then(Value::as_u64)
        .ok_or_else(|| LumenError::DocInvalid("accessor has no count".to_string()))
}

fn check_len(what: &str, len: usize, max: usize) -> Result<(), LumenError> {
    if len > max {
        return Err(LumenError::DocInvalid(format!(
            "glTF has {len} {what}; at most {max} are accepted"
        )));
    }
    Ok(())
}

fn json_index_list(value: &Value, what: &str) -> Result<Vec<usize>, LumenError> {
    let list = value
        .as_array()
        .ok_or_else(|| LumenError::DocInvalid(format!("{what} is not an array")))?;
    let mut out = Vec::with_capacity(list.len());
    for entry in list {
        out.push(json_usize(entry, what)?);
    }
    Ok(out)
}

fn json_usize(value: &Value, what: &str) -> Result<usize, LumenError> {
    let raw = value
        .as_u64()
        .ok_or_else(|| LumenError::DocInvalid(format!("{what} is not a non-negative integer")))?;
    usize::try_from(raw)
        .map_err(|_| LumenError::DocInvalid(format!("{what} does not fit in a usize")))
}

fn json_vec_f64<const N: usize>(value: &Value, what: &str) -> Result<[f64; N], LumenError> {
    let list = value
        .as_array()
        .ok_or_else(|| LumenError::DocInvalid(format!("{what} is not an array")))?;
    if list.len() != N {
        return Err(LumenError::DocInvalid(format!(
            "{what} must have {N} components, has {}",
            list.len()
        )));
    }
    let mut out = [0.0f64; N];
    for (slot, entry) in out.iter_mut().zip(list.iter()) {
        let number = entry
            .as_f64()
            .filter(|n| n.is_finite())
            .ok_or_else(|| LumenError::DocInvalid(format!("{what} has a non-finite component")))?;
        *slot = number;
    }
    Ok(out)
}

fn optional_array<'a>(root: &'a Map<String, Value>, key: &str) -> Result<&'a [Value], LumenError> {
    match root.get(key) {
        Some(value) => value
            .as_array()
            .map(Vec::as_slice)
            .ok_or_else(|| LumenError::DocInvalid(format!("glTF {key} is not an array"))),
        None => Ok(&[]),
    }
}

fn value_or_zero(object: &Value, key: &str) -> Result<u64, LumenError> {
    match object.get(key) {
        Some(value) => value
            .as_u64()
            .ok_or_else(|| LumenError::DocInvalid(format!("{key} is not a non-negative integer"))),
        None => Ok(0),
    }
}

/// Rotate a validated document 180 degrees about Y: one wrapper node
/// per non-empty scene becomes the scene's only root, carrying the
/// quaternion `[0, 1, 0, 0]`. No existing node, mesh, accessor, or
/// buffer is modified. Scenes that share a root node are rejected —
/// wrapping a shared root twice would give it two parents, which glTF
/// forbids; oracle output has one scene and never shares roots.
fn rotate_document(document: &mut Value) -> Result<u32, LumenError> {
    // Phase 1 (read-only): collect each scene's roots and reject any
    // root that appears in more than one scene.
    let (nodes_len, scene_roots) = {
        let root = document
            .as_object()
            .ok_or_else(|| LumenError::DocInvalid("glTF root is not an object".to_string()))?;
        let scenes = root
            .get("scenes")
            .and_then(Value::as_array)
            .ok_or_else(|| LumenError::DocInvalid("glTF has no scenes array".to_string()))?;
        let mut scene_roots: Vec<Vec<usize>> = Vec::with_capacity(scenes.len());
        for scene in scenes {
            let roots = match scene.get("nodes") {
                Some(value) => json_index_list(value, "scene nodes")?,
                None => Vec::new(),
            };
            scene_roots.push(roots);
        }
        let mut seen: Vec<usize> = Vec::new();
        for roots in &scene_roots {
            for root_index in roots {
                if seen.contains(root_index) {
                    return Err(LumenError::DocInvalid(format!(
                        "scene root node {root_index} is shared between scenes; \
                         intake cannot wrap it without giving it two parents"
                    )));
                }
                seen.push(*root_index);
            }
        }
        let nodes_len = root
            .get("nodes")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        (nodes_len, scene_roots)
    };
    let wrapper_count = scene_roots.iter().filter(|roots| !roots.is_empty()).count();
    check_len("nodes", nodes_len + wrapper_count, NODES_MAX)?;
    if wrapper_count > 0
        && document
            .as_object()
            .is_some_and(|root| root.get("nodes").is_none())
    {
        return Err(LumenError::DocInvalid(
            "glTF scenes name roots but the document has no nodes array".to_string(),
        ));
    }
    // Phase 2 (mutating): append each wrapper, then point its scene at
    // it. Scene indices stay valid: only the `nodes` value of each scene
    // object is replaced, never the scenes array itself.
    let root = document
        .as_object_mut()
        .ok_or_else(|| LumenError::DocInvalid("glTF root is not an object".to_string()))?;
    let mut added: u32 = 0;
    for (scene_index, roots) in scene_roots.iter().enumerate() {
        if roots.is_empty() {
            continue;
        }
        let mut extras = Map::new();
        extras.insert("lumen_oracle_rotation_degrees_y".to_string(), json!(180));
        extras.insert(
            "lumen_oracle_source".to_string(),
            json!(OracleSource::Pixal3d.as_str()),
        );
        let mut wrapper = Map::new();
        wrapper.insert(
            "children".to_string(),
            Value::Array(roots.iter().map(|index| json!(index)).collect()),
        );
        wrapper.insert("extras".to_string(), Value::Object(extras));
        wrapper.insert("name".to_string(), json!(ROTATE_Y180_NODE_NAME));
        wrapper.insert("rotation".to_string(), json!([0.0, 1.0, 0.0, 0.0]));
        let wrapper_index = {
            let nodes = root
                .get_mut("nodes")
                .and_then(Value::as_array_mut)
                .ok_or_else(|| {
                    LumenError::DocInvalid("glTF has no nodes array to wrap".to_string())
                })?;
            nodes.push(Value::Object(wrapper));
            nodes.len() - 1
        };
        let scenes = root
            .get_mut("scenes")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| LumenError::DocInvalid("glTF scenes vanished".to_string()))?;
        let scene = scenes
            .get_mut(scene_index)
            .and_then(Value::as_object_mut)
            .ok_or_else(|| LumenError::DocInvalid("scene is not an object".to_string()))?;
        scene.insert("nodes".to_string(), json!([wrapper_index]));
        added += 1;
    }
    Ok(added)
}

/// Serialize a document plus the original binary chunk back into a GLB.
/// The binary chunk bytes are copied verbatim; the JSON chunk is padded
/// with spaces, per the GLB specification.
fn build_glb(document: &Value, bin_chunk: Option<&[u8]>) -> Result<Vec<u8>, LumenError> {
    let mut json_bytes = serde_json::to_vec(document)
        .map_err(|e| LumenError::BadParam(format!("glTF JSON serialize failed: {e}")))?;
    while !json_bytes.len().is_multiple_of(4) {
        json_bytes.push(b' ');
    }
    let bin_len = bin_chunk.map_or(0, <[u8]>::len);
    if !bin_len.is_multiple_of(4) {
        return Err(LumenError::DocInvalid(
            "binary chunk length is not 4-byte aligned".to_string(),
        ));
    }
    let total = GLB_HEADER_BYTES
        + CHUNK_HEADER_BYTES
        + json_bytes.len()
        + if bin_chunk.is_some() {
            CHUNK_HEADER_BYTES + bin_len
        } else {
            0
        };
    let total_u32 = u32::try_from(total)
        .map_err(|_| LumenError::DocInvalid("rebuilt glb exceeds u32 length".to_string()))?;
    let json_len_u32 = u32::try_from(json_bytes.len())
        .map_err(|_| LumenError::DocInvalid("glTF JSON exceeds u32 length".to_string()))?;
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(&GLB_MAGIC.to_le_bytes());
    out.extend_from_slice(&GLB_VERSION.to_le_bytes());
    out.extend_from_slice(&total_u32.to_le_bytes());
    out.extend_from_slice(&json_len_u32.to_le_bytes());
    out.extend_from_slice(&CHUNK_TYPE_JSON.to_le_bytes());
    out.extend_from_slice(&json_bytes);
    if let Some(bin) = bin_chunk {
        let bin_len_u32 = u32::try_from(bin.len())
            .map_err(|_| LumenError::DocInvalid("binary chunk exceeds u32 length".to_string()))?;
        out.extend_from_slice(&bin_len_u32.to_le_bytes());
        out.extend_from_slice(&CHUNK_TYPE_BIN.to_le_bytes());
        out.extend_from_slice(bin);
    }
    assert_eq!(out.len(), total, "glb rebuild length bookkeeping");
    Ok(out)
}

/// Resolve a user-supplied GLB input path against an explicit project
/// root, fail closed (same rules as `crate::resolve_sprite_path`, with
/// the `.glb` extension).
fn resolve_glb_path(user_path: &str, root: &Path) -> Result<PathBuf, LumenError> {
    if user_path.is_empty() {
        return Err(LumenError::PathRejected("empty path".to_string()));
    }
    let root = root
        .canonicalize()
        .map_err(|e| LumenError::PathRejected(format!("project root not accessible: {e}")))?;
    if Path::new(user_path).is_absolute() {
        return Err(LumenError::PathRejected(
            "absolute paths are not accepted; use a project-relative path".to_string(),
        ));
    }
    let candidate = root.join(user_path);
    let canonical = candidate.canonicalize().map_err(|_| {
        LumenError::PathRejected("file does not exist inside the project".to_string())
    })?;
    if !canonical.starts_with(&root) {
        return Err(LumenError::PathRejected(
            "path escapes the project root".to_string(),
        ));
    }
    match canonical.extension().and_then(|e| e.to_str()) {
        Some(ext) if ext.eq_ignore_ascii_case("glb") => Ok(canonical),
        _ => Err(LumenError::PathRejected(
            "only .glb oracle models are accepted".to_string(),
        )),
    }
}

/// Temp-file + rename, so a failed intake never leaves a partial GLB.
fn write_atomic(path: &Path, contents: &[u8]) -> Result<(), LumenError> {
    let mut temp = path.as_os_str().to_owned();
    temp.push(".tmp");
    let temp_path = PathBuf::from(temp);
    std::fs::write(&temp_path, contents).map_err(LumenError::from)?;
    std::fs::rename(&temp_path, path).map_err(LumenError::from)?;
    Ok(())
}

/// 64-bit FNV-1a, 16 lowercase hex digits. Change detector only.
fn fnv1a64_hex(bytes: &[u8]) -> String {
    let mut hash = FNV1A64_OFFSET;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(FNV1A64_PRIME);
    }
    format!("{hash:016x}")
}
