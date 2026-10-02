//! Inspection and QA tool tests: validation (honest input does the job) and
//! adversarial (hostile or malformed input fails closed, never panics).
//!
//! Count: 35 tests — 16 validation, 19 adversarial (per-tool split in the
//! section headers).

use std::path::Path;

use image::{ImageBuffer, Rgba};
use lumen::LumenError;
use lumen::doc::{
    BlendMode, Frame, Layer, SpriteDoc, Tag, identity_mod, load_doc, new_doc, save_doc,
};
use lumen::inspect::{
    BBox, ColorUsageRequest, HistogramRequest, InspectLayerRequest, InspectSpriteRequest,
    color_usage, histogram, inspect_layer, inspect_sprite,
};
use lumen::qa::{
    AuditAnimationRequest, CompareFramesRequest, RunLuaScriptRequest, ValidateSceneRequest,
    audit_animation, compare_frames, run_lua_script, validate_scene,
};

const RED: [u8; 4] = [255, 0, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const CLEAR: [u8; 4] = [0, 0, 0, 0];

/// 8x8 doc: "Background" with a red 2x3 block at (2,1), plus a fully blue
/// "Top" layer at 50% opacity. Two identical frames.
fn sample_doc() -> SpriteDoc {
    let mut doc = new_doc(8, 8).expect("new doc");
    for y in 1..4 {
        for x in 2..4 {
            doc.layers[0].image.put_pixel(x, y, Rgba(RED));
        }
    }
    add_layer(&mut doc, "Top", BLUE);
    doc.layers[1].opacity = 0.5;
    let second = doc.frames[0].clone();
    doc.frames.push(second);
    doc
}

fn add_layer(doc: &mut SpriteDoc, name: &str, fill: [u8; 4]) {
    doc.layers.push(Layer {
        name: name.to_string(),
        visible: true,
        opacity: 1.0,
        blend: BlendMode::Normal,
        image: ImageBuffer::from_pixel(doc.width, doc.height, Rgba(fill)),
    });
    for frame in &mut doc.frames {
        frame.layer_mods.push(identity_mod());
    }
}

/// Save a valid doc, then hand-edit its stored JSON.
fn write_corrupted(root: &Path, name: &str, edit: impl FnOnce(&mut serde_json::Value)) {
    save_doc(root, &sample_doc(), name).expect("save");
    let path = root.join(name);
    let mut json: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).expect("read")).expect("json");
    edit(&mut json);
    std::fs::write(&path, serde_json::to_vec(&json).expect("encode")).expect("write");
}

fn assert_bad_param<T: std::fmt::Debug>(r: Result<T, LumenError>, needle: &str) {
    match r {
        Err(LumenError::BadParam(msg)) => assert!(msg.contains(needle), "message: {msg}"),
        other => panic!("expected BadParam containing {needle:?}, got {other:?}"),
    }
}

fn assert_doc_invalid<T: std::fmt::Debug>(r: Result<T, LumenError>, needle: &str) {
    match r {
        Err(LumenError::DocInvalid(msg)) => assert!(msg.contains(needle), "message: {msg}"),
        other => panic!("expected DocInvalid containing {needle:?}, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// inspect_sprite — 1 validation, 2 adversarial
// ---------------------------------------------------------------------------

#[tokio::test]
async fn inspect_sprite_reports_layers_tags_frames() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut doc = sample_doc();
    doc.tags.push(Tag { name: "idle".into(), from_frame: 0, to_frame: 1 });
    doc.palette = vec![RED, BLUE];
    save_doc(dir.path(), &doc, "s.lumen.json").expect("save");
    let req = InspectSpriteRequest { doc: "s.lumen.json".into() };
    let r = inspect_sprite(dir.path(), req).await.expect("inspect");
    assert_eq!((r.width, r.height, r.frames, r.palette_entries), (8, 8, 2, 2));
    assert!(r.path.ends_with("s.lumen.json"));
    assert_eq!(r.layers.len(), 2);
    assert_eq!(r.layers[0].nonzero_pixels, 6);
    assert_eq!(r.layers[1].nonzero_pixels, 64);
    assert_eq!(r.layers[1].blend, "normal");
    assert_eq!(r.layers[1].opacity, 0.5);
    assert_eq!(r.tags[0].frame_count, 2);
}

#[tokio::test]
async fn inspect_sprite_rejects_missing_escaping_and_wrong_extension() {
    let dir = tempfile::tempdir().expect("tempdir");
    for bad in ["missing.lumen.json", "../x.lumen.json", "x.json", "/tmp/x.lumen.json"] {
        let r = inspect_sprite(dir.path(), InspectSpriteRequest { doc: bad.into() }).await;
        assert!(matches!(r, Err(LumenError::PathRejected(_))), "{bad}: {r:?}");
    }
}

#[tokio::test]
async fn inspect_sprite_corrupted_doc_is_doc_invalid() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_corrupted(dir.path(), "c.lumen.json", |j| {
        j["layers"][0]["png_base64"] = "!!!not base64!!!".into();
    });
    let r = inspect_sprite(dir.path(), InspectSpriteRequest { doc: "c.lumen.json".into() }).await;
    assert_doc_invalid(r, "base64");
}

// ---------------------------------------------------------------------------
// inspect_layer — 2 validation, 1 adversarial
// ---------------------------------------------------------------------------

#[tokio::test]
async fn inspect_layer_bbox_and_unique_colors() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut doc = sample_doc();
    doc.layers[0].image.put_pixel(5, 6, Rgba([1, 2, 3, 4]));
    save_doc(dir.path(), &doc, "s.lumen.json").expect("save");
    let req = InspectLayerRequest { doc: "s.lumen.json".into(), layer: 0 };
    let r = inspect_layer(dir.path(), req).await.expect("inspect");
    assert_eq!(r.name, "Background");
    assert_eq!(r.unique_colors, 2);
    assert_eq!(r.nonzero_pixels, 7);
    assert_eq!(r.bbox, Some(BBox { x: 2, y: 1, w: 4, h: 6 }));
}

#[tokio::test]
async fn inspect_layer_fully_transparent_has_no_bbox() {
    let dir = tempfile::tempdir().expect("tempdir");
    save_doc(dir.path(), &new_doc(3, 3).expect("new"), "e.lumen.json").expect("save");
    let req = InspectLayerRequest { doc: "e.lumen.json".into(), layer: 0 };
    let r = inspect_layer(dir.path(), req).await.expect("inspect");
    assert_eq!((r.unique_colors, r.nonzero_pixels, r.bbox), (0, 0, None));
}

#[tokio::test]
async fn inspect_layer_out_of_range_is_bad_param() {
    let dir = tempfile::tempdir().expect("tempdir");
    save_doc(dir.path(), &sample_doc(), "s.lumen.json").expect("save");
    for layer in [2, usize::MAX] {
        let req = InspectLayerRequest { doc: "s.lumen.json".into(), layer };
        assert_bad_param(inspect_layer(dir.path(), req).await, "out of range");
    }
}

// ---------------------------------------------------------------------------
// histogram — 2 validation, 2 adversarial
// ---------------------------------------------------------------------------

#[tokio::test]
async fn histogram_of_layer_sorted_and_truncated() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut doc = sample_doc();
    doc.layers[0].image.put_pixel(7, 7, Rgba(BLUE));
    save_doc(dir.path(), &doc, "s.lumen.json").expect("save");
    let req = HistogramRequest { doc: "s.lumen.json".into(), layer: Some(0), max_entries: 10 };
    let h = histogram(dir.path(), req).await.expect("histogram");
    let pairs: Vec<(&str, u64)> = h.entries.iter().map(|e| (e.color.as_str(), e.count)).collect();
    assert_eq!(pairs, vec![("#FF0000FF", 6), ("#0000FFFF", 1)]);
    let req = HistogramRequest { doc: "s.lumen.json".into(), layer: Some(0), max_entries: 1 };
    assert_eq!(histogram(dir.path(), req).await.expect("histogram").entries.len(), 1);
}

#[tokio::test]
async fn histogram_of_composite_frame_zero() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut doc = new_doc(2, 1).expect("new");
    doc.layers[0].image.put_pixel(0, 0, Rgba(RED));
    save_doc(dir.path(), &doc, "s.lumen.json").expect("save");
    let req = HistogramRequest { doc: "s.lumen.json".into(), layer: None, max_entries: 4 };
    let h = histogram(dir.path(), req).await.expect("histogram");
    assert_eq!(h.entries.len(), 1);
    assert_eq!((h.entries[0].color.as_str(), h.entries[0].count), ("#FF0000FF", 1));
}

#[tokio::test]
async fn histogram_fully_transparent_layer_is_empty_not_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    save_doc(dir.path(), &new_doc(4, 4).expect("new"), "e.lumen.json").expect("save");
    for layer in [Some(0), None] {
        let req = HistogramRequest { doc: "e.lumen.json".into(), layer, max_entries: 4 };
        assert!(histogram(dir.path(), req).await.expect("histogram").entries.is_empty());
    }
}

#[tokio::test]
async fn histogram_rejects_bad_max_entries_and_layer() {
    let dir = tempfile::tempdir().expect("tempdir");
    save_doc(dir.path(), &sample_doc(), "s.lumen.json").expect("save");
    for max_entries in [0, 4097] {
        let req = HistogramRequest { doc: "s.lumen.json".into(), layer: None, max_entries };
        assert_bad_param(histogram(dir.path(), req).await, "max_entries");
    }
    let req = HistogramRequest { doc: "s.lumen.json".into(), layer: Some(5), max_entries: 4 };
    assert_bad_param(histogram(dir.path(), req).await, "out of range");
}

// ---------------------------------------------------------------------------
// color_usage — 2 validation, 1 adversarial
// ---------------------------------------------------------------------------

#[tokio::test]
async fn color_usage_lists_only_layers_with_hits() {
    let dir = tempfile::tempdir().expect("tempdir");
    save_doc(dir.path(), &sample_doc(), "s.lumen.json").expect("save");
    let req = ColorUsageRequest { doc: "s.lumen.json".into(), color: "#0000ff".into() };
    let u = color_usage(dir.path(), req).await.expect("usage");
    assert_eq!(u.color, "#0000FFFF");
    assert_eq!(u.hits.len(), 1);
    assert_eq!((u.hits[0].layer, u.hits[0].name.as_str(), u.hits[0].count), (1, "Top", 64));
    // Exact RGBA match: half-transparent blue matches nothing.
    let req = ColorUsageRequest { doc: "s.lumen.json".into(), color: "#0000FF80".into() };
    assert!(color_usage(dir.path(), req).await.expect("usage").hits.is_empty());
}

#[tokio::test]
async fn color_usage_counts_hits_across_several_layers() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut doc = sample_doc();
    add_layer(&mut doc, "Red", RED);
    save_doc(dir.path(), &doc, "s.lumen.json").expect("save");
    let req = ColorUsageRequest { doc: "s.lumen.json".into(), color: "#FF0000FF".into() };
    let u = color_usage(dir.path(), req).await.expect("usage");
    let hits: Vec<(usize, u64)> = u.hits.iter().map(|h| (h.layer, h.count)).collect();
    assert_eq!(hits, vec![(0, 6), (2, 64)]);
}

#[tokio::test]
async fn color_usage_rejects_malformed_hex() {
    let dir = tempfile::tempdir().expect("tempdir");
    save_doc(dir.path(), &sample_doc(), "s.lumen.json").expect("save");
    let long = format!("#{}", "F".repeat(10_000));
    for bad in ["red", "#FFF", "#FF00FF0", "#ZZ0000", "#é0000", "", long.as_str()] {
        let req = ColorUsageRequest { doc: "s.lumen.json".into(), color: bad.into() };
        let r = color_usage(dir.path(), req).await;
        assert_bad_param(r, "not #RRGGBB");
    }
}

// ---------------------------------------------------------------------------
// validate_scene — 2 validation, 3 adversarial
// ---------------------------------------------------------------------------

#[tokio::test]
async fn validate_scene_valid_doc_with_warnings() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut doc = sample_doc();
    doc.layers[1].visible = false;
    doc.tags.push(Tag { name: "walk".into(), from_frame: 0, to_frame: 0 });
    doc.tags.push(Tag { name: "walk".into(), from_frame: 1, to_frame: 1 });
    save_doc(dir.path(), &doc, "s.lumen.json").expect("save");
    let r = validate_scene(dir.path(), ValidateSceneRequest { doc: "s.lumen.json".into() })
        .await
        .expect("validate");
    assert!(r.valid, "{:?}", r.errors);
    assert!(r.errors.is_empty());
    assert_eq!(r.warnings.len(), 2, "{:?}", r.warnings);
    assert!(r.warnings.iter().any(|w| w.contains("invisible")));
    assert!(r.warnings.iter().any(|w| w.contains("duplicate tag")));
}

#[tokio::test]
async fn validate_scene_fresh_doc_is_clean() {
    let dir = tempfile::tempdir().expect("tempdir");
    save_doc(dir.path(), &new_doc(16, 16).expect("new"), "n.lumen.json").expect("save");
    let r = validate_scene(dir.path(), ValidateSceneRequest { doc: "n.lumen.json".into() })
        .await
        .expect("validate");
    assert!(r.valid && r.errors.is_empty() && r.warnings.is_empty(), "{r:?}");
}

#[tokio::test]
async fn validate_scene_reports_every_structural_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_corrupted(dir.path(), "c.lumen.json", |j| {
        // Frame 0 loses a layer mod; frame 1 gets hostile mod values.
        j["frames"][0]["layer_mods"].as_array_mut().expect("mods").pop();
        j["frames"][1]["layer_mods"][0]["offset_x"] = i32::MIN.into();
        j["frames"][1]["layer_mods"][0]["scale"] = 100.0.into();
        j["frames"][1]["layer_mods"][1]["opacity_mult"] = 1.5.into();
        j["layers"][0]["name"] = "".into();
        j["width"] = 0.into();
        j["tags"] = serde_json::json!([{ "name": "x", "from_frame": 3, "to_frame": 1 }]);
    });
    let r = validate_scene(dir.path(), ValidateSceneRequest { doc: "c.lumen.json".into() })
        .await
        .expect("a report, not an error");
    assert!(!r.valid);
    for needle in ["1 layer mods for 2 layers", "offset", "scale", "opacity_mult", "empty name"] {
        assert!(r.errors.iter().any(|e| e.contains(needle)), "{needle}: {:?}", r.errors);
    }
    assert!(r.errors.iter().any(|e| e.contains("dimensions")), "{:?}", r.errors);
    assert!(r.errors.iter().any(|e| e.contains("tag 0: range 3..=1")), "{:?}", r.errors);
    assert_eq!(r.errors.len(), 7, "{:?}", r.errors);
}

#[tokio::test]
async fn validate_scene_garbage_json_and_bad_payload_are_reported() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("g.lumen.json"), b"{ not json").expect("write");
    let r = validate_scene(dir.path(), ValidateSceneRequest { doc: "g.lumen.json".into() })
        .await
        .expect("report");
    assert!(!r.valid && r.errors[0].contains("json parse failed"), "{:?}", r.errors);
    // Metadata is fine; only the PNG payload is broken: the full load catches it.
    write_corrupted(dir.path(), "p.lumen.json", |j| {
        j["layers"][1]["png_base64"] = "AAAA".into();
    });
    let r = validate_scene(dir.path(), ValidateSceneRequest { doc: "p.lumen.json".into() })
        .await
        .expect("report");
    assert!(!r.valid && r.errors[0].contains("full load failed"), "{:?}", r.errors);
}

#[tokio::test]
async fn validate_scene_bounds_report_size_on_hostile_tag_flood() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_corrupted(dir.path(), "f.lumen.json", |j| {
        let tag = serde_json::json!({ "name": "", "from_frame": 9, "to_frame": 0 });
        j["tags"] = serde_json::Value::Array(vec![tag; 5_000]);
    });
    let r = validate_scene(dir.path(), ValidateSceneRequest { doc: "f.lumen.json".into() })
        .await
        .expect("report");
    assert!(!r.valid);
    assert_eq!(r.errors.len(), 257, "256 errors + one suppression note");
    assert!(r.errors[256].contains("further errors suppressed"));
}

// ---------------------------------------------------------------------------
// audit_animation — 2 validation, 2 adversarial
// ---------------------------------------------------------------------------

fn three_frame_doc() -> SpriteDoc {
    let mut doc = new_doc(4, 4).expect("new");
    doc.layers[0].image.put_pixel(0, 0, Rgba(RED));
    for offset_x in [1, 2] {
        let mut mods = vec![identity_mod()];
        mods[0].offset_x = offset_x;
        doc.frames.push(Frame { duration_ms: 100, layer_mods: mods });
    }
    doc
}

#[tokio::test]
async fn audit_clean_animation_has_no_issues() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut doc = three_frame_doc();
    doc.tags.push(Tag { name: "a".into(), from_frame: 0, to_frame: 1 });
    doc.tags.push(Tag { name: "b".into(), from_frame: 2, to_frame: 2 });
    save_doc(dir.path(), &doc, "s.lumen.json").expect("save");
    let r = audit_animation(dir.path(), AuditAnimationRequest { doc: "s.lumen.json".into() })
        .await
        .expect("audit");
    assert!(r.issues.is_empty(), "{:?}", r.issues);
}

#[tokio::test]
async fn audit_flags_dead_frame_and_long_duration() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut doc = three_frame_doc();
    doc.frames[2].layer_mods[0].offset_x = 1;
    doc.frames[2].duration_ms = 10_001;
    save_doc(dir.path(), &doc, "s.lumen.json").expect("save");
    let r = audit_animation(dir.path(), AuditAnimationRequest { doc: "s.lumen.json".into() })
        .await
        .expect("audit");
    let summary: Vec<(&str, Option<usize>)> =
        r.issues.iter().map(|i| (i.severity.as_str(), i.frame)).collect();
    assert_eq!(summary, vec![("warning", Some(2)), ("warning", Some(2))], "{:?}", r.issues);
    assert!(r.issues.iter().any(|i| i.message.contains("dead frame")));
    assert!(r.issues.iter().any(|i| i.message.contains("10001")));
}

#[tokio::test]
async fn audit_zero_ms_frames_are_errors() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut doc = three_frame_doc();
    doc.frames[0].duration_ms = 0;
    doc.frames[1].duration_ms = 0;
    save_doc(dir.path(), &doc, "s.lumen.json").expect("save");
    let r = audit_animation(dir.path(), AuditAnimationRequest { doc: "s.lumen.json".into() })
        .await
        .expect("audit");
    let errors: Vec<Option<usize>> =
        r.issues.iter().filter(|i| i.severity == "error").map(|i| i.frame).collect();
    assert_eq!(errors, vec![Some(0), Some(1)]);
}

#[tokio::test]
async fn audit_flags_overlapping_tags_and_layer_hidden_everywhere() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut doc = three_frame_doc();
    add_layer(&mut doc, "Ghost", BLUE);
    for frame in &mut doc.frames {
        frame.layer_mods[1].opacity_mult = 0.0;
    }
    doc.tags.push(Tag { name: "outer".into(), from_frame: 0, to_frame: 2 });
    doc.tags.push(Tag { name: "inner".into(), from_frame: 1, to_frame: 1 });
    save_doc(dir.path(), &doc, "s.lumen.json").expect("save");
    let r = audit_animation(dir.path(), AuditAnimationRequest { doc: "s.lumen.json".into() })
        .await
        .expect("audit");
    let messages: Vec<&str> = r.issues.iter().map(|i| i.message.as_str()).collect();
    assert!(messages.contains(&"tag inner overlaps tag outer"), "{messages:?}");
    assert!(messages.iter().any(|m| m.contains("Ghost") && m.contains("every frame")));
    assert!(!messages.iter().any(|m| m.contains("dead frame")), "{messages:?}");
}

// ---------------------------------------------------------------------------
// compare_frames — 2 validation, 2 adversarial
// ---------------------------------------------------------------------------

#[tokio::test]
async fn compare_frames_reports_diff_and_bbox() {
    let dir = tempfile::tempdir().expect("tempdir");
    save_doc(dir.path(), &three_frame_doc(), "s.lumen.json").expect("save");
    let req = CompareFramesRequest { doc: "s.lumen.json".into(), a: 0, b: 2 };
    let d = compare_frames(dir.path(), req).await.expect("compare");
    // Pixel moves from (0,0) to (2,0): two pixels differ.
    assert!(!d.identical);
    assert_eq!((d.differing_pixels, d.total_pixels), (2, 16));
    assert_eq!(d.diff_ratio, 0.125);
    assert_eq!(d.bbox, Some(lumen::qa::BBox { x: 0, y: 0, w: 3, h: 1 }));
}

#[tokio::test]
async fn compare_frames_full_frame_change_has_ratio_one() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut doc = sample_doc();
    add_layer(&mut doc, "Flash", [255, 255, 255, 255]);
    doc.frames[1].layer_mods[2].opacity_mult = 0.0;
    save_doc(dir.path(), &doc, "s.lumen.json").expect("save");
    let req = CompareFramesRequest { doc: "s.lumen.json".into(), a: 0, b: 1 };
    let d = compare_frames(dir.path(), req).await.expect("compare");
    assert_eq!((d.differing_pixels, d.total_pixels, d.diff_ratio), (64, 64, 1.0));
    assert_eq!(d.bbox, Some(lumen::qa::BBox { x: 0, y: 0, w: 8, h: 8 }));
}

#[tokio::test]
async fn compare_frames_same_index_twice_is_identical() {
    let dir = tempfile::tempdir().expect("tempdir");
    save_doc(dir.path(), &three_frame_doc(), "s.lumen.json").expect("save");
    let req = CompareFramesRequest { doc: "s.lumen.json".into(), a: 1, b: 1 };
    let d = compare_frames(dir.path(), req).await.expect("compare");
    assert!(d.identical);
    assert_eq!((d.differing_pixels, d.diff_ratio, d.bbox), (0, 0.0, None));
}

#[tokio::test]
async fn compare_frames_out_of_range_is_bad_param() {
    let dir = tempfile::tempdir().expect("tempdir");
    save_doc(dir.path(), &three_frame_doc(), "s.lumen.json").expect("save");
    for (a, b) in [(0, 3), (usize::MAX, 0)] {
        let req = CompareFramesRequest { doc: "s.lumen.json".into(), a, b };
        assert_bad_param(compare_frames(dir.path(), req).await, "out of range");
    }
}

// ---------------------------------------------------------------------------
// run_lua_script — 3 validation, 6 adversarial
// ---------------------------------------------------------------------------

fn lua_req(script: &str, opt_in: bool) -> RunLuaScriptRequest {
    RunLuaScriptRequest {
        doc: "s.lumen.json".into(),
        output: Some("out.lumen.json".into()),
        script: script.to_string(),
        opt_in,
    }
}

#[tokio::test]
async fn lua_script_edits_pixels_and_saves_to_output() {
    let dir = tempfile::tempdir().expect("tempdir");
    save_doc(dir.path(), &sample_doc(), "s.lumen.json").expect("save");
    let script = r#"
        assert(sprite.width() == 8 and sprite.height() == 8 and sprite.layers() == 2)
        local p = sprite.get(0, 2, 1)
        assert(p.r == 255 and p.g == 0 and p.b == 0 and p.a == 255)
        sprite.fill(1, 0, 0, 0, 0)
        for x = 0, 6 do sprite.set(1, x, 7, 10, 20, 30, 255) end
        sprite.set(1, 14 / 2, 7, 10, 20, 30, 255)  -- integral float accepted
        sprite.set(1, -1, 0, 1, 1, 1, 1)   -- clipped silently
        sprite.set(1, 8, 8, 1, 1, 1, 1)    -- clipped silently
    "#;
    let saved = run_lua_script(dir.path(), lua_req(script, true)).await.expect("lua");
    assert!(saved.path.ends_with("out.lumen.json"));
    let out = load_doc(dir.path(), "out.lumen.json").expect("load");
    let top = &out.layers[1].image;
    assert_eq!(top.get_pixel(0, 0).0, CLEAR);
    assert_eq!(top.get_pixel(5, 7).0, [10, 20, 30, 255]);
    assert_eq!(top.pixels().filter(|p| p[3] != 0).count(), 8);
}

#[tokio::test]
async fn lua_sandbox_exposes_only_documented_globals() {
    let dir = tempfile::tempdir().expect("tempdir");
    save_doc(dir.path(), &sample_doc(), "s.lumen.json").expect("save");
    let script = r#"
        for _, name in ipairs({"io", "os", "debug", "package", "coroutine", "require",
                               "dofile", "loadfile", "load", "print", "warn", "pcall",
                               "xpcall", "setmetatable", "collectgarbage"}) do
            assert(_G[name] == nil, name .. " must not be reachable")
        end
        assert(string.upper("a") == "A" and table.concat({1, 2}) == "12")
        assert(math.floor(1.5) == 1 and utf8.char(72) == "H")
    "#;
    run_lua_script(dir.path(), lua_req(script, true)).await.expect("sandbox shape");
}

#[test]
fn lua_tool_future_is_send() {
    // The MCP runtime may move tool futures across threads; the non-Send Lua
    // state must never be held across an await.
    fn assert_send<T: Send>(_: &T) {}
    let dir = tempfile::tempdir().expect("tempdir");
    let fut = run_lua_script(dir.path(), lua_req("", true));
    assert_send(&fut);
}

#[tokio::test]
async fn lua_refuses_without_opt_in_and_writes_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    save_doc(dir.path(), &sample_doc(), "s.lumen.json").expect("save");
    let r = run_lua_script(dir.path(), lua_req("sprite.fill(0, 1, 1, 1, 1)", false)).await;
    assert_bad_param(r, "refused: run_lua_script requires explicit opt_in=true");
    assert!(!dir.path().join("out.lumen.json").exists());
}

#[tokio::test]
async fn lua_os_execute_and_io_fail_closed_without_saving() {
    let dir = tempfile::tempdir().expect("tempdir");
    save_doc(dir.path(), &sample_doc(), "s.lumen.json").expect("save");
    for script in [
        "os.execute('touch /tmp/pwned-lumen')",
        "io.open('/etc/passwd')",
        "require('os')",
        "sprite.fill(0, 1, 2, 3, 4); error('boom')",
    ] {
        let r = run_lua_script(dir.path(), lua_req(script, true)).await;
        assert_doc_invalid(r, "lua:");
        assert!(!dir.path().join("out.lumen.json").exists(), "{script} must not save");
    }
}

#[tokio::test]
async fn lua_infinite_loops_hit_the_instruction_budget() {
    let dir = tempfile::tempdir().expect("tempdir");
    save_doc(dir.path(), &sample_doc(), "s.lumen.json").expect("save");
    let r = run_lua_script(dir.path(), lua_req("while true do end", true)).await;
    assert_doc_invalid(r, "instruction budget");
    // A loop that only calls into Rust still burns VM instructions.
    let r = run_lua_script(dir.path(), lua_req("while true do sprite.width() end", true)).await;
    assert_doc_invalid(r, "instruction budget");
}

#[tokio::test]
async fn lua_memory_bomb_hits_the_memory_limit() {
    let dir = tempfile::tempdir().expect("tempdir");
    save_doc(dir.path(), &sample_doc(), "s.lumen.json").expect("save");
    let script = "local t = {} \
                  for i = 1, 64 do t[i] = string.rep('x', 4 * 1024 * 1024, tostring(i)) end";
    let r = run_lua_script(dir.path(), lua_req(script, true)).await;
    assert_doc_invalid(r, "memory");
}

#[tokio::test]
async fn lua_rejects_oversized_script_bytecode_and_bad_arguments() {
    let dir = tempfile::tempdir().expect("tempdir");
    save_doc(dir.path(), &sample_doc(), "s.lumen.json").expect("save");
    let big = format!("--{}", "x".repeat(65_535));
    assert_eq!(big.len(), 65_537);
    assert_bad_param(run_lua_script(dir.path(), lua_req(&big, true)).await, "65536");
    // A binary chunk header must be refused by text-only loading.
    for script in [
        "\x1bLua\x54\x00junk",
        "sprite.set(0, 0, 0, 256, 0, 0, 255)",
        "sprite.set(2, 0, 0, 1, 1, 1, 1)",
        "sprite.fill(-1, 0, 0, 0, 0)",
        "sprite.get(0, 8, 0)",
        "sprite.set(0, 0.5, 0, 1, 1, 1, 1)",
        "sprite.fill(0, 255.5, 0, 0, 0)",
        "sprite.get(0, 0/0, 0)",
        "sprite.get(0, '1', 0)",
    ] {
        let r = run_lua_script(dir.path(), lua_req(script, true)).await;
        assert!(matches!(r, Err(LumenError::DocInvalid(_))), "{script:?}: {r:?}");
    }
}

#[tokio::test]
async fn lua_rejects_non_document_output_before_running() {
    let dir = tempfile::tempdir().expect("tempdir");
    save_doc(dir.path(), &sample_doc(), "s.lumen.json").expect("save");
    let mut req = lua_req("sprite.fill(0, 1, 1, 1, 1)", true);
    req.output = Some("../../etc/evil.png".into());
    assert_bad_param(run_lua_script(dir.path(), req).await, ".lumen.json");
    let mut req = lua_req("sprite.fill(0, 1, 1, 1, 1)", true);
    req.output = Some("../escape.lumen.json".into());
    assert!(matches!(run_lua_script(dir.path(), req).await, Err(LumenError::PathRejected(_))));
}
