//! The A2A skill registry: one static table that both dispatch and the agent
//! card read, so the advertised surface cannot drift from what executes.
//!
//! Every entry calls the SAME library operation the MCP tool calls; this
//! file adds only argument decoding and result encoding. Deliberately absent:
//! `run_lua_script` (the raw Lua hatch stays MCP-only behind its per-call
//! opt-in; an external agent must never be able to flip it).

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;

use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::{
    LumenError, bevy, dream, export, inspect, palette, pipeline, qa, sprite_ops, style, text,
};

/// Boxed operation future. `Send` so tasks run on the multi-thread runtime.
pub(crate) type OpFuture = Pin<Box<dyn Future<Output = Result<Value, LumenError>> + Send>>;

/// One operation: owned project root plus raw JSON arguments.
pub(crate) type OpFn = fn(PathBuf, Value) -> OpFuture;

/// Capability groups advertised on the agent card (A2A_DESIGN.md). A group
/// appears on the card only when at least one skill carries it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Capability {
    SpriteCore,
    LightshowPipeline,
    Fidelity,
    Styles,
    DreamAutomation,
    ExportBackends,
    BevyBridge,
}

impl Capability {
    /// Kebab-case wire id, used as the skill tag on the card.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::SpriteCore => "sprite-core",
            Self::LightshowPipeline => "lightshow-pipeline",
            Self::Fidelity => "fidelity",
            Self::Styles => "styles",
            Self::DreamAutomation => "dream-automation",
            Self::ExportBackends => "export-backends",
            Self::BevyBridge => "bevy-bridge",
        }
    }
}

/// One dispatchable skill. `name` equals the MCP tool name.
pub struct SkillSpec {
    pub name: &'static str,
    pub capability: Capability,
    pub description: &'static str,
    pub(crate) run: OpFn,
}

impl std::fmt::Debug for SkillSpec {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SkillSpec")
            .field("name", &self.name)
            .field("capability", &self.capability)
            .finish_non_exhaustive()
    }
}

/// Look up a skill by exact name.
pub fn find_skill(name: &str) -> Option<&'static SkillSpec> {
    SKILLS.iter().find(|s| s.name == name)
}

fn decode_args<T: DeserializeOwned>(args: Value) -> Result<T, LumenError> {
    serde_json::from_value(args).map_err(|e| LumenError::BadParam(format!("arguments: {e}")))
}

fn encode_output<T: serde::Serialize>(out: &T) -> Result<Value, LumenError> {
    serde_json::to_value(out).map_err(|e| LumenError::Io(format!("result encode failed: {e}")))
}

/// Wrap `async fn(&Path, Req) -> Result<Out, _>`; `Req` is inferred from `$f`.
macro_rules! op {
    ($f:path) => {
        |root: PathBuf, args: Value| -> OpFuture {
            Box::pin(async move {
                let req = decode_args(args)?;
                let out = $f(&root, req).await?;
                encode_output(&out)
            })
        }
    };
}

/// Wrap `async fn(Req) -> Result<Out, _>` for operations without a root.
macro_rules! op_rootless {
    ($f:path) => {
        |_root: PathBuf, args: Value| -> OpFuture {
            Box::pin(async move {
                let req = decode_args(args)?;
                let out = $f(req).await?;
                encode_output(&out)
            })
        }
    };
}

/// Arguments for the two root-level tools that take a bare path.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct PathArgs {
    path: String,
}

async fn sprite_info_op(root: &Path, req: PathArgs) -> Result<crate::SpriteInfo, LumenError> {
    crate::sprite_info(root, &req.path)
}

async fn validate_atlas_op(root: &Path, req: PathArgs) -> Result<crate::AtlasReport, LumenError> {
    crate::validate_atlas(root, &req.path)
}

macro_rules! skill {
    ($name:literal, $cap:ident, $desc:literal, $run:expr) => {
        SkillSpec {
            name: $name,
            capability: Capability::$cap,
            description: $desc,
            run: $run,
        }
    };
}

/// The registry. Order is the card order; names are unique (asserted by the
/// card builder).
pub static SKILLS: &[SkillSpec] = &[
    skill!("sprite_info", SpriteCore, "Report size, color type and alpha of a .png.",
        op!(sprite_info_op)),
    skill!("new_sprite", SpriteCore, "Create a .lumen.json document.", op!(sprite_ops::new_sprite)),
    skill!("add_layer", SpriteCore, "Add a layer.", op!(sprite_ops::add_layer)),
    skill!("delete_layer", SpriteCore, "Delete a layer.", op!(sprite_ops::delete_layer)),
    skill!("reorder_layer", SpriteCore, "Move a layer in the stack.",
        op!(sprite_ops::reorder_layer)),
    skill!("rename_layer", SpriteCore, "Rename a layer.", op!(sprite_ops::rename_layer)),
    skill!("set_layer_props", SpriteCore, "Set layer visibility, opacity, blend.",
        op!(sprite_ops::set_layer_props)),
    skill!("add_frame", SpriteCore, "Add an animation frame.", op!(sprite_ops::add_frame)),
    skill!("delete_frame", SpriteCore, "Delete a frame.", op!(sprite_ops::delete_frame)),
    skill!("set_frame_duration", SpriteCore, "Set a frame duration.",
        op!(sprite_ops::set_frame_duration)),
    skill!("set_frame_mod", SpriteCore, "Set a per-frame layer offset/opacity/scale.",
        op!(sprite_ops::set_frame_mod)),
    skill!("add_tag", SpriteCore, "Add a named frame range.", op!(sprite_ops::add_tag)),
    skill!("delete_tag", SpriteCore, "Delete a tag.", op!(sprite_ops::delete_tag)),
    skill!("set_pixel", SpriteCore, "Set one pixel.", op!(sprite_ops::set_pixel)),
    skill!("fill_rect", SpriteCore, "Fill a rectangle.", op!(sprite_ops::fill_rect)),
    skill!("draw_line", SpriteCore, "Draw a line.", op!(sprite_ops::draw_line)),
    skill!("draw_circle", SpriteCore, "Draw a circle.", op!(sprite_ops::draw_circle)),
    skill!("flood_fill", SpriteCore, "Flood-fill a region.", op!(sprite_ops::flood_fill)),
    skill!("flip", SpriteCore, "Flip a layer or document.", op!(sprite_ops::flip)),
    skill!("rotate", SpriteCore, "Rotate by a right angle.", op!(sprite_ops::rotate)),
    skill!("resize_canvas", SpriteCore, "Resize the canvas.", op!(sprite_ops::resize_canvas)),
    skill!("crop", SpriteCore, "Crop the canvas.", op!(sprite_ops::crop)),
    skill!("tween_frames", SpriteCore, "Interpolate frames.", op!(sprite_ops::tween_frames)),
    skill!("inspect_sprite", SpriteCore, "Summarize a document.", op!(inspect::inspect_sprite)),
    skill!("inspect_layer", SpriteCore, "Summarize one layer.", op!(inspect::inspect_layer)),
    skill!("histogram", SpriteCore, "Color histogram.", op!(inspect::histogram)),
    skill!("color_usage", SpriteCore, "Where a color is used.", op!(inspect::color_usage)),
    skill!("palette_presets", SpriteCore, "List built-in palettes.",
        op!(palette::palette_presets)),
    skill!("palette_apply", SpriteCore, "Map a document onto a palette.",
        op!(palette::palette_apply)),
    skill!("palette_extract", SpriteCore, "Extract a palette.", op!(palette::palette_extract)),
    skill!("palette_ramp", SpriteCore, "Build a color ramp.", op!(palette::palette_ramp)),
    skill!("quantize", SpriteCore, "Reduce colors.", op!(palette::quantize)),
    skill!("text_rasterize", SpriteCore, "Rasterize bitmap text onto a layer.",
        op!(text::text_rasterize)),
    skill!("text_measure", SpriteCore, "Measure bitmap text.", op_rootless!(text::text_measure)),
    skill!("validate_atlas", LightshowPipeline, "Check the Light Show 384x1152 atlas contract.",
        op!(validate_atlas_op)),
    skill!("build_fullbody_sheet", LightshowPipeline, "Assemble a full-body sheet.",
        op!(pipeline::build_fullbody_sheet)),
    skill!("generate_mature_variant", LightshowPipeline, "Derive a mature variant.",
        op!(pipeline::generate_mature_variant)),
    skill!("contact_sheet", LightshowPipeline, "Render a review contact sheet.",
        op!(pipeline::contact_sheet)),
    skill!("validate_scene", LightshowPipeline, "QA a scene document.",
        op!(qa::validate_scene)),
    skill!("audit_animation", LightshowPipeline, "QA animation timing and tags.",
        op!(qa::audit_animation)),
    skill!("compare_frames", LightshowPipeline, "Diff two frames.", op!(qa::compare_frames)),
    skill!("fidelity_set", Fidelity, "Apply a fidelity preset.", op!(style::fidelity_set)),
    skill!("style_list", Styles, "List style presets.", op!(style::style_list)),
    skill!("style_apply", Styles, "Apply a style preset.", op!(style::style_apply)),
    skill!("dream_ambient", DreamAutomation, "Generate ambient sparkle frames.",
        op!(dream::dream_ambient)),
    skill!("dream_act", DreamAutomation, "Animate a performance.", op!(dream::dream_act)),
    skill!("dream_transition", DreamAutomation, "Animate a transition.",
        op!(dream::dream_transition)),
    skill!("dream_text", DreamAutomation, "Animate a text effect.", op!(dream::dream_text)),
    skill!("export_sprite", ExportBackends, "Export a frame to .png.",
        op!(export::export_sprite)),
    skill!("export_sheet", ExportBackends, "Export a sprite sheet + metadata.",
        op!(export::export_sheet)),
    skill!("export_tag", ExportBackends, "Export a tag as a strip.", op!(export::export_tag)),
    skill!("import_layer", ExportBackends, "Import a .png as a layer.",
        op!(export::import_layer)),
    skill!("bevy_discover", BevyBridge, "Classify a BRP endpoint (loopback unless opted in).",
        op_rootless!(bevy::bevy_status)),
    skill!("bevy_status", BevyBridge, "Report BRP endpoint reachability.",
        op_rootless!(bevy::bevy_status)),
    skill!("bevy_call", BevyBridge, "Pass one JSON-RPC call to a BRP endpoint.",
        op_rootless!(bevy::bevy_call)),
];
