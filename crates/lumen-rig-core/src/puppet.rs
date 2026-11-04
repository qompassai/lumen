//! The puppet stage asset (SPEC.md section 13): the native
//! rig-format data a 2D puppet evaluates from.
//!
//! A puppet stage asset binds to one model asset (its scalar
//! parameters and its part tree are the shared currency) and
//! carries everything the model format deliberately does not:
//! vec2 parameter grids, the node transform forest with its
//! offset bindings, drawable semantics in the INP blend family,
//! masks, and SimplePhysics pendulum definitions.
//!
//! Like the model layer, this module keeps raw and validated
//! types distinct: [`parse_puppet`] takes bytes plus the bound
//! [`Model`] and returns either a fully validated
//! [`PuppetStage`] or the first [`PuppetError`] in section-13.8
//! order. Nothing is repaired, dropped, or clamped here.

use std::collections::{HashMap, HashSet};
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::limits::*;
use crate::model::{Model, Parameter};
use crate::validate::{map_serde_error, probe_header};

/// The stage asset's format token (SPEC 13.1).
pub const PUPPET_FORMAT: &str = "lumen-rig-puppet";
/// The stage asset's format version (SPEC 13.1).
pub const PUPPET_FORMAT_VERSION: u32 = 1;

/// A puppet-stage failure. Every variant carries a stable
/// `LRP-` code (SPEC 13.9); codes are never renumbered.
#[derive(Debug, Clone, PartialEq)]
pub enum PuppetError {
    /// LRP-101: the asset exceeds the byte bound.
    InputTooLarge { bytes: usize, max: usize },
    /// LRP-102: the bytes are not well-formed JSON.
    Syntax {
        line: usize,
        column: usize,
        message: String,
    },
    /// LRP-103: the JSON does not match the section-13 schema.
    Schema { message: String },
    /// LRP-105: wrong format token or format version.
    UnsupportedFormat { found: String },
    /// LRP-105: wrong format version.
    UnsupportedVersion { found: u32 },
    /// LRP-201: an id violates the shared id grammar.
    InvalidId { id: String },
    /// LRP-202: an id repeats within its collection.
    DuplicateId { kind: &'static str, id: String },
    /// LRP-203: a reference names something that does not exist
    /// (or exists with the wrong kind).
    DanglingReference { context: String, id: String },
    /// LRP-204: the node forest or the mask relation is cyclic,
    /// too deep, or does not have exactly one root.
    TreeShape { message: String },
    /// LRP-205: two physics nodes write the same parameter.
    MultipleWriters { parameter: String },
    /// LRP-206: the stage disagrees with its bound model
    /// (parameter channel ranges/defaults, node/part identity).
    ModelMismatch { message: String },
    /// LRP-207: a payload sits on the wrong node kind, or a
    /// required payload is missing.
    KindMismatch { message: String },
    /// LRP-208: evaluation was handed the wrong texture count.
    TextureCount { expected: usize, found: usize },
    /// LRP-301: a number is not finite.
    NonFinite { context: String },
    /// LRP-302: a scalar value is outside its legal range.
    OutOfRange { context: String },
    /// LRP-303: a binding grid's shape disagrees with its
    /// parameter's axes or its mesh's vertex count.
    GridShape { context: String },
    /// LRP-304: axis points are not strictly increasing, fall
    /// outside the parameter's range, or exceed the key bound.
    AxisShape { context: String },
    /// LRP-305: a collection exceeds its section-13.11 bound.
    TooMany {
        kind: &'static str,
        count: usize,
        max: usize,
    },
    /// LRP-306: an evaluation step was given an illegal dt.
    InvalidStep { dt: f32 },
}

impl PuppetError {
    /// The stable `LRP-` code for this failure.
    pub fn code(&self) -> &'static str {
        match self {
            PuppetError::InputTooLarge { .. } => "LRP-101",
            PuppetError::Syntax { .. } => "LRP-102",
            PuppetError::Schema { .. } => "LRP-103",
            PuppetError::UnsupportedFormat { .. } => "LRP-105",
            PuppetError::UnsupportedVersion { .. } => "LRP-105",
            PuppetError::InvalidId { .. } => "LRP-201",
            PuppetError::DuplicateId { .. } => "LRP-202",
            PuppetError::DanglingReference { .. } => "LRP-203",
            PuppetError::TreeShape { .. } => "LRP-204",
            PuppetError::MultipleWriters { .. } => "LRP-205",
            PuppetError::ModelMismatch { .. } => "LRP-206",
            PuppetError::KindMismatch { .. } => "LRP-207",
            PuppetError::TextureCount { .. } => "LRP-208",
            PuppetError::NonFinite { .. } => "LRP-301",
            PuppetError::OutOfRange { .. } => "LRP-302",
            PuppetError::GridShape { .. } => "LRP-303",
            PuppetError::AxisShape { .. } => "LRP-304",
            PuppetError::TooMany { .. } => "LRP-305",
            PuppetError::InvalidStep { .. } => "LRP-306",
        }
    }
}

impl fmt::Display for PuppetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let code = self.code();
        match self {
            PuppetError::InputTooLarge { bytes, max } => {
                write!(
                    f,
                    "{code}: puppet asset is {bytes} bytes, over the {max}-byte bound"
                )
            }
            PuppetError::Syntax {
                line,
                column,
                message,
            } => {
                write!(f, "{code}: JSON syntax error at {line}:{column}: {message}")
            }
            PuppetError::Schema { message } => write!(f, "{code}: schema error: {message}"),
            PuppetError::UnsupportedFormat { found } => {
                write!(f, "{code}: unsupported puppet format {found:?}")
            }
            PuppetError::UnsupportedVersion { found } => {
                write!(f, "{code}: unsupported puppet format version {found}")
            }
            PuppetError::InvalidId { id } => write!(f, "{code}: invalid id {id:?}"),
            PuppetError::DuplicateId { kind, id } => {
                write!(f, "{code}: duplicate {kind} id {id:?}")
            }
            PuppetError::DanglingReference { context, id } => {
                write!(f, "{code}: {context} references unknown id {id:?}")
            }
            PuppetError::TreeShape { message } => write!(f, "{code}: {message}"),
            PuppetError::MultipleWriters { parameter } => {
                write!(
                    f,
                    "{code}: parameter {parameter:?} has more than one physics writer"
                )
            }
            PuppetError::ModelMismatch { message } => {
                write!(f, "{code}: stage/model mismatch: {message}")
            }
            PuppetError::KindMismatch { message } => write!(f, "{code}: {message}"),
            PuppetError::TextureCount { expected, found } => {
                write!(f, "{code}: expected {expected} textures, got {found}")
            }
            PuppetError::NonFinite { context } => {
                write!(f, "{code}: non-finite number in {context}")
            }
            PuppetError::OutOfRange { context } => {
                write!(f, "{code}: value out of range in {context}")
            }
            PuppetError::GridShape { context } => {
                write!(f, "{code}: binding grid shape error in {context}")
            }
            PuppetError::AxisShape { context } => {
                write!(f, "{code}: axis shape error in {context}")
            }
            PuppetError::TooMany { kind, count, max } => {
                write!(f, "{code}: {count} {kind}, over the bound of {max}")
            }
            PuppetError::InvalidStep { dt } => {
                write!(f, "{code}: illegal evaluation step dt={dt}")
            }
        }
    }
}

impl std::error::Error for PuppetError {}

fn schema(message: impl Into<String>) -> PuppetError {
    PuppetError::Schema {
        message: message.into(),
    }
}

/* ------------------------------------------------------------------ */
/* Validated types                                                     */
/* ------------------------------------------------------------------ */

/// A validated puppet stage asset: proof of validation, like
/// [`Model`]. Fields are resolved (defaults applied).
#[derive(Debug, Clone, PartialEq)]
pub struct PuppetStage {
    /// The `id` of the model asset this stage binds to.
    pub model: String,
    /// Free-text generator stamp, if present.
    pub generator: Option<String>,
    /// Import provenance, if the stage came from an importer.
    pub provenance: Option<PuppetProvenance>,
    /// Vec2 parameters, in asset order (evaluation order).
    pub parameters: Vec<PuppetParameter>,
    /// Texture table; meshes index it by id.
    pub textures: Vec<PuppetTexture>,
    /// The node forest, in asset order.
    pub nodes: Vec<PuppetNode>,
    /// Offset bindings, in asset order.
    pub bindings: Vec<PuppetBinding>,
    /// Puppet-global physics constants.
    pub physics: PuppetPhysics,
}

/// Where a stage asset came from. Authoring metadata only;
/// evaluation never reads it.
#[derive(Debug, Clone, PartialEq)]
pub struct PuppetProvenance {
    /// Source puppet name (may be empty in the source format).
    pub source_name: String,
    /// Source format/exporter version string.
    pub source_version: String,
    /// `(stage id, original display name)` pairs for parameters
    /// and nodes whose names the id grammar could not carry.
    pub names: Vec<[String; 2]>,
}

/// One vec2 parameter: two scalar channels of the bound model
/// plus the binding-grid axes the model format has no room for.
#[derive(Debug, Clone, PartialEq)]
pub struct PuppetParameter {
    /// Stage-local id.
    pub id: String,
    /// Model scalar parameter id of the x channel.
    pub x: String,
    /// Model scalar parameter id of the y channel; `None` for
    /// a 1D parameter (its y value is fixed at `default[1]`).
    pub y: Option<String>,
    /// Per-channel minimum.
    pub min: [f32; 2],
    /// Per-channel maximum.
    pub max: [f32; 2],
    /// Per-channel default (the frame-reset value).
    pub default: [f32; 2],
    /// Grid axis points per channel: strictly increasing,
    /// inside `[min, max]`. `axis_points[1]` is empty iff
    /// `y` is `None`.
    pub axis_points: [Vec<f32>; 2],
}

/// One texture slot of the stage's texture table.
#[derive(Debug, Clone, PartialEq)]
pub struct PuppetTexture {
    /// Stage-local id.
    pub id: String,
    /// Relative source path, under the model's path rules.
    pub source: String,
}

/// What a node is. The puppet stage's node kinds mirror the
/// source format's node types one-for-one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PuppetNodeKind {
    /// A plain transform group.
    Group,
    /// A drawable textured mesh.
    Part,
    /// A drawable group: direct drawable children render into
    /// an offscreen buffer composited as one drawable.
    Composite,
    /// A transform group whose own mesh deforms its children
    /// in the authoring tool. The reference runtime does not
    /// evaluate that mesh; the stage carries it as inert data
    /// (SPEC 13.5) and evaluates the node as a group.
    MeshGroup,
    /// A SimplePhysics driver node.
    Physics,
}

impl PuppetNodeKind {
    fn from_format_str(s: &str) -> Option<Self> {
        match s {
            "group" => Some(PuppetNodeKind::Group),
            "part" => Some(PuppetNodeKind::Part),
            "composite" => Some(PuppetNodeKind::Composite),
            "mesh_group" => Some(PuppetNodeKind::MeshGroup),
            "physics" => Some(PuppetNodeKind::Physics),
            _ => None,
        }
    }
    fn to_format_str(self) -> &'static str {
        match self {
            PuppetNodeKind::Group => "group",
            PuppetNodeKind::Part => "part",
            PuppetNodeKind::Composite => "composite",
            PuppetNodeKind::MeshGroup => "mesh_group",
            PuppetNodeKind::Physics => "physics",
        }
    }
}

/// One node of the puppet forest.
#[derive(Debug, Clone, PartialEq)]
pub struct PuppetNode {
    /// Stage id; identical to the bound model's part id.
    pub id: String,
    /// Parent node id; `None` only for the single root.
    pub parent: Option<String>,
    /// Node kind.
    pub kind: PuppetNodeKind,
    /// Authoring enable flag. Carried data: the reference
    /// runtime does not consult it during evaluation, and
    /// neither does the stage evaluator (SPEC 13.5).
    pub enabled: bool,
    /// If true, the node's transform/zsort inherit from the
    /// root instead of the parent.
    pub lock_to_root: bool,
    /// Base z-sort offset (accumulates down the tree).
    pub zsort: f32,
    /// Base local transform.
    pub transform: PuppetTransform,
    /// The drawable mesh; present iff `kind == Part`.
    pub mesh: Option<PuppetMesh>,
    /// A mesh group's own mesh: inert carried data, present
    /// only on `MeshGroup` nodes (SPEC 13.5).
    pub group_mesh: Option<PuppetMesh>,
    /// Drawable state; present iff Part or Composite.
    pub drawable: Option<PuppetDrawable>,
    /// Mask sources clipping this drawable, applied in order.
    pub masks: Vec<PuppetMask>,
    /// Alpha threshold for this drawable's mask test.
    pub mask_threshold: f32,
    /// Physics definition; present iff `kind == Physics`.
    pub physics: Option<PuppetPhysicsNode>,
}

/// A node's base local transform. Angles are degrees, in the
/// shared format convention; the evaluator converts.
#[derive(Debug, Clone, PartialEq)]
pub struct PuppetTransform {
    /// Translation in puppet pixels (x right, y down, z
    /// toward the viewer) — the puppet stage keeps the source
    /// format's y-down plane (SPEC 13.2).
    pub translation: [f32; 3],
    /// Euler angles, degrees, XYZ order.
    pub rotation_degrees: [f32; 3],
    /// Per-axis scale.
    pub scale: [f32; 2],
    /// Authoring pixel-snap flag; carried, not evaluated
    /// (the reference runtime ignores it too).
    pub pixel_snap: bool,
}

/// A textured triangle mesh in node-local space.
#[derive(Debug, Clone, PartialEq)]
pub struct PuppetMesh {
    /// Stage-local id.
    pub id: String,
    /// Texture table id. Always present on a part's mesh; a
    /// mesh group's inert mesh may carry none (the source
    /// format gives mesh groups no texture slot).
    pub texture: Option<String>,
    /// Mesh origin: vertices are offset by it before the
    /// node transform applies (source-format semantics).
    pub origin: [f32; 2],
    /// Vertex positions, node-local puppet pixels.
    pub vertices: Vec<[f32; 2]>,
    /// Per-vertex texture coordinates, same length.
    pub uvs: Vec<[f32; 2]>,
    /// Triangle indices, a multiple of three.
    pub indices: Vec<u32>,
}

/// The stage's blend family: the seven modes the source
/// format defines. Unlike the model's W3C set, `ClipToLower`
/// and `SliceFromLower` are first-class here; they are
/// framebuffer-alpha operations (SPEC 13.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PuppetBlendMode {
    /// Premultiplied source-over.
    Normal,
    /// Multiply.
    Multiply,
    /// Color dodge.
    ColorDodge,
    /// Linear dodge (add).
    LinearDodge,
    /// Screen.
    Screen,
    /// Source weighted by the destination alpha below it.
    ClipToLower,
    /// Subtractive slice from the destination.
    SliceFromLower,
}

impl PuppetBlendMode {
    fn from_format_str(s: &str) -> Option<Self> {
        match s {
            "normal" => Some(PuppetBlendMode::Normal),
            "multiply" => Some(PuppetBlendMode::Multiply),
            "color_dodge" => Some(PuppetBlendMode::ColorDodge),
            "linear_dodge" => Some(PuppetBlendMode::LinearDodge),
            "screen" => Some(PuppetBlendMode::Screen),
            "clip_to_lower" => Some(PuppetBlendMode::ClipToLower),
            "slice_from_lower" => Some(PuppetBlendMode::SliceFromLower),
            _ => None,
        }
    }
    fn to_format_str(self) -> &'static str {
        match self {
            PuppetBlendMode::Normal => "normal",
            PuppetBlendMode::Multiply => "multiply",
            PuppetBlendMode::ColorDodge => "color_dodge",
            PuppetBlendMode::LinearDodge => "linear_dodge",
            PuppetBlendMode::Screen => "screen",
            PuppetBlendMode::ClipToLower => "clip_to_lower",
            PuppetBlendMode::SliceFromLower => "slice_from_lower",
        }
    }
}

/// Drawable state of a Part or Composite.
#[derive(Debug, Clone, PartialEq)]
pub struct PuppetDrawable {
    /// Blend mode.
    pub blend_mode: PuppetBlendMode,
    /// Base opacity in `[0, 1]`; parameter bindings multiply
    /// it per frame (SPEC 13.4).
    pub opacity: f32,
    /// Multiplicative color, each channel in `[0, 1]`.
    pub tint: [f32; 3],
    /// Screen-blend color, each channel in `[0, 1]`.
    pub screen_tint: [f32; 3],
}

/// Whether a mask source adds to or cuts the covered region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PuppetMaskMode {
    /// Content shows where the source's alpha passes.
    Mask,
    /// Content hides where the source's alpha passes.
    DodgeMask,
}

impl PuppetMaskMode {
    fn from_format_str(s: &str) -> Option<Self> {
        match s {
            "mask" => Some(PuppetMaskMode::Mask),
            "dodge_mask" => Some(PuppetMaskMode::DodgeMask),
            _ => None,
        }
    }
    fn to_format_str(self) -> &'static str {
        match self {
            PuppetMaskMode::Mask => "mask",
            PuppetMaskMode::DodgeMask => "dodge_mask",
        }
    }
}

/// One mask source: a Part node whose rendered alpha clips
/// the drawable carrying this mask.
#[derive(Debug, Clone, PartialEq)]
pub struct PuppetMask {
    /// Source node id (a Part).
    pub source: String,
    /// Mask or dodge.
    pub mode: PuppetMaskMode,
}

/// The pendulum family a physics node simulates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PuppetPhysicsModel {
    /// Rigid pendulum (angle state, RK4).
    Rigid,
    /// Spring pendulum (position/velocity state, RK4).
    Spring,
}

impl PuppetPhysicsModel {
    fn from_format_str(s: &str) -> Option<Self> {
        match s {
            "rigid" => Some(PuppetPhysicsModel::Rigid),
            "spring" => Some(PuppetPhysicsModel::Spring),
            _ => None,
        }
    }
    fn to_format_str(self) -> &'static str {
        match self {
            PuppetPhysicsModel::Rigid => "rigid",
            PuppetPhysicsModel::Spring => "spring",
        }
    }
}

/// How a pendulum's bob state maps to a vec2 parameter value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PuppetMapMode {
    /// Direction × relative length, rest at (0, -1) removed.
    Xy,
    /// XY, swizzled.
    Yx,
    /// (angle/π, relative length).
    AngleLength,
    /// (relative length, angle/π).
    LengthAngle,
}

impl PuppetMapMode {
    fn from_format_str(s: &str) -> Option<Self> {
        match s {
            "xy" => Some(PuppetMapMode::Xy),
            "yx" => Some(PuppetMapMode::Yx),
            "angle_length" => Some(PuppetMapMode::AngleLength),
            "length_angle" => Some(PuppetMapMode::LengthAngle),
            _ => None,
        }
    }
    fn to_format_str(self) -> &'static str {
        match self {
            PuppetMapMode::Xy => "xy",
            PuppetMapMode::Yx => "yx",
            PuppetMapMode::AngleLength => "angle_length",
            PuppetMapMode::LengthAngle => "length_angle",
        }
    }
}

/// A SimplePhysics definition: one pendulum writing one
/// vec2 parameter (the stage's one-writer rule, SPEC 13.7).
#[derive(Debug, Clone, PartialEq)]
pub struct PuppetPhysicsNode {
    /// The output parameter (stage parameter id).
    pub parameter: String,
    /// Pendulum family.
    pub model: PuppetPhysicsModel,
    /// Output mapping.
    pub map_mode: PuppetMapMode,
    /// If true, the anchor is the node's relative translation
    /// and output stays in node-local space.
    pub local_only: bool,
    /// Gravity scale (1.0 = puppet gravity).
    pub gravity: f32,
    /// Pendulum length, puppet pixels. Positive.
    pub length: f32,
    /// Spring frequency, Hz. Positive for spring pendulums.
    pub frequency: f32,
    /// Angular damping coefficient.
    pub angle_damping: f32,
    /// Length damping coefficient.
    pub length_damping: f32,
    /// Per-channel output scale.
    pub output_scale: [f32; 2],
}

/// Puppet-global physics constants.
#[derive(Debug, Clone, PartialEq)]
pub struct PuppetPhysics {
    /// Pixels per meter (source-format constant).
    pub pixels_per_meter: f32,
    /// Gravity, m/s².
    pub gravity: f32,
}

/// What a binding drives on its target node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PuppetBindingKind {
    /// `translation.x += value` (pixels).
    TranslationX,
    /// `translation.y += value` (pixels).
    TranslationY,
    /// `scale.x *= value`.
    ScaleX,
    /// `scale.y *= value`.
    ScaleY,
    /// `rotation.x += value` (degrees).
    RotationX,
    /// `rotation.y += value` (degrees).
    RotationY,
    /// `rotation.z += value` (degrees).
    RotationZ,
    /// `zsort += value`.
    ZSort,
    /// Drawable opacity factor `*= value`.
    Opacity,
    /// Per-vertex displacement field, added.
    Deform,
}

impl PuppetBindingKind {
    fn from_format_str(s: &str) -> Option<Self> {
        match s {
            "translation_x" => Some(PuppetBindingKind::TranslationX),
            "translation_y" => Some(PuppetBindingKind::TranslationY),
            "scale_x" => Some(PuppetBindingKind::ScaleX),
            "scale_y" => Some(PuppetBindingKind::ScaleY),
            "rotation_x" => Some(PuppetBindingKind::RotationX),
            "rotation_y" => Some(PuppetBindingKind::RotationY),
            "rotation_z" => Some(PuppetBindingKind::RotationZ),
            "z_sort" => Some(PuppetBindingKind::ZSort),
            "opacity" => Some(PuppetBindingKind::Opacity),
            "deform" => Some(PuppetBindingKind::Deform),
            _ => None,
        }
    }
    fn to_format_str(self) -> &'static str {
        match self {
            PuppetBindingKind::TranslationX => "translation_x",
            PuppetBindingKind::TranslationY => "translation_y",
            PuppetBindingKind::ScaleX => "scale_x",
            PuppetBindingKind::ScaleY => "scale_y",
            PuppetBindingKind::RotationX => "rotation_x",
            PuppetBindingKind::RotationY => "rotation_y",
            PuppetBindingKind::RotationZ => "rotation_z",
            PuppetBindingKind::ZSort => "z_sort",
            PuppetBindingKind::Opacity => "opacity",
            PuppetBindingKind::Deform => "deform",
        }
    }
    /// True for the per-vertex displacement kind.
    pub fn is_deform(self) -> bool {
        matches!(self, PuppetBindingKind::Deform)
    }
}

/// How a binding grid interpolates between axis points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PuppetInterpolate {
    /// Bilinear interpolation.
    Linear,
    /// Nearest grid point.
    Nearest,
    /// Carried, not applied: the reference runtime's shipped
    /// build drops cubic bindings at load, and the stage
    /// mirrors that effective behavior until the corpus is
    /// re-proved (SPEC 13.4).
    Cubic,
}

impl PuppetInterpolate {
    fn from_format_str(s: &str) -> Option<Self> {
        match s {
            "linear" => Some(PuppetInterpolate::Linear),
            "nearest" => Some(PuppetInterpolate::Nearest),
            "cubic" => Some(PuppetInterpolate::Cubic),
            _ => None,
        }
    }
    fn to_format_str(self) -> &'static str {
        match self {
            PuppetInterpolate::Linear => "linear",
            PuppetInterpolate::Nearest => "nearest",
            PuppetInterpolate::Cubic => "cubic",
        }
    }
}

/// A binding's value grid. Indexed `[x][y]` by axis-point
/// index — the source format's orientation (SPEC 13.4).
#[derive(Debug, Clone, PartialEq)]
pub enum PuppetBindingValues {
    /// Scalar grid for every kind except `Deform`.
    Scalar(Vec<Vec<f32>>),
    /// Per-cell vertex displacement fields for `Deform`.
    Deform(Vec<Vec<Vec<[f32; 2]>>>),
}

/// One parameter→node offset binding.
#[derive(Debug, Clone, PartialEq)]
pub struct PuppetBinding {
    /// Driving parameter (stage parameter id).
    pub parameter: String,
    /// Target node id.
    pub node: String,
    /// What the binding drives.
    pub kind: PuppetBindingKind,
    /// Grid interpolation mode.
    pub interpolate: PuppetInterpolate,
    /// The value grid.
    pub values: PuppetBindingValues,
}

/* ------------------------------------------------------------------ */
/* Raw layer                                                           */
/* ------------------------------------------------------------------ */

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawStage {
    format: String,
    format_version: u32,
    model: String,
    generator: Option<String>,
    provenance: Option<RawProvenance>,
    parameters: Vec<RawParameter>,
    textures: Vec<RawTexture>,
    nodes: Vec<RawNode>,
    bindings: Vec<RawBinding>,
    physics: RawPhysics,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawProvenance {
    source_name: String,
    source_version: String,
    names: Vec<[String; 2]>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawParameter {
    id: String,
    x: String,
    y: Option<String>,
    min: [f32; 2],
    max: [f32; 2],
    default: [f32; 2],
    axis_points: [Vec<f32>; 2],
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTexture {
    id: String,
    source: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawNode {
    id: String,
    parent: Option<String>,
    kind: String,
    enabled: Option<bool>,
    lock_to_root: Option<bool>,
    zsort: Option<f32>,
    transform: RawTransform,
    mesh: Option<RawMesh>,
    group_mesh: Option<RawMesh>,
    drawable: Option<RawDrawable>,
    masks: Option<Vec<RawMask>>,
    mask_threshold: Option<f32>,
    physics: Option<RawPhysicsNode>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawTransform {
    translation: [f32; 3],
    rotation_degrees: [f32; 3],
    scale: [f32; 2],
    pixel_snap: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMesh {
    id: String,
    texture: Option<String>,
    origin: [f32; 2],
    vertices: Vec<[f32; 2]>,
    uvs: Vec<[f32; 2]>,
    indices: Vec<u32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDrawable {
    blend_mode: String,
    opacity: f32,
    tint: Option<[f32; 3]>,
    screen_tint: Option<[f32; 3]>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMask {
    source: String,
    mode: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPhysicsNode {
    parameter: String,
    model: String,
    map_mode: String,
    local_only: Option<bool>,
    gravity: f32,
    length: f32,
    frequency: f32,
    angle_damping: f32,
    length_damping: f32,
    output_scale: [f32; 2],
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPhysics {
    pixels_per_meter: f32,
    gravity: f32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBinding {
    parameter: String,
    node: String,
    kind: String,
    interpolate: String,
    values: serde_json::Value,
}

/* ------------------------------------------------------------------ */
/* Parser + validator                                                  */
/* ------------------------------------------------------------------ */

/// The shared id grammar (SPEC 2): 1..=64 bytes of
/// `[A-Za-z0-9_-]`. Mirrors the model validator's rule.
fn check_id(id: &str) -> Result<(), PuppetError> {
    let ok = !id.is_empty()
        && id.len() <= ID_BYTES_MAX
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if ok {
        Ok(())
    } else {
        Err(PuppetError::InvalidId { id: id.to_string() })
    }
}

fn check_finite(value: f32, context: &str) -> Result<(), PuppetError> {
    if value.is_finite() {
        Ok(())
    } else {
        Err(PuppetError::NonFinite {
            context: context.to_string(),
        })
    }
}

fn check_vec<const N: usize>(values: &[f32; N], context: &str) -> Result<(), PuppetError> {
    for v in values {
        check_finite(*v, context)?;
    }
    Ok(())
}

/// Parse and validate a puppet stage asset against its bound
/// model. Checks run in SPEC 13.8 order and stop at the first
/// failure.
pub fn parse_puppet(bytes: &[u8], model: &Model) -> Result<PuppetStage, PuppetError> {
    if bytes.len() > INPUT_BYTES_MAX {
        return Err(PuppetError::InputTooLarge {
            bytes: bytes.len(),
            max: INPUT_BYTES_MAX,
        });
    }
    probe_header(bytes, PUPPET_FORMAT).map_err(|e| match e {
        crate::error::ParseError::UnsupportedFormat { found } => {
            PuppetError::UnsupportedFormat { found }
        }
        crate::error::ParseError::UnsupportedVersion { found } => {
            PuppetError::UnsupportedVersion { found }
        }
        other => PuppetError::Schema {
            message: other.to_string(),
        },
    })?;
    let raw: RawStage = serde_json::from_slice(bytes).map_err(|e| {
        let mapped = map_serde_error(e);
        match mapped {
            crate::error::ParseError::Syntax {
                line,
                column,
                message,
            } => PuppetError::Syntax {
                line,
                column,
                message,
            },
            crate::error::ParseError::NonFinite { context } => PuppetError::NonFinite { context },
            other => schema(other.to_string()),
        }
    })?;
    validate_stage(raw, model)
}

fn validate_stage(raw: RawStage, model: &Model) -> Result<PuppetStage, PuppetError> {
    if raw.format != PUPPET_FORMAT {
        return Err(PuppetError::UnsupportedFormat { found: raw.format });
    }
    if raw.format_version != PUPPET_FORMAT_VERSION {
        return Err(PuppetError::UnsupportedVersion {
            found: raw.format_version,
        });
    }
    if raw.model != model.id {
        return Err(PuppetError::ModelMismatch {
            message: format!(
                "stage binds to model {:?} but was parsed against {:?}",
                raw.model, model.id
            ),
        });
    }

    // Bounds (SPEC 13.11).
    if raw.parameters.len() > PUPPET_PARAMETERS_MAX {
        return Err(PuppetError::TooMany {
            kind: "parameters",
            count: raw.parameters.len(),
            max: PUPPET_PARAMETERS_MAX,
        });
    }
    if raw.textures.len() > PUPPET_TEXTURES_MAX {
        return Err(PuppetError::TooMany {
            kind: "textures",
            count: raw.textures.len(),
            max: PUPPET_TEXTURES_MAX,
        });
    }
    if raw.nodes.len() > PUPPET_NODES_MAX {
        return Err(PuppetError::TooMany {
            kind: "nodes",
            count: raw.nodes.len(),
            max: PUPPET_NODES_MAX,
        });
    }
    if raw.bindings.len() > PUPPET_BINDINGS_MAX {
        return Err(PuppetError::TooMany {
            kind: "bindings",
            count: raw.bindings.len(),
            max: PUPPET_BINDINGS_MAX,
        });
    }

    // Model scalar lookup for channel cross-checks.
    let mut scalars: HashMap<&str, (f32, f32, f32)> = HashMap::new();
    for param in &model.parameters {
        if let Parameter::Scalar(s) = param {
            scalars.insert(s.id.as_str(), (s.min, s.max, s.default));
        }
    }
    let model_parts: HashSet<&str> = model.parts.iter().map(|p| p.id.as_str()).collect();

    // Parameters.
    let mut param_ids: HashSet<&str> = HashSet::new();
    let mut parameters: Vec<PuppetParameter> = Vec::with_capacity(raw.parameters.len());
    for rp in &raw.parameters {
        check_id(&rp.id)?;
        if !param_ids.insert(rp.id.as_str()) {
            return Err(PuppetError::DuplicateId {
                kind: "parameter",
                id: rp.id.clone(),
            });
        }
        check_id(&rp.x)?;
        if let Some(y) = &rp.y {
            check_id(y)?;
        }
        for (channel, axis) in [(0usize, "x"), (1usize, "y")] {
            check_finite(rp.min[channel], "parameter min")?;
            check_finite(rp.max[channel], "parameter max")?;
            check_finite(rp.default[channel], "parameter default")?;
            if rp.min[channel].partial_cmp(&rp.max[channel]) != Some(std::cmp::Ordering::Less) {
                return Err(PuppetError::OutOfRange {
                    context: format!(
                        "parameter {:?} channel {axis}: min must be below max",
                        rp.id
                    ),
                });
            }
            if rp.default[channel] < rp.min[channel] || rp.default[channel] > rp.max[channel] {
                return Err(PuppetError::OutOfRange {
                    context: format!(
                        "parameter {:?} channel {axis}: default outside [min, max]",
                        rp.id
                    ),
                });
            }
            // Axis shape.
            let points = &rp.axis_points[channel];
            if channel == 1 && rp.y.is_none() {
                if !points.is_empty() {
                    return Err(PuppetError::AxisShape {
                        context: format!(
                            "parameter {:?}: y axis points present but the parameter is 1D",
                            rp.id
                        ),
                    });
                }
                continue;
            }
            if points.is_empty() || points.len() > KEYS_PER_AXIS_MAX {
                return Err(PuppetError::AxisShape {
                    context: format!(
                        "parameter {:?} channel {axis}: {} axis points, bound is 1..={KEYS_PER_AXIS_MAX}",
                        rp.id,
                        points.len()
                    ),
                });
            }
            for w in points.windows(2) {
                check_finite(w[0], "axis point")?;
                if w[0].partial_cmp(&w[1]) != Some(std::cmp::Ordering::Less) {
                    return Err(PuppetError::AxisShape {
                        context: format!(
                            "parameter {:?} channel {axis}: axis points not strictly increasing",
                            rp.id
                        ),
                    });
                }
            }
            check_finite(points[points.len() - 1], "axis point")?;
            if points[0] < rp.min[channel] || points[points.len() - 1] > rp.max[channel] {
                return Err(PuppetError::AxisShape {
                    context: format!(
                        "parameter {:?} channel {axis}: axis points outside [min, max]",
                        rp.id
                    ),
                });
            }
        }
        // Channel cross-check against the model (LRP-206): the
        // scalar must exist with identical min/max/default.
        let check_channel = |scalar_id: &str, channel: usize| -> Result<(), PuppetError> {
            match scalars.get(scalar_id) {
                None => Err(PuppetError::DanglingReference {
                    context: format!("parameter {:?} channel", rp.id),
                    id: scalar_id.to_string(),
                }),
                Some(&(min, max, default)) => {
                    if min != rp.min[channel]
                        || max != rp.max[channel]
                        || default != rp.default[channel]
                    {
                        return Err(PuppetError::ModelMismatch {
                            message: format!(
                                "parameter {:?} channel {} disagrees with model scalar {:?}",
                                rp.id, channel, scalar_id
                            ),
                        });
                    }
                    Ok(())
                }
            }
        };
        check_channel(&rp.x, 0)?;
        if let Some(y) = &rp.y {
            check_channel(y, 1)?;
        }
        parameters.push(PuppetParameter {
            id: rp.id.clone(),
            x: rp.x.clone(),
            y: rp.y.clone(),
            min: rp.min,
            max: rp.max,
            default: rp.default,
            axis_points: [rp.axis_points[0].clone(), rp.axis_points[1].clone()],
        });
    }

    // Textures.
    let mut texture_ids: HashSet<&str> = HashSet::new();
    let mut textures: Vec<PuppetTexture> = Vec::with_capacity(raw.textures.len());
    for rt in &raw.textures {
        check_id(&rt.id)?;
        if !texture_ids.insert(rt.id.as_str()) {
            return Err(PuppetError::DuplicateId {
                kind: "texture",
                id: rt.id.clone(),
            });
        }
        check_texture_source(&rt.source)?;
        textures.push(PuppetTexture {
            id: rt.id.clone(),
            source: rt.source.clone(),
        });
    }

    // Nodes.
    let mut node_index: HashMap<&str, usize> = HashMap::new();
    let mut nodes: Vec<PuppetNode> = Vec::with_capacity(raw.nodes.len());
    for rn in &raw.nodes {
        check_id(&rn.id)?;
        if node_index.insert(rn.id.as_str(), nodes.len()).is_some() {
            return Err(PuppetError::DuplicateId {
                kind: "node",
                id: rn.id.clone(),
            });
        }
        // Node ids are the bound model's part ids (LRP-206).
        if !model_parts.contains(rn.id.as_str()) {
            return Err(PuppetError::ModelMismatch {
                message: format!("node {:?} has no part in the bound model", rn.id),
            });
        }
        let kind = PuppetNodeKind::from_format_str(&rn.kind)
            .ok_or_else(|| schema(format!("node {:?}: unknown kind {:?}", rn.id, rn.kind)))?;
        check_vec(&rn.transform.translation, "node translation")?;
        check_vec(&rn.transform.rotation_degrees, "node rotation")?;
        check_vec(&rn.transform.scale, "node scale")?;
        let zsort = rn.zsort.unwrap_or(0.0);
        check_finite(zsort, "node zsort")?;
        let mask_threshold = rn.mask_threshold.unwrap_or(0.5);
        check_finite(mask_threshold, "mask threshold")?;
        if !(0.0..=1.0).contains(&mask_threshold) {
            return Err(PuppetError::OutOfRange {
                context: format!("node {:?}: mask_threshold outside [0, 1]", rn.id),
            });
        }

        // Kind/payload discipline (LRP-207).
        let mesh = match (kind, &rn.mesh) {
            (PuppetNodeKind::Part, Some(m)) => Some(validate_mesh(m, &texture_ids, true)?),
            (PuppetNodeKind::Part, None) => {
                return Err(PuppetError::KindMismatch {
                    message: format!("part node {:?} carries no mesh", rn.id),
                });
            }
            (_, Some(_)) => {
                return Err(PuppetError::KindMismatch {
                    message: format!("node {:?} of kind {} carries a part mesh", rn.id, rn.kind),
                });
            }
            (_, None) => None,
        };
        let group_mesh = match (kind, &rn.group_mesh) {
            (PuppetNodeKind::MeshGroup, Some(m)) => Some(validate_mesh(m, &texture_ids, false)?),
            (PuppetNodeKind::MeshGroup, None) => None,
            (_, Some(_)) => {
                return Err(PuppetError::KindMismatch {
                    message: format!("node {:?}: group_mesh on a non-mesh-group node", rn.id),
                });
            }
            (_, None) => None,
        };
        let drawable = match (kind, &rn.drawable) {
            (PuppetNodeKind::Part | PuppetNodeKind::Composite, Some(d)) => {
                Some(validate_drawable(d, &rn.id)?)
            }
            (PuppetNodeKind::Part | PuppetNodeKind::Composite, None) => {
                return Err(PuppetError::KindMismatch {
                    message: format!("drawable node {:?} carries no drawable state", rn.id),
                });
            }
            (_, Some(_)) => {
                return Err(PuppetError::KindMismatch {
                    message: format!(
                        "node {:?} of kind {} carries drawable state",
                        rn.id, rn.kind
                    ),
                });
            }
            (_, None) => None,
        };
        let mut masks: Vec<PuppetMask> = Vec::new();
        for rm in rn.masks.as_deref().unwrap_or(&[]) {
            let mode = PuppetMaskMode::from_format_str(&rm.mode).ok_or_else(|| {
                schema(format!("node {:?}: unknown mask mode {:?}", rn.id, rm.mode))
            })?;
            masks.push(PuppetMask {
                source: rm.source.clone(),
                mode,
            });
        }
        if !masks.is_empty() && drawable.is_none() {
            return Err(PuppetError::KindMismatch {
                message: format!("node {:?} carries masks but is not a drawable", rn.id),
            });
        }
        let physics = match (kind, &rn.physics) {
            (PuppetNodeKind::Physics, Some(p)) => Some(validate_physics_node(p, &rn.id)?),
            (PuppetNodeKind::Physics, None) => {
                return Err(PuppetError::KindMismatch {
                    message: format!("physics node {:?} carries no physics definition", rn.id),
                });
            }
            (_, Some(_)) => {
                return Err(PuppetError::KindMismatch {
                    message: format!(
                        "node {:?} of kind {} carries a physics definition",
                        rn.id, rn.kind
                    ),
                });
            }
            (_, None) => None,
        };

        nodes.push(PuppetNode {
            id: rn.id.clone(),
            parent: rn.parent.clone(),
            kind,
            enabled: rn.enabled.unwrap_or(true),
            lock_to_root: rn.lock_to_root.unwrap_or(false),
            zsort,
            transform: PuppetTransform {
                translation: rn.transform.translation,
                rotation_degrees: rn.transform.rotation_degrees,
                scale: rn.transform.scale,
                pixel_snap: rn.transform.pixel_snap.unwrap_or(false),
            },
            mesh,
            group_mesh,
            drawable,
            masks,
            mask_threshold,
            physics,
        });
    }

    // Every model part is a stage node (LRP-206): the stage is
    // the model's whole tree, not a subset.
    if nodes.len() != model.parts.len() {
        return Err(PuppetError::ModelMismatch {
            message: format!(
                "stage has {} nodes but the bound model has {} parts",
                nodes.len(),
                model.parts.len()
            ),
        });
    }
    // Parent links mirror the model's part parents (LRP-206),
    // parents exist, and model parent identity = stage parent.
    let part_parent: HashMap<&str, Option<&str>> = model
        .parts
        .iter()
        .map(|p| (p.id.as_str(), p.parent.as_deref()))
        .collect();
    for node in &nodes {
        if node.parent.as_deref() != part_parent[node.id.as_str()] {
            return Err(PuppetError::ModelMismatch {
                message: format!(
                    "node {:?}: stage parent {:?} disagrees with model part parent {:?}",
                    node.id,
                    node.parent,
                    part_parent[node.id.as_str()]
                ),
            });
        }
    }
    // Forest shape: exactly one root, depth within bound.
    let roots = nodes.iter().filter(|n| n.parent.is_none()).count();
    if roots != 1 {
        return Err(PuppetError::TreeShape {
            message: format!("node forest has {roots} roots, expected exactly one"),
        });
    }
    for node in &nodes {
        let mut depth = 0usize;
        let mut cursor = node;
        while let Some(parent) = &cursor.parent {
            depth += 1;
            if depth > TREE_DEPTH_MAX {
                return Err(PuppetError::TreeShape {
                    message: format!("node {:?}: depth over TREE_DEPTH_MAX", node.id),
                });
            }
            cursor = &nodes[node_index[parent.as_str()]];
        }
    }
    // Mask sources: exist, are Parts, and the mask relation is
    // acyclic (LRP-203/LRP-204).
    for node in &nodes {
        for mask in &node.masks {
            match node_index.get(mask.source.as_str()) {
                None => {
                    return Err(PuppetError::DanglingReference {
                        context: format!("node {:?} mask", node.id),
                        id: mask.source.clone(),
                    });
                }
                Some(&si) if nodes[si].kind != PuppetNodeKind::Part => {
                    return Err(PuppetError::DanglingReference {
                        context: format!(
                            "node {:?} mask source (mask sources must be parts)",
                            node.id
                        ),
                        id: mask.source.clone(),
                    });
                }
                _ => {}
            }
        }
    }
    // Three-color DFS over the mask relation: a gray revisit
    // is a cycle; black nodes are proven acyclic (diamonds are
    // legal).
    fn mask_visit<'n>(
        cur: &'n str,
        nodes: &'n [PuppetNode],
        node_index: &HashMap<&str, usize>,
        color: &mut HashMap<&'n str, u8>,
    ) -> Result<(), PuppetError> {
        color.insert(cur, 1);
        for mask in &nodes[node_index[cur]].masks {
            let src = mask.source.as_str();
            match color.get(src).copied().unwrap_or(0) {
                1 => {
                    return Err(PuppetError::TreeShape {
                        message: format!("mask relation cycles through {src:?}"),
                    });
                }
                0 => mask_visit(src, nodes, node_index, color)?,
                _ => {}
            }
        }
        color.insert(cur, 2);
        Ok(())
    }
    let mut color: HashMap<&str, u8> = HashMap::new();
    for node in &nodes {
        if color.get(node.id.as_str()).copied().unwrap_or(0) == 0 {
            mask_visit(node.id.as_str(), &nodes, &node_index, &mut color)?;
        }
    }

    // Physics globals + one-writer rule.
    check_finite(raw.physics.pixels_per_meter, "pixels_per_meter")?;
    check_finite(raw.physics.gravity, "physics gravity")?;
    if raw.physics.pixels_per_meter <= 0.0 {
        return Err(PuppetError::OutOfRange {
            context: "pixels_per_meter must be positive".to_string(),
        });
    }
    let mut writers: HashSet<&str> = HashSet::new();
    for node in &nodes {
        if let Some(phys) = &node.physics {
            if !param_ids.contains(phys.parameter.as_str()) {
                return Err(PuppetError::DanglingReference {
                    context: format!("physics node {:?} output", node.id),
                    id: phys.parameter.clone(),
                });
            }
            if !writers.insert(phys.parameter.as_str()) {
                return Err(PuppetError::MultipleWriters {
                    parameter: phys.parameter.clone(),
                });
            }
        }
    }

    // Bindings.
    let param_by_id: HashMap<&str, &PuppetParameter> =
        parameters.iter().map(|p| (p.id.as_str(), p)).collect();
    let mut seen_bindings: HashSet<(&str, &str, PuppetBindingKind)> = HashSet::new();
    let mut bindings: Vec<PuppetBinding> = Vec::with_capacity(raw.bindings.len());
    for rb in &raw.bindings {
        let param = param_by_id.get(rb.parameter.as_str()).ok_or_else(|| {
            PuppetError::DanglingReference {
                context: "binding".to_string(),
                id: rb.parameter.clone(),
            }
        })?;
        let node = &nodes[*node_index.get(rb.node.as_str()).ok_or_else(|| {
            PuppetError::DanglingReference {
                context: "binding".to_string(),
                id: rb.node.clone(),
            }
        })?];
        let kind = PuppetBindingKind::from_format_str(&rb.kind).ok_or_else(|| {
            schema(format!(
                "binding on {:?}: unknown kind {:?}",
                rb.node, rb.kind
            ))
        })?;
        let interpolate = PuppetInterpolate::from_format_str(&rb.interpolate).ok_or_else(|| {
            schema(format!(
                "binding on {:?}: unknown interpolate {:?}",
                rb.node, rb.interpolate
            ))
        })?;
        if !seen_bindings.insert((rb.parameter.as_str(), rb.node.as_str(), kind)) {
            return Err(PuppetError::DuplicateId {
                kind: "binding (parameter, node, kind)",
                id: format!("{} -> {} {}", rb.parameter, rb.node, rb.kind),
            });
        }
        // Target-kind discipline (LRP-207).
        match kind {
            PuppetBindingKind::Opacity => {
                if node.drawable.is_none() {
                    return Err(PuppetError::KindMismatch {
                        message: format!("opacity binding targets non-drawable node {:?}", rb.node),
                    });
                }
            }
            PuppetBindingKind::Deform if node.mesh.is_none() && node.group_mesh.is_none() => {
                return Err(PuppetError::KindMismatch {
                    message: format!(
                        "deform binding targets node {:?}, which has no mesh",
                        rb.node
                    ),
                });
            }
            _ => {}
        }
        let nx = param.axis_points[0].len();
        let ny = if param.y.is_some() {
            param.axis_points[1].len()
        } else {
            1
        };
        let values = validate_binding_values(&rb.values, kind, param, node, nx, ny)?;
        bindings.push(PuppetBinding {
            parameter: rb.parameter.clone(),
            node: rb.node.clone(),
            kind,
            interpolate,
            values,
        });
    }

    Ok(PuppetStage {
        model: raw.model,
        generator: raw.generator,
        provenance: raw.provenance.map(|p| PuppetProvenance {
            source_name: p.source_name,
            source_version: p.source_version,
            names: p.names,
        }),
        parameters,
        textures,
        nodes,
        bindings,
        physics: PuppetPhysics {
            pixels_per_meter: raw.physics.pixels_per_meter,
            gravity: raw.physics.gravity,
        },
    })
}

/// Texture source path rules, mirroring the model's LRF-206
/// discipline (relative, no traversal, bounded length).
fn check_texture_source(source: &str) -> Result<(), PuppetError> {
    let bad = source.is_empty()
        || source.len() > PATH_BYTES_MAX
        || source.starts_with('/')
        || source.contains('\\')
        || source.split('/').any(|seg| seg == ".." || seg.is_empty());
    if bad {
        return Err(PuppetError::OutOfRange {
            context: format!("texture source {source:?} is not a clean relative path"),
        });
    }
    Ok(())
}

fn validate_mesh(
    raw: &RawMesh,
    texture_ids: &HashSet<&str>,
    require_texture: bool,
) -> Result<PuppetMesh, PuppetError> {
    check_id(&raw.id)?;
    match &raw.texture {
        Some(texture) => {
            if !texture_ids.contains(texture.as_str()) {
                return Err(PuppetError::DanglingReference {
                    context: format!("mesh {:?}", raw.id),
                    id: texture.clone(),
                });
            }
        }
        None if require_texture => {
            return Err(PuppetError::KindMismatch {
                message: format!("part mesh {:?} carries no texture", raw.id),
            });
        }
        None => {}
    }
    if raw.vertices.is_empty() || raw.vertices.len() > VERTICES_PER_MESH_MAX {
        return Err(PuppetError::TooMany {
            kind: "vertices per mesh",
            count: raw.vertices.len(),
            max: VERTICES_PER_MESH_MAX,
        });
    }
    if raw.uvs.len() != raw.vertices.len() {
        return Err(PuppetError::GridShape {
            context: format!(
                "mesh {:?}: {} uvs for {} vertices",
                raw.id,
                raw.uvs.len(),
                raw.vertices.len()
            ),
        });
    }
    if raw.indices.is_empty()
        || !raw.indices.len().is_multiple_of(3)
        || raw.indices.len() > INDICES_PER_MESH_MAX
    {
        return Err(PuppetError::GridShape {
            context: format!(
                "mesh {:?}: {} indices (must be a positive multiple of 3)",
                raw.id,
                raw.indices.len()
            ),
        });
    }
    let vcount = raw.vertices.len() as u32;
    for &ix in &raw.indices {
        if ix >= vcount {
            return Err(PuppetError::GridShape {
                context: format!("mesh {:?}: index {ix} out of bounds", raw.id),
            });
        }
    }
    for v in &raw.vertices {
        check_vec(v, "mesh vertex")?;
        if v[0].abs() > COORD_MAGNITUDE_MAX || v[1].abs() > COORD_MAGNITUDE_MAX {
            return Err(PuppetError::OutOfRange {
                context: format!("mesh {:?}: vertex beyond coordinate bound", raw.id),
            });
        }
    }
    for uv in &raw.uvs {
        check_vec(uv, "mesh uv")?;
    }
    check_vec(&raw.origin, "mesh origin")?;
    Ok(PuppetMesh {
        id: raw.id.clone(),
        texture: raw.texture.clone(),
        origin: raw.origin,
        vertices: raw.vertices.clone(),
        uvs: raw.uvs.clone(),
        indices: raw.indices.clone(),
    })
}

fn validate_drawable(raw: &RawDrawable, node_id: &str) -> Result<PuppetDrawable, PuppetError> {
    let blend_mode = PuppetBlendMode::from_format_str(&raw.blend_mode).ok_or_else(|| {
        schema(format!(
            "node {node_id:?}: unknown blend_mode {:?}",
            raw.blend_mode
        ))
    })?;
    check_finite(raw.opacity, "drawable opacity")?;
    if !(0.0..=1.0).contains(&raw.opacity) {
        return Err(PuppetError::OutOfRange {
            context: format!("node {node_id:?}: opacity outside [0, 1]"),
        });
    }
    let tint = raw.tint.unwrap_or([1.0, 1.0, 1.0]);
    let screen_tint = raw.screen_tint.unwrap_or([0.0, 0.0, 0.0]);
    for (name, color) in [("tint", tint), ("screen_tint", screen_tint)] {
        check_vec(&color, name)?;
        if color.iter().any(|c| !(0.0..=1.0).contains(c)) {
            return Err(PuppetError::OutOfRange {
                context: format!("node {node_id:?}: {name} channel outside [0, 1]"),
            });
        }
    }
    Ok(PuppetDrawable {
        blend_mode,
        opacity: raw.opacity,
        tint,
        screen_tint,
    })
}

fn validate_physics_node(
    raw: &RawPhysicsNode,
    node_id: &str,
) -> Result<PuppetPhysicsNode, PuppetError> {
    let model = PuppetPhysicsModel::from_format_str(&raw.model).ok_or_else(|| {
        schema(format!(
            "physics node {node_id:?}: unknown model {:?}",
            raw.model
        ))
    })?;
    let map_mode = PuppetMapMode::from_format_str(&raw.map_mode).ok_or_else(|| {
        schema(format!(
            "physics node {node_id:?}: unknown map_mode {:?}",
            raw.map_mode
        ))
    })?;
    for (name, value) in [
        ("gravity", raw.gravity),
        ("length", raw.length),
        ("frequency", raw.frequency),
        ("angle_damping", raw.angle_damping),
        ("length_damping", raw.length_damping),
    ] {
        check_finite(value, name)?;
    }
    check_vec(&raw.output_scale, "output_scale")?;
    if raw.length <= 0.0 {
        return Err(PuppetError::OutOfRange {
            context: format!("physics node {node_id:?}: length must be positive"),
        });
    }
    if model == PuppetPhysicsModel::Spring && raw.frequency <= 0.0 {
        return Err(PuppetError::OutOfRange {
            context: format!("physics node {node_id:?}: spring frequency must be positive"),
        });
    }
    Ok(PuppetPhysicsNode {
        parameter: raw.parameter.clone(),
        model,
        map_mode,
        local_only: raw.local_only.unwrap_or(false),
        gravity: raw.gravity,
        length: raw.length,
        frequency: raw.frequency,
        angle_damping: raw.angle_damping,
        length_damping: raw.length_damping,
        output_scale: raw.output_scale,
    })
}

fn grid_f32(value: &serde_json::Value, context: &str) -> Result<f32, PuppetError> {
    match value.as_f64() {
        Some(v) => {
            let v = v as f32;
            check_finite(v, context)?;
            Ok(v)
        }
        None => Err(schema(format!("{context}: expected a number"))),
    }
}

fn validate_binding_values(
    raw: &serde_json::Value,
    kind: PuppetBindingKind,
    param: &PuppetParameter,
    node: &PuppetNode,
    nx: usize,
    ny: usize,
) -> Result<PuppetBindingValues, PuppetError> {
    let context = format!("binding {} -> {}", param.id, node.id);
    let outer = raw
        .as_array()
        .ok_or_else(|| schema(format!("{context}: values must be an array")))?;
    if outer.len() != nx {
        return Err(PuppetError::GridShape {
            context: format!(
                "{context}: grid has {} x cells for {nx} x axis points",
                outer.len()
            ),
        });
    }
    if kind.is_deform() {
        let mesh = node
            .mesh
            .as_ref()
            .or(node.group_mesh.as_ref())
            .expect("checked by caller");
        let vcount = mesh.vertices.len();
        let mut grid: Vec<Vec<Vec<[f32; 2]>>> = Vec::with_capacity(nx);
        for (xi, row) in outer.iter().enumerate() {
            let row = row
                .as_array()
                .ok_or_else(|| schema(format!("{context}: row {xi} not an array")))?;
            if row.len() != ny {
                return Err(PuppetError::GridShape {
                    context: format!("{context}: row {xi} has {} y cells for {ny}", row.len()),
                });
            }
            let mut out_row: Vec<Vec<[f32; 2]>> = Vec::with_capacity(ny);
            for cell in row {
                let cell = cell
                    .as_array()
                    .ok_or_else(|| schema(format!("{context}: deform cell not an array")))?;
                if cell.len() != vcount {
                    return Err(PuppetError::GridShape {
                        context: format!(
                            "{context}: deform cell has {} vertices, mesh has {vcount}",
                            cell.len()
                        ),
                    });
                }
                let mut out_cell: Vec<[f32; 2]> = Vec::with_capacity(vcount);
                for pair in cell {
                    let pair = pair
                        .as_array()
                        .ok_or_else(|| schema(format!("{context}: displacement not a pair")))?;
                    if pair.len() != 2 {
                        return Err(schema(format!("{context}: displacement not a pair")));
                    }
                    out_cell.push([grid_f32(&pair[0], &context)?, grid_f32(&pair[1], &context)?]);
                }
                out_row.push(out_cell);
            }
            grid.push(out_row);
        }
        Ok(PuppetBindingValues::Deform(grid))
    } else {
        let mut grid: Vec<Vec<f32>> = Vec::with_capacity(nx);
        for (xi, row) in outer.iter().enumerate() {
            let row = row
                .as_array()
                .ok_or_else(|| schema(format!("{context}: row {xi} not an array")))?;
            if row.len() != ny {
                return Err(PuppetError::GridShape {
                    context: format!("{context}: row {xi} has {} y cells for {ny}", row.len()),
                });
            }
            let mut out_row: Vec<f32> = Vec::with_capacity(ny);
            for cell in row {
                out_row.push(grid_f32(cell, &context)?);
            }
            grid.push(out_row);
        }
        Ok(PuppetBindingValues::Scalar(grid))
    }
}

/* ------------------------------------------------------------------ */
/* Canonical emission                                                  */
/* ------------------------------------------------------------------ */

#[derive(Serialize)]
struct StageOut<'a> {
    format: &'static str,
    format_version: u32,
    model: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    generator: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    provenance: Option<ProvenanceOut<'a>>,
    parameters: Vec<ParameterOut<'a>>,
    textures: Vec<TextureOut<'a>>,
    nodes: Vec<NodeOut<'a>>,
    bindings: Vec<BindingOut<'a>>,
    physics: PhysicsOut,
}

#[derive(Serialize)]
struct ProvenanceOut<'a> {
    source_name: &'a str,
    source_version: &'a str,
    names: &'a [[String; 2]],
}

#[derive(Serialize)]
struct ParameterOut<'a> {
    id: &'a str,
    x: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    y: Option<&'a str>,
    min: [f32; 2],
    max: [f32; 2],
    default: [f32; 2],
    axis_points: &'a [Vec<f32>; 2],
}

#[derive(Serialize)]
struct TextureOut<'a> {
    id: &'a str,
    source: &'a str,
}

#[derive(Serialize)]
struct NodeOut<'a> {
    id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    parent: Option<&'a str>,
    kind: &'static str,
    enabled: bool,
    lock_to_root: bool,
    zsort: f32,
    transform: TransformOut,
    #[serde(skip_serializing_if = "Option::is_none")]
    mesh: Option<MeshOut<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    group_mesh: Option<MeshOut<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    drawable: Option<DrawableOut>,
    masks: Vec<MaskOut<'a>>,
    mask_threshold: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    physics: Option<PhysicsNodeOut<'a>>,
}

#[derive(Serialize)]
struct TransformOut {
    translation: [f32; 3],
    rotation_degrees: [f32; 3],
    scale: [f32; 2],
    pixel_snap: bool,
}

#[derive(Serialize)]
struct MeshOut<'a> {
    id: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    texture: Option<&'a str>,
    origin: [f32; 2],
    vertices: &'a [[f32; 2]],
    uvs: &'a [[f32; 2]],
    indices: &'a [u32],
}

#[derive(Serialize)]
struct DrawableOut {
    blend_mode: &'static str,
    opacity: f32,
    tint: [f32; 3],
    screen_tint: [f32; 3],
}

#[derive(Serialize)]
struct MaskOut<'a> {
    source: &'a str,
    mode: &'static str,
}

#[derive(Serialize)]
struct PhysicsNodeOut<'a> {
    parameter: &'a str,
    model: &'static str,
    map_mode: &'static str,
    local_only: bool,
    gravity: f32,
    length: f32,
    frequency: f32,
    angle_damping: f32,
    length_damping: f32,
    output_scale: [f32; 2],
}

#[derive(Serialize)]
struct BindingOut<'a> {
    parameter: &'a str,
    node: &'a str,
    kind: &'static str,
    interpolate: &'static str,
    values: &'a PuppetBindingValues,
}

#[derive(Serialize)]
struct PhysicsOut {
    pixels_per_meter: f32,
    gravity: f32,
}

impl Serialize for PuppetBindingValues {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            PuppetBindingValues::Scalar(grid) => grid.serialize(serializer),
            PuppetBindingValues::Deform(grid) => grid.serialize(serializer),
        }
    }
}

impl PuppetStage {
    /// Emit the canonical JSON form (SPEC 13.3): spec field
    /// order, 2-space indent, trailing newline. Law:
    /// `parse ∘ emit` is the identity on validated stages and
    /// `emit ∘ parse ∘ emit` is byte-stable.
    pub fn to_canonical_json(&self) -> String {
        fn mesh_out(m: &PuppetMesh) -> MeshOut<'_> {
            MeshOut {
                id: &m.id,
                texture: m.texture.as_deref(),
                origin: m.origin,
                vertices: &m.vertices,
                uvs: &m.uvs,
                indices: &m.indices,
            }
        }
        let out = StageOut {
            format: PUPPET_FORMAT,
            format_version: PUPPET_FORMAT_VERSION,
            model: &self.model,
            generator: self.generator.as_deref(),
            provenance: self.provenance.as_ref().map(|p| ProvenanceOut {
                source_name: &p.source_name,
                source_version: &p.source_version,
                names: &p.names,
            }),
            parameters: self
                .parameters
                .iter()
                .map(|p| ParameterOut {
                    id: &p.id,
                    x: &p.x,
                    y: p.y.as_deref(),
                    min: p.min,
                    max: p.max,
                    default: p.default,
                    axis_points: &p.axis_points,
                })
                .collect(),
            textures: self
                .textures
                .iter()
                .map(|t| TextureOut {
                    id: &t.id,
                    source: &t.source,
                })
                .collect(),
            nodes: self
                .nodes
                .iter()
                .map(|n| NodeOut {
                    id: &n.id,
                    parent: n.parent.as_deref(),
                    kind: n.kind.to_format_str(),
                    enabled: n.enabled,
                    lock_to_root: n.lock_to_root,
                    zsort: n.zsort,
                    transform: TransformOut {
                        translation: n.transform.translation,
                        rotation_degrees: n.transform.rotation_degrees,
                        scale: n.transform.scale,
                        pixel_snap: n.transform.pixel_snap,
                    },
                    mesh: n.mesh.as_ref().map(mesh_out),
                    group_mesh: n.group_mesh.as_ref().map(mesh_out),
                    drawable: n.drawable.as_ref().map(|d| DrawableOut {
                        blend_mode: d.blend_mode.to_format_str(),
                        opacity: d.opacity,
                        tint: d.tint,
                        screen_tint: d.screen_tint,
                    }),
                    masks: n
                        .masks
                        .iter()
                        .map(|m| MaskOut {
                            source: &m.source,
                            mode: m.mode.to_format_str(),
                        })
                        .collect(),
                    mask_threshold: n.mask_threshold,
                    physics: n.physics.as_ref().map(|p| PhysicsNodeOut {
                        parameter: &p.parameter,
                        model: p.model.to_format_str(),
                        map_mode: p.map_mode.to_format_str(),
                        local_only: p.local_only,
                        gravity: p.gravity,
                        length: p.length,
                        frequency: p.frequency,
                        angle_damping: p.angle_damping,
                        length_damping: p.length_damping,
                        output_scale: p.output_scale,
                    }),
                })
                .collect(),
            bindings: self
                .bindings
                .iter()
                .map(|b| BindingOut {
                    parameter: &b.parameter,
                    node: &b.node,
                    kind: b.kind.to_format_str(),
                    interpolate: b.interpolate.to_format_str(),
                    values: &b.values,
                })
                .collect(),
            physics: PhysicsOut {
                pixels_per_meter: self.physics.pixels_per_meter,
                gravity: self.physics.gravity,
            },
        };
        let mut text = serde_json::to_string_pretty(&out).expect("stage emission cannot fail");
        text.push('\n');
        text
    }
}
