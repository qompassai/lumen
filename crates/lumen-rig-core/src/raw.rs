//! The raw layer: serde shapes mirroring the JSON text exactly.
//!
//! Raw values are NOT proof of anything — they are the input to
//! `crate::validate`. Keeping raw and validated types distinct is
//! what lets the validator own every invariant (the Tiger Style
//! "validated vs raw" distinction). Every struct denies unknown
//! fields: an unrecognized key is `LRF-104`, never ignored.

use serde::Deserialize;

/// Header probe, read before full validation so a wrong format
/// or version reports as itself (SPEC.md section 8, step 2),
/// not as a downstream schema error. Deliberately permissive:
/// no `deny_unknown_fields`, only the two header fields.
#[derive(Deserialize)]
pub struct RawHeader {
    pub format: Option<String>,
    pub format_version: Option<u32>,
}

/// The full raw model document.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawModel {
    pub format: String,
    pub format_version: u32,
    pub id: String,
    pub generator: Option<String>,
    pub parameters: Vec<RawParameter>,
    pub parts: Vec<RawPart>,
    pub textures: Vec<RawTexture>,
    pub deformers: Vec<RawDeformer>,
    pub meshes: Vec<RawMesh>,
    pub physics: Vec<RawPhysicsGroup>,
}

/// A raw parameter, tagged by `kind`.
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RawParameter {
    Scalar {
        id: String,
        min: f32,
        max: f32,
        default: f32,
    },
    Combined {
        id: String,
        min: f32,
        max: f32,
        terms: Vec<RawCombinedTerm>,
        offset: f32,
    },
}

/// One raw weighted term of a combined parameter.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawCombinedTerm {
    pub parameter: String,
    pub weight: f32,
}

/// A raw part. `translation_keyforms` is the BIND-1
/// amendment field (SPEC.md section 3.2); it is optional and
/// defaulted, so pre-amendment documents parse unchanged.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawPart {
    pub id: String,
    pub parent: Option<String>,
    pub opacity: Option<f32>,
    pub translation_keyforms: Option<RawLattice<RawTranslationForm>>,
}

/// A raw texture reference.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawTexture {
    pub id: String,
    pub source: String,
}

/// A raw deformer, tagged by `kind`.
#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum RawDeformer {
    Rotation {
        id: String,
        parent: Option<String>,
        origin: [f32; 2],
        base_angle_degrees: f32,
        keyforms: Option<RawLattice<RawRotationForm>>,
    },
    Warp {
        id: String,
        parent: Option<String>,
        columns: u32,
        rows: u32,
        rest_positions: Vec<[f32; 2]>,
        keyforms: Option<RawLattice<RawWarpForm>>,
    },
}

/// A raw keyform lattice: shape unvalidated (axis references,
/// key ordering, and form completeness are the validator's job).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawLattice<F> {
    pub axes: Vec<String>,
    pub keys: Vec<Vec<f32>>,
    pub forms: Vec<F>,
}

/// A raw rotation keyform.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawRotationForm {
    pub at: Vec<u32>,
    pub angle_degrees: f32,
    pub scale: [f32; 2],
}

/// A raw warp keyform.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawWarpForm {
    pub at: Vec<u32>,
    pub positions: Vec<[f32; 2]>,
}

/// A raw mesh keyform.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawMeshForm {
    pub at: Vec<u32>,
    pub vertices: Vec<[f32; 2]>,
}

/// A raw part-translation keyform (BIND-1).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawTranslationForm {
    pub at: Vec<u32>,
    pub translation: [f32; 2],
}

/// A raw mesh. `blend_mode` stays a string here so an unknown
/// mode can be reported as `LRF-405`, not a generic schema error.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawMesh {
    pub id: String,
    pub part: String,
    pub texture: String,
    pub deformer: Option<String>,
    pub vertices: Vec<[f32; 2]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    pub opacity: Option<f32>,
    pub blend_mode: Option<String>,
    pub draw_order: Option<i32>,
    pub rendered: Option<bool>,
    pub clipping_mask_ids: Option<Vec<String>>,
    pub keyforms: Option<RawLattice<RawMeshForm>>,
}

/// A raw physics group.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawPhysicsGroup {
    pub id: String,
    pub gravity: Option<[f32; 2]>,
    pub wind: Option<[f32; 2]>,
    pub turbulence_amplitude: Option<f32>,
    pub inputs: Vec<RawPhysicsInput>,
    pub strands: Vec<RawStrand>,
}

/// A raw physics input.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawPhysicsInput {
    pub parameter: String,
    pub scale_degrees: f32,
}

/// A raw physics strand.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawStrand {
    pub id: String,
    pub anchor: RawAnchor,
    pub particles: Vec<RawParticle>,
    pub output: Option<RawPhysicsOutput>,
}

/// A raw strand anchor.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawAnchor {
    pub kind: RawAnchorKind,
    pub id: String,
    pub offset: [f32; 2],
}

/// Which collection a raw anchor names.
#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum RawAnchorKind {
    Deformer,
    Mesh,
}

/// A raw particle.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawParticle {
    pub length: f32,
    pub radius: f32,
    pub mobility: f32,
    pub delay: f32,
    pub damping: f32,
}

/// A raw strand output.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawPhysicsOutput {
    pub parameter: String,
    pub scale: f32,
    pub offset: f32,
}

/// The full raw motion document. `duration_seconds` is read as
/// f64 so an out-of-f32-range value survives to the validator,
/// which reports it as `LRF-301` (the value, not the grammar,
/// is what is wrong) instead of a generic parse failure.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawMotion {
    pub format: String,
    pub format_version: u32,
    pub id: String,
    pub model: String,
    pub duration_seconds: f64,
    #[serde(rename = "loop")]
    pub loop_: Option<bool>,
    pub frames_per_second: u32,
    pub curves: Vec<RawCurve>,
    pub audio: Option<RawAudio>,
    pub fathom: Option<RawFathom>,
}

/// A raw parameter curve. `interpolation` on keys stays a
/// string here so an unknown mode can be reported precisely,
/// not as a generic schema error.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawCurve {
    pub parameter: String,
    pub fade_in_seconds: Option<f32>,
    pub fade_out_seconds: Option<f32>,
    pub keyframes: Vec<RawKeyframe>,
}

/// A raw keyframe. Tangents stay optional pairs; which keys may
/// carry them is the validator's job (`LRF-502`).
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawKeyframe {
    pub time: f32,
    pub value: f32,
    pub interpolation: Option<String>,
    pub tangent_in: Option<[f32; 2]>,
    pub tangent_out: Option<[f32; 2]>,
}

/// The raw `audio` object: baked curve sets only (SPEC.md
/// section 6.1). The runtime follower (section 6.2) is a host
/// binding, not asset data, and has no raw shape here.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawAudio {
    pub baked: Vec<RawBakedCurveSet>,
}

/// A raw baked curve set: a curve plus its provenance.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawBakedCurveSet {
    pub parameter: String,
    pub fade_in_seconds: Option<f32>,
    pub fade_out_seconds: Option<f32>,
    pub keyframes: Vec<RawKeyframe>,
    pub provenance: RawBakedProvenance,
}

/// Raw baked-set provenance. `kind` stays a string so an
/// unknown kind reports precisely.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawBakedProvenance {
    pub kind: String,
    pub tool: String,
    pub tool_version: String,
    pub source_audio_sha256: Option<String>,
    pub window_seconds: Option<f32>,
    pub hop_seconds: Option<f32>,
}

/// The raw fathom binding (SPEC.md section 4.4). Vocabulary
/// strings stay strings so unknown values report precisely.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawFathom {
    pub schema: String,
    pub phases: Vec<RawFathomPhase>,
    pub arc: RawFathomArc,
}

/// A raw fathom phase.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawFathomPhase {
    pub label: String,
    pub start_frame: u32,
    pub end_frame: u32,
    pub provenance: String,
    pub source: Option<String>,
}

/// The raw fathom arc descriptor.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RawFathomArc {
    pub kind: String,
}
