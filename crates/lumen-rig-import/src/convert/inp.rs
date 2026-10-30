//! INP (.inx, Inochi2D puppet) -> native conversion: the
//! upstream stage of exporter phase E2 (BRIEF.md in
//! `~/workspace/moc3-export-20261026/`). The R5a importer reads
//! Cubism bundles; this stage reads the estate's own Torogao
//! fixture format so a vendored INP model (Aka) can travel the
//! full outbound path INP -> native -> moc3 -> native.
//!
//! Format facts are taken from the primary sources, not from
//! the fixture alone:
//! - The INP1 container (magic `TRNSRTS\0`, big-endian u32
//!   payload length, JSON payload, `TEX_SECT`, optional
//!   `EXT_SECT`) follows the Inochi2D reference implementation's
//!   `fmt` package (read downstream-only; nothing here is
//!   translated from it — the layout is a wire fact).
//! - Binding semantics follow the same implementation's `core`
//!   package: a `deform` binding's grid holds per-vertex OFFSETS
//!   that are SUMMED into the drawable's deformation stack;
//!   `transform.*` / `zSort` value bindings accumulate into a
//!   per-node offset transform (translations and rotations add,
//!   scales multiply) which is MERGED with the node's authored
//!   transform component-wise (`calcOffset`) before the
//!   hierarchy composes. Runtime evaluation interpolates the
//!   dense value grids and never consults `isSet` (an
//!   editor-side marker; the editor materializes interpolated
//!   values into the grid before saving), so this stage reads
//!   the grids as the evaluated surface.
//! - Node matrices are `T * R * S` with
//!   `R = Rz * Ry * Rx` (the euler convention of the reference
//!   math library), composed parent-before-child.
//!
//! Mapping dispositions are MAPPING.md's: parameters split
//! per axis into native scalars (section 3.4), `ClipToLower`
//! lands as clipping (section 6.3), SimplePhysics maps per
//! section 9.2, and the section-14 upstream constructs are
//! handled as follows. Node transforms have no native
//! counterpart as such; this stage converts each DRIVEN node
//! into one native warp deformer whose 1x1 grid spans the
//! model's rest bounds and whose keyform corners are the
//! node's composed transform applied to those corners — a
//! bilinear cell map reproduces an affine map exactly, so the
//! only approximation is the deformer factorization itself:
//! with `E_k` the driven node's effective static transform
//! (authored merged with its driven offsets at the default
//! pose) and `P` the static world matrix of its parent, the
//! warp encodes `A_k(g) = P * Merged_k(g) * E_k^-1 * P^-1`
//! over the node's kept lattice axes. `A_k` is the identity at
//! the default pose and the factorization is exact whenever
//! at most one driven node deviates from default; simultaneous
//! deviations compose with a second-order commutator error,
//! recorded in the report. A lattice holds at most two axes,
//! so excess driving axes (and every axis sourced from a
//! physics-output parameter, which LRF-207 forbids as a
//! lattice axis) are folded at their default values and named
//! individually in the report with their measured impact.
//!
//! Coordinates are carried verbatim (the inbound converter's
//! precedent, MAPPING.md section 2): no rescale, no re-origin,
//! no y-flip. Vertex z components are projected away by the
//! same orthographic projection the reference renderer
//! applies; the maximum |z| is recorded. Textures are
//! references in the native format: this stage reads texture
//! slot headers (byte length, TGA dimensions) and never
//! touches pixel bytes — no downscale, no recompression, no
//! format conversion anywhere in the stage.

use std::collections::BTreeMap;
use std::collections::BTreeSet;

use lumen_rig_core::{
    Anchor, AnchorKind, BlendMode, Deformer, Lattice, Mesh, MeshForm, Model, Parameter, Part,
    Particle, PhysicsGroup, PhysicsOutput, ScalarParameter, Strand, Texture, WarpDeformer,
    WarpForm, parse_model,
};
use serde::{Deserialize, Serialize};

use crate::convert::GENERATOR;
use crate::convert::ReportEntry;
use crate::error::ConvertError;

/// Whole-file bound for one .inx (Aka is 17.7 MB; the bound
/// exists so a hostile or truncated stream fails typed instead
/// of exhausting memory).
const INP_BYTES_MAX: usize = 256 * 1024 * 1024;
/// Node-count bound (Aka: 100).
const NODES_MAX: usize = 4096;
/// Parameter-count bound (Aka: 34).
const INP_PARAMETERS_MAX: usize = 1024;
/// Binding-count bound across all parameters (Aka: 259).
const BINDINGS_MAX: usize = 16384;
/// Draw-order scale: one INP zsort unit maps to this many
/// native draw-order steps (order-exact, values rescaled —
/// the section 7 lossy class, applied at this hop).
const ZSORT_TO_DRAW_ORDER: f32 = 1000.0;

/// What a successful INP conversion produced.
#[derive(Debug)]
pub struct InpOutcome {
    /// The converted, re-validated native model.
    pub model: Model,
    /// The conversion report (counts, losses, syntheses).
    pub report: InpReport,
}

/// The INP conversion report: the inbound mirror of
/// `ExportReport`. Every construct that was approximated,
/// dropped, or synthesized at this hop is an entry; nothing is
/// silent.
#[derive(Debug, Clone, Serialize)]
pub struct InpReport {
    pub format: &'static str,
    pub model_id: String,
    /// Source container summary.
    pub source: InpSourceSummary,
    pub counts: InpCounts,
    /// Constructs carried with information or behavior given
    /// up; the mechanism is named per entry.
    pub lossy: Vec<ReportEntry>,
    /// Constructs with no native target, not carried.
    pub no_map: Vec<ReportEntry>,
    /// Values the source does not carry that this stage had to
    /// synthesize to satisfy the native schema.
    pub synthesized: Vec<ReportEntry>,
    pub notes: Vec<String>,
}

impl InpReport {
    /// Canonical JSON form (2-space pretty print, one trailing
    /// newline), mirroring the export report's emission.
    pub fn to_canonical_json(&self) -> String {
        let mut text = serde_json::to_string_pretty(self)
            .expect("serializing a plain-data report is infallible");
        text.push('\n');
        text
    }
}

/// Source container summary carried in the report.
#[derive(Debug, Clone, Serialize)]
pub struct InpSourceSummary {
    /// Total .inx byte length.
    pub inx_bytes: usize,
    /// JSON payload byte length.
    pub payload_bytes: usize,
    /// Texture slot count in TEX_SECT (referenced or not).
    pub texture_slots: usize,
    /// Per-slot inventory (byte length, slot type, TGA header
    /// dimensions): the fidelity baseline for the texture
    /// bytes this stage never reads.
    pub texture_inventory: Vec<InpTextureSlot>,
    /// EXT_SECT entries (name, byte length); contents are not
    /// interpreted by this stage.
    pub ext_sections: Vec<(String, usize)>,
}

/// One texture slot's header inventory.
#[derive(Debug, Clone, Serialize)]
pub struct InpTextureSlot {
    pub index: usize,
    pub byte_len: usize,
    pub kind: u8,
    pub width: Option<u32>,
    pub height: Option<u32>,
}

/// Count reconciliation between the INP source and the native
/// product.
#[derive(Debug, Clone, Serialize)]
pub struct InpCounts {
    pub inp_parameters: usize,
    pub native_parameters: usize,
    pub inp_nodes: usize,
    pub native_parts: usize,
    pub native_meshes: usize,
    pub native_deformers: usize,
    pub physics_groups: usize,
    pub physics_strands: usize,
    pub bindings_total: usize,
    pub deform_bindings: usize,
    pub value_bindings: usize,
    pub kept_lattice_axes: usize,
    pub folded_axes: usize,
}

fn inp_error(message: impl Into<String>) -> ConvertError {
    ConvertError::Inp {
        message: message.into(),
    }
}

// ----------------------------------------------------------------
// Container

/// One TEX_SECT slot header (pixel bytes are never read into
/// any structure; the slot's byte range is validated and its
/// TGA header parsed for dimensions only).
#[derive(Debug, Clone)]
struct TextureSlot {
    byte_len: usize,
    kind: u8,
    width: Option<u32>,
    height: Option<u32>,
}

/// A parsed INP1 container.
struct Container {
    puppet: RawPuppet,
    payload_bytes: usize,
    textures: Vec<TextureSlot>,
    ext_sections: Vec<(String, usize)>,
}

/// Read a big-endian u32 at `offset`, bounds-checked.
fn be_u32(bytes: &[u8], offset: usize, what: &str) -> Result<u32, ConvertError> {
    let end = offset
        .checked_add(4)
        .ok_or_else(|| inp_error(format!("offset overflow reading {what}")))?;
    let slice = bytes.get(offset..end).ok_or_else(|| {
        inp_error(format!(
            "truncated container: {what} at byte {offset} exceeds the {}-byte file",
            bytes.len()
        ))
    })?;
    Ok(u32::from_be_bytes([slice[0], slice[1], slice[2], slice[3]]))
}

/// Parse and validate the INP1 container, returning the JSON
/// payload as typed data plus the texture/extension section
/// inventories. Fails typed on any structural defect; never
/// panics, never guesses a layout.
fn parse_container(bytes: &[u8]) -> Result<Container, ConvertError> {
    if bytes.len() > INP_BYTES_MAX {
        return Err(inp_error(format!(
            "file is {} bytes, over the {INP_BYTES_MAX}-byte bound",
            bytes.len()
        )));
    }
    if bytes.len() < 12 || &bytes[0..8] != b"TRNSRTS\0" {
        return Err(inp_error(
            "bad magic: not an INP1 puppet (expected TRNSRTS\\0)",
        ));
    }
    let payload_len = be_u32(bytes, 8, "payload length")? as usize;
    let payload_end = 12usize
        .checked_add(payload_len)
        .ok_or_else(|| inp_error("payload length overflows the file size"))?;
    let payload = bytes.get(12..payload_end).ok_or_else(|| {
        inp_error(format!(
            "truncated container: payload of {payload_len} bytes at offset 12 exceeds the {}-byte file",
            bytes.len()
        ))
    })?;
    let puppet: RawPuppet = serde_json::from_slice(payload).map_err(|error| {
        inp_error(format!(
            "payload JSON does not match the INP schema: {error}"
        ))
    })?;

    let mut offset = payload_end;
    let magic = bytes
        .get(offset..offset + 8)
        .ok_or_else(|| inp_error("truncated container: TEX_SECT header missing"))?;
    if magic != b"TEX_SECT" {
        return Err(inp_error(format!(
            "expected TEX_SECT at byte {offset}, found {magic:?}"
        )));
    }
    offset += 8;
    let slot_count = be_u32(bytes, offset, "texture slot count")? as usize;
    offset += 4;
    if slot_count > 4096 {
        return Err(inp_error(format!(
            "texture slot count {slot_count} exceeds the 4096 bound"
        )));
    }
    let mut textures = Vec::with_capacity(slot_count);
    for index in 0..slot_count {
        let byte_len = be_u32(bytes, offset, "texture slot length")? as usize;
        offset += 4;
        let kind = *bytes.get(offset).ok_or_else(|| {
            inp_error(format!(
                "truncated container: texture slot {index} type byte"
            ))
        })?;
        offset += 1;
        let blob = bytes.get(offset..offset + byte_len).ok_or_else(|| {
            inp_error(format!(
                "truncated container: texture slot {index} of {byte_len} bytes exceeds the file"
            ))
        })?;
        // TGA header: width/height are little-endian u16 at
        // bytes 12..16. Dimensions are report data only.
        let (width, height) = if blob.len() >= 18 {
            (
                Some(u16::from_le_bytes([blob[12], blob[13]]) as u32),
                Some(u16::from_le_bytes([blob[14], blob[15]]) as u32),
            )
        } else {
            (None, None)
        };
        textures.push(TextureSlot {
            byte_len,
            kind,
            width,
            height,
        });
        offset += byte_len;
    }

    let mut ext_sections = Vec::new();
    if bytes.len() >= offset + 8 && &bytes[offset..offset + 8] == b"EXT_SECT" {
        offset += 8;
        let section_count = be_u32(bytes, offset, "extension section count")? as usize;
        offset += 4;
        if section_count > 256 {
            return Err(inp_error(format!(
                "extension section count {section_count} exceeds the 256 bound"
            )));
        }
        for _ in 0..section_count {
            let name_len = be_u32(bytes, offset, "extension name length")? as usize;
            offset += 4;
            let name_bytes = bytes
                .get(offset..offset + name_len)
                .ok_or_else(|| inp_error("truncated container: extension section name"))?;
            let name = String::from_utf8_lossy(name_bytes).into_owned();
            offset += name_len;
            let data_len = be_u32(bytes, offset, "extension data length")? as usize;
            offset += 4;
            if bytes.get(offset..offset + data_len).is_none() {
                return Err(inp_error(format!(
                    "truncated container: extension section {name:?} data"
                )));
            }
            offset += data_len;
            ext_sections.push((name, data_len));
        }
    }
    if offset != bytes.len() {
        return Err(inp_error(format!(
            "container has {} trailing byte(s) after the last section",
            bytes.len() - offset
        )));
    }
    Ok(Container {
        puppet,
        payload_bytes: payload_len,
        textures,
        ext_sections,
    })
}

// ----------------------------------------------------------------
// Raw payload schema (serde). Unknown fields are ignored;
// required structure is enforced by the conversion pass, which
// reports defects with node/parameter context the raw types
// cannot carry.

#[derive(Debug, Deserialize)]
struct RawPuppet {
    #[serde(default)]
    meta: serde_json::Value,
    physics: RawGlobalPhysics,
    nodes: RawNode,
    param: Vec<RawParam>,
    #[serde(default)]
    automation: Option<serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct RawGlobalPhysics {
    #[serde(rename = "pixelsPerMeter")]
    pixels_per_meter: f32,
    gravity: f32,
}

#[derive(Debug, Deserialize)]
struct RawTransform {
    trans: [f32; 3],
    rot: [f32; 3],
    scale: [f32; 2],
}

#[derive(Debug, Deserialize)]
struct RawMesh {
    /// Flat x/y pairs in the source JSON (the INP serializer
    /// flattens the vertex array); regrouped at decode time.
    verts: Vec<f32>,
    /// Flat u/v pairs, same convention.
    uvs: Vec<f32>,
    indices: Vec<u32>,
    #[serde(default)]
    origin: [f32; 2],
}

/// A decoded mesh: vertex and uv pairs regrouped from the
/// source's flat arrays.
#[derive(Debug, Clone)]
struct MeshData {
    verts: Vec<[f32; 2]>,
    uvs: Vec<[f32; 2]>,
    indices: Vec<u32>,
    origin: [f32; 2],
}

/// Regroup a flat pair array, failing typed on an odd length.
fn regroup_pairs(flat: &[f32], ctx: &str) -> Result<Vec<[f32; 2]>, ConvertError> {
    if !flat.len().is_multiple_of(2) {
        return Err(inp_error(format!(
            "{ctx}: flat pair array has odd length {}",
            flat.len()
        )));
    }
    let mut out = Vec::with_capacity(flat.len() / 2);
    for pair in flat.as_chunks::<2>().0 {
        if !pair[0].is_finite() || !pair[1].is_finite() {
            return Err(inp_error(format!(
                "{ctx}: pair array holds a non-finite value"
            )));
        }
        out.push([pair[0], pair[1]]);
    }
    Ok(out)
}

/// Decode one raw mesh into pairs, cross-checking the arrays.
fn decode_mesh(raw: &RawMesh, part_name: &str) -> Result<MeshData, ConvertError> {
    let ctx = format!("part {part_name:?} mesh");
    let verts = regroup_pairs(&raw.verts, &ctx)?;
    let uvs = regroup_pairs(&raw.uvs, &ctx)?;
    if uvs.len() != verts.len() {
        return Err(inp_error(format!(
            "{ctx}: {} uvs for {} vertices",
            uvs.len(),
            verts.len()
        )));
    }
    if !raw.indices.len().is_multiple_of(3)
        || raw
            .indices
            .iter()
            .any(|index| *index as usize >= verts.len())
    {
        return Err(inp_error(format!(
            "{ctx}: index buffer is not a valid triangle list over {} vertices",
            verts.len()
        )));
    }
    Ok(MeshData {
        verts,
        uvs,
        indices: raw.indices.clone(),
        origin: raw.origin,
    })
}

#[derive(Debug, Clone, Deserialize)]
struct RawMask {
    source: u32,
    mode: String,
}

#[derive(Debug, Deserialize)]
struct RawNode {
    uuid: u32,
    name: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default = "default_true")]
    enabled: bool,
    #[serde(default)]
    zsort: f32,
    transform: RawTransform,
    #[serde(default)]
    children: Vec<RawNode>,
    // Part / Composite fields.
    mesh: Option<RawMesh>,
    #[serde(default = "default_one")]
    opacity: f32,
    #[serde(default)]
    blend_mode: Option<String>,
    #[serde(default)]
    textures: Vec<i32>,
    #[serde(default)]
    masks: Option<Vec<RawMask>>,
    #[serde(default)]
    mask_threshold: Option<f32>,
    #[serde(default)]
    tint: Option<serde_json::Value>,
    #[serde(default)]
    screen_tint: Option<serde_json::Value>,
    // SimplePhysics fields.
    param: Option<u32>,
    model_type: Option<String>,
    map_mode: Option<String>,
    gravity: Option<f32>,
    length: Option<f32>,
    frequency: Option<f32>,
    angle_damping: Option<f32>,
    length_damping: Option<f32>,
    output_scale: Option<[f32; 2]>,
}

fn default_true() -> bool {
    true
}

fn default_one() -> f32 {
    1.0
}

#[derive(Debug, Deserialize)]
struct RawParam {
    uuid: u32,
    name: String,
    is_vec2: bool,
    min: [f32; 2],
    max: [f32; 2],
    defaults: [f32; 2],
    axis_points: [Vec<f32>; 2],
    #[serde(default)]
    bindings: Vec<RawBinding>,
}

#[derive(Debug, Deserialize)]
struct RawBinding {
    node: u32,
    param_name: String,
    values: serde_json::Value,
    #[serde(default, rename = "isSet")]
    is_set: Vec<Vec<bool>>,
    #[serde(default)]
    interpolate_mode: Option<String>,
}

// ----------------------------------------------------------------
// Matrix math. Row-major storage, column-vector application:
// `apply(m, p)` computes `m * [x, y, z, 1]`. Composition
// `mul(a, b)` applies `b` first. All matrices built here are
// affine (bottom row 0 0 0 1); products of affine matrices are
// affine, so the inverse below only handles the affine case
// and says so.

#[derive(Debug, Clone, Copy)]
struct Mat4 {
    m: [[f32; 4]; 4],
}

impl Mat4 {
    fn identity() -> Mat4 {
        let mut m = [[0.0f32; 4]; 4];
        for (i, row) in m.iter_mut().enumerate() {
            row[i] = 1.0;
        }
        Mat4 { m }
    }

    fn mul(self, other: Mat4) -> Mat4 {
        let mut out = [[0.0f32; 4]; 4];
        for (row, out_row) in out.iter_mut().enumerate() {
            for (col, cell) in out_row.iter_mut().enumerate() {
                *cell = (0..4).map(|k| self.m[row][k] * other.m[k][col]).sum();
            }
        }
        Mat4 { m: out }
    }

    fn apply(&self, p: [f32; 3]) -> [f32; 3] {
        let v = [p[0], p[1], p[2], 1.0];
        let mut out = [0.0f32; 3];
        for (row, slot) in out.iter_mut().enumerate() {
            *slot = (0..4).map(|k| self.m[row][k] * v[k]).sum();
        }
        out
    }

    /// Inverse of an affine matrix (linear part via the 3x3
    /// adjugate). `None` when the linear part is singular.
    fn inverse_affine(&self) -> Option<Mat4> {
        let a = &self.m;
        let det = a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1])
            - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0])
            + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0]);
        if !det.is_finite() || det.abs() < 1e-12 {
            return None;
        }
        let inv_det = 1.0 / det;
        let mut lin = [[0.0f32; 3]; 3];
        lin[0][0] = (a[1][1] * a[2][2] - a[1][2] * a[2][1]) * inv_det;
        lin[0][1] = (a[0][2] * a[2][1] - a[0][1] * a[2][2]) * inv_det;
        lin[0][2] = (a[0][1] * a[1][2] - a[0][2] * a[1][1]) * inv_det;
        lin[1][0] = (a[1][2] * a[2][0] - a[1][0] * a[2][2]) * inv_det;
        lin[1][1] = (a[0][0] * a[2][2] - a[0][2] * a[2][0]) * inv_det;
        lin[1][2] = (a[0][2] * a[1][0] - a[0][0] * a[1][2]) * inv_det;
        lin[2][0] = (a[1][0] * a[2][1] - a[1][1] * a[2][0]) * inv_det;
        lin[2][1] = (a[0][1] * a[2][0] - a[0][0] * a[2][1]) * inv_det;
        lin[2][2] = (a[0][0] * a[1][1] - a[0][1] * a[1][0]) * inv_det;
        let t = [a[0][3], a[1][3], a[2][3]];
        let mut out = Mat4::identity();
        for (row, lin_row) in lin.iter().enumerate() {
            for (col, value) in lin_row.iter().enumerate() {
                out.m[row][col] = *value;
            }
            out.m[row][3] = -(0..3).map(|k| lin[row][k] * t[k]).sum::<f32>();
        }
        Some(out)
    }
}

/// A translation/rotation/scale triple (the INP transform
/// shape), before matrix composition.
#[derive(Debug, Clone, Copy)]
struct Trs {
    trans: [f32; 3],
    rot: [f32; 3],
    scale: [f32; 2],
}

impl Trs {
    fn identity() -> Trs {
        Trs {
            trans: [0.0; 3],
            rot: [0.0; 3],
            scale: [1.0, 1.0],
        }
    }

    /// `T * R * S` with `R = Rz * Ry * Rx` (the reference math
    /// library's euler convention, verified in its source:
    /// the quaternion is Qz (x) Qy (x) Qx).
    fn matrix(&self) -> Mat4 {
        let (sx, sy) = (self.scale[0], self.scale[1]);
        let (cx, x_sin) = (self.rot[0].cos(), self.rot[0].sin());
        let (cy, y_sin) = (self.rot[1].cos(), self.rot[1].sin());
        let (cz, z_sin) = (self.rot[2].cos(), self.rot[2].sin());
        // R = Rz * Ry * Rx, expanded once (row-major).
        let r = [
            [
                cz * cy,
                cz * y_sin * x_sin - z_sin * cx,
                cz * y_sin * cx + z_sin * x_sin,
            ],
            [
                z_sin * cy,
                z_sin * y_sin * x_sin + cz * cx,
                z_sin * y_sin * cx - cz * x_sin,
            ],
            [-y_sin, cy * x_sin, cy * cx],
        ];
        let mut out = Mat4::identity();
        for (row, r_row) in r.iter().enumerate() {
            out.m[row][0] = r_row[0] * sx;
            out.m[row][1] = r_row[1] * sy;
            out.m[row][2] = r_row[2];
            out.m[row][3] = self.trans[row];
        }
        out
    }

    /// The reference `calcOffset` merge: translations and
    /// rotations add component-wise, scales multiply.
    fn merged_with_offset(&self, offset: &Trs) -> Trs {
        Trs {
            trans: [
                self.trans[0] + offset.trans[0],
                self.trans[1] + offset.trans[1],
                self.trans[2] + offset.trans[2],
            ],
            rot: [
                self.rot[0] + offset.rot[0],
                self.rot[1] + offset.rot[1],
                self.rot[2] + offset.rot[2],
            ],
            scale: [
                self.scale[0] * offset.scale[0],
                self.scale[1] * offset.scale[1],
            ],
        }
    }
}

// ----------------------------------------------------------------
// Decoded bindings. The JSON grids are shape-checked once, at
// decode time, against the owning parameter's axis points and
// (for deform) the target mesh's vertex count; evaluation after
// that is total.

#[derive(Debug, Clone)]
enum BindingValues {
    /// Scalar grid for a `transform.*` / `zSort` binding,
    /// indexed [axis0][axis1].
    Scalar(Vec<Vec<f32>>),
    /// Per-vertex offset grid for a `deform` binding,
    /// indexed [axis0][axis1], one offset list per point.
    Deform(Vec<Vec<Vec<[f32; 2]>>>),
}

#[derive(Debug, Clone)]
struct Binding {
    /// Index into the flattened node table.
    node: usize,
    param_name: String,
    values: BindingValues,
}

/// One INP parameter, decoded: its axes, defaults, and
/// bindings by target.
#[derive(Debug)]
struct Param {
    uuid: u32,
    name: String,
    is_vec2: bool,
    min: [f32; 2],
    max: [f32; 2],
    defaults: [f32; 2],
    axis_points: [Vec<f32>; 2],
    bindings: Vec<Binding>,
    /// Native scalar ids per axis (axis 1 exists only for
    /// vec2 parameters).
    native_ids: [Option<String>; 2],
    /// True when a SimplePhysics node writes this parameter:
    /// it is a physics output and LRF-207 bars it from every
    /// lattice axis.
    is_physics_output: bool,
}

impl Param {
    /// The parameter's value range on one axis.
    fn range(&self, axis: usize) -> (f32, f32) {
        (self.min[axis], self.max[axis])
    }

    /// Axis key values in parameter space: the grid points a
    /// native lattice would carry for this axis.
    fn axis_keys(&self, axis: usize) -> Vec<f32> {
        let (min, max) = self.range(axis);
        self.axis_points[axis]
            .iter()
            .map(|fraction| min + fraction * (max - min))
            .collect()
    }

    /// The grid segment and local fraction for evaluating this
    /// parameter at `value` on `axis`: the reference driver's
    /// segment search over the normalized axis points.
    fn segment(&self, axis: usize, value: f32) -> (usize, f32) {
        let points = &self.axis_points[axis];
        if points.len() < 2 {
            return (0, 0.0);
        }
        let (min, max) = self.range(axis);
        let normalized = if (max - min).abs() > f32::EPSILON {
            (value - min) / (max - min)
        } else {
            0.0
        };
        let clamped = normalized.clamp(points[0], points[points.len() - 1]);
        let mut index = 0;
        while index + 1 < points.len() - 1 && points[index + 1] < clamped {
            index += 1;
        }
        let span = points[index + 1] - points[index];
        let fraction = if span > f32::EPSILON {
            (clamped - points[index]) / span
        } else {
            0.0
        };
        (index, fraction)
    }
}

/// A pose: one value per INP parameter axis. Evaluation is a
/// pure function of the pose; the default pose is every
/// parameter at its authored default.
type Pose = Vec<[f32; 2]>;

fn default_pose(params: &[Param]) -> Pose {
    params.iter().map(|param| param.defaults).collect()
}

/// Bilinear evaluation of a scalar binding grid at a pose.
fn eval_scalar(param: &Param, values: &[Vec<f32>], pose: &Pose, param_index: usize) -> f32 {
    let value = pose[param_index];
    let (x0, fx) = param.segment(0, value[0]);
    let (y0, fy) = param.segment(1, value[1]);
    let x1 = (x0 + 1).min(values.len() - 1);
    let y1 = (y0 + 1).min(values[0].len() - 1);
    let top = values[x0][y0] * (1.0 - fx) + values[x1][y0] * fx;
    let bottom = values[x0][y1] * (1.0 - fx) + values[x1][y1] * fx;
    top * (1.0 - fy) + bottom * fy
}

/// Bilinear evaluation of a deform binding grid at a pose:
/// one offset per vertex, each interpolated independently
/// (the reference driver's per-point lerp).
fn eval_deform(
    param: &Param,
    values: &[Vec<Vec<[f32; 2]>>],
    pose: &Pose,
    param_index: usize,
) -> Vec<[f32; 2]> {
    let value = pose[param_index];
    let (x0, fx) = param.segment(0, value[0]);
    let (y0, fy) = param.segment(1, value[1]);
    let x1 = (x0 + 1).min(values.len() - 1);
    let y1 = (y0 + 1).min(values[0].len() - 1);
    let corners = [
        &values[x0][y0],
        &values[x1][y0],
        &values[x0][y1],
        &values[x1][y1],
    ];
    let weights = [
        (1.0 - fx) * (1.0 - fy),
        fx * (1.0 - fy),
        (1.0 - fx) * fy,
        fx * fy,
    ];
    let vertex_count = corners[0].len();
    let mut out = vec![[0.0f32; 2]; vertex_count];
    for (corner, weight) in corners.iter().zip(weights.iter()) {
        if *weight == 0.0 {
            continue;
        }
        for (slot, offset) in out.iter_mut().zip(corner.iter()) {
            slot[0] += offset[0] * weight;
            slot[1] += offset[1] * weight;
        }
    }
    out
}

/// Whether a scalar grid actually varies along an axis
/// (exact comparison: the grids are dense evaluated surfaces,
/// so a constant axis is exactly constant).
fn scalar_varies(values: &[Vec<f32>], axis: usize) -> bool {
    if axis == 0 {
        if values.len() < 2 {
            return false;
        }
        values[1..].iter().any(|row| *row != values[0])
    } else {
        values
            .iter()
            .any(|row| row.len() > 1 && row[1..].iter().any(|v| *v != row[0]))
    }
}

/// Whether a deform grid actually varies along an axis.
fn deform_varies(values: &[Vec<Vec<[f32; 2]>>], axis: usize) -> bool {
    if axis == 0 {
        if values.len() < 2 {
            return false;
        }
        values[1..].iter().any(|row| *row != values[0])
    } else {
        values
            .iter()
            .any(|row| row.len() > 1 && row[1..].iter().any(|v| *v != row[0]))
    }
}

fn binding_varies(binding: &Binding, axis: usize) -> bool {
    match &binding.values {
        BindingValues::Scalar(grid) => scalar_varies(grid, axis),
        BindingValues::Deform(grid) => deform_varies(grid, axis),
    }
}

// ----------------------------------------------------------------
// Id minting. Native ids are `[A-Za-z0-9_-]{1,64}` (SPEC
// section 1.3). INP names are display strings ("Head::
// Yaw-Pitch"); the minting rule is deterministic: every
// disallowed character becomes `_`, runs are NOT collapsed,
// leading/trailing `_` are trimmed, an empty result becomes
// "unnamed", and collisions inside one collection take `_2`,
// `_3`, ... in declaration order. Every mint is reported.

/// Sanitize one INP display name into a native id base.
fn sanitize_id(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
            out.push(ch);
        } else {
            out.push('_');
        }
    }
    let trimmed = out.trim_matches('_');
    let base = if trimmed.is_empty() {
        "unnamed"
    } else {
        trimmed
    };
    let mut base = base.to_string();
    if base.len() > 60 {
        base.truncate(60);
    }
    base
}

/// Mint a unique id within one collection.
fn mint_id(base: &str, used: &mut BTreeSet<String>) -> String {
    if used.insert(base.to_string()) {
        return base.to_string();
    }
    for suffix in 2..10000u32 {
        let candidate = format!("{base}_{suffix}");
        if used.insert(candidate.clone()) {
            return candidate;
        }
    }
    // The suffix space is bounded and the input collections
    // are bounded far below it; this is unreachable in
    // practice and preferable to an unbounded loop.
    format!("{base}_x")
}

// ----------------------------------------------------------------
// Flattened node table (DFS pre-order; parents precede
// children, which the world-matrix pass relies on).

#[derive(Debug)]
struct NodeInfo {
    uuid: u32,
    name: String,
    kind: String,
    enabled: bool,
    zsort: f32,
    authored: Trs,
    parent: Option<usize>,
    /// Tree-order index among all nodes.
    index: usize,
    mesh: Option<MeshData>,
    opacity: f32,
    blend_mode: Option<String>,
    texture_slots: Vec<i32>,
    masks: Vec<RawMask>,
    mask_threshold: Option<f32>,
    has_nondefault_tint: bool,
    // SimplePhysics payload (kind == "SimplePhysics").
    physics_param: Option<u32>,
    physics_model_type: Option<String>,
    physics_map_mode: Option<String>,
    physics_gravity: Option<f32>,
    physics_length: Option<f32>,
    physics_frequency: Option<f32>,
    physics_angle_damping: Option<f32>,
    physics_length_damping: Option<f32>,
    physics_output_scale: Option<[f32; 2]>,
}

fn flatten_nodes(root: &RawNode) -> Result<Vec<NodeInfo>, ConvertError> {
    fn push(
        raw: &RawNode,
        parent: Option<usize>,
        out: &mut Vec<NodeInfo>,
    ) -> Result<(), ConvertError> {
        if out.len() >= NODES_MAX {
            return Err(inp_error(format!(
                "node count exceeds the {NODES_MAX} bound"
            )));
        }
        match raw.kind.as_str() {
            "Node" | "Part" | "Composite" | "SimplePhysics" => {}
            other => {
                return Err(inp_error(format!(
                    "node {:?} has unknown type {other:?}",
                    raw.name
                )));
            }
        }
        if raw.kind != "Part" && raw.mesh.is_some() {
            return Err(inp_error(format!(
                "node {:?} of type {} carries a mesh; only Part nodes do",
                raw.name, raw.kind
            )));
        }
        let index = out.len();
        let has_nondefault_tint =
            tint_is_nondefault(raw.tint.as_ref()) || tint_is_nondefault(raw.screen_tint.as_ref());
        let mesh = match &raw.mesh {
            Some(raw_mesh) => Some(decode_mesh(raw_mesh, &raw.name)?),
            None => None,
        };
        out.push(NodeInfo {
            uuid: raw.uuid,
            name: raw.name.clone(),
            kind: raw.kind.clone(),
            enabled: raw.enabled,
            zsort: raw.zsort,
            authored: Trs {
                trans: raw.transform.trans,
                rot: raw.transform.rot,
                scale: raw.transform.scale,
            },
            parent,
            index,
            mesh,
            opacity: raw.opacity,
            blend_mode: raw.blend_mode.clone(),
            texture_slots: raw.textures.clone(),
            masks: raw.masks.clone().unwrap_or_default(),
            mask_threshold: raw.mask_threshold,
            has_nondefault_tint,
            physics_param: raw.param,
            physics_model_type: raw.model_type.clone(),
            physics_map_mode: raw.map_mode.clone(),
            physics_gravity: raw.gravity,
            physics_length: raw.length,
            physics_frequency: raw.frequency,
            physics_angle_damping: raw.angle_damping,
            physics_length_damping: raw.length_damping,
            physics_output_scale: raw.output_scale,
        });
        for child in &raw.children {
            push(child, Some(index), out)?;
        }
        Ok(())
    }
    let mut out = Vec::new();
    push(root, None, &mut out)?;
    let mut seen = BTreeSet::new();
    for node in &out {
        if !seen.insert(node.uuid) {
            return Err(inp_error(format!(
                "duplicate node uuid {} ({:?})",
                node.uuid, node.name
            )));
        }
    }
    Ok(out)
}

/// A tint is non-default when it is a 3-component value other
/// than (1, 1, 1) for `tint` or (0, 0, 0) for `screenTint`.
/// INP serializes both as either a scalar or a 3-array
/// depending on the writer's mood; both shapes are read.
fn tint_is_nondefault(value: Option<&serde_json::Value>) -> bool {
    let Some(value) = value else {
        return false;
    };
    let components: Vec<f32> = match value {
        serde_json::Value::Number(number) => {
            vec![number.as_f64().unwrap_or(0.0) as f32; 3]
        }
        serde_json::Value::Array(items) => items
            .iter()
            .map(|item| item.as_f64().unwrap_or(0.0) as f32)
            .collect(),
        _ => return true,
    };
    !(components == [1.0, 1.0, 1.0] || components == [0.0, 0.0, 0.0])
}

// ----------------------------------------------------------------
// Parameter and binding decoding.

/// Decode one scalar binding grid, shape-checked against the
/// parameter's axis points.
fn decode_scalar_grid(
    value: &serde_json::Value,
    axis_points: &[Vec<f32>; 2],
    ctx: &str,
) -> Result<Vec<Vec<f32>>, ConvertError> {
    let rows = value
        .as_array()
        .ok_or_else(|| inp_error(format!("{ctx}: binding values are not an array")))?;
    if rows.len() != axis_points[0].len() {
        return Err(inp_error(format!(
            "{ctx}: binding grid has {} rows, parameter axis 0 has {} points",
            rows.len(),
            axis_points[0].len()
        )));
    }
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let cells = row
            .as_array()
            .ok_or_else(|| inp_error(format!("{ctx}: binding grid row is not an array")))?;
        if cells.len() != axis_points[1].len() {
            return Err(inp_error(format!(
                "{ctx}: binding grid row has {} cells, parameter axis 1 has {} points",
                cells.len(),
                axis_points[1].len()
            )));
        }
        let mut decoded = Vec::with_capacity(cells.len());
        for cell in cells {
            let number = cell
                .as_f64()
                .ok_or_else(|| inp_error(format!("{ctx}: binding grid cell is not a number")))?
                as f32;
            if !number.is_finite() {
                return Err(inp_error(format!(
                    "{ctx}: binding grid holds a non-finite value"
                )));
            }
            decoded.push(number);
        }
        out.push(decoded);
    }
    Ok(out)
}

/// Decode one deform binding grid, shape-checked against the
/// parameter's axis points and the target mesh's vertex count.
fn decode_deform_grid(
    value: &serde_json::Value,
    axis_points: &[Vec<f32>; 2],
    vertex_count: usize,
    ctx: &str,
) -> Result<Vec<Vec<Vec<[f32; 2]>>>, ConvertError> {
    let rows = value
        .as_array()
        .ok_or_else(|| inp_error(format!("{ctx}: deform values are not an array")))?;
    if rows.len() != axis_points[0].len() {
        return Err(inp_error(format!(
            "{ctx}: deform grid has {} rows, parameter axis 0 has {} points",
            rows.len(),
            axis_points[0].len()
        )));
    }
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        let cells = row
            .as_array()
            .ok_or_else(|| inp_error(format!("{ctx}: deform grid row is not an array")))?;
        if cells.len() != axis_points[1].len() {
            return Err(inp_error(format!(
                "{ctx}: deform grid row has {} cells, parameter axis 1 has {} points",
                cells.len(),
                axis_points[1].len()
            )));
        }
        let mut decoded_row = Vec::with_capacity(cells.len());
        for cell in cells {
            let offsets = cell
                .as_array()
                .ok_or_else(|| inp_error(format!("{ctx}: deform grid point is not an array")))?;
            if offsets.len() != vertex_count {
                return Err(inp_error(format!(
                    "{ctx}: deform grid point holds {} offsets, target mesh has {vertex_count} vertices",
                    offsets.len()
                )));
            }
            let mut decoded = Vec::with_capacity(vertex_count);
            for offset in offsets {
                let pair = offset
                    .as_array()
                    .ok_or_else(|| inp_error(format!("{ctx}: deform offset is not a pair")))?;
                if pair.len() != 2 {
                    return Err(inp_error(format!(
                        "{ctx}: deform offset has {} components, expected 2",
                        pair.len()
                    )));
                }
                let x = pair[0]
                    .as_f64()
                    .ok_or_else(|| inp_error(format!("{ctx}: deform offset x is not a number")))?
                    as f32;
                let y = pair[1]
                    .as_f64()
                    .ok_or_else(|| inp_error(format!("{ctx}: deform offset y is not a number")))?
                    as f32;
                if !x.is_finite() || !y.is_finite() {
                    return Err(inp_error(format!(
                        "{ctx}: deform offset holds a non-finite value"
                    )));
                }
                decoded.push([x, y]);
            }
            decoded_row.push(decoded);
        }
        out.push(decoded_row);
    }
    Ok(out)
}

/// Count unset editor keypoints in an `isSet` grid, validating
/// its shape when present. An absent grid counts as zero (the
/// field is optional in the source format).
fn count_unset(
    is_set: &[Vec<bool>],
    axis_points: &[Vec<f32>; 2],
    ctx: &str,
) -> Result<usize, ConvertError> {
    if is_set.is_empty() {
        return Ok(0);
    }
    if is_set.len() != axis_points[0].len()
        || is_set.iter().any(|row| row.len() != axis_points[1].len())
    {
        return Err(inp_error(format!(
            "{ctx}: isSet grid shape does not match the parameter's axis points"
        )));
    }
    Ok(is_set.iter().flatten().filter(|set| !**set).count())
}

/// Validate one parameter's scalar fields and axis points.
fn validate_param(raw: &RawParam) -> Result<(), ConvertError> {
    if raw.is_vec2 {
        // Both axes are real.
    }
    for axis in 0..2 {
        let (min, max, default) = (raw.min[axis], raw.max[axis], raw.defaults[axis]);
        if !min.is_finite() || !max.is_finite() || !default.is_finite() {
            return Err(inp_error(format!(
                "parameter {:?} axis {axis} holds a non-finite range value",
                raw.name
            )));
        }
        if min > max {
            return Err(inp_error(format!(
                "parameter {:?} axis {axis} has min {min} above max {max}",
                raw.name
            )));
        }
        if default < min || default > max {
            return Err(inp_error(format!(
                "parameter {:?} axis {axis} default {default} lies outside [{min}, {max}]",
                raw.name
            )));
        }
        let points = &raw.axis_points[axis];
        if points.is_empty() {
            return Err(inp_error(format!(
                "parameter {:?} axis {axis} has no axis points",
                raw.name
            )));
        }
        for pair in points.windows(2) {
            if pair[1] <= pair[0] {
                return Err(inp_error(format!(
                    "parameter {:?} axis {axis} points are not strictly increasing",
                    raw.name
                )));
            }
        }
        for point in points {
            if !point.is_finite() || *point < 0.0 || *point > 1.0 {
                return Err(inp_error(format!(
                    "parameter {:?} axis {axis} point {point} lies outside [0, 1]",
                    raw.name
                )));
            }
        }
    }
    Ok(())
}

// ----------------------------------------------------------------
// Conversion.

/// One candidate lattice axis: a (parameter, axis) pair with
/// its measured impact (the maximum deviation the axis causes
/// anywhere on its grid, in model units).
#[derive(Debug, Clone, Copy)]
struct AxisCandidate {
    param: usize,
    axis: usize,
    impact: f32,
}

/// Rank candidates: impact descending, then source order
/// (parameter declaration, axis) ascending, so the choice is
/// deterministic and the strongest drivers win the two
/// lattice slots.
fn rank_axes(mut candidates: Vec<AxisCandidate>) -> Vec<AxisCandidate> {
    candidates.sort_by(|a, b| {
        b.impact
            .partial_cmp(&a.impact)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.param.cmp(&b.param))
            .then(a.axis.cmp(&b.axis))
    });
    candidates
}

/// Convert one INP puppet (the whole .inx byte string) into a
/// validated native model plus its conversion report.
/// `model_id` is the caller's choice (the CLI uses the .inx
/// file stem); it must satisfy the native id rules.
pub fn convert_inp(bytes: &[u8], model_id: &str) -> Result<InpOutcome, ConvertError> {
    let container = parse_container(bytes)?;
    let puppet = &container.puppet;
    let nodes = flatten_nodes(&puppet.nodes)?;
    let mut node_by_uuid: BTreeMap<u32, usize> = BTreeMap::new();
    for node in &nodes {
        node_by_uuid.insert(node.uuid, node.index);
    }

    // --- parameters and bindings -----------------------------
    if puppet.param.len() > INP_PARAMETERS_MAX {
        return Err(inp_error(format!(
            "parameter count {} exceeds the {INP_PARAMETERS_MAX} bound",
            puppet.param.len()
        )));
    }
    let mut params: Vec<Param> = Vec::with_capacity(puppet.param.len());
    let mut total_bindings = 0usize;
    for raw in &puppet.param {
        validate_param(raw)?;
        let bindings_len = raw.bindings.len();
        total_bindings += bindings_len;
        params.push(Param {
            uuid: raw.uuid,
            name: raw.name.clone(),
            is_vec2: raw.is_vec2,
            min: raw.min,
            max: raw.max,
            defaults: raw.defaults,
            axis_points: raw.axis_points.clone(),
            bindings: Vec::new(),
            native_ids: [None, None],
            is_physics_output: false,
        });
    }
    if total_bindings > BINDINGS_MAX {
        return Err(inp_error(format!(
            "binding count {total_bindings} exceeds the {BINDINGS_MAX} bound"
        )));
    }
    // Physics outputs are known from the node table: every
    // SimplePhysics node's driven parameter.
    let mut driven_by: BTreeMap<u32, usize> = BTreeMap::new();
    for node in &nodes {
        if node.kind == "SimplePhysics" {
            let param_uuid = node.physics_param.ok_or_else(|| {
                inp_error(format!(
                    "SimplePhysics node {:?} names no driven parameter",
                    node.name
                ))
            })?;
            if driven_by.insert(param_uuid, node.index).is_some() {
                return Err(inp_error(format!(
                    "parameter uuid {param_uuid} is driven by two SimplePhysics nodes; the native rule is one writer per output parameter (LRF-601)"
                )));
            }
        }
    }
    for param_uuid in driven_by.keys() {
        let index = params
            .iter()
            .position(|param| param.uuid == *param_uuid)
            .ok_or_else(|| {
                inp_error(format!(
                    "a SimplePhysics node drives parameter uuid {param_uuid}, which does not exist"
                ))
            })?;
        params[index].is_physics_output = true;
    }
    // Native scalar ids, minted per parameter in declaration
    // order (one collection, one used-set).
    let mut used_param_ids: BTreeSet<String> = BTreeSet::new();
    let mut id_notes: Vec<String> = Vec::new();
    for param in &mut params {
        let base = sanitize_id(&param.name);
        let axis_count = if param.is_vec2 { 2 } else { 1 };
        for axis in 0..axis_count {
            let wanted = if param.is_vec2 {
                format!("{base}_{}", if axis == 0 { "x" } else { "y" })
            } else {
                base.clone()
            };
            let minted = mint_id(&wanted, &mut used_param_ids);
            if minted != wanted {
                id_notes.push(format!(
                    "parameter {:?} axis {axis}: id {wanted:?} collided, minted {minted:?}",
                    param.name
                ));
            }
            param.native_ids[axis] = Some(minted);
        }
    }
    // Decode bindings now that targets and grids can be
    // shape-checked.
    let mut deform_count = 0usize;
    let mut value_count = 0usize;
    let mut unset_points_total = 0usize;
    let mut bindings_with_unset = 0usize;
    let mut non_linear_modes: Vec<String> = Vec::new();
    for raw in &puppet.param {
        let param_index = params
            .iter()
            .position(|param| param.uuid == raw.uuid)
            .expect("parameters were built from the same list");
        for raw_binding in &raw.bindings {
            let ctx = format!(
                "parameter {:?} binding ({})",
                raw.name, raw_binding.param_name
            );
            let node_index = *node_by_uuid.get(&raw_binding.node).ok_or_else(|| {
                inp_error(format!(
                    "{ctx}: targets unknown node uuid {}",
                    raw_binding.node
                ))
            })?;
            let mode = raw_binding
                .interpolate_mode
                .clone()
                .unwrap_or_else(|| "Linear".to_string());
            match mode.as_str() {
                "Linear" | "Cubic" | "Nearest" => {}
                other => {
                    return Err(inp_error(format!(
                        "{ctx}: unknown interpolate_mode {other:?}"
                    )));
                }
            }
            if mode != "Linear" {
                non_linear_modes.push(format!("{ctx}: {mode}"));
            }
            let unset = count_unset(&raw_binding.is_set, &raw.axis_points, &ctx)?;
            if unset > 0 {
                bindings_with_unset += 1;
                unset_points_total += unset;
            }
            let values = if raw_binding.param_name == "deform" {
                let target = &nodes[node_index];
                if target.kind != "Part" {
                    return Err(inp_error(format!(
                        "{ctx}: deform target {:?} is a {}, not a Part",
                        target.name, target.kind
                    )));
                }
                let mesh = target.mesh.as_ref().ok_or_else(|| {
                    inp_error(format!(
                        "{ctx}: deform target {:?} has no mesh",
                        target.name
                    ))
                })?;
                deform_count += 1;
                BindingValues::Deform(decode_deform_grid(
                    &raw_binding.values,
                    &raw.axis_points,
                    mesh.verts.len(),
                    &ctx,
                )?)
            } else if raw_binding.param_name == "zSort"
                || raw_binding.param_name.starts_with("transform.")
            {
                match raw_binding.param_name.as_str() {
                    "transform.t.x" | "transform.t.y" | "transform.t.z" | "transform.r.x"
                    | "transform.r.y" | "transform.r.z" | "transform.s.x" | "transform.s.y"
                    | "zSort" => {}
                    other => {
                        return Err(inp_error(format!("{ctx}: unknown value binding {other:?}")));
                    }
                }
                value_count += 1;
                BindingValues::Scalar(decode_scalar_grid(
                    &raw_binding.values,
                    &raw.axis_points,
                    &ctx,
                )?)
            } else {
                return Err(inp_error(format!(
                    "{ctx}: unknown binding kind {:?}",
                    raw_binding.param_name
                )));
            };
            params[param_index].bindings.push(Binding {
                node: node_index,
                param_name: raw_binding.param_name.clone(),
                values,
            });
        }
    }
    // Binding lookup tables by target node.
    let mut scalar_by_node: Vec<Vec<(usize, usize)>> = vec![Vec::new(); nodes.len()];
    let mut deform_by_node: Vec<Vec<(usize, usize)>> = vec![Vec::new(); nodes.len()];
    for (param_index, param) in params.iter().enumerate() {
        for (binding_index, binding) in param.bindings.iter().enumerate() {
            match &binding.values {
                BindingValues::Scalar(_) => {
                    scalar_by_node[binding.node].push((param_index, binding_index));
                }
                BindingValues::Deform(_) => {
                    deform_by_node[binding.node].push((param_index, binding_index));
                }
            }
        }
    }

    let default = default_pose(&params);
    // The accumulated driven offset of one node at one pose:
    // (offset transform, zSort offset). Physics-sourced and
    // folded bindings are included at whatever the pose holds
    // for their parameters — callers keep non-kept parameters
    // at default, which is exactly the fold semantics.
    let accumulate = |node_index: usize, pose: &Pose| -> (Trs, f32) {
        let mut offset = Trs::identity();
        let mut z = 0.0f32;
        for &(param_index, binding_index) in &scalar_by_node[node_index] {
            let param = &params[param_index];
            let binding = &param.bindings[binding_index];
            let BindingValues::Scalar(grid) = &binding.values else {
                continue;
            };
            let value = eval_scalar(param, grid, pose, param_index);
            match binding.param_name.as_str() {
                "transform.t.x" => offset.trans[0] += value,
                "transform.t.y" => offset.trans[1] += value,
                "transform.t.z" => offset.trans[2] += value,
                "transform.r.x" => offset.rot[0] += value,
                "transform.r.y" => offset.rot[1] += value,
                "transform.r.z" => offset.rot[2] += value,
                "transform.s.x" => offset.scale[0] *= value,
                "transform.s.y" => offset.scale[1] *= value,
                "zSort" => z += value,
                _ => {}
            }
        }
        (offset, z)
    };
    let merged_matrix = |node_index: usize, pose: &Pose| -> Mat4 {
        let (offset, _) = accumulate(node_index, pose);
        nodes[node_index]
            .authored
            .merged_with_offset(&offset)
            .matrix()
    };

    // --- statics: effective transforms, worlds, zsort ---------
    let mut world: Vec<Mat4> = Vec::with_capacity(nodes.len());
    let mut zsort_world: Vec<f32> = Vec::with_capacity(nodes.len());
    for node in &nodes {
        let effective = merged_matrix(node.index, &default);
        let composed = match node.parent {
            Some(parent) => world[parent].mul(effective),
            None => effective,
        };
        world.push(composed);
        let (_, z_offset) = accumulate(node.index, &default);
        let base = node.parent.map(|parent| zsort_world[parent]).unwrap_or(0.0);
        zsort_world.push(base + node.zsort + z_offset);
    }
    let effective_static: Vec<Mat4> = nodes
        .iter()
        .map(|node| merged_matrix(node.index, &default))
        .collect();

    // --- rest state -------------------------------------------
    // The native rest form of a mesh: authored chain plus every
    // deform binding at the default pose (folded and kept
    // alike — the kept lattice's default form reproduces this
    // state exactly, since form evaluation includes all
    // bindings).
    let mesh_pose_verts = |node_index: usize, pose: &Pose| -> Vec<[f32; 3]> {
        let node = &nodes[node_index];
        let mesh = node.mesh.as_ref().expect("meshes iterate meshed parts");
        let mut offsets = vec![[0.0f32; 2]; mesh.verts.len()];
        for &(param_index, binding_index) in &deform_by_node[node_index] {
            let param = &params[param_index];
            let binding = &param.bindings[binding_index];
            let BindingValues::Deform(grid) = &binding.values else {
                continue;
            };
            let evaluated = eval_deform(param, grid, pose, param_index);
            for (slot, offset) in offsets.iter_mut().zip(evaluated.iter()) {
                slot[0] += offset[0];
                slot[1] += offset[1];
            }
        }
        mesh.verts
            .iter()
            .zip(offsets.iter())
            .map(|(vertex, offset)| {
                world[node_index].apply([
                    vertex[0] + mesh.origin[0] + offset[0],
                    vertex[1] + mesh.origin[1] + offset[1],
                    0.0,
                ])
            })
            .collect()
    };
    let mut max_abs_z = 0.0f32;
    let mut rest_verts: Vec<Option<Vec<[f32; 2]>>> = Vec::with_capacity(nodes.len());
    let mut bounds: Option<([f32; 2], [f32; 2])> = None;
    for node in &nodes {
        if node.mesh.is_none() {
            rest_verts.push(None);
            continue;
        }
        let verts3 = mesh_pose_verts(node.index, &default);
        let mut verts = Vec::with_capacity(verts3.len());
        for point in &verts3 {
            max_abs_z = max_abs_z.max(point[2].abs());
            let (min, max) = bounds.get_or_insert(([point[0], point[1]], [point[0], point[1]]));
            min[0] = min[0].min(point[0]);
            min[1] = min[1].min(point[1]);
            max[0] = max[0].max(point[0]);
            max[1] = max[1].max(point[1]);
            verts.push([point[0], point[1]]);
        }
        rest_verts.push(Some(verts));
    }
    let (bounds_min, bounds_max) = bounds.ok_or_else(|| {
        inp_error("the puppet has no meshed parts; there is no drawable model to convert")
    })?;
    let pad = [
        (bounds_max[0] - bounds_min[0]) * 0.05 + 1.0,
        (bounds_max[1] - bounds_min[1]) * 0.05 + 1.0,
    ];
    let corners: [[f32; 2]; 4] = [
        [bounds_min[0] - pad[0], bounds_min[1] - pad[1]],
        [bounds_max[0] + pad[0], bounds_min[1] - pad[1]],
        [bounds_min[0] - pad[0], bounds_max[1] + pad[1]],
        [bounds_max[0] + pad[0], bounds_max[1] + pad[1]],
    ];
    let corners3: [[f32; 3]; 4] = corners.map(|corner| [corner[0], corner[1], 0.0]);

    let mut lossy: Vec<ReportEntry> = Vec::new();
    let mut no_map: Vec<ReportEntry> = Vec::new();
    let mut synthesized: Vec<ReportEntry> = Vec::new();
    let mut notes: Vec<String> = Vec::new();
    let mut kept_axes_total = 0usize;
    let mut folded_axes_total = 0usize;
    notes.extend(id_notes);
    if max_abs_z > 0.0 {
        lossy.push(ReportEntry {
            item: "out-of-plane depth (node transform z)".to_string(),
            detail: format!(
                "source vertices are planar; depth enters through node transforms (trans.z, rot.x, rot.y) and the composed world rest positions carry |z| up to {max_abs_z}; the native format is planar and the outbound renderer projects orthographically, so z is projected away (MAPPING.md section 14, projection row)"
            ),
        });
    }

    // --- driven nodes -> warp deformers ------------------------
    // A node's warp exists when at least one of its transform
    // axes survives ranking. Folded transform bindings are
    // named individually.
    let mut warp_id_by_node: Vec<Option<String>> = vec![None; nodes.len()];
    let mut deformers: Vec<Deformer> = Vec::new();
    let mut used_deformer_ids: BTreeSet<String> = BTreeSet::new();
    // First pass: decide driven-ness and kept axes per node.
    let mut kept_by_node: Vec<Vec<AxisCandidate>> = vec![Vec::new(); nodes.len()];
    for node in &nodes {
        // Physics-sourced transform bindings fold wholesale;
        // they are reported with the physics pass below.
        let mut varying: BTreeSet<(usize, usize)> = BTreeSet::new();
        for &(param_index, binding_index) in &scalar_by_node[node.index] {
            let param = &params[param_index];
            if param.is_physics_output {
                continue;
            }
            let binding = &param.bindings[binding_index];
            if !binding.param_name.starts_with("transform.") {
                continue;
            }
            for (axis, native_id) in param.native_ids.iter().enumerate() {
                if native_id.is_some() && binding_varies(binding, axis) {
                    varying.insert((param_index, axis));
                }
            }
        }
        let mut candidates: Vec<AxisCandidate> = varying
            .into_iter()
            .map(|(param, axis)| AxisCandidate {
                param,
                axis,
                impact: 0.0,
            })
            .collect();
        if candidates.is_empty() {
            continue;
        }
        let parent_world = node
            .parent
            .map(|parent| world[parent])
            .unwrap_or_else(Mat4::identity);
        let parent_inverse = parent_world.inverse_affine().ok_or_else(|| {
            inp_error(format!(
                "node {:?}: parent world transform is singular",
                node.name
            ))
        })?;
        let effective_inverse = effective_static[node.index]
            .inverse_affine()
            .ok_or_else(|| {
                inp_error(format!(
                    "node {:?}: effective static transform is singular",
                    node.name
                ))
            })?;
        for candidate in &mut candidates {
            let param = &params[candidate.param];
            let mut impact = 0.0f32;
            for key in param.axis_keys(candidate.axis) {
                let mut pose = default.clone();
                pose[candidate.param][candidate.axis] = key;
                let merged = merged_matrix(node.index, &pose);
                let map = parent_world
                    .mul(merged)
                    .mul(effective_inverse)
                    .mul(parent_inverse);
                for corner in &corners3 {
                    let moved = map.apply(*corner);
                    let delta =
                        ((moved[0] - corner[0]).powi(2) + (moved[1] - corner[1]).powi(2)).sqrt();
                    impact = impact.max(delta);
                }
            }
            candidate.impact = impact;
        }
        let ranked = rank_axes(candidates);
        let kept: Vec<AxisCandidate> = ranked.iter().take(2).copied().collect();
        for folded in ranked.iter().skip(2) {
            folded_axes_total += 1;
            lossy.push(ReportEntry {
                item: format!(
                    "node {:?} transform axis {} (parameter {:?})",
                    node.name,
                    if folded.axis == 0 { "x" } else { "y" },
                    params[folded.param].name
                ),
                detail: format!(
                    "axis exceeds the two-axis lattice limit and is folded at its default value; measured impact {:.4} model units (MAPPING.md sections 3.4/14 lattice bound)",
                    folded.impact
                ),
            });
        }
        if !kept.is_empty() {
            kept_axes_total += kept.len();
            kept_by_node[node.index] = kept;
        }
    }
    // Second pass: build the warps in tree order so parents
    // exist before children reference them.
    for node in &nodes {
        let kept = &kept_by_node[node.index];
        if kept.is_empty() {
            continue;
        }
        let base = format!("{}__warp", sanitize_id(&node.name));
        let warp_id = mint_id(&base, &mut used_deformer_ids);
        let parent = nearest_driven_ancestor(&nodes, warp_id_by_node.as_slice(), node.index);
        let parent_world = node
            .parent
            .map(|ancestor| world[ancestor])
            .unwrap_or_else(Mat4::identity);
        let parent_inverse = parent_world
            .inverse_affine()
            .expect("singularity was checked in the ranking pass");
        let effective_inverse = effective_static[node.index]
            .inverse_affine()
            .expect("singularity was checked in the ranking pass");
        let axes: Vec<String> = kept
            .iter()
            .map(|candidate| {
                params[candidate.param].native_ids[candidate.axis]
                    .clone()
                    .expect("kept axes have native ids")
            })
            .collect();
        let keys: Vec<Vec<f32>> = kept
            .iter()
            .map(|candidate| params[candidate.param].axis_keys(candidate.axis))
            .collect();
        let mut forms: Vec<WarpForm> = Vec::new();
        let key_counts: Vec<usize> = keys.iter().map(Vec::len).collect();
        let combos = lattice_combos(&key_counts);
        for combo in &combos {
            let mut pose = default.clone();
            apply_combo(&mut pose, kept, &keys, combo);
            let merged = merged_matrix(node.index, &pose);
            let map = parent_world
                .mul(merged)
                .mul(effective_inverse)
                .mul(parent_inverse);
            forms.push(WarpForm {
                at: combo.clone(),
                positions: corners3
                    .iter()
                    .map(|corner| {
                        let moved = map.apply(*corner);
                        [moved[0], moved[1]]
                    })
                    .collect(),
            });
        }
        deformers.push(Deformer::Warp(WarpDeformer {
            id: warp_id.clone(),
            parent,
            columns: 1,
            rows: 1,
            rest_positions: corners.to_vec(),
            keyforms: Some(Lattice { axes, keys, forms }),
        }));
        warp_id_by_node[node.index] = Some(warp_id);
    }

    // --- parts and meshes ---------------------------------------
    // Part ids are minted first: masks and physics anchors
    // cross-reference them.
    let mut part_id_by_node: Vec<Option<String>> = vec![None; nodes.len()];
    let mut used_part_ids: BTreeSet<String> = BTreeSet::new();
    for node in &nodes {
        if node.kind != "Part" {
            continue;
        }
        let wanted = sanitize_id(&node.name);
        let minted = mint_id(&wanted, &mut used_part_ids);
        if minted != wanted {
            notes.push(format!(
                "part {:?}: id {wanted:?} collided, minted {minted:?}",
                node.name
            ));
        }
        part_id_by_node[node.index] = Some(minted);
    }
    let native_parent_part = |index: usize| -> Option<String> {
        let mut cursor = nodes[index].parent;
        while let Some(current) = cursor {
            if let Some(id) = &part_id_by_node[current] {
                return Some(id.clone());
            }
            cursor = nodes[current].parent;
        }
        None
    };
    let mut parts: Vec<Part> = Vec::new();
    for node in &nodes {
        if node.kind != "Part" {
            continue;
        }
        parts.push(Part {
            id: part_id_by_node[node.index].clone().expect("minted above"),
            parent: native_parent_part(node.index),
            opacity: node.opacity,
            translation_keyforms: None,
        });
    }
    // Rest draw order (authored zsort at default, ties broken
    // by tree order): the order ClipToLower clips against.
    let mut rest_order: Vec<usize> = nodes
        .iter()
        .filter(|node| node.mesh.is_some())
        .map(|node| node.index)
        .collect();
    rest_order.sort_by(|a, b| {
        zsort_world[*a]
            .partial_cmp(&zsort_world[*b])
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.cmp(b))
    });
    let mut rest_predecessor: BTreeMap<usize, usize> = BTreeMap::new();
    for pair in rest_order.windows(2) {
        rest_predecessor.insert(pair[1], pair[0]);
    }
    let mut draw_order_clamps = 0usize;
    let mut meshes: Vec<Mesh> = Vec::new();
    for node in &nodes {
        let Some(mesh) = &node.mesh else {
            continue;
        };
        let part_id = part_id_by_node[node.index]
            .clone()
            .expect("meshed nodes are parts");
        // Deform axis selection for this mesh.
        let mut varying: BTreeSet<(usize, usize)> = BTreeSet::new();
        for &(param_index, binding_index) in &deform_by_node[node.index] {
            let param = &params[param_index];
            if param.is_physics_output {
                // Physics-driven deform bindings fold at
                // default, individually named.
                let binding = &param.bindings[binding_index];
                let BindingValues::Deform(grid) = &binding.values else {
                    continue;
                };
                let mut max_offset = 0.0f32;
                for row in grid {
                    for point in row {
                        for offset in point {
                            max_offset =
                                max_offset.max((offset[0].powi(2) + offset[1].powi(2)).sqrt());
                        }
                    }
                }
                folded_axes_total += 1;
                lossy.push(ReportEntry {
                    item: format!(
                        "part {:?} deform binding from physics parameter {:?}",
                        node.name, param.name
                    ),
                    detail: format!(
                        "folded at the default pose: a physics output may not be a lattice axis (LRF-207, SPEC section 3.7); the default-pose offset is carried in the rest form and the pose-varying part (max |offset| {max_offset:.4}) has no native carrier at this hop"
                    ),
                });
                continue;
            }
            let binding = &param.bindings[binding_index];
            for (axis, native_id) in params[param_index].native_ids.iter().enumerate() {
                if native_id.is_some() && binding_varies(binding, axis) {
                    varying.insert((param_index, axis));
                }
            }
        }
        let mut candidates: Vec<AxisCandidate> = varying
            .into_iter()
            .map(|(param, axis)| AxisCandidate {
                param,
                axis,
                impact: 0.0,
            })
            .collect();
        for candidate in &mut candidates {
            let param = &params[candidate.param];
            let mut impact = 0.0f32;
            for &(param_index, binding_index) in &deform_by_node[node.index] {
                if param_index != candidate.param {
                    continue;
                }
                let binding = &param.bindings[binding_index];
                let BindingValues::Deform(grid) = &binding.values else {
                    continue;
                };
                for (i, row) in grid.iter().enumerate() {
                    for (j, _) in row.iter().enumerate() {
                        let mut at_point = default.clone();
                        at_point[candidate.param] = [param.axis_keys(0)[i], param.axis_keys(1)[j]];
                        let mut at_default_axis = at_point.clone();
                        at_default_axis[candidate.param][candidate.axis] =
                            param.defaults[candidate.axis];
                        let full = eval_deform(param, grid, &at_point, candidate.param);
                        let base = eval_deform(param, grid, &at_default_axis, candidate.param);
                        for (a, b) in full.iter().zip(base.iter()) {
                            let delta = ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt();
                            impact = impact.max(delta);
                        }
                    }
                }
            }
            candidate.impact = impact;
        }
        let ranked = rank_axes(candidates);
        let kept: Vec<AxisCandidate> = ranked.iter().take(2).copied().collect();
        for folded in ranked.iter().skip(2) {
            folded_axes_total += 1;
            lossy.push(ReportEntry {
                item: format!(
                    "part {:?} deform axis {} (parameter {:?})",
                    node.name,
                    if folded.axis == 0 { "x" } else { "y" },
                    params[folded.param].name
                ),
                detail: format!(
                    "axis exceeds the two-axis lattice limit and is folded at its default value; measured impact {:.4} model units; its default-pose offset is carried in the rest form",
                    folded.impact
                ),
            });
        }
        kept_axes_total += kept.len();
        // The lattice, when any axis survives.
        let keyforms = if kept.is_empty() {
            None
        } else {
            let axes: Vec<String> = kept
                .iter()
                .map(|candidate| {
                    params[candidate.param].native_ids[candidate.axis]
                        .clone()
                        .expect("kept axes have native ids")
                })
                .collect();
            let keys: Vec<Vec<f32>> = kept
                .iter()
                .map(|candidate| params[candidate.param].axis_keys(candidate.axis))
                .collect();
            let key_counts: Vec<usize> = keys.iter().map(Vec::len).collect();
            let combos = lattice_combos(&key_counts);
            let form_verts = combos.len() * mesh.verts.len();
            if form_verts > 4_000_000 {
                return Err(inp_error(format!(
                    "part {:?}: lattice would hold {form_verts} form-vertices, over the construction bound",
                    node.name
                )));
            }
            let mut forms: Vec<MeshForm> = Vec::with_capacity(combos.len());
            for combo in &combos {
                let mut pose = default.clone();
                apply_combo(&mut pose, &kept, &keys, combo);
                let verts3 = mesh_pose_verts(node.index, &pose);
                forms.push(MeshForm {
                    at: combo.clone(),
                    vertices: verts3.iter().map(|point| [point[0], point[1]]).collect(),
                });
            }
            Some(Lattice { axes, keys, forms })
        };
        // Masks (MAPPING.md section 6.3).
        let mut clipping_mask_ids: Vec<String> = Vec::new();
        for mask in &node.masks {
            if mask.mode != "Mask" {
                no_map.push(ReportEntry {
                    item: format!("part {:?} mask mode {:?}", node.name, mask.mode),
                    detail: "only the \"Mask\" mode has a native landing (clipping_mask_ids); this mode is dropped".to_string(),
                });
                continue;
            }
            let source = *node_by_uuid.get(&mask.source).ok_or_else(|| {
                inp_error(format!(
                    "part {:?}: mask source uuid {} does not exist",
                    node.name, mask.source
                ))
            })?;
            let mask_mesh = part_id_by_node[source].clone().ok_or_else(|| {
                inp_error(format!(
                    "part {:?}: mask source {:?} is not a Part",
                    node.name, nodes[source].name
                ))
            })?;
            if nodes[source].mesh.is_none() {
                return Err(inp_error(format!(
                    "part {:?}: mask source {:?} has no mesh to clip against",
                    node.name, nodes[source].name
                )));
            }
            clipping_mask_ids.push(mask_mesh);
        }
        // Blend mode (MAPPING.md section 6.3): ClipToLower is
        // realized as clipping against the rest-order
        // predecessor; the rest are the native 18 by name.
        let blend_raw = node.blend_mode.as_deref().unwrap_or("Normal");
        let blend_mode = if blend_raw == "ClipToLower" {
            match rest_predecessor.get(&node.index) {
                Some(predecessor) => {
                    clipping_mask_ids.push(
                        part_id_by_node[*predecessor]
                            .clone()
                            .expect("predecessors are parts"),
                    );
                    notes.push(format!(
                        "part {:?}: ClipToLower realized as clipping against {:?} (its predecessor in rest draw order)",
                        node.name, nodes[*predecessor].name
                    ));
                }
                None => {
                    no_map.push(ReportEntry {
                        item: format!("part {:?} blend mode ClipToLower", node.name),
                        detail: "the part is first in rest draw order; there is no lower drawable to clip against, so the blend drops to Normal with no mask".to_string(),
                    });
                }
            }
            BlendMode::Normal
        } else {
            BlendMode::from_format_str(&camel_to_snake(blend_raw)).ok_or_else(|| {
                inp_error(format!(
                    "part {:?}: blend mode {blend_raw:?} has no native counterpart",
                    node.name
                ))
            })?
        };
        // Texture: exactly one slot per meshed part.
        if node.texture_slots.len() != 1 {
            return Err(inp_error(format!(
                "part {:?} references {} texture slots; the native mesh names exactly one",
                node.name,
                node.texture_slots.len()
            )));
        }
        let slot = node.texture_slots[0];
        if slot < 0 || slot as usize >= container.textures.len() {
            return Err(inp_error(format!(
                "part {:?}: texture slot {slot} is outside the {}-slot TEX_SECT",
                node.name,
                container.textures.len()
            )));
        }
        let draw_order_f = (zsort_world[node.index] * ZSORT_TO_DRAW_ORDER).round();
        if draw_order_f < -10000.0 || draw_order_f > 10000.0 {
            draw_order_clamps += 1;
        }
        let draw_order = draw_order_f.clamp(-10000.0, 10000.0) as i32;
        // The mesh's deformer: the nearest driven node on the
        // path from this part to the root, itself included.
        let mut deformer: Option<String> = None;
        let mut cursor = Some(node.index);
        while let Some(current) = cursor {
            if let Some(id) = &warp_id_by_node[current] {
                deformer = Some(id.clone());
                break;
            }
            cursor = nodes[current].parent;
        }
        if node.has_nondefault_tint {
            no_map.push(ReportEntry {
                item: format!("part {:?} tint / screenTint", node.name),
                detail: "non-default tint has no native field and no moc3 slot (MAPPING.md section 6.2); dropped".to_string(),
            });
        }
        meshes.push(Mesh {
            id: part_id.clone(),
            part: part_id,
            texture: format!("texture_{slot:02}"),
            deformer,
            vertices: rest_verts[node.index]
                .clone()
                .expect("meshed parts have rest verts"),
            uvs: mesh.uvs.clone(),
            indices: mesh.indices.clone(),
            opacity: node.opacity,
            blend_mode,
            draw_order,
            rendered: node.enabled,
            clipping_mask_ids,
            keyforms,
        });
    }
    if draw_order_clamps > 0 {
        notes.push(format!(
            "{draw_order_clamps} mesh draw order(s) hit the native [-10000, 10000] clamp after the x{ZSORT_TO_DRAW_ORDER} rescale"
        ));
    }
    let draw_order_ties = {
        let mut counts: BTreeMap<i32, usize> = BTreeMap::new();
        for mesh in &meshes {
            *counts.entry(mesh.draw_order).or_default() += 1;
        }
        counts.values().filter(|count| **count > 1).count()
    };
    if draw_order_ties > 0 {
        notes.push(format!(
            "{draw_order_ties} draw-order value(s) are shared by more than one mesh after the x{ZSORT_TO_DRAW_ORDER} rescale; native consumers order ties by id (SPEC section 4.4)"
        ));
    }
    // zSort value bindings vary per pose; the native landing
    // is one static draw order per mesh, so the variation is
    // folded at default and named per binding.
    for param in &params {
        for binding in &param.bindings {
            if binding.param_name != "zSort" {
                continue;
            }
            if binding_varies(binding, 0) || binding_varies(binding, 1) {
                let BindingValues::Scalar(grid) = &binding.values else {
                    continue;
                };
                let mut lo = f32::INFINITY;
                let mut hi = f32::NEG_INFINITY;
                for row in grid {
                    for value in row {
                        lo = lo.min(*value);
                        hi = hi.max(*value);
                    }
                }
                lossy.push(ReportEntry {
                    item: format!(
                        "node {:?} zSort binding (parameter {:?})",
                        nodes[binding.node].name, param.name
                    ),
                    detail: format!(
                        "per-pose zSort offsets in [{lo}, {hi}] fold to the default-pose value in the static draw order (MAPPING.md section 7: order is the carrier, per-pose variation has no native form)"
                    ),
                });
            }
        }
    }

    // --- physics (MAPPING.md section 9.2) -----------------------
    let mut physics: Vec<PhysicsGroup> = Vec::new();
    let mut strand_total = 0usize;
    let mut used_group_ids: BTreeSet<String> = BTreeSet::new();
    let mut dropped_physics_fields: Vec<String> = Vec::new();
    for node in &nodes {
        if node.kind != "SimplePhysics" {
            continue;
        }
        if !node.enabled {
            no_map.push(ReportEntry {
                item: format!("SimplePhysics node {:?}", node.name),
                detail: "disabled in the source; the native model carries no disabled physics groups, so it is dropped".to_string(),
            });
            continue;
        }
        let model_type = node.physics_model_type.as_deref().unwrap_or("");
        if model_type != "SpringPendulum" {
            return Err(inp_error(format!(
                "SimplePhysics node {:?}: model_type {model_type:?} is not the SpringPendulum this stage implements",
                node.name
            )));
        }
        let length = node.physics_length.ok_or_else(|| {
            inp_error(format!(
                "SimplePhysics node {:?} carries no pendulum length",
                node.name
            ))
        })?;
        if length <= 0.0 || !length.is_finite() {
            return Err(inp_error(format!(
                "SimplePhysics node {:?}: pendulum length {length} is not positive and finite",
                node.name
            )));
        }
        let output_scale = node.physics_output_scale.ok_or_else(|| {
            inp_error(format!(
                "SimplePhysics node {:?} carries no output_scale",
                node.name
            ))
        })?;
        let param_uuid = node.physics_param.expect("validated in the census pass");
        let param = params
            .iter()
            .find(|param| param.uuid == param_uuid)
            .expect("driven parameters were validated in the census pass");
        let group_base = sanitize_id(&node.name);
        let group_id = mint_id(&group_base, &mut used_group_ids);
        // Anchor: the nearest ancestor part's mesh, with the
        // physics node's world rest position as the offset
        // (the inbound converter's anchor convention).
        let mut anchor_mesh: Option<String> = None;
        let mut cursor = node.parent;
        while let Some(current) = cursor {
            if nodes[current].mesh.is_some() {
                anchor_mesh = part_id_by_node[current].clone();
                break;
            }
            cursor = nodes[current].parent;
        }
        let anchor_mesh = match anchor_mesh {
            Some(id) => id,
            None => meshes
                .first()
                .map(|mesh| mesh.id.clone())
                .ok_or_else(|| inp_error("no mesh exists to anchor physics to"))?,
        };
        let anchor_point = world[node.index].apply([0.0, 0.0, 0.0]);
        let components: &[usize] = if param.is_vec2 { &[0, 1] } else { &[0] };
        let mut strands: Vec<Strand> = Vec::with_capacity(components.len());
        for &component in components {
            let suffix = if param.is_vec2 {
                if component == 0 { "x" } else { "y" }
            } else {
                "out"
            };
            strands.push(Strand {
                id: format!("{group_id}_{suffix}"),
                anchor: Anchor {
                    kind: AnchorKind::Mesh,
                    id: anchor_mesh.clone(),
                    offset: [anchor_point[0], anchor_point[1]],
                },
                particles: vec![Particle {
                    length,
                    radius: 0.0,
                    mobility: 1.0,
                    delay: 0.0,
                    damping: 0.0,
                }],
                output: Some(PhysicsOutput {
                    parameter: param.native_ids[component]
                        .clone()
                        .expect("driven parameters have native ids"),
                    scale: output_scale[component],
                    offset: 0.0,
                }),
            });
        }
        strand_total += strands.len();
        lossy.push(ReportEntry {
            item: format!("SimplePhysics node {:?} anchor driver", node.name),
            detail: format!(
                "map_mode {:?}: the pendulum follows its anchor node's motion in the source runtime; native v1 physics groups carry no input construct (SPEC section 12.1), so the driver is not carried (MAPPING.md section 9.2)",
                node.physics_map_mode.as_deref().unwrap_or("")
            ),
        });
        dropped_physics_fields.push(format!(
            "{:?}: frequency {:?}, angle_damping {:?}, length_damping {:?}",
            node.name,
            node.physics_frequency,
            node.physics_angle_damping,
            node.physics_length_damping
        ));
        physics.push(PhysicsGroup {
            id: group_id,
            gravity: [
                0.0,
                puppet.physics.gravity * node.physics_gravity.unwrap_or(1.0),
            ],
            wind: [0.0, 0.0],
            turbulence_amplitude: 0.0,
            inputs: Vec::new(),
            strands,
        });
        // Physics-driven transform bindings on any node fold
        // at default (the warp pass skipped this parameter).
        for binding in &param.bindings {
            if binding.param_name.starts_with("transform.") {
                folded_axes_total += 1;
                lossy.push(ReportEntry {
                    item: format!(
                        "node {:?} {} binding from physics parameter {:?}",
                        nodes[binding.node].name, binding.param_name, param.name
                    ),
                    detail: "folded at the default pose: a physics output may not be a lattice axis (LRF-207, SPEC section 3.7)".to_string(),
                });
            }
        }
    }
    if !dropped_physics_fields.is_empty() {
        no_map.push(ReportEntry {
            item: "SimplePhysics spring tuning fields".to_string(),
            detail: format!(
                "frequency / angle_damping / length_damping have no native strand fields in v1 (MAPPING.md section 9.2: spring pendulum onto rigid distance constraint); values: {}",
                dropped_physics_fields.join("; ")
            ),
        });
        synthesized.push(ReportEntry {
            item: "physics strand particle fields".to_string(),
            detail: "radius 0.0, mobility 1.0, delay 0.0, damping 0.0 and output offset 0.0 are synthesized: the INP spring pendulum carries no per-particle fields (the inbound converter makes the same damping choice, MAPPING.md section 9.2)".to_string(),
        });
    }

    // --- textures, parameters, model ----------------------------
    let textures: Vec<Texture> = container
        .textures
        .iter()
        .enumerate()
        .map(|(index, _)| Texture {
            id: format!("texture_{index:02}"),
            source: format!("texture_{index:02}.tga"),
        })
        .collect();
    let mut native_parameters: Vec<Parameter> = Vec::new();
    for param in &params {
        for axis in 0..2 {
            if let Some(id) = &param.native_ids[axis] {
                native_parameters.push(Parameter::Scalar(ScalarParameter {
                    id: id.clone(),
                    min: param.min[axis],
                    max: param.max[axis],
                    default: param.defaults[axis],
                }));
            }
        }
    }
    let model = Model {
        id: model_id.to_string(),
        generator: Some(GENERATOR.to_string()),
        parameters: native_parameters,
        parts,
        textures,
        deformers,
        meshes,
        physics,
    };

    // --- report --------------------------------------------------
    let composite_count = nodes.iter().filter(|node| node.kind == "Composite").count();
    if composite_count > 0 {
        no_map.push(ReportEntry {
            item: format!("{composite_count} Composite node(s)"),
            detail: "composite grouping (offscreen render of a subtree) has no native construct (MAPPING.md section 14); children are reparented to the nearest Part ancestor and composite transforms are composed into descendants statically".to_string(),
        });
    }
    if puppet.automation.is_some() {
        no_map.push(ReportEntry {
            item: "automation block".to_string(),
            detail: "the puppet's automation data has no native construct (MAPPING.md section 14); dropped".to_string(),
        });
    }
    let threshold_count = nodes
        .iter()
        .filter(|node| node.mask_threshold.is_some())
        .count();
    if threshold_count > 0 {
        no_map.push(ReportEntry {
            item: format!("mask_threshold on {threshold_count} node(s)"),
            detail:
                "no native or moc3 field carries a per-part mask threshold (MAPPING.md section 6.3)"
                    .to_string(),
        });
    }
    for mode in &non_linear_modes {
        lossy.push(ReportEntry {
            item: format!("interpolate_mode {mode}"),
            detail: "grid-point values are exact; between grid points the native lattice interpolates bilinearly where the source driver uses this mode (MAPPING.md section 3.4 evaluation class)".to_string(),
        });
    }
    if bindings_with_unset > 0 {
        notes.push(format!(
            "{bindings_with_unset} binding(s) carry {unset_points_total} editor-unset grid point(s): the reference runtime interpolates the dense value grids and never consults isSet (the editor materializes interpolated values before saving), so the grids are read as the evaluated surface"
        ));
    }
    if puppet.meta.is_object() {
        notes.push(
            "the puppet meta block (title/author/version) is bundle-level data: it lands in the release bundle at phase E3, not in the native model"
                .to_string(),
        );
    }
    if !container.ext_sections.is_empty() {
        notes.push(format!(
            "EXT_SECT carries {} entr(ies) the converter inventories but does not interpret: {:?}",
            container.ext_sections.len(),
            container.ext_sections
        ));
    }
    notes.push(format!(
        "physics pixelsPerMeter {} is a simulation constant of the source runtime, not a canvas value; it is not carried (canvas is an exporter input, MAPPING.md section 2)",
        puppet.physics.pixels_per_meter
    ));
    notes.push(
        "texture pixels are never read by this stage: slots are inventoried by byte length and TGA header dimensions only, so no downscale, recompression, or format conversion can occur here".to_string(),
    );
    if !model.physics.is_empty() {
        notes.push(
            "physics group gravity is [0, +(global gravity x node multiplier)]: coordinates are carried verbatim in the source's y-down space (MAPPING.md section 2), so positive y is the downward direction the source values mean".to_string(),
        );
    }
    lossy.push(ReportEntry {
        item: "driven-node transform factorization".to_string(),
        detail: "each driven node becomes one warp deformer whose map is exact at the default pose and exact when at most one driven node deviates from it; simultaneous deviations of two or more driven ancestors compose with a second-order commutator error (the factorization MAPPING.md section 14 names for transform composition)".to_string(),
    });

    let report = InpReport {
        format: "inp-conversion-report",
        model_id: model_id.to_string(),
        source: InpSourceSummary {
            inx_bytes: bytes.len(),
            payload_bytes: container.payload_bytes,
            texture_slots: container.textures.len(),
            texture_inventory: container
                .textures
                .iter()
                .enumerate()
                .map(|(index, slot)| InpTextureSlot {
                    index,
                    byte_len: slot.byte_len,
                    kind: slot.kind,
                    width: slot.width,
                    height: slot.height,
                })
                .collect(),
            ext_sections: container.ext_sections.clone(),
        },
        counts: InpCounts {
            inp_parameters: params.len(),
            native_parameters: model.parameters.len(),
            inp_nodes: nodes.len(),
            native_parts: model.parts.len(),
            native_meshes: model.meshes.len(),
            native_deformers: model.deformers.len(),
            physics_groups: model.physics.len(),
            physics_strands: strand_total,
            bindings_total: total_bindings,
            deform_bindings: deform_count,
            value_bindings: value_count,
            kept_lattice_axes: kept_axes_total,
            folded_axes: folded_axes_total,
        },
        lossy,
        no_map,
        synthesized,
        notes,
    };
    let canonical = model.to_canonical_json();
    let validated = parse_model(canonical.as_bytes())?;
    Ok(InpOutcome {
        model: validated,
        report,
    })
}

/// The nearest strict ancestor of `index` that already owns a
/// warp deformer.
fn nearest_driven_ancestor(
    nodes: &[NodeInfo],
    warp_id_by_node: &[Option<String>],
    index: usize,
) -> Option<String> {
    let mut cursor = nodes[index].parent;
    while let Some(current) = cursor {
        if let Some(id) = &warp_id_by_node[current] {
            return Some(id.clone());
        }
        cursor = nodes[current].parent;
    }
    None
}

/// All index combinations over the given per-axis key counts,
/// in ascending lexicographic order (the native form order,
/// SPEC section 3.6).
fn lattice_combos(key_counts: &[usize]) -> Vec<Vec<u32>> {
    let mut combos: Vec<Vec<u32>> = vec![Vec::new()];
    for &count in key_counts {
        let mut next = Vec::with_capacity(combos.len() * count);
        for prefix in &combos {
            for i in 0..count {
                let mut combo = prefix.clone();
                combo.push(i as u32);
                next.push(combo);
            }
        }
        combos = next;
    }
    combos
}

/// Set a pose's kept axes from one lattice combination.
fn apply_combo(pose: &mut Pose, kept: &[AxisCandidate], keys: &[Vec<f32>], combo: &[u32]) {
    for (slot, candidate) in kept.iter().enumerate() {
        pose[candidate.param][candidate.axis] = keys[slot][combo[slot] as usize];
    }
}

/// CamelCase -> snake_case for the blend-mode vocabulary
/// ("ColorDodge" -> "color_dodge"), matching the native
/// format strings of `BlendMode::from_format_str`.
fn camel_to_snake(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 4);
    for (index, ch) in name.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if index > 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}
