//! Worker D gate tests: style presets (`style.rs`) and dream animation
//! (`dream.rs`). Roughly 50/50 validation/adversarial per tool.
//!
//! Validation tests prove the tool does its job on honest input.
//! Adversarial tests prove it fails closed on hostile or malformed input
//! and never leaves a partial side effect. Every test owns a fresh tempdir
//! as the project root; no process-global state is touched.

use std::collections::BTreeSet;
use std::path::Path;

use image::Rgba;
use lumen::LumenError;
use lumen::doc::{
    BlendMode, Frame, Layer, LayerMod, SpriteDoc, Tag, identity_mod, load_doc, new_doc, save_doc,
    sync_frame_mods,
};
use lumen::dream::{
    AMBIENT_LAYER_NAME, DreamActReq, DreamAmbientReq, DreamTextReq, DreamTransitionReq, dream_act,
    dream_ambient, dream_text, dream_transition,
};
use lumen::style::{
    FidelitySetReq, StyleApplyReq, StyleListReq, fidelity_set, style_apply, style_list,
};

const DOC: &str = "s.lumen.json";

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// A document with `layer_count` layers and `frame_count` 100 ms frames.
fn make_doc(width: u32, height: u32, layer_count: usize, frame_count: usize) -> SpriteDoc {
    let mut doc = new_doc(width, height).expect("new doc");
    for i in 1..layer_count {
        let image = doc.layers[0].image.clone();
        doc.layers.push(Layer {
            name: format!("layer{i}"),
            visible: true,
            opacity: 1.0,
            blend: BlendMode::Normal,
            image,
        });
    }
    sync_frame_mods(&mut doc);
    let frame = Frame { duration_ms: 100, layer_mods: doc.frames[0].layer_mods.clone() };
    doc.frames = vec![frame; frame_count];
    doc
}

/// A 64x64 single-layer doc of 4096 distinct opaque colors, so even after
/// 4px block snapping (256 blocks) the 8bit 32-color cap must quantize.
fn gradient_doc() -> SpriteDoc {
    let mut doc = make_doc(64, 64, 1, 1);
    for (x, y, p) in doc.layers[0].image.enumerate_pixels_mut() {
        *p = Rgba([(x * 4) as u8, (y * 4) as u8, ((x + y) * 2) as u8, 255]);
    }
    doc
}

fn save(root: &Path, doc: &SpriteDoc) {
    save_doc(root, doc, DOC).expect("save fixture");
}

fn bytes(root: &Path, name: &str) -> Vec<u8> {
    std::fs::read(root.join(name)).expect("read saved doc")
}

fn assert_bad_param_listing(err: LumenError, names: &[&str]) {
    match err {
        LumenError::BadParam(msg) => {
            for name in names {
                assert!(msg.contains(name), "error {msg:?} does not list {name:?}");
            }
        }
        other => panic!("expected BadParam, got {other:?}"),
    }
}

fn assert_bad_param(result: Result<lumen::doc::DocSaved, LumenError>) {
    match result {
        Err(LumenError::BadParam(_)) => {}
        other => panic!("expected BadParam, got {other:?}"),
    }
}

fn ambient_req(seed: u64) -> DreamAmbientReq {
    DreamAmbientReq {
        doc: DOC.to_string(),
        output: None,
        seed,
        frames: None,
        sparkles: None,
        drift: false,
    }
}

fn transition_req(
    from_frame: usize,
    to_frame: usize,
    kind: &str,
    steps: u32,
) -> DreamTransitionReq {
    DreamTransitionReq {
        doc: DOC.to_string(),
        output: None,
        from_frame,
        to_frame,
        kind: kind.to_string(),
        steps,
    }
}

fn text_req(text_layer: usize, effect: &str, frames: Option<u32>) -> DreamTextReq {
    DreamTextReq {
        doc: DOC.to_string(),
        output: None,
        text_layer,
        seed: 7,
        effect: effect.to_string(),
        frames,
    }
}

// ---------------------------------------------------------------------------
// fidelity_set — 2 validation, 3 adversarial
// ---------------------------------------------------------------------------

#[tokio::test]
async fn fidelity_8bit_caps_colors_and_makes_4px_blocks() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    save(root, &gradient_doc());
    let req = FidelitySetReq {
        doc: DOC.to_string(),
        output: Some("out.lumen.json".to_string()),
        preset: "8bit".to_string(),
    };
    let saved = fidelity_set(root, req).await.expect("8bit applies");
    assert_eq!((saved.width, saved.height, saved.layers), (64, 64, 1));
    let out = load_doc(root, "out.lumen.json").expect("load output");
    let img = &out.layers[0].image;
    let distinct: BTreeSet<[u8; 4]> = img.pixels().map(|p| p.0).collect();
    assert!(distinct.len() <= 32, "{} colors exceed the 8bit cap", distinct.len());
    assert!(distinct.len() >= 16, "median-cut should use most of the budget");
    for (x, y, p) in img.enumerate_pixels() {
        assert_eq!(p, img.get_pixel(x - x % 4, y - y % 4), "pixel ({x},{y}) not blocky");
        assert_eq!(p[3], 255, "alpha must survive");
    }
    // Input untouched when an output path is given.
    let input = load_doc(root, DOC).expect("input");
    assert_eq!(input.layers[0].image, gradient_doc().layers[0].image);
}

#[tokio::test]
async fn fidelity_max_and_studio_are_identity() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let original = gradient_doc();
    save(root, &original);
    for preset in ["max", "studio", "64bit"] {
        let req = FidelitySetReq {
            doc: DOC.to_string(),
            output: None,
            preset: preset.to_string(),
        };
        fidelity_set(root, req).await.expect("identity preset applies");
        let after = load_doc(root, DOC).expect("load");
        assert_eq!(after.layers[0].image, original.layers[0].image, "{preset} changed pixels");
    }
}

#[tokio::test]
async fn fidelity_unknown_preset_lists_all_six() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    save(root, &gradient_doc());
    let req = FidelitySetReq { doc: DOC.to_string(), output: None, preset: "4bit".to_string() };
    let err = fidelity_set(root, req).await.expect_err("unknown preset must fail");
    assert_bad_param_listing(err, &["8bit", "16bit", "32bit", "64bit", "studio", "max"]);
}

#[tokio::test]
async fn fidelity_bad_output_extension_writes_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    save(root, &gradient_doc());
    let before = bytes(root, DOC);
    let req = FidelitySetReq {
        doc: DOC.to_string(),
        output: Some("out.json".to_string()),
        preset: "8bit".to_string(),
    };
    assert_bad_param(fidelity_set(root, req).await);
    assert!(!root.join("out.json").exists());
    assert_eq!(bytes(root, DOC), before, "input must be untouched");
}

#[tokio::test]
async fn fidelity_output_escaping_root_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("proj");
    std::fs::create_dir(&root).expect("mkdir");
    save(&root, &gradient_doc());
    let req = FidelitySetReq {
        doc: DOC.to_string(),
        output: Some("../evil.lumen.json".to_string()),
        preset: "16bit".to_string(),
    };
    let err = fidelity_set(&root, req).await.expect_err("escape must fail");
    assert!(matches!(err, LumenError::PathRejected(_)), "got {err:?}");
    assert!(!dir.path().join("evil.lumen.json").exists());
}

// ---------------------------------------------------------------------------
// style_list — 1 validation, 1 adversarial
// ---------------------------------------------------------------------------

#[tokio::test]
async fn style_list_has_exactly_the_13_presets() {
    let dir = tempfile::tempdir().expect("tempdir");
    let list = style_list(dir.path(), StyleListReq {}).await.expect("list");
    let names: Vec<&str> = list.styles.iter().map(|s| s.name.as_str()).collect();
    let mut expected: Vec<String> = (1..=9).map(|i| format!("ref_{i:02}")).collect();
    expected.extend(["dark_fantasy", "solarpunk", "synthwave", "ukiyo_e"].map(String::from));
    assert_eq!(names, expected);
    for style in &list.styles {
        assert!(!style.description.is_empty());
        if style.name.starts_with("ref_") {
            assert!(style.description.contains("awaiting Matt"), "{}", style.name);
        }
    }
}

#[tokio::test]
async fn style_list_names_all_apply_and_near_misses_do_not() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    save(root, &gradient_doc());
    let list = style_list(root, StyleListReq {}).await.expect("list");
    for style in &list.styles {
        let req = StyleApplyReq {
            doc: DOC.to_string(),
            output: Some("o.lumen.json".to_string()),
            style: style.name.clone(),
            strength: 1.0,
        };
        style_apply(root, req).await.expect("every listed style applies");
    }
    for bad in ["REF_01", "ref_10", "ref_00", " synthwave", "synthwave\0", ""] {
        let req = StyleApplyReq {
            doc: DOC.to_string(),
            output: None,
            style: bad.to_string(),
            strength: 1.0,
        };
        assert_bad_param(style_apply(root, req).await);
    }
}

// ---------------------------------------------------------------------------
// style_apply — 3 validation, 4 adversarial
// ---------------------------------------------------------------------------

#[tokio::test]
async fn style_apply_strength_zero_is_pixel_identical() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let original = gradient_doc();
    save(root, &original);
    let req = StyleApplyReq {
        doc: DOC.to_string(),
        output: None,
        style: "synthwave".to_string(),
        strength: 0.0,
    };
    style_apply(root, req).await.expect("strength 0 succeeds");
    let after = load_doc(root, DOC).expect("load");
    assert_eq!(after.layers[0].image, original.layers[0].image);
}

#[tokio::test]
async fn style_apply_full_strength_changes_color() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let original = gradient_doc();
    save(root, &original);
    let req = StyleApplyReq {
        doc: DOC.to_string(),
        output: None,
        style: "dark_fantasy".to_string(),
        strength: 1.0,
    };
    style_apply(root, req).await.expect("applies");
    let after = load_doc(root, DOC).expect("load");
    assert_ne!(after.layers[0].image, original.layers[0].image);
    // dark_fantasy darkens (light 0.75): mean brightness must drop.
    let sum = |d: &SpriteDoc| -> u64 {
        let channels = |p: &Rgba<u8>| u64::from(p[0]) + u64::from(p[1]) + u64::from(p[2]);
        d.layers[0].image.pixels().map(channels).sum()
    };
    assert!(sum(&after) < sum(&original));
}

#[tokio::test]
async fn style_apply_ukiyo_e_posterizes_gray_to_level() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let mut doc = make_doc(2, 2, 1, 1);
    doc.layers[0].image.put_pixel(0, 0, Rgba([128, 128, 128, 255]));
    save(root, &doc);
    let req = StyleApplyReq {
        doc: DOC.to_string(),
        output: None,
        style: "ukiyo_e".to_string(),
        strength: 1.0,
    };
    style_apply(root, req).await.expect("applies");
    let after = load_doc(root, DOC).expect("load");
    // l = 0.502 * 1.05 = 0.527 → contrast 0.95 → 0.526 → 6 levels → 0.6.
    assert_eq!(after.layers[0].image.get_pixel(0, 0), &Rgba([153, 153, 153, 255]));
}

#[tokio::test]
async fn style_apply_preserves_alpha_and_transparent_pixels() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let mut doc = make_doc(2, 1, 1, 1);
    doc.layers[0].image.put_pixel(0, 0, Rgba([200, 40, 90, 77]));
    doc.layers[0].image.put_pixel(1, 0, Rgba([12, 34, 56, 0]));
    save(root, &doc);
    let req = StyleApplyReq {
        doc: DOC.to_string(),
        output: None,
        style: "synthwave".to_string(),
        strength: 1.0,
    };
    style_apply(root, req).await.expect("applies");
    let after = load_doc(root, DOC).expect("load");
    let img = &after.layers[0].image;
    assert_eq!(img.get_pixel(0, 0)[3], 77, "semi-transparent alpha must survive");
    assert_ne!(img.get_pixel(0, 0), &Rgba([200, 40, 90, 77]), "rgb should be graded");
    assert_eq!(img.get_pixel(1, 0)[3], 0, "transparent pixel stays transparent");
}

#[tokio::test]
async fn style_apply_rejects_out_of_range_and_non_finite_strength() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    save(root, &gradient_doc());
    let before = bytes(root, DOC);
    for strength in [1.5, -0.1, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let req = StyleApplyReq {
            doc: DOC.to_string(),
            output: None,
            style: "synthwave".to_string(),
            strength,
        };
        assert_bad_param(style_apply(root, req).await);
    }
    assert_eq!(bytes(root, DOC), before, "rejected calls must not write");
}

#[tokio::test]
async fn style_apply_unknown_style_lists_all_13() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    save(root, &gradient_doc());
    let req = StyleApplyReq {
        doc: DOC.to_string(),
        output: None,
        style: "vaporwave".to_string(),
        strength: 0.5,
    };
    let err = style_apply(root, req).await.expect_err("unknown style must fail");
    let mut names: Vec<String> = (1..=9).map(|i| format!("ref_{i:02}")).collect();
    names.extend(["dark_fantasy", "solarpunk", "synthwave", "ukiyo_e"].map(String::from));
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    assert_bad_param_listing(err, &refs);
}

#[tokio::test]
async fn style_apply_missing_doc_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let req = StyleApplyReq {
        doc: "nope.lumen.json".to_string(),
        output: None,
        style: "synthwave".to_string(),
        strength: 0.5,
    };
    let err = style_apply(dir.path(), req).await.expect_err("missing doc must fail");
    assert!(matches!(err, LumenError::PathRejected(_)), "got {err:?}");
}

// ---------------------------------------------------------------------------
// dream_ambient — 3 validation, 4 adversarial
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ambient_defaults_add_layer_and_eight_twinkling_frames() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    save(root, &make_doc(32, 32, 1, 2));
    let saved = dream_ambient(root, ambient_req(42)).await.expect("ambient applies");
    assert_eq!((saved.layers, saved.frames), (2, 10));
    let doc = load_doc(root, DOC).expect("load");
    let layer = &doc.layers[1];
    assert_eq!(layer.name, AMBIENT_LAYER_NAME);
    let lit = layer.image.pixels().filter(|p| p[3] != 0).count();
    assert!((1..=64).contains(&lit), "{lit} sparkles");
    assert!(layer.image.pixels().filter(|p| p[3] != 0).all(|p| p.0 == [255, 255, 255, 255]));
    // Pre-existing frames get identity for the new layer.
    assert_eq!(doc.frames[0].layer_mods[1].opacity_mult, 1.0);
    for frame in &doc.frames[2..] {
        assert_eq!(frame.duration_ms, 120);
        let op = frame.layer_mods[1].opacity_mult;
        assert!((0.35..=1.0).contains(&op), "opacity {op}");
        assert_eq!((frame.layer_mods[1].offset_x, frame.layer_mods[1].offset_y), (0, 0));
    }
    let distinct: BTreeSet<u32> =
        doc.frames[2..].iter().map(|f| f.layer_mods[1].opacity_mult.to_bits()).collect();
    assert!(distinct.len() > 1, "opacity must twinkle across frames");
}

#[tokio::test]
async fn ambient_drift_follows_the_formula() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    save(root, &make_doc(16, 16, 1, 1));
    let req = DreamAmbientReq {
        frames: Some(16),
        sparkles: Some(0),
        drift: true,
        ..ambient_req(1)
    };
    dream_ambient(root, req).await.expect("ambient applies");
    let doc = load_doc(root, DOC).expect("load");
    assert_eq!(doc.frames.len(), 17);
    for (k, frame) in doc.frames[1..].iter().enumerate() {
        let kf = k as f32;
        let m = &frame.layer_mods[1];
        assert_eq!(m.offset_x, (3.0 * (kf * 0.7).sin()).round() as i32, "frame {k}");
        assert_eq!(m.offset_y, (2.0 * (kf * 0.5).cos()).round() as i32, "frame {k}");
    }
    assert_eq!(doc.layers[1].image.pixels().filter(|p| p[3] != 0).count(), 0);
}

#[tokio::test]
async fn ambient_is_deterministic_in_seed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    save(root, &make_doc(24, 24, 1, 1));
    let run = |seed: u64, out: &'static str| {
        let req = DreamAmbientReq { output: Some(out.to_string()), ..ambient_req(seed) };
        dream_ambient(root, req)
    };
    run(99, "a.lumen.json").await.expect("first run");
    run(99, "b.lumen.json").await.expect("second run");
    run(100, "c.lumen.json").await.expect("other seed");
    assert_eq!(bytes(root, "a.lumen.json"), bytes(root, "b.lumen.json"), "same seed");
    assert_ne!(bytes(root, "a.lumen.json"), bytes(root, "c.lumen.json"), "different seed");
}

#[tokio::test]
async fn ambient_rejects_out_of_range_frames_and_sparkles() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    save(root, &make_doc(8, 8, 1, 1));
    let before = bytes(root, DOC);
    assert_bad_param(
        dream_ambient(root, DreamAmbientReq { frames: Some(0), ..ambient_req(1) }).await,
    );
    assert_bad_param(
        dream_ambient(root, DreamAmbientReq { frames: Some(65), ..ambient_req(1) }).await,
    );
    assert_bad_param(
        dream_ambient(root, DreamAmbientReq { sparkles: Some(513), ..ambient_req(1) }).await,
    );
    assert_eq!(bytes(root, DOC), before, "rejected calls must not write");
}

#[tokio::test]
async fn ambient_twice_is_rejected_not_duplicated() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    save(root, &make_doc(8, 8, 1, 1));
    dream_ambient(root, ambient_req(1)).await.expect("first run");
    assert_bad_param(dream_ambient(root, ambient_req(2)).await);
    assert_eq!(load_doc(root, DOC).expect("load").layers.len(), 2);
}

#[tokio::test]
async fn ambient_respects_frame_and_layer_limits() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    save(root, &make_doc(4, 4, 1, 1020));
    assert_bad_param(dream_ambient(root, ambient_req(1)).await);
    save(root, &make_doc(4, 4, 64, 1));
    assert_bad_param(dream_ambient(root, ambient_req(1)).await);
}

#[tokio::test]
async fn ambient_survives_1x1_canvas_and_max_sparkles() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    save(root, &make_doc(1, 1, 1, 1));
    let req = DreamAmbientReq {
        sparkles: Some(512),
        frames: Some(64),
        ..ambient_req(u64::MAX)
    };
    let saved = dream_ambient(root, req).await.expect("tiny canvas works");
    assert_eq!(saved.frames, 65);
}

// ---------------------------------------------------------------------------
// dream_act — 2 validation, 3 adversarial
// ---------------------------------------------------------------------------

#[tokio::test]
async fn act_idle_matches_formula_on_all_layers() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    save(root, &make_doc(8, 8, 2, 4));
    let req = DreamActReq {
        doc: DOC.to_string(),
        output: None,
        seed: 0,
        performance: "idle".to_string(),
    };
    dream_act(root, req).await.expect("idle applies");
    let doc = load_doc(root, DOC).expect("load");
    // k = 1 of 4: θ = π/2 → scale 1.02, offset_y 1.
    for m in &doc.frames[1].layer_mods {
        assert!((m.scale - 1.02).abs() < 1e-6, "scale {}", m.scale);
        assert_eq!((m.offset_x, m.offset_y), (0, 1));
    }
    assert_eq!(doc.frames[0].layer_mods[0].scale, 1.0);
}

#[tokio::test]
async fn act_bounce_hops_up_and_overwrites_existing_mods() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let mut doc = make_doc(8, 8, 1, 6);
    for frame in &mut doc.frames {
        frame.layer_mods[0] = LayerMod { offset_x: 5, offset_y: 5, opacity_mult: 0.5, scale: 2.0 };
    }
    save(root, &doc);
    let req = DreamActReq {
        doc: DOC.to_string(),
        output: None,
        seed: 0,
        performance: "bounce".to_string(),
    };
    dream_act(root, req).await.expect("bounce applies");
    let doc = load_doc(root, DOC).expect("load");
    let offsets: Vec<i32> = doc.frames.iter().map(|f| f.layer_mods[0].offset_y).collect();
    assert!(offsets.iter().all(|&y| (-4..=0).contains(&y)), "{offsets:?}");
    assert_eq!(offsets.iter().min(), Some(&-4), "peak hop is 4px");
    for frame in &doc.frames {
        let m = &frame.layer_mods[0];
        assert_eq!((m.offset_x, m.opacity_mult), (0, 1.0), "old mods are overwritten");
    }
}

#[tokio::test]
async fn act_single_frame_doc_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    save(root, &make_doc(8, 8, 1, 1));
    let req = DreamActReq {
        doc: DOC.to_string(),
        output: None,
        seed: 0,
        performance: "idle".to_string(),
    };
    assert_bad_param(dream_act(root, req).await);
}

#[tokio::test]
async fn act_unknown_performance_lists_valid_values() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    save(root, &make_doc(8, 8, 1, 3));
    let req = DreamActReq {
        doc: DOC.to_string(),
        output: None,
        seed: 0,
        performance: "dance".to_string(),
    };
    let err = dream_act(root, req).await.expect_err("unknown performance must fail");
    assert_bad_param_listing(err, &["idle", "breathe", "bounce"]);
}

#[tokio::test]
async fn act_extreme_seed_keeps_mods_valid() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    save(root, &make_doc(8, 8, 1, 1024));
    for performance in ["idle", "breathe", "bounce"] {
        let req = DreamActReq {
            doc: DOC.to_string(),
            output: None,
            seed: u64::MAX,
            performance: performance.to_string(),
        };
        dream_act(root, req).await.expect("max seed + max frames still saves");
    }
}

// ---------------------------------------------------------------------------
// dream_transition — 3 validation, 4 adversarial
// ---------------------------------------------------------------------------

#[tokio::test]
async fn transition_dissolve_inserts_and_crossfades() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    save(root, &make_doc(8, 8, 1, 2));
    let saved = dream_transition(root, transition_req(0, 1, "dissolve", 3)).await.expect("ok");
    assert_eq!(saved.frames, 5);
    let doc = load_doc(root, DOC).expect("load");
    let durations: Vec<u32> = doc.frames.iter().map(|f| f.duration_ms).collect();
    assert_eq!(durations, [100, 90, 90, 90, 100]);
    let ops: Vec<f32> = doc.frames.iter().map(|f| f.layer_mods[0].opacity_mult).collect();
    assert_eq!(ops[2], 0.0, "midpoint fully dissolved");
    assert!(ops[1] > 0.0 && ops[1] < 1.0 && ops[3] > 0.0 && ops[3] < 1.0, "{ops:?}");
}

#[tokio::test]
async fn transition_iris_and_wipe_hit_their_extremes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    save(root, &make_doc(10, 8, 1, 2));
    dream_transition(root, transition_req(0, 1, "iris", 1)).await.expect("iris");
    let doc = load_doc(root, DOC).expect("load");
    assert_eq!(doc.frames[1].layer_mods[0].scale, 0.0625, "iris closes to doc minimum");
    save(root, &make_doc(10, 8, 1, 2));
    dream_transition(root, transition_req(0, 1, "wipe", 3)).await.expect("wipe");
    let doc = load_doc(root, DOC).expect("load");
    let xs: Vec<i32> = doc.frames[1..4].iter().map(|f| f.layer_mods[0].offset_x).collect();
    assert_eq!(xs, [-5, 10, 5], "slides off left, re-enters from right");
}

#[tokio::test]
async fn transition_shifts_tags_after_insertion_point() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let mut doc = make_doc(8, 8, 1, 3);
    doc.tags = vec![
        Tag { name: "start".to_string(), from_frame: 0, to_frame: 0 },
        Tag { name: "end".to_string(), from_frame: 1, to_frame: 2 },
    ];
    save(root, &doc);
    dream_transition(root, transition_req(0, 2, "bloom", 4)).await.expect("bloom");
    let doc = load_doc(root, DOC).expect("load");
    let ranges: Vec<(u32, u32)> = doc.tags.iter().map(|t| (t.from_frame, t.to_frame)).collect();
    assert_eq!(ranges, [(0, 0), (5, 6)]);
    let mid = &doc.frames[2].layer_mods[0];
    assert!(mid.scale > 1.0 && mid.opacity_mult < 1.0, "bloom pulses and dips");
}

#[tokio::test]
async fn transition_same_frame_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    save(root, &make_doc(8, 8, 1, 2));
    assert_bad_param(dream_transition(root, transition_req(1, 1, "dissolve", 2)).await);
}

#[tokio::test]
async fn transition_out_of_range_frames_are_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    save(root, &make_doc(8, 8, 1, 2));
    let before = bytes(root, DOC);
    assert_bad_param(dream_transition(root, transition_req(0, 2, "wipe", 2)).await);
    assert_bad_param(dream_transition(root, transition_req(usize::MAX, 0, "wipe", 2)).await);
    assert_eq!(bytes(root, DOC), before);
}

#[tokio::test]
async fn transition_bad_steps_are_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    save(root, &make_doc(8, 8, 1, 2));
    assert_bad_param(dream_transition(root, transition_req(0, 1, "iris", 0)).await);
    assert_bad_param(dream_transition(root, transition_req(0, 1, "iris", 65)).await);
    save(root, &make_doc(8, 8, 1, 1000));
    assert_bad_param(dream_transition(root, transition_req(0, 1, "iris", 64)).await);
}

#[tokio::test]
async fn transition_unknown_kind_lists_valid_values() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    save(root, &make_doc(8, 8, 1, 2));
    let err = dream_transition(root, transition_req(0, 1, "fade", 2))
        .await
        .expect_err("unknown kind must fail");
    assert_bad_param_listing(err, &["dissolve", "wipe", "iris", "bloom"]);
}

// ---------------------------------------------------------------------------
// dream_text — 3 validation, 3 adversarial
// ---------------------------------------------------------------------------

#[tokio::test]
async fn text_typewriter_eases_in_only_the_text_layer() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let mut doc = make_doc(8, 20, 2, 1);
    doc.frames[0].layer_mods[0] = LayerMod { offset_x: 3, ..identity_mod() };
    save(root, &doc);
    let saved = dream_text(root, text_req(1, "typewriter", None)).await.expect("ok");
    assert_eq!(saved.frames, 13);
    let doc = load_doc(root, DOC).expect("load");
    let first = &doc.frames[1].layer_mods[1];
    let last = &doc.frames[12].layer_mods[1];
    assert_eq!((first.offset_y, first.opacity_mult), (20, 0.0));
    assert_eq!((last.offset_y, last.opacity_mult), (0, 1.0));
    let ys: Vec<i32> = doc.frames[1..].iter().map(|f| f.layer_mods[1].offset_y).collect();
    assert!(ys.windows(2).all(|w| w[1] <= w[0]), "monotone ease {ys:?}");
    for frame in &doc.frames[1..] {
        assert_eq!(frame.duration_ms, 90);
        assert_eq!(frame.layer_mods[0].offset_x, 3, "other layers keep the last pose");
    }
}

#[tokio::test]
async fn text_glitch_is_bounded_and_deterministic() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    save(root, &make_doc(8, 8, 1, 1));
    let run = |out: &'static str| {
        let req = DreamTextReq {
            output: Some(out.to_string()),
            ..text_req(0, "glitch", Some(96))
        };
        dream_text(root, req)
    };
    run("a.lumen.json").await.expect("glitch");
    run("b.lumen.json").await.expect("glitch again");
    assert_eq!(bytes(root, "a.lumen.json"), bytes(root, "b.lumen.json"));
    let doc = load_doc(root, "a.lumen.json").expect("load");
    for frame in &doc.frames[1..] {
        let m = &frame.layer_mods[0];
        assert!((-2..=2).contains(&m.offset_x) && (-2..=2).contains(&m.offset_y));
        assert!(m.opacity_mult == 1.0 || m.opacity_mult == 0.2, "{}", m.opacity_mult);
    }
    let dims = doc.frames[1..].iter().filter(|f| f.layer_mods[0].opacity_mult == 0.2).count();
    assert!(dims > 0 && dims < 96, "flicker happens but not always ({dims})");
}

#[tokio::test]
async fn text_pulse_follows_sine() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    save(root, &make_doc(8, 8, 1, 1));
    dream_text(root, text_req(0, "pulse", Some(4))).await.expect("pulse");
    let doc = load_doc(root, DOC).expect("load");
    let scales: Vec<f32> = doc.frames[1..].iter().map(|f| f.layer_mods[0].scale).collect();
    assert!((scales[1] - 1.08).abs() < 1e-6 && (scales[3] - 0.92).abs() < 1e-6, "{scales:?}");
}

#[tokio::test]
async fn text_bad_layer_index_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    save(root, &make_doc(8, 8, 2, 1));
    assert_bad_param(dream_text(root, text_req(2, "pulse", None)).await);
    assert_bad_param(dream_text(root, text_req(usize::MAX, "pulse", None)).await);
}

#[tokio::test]
async fn text_unknown_effect_lists_valid_values() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    save(root, &make_doc(8, 8, 1, 1));
    let err = dream_text(root, text_req(0, "wobble", None)).await.expect_err("must fail");
    assert_bad_param_listing(err, &["typewriter", "glitch", "pulse"]);
}

#[tokio::test]
async fn text_out_of_range_frames_are_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    save(root, &make_doc(8, 8, 1, 1));
    let before = bytes(root, DOC);
    assert_bad_param(dream_text(root, text_req(0, "pulse", Some(0))).await);
    assert_bad_param(dream_text(root, text_req(0, "pulse", Some(97))).await);
    assert_eq!(bytes(root, DOC), before);
}
