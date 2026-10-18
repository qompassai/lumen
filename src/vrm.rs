//! VRM avatar import, texture grading, and export — the VRoid Studio
//! wiring, at the FILE/FORMAT layer.
//!
//! Boundary, stated plainly: VRoid Studio is a desktop GUI authoring app.
//! Lumen does not drive it, script it, or pretend to integrate with the
//! application. The wiring is the file it exports. In the factory pipeline
//! (see `.ai/memory/current.md`: canonical interchange is GLB/glTF 2.0 and
//! "VRM 1.0 nearly free on top of GLB"):
//!
//!   author in VRoid Studio (the companion's canonical sprite/portrait
//!   design is the design authority the Studio model is built from)
//!     → export `.vrm`
//!     → lumen `vrm_inspect` (validate + report) and `vrm_grade`
//!       (apply a style preset, e.g. `demo_toon`, to the embedded
//!       textures)
//!     → onward to Blender / game engines as a standard VRM file.
//!
//! What this module does NOT do: geometry, bones, blendshapes, and the
//! VRM extension metadata are never edited. A graded export differs from
//! its input only in the replaced texture bytes and the bufferView /
//! buffer bookkeeping that replacement forces; the parsed JSON outside
//! `images`, `bufferViews`, and `buffers` is identical, and the VRM
//! extension subtree is deep-equal. At `strength == 0.0` the export is
//! the input file byte for byte (the oracle intake's passthrough rule).
//!
//! Contract summary:
//! - Accepted: an existing `.vrm` (glTF 2.0 binary) inside the project
//!   root carrying a `VRMC_vrm` (1.0) or `VRM` (0.x) extension, with
//!   textures embedded as bufferViews into the GLB binary chunk.
//! - Rejected (typed `LumenError`, before any write): non-GLB input,
//!   glTF != 2.0, truncated or out-of-bounds chunks/bufferViews, a glTF
//!   with no VRM extension, external or data-URI textures, JPEG textures
//!   (this build's `image` features are png + gif; grading fails closed
//!   rather than silently skipping a texture), unknown style names, and
//!   out-of-range strengths (via `style::grade_image`).
//! - Bounds: input <= 256 MiB; textures <= 4096x4096 pixels; every
//!   chunk and bufferView read is bounds-checked against the buffers.
//! - The original texture bytes stay in the binary chunk as unreachable
//!   dead space (graded replacements are appended and the images
//!   repointed to new bufferViews): no other bufferView offset can move,
//!   so geometry data is provably untouched. The file grows by the size
//!   of the replaced textures.

use std::io::Cursor;
use std::path::{Path, PathBuf};

use image::{ExtendedColorType, ImageEncoder, ImageFormat, RgbaImage};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::style::grade_image;
use crate::{LumenError, resolve_write_path};

/// Largest VRM file accepted (mirrors the oracle intake's GLB bound).
pub const VRM_BYTES_MAX: u64 = 256 * 1024 * 1024;

/// Largest single texture accepted, in pixels (4096 x 4096).
pub const MAX_TEXTURE_PIXELS: u64 = 16_777_216;

const GLB_MAGIC: &[u8; 4] = b"glTF";
const GLB_VERSION: u32 = 2;
const CHUNK_JSON: u32 = 0x4E4F_534A;
const CHUNK_BIN: u32 = 0x004E_4942;

// ---------------------------------------------------------------------------
// Public contract
// ---------------------------------------------------------------------------

/// Request for `vrm_inspect`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct VrmInspectReq {
    /// Project-relative input `.vrm` file.
    pub vrm: String,
}

/// Request for `vrm_grade`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct VrmGradeReq {
    /// Project-relative input `.vrm` file.
    pub vrm: String,
    /// Project-relative output `.vrm`; omit to overwrite the input.
    pub output: Option<String>,
    /// Style preset name (see `style_list`).
    pub style: String,
    /// 0.0 (byte-identical passthrough) ..= 1.0 (full grade).
    pub strength: f32,
}

/// What `vrm_inspect` reports.
#[derive(Debug, Clone, Serialize)]
pub struct VrmInfo {
    /// "vrm-1.0" (VRMC_vrm extension) or "vrm-0.x" (VRM extension).
    pub flavor: String,
    /// The extension's spec/exporter version string, when present.
    pub spec_version: Option<String>,
    /// Avatar display name from the VRM meta block, when present.
    pub name: Option<String>,
    /// Avatar author(s) from the VRM meta block, when present.
    pub author: Option<String>,
    pub mesh_count: usize,
    pub node_count: usize,
    pub textures: Vec<VrmTextureInfo>,
    /// Total file size in bytes.
    pub byte_len: u64,
}

/// One embedded texture's summary.
#[derive(Debug, Clone, Serialize)]
pub struct VrmTextureInfo {
    pub index: usize,
    /// The image entry's mimeType ("external" when it has no bufferView).
    pub mime_type: String,
    /// Decoded pixel width; `None` when not decodable in this build.
    pub width: Option<u32>,
    /// Decoded pixel height; `None` when not decodable in this build.
    pub height: Option<u32>,
    /// Bytes the texture occupies in the binary chunk (0 when external).
    pub byte_length: usize,
}

/// What `vrm_grade` reports.
#[derive(Debug, Clone, Serialize)]
pub struct VrmGraded {
    /// Project-relative output path written.
    pub path: String,
    /// The style preset applied.
    pub style: String,
    /// Textures actually re-encoded (0 for the strength-0 passthrough).
    pub textures_graded: usize,
    /// Output file size in bytes.
    pub bytes_written: u64,
}

/// Inspect a VRM file: flavor, meta block, structure, embedded textures.
///
/// Errors: bad paths → `PathRejected`; malformed GLB or a glTF with no
/// VRM extension → `DocInvalid`.
pub async fn vrm_inspect(root: &Path, req: VrmInspectReq) -> Result<VrmInfo, LumenError> {
    let bytes = read_vrm(root, &req.vrm)?;
    let parsed = parse_vrm(&bytes)?;
    Ok(parsed.info())
}

/// Grade a VRM file's embedded textures with a style preset and write
/// the repackaged file (see the module docs for exactly what changes).
///
/// Errors: bad paths → `PathRejected`; malformed/non-VRM input,
/// external or JPEG textures, or a file with no embedded textures →
/// `DocInvalid`; unknown style or bad strength → `BadParam` (from
/// `style::grade_image`). Failures never write an output file.
pub async fn vrm_grade(root: &Path, req: VrmGradeReq) -> Result<VrmGraded, LumenError> {
    let bytes = read_vrm(root, &req.vrm)?;
    let mut parsed = parse_vrm(&bytes)?;
    if parsed.textures.is_empty() {
        return Err(LumenError::DocInvalid(
            "VRM carries no embedded textures to grade".to_string(),
        ));
    }
    // Validate style + strength against the first texture's pixels even
    // on the passthrough path, so a bad request fails the same way
    // whether or not it would have re-encoded anything.
    let mut probe = parsed.decode_texture(0)?;
    grade_image(&mut probe, &req.style, req.strength)?;
    let output_rel = req.output.clone().unwrap_or_else(|| req.vrm.clone());
    let output_path = resolve_write_path(&output_rel, root, &["vrm"])?;
    if req.strength == 0.0 {
        // Passthrough: the input bytes are the output bytes.
        write_atomic(&output_path, &bytes)?;
        return Ok(VrmGraded {
            path: output_rel,
            style: req.style,
            textures_graded: 0,
            bytes_written: bytes.len() as u64,
        });
    }
    let graded = parsed.grade_all(&req.style, req.strength)?;
    let out_bytes = parsed.build_glb()?;
    write_atomic(&output_path, &out_bytes)?;
    Ok(VrmGraded {
        path: output_rel,
        style: req.style,
        textures_graded: graded,
        bytes_written: out_bytes.len() as u64,
    })
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

struct Texture {
    index: usize,
    mime_type: String,
    /// (byteOffset, byteLength) into the binary chunk.
    range: Option<(usize, usize)>,
}

struct ParsedVrm {
    document: Value,
    bin: Vec<u8>,
    flavor: &'static str,
    textures: Vec<Texture>,
    byte_len: u64,
}

impl ParsedVrm {
    fn info(&self) -> VrmInfo {
        let ext = self.extension();
        let (name, author, spec_version) = match self.flavor {
            "vrm-1.0" => {
                let meta = ext.get("meta");
                let name = meta
                    .and_then(|m| m.get("name"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let author = meta
                    .and_then(|m| m.get("authors"))
                    .and_then(Value::as_array)
                    .map(|authors| {
                        authors
                            .iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join(", ")
                    });
                let spec = ext
                    .get("specVersion")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                (name, author, spec)
            }
            _ => {
                let meta = ext.get("meta");
                let name = meta
                    .and_then(|m| m.get("title"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let author = meta
                    .and_then(|m| m.get("author"))
                    .and_then(Value::as_str)
                    .map(str::to_string);
                let spec = ext
                    .get("exporterVersion")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                (name, author, spec)
            }
        };
        let textures = self
            .textures
            .iter()
            .map(|t| {
                let (width, height) = match (t.mime_type.as_str(), t.range) {
                    ("image/png", Some(_)) => self
                        .decode_texture(t.index)
                        .map(|img| (Some(img.width()), Some(img.height())))
                        .unwrap_or((None, None)),
                    _ => (None, None),
                };
                VrmTextureInfo {
                    index: t.index,
                    mime_type: if t.range.is_some() {
                        t.mime_type.clone()
                    } else {
                        "external".to_string()
                    },
                    width,
                    height,
                    byte_length: t.range.map_or(0, |(_, len)| len),
                }
            })
            .collect();
        VrmInfo {
            flavor: self.flavor.to_string(),
            spec_version,
            name,
            author,
            mesh_count: array_len(&self.document, "meshes"),
            node_count: array_len(&self.document, "nodes"),
            textures,
            byte_len: self.byte_len,
        }
    }

    fn extension(&self) -> &Value {
        let extensions = &self.document["extensions"];
        match self.flavor {
            "vrm-1.0" => &extensions["VRMC_vrm"],
            _ => &extensions["VRM"],
        }
    }

    fn texture_bytes(&self, texture: &Texture) -> Result<&[u8], LumenError> {
        let (off, len) = texture.range.ok_or_else(|| {
            LumenError::DocInvalid(format!(
                "image {} is not embedded (uri-based textures are unsupported)",
                texture.index
            ))
        })?;
        self.bin.get(off..off + len).ok_or_else(|| {
            LumenError::DocInvalid(format!(
                "image {} bufferView lies outside the binary chunk",
                texture.index
            ))
        })
    }

    fn decode_texture(&self, index: usize) -> Result<RgbaImage, LumenError> {
        let texture = &self.textures[index];
        if texture.mime_type != "image/png" {
            return Err(LumenError::DocInvalid(format!(
                "image {} is {} — only image/png textures can be graded in this build",
                texture.index, texture.mime_type
            )));
        }
        let bytes = self.texture_bytes(texture)?;
        let img = image::load_from_memory_with_format(bytes, ImageFormat::Png).map_err(|e| {
            LumenError::DocInvalid(format!(
                "image {} is not a decodable PNG: {e}",
                texture.index
            ))
        })?;
        let pixels = u64::from(img.width()) * u64::from(img.height());
        if pixels > MAX_TEXTURE_PIXELS {
            return Err(LumenError::DocInvalid(format!(
                "image {} is {}x{} — over the {MAX_TEXTURE_PIXELS}-pixel texture bound",
                texture.index,
                img.width(),
                img.height()
            )));
        }
        Ok(img.to_rgba8())
    }

    /// Grade every embedded texture and rebuild the document + binary
    /// chunk. Returns the number of textures re-encoded.
    fn grade_all(&mut self, style: &str, strength: f32) -> Result<usize, LumenError> {
        let mut new_bin = self.bin.clone();
        let mut new_views: Vec<(usize, usize, usize)> = Vec::new(); // (image, off, len)
        for t in &self.textures {
            let mut img = self.decode_texture(t.index)?;
            grade_image(&mut img, style, strength)?;
            let mut cursor = Cursor::new(Vec::new());
            let encoder = image::codecs::png::PngEncoder::new(&mut cursor);
            encoder
                .write_image(
                    img.as_raw(),
                    img.width(),
                    img.height(),
                    ExtendedColorType::Rgba8,
                )
                .map_err(|e| LumenError::Io(format!("re-encode texture {}: {e}", t.index)))?;
            let png = cursor.into_inner();
            while !new_bin.len().is_multiple_of(4) {
                new_bin.push(0);
            }
            let off = new_bin.len();
            let len = png.len();
            new_bin.extend_from_slice(&png);
            new_views.push((t.index, off, len));
        }
        let doc = self
            .document
            .as_object_mut()
            .ok_or_else(|| LumenError::DocInvalid("glTF root must be a JSON object".to_string()))?;
        let views = doc
            .get_mut("bufferViews")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| LumenError::DocInvalid("glTF has no bufferViews".to_string()))?;
        let mut image_to_view: Vec<(usize, usize)> = Vec::new();
        for (image_index, off, len) in new_views {
            views.push(json!({
                "buffer": 0,
                "byteLength": len,
                "byteOffset": off,
            }));
            image_to_view.push((image_index, views.len() - 1));
        }
        let images = doc
            .get_mut("images")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| LumenError::DocInvalid("glTF has no images".to_string()))?;
        for (image_index, view_index) in image_to_view {
            images[image_index]["bufferView"] = json!(view_index);
        }
        if let Some(buffers) = doc.get_mut("buffers").and_then(Value::as_array_mut)
            && let Some(first) = buffers.first_mut()
        {
            first["byteLength"] = json!(new_bin.len());
        }
        self.bin = new_bin;
        Ok(self.textures.len())
    }

    fn build_glb(&self) -> Result<Vec<u8>, LumenError> {
        let json_bytes = serde_json::to_vec(&self.document)
            .map_err(|e| LumenError::Io(format!("serialize glTF JSON: {e}")))?;
        let json_pad = (4 - json_bytes.len() % 4) % 4;
        let bin_pad = (4 - self.bin.len() % 4) % 4;
        let total = 12 + 8 + json_bytes.len() + json_pad + 8 + self.bin.len() + bin_pad;
        if total > u32::MAX as usize {
            return Err(LumenError::DocInvalid(
                "graded output would exceed the GLB 4 GiB length field".to_string(),
            ));
        }
        let mut out = Vec::with_capacity(total);
        out.extend_from_slice(GLB_MAGIC);
        out.extend_from_slice(&GLB_VERSION.to_le_bytes());
        out.extend_from_slice(&(total as u32).to_le_bytes());
        out.extend_from_slice(&((json_bytes.len() + json_pad) as u32).to_le_bytes());
        out.extend_from_slice(&CHUNK_JSON.to_le_bytes());
        out.extend_from_slice(&json_bytes);
        out.extend(std::iter::repeat_n(b' ', json_pad));
        out.extend_from_slice(&((self.bin.len() + bin_pad) as u32).to_le_bytes());
        out.extend_from_slice(&CHUNK_BIN.to_le_bytes());
        out.extend_from_slice(&self.bin);
        out.extend(std::iter::repeat_n(0u8, bin_pad));
        Ok(out)
    }
}

fn parse_vrm(bytes: &[u8]) -> Result<ParsedVrm, LumenError> {
    let invalid = |msg: &str| LumenError::DocInvalid(msg.to_string());
    if bytes.len() < 20 {
        return Err(invalid("file is too small to be a GLB"));
    }
    if &bytes[0..4] != GLB_MAGIC {
        return Err(invalid("missing glTF magic — not a GLB/VRM file"));
    }
    let version = read_u32(bytes, 4)?;
    if version != GLB_VERSION {
        return Err(LumenError::DocInvalid(format!(
            "glTF version {version} is not supported (need 2)"
        )));
    }
    let total = read_u32(bytes, 8)? as usize;
    if total > bytes.len() {
        return Err(invalid("GLB length header exceeds the actual file size"));
    }
    // Chunk 0 must be JSON.
    let json_len = read_u32(bytes, 12)? as usize;
    if read_u32(bytes, 16)? != CHUNK_JSON {
        return Err(invalid("first GLB chunk is not JSON"));
    }
    let json_end = 20usize
        .checked_add(json_len)
        .filter(|end| *end <= bytes.len())
        .ok_or_else(|| invalid("JSON chunk overruns the file"))?;
    let document: Value = serde_json::from_slice(&bytes[20..json_end])
        .map_err(|e| LumenError::DocInvalid(format!("glTF JSON does not parse: {e}")))?;
    if document["asset"]["version"].as_str() != Some("2.0") {
        return Err(invalid("asset.version is not \"2.0\""));
    }
    // Chunk 1, when present, must be the binary buffer.
    let mut bin: Vec<u8> = Vec::new();
    if json_end < bytes.len() {
        if json_end + 8 > bytes.len() {
            return Err(invalid("truncated second chunk header"));
        }
        let bin_len = read_u32(bytes, json_end)? as usize;
        if read_u32(bytes, json_end + 4)? != CHUNK_BIN {
            return Err(invalid("second GLB chunk is not BIN"));
        }
        let bin_start = json_end + 8;
        let bin_end = bin_start
            .checked_add(bin_len)
            .filter(|end| *end <= bytes.len())
            .ok_or_else(|| invalid("binary chunk overruns the file (truncated?)"))?;
        bin = bytes[bin_start..bin_end].to_vec();
    }
    // VRM flavor: exactly one of the two extension shapes.
    let extensions = &document["extensions"];
    let flavor = if extensions["VRMC_vrm"].is_object() {
        "vrm-1.0"
    } else if extensions["VRM"].is_object() {
        "vrm-0.x"
    } else {
        return Err(invalid(
            "not a VRM: neither the VRMC_vrm (1.0) nor VRM (0.x) extension is present",
        ));
    };
    // Buffers: the GLB binary must be the only buffer.
    let buffers = document["buffers"].as_array();
    match buffers {
        Some(list) if list.len() == 1 && list[0].get("uri").is_none() => {}
        Some(list) if list.is_empty() => {
            return Err(invalid("glTF declares no buffers"));
        }
        _ => {
            return Err(invalid(
                "only a single embedded GLB buffer is supported (external buffers are rejected)",
            ));
        }
    }
    // Textures: resolve each image's bufferView into a byte range.
    let views = document["bufferViews"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let mut textures = Vec::new();
    if let Some(images) = document["images"].as_array() {
        for (index, image) in images.iter().enumerate() {
            let mime_type = image["mimeType"]
                .as_str()
                .unwrap_or("image/png")
                .to_string();
            let range = match image.get("bufferView").and_then(Value::as_u64) {
                Some(view_index) => {
                    let view = views.get(view_index as usize).ok_or_else(|| {
                        LumenError::DocInvalid(format!(
                            "image {index} references missing bufferView {view_index}"
                        ))
                    })?;
                    if view.get("buffer").and_then(Value::as_u64).unwrap_or(0) != 0 {
                        return Err(LumenError::DocInvalid(format!(
                            "image {index} uses a non-GLB buffer"
                        )));
                    }
                    let off = view["byteOffset"].as_u64().unwrap_or(0) as usize;
                    let len = view["byteLength"].as_u64().ok_or_else(|| {
                        LumenError::DocInvalid(format!("bufferView {view_index} has no byteLength"))
                    })? as usize;
                    if off.checked_add(len).is_none_or(|end| end > bin.len()) {
                        return Err(LumenError::DocInvalid(format!(
                            "image {index} bufferView lies outside the binary chunk"
                        )));
                    }
                    Some((off, len))
                }
                None => None,
            };
            textures.push(Texture {
                index,
                mime_type,
                range,
            });
        }
    }
    Ok(ParsedVrm {
        document,
        bin,
        flavor,
        textures,
        byte_len: bytes.len() as u64,
    })
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, LumenError> {
    let slice = bytes.get(offset..offset + 4).ok_or_else(|| {
        LumenError::DocInvalid(format!("unexpected end of file at byte {offset}"))
    })?;
    Ok(u32::from_le_bytes(
        slice.try_into().expect("four bytes checked"),
    ))
}

fn array_len(document: &Value, key: &str) -> usize {
    document[key].as_array().map_or(0, Vec::len)
}

// ---------------------------------------------------------------------------
// Paths and I/O
// ---------------------------------------------------------------------------

fn resolve_vrm_path(user_path: &str, root: &Path) -> Result<PathBuf, LumenError> {
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
        Some(ext) if ext.eq_ignore_ascii_case("vrm") => Ok(canonical),
        _ => Err(LumenError::PathRejected(
            "only .vrm files are accepted".to_string(),
        )),
    }
}

fn read_vrm(root: &Path, user_path: &str) -> Result<Vec<u8>, LumenError> {
    let path = resolve_vrm_path(user_path, root)?;
    let metadata = std::fs::metadata(&path)?;
    if metadata.len() > VRM_BYTES_MAX {
        return Err(LumenError::DocInvalid(format!(
            "file is {} bytes; the limit is {VRM_BYTES_MAX}",
            metadata.len()
        )));
    }
    std::fs::read(&path).map_err(LumenError::from)
}

/// Write `contents` to `path` via a sibling temp file + rename, so a
/// crash never leaves a torn VRM behind (the oracle intake's rule).
fn write_atomic(path: &Path, contents: &[u8]) -> Result<(), LumenError> {
    let tmp = path.with_extension("vrm.tmp");
    std::fs::write(&tmp, contents)?;
    std::fs::rename(&tmp, path).map_err(LumenError::from)
}
