//! Cubism JSON sidecar formats: model3, cdi3, physics3, motion3,
//! exp3. These are the interchange JSON documents the Cubism
//! editor/exporter emits next to a `.moc3`. Field names follow the
//! files as written (PascalCase) via serde renames.
//!
//! Parsing is tolerant of *absent* optional fields (older exports
//! omit them) but never of wrong types: a schema violation is a
//! loud `ConvertError::Json`.

use std::collections::BTreeMap;

use serde::Deserialize;

// ---------------------------------------------------------------------
// model3.json
// ---------------------------------------------------------------------

/// A `.model3.json` document: the model's file manifest.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Model3 {
    pub version: u32,
    pub file_references: FileReferences,
    #[serde(default)]
    pub groups: Vec<GroupEntry>,
    #[serde(default)]
    pub hit_areas: Vec<HitArea>,
}

/// The `FileReferences` block of a model3 document.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct FileReferences {
    pub moc: String,
    #[serde(default)]
    pub textures: Vec<String>,
    pub physics: Option<String>,
    pub pose: Option<String>,
    pub display_info: Option<String>,
    #[serde(default)]
    pub expressions: Vec<ExpressionRef>,
    #[serde(default)]
    pub motions: BTreeMap<String, Vec<MotionRef>>,
    pub user_data: Option<String>,
    /// Any further reference kinds a newer exporter adds. They are
    /// inventoried here so the conversion report can name them as
    /// dropped-with-reason instead of the parser ignoring them.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// One entry of `FileReferences.Expressions`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ExpressionRef {
    pub name: String,
    pub file: String,
}

/// One entry of a `FileReferences.Motions` group.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MotionRef {
    pub file: String,
    pub fade_in_time: Option<f32>,
    pub fade_out_time: Option<f32>,
    pub sound: Option<String>,
    pub text: Option<String>,
}

/// One entry of `Groups` (EyeBlink / LipSync style groupings).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct GroupEntry {
    pub target: String,
    pub name: String,
    #[serde(default)]
    pub ids: Vec<String>,
}

/// One entry of `HitAreas`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct HitArea {
    pub id: String,
    pub name: String,
}

// ---------------------------------------------------------------------
// cdi3.json
// ---------------------------------------------------------------------

/// A `.cdi3.json` display-info document: names and groupings for
/// the editor UI. Carries no geometry; the converter cross-checks
/// its inventory against the moc3 and reports discrepancies.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Cdi3 {
    pub version: u32,
    #[serde(default)]
    pub parameters: Vec<CdiParameter>,
    #[serde(default)]
    pub parameter_groups: Vec<CdiNamed>,
    #[serde(default)]
    pub parts: Vec<CdiNamed>,
}

/// One cdi3 parameter entry (display name + owning group).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CdiParameter {
    pub id: String,
    pub group_id: Option<String>,
    pub name: String,
}

/// A cdi3 entry that is just an id and a display name.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CdiNamed {
    pub id: String,
    pub name: String,
}

// ---------------------------------------------------------------------
// physics3.json
// ---------------------------------------------------------------------

/// A `.physics3.json` document.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Physics3 {
    pub version: u32,
    pub meta: PhysicsMeta,
    #[serde(default)]
    pub physics_settings: Vec<PhysicsSetting>,
}

/// The `Meta` block of a physics3 document.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PhysicsMeta {
    pub physics_setting_count: u32,
    pub total_input_count: Option<u32>,
    pub total_output_count: Option<u32>,
    pub vertex_count: Option<u32>,
    pub effective_forces: EffectiveForces,
    #[serde(default)]
    pub physics_dictionary: Vec<CdiNamed>,
}

/// Scene-wide forces in a physics3 document (unitless direction
/// vectors in the source format; conventionally gravity (0, -1)).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct EffectiveForces {
    pub gravity: Vec2,
    pub wind: Vec2,
}

/// A 2D vector as written in Cubism JSON (`{"X": .., "Y": ..}`).
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

/// One physics setting: inputs -> pendulum chain -> outputs.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PhysicsSetting {
    pub id: String,
    #[serde(default)]
    pub input: Vec<PhysicsInput>,
    #[serde(default)]
    pub output: Vec<PhysicsOutput>,
    #[serde(default)]
    pub vertices: Vec<PhysicsVertex>,
    pub normalization: PhysicsNormalization,
}

/// One physics input: a parameter read through a normalization
/// window, weighted. `input_type` is `"X"` (position) or `"Angle"`.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PhysicsInput {
    pub source: PhysicsEndpoint,
    pub weight: f32,
    #[serde(rename = "Type")]
    pub input_type: String,
    #[serde(default)]
    pub reflect: bool,
}

/// One physics output: writes a parameter from the pendulum angle
/// at `vertex_index`, scaled. `output_type` is `"Angle"` in every
/// observed export.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PhysicsOutput {
    pub destination: PhysicsEndpoint,
    pub vertex_index: u32,
    pub scale: f32,
    pub weight: f32,
    #[serde(rename = "Type", default = "default_output_type")]
    pub output_type: String,
    #[serde(default)]
    pub reflect: bool,
}

fn default_output_type() -> String {
    "Angle".to_string()
}

/// A `{Target, Id}` endpoint reference in physics3.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PhysicsEndpoint {
    pub target: String,
    pub id: String,
}

/// One vertex of a physics pendulum chain.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PhysicsVertex {
    pub position: Vec2,
    pub mobility: f32,
    pub delay: f32,
    pub acceleration: f32,
    pub radius: f32,
}

/// The normalization windows of a physics setting, per input type.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct PhysicsNormalization {
    pub position: NormalizationRange,
    pub angle: NormalizationRange,
}

/// One normalization window: source parameter values are remapped
/// so `default` is the center and `minimum`/`maximum` the extremes.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct NormalizationRange {
    pub minimum: f32,
    pub default: f32,
    pub maximum: f32,
}

// ---------------------------------------------------------------------
// motion3.json
// ---------------------------------------------------------------------

/// A `.motion3.json` document.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Motion3 {
    pub version: u32,
    pub meta: MotionMeta,
    #[serde(default)]
    pub curves: Vec<MotionCurve>,
    #[serde(default)]
    pub user_data: Vec<MotionUserData>,
}

/// The `Meta` block of a motion3 document.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MotionMeta {
    pub duration: f32,
    pub fps: f32,
    #[serde(rename = "Loop")]
    pub loop_motion: bool,
    pub are_beziers_restricted: Option<bool>,
    pub curve_count: Option<u32>,
    pub total_segment_count: Option<u32>,
    pub total_point_count: Option<u32>,
}

/// One motion curve. `segments` is the packed Cubism segment
/// stream: an initial `(time, value)` point, then per segment a
/// type tag (0 linear, 1 bezier, 2 stepped, 3 inverse-stepped)
/// followed by the segment's points.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MotionCurve {
    pub target: String,
    pub id: String,
    pub fade_in_time: Option<f32>,
    pub fade_out_time: Option<f32>,
    #[serde(default)]
    pub segments: Vec<f32>,
}

/// One motion user-data event (consumer-layer data).
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct MotionUserData {
    pub time: f32,
    pub value: String,
}

// ---------------------------------------------------------------------
// exp3.json
// ---------------------------------------------------------------------

/// An `.exp3.json` expression document.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct Exp3 {
    #[serde(rename = "Type")]
    pub expression_type: Option<String>,
    pub fade_in_time: Option<f32>,
    pub fade_out_time: Option<f32>,
    #[serde(default)]
    pub parameters: Vec<ExpParameter>,
}

/// One expression parameter assignment. `blend` is `"Add"`,
/// `"Multiply"`, or `"Overwrite"`; absent means Overwrite.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ExpParameter {
    pub id: String,
    pub value: f32,
    pub blend: Option<String>,
}
