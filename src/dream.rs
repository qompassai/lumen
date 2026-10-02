//! Dream automation: ambient, acting, transitions, kinetic text.
//!
//! Every tool animates through per-frame `LayerMod`s (offset, opacity
//! multiplier, scale) and/or appended or inserted frames. Pixel content is
//! never rewritten, except that `dream_ambient` appends one generated
//! sparkle layer.
//!
//! Determinism: randomness comes only from a private `xorshift64` generator
//! whose state is derived from the caller's `seed` through `splitmix64`
//! (so seed 0 is valid and nearby seeds diverge immediately). No state is
//! kept between calls: same input doc + same seed + same params produces a
//! byte-identical output doc on the same build. (`f32::sin`/`cos` come from
//! the platform math library, so bit-exactness across platforms is not
//! promised — only within one build.)
//!
//! Contract summary:
//! - Accepted: an existing `.lumen.json` document inside the project root;
//!   an optional `.lumen.json` output (`None` = save in place).
//! - Rejected: unknown performance/kind/effect names (the error lists the
//!   valid values), out-of-range counts and indices, results that would
//!   exceed `DOC_MAX_FRAMES` or `DOC_MAX_LAYERS` — all `BadParam`, raised
//!   before the document is touched.
//! - Bounds: appended frames <= 96 per call, sparkles <= 512, transition
//!   steps <= 64; every loop is bounded by those or the existing doc limits.

#![forbid(unsafe_code)]

use std::f32::consts::{PI, TAU};
use std::path::Path;

use image::{Rgba, RgbaImage};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::LumenError;
use crate::doc::{
    BlendMode, DOC_MAX_FRAMES, DOC_MAX_LAYERS, DocSaved, Frame, Layer, LayerMod, SpriteDoc,
    identity_mod, load_doc, save_doc, sync_frame_mods,
};

/// Name of the layer `dream_ambient` appends.
pub const AMBIENT_LAYER_NAME: &str = "dream_ambient";

const AMBIENT_FRAMES_DEFAULT: u32 = 8;
const AMBIENT_FRAMES_MAX: u32 = 64;
const AMBIENT_SPARKLES_DEFAULT: u32 = 64;
const AMBIENT_SPARKLES_MAX: u32 = 512;
const AMBIENT_FRAME_MS: u32 = 120;
const TRANSITION_STEPS_MAX: u32 = 64;
const TRANSITION_FRAME_MS: u32 = 90;
const TEXT_FRAMES_DEFAULT: u32 = 12;
const TEXT_FRAMES_MAX: u32 = 96;
const TEXT_FRAME_MS: u32 = 90;
/// Golden angle in radians, π(3 − √5): successive frames never repeat phase.
const GOLDEN_ANGLE_RAD: f32 = 2.399_963;
/// The iris closes to the smallest scale a document accepts (0.0625), not
/// 0.05: anything smaller fails document validation on save.
const IRIS_MIN_SCALE: f32 = 0.0625;
const BLOOM_PEAK_SCALE: f32 = 1.6;
const BLOOM_OPACITY_DIP: f32 = 0.4;
/// Probability that a glitch frame flickers down to `GLITCH_DIM_OPACITY`.
const GLITCH_FLICKER_PROBABILITY: f32 = 0.3;
const GLITCH_DIM_OPACITY: f32 = 0.2;

const PERFORMANCES: [&str; 3] = ["idle", "breathe", "bounce"];
const TRANSITION_KINDS: [&str; 4] = ["dissolve", "wipe", "iris", "bloom"];
const TEXT_EFFECTS: [&str; 3] = ["typewriter", "glitch", "pulse"];

// ---------------------------------------------------------------------------
// Public contract
// ---------------------------------------------------------------------------

/// Request for `dream_ambient`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct DreamAmbientReq {
    /// Project-relative input `.lumen.json` document.
    pub doc: String,
    /// Project-relative output `.lumen.json`; omit to overwrite the input.
    pub output: Option<String>,
    /// RNG seed; same seed + params = identical output.
    pub seed: u64,
    /// Frames to append, 1..=64 (default 8).
    pub frames: Option<u32>,
    /// Sparkle pixels to scatter, 0..=512 (default 64).
    pub sparkles: Option<u32>,
    /// Slowly drift the sparkle layer.
    pub drift: bool,
}

/// Request for `dream_act`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct DreamActReq {
    /// Project-relative input `.lumen.json` document.
    pub doc: String,
    /// Project-relative output `.lumen.json`; omit to overwrite the input.
    pub output: Option<String>,
    /// Perturbs the loop phase deterministically.
    pub seed: u64,
    /// One of: idle, breathe, bounce.
    pub performance: String,
}

/// Request for `dream_transition`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct DreamTransitionReq {
    /// Project-relative input `.lumen.json` document.
    pub doc: String,
    /// Project-relative output `.lumen.json`; omit to overwrite the input.
    pub output: Option<String>,
    /// Frame index the transition starts from (new frames go after it).
    pub from_frame: usize,
    /// Frame index the transition lands on. Must differ from `from_frame`.
    pub to_frame: usize,
    /// One of: dissolve, wipe, iris, bloom.
    pub kind: String,
    /// Frames to insert, 1..=64.
    pub steps: u32,
}

/// Request for `dream_text`.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct DreamTextReq {
    /// Project-relative input `.lumen.json` document.
    pub doc: String,
    /// Project-relative output `.lumen.json`; omit to overwrite the input.
    pub output: Option<String>,
    /// Index of the (already rasterized) text layer to animate.
    pub text_layer: usize,
    /// RNG seed (used by `glitch`).
    pub seed: u64,
    /// One of: typewriter, glitch, pulse.
    pub effect: String,
    /// Frames to append, 1..=96 (default 12).
    pub frames: Option<u32>,
}

/// Append a twinkling sparkle overlay and `frames` new frames (120 ms each).
///
/// Adds one layer named `dream_ambient` (rejected if one already exists)
/// holding `sparkles` white, fully opaque single pixels at seeded positions
/// (two sparkles may land on the same pixel). Existing frames get the
/// identity mod for the new layer. Each appended frame copies the last
/// existing frame's mods for the other layers; the ambient layer's mod in
/// frame `k` gets `opacity_mult = 0.35 + 0.65 * (0.5 + 0.5 * sin(k * golden
/// + phase))`, where `phase` is derived statelessly from the seed. With
/// `drift`, it also gets `offset = (round(3 sin(0.7k)), round(2 cos(0.5k)))`.
///
/// Limitation: a `LayerMod` applies to a whole layer, so all sparkles share
/// one twinkle phase; per-sparkle phases cannot be expressed in the format.
///
/// Errors: frames outside 1..=64, sparkles above 512, a full layer stack, or
/// a frame list that would exceed `DOC_MAX_FRAMES` → `BadParam`.
pub async fn dream_ambient(root: &Path, req: DreamAmbientReq) -> Result<DocSaved, LumenError> {
    let frames = req.frames.unwrap_or(AMBIENT_FRAMES_DEFAULT);
    check_count("frames", frames, 1, AMBIENT_FRAMES_MAX)?;
    let sparkles = req.sparkles.unwrap_or(AMBIENT_SPARKLES_DEFAULT);
    check_count("sparkles", sparkles, 0, AMBIENT_SPARKLES_MAX)?;
    let target = output_target(&req.doc, req.output.as_deref())?;
    let mut doc = load_doc(root, &req.doc)?;
    if doc.layers.len() >= DOC_MAX_LAYERS {
        return Err(LumenError::BadParam(format!(
            "document already has the maximum {DOC_MAX_LAYERS} layers"
        )));
    }
    if doc.layers.iter().any(|l| l.name == AMBIENT_LAYER_NAME) {
        return Err(LumenError::BadParam(format!(
            "document already has a {AMBIENT_LAYER_NAME:?} layer"
        )));
    }
    check_frame_room(&doc, frames)?;

    let mut rng = XorShift64::new(req.seed);
    let mut image = RgbaImage::from_pixel(doc.width, doc.height, Rgba([0, 0, 0, 0]));
    for _ in 0..sparkles {
        let x = rng.next_index(doc.width);
        let y = rng.next_index(doc.height);
        image.put_pixel(x, y, Rgba([255, 255, 255, 255]));
    }
    doc.layers.push(Layer {
        name: AMBIENT_LAYER_NAME.to_string(),
        visible: true,
        opacity: 1.0,
        blend: BlendMode::Normal,
        image,
    });
    sync_frame_mods(&mut doc);

    let ambient_idx = doc.layers.len() - 1;
    let phase_rad = unit_f32(splitmix64(req.seed ^ 0xA5A5_A5A5_A5A5_A5A5)) * TAU;
    let base = last_frame_mods(&doc);
    for k in 0..frames {
        let kf = k as f32;
        let mut mods = base.clone();
        let wave = 0.5 + 0.5 * (kf * GOLDEN_ANGLE_RAD + phase_rad).sin();
        mods[ambient_idx].opacity_mult = (0.35 + 0.65 * wave).clamp(0.0, 1.0);
        if req.drift {
            mods[ambient_idx].offset_x = (3.0 * (kf * 0.7).sin()).round() as i32;
            mods[ambient_idx].offset_y = (2.0 * (kf * 0.5).cos()).round() as i32;
        }
        doc.frames.push(Frame { duration_ms: AMBIENT_FRAME_MS, layer_mods: mods });
    }
    save_to(root, target, &doc)
}

/// Rewrite every frame's mods with a looping performance.
///
/// **DESTRUCTIVE TO EXISTING MODS: every `LayerMod` of every frame and
/// every layer is OVERWRITTEN** (offsets, opacity multiplier reset to 1.0,
/// scale). Durations, tags, and pixels are untouched. With `n` frames,
/// `θ = 2πk/n + seed_phase`, `seed_phase = (seed % 1000) / 1000 · 2π`:
/// - idle: `scale = 1 + 0.02 sin θ`, `offset_y = round(sin θ)`.
/// - breathe: `scale = 1 + 0.04 sin(πk/n + seed_phase)` (half cycle per
///   loop, so slower), offsets 0.
/// - bounce: `offset_y = -round(4 |sin(πk/n + seed_phase)|)`,
///   `scale = 1 + 0.03 sin θ`.
///
/// Errors: unknown performance → `BadParam` listing valid values; fewer
/// than 2 frames → `BadParam`.
pub async fn dream_act(root: &Path, req: DreamActReq) -> Result<DocSaved, LumenError> {
    let performance = find_name("performance", &req.performance, &PERFORMANCES)?;
    let target = output_target(&req.doc, req.output.as_deref())?;
    let mut doc = load_doc(root, &req.doc)?;
    let frame_count = doc.frames.len();
    if frame_count < 2 {
        return Err(LumenError::BadParam(format!(
            "dream_act needs at least 2 frames; document has {frame_count}"
        )));
    }
    let seed_phase = (req.seed % 1000) as f32 / 1000.0 * TAU;
    let n = frame_count as f32;
    for (k, frame) in doc.frames.iter_mut().enumerate() {
        let kf = k as f32;
        let full = TAU * kf / n + seed_phase;
        let half = PI * kf / n + seed_phase;
        let (offset_y, scale) = match performance {
            "idle" => (full.sin().round() as i32, 1.0 + 0.02 * full.sin()),
            "breathe" => (0, 1.0 + 0.04 * half.sin()),
            _ => (-(4.0 * half.sin().abs()).round() as i32, 1.0 + 0.03 * full.sin()),
        };
        let m = LayerMod { offset_x: 0, offset_y, opacity_mult: 1.0, scale };
        for slot in &mut frame.layer_mods {
            *slot = m.clone();
        }
    }
    save_to(root, target, &doc)
}

/// Insert `steps` in-between frames (90 ms each) right after `from_frame`,
/// interpolating every layer's mod from frame A (`from_frame`) to frame B
/// (`to_frame`). With `t = i / (steps + 1)` for step `i` in 1..=steps,
/// offsets, opacity, and scale lerp A→B, then the kind overrides:
/// - dissolve: opacity fades A→0 over the first half, 0→B over the second.
/// - wipe: `offset_x` slides A→−width, then enters from +width → B.
/// - iris: scale shrinks A→0.0625 then grows 0.0625→B (0.0625 is the
///   document's minimum scale; the brief's 0.05 would fail validation).
/// - bloom: scale pulses A→1.6→B; opacity dips to 0.6× at mid-way.
///
/// Tags whose endpoints lie after `from_frame` shift by `steps`, so they
/// keep naming the same original frames.
///
/// Errors: `from == to`, indices out of range, steps outside 1..=64, unknown
/// kind (lists valid values), or exceeding `DOC_MAX_FRAMES` → `BadParam`.
pub async fn dream_transition(
    root: &Path,
    req: DreamTransitionReq,
) -> Result<DocSaved, LumenError> {
    let kind = find_name("kind", &req.kind, &TRANSITION_KINDS)?;
    check_count("steps", req.steps, 1, TRANSITION_STEPS_MAX)?;
    if req.from_frame == req.to_frame {
        return Err(LumenError::BadParam(
            "from_frame and to_frame must differ".to_string(),
        ));
    }
    let target = output_target(&req.doc, req.output.as_deref())?;
    let mut doc = load_doc(root, &req.doc)?;
    let frame_count = doc.frames.len();
    if req.from_frame >= frame_count || req.to_frame >= frame_count {
        return Err(LumenError::BadParam(format!(
            "from_frame {} / to_frame {} out of range ({frame_count} frames)",
            req.from_frame, req.to_frame
        )));
    }
    check_frame_room(&doc, req.steps)?;

    let from_mods = doc.frames[req.from_frame].layer_mods.clone();
    let to_mods = doc.frames[req.to_frame].layer_mods.clone();
    let width = doc.width as f32;
    let mut inserted = Vec::with_capacity(req.steps as usize);
    for i in 1..=req.steps {
        let t = i as f32 / (req.steps + 1) as f32;
        let mods = from_mods
            .iter()
            .zip(to_mods.iter())
            .map(|(a, b)| transition_mod(kind, a, b, t, width))
            .collect();
        inserted.push(Frame { duration_ms: TRANSITION_FRAME_MS, layer_mods: mods });
    }
    let at = req.from_frame + 1;
    doc.frames.splice(at..at, inserted);
    shift_tags_after(&mut doc, req.from_frame, req.steps)?;
    save_to(root, target, &doc)
}

/// Append `frames` new frames (90 ms each) that animate ONLY layer
/// `text_layer`; other layers copy the last existing frame's mods. The text
/// layer's mod starts from identity in each new frame, then:
/// - typewriter: `offset_y` eases out from `+height` to 0 (cubic), opacity
///   rises 0→1 linearly (first frame hidden, last frame at rest).
/// - glitch: seeded jitter, `offset_x/y` ∈ -2..=2, opacity flickers between
///   1.0 and 0.2 (deterministic in `seed`).
/// - pulse: `scale = 1 + 0.08 sin(2πk / frames)`.
///
/// Pair with a text-rasterizing tool for the layer's pixel content.
///
/// Errors: bad layer index, frames outside 1..=96, unknown effect (lists
/// valid values), or exceeding `DOC_MAX_FRAMES` → `BadParam`.
pub async fn dream_text(root: &Path, req: DreamTextReq) -> Result<DocSaved, LumenError> {
    let effect = find_name("effect", &req.effect, &TEXT_EFFECTS)?;
    let frames = req.frames.unwrap_or(TEXT_FRAMES_DEFAULT);
    check_count("frames", frames, 1, TEXT_FRAMES_MAX)?;
    let target = output_target(&req.doc, req.output.as_deref())?;
    let mut doc = load_doc(root, &req.doc)?;
    if req.text_layer >= doc.layers.len() {
        return Err(LumenError::BadParam(format!(
            "text_layer {} out of range ({} layers)",
            req.text_layer,
            doc.layers.len()
        )));
    }
    check_frame_room(&doc, frames)?;

    let mut rng = XorShift64::new(req.seed);
    let height = doc.height as f32;
    let base = last_frame_mods(&doc);
    for k in 0..frames {
        let kf = k as f32;
        let mut m = identity_mod();
        match effect {
            "typewriter" => {
                let t = if frames > 1 { kf / (frames - 1) as f32 } else { 1.0 };
                let eased = 1.0 - (1.0 - t).powi(3);
                m.offset_y = (height * (1.0 - eased)).round() as i32;
                m.opacity_mult = t.clamp(0.0, 1.0);
            }
            "glitch" => {
                m.offset_x = rng.next_jitter();
                m.offset_y = rng.next_jitter();
                if rng.next_f32() < GLITCH_FLICKER_PROBABILITY {
                    m.opacity_mult = GLITCH_DIM_OPACITY;
                }
            }
            _ => m.scale = 1.0 + 0.08 * (TAU * kf / frames as f32).sin(),
        }
        let mut mods = base.clone();
        mods[req.text_layer] = m;
        doc.frames.push(Frame { duration_ms: TEXT_FRAME_MS, layer_mods: mods });
    }
    save_to(root, target, &doc)
}

// ---------------------------------------------------------------------------
// Deterministic RNG
// ---------------------------------------------------------------------------

/// SplitMix64 finalizer: a bijective 64-bit mix used to derive xorshift
/// state from any seed (including 0) and to hash (seed, salt) pairs.
fn splitmix64(x: u64) -> u64 {
    let mut z = x.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Top 24 bits of a u64 as an f32 in [0, 1). Exact: 24 bits fit the mantissa.
fn unit_f32(bits: u64) -> f32 {
    (bits >> 40) as f32 / (1u64 << 24) as f32
}

/// Marsaglia xorshift64 (13, 7, 17). State is never zero.
struct XorShift64 {
    state: u64,
}

impl XorShift64 {
    fn new(seed: u64) -> Self {
        let mixed = splitmix64(seed);
        // splitmix64 is a bijection, so exactly one seed maps to 0.
        let state = if mixed == 0 { 0x9E37_79B9_7F4A_7C15 } else { mixed };
        Self { state }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    /// A float in [0, 1).
    fn next_f32(&mut self) -> f32 {
        unit_f32(self.next_u64())
    }

    /// An index in 0..bound. `bound` must be >= 1.
    fn next_index(&mut self, bound: u32) -> u32 {
        assert!(bound >= 1, "document dimensions are validated >= 1");
        (self.next_u64() % u64::from(bound)) as u32
    }

    /// An offset in -2..=2.
    fn next_jitter(&mut self) -> i32 {
        (self.next_u64() % 5) as i32 - 2
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn find_name(
    what: &str,
    name: &str,
    valid: &[&'static str],
) -> Result<&'static str, LumenError> {
    valid.iter().copied().find(|v| *v == name).ok_or_else(|| {
        LumenError::BadParam(format!("unknown {what} {name:?}; valid: {}", valid.join(", ")))
    })
}

fn check_count(what: &str, value: u32, min: u32, max: u32) -> Result<(), LumenError> {
    if !(min..=max).contains(&value) {
        return Err(LumenError::BadParam(format!(
            "{what} {value} is outside {min}..={max}"
        )));
    }
    Ok(())
}

fn check_frame_room(doc: &SpriteDoc, added: u32) -> Result<(), LumenError> {
    let total = doc.frames.len().saturating_add(added as usize);
    if total > DOC_MAX_FRAMES {
        return Err(LumenError::BadParam(format!(
            "adding {added} frames would exceed the {DOC_MAX_FRAMES}-frame limit"
        )));
    }
    Ok(())
}

/// The last frame's mods, the base new frames build on. Loaded documents
/// always have >= 1 frame with one mod per layer.
fn last_frame_mods(doc: &SpriteDoc) -> Vec<LayerMod> {
    let last = doc.frames.last().map(|f| f.layer_mods.clone());
    let mods = last.unwrap_or_default();
    assert_eq!(mods.len(), doc.layers.len(), "frame mods align with layers");
    mods
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Two-leg path a→mid over t in [0, 0.5), then mid→b over [0.5, 1].
fn via(a: f32, mid: f32, b: f32, t: f32) -> f32 {
    if t < 0.5 { lerp(a, mid, t * 2.0) } else { lerp(mid, b, t * 2.0 - 1.0) }
}

fn transition_mod(kind: &str, a: &LayerMod, b: &LayerMod, t: f32, width: f32) -> LayerMod {
    let mut offset_x = lerp(a.offset_x as f32, b.offset_x as f32, t);
    let offset_y = lerp(a.offset_y as f32, b.offset_y as f32, t);
    let mut opacity = lerp(a.opacity_mult, b.opacity_mult, t);
    let mut scale = lerp(a.scale, b.scale, t);
    match kind {
        "dissolve" => opacity = via(a.opacity_mult, 0.0, b.opacity_mult, t),
        "wipe" => {
            offset_x = if t < 0.5 {
                lerp(a.offset_x as f32, -width, t * 2.0)
            } else {
                lerp(width, b.offset_x as f32, t * 2.0 - 1.0)
            };
        }
        "iris" => scale = via(a.scale, IRIS_MIN_SCALE, b.scale, t),
        _ => {
            scale = via(a.scale, BLOOM_PEAK_SCALE, b.scale, t);
            opacity *= 1.0 - BLOOM_OPACITY_DIP * (PI * t).sin();
        }
    }
    LayerMod {
        offset_x: offset_x.round() as i32,
        offset_y: offset_y.round() as i32,
        opacity_mult: opacity.clamp(0.0, 1.0),
        scale: scale.clamp(IRIS_MIN_SCALE, 16.0),
    }
}

/// Shift tag endpoints that sit after `after` by `count` inserted frames.
fn shift_tags_after(doc: &mut SpriteDoc, after: usize, count: u32) -> Result<(), LumenError> {
    let after = u32::try_from(after)
        .map_err(|_| LumenError::BadParam("frame index exceeds u32".to_string()))?;
    let shift = |v: u32| if v > after { v.checked_add(count) } else { Some(v) };
    for tag in &mut doc.tags {
        match (shift(tag.from_frame), shift(tag.to_frame)) {
            (Some(from), Some(to)) => {
                tag.from_frame = from;
                tag.to_frame = to;
            }
            _ => return Err(LumenError::BadParam("tag index overflow".to_string())),
        }
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Save plumbing (duplicated per module by design; a later pass dedups)
// ---------------------------------------------------------------------------

fn output_target<'a>(input: &'a str, output: Option<&'a str>) -> Result<&'a str, LumenError> {
    match output {
        None => Ok(input),
        Some(o) if o.ends_with(".lumen.json") => Ok(o),
        Some(_) => Err(LumenError::BadParam("output must end in .lumen.json".to_string())),
    }
}

fn save_to(root: &Path, target: &str, doc: &SpriteDoc) -> Result<DocSaved, LumenError> {
    let path = save_doc(root, doc, target)?;
    Ok(DocSaved::of(&path, doc))
}
