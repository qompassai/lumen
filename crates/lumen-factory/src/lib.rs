//! lumen-factory — procedural sprite-to-3D character factory for lumen.
//!
//! Contract summary:
//! - Phase 1a scope: canonical Mixamo-compatible skeleton, a deterministic
//!   procedural base mesh (no external assets), skin weights with at most
//!   four influences per vertex, and a deterministic GLB writer plus a
//!   fail-closed GLB reader for parse-back validation.
//! - Phase 1b scope: blendshape composition (`blend`) — ARKit channel
//!   displacement fields over the head region masks, emotion /
//!   procedural / sheet morph targets composed from the ratified
//!   templates file at a caller-supplied path.
//! - Phase 4 scope: structural export validation (`validate`) — a
//!   named-rule, fail-closed validator over GLB/VRM bytes producing a
//!   JSON report (schema `lumen.factory-validation` v1).
//! - Phase 5 scope: ramp/toon material configuration (`toon`) as
//!   data in GLB extras, plus the Blender render proof.
//! - Factory-gap scope (GAP-11-1..4): blendshape displacement
//!   amplitudes as a measured, checked-in spec (`amplitudes`);
//!   per-character bust delta and per-band width/depth scales
//!   carried end-to-end by the identity and parameter records
//!   (`identity` / `params` / `base` / `project`).
//! - Integration update: the factory branch carries lumen main's
//!   landed VRoid wiring; the VRM proportions bridge (`params`)
//!   consumes VRM structure alongside it (see `vrm` for the division
//!   of labor).
//! - Every public function fails closed: invalid input produces a typed
//!   [`FactoryError`], never a panic and never a partial side effect.
//! - All generation math is in-crate and ordered: the same inputs always
//!   produce a byte-identical GLB.
//! - Units are meters, Y-up, facing +Z (glTF 2.0 right-handed convention).

#![forbid(unsafe_code)]

pub mod amplitudes;
pub mod base;
pub mod blend;
pub mod fidelity;
pub mod gltf;
pub mod identity;
pub mod params;
pub mod project;
pub mod toon;
pub mod validate;
pub mod vrm;

use sha2::{Digest, Sha256};
use thiserror::Error;

/// Maximum accepted GLB size for the reader. Anything larger is rejected
/// before parsing so a hostile file cannot blow up memory.
pub const MAX_GLB_BYTES: usize = 256 * 1024 * 1024;
/// Maximum vertices accepted in any single generated mesh part.
pub const MAX_PART_VERTICES: usize = 1_000_000;
/// Maximum joints in the canonical skeleton.
pub const MAX_JOINTS: usize = 256;
/// Maximum morph targets on a single primitive.
pub const MAX_MORPH_TARGETS: usize = 64;
/// Maximum skin influences per vertex (glTF `JOINTS_0`/`WEIGHTS_0` vec4).
pub const MAX_SKIN_INFLUENCES: usize = 4;

/// Typed failures for the factory crate.
#[derive(Debug, Error)]
pub enum FactoryError {
    /// A base-mesh or skeleton input failed validation.
    #[error("bad parameter: {0}")]
    BadParam(String),
    /// The GLB bytes are structurally invalid or corrupt.
    #[error("invalid glb: {0}")]
    InvalidGlb(String),
    /// Filesystem failure.
    #[error("io error: {0}")]
    Io(String),
    /// A mesh invariant was violated during generation.
    #[error("mesh invalid: {0}")]
    MeshInvalid(String),
    /// The GLB is well-formed but carries no VRM extension.
    #[error("not a vrm: {0}")]
    NotVrm(String),
}

impl From<std::io::Error> for FactoryError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

impl From<serde_json::Error> for FactoryError {
    fn from(e: serde_json::Error) -> Self {
        Self::InvalidGlb(e.to_string())
    }
}

/// Dimensions of a PNG byte slice, read from its header via the `image`
/// crate without a full decode. Fails closed on non-PNG input.
///
/// This is the Phase 1a anchor for the PNG dependency; Phase 2 ingest
/// builds full decoding on the same bound (`MAX_GLB_BYTES`-class caps).
pub fn png_dimensions(bytes: &[u8]) -> Result<(u32, u32), FactoryError> {
    use std::io::Cursor;
    if bytes.len() > MAX_GLB_BYTES {
        return Err(FactoryError::BadParam(
            "png input exceeds size bound".to_string(),
        ));
    }
    let reader = image::ImageReader::with_format(Cursor::new(bytes), image::ImageFormat::Png);
    reader
        .into_dimensions()
        .map_err(|e| FactoryError::BadParam(format!("not a readable png: {e}")))
}

/// SHA-256 hex digest of `bytes` (lowercase, 64 chars). Deterministic.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let digest = hasher.finalize();
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}
