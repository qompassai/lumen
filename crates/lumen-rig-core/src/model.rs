//! The validated model: the only shape downstream stages consume.
//!
//! A `Model` value is proof of validation — every invariant in
//! SPEC.md section 3 has been checked by `crate::validate`. Fields
//! the format makes optional-with-default are stored resolved
//! (for example `opacity` is always present here), so evaluation
//! never re-applies defaults.

use serde::Serialize;

/// The format version this crate implements.
pub const FORMAT_VERSION: u32 = 1;

/// A validated lumen rig model (format `lumen-rig-model`, v1).
#[derive(Debug, Clone, PartialEq)]
pub struct Model {
    pub id: String,
    pub generator: Option<String>,
    pub parameters: Vec<Parameter>,
    pub parts: Vec<Part>,
    pub textures: Vec<Texture>,
    pub deformers: Vec<Deformer>,
    pub meshes: Vec<Mesh>,
    pub physics: Vec<PhysicsGroup>,
}

/// A parameter: a named scalar channel (SPEC.md section 3.1).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Parameter {
    Scalar(ScalarParameter),
    Combined(CombinedParameter),
}

impl Parameter {
    /// The parameter's id, whichever kind it is.
    pub fn id(&self) -> &str {
        match self {
            Parameter::Scalar(param) => &param.id,
            Parameter::Combined(param) => &param.id,
        }
    }

    /// True for scalar parameters (the only legal lattice axes,
    /// physics inputs, and physics outputs).
    pub fn is_scalar(&self) -> bool {
        matches!(self, Parameter::Scalar(_))
    }
}

/// A scalar parameter with a declared range and rest value.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ScalarParameter {
    pub id: String,
    pub min: f32,
    pub max: f32,
    pub default: f32,
}

/// A derived parameter: `offset + sum(weight_i * value_i)`.
/// Its reachable range was proven contained in `[min, max]`
/// at validation time.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CombinedParameter {
    pub id: String,
    pub min: f32,
    pub max: f32,
    pub terms: Vec<CombinedTerm>,
    pub offset: f32,
}

/// One weighted input to a combined parameter.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CombinedTerm {
    pub parameter: String,
    pub weight: f32,
}

/// A named group in the part forest (SPEC.md section 3.2).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Part {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    pub opacity: f32,
}

/// A reference to external image data (SPEC.md section 3.3).
/// The format never embeds pixels.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Texture {
    pub id: String,
    pub source: String,
}

/// A deformer: rotation or warp (SPEC.md section 3.4).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Deformer {
    Rotation(RotationDeformer),
    Warp(WarpDeformer),
}

impl Deformer {
    /// The deformer's id, whichever kind it is.
    pub fn id(&self) -> &str {
        match self {
            Deformer::Rotation(deformer) => &deformer.id,
            Deformer::Warp(deformer) => &deformer.id,
        }
    }

    /// The parent deformer id, if any.
    pub fn parent(&self) -> Option<&str> {
        match self {
            Deformer::Rotation(deformer) => deformer.parent.as_deref(),
            Deformer::Warp(deformer) => deformer.parent.as_deref(),
        }
    }
}

/// A rotation deformer: rotates and scales its subtree about
/// `origin`. Keyform angles are absolute, in degrees.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RotationDeformer {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    pub origin: [f32; 2],
    pub base_angle_degrees: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keyforms: Option<Lattice<RotationForm>>,
}

/// A warp deformer: a control grid deforming its whole subtree
/// (a warp mesh group). `rest_positions` is row-major,
/// `(columns + 1) * (rows + 1)` points.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WarpDeformer {
    pub id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<String>,
    pub columns: u32,
    pub rows: u32,
    pub rest_positions: Vec<[f32; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keyforms: Option<Lattice<WarpForm>>,
}

/// A keyform lattice (SPEC.md section 3.7): complete object
/// states addressed by one or two scalar parameters. `forms`
/// is stored in canonical order (ascending lattice coordinate).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Lattice<F> {
    pub axes: Vec<String>,
    pub keys: Vec<Vec<f32>>,
    pub forms: Vec<F>,
}

/// A rotation keyform payload.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RotationForm {
    pub at: Vec<u32>,
    pub angle_degrees: f32,
    pub scale: [f32; 2],
}

/// A warp keyform payload: absolute control-point positions.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WarpForm {
    pub at: Vec<u32>,
    pub positions: Vec<[f32; 2]>,
}

/// A mesh keyform payload: absolute vertex positions.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MeshForm {
    pub at: Vec<u32>,
    pub vertices: Vec<[f32; 2]>,
}

/// The 18 blend modes of SPEC.md section 3.5 (W3C Compositing
/// and Blending Level 1 plus the two linear modes).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BlendMode {
    Normal,
    Multiply,
    Screen,
    Overlay,
    Darken,
    Lighten,
    ColorDodge,
    ColorBurn,
    HardLight,
    SoftLight,
    Difference,
    Exclusion,
    Hue,
    Saturation,
    Color,
    Luminosity,
    LinearDodge,
    LinearBurn,
}

impl BlendMode {
    /// Map a format string to a blend mode. Returns `None` for
    /// anything outside the section-3.5 set — the validator
    /// turns that into `LRF-405`, never a substitution.
    pub fn from_format_str(text: &str) -> Option<BlendMode> {
        let mode = match text {
            "normal" => BlendMode::Normal,
            "multiply" => BlendMode::Multiply,
            "screen" => BlendMode::Screen,
            "overlay" => BlendMode::Overlay,
            "darken" => BlendMode::Darken,
            "lighten" => BlendMode::Lighten,
            "color_dodge" => BlendMode::ColorDodge,
            "color_burn" => BlendMode::ColorBurn,
            "hard_light" => BlendMode::HardLight,
            "soft_light" => BlendMode::SoftLight,
            "difference" => BlendMode::Difference,
            "exclusion" => BlendMode::Exclusion,
            "hue" => BlendMode::Hue,
            "saturation" => BlendMode::Saturation,
            "color" => BlendMode::Color,
            "luminosity" => BlendMode::Luminosity,
            "linear_dodge" => BlendMode::LinearDodge,
            "linear_burn" => BlendMode::LinearBurn,
            _ => return None,
        };
        Some(mode)
    }
}

/// A drawable mesh (SPEC.md section 3.5).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Mesh {
    pub id: String,
    pub part: String,
    pub texture: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deformer: Option<String>,
    pub vertices: Vec<[f32; 2]>,
    pub uvs: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
    pub opacity: f32,
    pub blend_mode: BlendMode,
    pub draw_order: i32,
    pub rendered: bool,
    pub clipping_mask_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub keyforms: Option<Lattice<MeshForm>>,
}

/// A physics group (definitions only; the evaluation model is
/// SPEC.md section 5 and lands in R2).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PhysicsGroup {
    pub id: String,
    pub gravity: [f32; 2],
    pub wind: [f32; 2],
    pub turbulence_amplitude: f32,
    pub inputs: Vec<PhysicsInput>,
    pub strands: Vec<Strand>,
}

/// A physics input: a scalar parameter rotating the group's
/// gravity vector by `scale_degrees * normalized_value`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PhysicsInput {
    pub parameter: String,
    pub scale_degrees: f32,
}

/// An anchored particle chain (SPEC.md section 3.6).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Strand {
    pub id: String,
    pub anchor: Anchor,
    pub particles: Vec<Particle>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<PhysicsOutput>,
}

/// What a strand's root is pinned to.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Anchor {
    pub kind: AnchorKind,
    pub id: String,
    pub offset: [f32; 2],
}

/// The collection an anchor names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AnchorKind {
    Deformer,
    Mesh,
}

/// One particle (one segment) of a strand.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Particle {
    pub length: f32,
    pub radius: f32,
    pub mobility: f32,
    pub delay: f32,
    pub damping: f32,
}

/// A strand output: writes one scalar parameter as
/// `offset + scale * tip_angle_degrees`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PhysicsOutput {
    pub parameter: String,
    pub scale: f32,
    pub offset: f32,
}

/// Serialization view emitting the model with the format header
/// fields first, in canonical order (SPEC.md section 1.2).
#[derive(Serialize)]
struct ModelOut<'a> {
    format: &'static str,
    format_version: u32,
    id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    generator: &'a Option<String>,
    parameters: &'a [Parameter],
    parts: &'a [Part],
    textures: &'a [Texture],
    deformers: &'a [Deformer],
    meshes: &'a [Mesh],
    physics: &'a [PhysicsGroup],
}

impl Model {
    /// Serialize to the canonical text form (SPEC.md section 1.2):
    /// spec field order, 2-space indent, shortest-round-trip
    /// floats, one trailing newline. A pure function of the model:
    /// `model_to_json` output re-parses to an identical model and
    /// re-emits byte-identically.
    pub fn to_canonical_json(&self) -> String {
        let out = ModelOut {
            format: "lumen-rig-model",
            format_version: FORMAT_VERSION,
            id: &self.id,
            generator: &self.generator,
            parameters: &self.parameters,
            parts: &self.parts,
            textures: &self.textures,
            deformers: &self.deformers,
            meshes: &self.meshes,
            physics: &self.physics,
        };
        // Serializing this plain-data view cannot fail: every type
        // is a struct/enum of strings, numbers, bools, and vecs
        // with derived Serialize and no custom implementations.
        let mut text = serde_json::to_string_pretty(&out)
            .expect("serializing a validated model is infallible");
        text.push('\n');
        text
    }
}
