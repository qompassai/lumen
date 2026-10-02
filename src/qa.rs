//! Scene validation, animation audit, frame comparison, raw Lua hatch.
//!
//! Contract summary:
//! - `validate_scene` reads the stored JSON leniently and *reports* structural
//!   problems instead of failing on the first one; only path/IO failures are
//!   errors. The other read-only tools load (and so fully validate) the
//!   document first and fail with a typed error if it is invalid.
//! - Reports are bounded: at most `MESSAGES_MAX` errors / warnings / issues;
//!   the rest are counted in a trailing "suppressed" note.
//! - `run_lua_script` is an experimental, opt-in escape hatch; see its docs.

#![forbid(unsafe_code)]

use std::cell::{Cell, RefCell};
use std::collections::HashSet;
use std::path::Path;
use std::rc::Rc;

use image::RgbaImage;
use mlua::{ChunkMode, HookTriggers, Lua, LuaOptions, StdLib, Value, VmState};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::doc::{
    DOC_MAX_FRAMES, DOC_MAX_LAYERS, DOC_MAX_NAME_CHARS, DOC_MAX_OFFSET_PX, DOC_VERSION, DocSaved,
    Frame, SpriteDoc, Tag, check_dims, composite_frame, load_doc, resolve_doc_path, save_doc,
};
use crate::{LumenError, MAX_SPRITE_BYTES};

/// Cap on reported errors, warnings, or issues per list.
const MESSAGES_MAX: usize = 256;
const DURATION_WARN_MS: u32 = 10_000;
const SCRIPT_BYTES_MAX: usize = 65_536;
const LUA_MEMORY_BYTES_MAX: usize = 64 * 1024 * 1024;
/// VM instructions between budget checks (the count-hook period).
const LUA_HOOK_PERIOD_INSTRUCTIONS: u32 = 1_000;
const LUA_INSTRUCTIONS_MAX: u64 = 100_000_000;
/// Total pixels `sprite.fill` may write per script (~16 full 4096^2 fills).
const LUA_FILL_PIXELS_MAX: u64 = 1 << 28;
const LUA_ERROR_CHARS_MAX: usize = 1_024;
/// Base-library globals removed from the sandbox. See `run_lua_script`.
const LUA_REMOVED_GLOBALS: [&str; 10] = [
    "dofile",
    "loadfile",
    "load",
    "require",
    "print",
    "warn",
    "pcall",
    "xpcall",
    "setmetatable",
    "collectgarbage",
];

// ---------------------------------------------------------------------------
// Requests and results
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ValidateSceneRequest {
    /// Project-relative `.lumen.json`.
    pub doc: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AuditAnimationRequest {
    /// Project-relative `.lumen.json`.
    pub doc: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CompareFramesRequest {
    /// Project-relative `.lumen.json`.
    pub doc: String,
    /// 0-based frame index.
    pub a: usize,
    /// 0-based frame index.
    pub b: usize,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RunLuaScriptRequest {
    /// Project-relative input `.lumen.json`.
    pub doc: String,
    /// Output `.lumen.json`; `None` saves over the input.
    pub output: Option<String>,
    /// Lua 5.4 source, at most 65536 bytes. Text only; bytecode is refused.
    pub script: String,
    /// Must be `true`; the tool refuses to run otherwise.
    pub opt_in: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct SceneReport {
    /// True when no structural error was found.
    pub valid: bool,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AnimationAudit {
    pub issues: Vec<AnimIssue>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AnimIssue {
    /// `"warning"` | `"error"`.
    pub severity: String,
    pub frame: Option<usize>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct FrameDiff {
    pub identical: bool,
    pub differing_pixels: u64,
    pub total_pixels: u64,
    /// `differing_pixels / total_pixels`, 0.0..=1.0.
    pub diff_ratio: f32,
    /// Bounding box of differing pixels; `None` when identical.
    pub bbox: Option<BBox>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct BBox {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

// ---------------------------------------------------------------------------
// validate_scene
// ---------------------------------------------------------------------------

/// Lenient view of the stored document: just enough to report problems that
/// `load_doc` would reject one at a time. Unknown fields (layer PNG payloads,
/// palette) are skipped by serde.
#[derive(Deserialize)]
struct RawLayer {
    name: String,
    visible: bool,
    opacity: f32,
}

#[derive(Deserialize)]
struct RawDoc {
    version: u32,
    width: u32,
    height: u32,
    layers: Vec<RawLayer>,
    frames: Vec<Frame>,
    tags: Vec<Tag>,
}

#[derive(Default)]
struct Findings {
    errors: Vec<String>,
    warnings: Vec<String>,
    errors_suppressed: u64,
    warnings_suppressed: u64,
}

impl Findings {
    fn error(&mut self, msg: String) {
        if self.errors.len() < MESSAGES_MAX {
            self.errors.push(msg);
        } else {
            self.errors_suppressed += 1;
        }
    }

    fn warning(&mut self, msg: String) {
        if self.warnings.len() < MESSAGES_MAX {
            self.warnings.push(msg);
        } else {
            self.warnings_suppressed += 1;
        }
    }

    fn has_errors(&self) -> bool {
        !self.errors.is_empty()
    }

    fn into_report(mut self) -> SceneReport {
        if self.errors_suppressed > 0 {
            self.errors.push(format!("{} further errors suppressed", self.errors_suppressed));
        }
        if self.warnings_suppressed > 0 {
            let note = format!("{} further warnings suppressed", self.warnings_suppressed);
            self.warnings.push(note);
        }
        SceneReport { valid: self.errors.is_empty(), errors: self.errors, warnings: self.warnings }
    }
}

/// Validate a stored document and report every structural error (would make
/// `load_doc` fail) and warning (legal but suspicious: invisible layers,
/// duplicate tag names). When the metadata checks pass, a full `load_doc`
/// also runs so PNG/base64 payload failures surface as errors too.
///
/// Empty layer/tag names are reported as *errors*, not warnings, because
/// `load_doc` rejects them: such a document cannot be used by any tool.
pub async fn validate_scene(
    root: &Path,
    req: ValidateSceneRequest,
) -> Result<SceneReport, LumenError> {
    let path = resolve_doc_path(&req.doc, root, true)?;
    let meta = std::fs::metadata(&path).map_err(LumenError::from)?;
    if meta.len() > MAX_SPRITE_BYTES {
        return Err(LumenError::PathRejected(format!(
            "document larger than {MAX_SPRITE_BYTES} bytes"
        )));
    }
    let bytes = std::fs::read(&path).map_err(LumenError::from)?;
    let mut findings = Findings::default();
    match serde_json::from_slice::<RawDoc>(&bytes) {
        Err(e) => findings.error(format!("json parse failed: {e}")),
        Ok(raw) => {
            check_canvas_and_layers(&raw, &mut findings);
            check_frames(&raw, &mut findings);
            check_tags(&raw, &mut findings);
        }
    }
    if !findings.has_errors()
        && let Err(e) = load_doc(root, &req.doc)
    {
        findings.error(format!("full load failed: {e}"));
    }
    Ok(findings.into_report())
}

fn check_name(what: &str, name: &str, findings: &mut Findings) {
    if name.is_empty() {
        findings.error(format!("{what} has an empty name"));
    } else if name.chars().count() > DOC_MAX_NAME_CHARS {
        findings.error(format!("{what} name exceeds {DOC_MAX_NAME_CHARS} characters"));
    }
}

fn check_canvas_and_layers(raw: &RawDoc, findings: &mut Findings) {
    if raw.version != DOC_VERSION {
        findings.error(format!("unsupported document version {}", raw.version));
    }
    if let Err(e) = check_dims(raw.width, raw.height) {
        findings.error(e.to_string());
    }
    if raw.layers.is_empty() || raw.layers.len() > DOC_MAX_LAYERS {
        findings.error(format!("layer count {} outside 1..={DOC_MAX_LAYERS}", raw.layers.len()));
    }
    for (idx, layer) in raw.layers.iter().take(DOC_MAX_LAYERS).enumerate() {
        check_name(&format!("layer {idx}"), &layer.name, findings);
        if !layer.opacity.is_finite() || !(0.0..=1.0).contains(&layer.opacity) {
            findings.error(format!("layer {idx}: opacity {} outside 0.0..=1.0", layer.opacity));
        }
        if !layer.visible {
            findings.warning(format!("layer {idx} ({:.32}) is invisible", layer.name));
        }
    }
}

fn check_frames(raw: &RawDoc, findings: &mut Findings) {
    if raw.frames.is_empty() || raw.frames.len() > DOC_MAX_FRAMES {
        findings.error(format!("frame count {} outside 1..={DOC_MAX_FRAMES}", raw.frames.len()));
    }
    let offset_max = DOC_MAX_OFFSET_PX.unsigned_abs();
    for (fidx, frame) in raw.frames.iter().take(DOC_MAX_FRAMES).enumerate() {
        if frame.layer_mods.len() != raw.layers.len() {
            findings.error(format!(
                "frame {fidx}: {} layer mods for {} layers",
                frame.layer_mods.len(),
                raw.layers.len()
            ));
        }
        for (midx, lm) in frame.layer_mods.iter().take(DOC_MAX_LAYERS).enumerate() {
            // unsigned_abs: i32::MIN.abs() would overflow.
            if lm.offset_x.unsigned_abs() > offset_max || lm.offset_y.unsigned_abs() > offset_max {
                findings.error(format!(
                    "frame {fidx} mod {midx}: offset ({}, {}) exceeds +-{DOC_MAX_OFFSET_PX}px",
                    lm.offset_x, lm.offset_y
                ));
            }
            if !lm.opacity_mult.is_finite() || !(0.0..=1.0).contains(&lm.opacity_mult) {
                findings.error(format!(
                    "frame {fidx} mod {midx}: opacity_mult {} outside 0.0..=1.0",
                    lm.opacity_mult
                ));
            }
            if !lm.scale.is_finite() || !(0.0625..=16.0).contains(&lm.scale) {
                findings.error(format!(
                    "frame {fidx} mod {midx}: scale {} outside 0.0625..=16.0",
                    lm.scale
                ));
            }
        }
    }
}

fn check_tags(raw: &RawDoc, findings: &mut Findings) {
    let mut seen: HashSet<&str> = HashSet::new();
    for (idx, tag) in raw.tags.iter().enumerate() {
        check_name(&format!("tag {idx}"), &tag.name, findings);
        if tag.from_frame > tag.to_frame || tag.to_frame as usize >= raw.frames.len() {
            findings.error(format!(
                "tag {idx}: range {}..={} outside 0..{}",
                tag.from_frame,
                tag.to_frame,
                raw.frames.len()
            ));
        }
        if !seen.insert(tag.name.as_str()) {
            findings.warning(format!("duplicate tag name {:.32}", tag.name));
        }
    }
}

// ---------------------------------------------------------------------------
// audit_animation
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Issues {
    list: Vec<AnimIssue>,
    suppressed: u64,
}

impl Issues {
    fn push(&mut self, severity: &str, frame: Option<usize>, message: String) {
        if self.list.len() < MESSAGES_MAX {
            self.list.push(AnimIssue { severity: severity.to_string(), frame, message });
        } else {
            self.suppressed += 1;
        }
    }
}

/// Audit timing and motion: zero-length frames (error), frames longer than
/// 10 s (warning), "dead" frames whose composite is pixel-identical to the
/// previous frame (warning), empty or overlapping tags (warning), and layers
/// invisible in every frame (warning). Composites every frame once, keeping
/// only the previous one in memory.
pub async fn audit_animation(
    root: &Path,
    req: AuditAnimationRequest,
) -> Result<AnimationAudit, LumenError> {
    let doc = load_doc(root, &req.doc)?;
    let mut issues = Issues::default();
    for (idx, frame) in doc.frames.iter().enumerate() {
        if frame.duration_ms == 0 {
            issues.push("error", Some(idx), "duration_ms is 0".to_string());
        } else if frame.duration_ms > DURATION_WARN_MS {
            let msg = format!("duration_ms {} exceeds {DURATION_WARN_MS}", frame.duration_ms);
            issues.push("warning", Some(idx), msg);
        }
    }
    let mut prev = composite_frame(&doc, 0)?;
    for idx in 1..doc.frames.len() {
        let cur = composite_frame(&doc, idx)?;
        if cur.as_raw() == prev.as_raw() {
            let msg = format!("dead frame: composite identical to frame {}", idx - 1);
            issues.push("warning", Some(idx), msg);
        }
        prev = cur;
    }
    audit_tags(&doc, &mut issues);
    for (lidx, layer) in doc.layers.iter().enumerate() {
        let hidden_everywhere = !layer.visible
            || layer.opacity <= 0.0
            || doc
                .frames
                .iter()
                .all(|f| f.layer_mods.get(lidx).is_none_or(|m| m.opacity_mult <= 0.0));
        if hidden_everywhere {
            let msg = format!("layer {lidx} ({:.32}) is invisible in every frame", layer.name);
            issues.push("warning", None, msg);
        }
    }
    let mut list = issues.list;
    if issues.suppressed > 0 {
        let message = format!("{} further issues suppressed", issues.suppressed);
        list.push(AnimIssue { severity: "warning".to_string(), frame: None, message });
    }
    Ok(AnimationAudit { issues: list })
}

/// Empty tags, then overlaps via one sort + sweep (O(T log T), no pairwise
/// scan over an unbounded tag list).
fn audit_tags(doc: &SpriteDoc, issues: &mut Issues) {
    let frames = doc.frames.len();
    let mut valid: Vec<&Tag> = Vec::new();
    for tag in &doc.tags {
        if tag.from_frame > tag.to_frame || tag.from_frame as usize >= frames {
            issues.push("warning", None, format!("tag {:.32} covers no frames", tag.name));
        } else {
            valid.push(tag);
        }
    }
    valid.sort_by_key(|t| (t.from_frame, t.to_frame));
    let mut reach: Option<&Tag> = None;
    for tag in valid {
        if let Some(prev) = reach {
            if tag.from_frame <= prev.to_frame {
                let msg = format!("tag {:.32} overlaps tag {:.32}", tag.name, prev.name);
                issues.push("warning", None, msg);
            }
            if tag.to_frame > prev.to_frame {
                reach = Some(tag);
            }
        } else {
            reach = Some(tag);
        }
    }
}

// ---------------------------------------------------------------------------
// compare_frames
// ---------------------------------------------------------------------------

/// Pixel-exact comparison of the composites of frames `a` and `b`.
pub async fn compare_frames(
    root: &Path,
    req: CompareFramesRequest,
) -> Result<FrameDiff, LumenError> {
    let doc = load_doc(root, &req.doc)?;
    let img_a = composite_frame(&doc, req.a)?;
    let img_b = composite_frame(&doc, req.b)?;
    let mut differing = 0u64;
    let mut bounds: Option<(u32, u32, u32, u32)> = None;
    for ((x, y, pa), pb) in img_a.enumerate_pixels().zip(img_b.pixels()) {
        if pa == pb {
            continue;
        }
        differing += 1;
        bounds = Some(match bounds {
            None => (x, y, x, y),
            Some((x0, y0, x1, y1)) => (x0.min(x), y0.min(y), x1.max(x), y1.max(y)),
        });
    }
    let total = u64::from(doc.width) * u64::from(doc.height);
    Ok(FrameDiff {
        identical: differing == 0,
        differing_pixels: differing,
        total_pixels: total,
        diff_ratio: (differing as f64 / total as f64) as f32,
        bbox: bounds.map(|(x0, y0, x1, y1)| BBox { x: x0, y: y0, w: x1 - x0 + 1, h: y1 - y0 + 1 }),
    })
}

// ---------------------------------------------------------------------------
// run_lua_script
// ---------------------------------------------------------------------------

/// EXPERIMENTAL raw Lua 5.4 hatch. Refuses unless `opt_in` is `true`.
///
/// The script runs against a working copy of the document's layer pixels;
/// the result is saved like any mutating tool only if the script finishes
/// without error (otherwise nothing is written).
///
/// Sandbox, exactly:
/// - Libraries: `string`, `table`, `math`, `utf8`, plus the Lua base library
///   minus `dofile`, `loadfile`, `load`, `require`, `print`, `warn`, `pcall`,
///   `xpcall`, `setmetatable`, `collectgarbage`. Not opened at all: `io`,
///   `os`, `debug`, `package`, `coroutine`. `print`/`warn` are gone because
///   stdout is the MCP transport; `pcall`/`xpcall` because they could catch
///   the budget error; `setmetatable` because `__gc` finalizer errors are
///   swallowed by Lua 5.4 and could also absorb it.
/// - The chunk is loaded in text mode; precompiled bytecode is refused.
/// - `sprite` table (layer indices and coordinates are 0-based integers;
///   channels are integers 0..=255, anything else raises an error):
///   `sprite.width()`, `sprite.height()`, `sprite.layers()`,
///   `sprite.get(layer, x, y)` -> `{r=, g=, b=, a=}` (out-of-canvas raises),
///   `sprite.set(layer, x, y, r, g, b, a)` (out-of-canvas coordinates are
///   clipped silently), `sprite.fill(layer, r, g, b, a)`.
/// - Budgets: 64 MiB Lua heap (mlua memory limit; layer pixels live in Rust
///   and are not counted), 100M VM instructions (count hook every 1000), and
///   2^28 total pixels written by `sprite.fill`.
/// - Residual risk: the count hook cannot interrupt C functions, so a
///   pathological `string.find`/`gsub` pattern can still burn CPU. The script
///   runs synchronously on the calling task's thread.
///
/// Lua errors (runtime, memory, budget) become `LumenError::DocInvalid` with
/// the Lua message, truncated to 1024 characters.
pub async fn run_lua_script(
    root: &Path,
    req: RunLuaScriptRequest,
) -> Result<DocSaved, LumenError> {
    if !req.opt_in {
        return Err(LumenError::BadParam(
            "refused: run_lua_script requires explicit opt_in=true".to_string(),
        ));
    }
    if req.script.len() > SCRIPT_BYTES_MAX {
        return Err(LumenError::BadParam(format!(
            "script is {} bytes; limit is {SCRIPT_BYTES_MAX}",
            req.script.len()
        )));
    }
    let target = output_target(&req.doc, req.output.as_deref())?;
    let mut doc = load_doc(root, &req.doc)?;
    let images: Vec<RgbaImage> = doc
        .layers
        .iter_mut()
        .map(|l| std::mem::replace(&mut l.image, RgbaImage::new(0, 0)))
        .collect();
    let images = run_sandboxed(&req.script, images, doc.width, doc.height)?;
    assert_eq!(images.len(), doc.layers.len(), "the sandbox never adds or drops layers");
    for (layer, image) in doc.layers.iter_mut().zip(images) {
        layer.image = image;
    }
    let path = save_doc(root, &doc, target)?;
    Ok(DocSaved::of(&path, &doc))
}

/// `None` keeps the input path; an explicit output must be a sprite document.
fn output_target<'a>(doc: &'a str, output: Option<&'a str>) -> Result<&'a str, LumenError> {
    match output {
        None => Ok(doc),
        Some(o) if o.ends_with(".lumen.json") => Ok(o),
        Some(_) => Err(LumenError::BadParam("output must end in .lumen.json".to_string())),
    }
}

fn lua_err(e: mlua::Error) -> LumenError {
    let msg: String = e.to_string().chars().take(LUA_ERROR_CHARS_MAX).collect();
    LumenError::DocInvalid(format!("lua: {msg}"))
}

fn runtime(msg: String) -> mlua::Error {
    mlua::Error::RuntimeError(msg)
}

fn run_sandboxed(
    script: &str,
    images: Vec<RgbaImage>,
    width: u32,
    height: u32,
) -> Result<Vec<RgbaImage>, LumenError> {
    let libs = StdLib::STRING | StdLib::TABLE | StdLib::MATH | StdLib::UTF8;
    let lua = Lua::new_with(libs, LuaOptions::default()).map_err(lua_err)?;
    lua.set_memory_limit(LUA_MEMORY_BYTES_MAX).map_err(lua_err)?;
    let globals = lua.globals();
    for name in LUA_REMOVED_GLOBALS {
        globals.set(name, Value::Nil).map_err(lua_err)?;
    }
    let executed = Cell::new(0u64);
    let triggers = HookTriggers::new().every_nth_instruction(LUA_HOOK_PERIOD_INSTRUCTIONS);
    lua.set_hook(triggers, move |_lua, _debug| {
        let total = executed.get() + u64::from(LUA_HOOK_PERIOD_INSTRUCTIONS);
        executed.set(total);
        if total > LUA_INSTRUCTIONS_MAX {
            return Err(runtime(format!("instruction budget of {LUA_INSTRUCTIONS_MAX} exceeded")));
        }
        Ok(VmState::Continue)
    })
    .map_err(lua_err)?;
    let state = Rc::new(RefCell::new(images));
    install_sprite_api(&lua, &state, width, height).map_err(lua_err)?;
    lua.load(script)
        .set_name("script")
        .set_mode(ChunkMode::Text)
        .exec()
        .map_err(lua_err)?;
    let images = std::mem::take(&mut *state.try_borrow_mut().map_err(|_| {
        LumenError::DocInvalid("lua: sprite state still borrowed".to_string())
    })?);
    Ok(images)
}

type Layers = Rc<RefCell<Vec<RgbaImage>>>;

fn layer_index(images: &[RgbaImage], layer: i64) -> mlua::Result<usize> {
    usize::try_from(layer)
        .ok()
        .filter(|&idx| idx < images.len())
        .ok_or_else(|| runtime(format!("layer {layer} out of range ({} layers)", images.len())))
}

/// Strict integer arguments: Lua integers, or floats with no fractional part
/// (e.g. `4/2`). mlua's own i64 conversion truncates `0.5` silently.
fn ints<const N: usize>(values: [Value; N]) -> mlua::Result<[i64; N]> {
    let mut out = [0i64; N];
    for (slot, value) in out.iter_mut().zip(values) {
        *slot = match value {
            Value::Integer(i) => i,
            Value::Number(f) if f.fract() == 0.0 && f.abs() < 9.0e15 => f as i64,
            other => {
                return Err(runtime(format!("expected an integer, got {}", other.type_name())));
            }
        };
    }
    Ok(out)
}

fn channels(rgba: [i64; 4]) -> mlua::Result<[u8; 4]> {
    let mut out = [0u8; 4];
    for (slot, value) in out.iter_mut().zip(rgba) {
        *slot = u8::try_from(value)
            .map_err(|_| runtime(format!("channel {value} outside 0..=255")))?;
    }
    Ok(out)
}

/// `Some((x, y))` when the coordinate lies on the canvas.
fn on_canvas(image: &RgbaImage, x: i64, y: i64) -> Option<(u32, u32)> {
    let x = u32::try_from(x).ok().filter(|&x| x < image.width())?;
    let y = u32::try_from(y).ok().filter(|&y| y < image.height())?;
    Some((x, y))
}

fn install_sprite_api(lua: &Lua, state: &Layers, width: u32, height: u32) -> mlua::Result<()> {
    let sprite = lua.create_table()?;
    sprite.set("width", lua.create_function(move |_, ()| Ok(width))?)?;
    sprite.set("height", lua.create_function(move |_, ()| Ok(height))?)?;
    let st = Rc::clone(state);
    let count = lua.create_function(move |_, ()| {
        let images = st.try_borrow().map_err(|_| runtime("sprite state busy".to_string()))?;
        Ok(images.len())
    })?;
    sprite.set("layers", count)?;
    let st = Rc::clone(state);
    let get = lua.create_function(move |lua, args: (Value, Value, Value)| {
        let [layer, x, y] = ints([args.0, args.1, args.2])?;
        let images = st.try_borrow().map_err(|_| runtime("sprite state busy".to_string()))?;
        let image = &images[layer_index(&images, layer)?];
        let (px, py) = on_canvas(image, x, y)
            .ok_or_else(|| runtime(format!("get: ({x}, {y}) is outside the canvas")))?;
        let p = image.get_pixel(px, py);
        let t = lua.create_table()?;
        t.set("r", p[0])?;
        t.set("g", p[1])?;
        t.set("b", p[2])?;
        t.set("a", p[3])?;
        Ok(t)
    })?;
    sprite.set("get", get)?;
    let st = Rc::clone(state);
    type SetArgs = (Value, Value, Value, Value, Value, Value, Value);
    let set = lua.create_function(move |_, args: SetArgs| {
        let [layer, x, y, r, g, b, a] =
            ints([args.0, args.1, args.2, args.3, args.4, args.5, args.6])?;
        let rgba = channels([r, g, b, a])?;
        let mut images =
            st.try_borrow_mut().map_err(|_| runtime("sprite state busy".to_string()))?;
        let idx = layer_index(&images, layer)?;
        let image = &mut images[idx];
        // Documented: off-canvas writes are clipped silently.
        if let Some((px, py)) = on_canvas(image, x, y) {
            image.put_pixel(px, py, image::Rgba(rgba));
        }
        Ok(())
    })?;
    sprite.set("set", set)?;
    let st = Rc::clone(state);
    let filled = Cell::new(0u64);
    let fill = lua.create_function(move |_, args: (Value, Value, Value, Value, Value)| {
        let [layer, r, g, b, a] = ints([args.0, args.1, args.2, args.3, args.4])?;
        let rgba = channels([r, g, b, a])?;
        let mut images =
            st.try_borrow_mut().map_err(|_| runtime("sprite state busy".to_string()))?;
        let idx = layer_index(&images, layer)?;
        let total = filled.get() + u64::from(width) * u64::from(height);
        if total > LUA_FILL_PIXELS_MAX {
            return Err(runtime(format!("fill budget of {LUA_FILL_PIXELS_MAX} pixels exceeded")));
        }
        filled.set(total);
        for px in images[idx].pixels_mut() {
            px.0 = rgba;
        }
        Ok(())
    })?;
    sprite.set("fill", fill)?;
    lua.globals().set("sprite", sprite)
}
