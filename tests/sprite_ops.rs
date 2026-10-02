//! Milestone 2 Phase A gate tests for `lumen::sprite_ops`.
//!
//! Naming is the classification: `ok_<tool>_*` tests are validation (the
//! tool does its job on honest input); `adv_<tool>_*` tests are adversarial
//! (hostile or malformed input fails closed, or is clipped, without panics
//! and without touching the document on disk). Every project root is a
//! fresh tempdir passed explicitly; no test mutates process-global state.

use std::path::Path;

use lumen::LumenError;
use lumen::doc::{SpriteDoc, load_doc};
use lumen::sprite_ops::{
    self as ops, AddFrameRequest, AddLayerRequest, AddTagRequest, CropRequest,
    DeleteFrameRequest, DeleteLayerRequest, DeleteTagRequest, DrawCircleRequest, DrawLineRequest,
    FillRectRequest, FlipRequest, FloodFillRequest, NewSpriteRequest, RenameLayerRequest,
    ReorderLayerRequest, ResizeCanvasRequest, RotateRequest, SetFrameDurationRequest,
    SetFrameModRequest, SetLayerPropsRequest, SetPixelRequest, TweenFramesRequest,
};

const DOC: &str = "s.lumen.json";
const CLEAR: [u8; 4] = [0, 0, 0, 0];
const RED: [u8; 4] = [255, 0, 0, 255];

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn make(root: &Path, width: u32, height: u32) {
    ops::new_sprite(
        root,
        NewSpriteRequest {
            width,
            height,
            background: "#00000000".to_string(),
            output: DOC.to_string(),
        },
    )
    .await
    .expect("new_sprite");
}

fn load(root: &Path) -> SpriteDoc {
    load_doc(root, DOC).expect("load doc")
}

fn px(doc: &SpriteDoc, layer: usize, x: u32, y: u32) -> [u8; 4] {
    doc.layers[layer].image.get_pixel(x, y).0
}

fn file_bytes(root: &Path) -> Vec<u8> {
    std::fs::read(root.join(DOC)).expect("read doc bytes")
}

#[track_caller]
fn assert_bad<T: std::fmt::Debug>(result: Result<T, LumenError>) {
    match result {
        Err(LumenError::BadParam(_)) => {}
        other => panic!("expected BadParam, got {other:?}"),
    }
}

async fn add_frames(root: &Path, count: usize) {
    for _ in 0..count {
        ops::add_frame(root, frame_req(100, None)).await.expect("add_frame");
    }
}

fn frame_req(duration_ms: u32, at: Option<usize>) -> AddFrameRequest {
    AddFrameRequest { doc: DOC.into(), output: None, duration_ms, at }
}

fn layer_req(name: &str) -> AddLayerRequest {
    AddLayerRequest {
        doc: DOC.into(),
        output: None,
        name: name.into(),
        index: None,
        blend: None,
        opacity: None,
        fill: None,
    }
}

fn tag_req(name: &str, from_frame: u32, to_frame: u32) -> AddTagRequest {
    AddTagRequest { doc: DOC.into(), output: None, name: name.into(), from_frame, to_frame }
}

fn mod_req(frame: usize, layer: usize) -> SetFrameModRequest {
    SetFrameModRequest {
        doc: DOC.into(),
        output: None,
        frame,
        layer,
        offset_x: None,
        offset_y: None,
        opacity_mult: None,
        scale: None,
    }
}

fn pixel_req(x: i32, y: i32, color: &str) -> SetPixelRequest {
    SetPixelRequest { doc: DOC.into(), output: None, layer: 0, x, y, color: color.into() }
}

fn tween_req(from: usize, to: usize, steps: u32, easing: &str) -> TweenFramesRequest {
    TweenFramesRequest {
        doc: DOC.into(),
        output: None,
        from_frame: from,
        to_frame: to,
        steps,
        easing: easing.into(),
    }
}

// ---------------------------------------------------------------------------
// new_sprite
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ok_new_sprite_creates_filled_background_and_100ms_frame() {
    let dir = tempfile::tempdir().expect("tempdir");
    let saved = ops::new_sprite(
        dir.path(),
        NewSpriteRequest {
            width: 4,
            height: 3,
            background: "#10203040".into(),
            output: "art/hero.lumen.json".into(),
        },
    )
    .await
    .expect("new_sprite");
    assert_eq!((saved.width, saved.height, saved.layers, saved.frames), (4, 3, 1, 1));
    let doc = load_doc(dir.path(), "art/hero.lumen.json").expect("load");
    assert_eq!(doc.layers[0].name, "background");
    assert_eq!(doc.frames[0].duration_ms, 100);
    assert_eq!(px(&doc, 0, 3, 2), [0x10, 0x20, 0x30, 0x40]);
}

#[tokio::test]
async fn adv_new_sprite_rejects_zero_and_oversized_dims() {
    let dir = tempfile::tempdir().expect("tempdir");
    for (width, height) in [(0, 0), (0, 8), (10_000, 10_000), (4097, 1)] {
        let result = ops::new_sprite(
            dir.path(),
            NewSpriteRequest {
                width,
                height,
                background: "#000000".into(),
                output: DOC.into(),
            },
        )
        .await;
        assert_bad(result);
    }
    assert!(!dir.path().join(DOC).exists(), "no file may be written");
}

#[tokio::test]
async fn adv_new_sprite_rejects_malformed_colors() {
    let dir = tempfile::tempdir().expect("tempdir");
    // "#ééé" is 7 bytes: '#' + six bytes of multi-byte UTF-8, which must not
    // panic on a char boundary.
    for color in ["red", "", "#", "#12345", "#1234567", "#GG0000", "123456", "#ééé", "#12 456"] {
        let result = ops::new_sprite(
            dir.path(),
            NewSpriteRequest { width: 2, height: 2, background: color.into(), output: DOC.into() },
        )
        .await;
        assert_bad(result);
    }
}

#[tokio::test]
async fn adv_new_sprite_rejects_bad_output_paths() {
    let dir = tempfile::tempdir().expect("tempdir");
    let req = |output: &str| NewSpriteRequest {
        width: 2,
        height: 2,
        background: "#000000".into(),
        output: output.into(),
    };
    assert_bad(ops::new_sprite(dir.path(), req("plain.json")).await);
    let traversal = ops::new_sprite(dir.path(), req("../escape.lumen.json")).await;
    assert!(matches!(traversal, Err(LumenError::PathRejected(_))), "{traversal:?}");
    let absolute = ops::new_sprite(dir.path(), req("/tmp/abs.lumen.json")).await;
    assert!(matches!(absolute, Err(LumenError::PathRejected(_))), "{absolute:?}");
}

// ---------------------------------------------------------------------------
// output handling (shared by every stateless tool)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ok_output_writes_copy_and_leaves_input_untouched() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 4, 4).await;
    let before = file_bytes(dir.path());
    let mut req = pixel_req(1, 1, "#FF0000");
    req.output = Some("out/copy.lumen.json".into());
    ops::set_pixel(dir.path(), req).await.expect("set_pixel");
    assert_eq!(file_bytes(dir.path()), before);
    let copy = load_doc(dir.path(), "out/copy.lumen.json").expect("load copy");
    assert_eq!(px(&copy, 0, 1, 1), RED);
}

#[tokio::test]
async fn adv_output_suffix_and_corrupt_input_fail_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 4, 4).await;
    let before = file_bytes(dir.path());
    let mut req = pixel_req(1, 1, "#FF0000");
    req.output = Some("evil.png".into());
    assert_bad(ops::set_pixel(dir.path(), req).await);
    assert_eq!(file_bytes(dir.path()), before);
    std::fs::write(dir.path().join("junk.lumen.json"), b"{not json").expect("write junk");
    let mut req = pixel_req(0, 0, "#FF0000");
    req.doc = "junk.lumen.json".into();
    let result = ops::set_pixel(dir.path(), req).await;
    assert!(matches!(result, Err(LumenError::DocInvalid(_))), "{result:?}");
}

// ---------------------------------------------------------------------------
// add_layer
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ok_add_layer_appends_with_props_and_identity_mods() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 3, 3).await;
    add_frames(dir.path(), 1).await;
    let mut req = layer_req("ink");
    req.blend = Some("multiply".into());
    req.opacity = Some(0.5);
    req.fill = Some("#FF0000".into());
    let saved = ops::add_layer(dir.path(), req).await.expect("add_layer");
    assert_eq!(saved.layers, 2);
    let doc = load(dir.path());
    assert_eq!(doc.layers[1].name, "ink");
    assert_eq!(doc.layers[1].opacity, 0.5);
    assert_eq!(px(&doc, 1, 2, 2), RED);
    for frame in &doc.frames {
        assert_eq!(frame.layer_mods.len(), 2);
        assert_eq!(frame.layer_mods[1].scale, 1.0);
    }
}

#[tokio::test]
async fn ok_add_layer_inserts_at_index_and_suffixes_duplicates() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 2, 2).await;
    ops::add_layer(dir.path(), layer_req("fx")).await.expect("first");
    let mut req = layer_req("fx");
    req.index = Some(0);
    ops::add_layer(dir.path(), req).await.expect("second");
    ops::add_layer(dir.path(), layer_req("fx")).await.expect("third");
    let doc = load(dir.path());
    let names: Vec<&str> = doc.layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["fx (2)", "background", "fx", "fx (3)"]);
}

#[tokio::test]
async fn ok_add_layer_truncates_long_name_to_limit() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 2, 2).await;
    let long = "é".repeat(500);
    ops::add_layer(dir.path(), layer_req(&long)).await.expect("first");
    ops::add_layer(dir.path(), layer_req(&long)).await.expect("collision");
    let doc = load(dir.path());
    assert_eq!(doc.layers[1].name.chars().count(), 128);
    assert_eq!(doc.layers[2].name.chars().count(), 128);
    assert!(doc.layers[2].name.ends_with(" (2)"));
}

#[tokio::test]
async fn adv_add_layer_rejects_bad_index_blend_opacity_name() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 2, 2).await;
    let before = file_bytes(dir.path());
    let mut req = layer_req("x");
    req.index = Some(2);
    assert_bad(ops::add_layer(dir.path(), req).await);
    let mut req = layer_req("x");
    req.blend = Some("overlay".into());
    assert_bad(ops::add_layer(dir.path(), req).await);
    for opacity in [2.0, -0.1, f32::NAN, f32::INFINITY] {
        let mut req = layer_req("x");
        req.opacity = Some(opacity);
        assert_bad(ops::add_layer(dir.path(), req).await);
    }
    assert_bad(ops::add_layer(dir.path(), layer_req("")).await);
    assert_bad(ops::add_layer(dir.path(), layer_req("evil\nname")).await);
    assert_eq!(file_bytes(dir.path()), before);
}

#[tokio::test]
async fn adv_add_layer_rejects_beyond_max_layers() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 1, 1).await;
    for i in 1..64 {
        ops::add_layer(dir.path(), layer_req(&format!("l{i}"))).await.expect("under cap");
    }
    assert_eq!(load(dir.path()).layers.len(), 64);
    assert_bad(ops::add_layer(dir.path(), layer_req("one too many")).await);
}

// ---------------------------------------------------------------------------
// delete_layer
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ok_delete_layer_removes_layer_and_its_mods() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 2, 2).await;
    ops::add_layer(dir.path(), layer_req("a")).await.expect("a");
    ops::add_layer(dir.path(), layer_req("b")).await.expect("b");
    let mut m = mod_req(0, 2);
    m.offset_x = Some(7);
    ops::set_frame_mod(dir.path(), m).await.expect("mod");
    let req = DeleteLayerRequest { doc: DOC.into(), output: None, index: 1 };
    ops::delete_layer(dir.path(), req).await.expect("delete");
    let doc = load(dir.path());
    assert_eq!(doc.layers[1].name, "b");
    assert_eq!(doc.frames[0].layer_mods.len(), 2);
    assert_eq!(doc.frames[0].layer_mods[1].offset_x, 7, "b's mod must follow b");
}

#[tokio::test]
async fn adv_delete_layer_rejects_last_layer_and_bad_index() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 2, 2).await;
    let req = |index| DeleteLayerRequest { doc: DOC.into(), output: None, index };
    assert_bad(ops::delete_layer(dir.path(), req(0)).await);
    assert_bad(ops::delete_layer(dir.path(), req(1)).await);
    assert_bad(ops::delete_layer(dir.path(), req(usize::MAX)).await);
    assert_eq!(load(dir.path()).layers.len(), 1);
}

// ---------------------------------------------------------------------------
// reorder_layer
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ok_reorder_layer_moves_layer_with_mods() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 2, 2).await;
    ops::add_layer(dir.path(), layer_req("a")).await.expect("a");
    ops::add_layer(dir.path(), layer_req("b")).await.expect("b");
    let mut m = mod_req(0, 2);
    m.offset_y = Some(-3);
    ops::set_frame_mod(dir.path(), m).await.expect("mod");
    let req = ReorderLayerRequest { doc: DOC.into(), output: None, from: 2, to: 0 };
    ops::reorder_layer(dir.path(), req).await.expect("reorder");
    let doc = load(dir.path());
    let names: Vec<&str> = doc.layers.iter().map(|l| l.name.as_str()).collect();
    assert_eq!(names, ["b", "background", "a"]);
    assert_eq!(doc.frames[0].layer_mods[0].offset_y, -3);
}

#[tokio::test]
async fn ok_reorder_layer_onto_itself_is_noop_success() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 2, 2).await;
    let req = ReorderLayerRequest { doc: DOC.into(), output: None, from: 0, to: 0 };
    ops::reorder_layer(dir.path(), req).await.expect("noop");
    assert_eq!(load(dir.path()).layers[0].name, "background");
}

#[tokio::test]
async fn adv_reorder_layer_rejects_out_of_range() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 2, 2).await;
    let req = |from, to| ReorderLayerRequest { doc: DOC.into(), output: None, from, to };
    assert_bad(ops::reorder_layer(dir.path(), req(0, 1)).await);
    assert_bad(ops::reorder_layer(dir.path(), req(1, 0)).await);
    assert_bad(ops::reorder_layer(dir.path(), req(usize::MAX, usize::MAX)).await);
}

// ---------------------------------------------------------------------------
// rename_layer
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ok_rename_layer_renames_and_self_name_is_not_collision() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 2, 2).await;
    ops::add_layer(dir.path(), layer_req("sky")).await.expect("sky");
    let req = |index, name: &str| RenameLayerRequest {
        doc: DOC.into(),
        output: None,
        index,
        name: name.into(),
    };
    ops::rename_layer(dir.path(), req(0, "ground")).await.expect("rename");
    ops::rename_layer(dir.path(), req(1, "sky")).await.expect("self rename");
    ops::rename_layer(dir.path(), req(1, "ground")).await.expect("collide");
    let doc = load(dir.path());
    assert_eq!(doc.layers[0].name, "ground");
    assert_eq!(doc.layers[1].name, "ground (2)");
}

#[tokio::test]
async fn adv_rename_layer_rejects_bad_index_and_names() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 2, 2).await;
    let req = |index, name: &str| RenameLayerRequest {
        doc: DOC.into(),
        output: None,
        index,
        name: name.into(),
    };
    assert_bad(ops::rename_layer(dir.path(), req(1, "x")).await);
    assert_bad(ops::rename_layer(dir.path(), req(0, "")).await);
    assert_bad(ops::rename_layer(dir.path(), req(0, "tab\there")).await);
    assert_eq!(load(dir.path()).layers[0].name, "background");
}

// ---------------------------------------------------------------------------
// set_layer_props
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ok_set_layer_props_updates_given_fields_only() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 2, 2).await;
    let req = SetLayerPropsRequest {
        doc: DOC.into(),
        output: None,
        index: 0,
        visible: Some(false),
        opacity: Some(0.25),
        blend: Some("screen".into()),
    };
    ops::set_layer_props(dir.path(), req).await.expect("props");
    let doc = load(dir.path());
    assert!(!doc.layers[0].visible);
    assert_eq!(doc.layers[0].opacity, 0.25);
    assert_eq!(doc.layers[0].blend, lumen::doc::BlendMode::Screen);
}

#[tokio::test]
async fn adv_set_layer_props_rejects_empty_nan_and_unknown_blend() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 2, 2).await;
    let req = |opacity, blend: Option<&str>, visible| SetLayerPropsRequest {
        doc: DOC.into(),
        output: None,
        index: 0,
        visible,
        opacity,
        blend: blend.map(String::from),
    };
    assert_bad(ops::set_layer_props(dir.path(), req(None, None, None)).await);
    assert_bad(ops::set_layer_props(dir.path(), req(Some(f32::NAN), None, None)).await);
    assert_bad(ops::set_layer_props(dir.path(), req(Some(2.0), None, None)).await);
    assert_bad(ops::set_layer_props(dir.path(), req(None, Some("Normal"), None)).await);
    let mut bad_index = req(None, None, Some(true));
    bad_index.index = 3;
    assert_bad(ops::set_layer_props(dir.path(), bad_index).await);
}

// ---------------------------------------------------------------------------
// add_frame
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ok_add_frame_inserts_identity_mods_and_shifts_tags() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 2, 2).await;
    ops::add_layer(dir.path(), layer_req("a")).await.expect("layer");
    add_frames(dir.path(), 2).await;
    ops::add_tag(dir.path(), tag_req("walk", 1, 2)).await.expect("tag");
    ops::add_frame(dir.path(), frame_req(250, Some(1))).await.expect("insert");
    let doc = load(dir.path());
    assert_eq!(doc.frames.len(), 4);
    assert_eq!(doc.frames[1].duration_ms, 250);
    assert_eq!(doc.frames[1].layer_mods.len(), 2);
    assert_eq!((doc.tags[0].from_frame, doc.tags[0].to_frame), (2, 3));
    ops::add_frame(dir.path(), frame_req(250, Some(3))).await.expect("inside tag");
    let doc = load(dir.path());
    assert_eq!((doc.tags[0].from_frame, doc.tags[0].to_frame), (2, 4));
}

#[tokio::test]
async fn adv_add_frame_rejects_bad_duration_and_position() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 2, 2).await;
    assert_bad(ops::add_frame(dir.path(), frame_req(0, None)).await);
    assert_bad(ops::add_frame(dir.path(), frame_req(60_001, None)).await);
    assert_bad(ops::add_frame(dir.path(), frame_req(u32::MAX, None)).await);
    assert_bad(ops::add_frame(dir.path(), frame_req(100, Some(2))).await);
    assert_eq!(load(dir.path()).frames.len(), 1);
}

#[tokio::test]
async fn adv_add_frame_rejects_beyond_max_frames() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 1, 1).await;
    add_frames(dir.path(), 1).await;
    for steps in [256, 256, 256, 254] {
        ops::tween_frames(dir.path(), tween_req(0, 1, steps, "linear")).await.expect("bulk");
    }
    assert_eq!(load(dir.path()).frames.len(), 1024);
    assert_bad(ops::add_frame(dir.path(), frame_req(100, None)).await);
}

// ---------------------------------------------------------------------------
// delete_frame
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ok_delete_frame_shifts_and_shrinks_tags() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 2, 2).await;
    add_frames(dir.path(), 4).await;
    ops::add_tag(dir.path(), tag_req("late", 3, 4)).await.expect("late");
    ops::add_tag(dir.path(), tag_req("span", 0, 2)).await.expect("span");
    let req = DeleteFrameRequest { doc: DOC.into(), output: None, index: 1 };
    ops::delete_frame(dir.path(), req).await.expect("delete");
    let doc = load(dir.path());
    assert_eq!(doc.frames.len(), 4);
    assert_eq!((doc.tags[0].from_frame, doc.tags[0].to_frame), (2, 3));
    assert_eq!((doc.tags[1].from_frame, doc.tags[1].to_frame), (0, 1));
}

#[tokio::test]
async fn adv_delete_frame_rejects_last_frame_bad_index_and_sole_tag_frame() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 2, 2).await;
    let req = |index| DeleteFrameRequest { doc: DOC.into(), output: None, index };
    assert_bad(ops::delete_frame(dir.path(), req(0)).await);
    add_frames(dir.path(), 1).await;
    assert_bad(ops::delete_frame(dir.path(), req(2)).await);
    ops::add_tag(dir.path(), tag_req("solo", 1, 1)).await.expect("tag");
    assert_bad(ops::delete_frame(dir.path(), req(1)).await);
    let doc = load(dir.path());
    assert_eq!((doc.frames.len(), doc.tags.len()), (2, 1));
}

// ---------------------------------------------------------------------------
// set_frame_duration
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ok_set_frame_duration_updates_one_frame() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 2, 2).await;
    add_frames(dir.path(), 1).await;
    let req = SetFrameDurationRequest {
        doc: DOC.into(),
        output: None,
        index: 1,
        duration_ms: 60_000,
    };
    ops::set_frame_duration(dir.path(), req).await.expect("duration");
    let doc = load(dir.path());
    assert_eq!((doc.frames[0].duration_ms, doc.frames[1].duration_ms), (100, 60_000));
}

#[tokio::test]
async fn adv_set_frame_duration_rejects_zero_overlong_and_bad_index() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 2, 2).await;
    let req = |index, duration_ms| SetFrameDurationRequest {
        doc: DOC.into(),
        output: None,
        index,
        duration_ms,
    };
    assert_bad(ops::set_frame_duration(dir.path(), req(0, 0)).await);
    assert_bad(ops::set_frame_duration(dir.path(), req(0, 60_001)).await);
    assert_bad(ops::set_frame_duration(dir.path(), req(1, 100)).await);
    assert_eq!(load(dir.path()).frames[0].duration_ms, 100);
}

// ---------------------------------------------------------------------------
// set_frame_mod
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ok_set_frame_mod_sets_given_fields() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 2, 2).await;
    let mut req = mod_req(0, 0);
    req.offset_x = Some(-8192);
    req.offset_y = Some(8192);
    req.opacity_mult = Some(0.0);
    req.scale = Some(0.0625);
    ops::set_frame_mod(dir.path(), req).await.expect("mod");
    let mut req = mod_req(0, 0);
    req.scale = Some(16.0);
    ops::set_frame_mod(dir.path(), req).await.expect("scale only");
    let m = &load(dir.path()).frames[0].layer_mods[0];
    assert_eq!((m.offset_x, m.offset_y, m.opacity_mult, m.scale), (-8192, 8192, 0.0, 16.0));
}

#[tokio::test]
async fn adv_set_frame_mod_rejects_all_none_and_out_of_range_values() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 2, 2).await;
    assert_bad(ops::set_frame_mod(dir.path(), mod_req(0, 0)).await);
    let mut cases = Vec::new();
    for offset in [8193, -8193, i32::MIN, i32::MAX] {
        let mut req = mod_req(0, 0);
        req.offset_x = Some(offset);
        cases.push(req);
    }
    for scale in [0.0, 0.06, 16.5, f32::NAN, f32::NEG_INFINITY] {
        let mut req = mod_req(0, 0);
        req.scale = Some(scale);
        cases.push(req);
    }
    for opacity_mult in [1.5, -0.01, f32::NAN] {
        let mut req = mod_req(0, 0);
        req.opacity_mult = Some(opacity_mult);
        cases.push(req);
    }
    for (frame, layer) in [(1, 0), (0, 1)] {
        let mut req = mod_req(frame, layer);
        req.offset_x = Some(1);
        cases.push(req);
    }
    for req in cases {
        assert_bad(ops::set_frame_mod(dir.path(), req).await);
    }
    assert_eq!(load(dir.path()).frames[0].layer_mods[0].offset_x, 0);
}

// ---------------------------------------------------------------------------
// add_tag / delete_tag
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ok_add_tag_records_inclusive_range() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 2, 2).await;
    add_frames(dir.path(), 2).await;
    ops::add_tag(dir.path(), tag_req("idle", 0, 2)).await.expect("idle");
    ops::add_tag(dir.path(), tag_req("blink", 1, 1)).await.expect("blink");
    let doc = load(dir.path());
    assert_eq!(doc.tags.len(), 2);
    assert_eq!((doc.tags[0].name.as_str(), doc.tags[0].to_frame), ("idle", 2));
}

#[tokio::test]
async fn adv_add_tag_rejects_inverted_overflowing_duplicate_and_long() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 2, 2).await;
    add_frames(dir.path(), 1).await;
    ops::add_tag(dir.path(), tag_req("idle", 0, 0)).await.expect("idle");
    assert_bad(ops::add_tag(dir.path(), tag_req("inv", 1, 0)).await);
    assert_bad(ops::add_tag(dir.path(), tag_req("far", 0, 2)).await);
    assert_bad(ops::add_tag(dir.path(), tag_req("max", 0, u32::MAX)).await);
    assert_bad(ops::add_tag(dir.path(), tag_req("idle", 1, 1)).await);
    assert_bad(ops::add_tag(dir.path(), tag_req(&"t".repeat(129), 0, 0)).await);
    assert_bad(ops::add_tag(dir.path(), tag_req("", 0, 0)).await);
    assert_eq!(load(dir.path()).tags.len(), 1);
}

#[tokio::test]
async fn ok_delete_tag_removes_named_tag() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 2, 2).await;
    ops::add_tag(dir.path(), tag_req("a", 0, 0)).await.expect("a");
    ops::add_tag(dir.path(), tag_req("b", 0, 0)).await.expect("b");
    let req = DeleteTagRequest { doc: DOC.into(), output: None, name: "a".into() };
    ops::delete_tag(dir.path(), req).await.expect("delete");
    let doc = load(dir.path());
    assert_eq!(doc.tags.len(), 1);
    assert_eq!(doc.tags[0].name, "b");
}

#[tokio::test]
async fn adv_delete_tag_rejects_missing_name() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 2, 2).await;
    ops::add_tag(dir.path(), tag_req("Walk", 0, 0)).await.expect("tag");
    for name in ["walk", "", "Walk "] {
        let req = DeleteTagRequest { doc: DOC.into(), output: None, name: name.into() };
        assert_bad(ops::delete_tag(dir.path(), req).await);
    }
    assert_eq!(load(dir.path()).tags.len(), 1);
}

// ---------------------------------------------------------------------------
// set_pixel
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ok_set_pixel_writes_exact_rgba() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 3, 3).await;
    ops::set_pixel(dir.path(), pixel_req(2, 1, "#0a0B0c80")).await.expect("pixel");
    let doc = load(dir.path());
    assert_eq!(px(&doc, 0, 2, 1), [10, 11, 12, 128]);
    assert_eq!(px(&doc, 0, 1, 1), CLEAR);
}

#[tokio::test]
async fn adv_set_pixel_clips_extreme_coords_and_rejects_bad_layer() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 3, 3).await;
    for (x, y) in [(-1, 0), (3, 0), (0, 3), (i32::MIN, i32::MIN), (i32::MAX, i32::MAX)] {
        ops::set_pixel(dir.path(), pixel_req(x, y, "#FF0000")).await.expect("clipped");
    }
    let doc = load(dir.path());
    assert!(doc.layers[0].image.pixels().all(|p| p.0 == CLEAR));
    let mut req = pixel_req(0, 0, "#FF0000");
    req.layer = 1;
    assert_bad(ops::set_pixel(dir.path(), req).await);
}

// ---------------------------------------------------------------------------
// fill_rect
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ok_fill_rect_fills_exact_region() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 5, 5).await;
    let req = FillRectRequest {
        doc: DOC.into(),
        output: None,
        layer: 0,
        x: 1,
        y: 2,
        w: 3,
        h: 2,
        color: "#FF0000".into(),
    };
    ops::fill_rect(dir.path(), req).await.expect("fill");
    let doc = load(dir.path());
    let filled = doc.layers[0].image.pixels().filter(|p| p.0 == RED).count();
    assert_eq!(filled, 6);
    assert_eq!(px(&doc, 0, 1, 2), RED);
    assert_eq!(px(&doc, 0, 3, 3), RED);
    assert_eq!(px(&doc, 0, 4, 3), CLEAR);
}

#[tokio::test]
async fn adv_fill_rect_clips_huge_rect_and_rejects_zero_size() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 4, 4).await;
    let req = |x, y, w, h| FillRectRequest {
        doc: DOC.into(),
        output: None,
        layer: 0,
        x,
        y,
        w,
        h,
        color: "#FF0000".into(),
    };
    ops::fill_rect(dir.path(), req(i32::MIN, i32::MIN, u32::MAX, u32::MAX))
        .await
        .expect("clipped huge rect");
    assert!(load(dir.path()).layers[0].image.pixels().all(|p| p.0 == RED));
    ops::fill_rect(dir.path(), req(i32::MAX, 0, u32::MAX, 1)).await.expect("fully off");
    assert_bad(ops::fill_rect(dir.path(), req(0, 0, 0, 1)).await);
    assert_bad(ops::fill_rect(dir.path(), req(0, 0, 1, 0)).await);
}

// ---------------------------------------------------------------------------
// draw_line
// ---------------------------------------------------------------------------

fn line_req(x0: i32, y0: i32, x1: i32, y1: i32) -> DrawLineRequest {
    DrawLineRequest {
        doc: DOC.into(),
        output: None,
        layer: 0,
        x0,
        y0,
        x1,
        y1,
        color: "#FF0000".into(),
    }
}

#[tokio::test]
async fn ok_draw_line_diagonal_and_reverse_endpoints_inclusive() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 5, 5).await;
    ops::draw_line(dir.path(), line_req(0, 0, 4, 4)).await.expect("diag");
    ops::draw_line(dir.path(), line_req(4, 0, 2, 0)).await.expect("reverse");
    let doc = load(dir.path());
    for i in 0..5 {
        assert_eq!(px(&doc, 0, i, i), RED);
    }
    for x in 2..5 {
        assert_eq!(px(&doc, 0, x, 0), RED);
    }
    assert_eq!(doc.layers[0].image.pixels().filter(|p| p.0 == RED).count(), 8);
}

#[tokio::test]
async fn adv_draw_line_clips_off_canvas_and_rejects_extreme_coords() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 4, 4).await;
    ops::draw_line(dir.path(), line_req(-32768, 1, 32768, 1)).await.expect("clipped");
    let doc = load(dir.path());
    assert_eq!(doc.layers[0].image.pixels().filter(|p| p.0 == RED).count(), 4);
    assert_bad(ops::draw_line(dir.path(), line_req(i32::MIN, 0, i32::MAX, 0)).await);
    assert_bad(ops::draw_line(dir.path(), line_req(0, 0, 0, 32769)).await);
}

// ---------------------------------------------------------------------------
// draw_circle
// ---------------------------------------------------------------------------

fn circle_req(cx: i32, cy: i32, r: u32, filled: bool) -> DrawCircleRequest {
    DrawCircleRequest {
        doc: DOC.into(),
        output: None,
        layer: 0,
        cx,
        cy,
        r,
        color: "#FF0000".into(),
        filled,
    }
}

#[tokio::test]
async fn ok_draw_circle_filled_and_outline_are_symmetric() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 9, 9).await;
    ops::draw_circle(dir.path(), circle_req(4, 4, 3, false)).await.expect("outline");
    let doc = load(dir.path());
    assert_eq!(px(&doc, 0, 4, 4), CLEAR, "outline must be hollow");
    for (x, y) in [(1, 4), (7, 4), (4, 1), (4, 7)] {
        assert_eq!(px(&doc, 0, x, y), RED, "cardinal point ({x},{y})");
    }
    assert_eq!(px(&doc, 0, 0, 0), CLEAR);
    ops::draw_circle(dir.path(), circle_req(4, 4, 0, true)).await.expect("dot");
    assert_eq!(px(&load(dir.path()), 0, 4, 4), RED);
    ops::draw_circle(dir.path(), circle_req(4, 4, 2, true)).await.expect("disk");
    let doc = load(dir.path());
    for (x, y) in [(4, 4), (3, 4), (4, 2), (6, 4)] {
        assert_eq!(px(&doc, 0, x, y), RED);
    }
}

#[tokio::test]
async fn adv_draw_circle_extreme_radius_is_bounded_and_bad_input_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 8, 8).await;
    ops::draw_circle(dir.path(), circle_req(i32::MIN, i32::MIN, u32::MAX, true))
        .await
        .expect("huge filled");
    assert!(load(dir.path()).layers[0].image.pixels().all(|p| p.0 == RED));
    ops::draw_circle(dir.path(), circle_req(i32::MAX, 0, u32::MAX, false))
        .await
        .expect("huge outline");
    let mut req = circle_req(0, 0, 1, true);
    req.layer = 9;
    assert_bad(ops::draw_circle(dir.path(), req).await);
    let mut req = circle_req(0, 0, 1, true);
    req.color = "#FF00".into();
    assert_bad(ops::draw_circle(dir.path(), req).await);
}

// ---------------------------------------------------------------------------
// flood_fill
// ---------------------------------------------------------------------------

fn flood_req(x: i32, y: i32, color: &str, tolerance: u8) -> FloodFillRequest {
    FloodFillRequest {
        doc: DOC.into(),
        output: None,
        layer: 0,
        x,
        y,
        color: color.into(),
        tolerance,
    }
}

#[tokio::test]
async fn ok_flood_fill_stops_at_wall() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 5, 5).await;
    let mut wall = line_req(2, 0, 2, 4);
    wall.color = "#0000FF".into();
    ops::draw_line(dir.path(), wall).await.expect("wall");
    ops::flood_fill(dir.path(), flood_req(0, 0, "#FF0000", 0)).await.expect("fill");
    let doc = load(dir.path());
    assert_eq!(doc.layers[0].image.pixels().filter(|p| p.0 == RED).count(), 10);
    assert_eq!(px(&doc, 0, 3, 0), CLEAR, "right of wall untouched");
    assert_eq!(px(&doc, 0, 2, 2), [0, 0, 255, 255], "wall untouched");
}

#[tokio::test]
async fn ok_flood_fill_tolerance_and_noop_on_matching_seed() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 3, 1).await;
    ops::set_pixel(dir.path(), pixel_req(1, 0, "#00000005")).await.expect("near");
    ops::set_pixel(dir.path(), pixel_req(2, 0, "#00000006")).await.expect("far");
    ops::flood_fill(dir.path(), flood_req(0, 0, "#FF0000", 5)).await.expect("fill");
    let doc = load(dir.path());
    assert_eq!([px(&doc, 0, 0, 0), px(&doc, 0, 1, 0)], [RED, RED]);
    assert_eq!(px(&doc, 0, 2, 0), [0, 0, 0, 6]);
    // Seed within tolerance of the fill color: successful no-op.
    ops::flood_fill(dir.path(), flood_req(2, 0, "#00000008", 2)).await.expect("noop");
    assert_eq!(px(&load(dir.path()), 0, 2, 0), [0, 0, 0, 6]);
}

#[tokio::test]
async fn adv_flood_fill_rejects_off_canvas_seed() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 4, 4).await;
    for (x, y) in [(-1, 0), (0, -1), (4, 0), (0, 4), (i32::MIN, i32::MAX)] {
        assert_bad(ops::flood_fill(dir.path(), flood_req(x, y, "#FF0000", 0)).await);
    }
    let mut req = flood_req(0, 0, "#FF0000", 0);
    req.layer = 1;
    assert_bad(ops::flood_fill(dir.path(), req).await);
}

#[tokio::test]
async fn adv_flood_fill_whole_large_canvas_with_max_tolerance_is_bounded() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 512, 512).await;
    ops::set_pixel(dir.path(), pixel_req(100, 100, "#12345680")).await.expect("speck");
    // Tolerance 255 puts every seed within tolerance of any fill color, so
    // the spec's "seed already matches" rule makes it a no-op.
    ops::flood_fill(dir.path(), flood_req(0, 0, "#FF0000", 255)).await.expect("noop");
    assert_eq!(px(&load(dir.path()), 0, 0, 0), CLEAR);
    // 254 still reaches every pixel (max channel diff from the clear seed is
    // 0x80 for the speck), filling all 262144 pixels through a bounded queue.
    ops::flood_fill(dir.path(), flood_req(0, 0, "#FF0000", 254)).await.expect("fill");
    assert!(load(dir.path()).layers[0].image.pixels().all(|p| p.0 == RED));
}

// ---------------------------------------------------------------------------
// flip
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ok_flip_horizontal_one_layer_and_vertical_all_layers() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 3, 2).await;
    ops::add_layer(dir.path(), layer_req("top")).await.expect("layer");
    ops::set_pixel(dir.path(), pixel_req(0, 0, "#FF0000")).await.expect("px0");
    let mut top = pixel_req(0, 0, "#FF0000");
    top.layer = 1;
    ops::set_pixel(dir.path(), top).await.expect("px1");
    let req = |layer, horizontal| FlipRequest { doc: DOC.into(), output: None, layer, horizontal };
    ops::flip(dir.path(), req(Some(0), true)).await.expect("h");
    let doc = load(dir.path());
    assert_eq!((px(&doc, 0, 2, 0), px(&doc, 1, 0, 0)), (RED, RED));
    ops::flip(dir.path(), req(None, false)).await.expect("v");
    let doc = load(dir.path());
    assert_eq!((px(&doc, 0, 2, 1), px(&doc, 1, 0, 1)), (RED, RED));
}

#[tokio::test]
async fn adv_flip_rejects_out_of_range_layer() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 2, 2).await;
    let before = file_bytes(dir.path());
    for layer in [1, usize::MAX] {
        let req = FlipRequest {
            doc: DOC.into(),
            output: None,
            layer: Some(layer),
            horizontal: true,
        };
        assert_bad(ops::flip(dir.path(), req).await);
    }
    assert_eq!(file_bytes(dir.path()), before);
}

// ---------------------------------------------------------------------------
// rotate
// ---------------------------------------------------------------------------

fn rotate_req(layer: Option<usize>, degrees: u16) -> RotateRequest {
    RotateRequest { doc: DOC.into(), output: None, layer, degrees }
}

#[tokio::test]
async fn ok_rotate_90_swaps_dims_and_maps_pixels_clockwise() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 3, 2).await;
    ops::set_pixel(dir.path(), pixel_req(0, 0, "#FF0000")).await.expect("px");
    let saved = ops::rotate(dir.path(), rotate_req(None, 90)).await.expect("rotate");
    assert_eq!((saved.width, saved.height), (2, 3));
    // Clockwise: top-left goes to top-right.
    assert_eq!(px(&load(dir.path()), 0, 1, 0), RED);
    ops::rotate(dir.path(), rotate_req(None, 270)).await.expect("back");
    let doc = load(dir.path());
    assert_eq!(((doc.width, doc.height), px(&doc, 0, 0, 0)), ((3, 2), RED));
}

#[tokio::test]
async fn ok_rotate_180_single_layer_keeps_dims() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 3, 2).await;
    ops::set_pixel(dir.path(), pixel_req(0, 0, "#FF0000")).await.expect("px");
    ops::rotate(dir.path(), rotate_req(Some(0), 180)).await.expect("rotate");
    let doc = load(dir.path());
    assert_eq!(((doc.width, doc.height), px(&doc, 0, 2, 1)), ((3, 2), RED));
}

#[tokio::test]
async fn adv_rotate_rejects_odd_angles_and_single_layer_on_non_square() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 3, 2).await;
    for degrees in [0, 45, 360, 91, u16::MAX] {
        assert_bad(ops::rotate(dir.path(), rotate_req(None, degrees)).await);
    }
    assert_bad(ops::rotate(dir.path(), rotate_req(Some(0), 90)).await);
    assert_bad(ops::rotate(dir.path(), rotate_req(Some(1), 180)).await);
    let doc = load(dir.path());
    assert_eq!((doc.width, doc.height), (3, 2));
}

// ---------------------------------------------------------------------------
// resize_canvas
// ---------------------------------------------------------------------------

fn resize_req(width: u32, height: u32, anchor: &str) -> ResizeCanvasRequest {
    ResizeCanvasRequest { doc: DOC.into(), output: None, width, height, anchor: anchor.into() }
}

#[tokio::test]
async fn ok_resize_canvas_anchors_place_content() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 2, 2).await;
    ops::set_pixel(dir.path(), pixel_req(0, 0, "#FF0000")).await.expect("px");
    ops::resize_canvas(dir.path(), resize_req(4, 4, "bottom-right")).await.expect("grow");
    let doc = load(dir.path());
    assert_eq!((doc.width, doc.height, doc.layers[0].image.width()), (4, 4, 4));
    assert_eq!((px(&doc, 0, 2, 2), px(&doc, 0, 0, 0)), (RED, CLEAR));
    ops::resize_canvas(dir.path(), resize_req(2, 2, "center")).await.expect("shrink");
    assert_eq!(px(&load(dir.path()), 0, 1, 1), RED);
    ops::resize_canvas(dir.path(), resize_req(3, 1, "top-left")).await.expect("tl");
    let doc = load(dir.path());
    assert_eq!((doc.width, doc.height, px(&doc, 0, 0, 0)), (3, 1, CLEAR));
}

#[tokio::test]
async fn adv_resize_canvas_rejects_unknown_anchor_and_bad_dims() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 2, 2).await;
    for anchor in ["middle", "Center", "top_left", ""] {
        assert_bad(ops::resize_canvas(dir.path(), resize_req(4, 4, anchor)).await);
    }
    for (width, height) in [(0, 4), (4, 0), (5000, 4), (u32::MAX, u32::MAX)] {
        assert_bad(ops::resize_canvas(dir.path(), resize_req(width, height, "center")).await);
    }
    assert_eq!(load(dir.path()).width, 2);
}

// ---------------------------------------------------------------------------
// crop
// ---------------------------------------------------------------------------

fn crop_req(x: u32, y: u32, w: u32, h: u32) -> CropRequest {
    CropRequest { doc: DOC.into(), output: None, x, y, w, h }
}

#[tokio::test]
async fn ok_crop_cuts_every_layer() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 5, 4).await;
    ops::add_layer(dir.path(), layer_req("a")).await.expect("layer");
    ops::set_pixel(dir.path(), pixel_req(3, 2, "#FF0000")).await.expect("px");
    let saved = ops::crop(dir.path(), crop_req(2, 1, 3, 3)).await.expect("crop");
    assert_eq!((saved.width, saved.height), (3, 3));
    let doc = load(dir.path());
    assert_eq!(px(&doc, 0, 1, 1), RED);
    assert_eq!(doc.layers[1].image.dimensions(), (3, 3));
}

#[tokio::test]
async fn adv_crop_rejects_off_canvas_zero_and_overflowing_rects() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 4, 4).await;
    for (x, y, w, h) in [
        (3, 0, 2, 1),
        (0, 3, 1, 2),
        (0, 0, 0, 1),
        (0, 0, 1, 0),
        (4, 0, 1, 1),
        (u32::MAX, 0, 2, 1),
        (1, 1, u32::MAX, u32::MAX),
    ] {
        assert_bad(ops::crop(dir.path(), crop_req(x, y, w, h)).await);
    }
    assert_eq!(load(dir.path()).width, 4);
}

// ---------------------------------------------------------------------------
// tween_frames
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ok_tween_frames_linear_interpolates_and_grows_tags() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 2, 2).await;
    add_frames(dir.path(), 1).await;
    ops::set_frame_duration(
        dir.path(),
        SetFrameDurationRequest { doc: DOC.into(), output: None, index: 0, duration_ms: 70 },
    )
    .await
    .expect("dur");
    let mut m = mod_req(1, 0);
    m.offset_x = Some(30);
    m.opacity_mult = Some(0.0);
    m.scale = Some(4.0);
    ops::set_frame_mod(dir.path(), m).await.expect("mod");
    ops::add_tag(dir.path(), tag_req("move", 0, 1)).await.expect("tag");
    let saved = ops::tween_frames(dir.path(), tween_req(0, 1, 2, "linear")).await.expect("tween");
    assert_eq!(saved.frames, 4);
    let doc = load(dir.path());
    let mid = &doc.frames[1].layer_mods[0];
    assert_eq!((mid.offset_x, doc.frames[1].duration_ms), (10, 70));
    assert!((mid.opacity_mult - 2.0 / 3.0).abs() < 1e-5);
    assert!((mid.scale - 2.0).abs() < 1e-5);
    assert_eq!(doc.frames[2].layer_mods[0].offset_x, 20);
    assert_eq!(doc.frames[3].layer_mods[0].offset_x, 30);
    assert_eq!((doc.tags[0].from_frame, doc.tags[0].to_frame), (0, 3));
}

#[tokio::test]
async fn ok_tween_frames_easing_curves_match_formulas() {
    // One step lands at t = 0.5: ease_in 0.25, ease_out 0.75, smoothstep 0.5.
    for (easing, expected) in [("ease_in", 25), ("ease_out", 75), ("ease_in_out", 50)] {
        let dir = tempfile::tempdir().expect("tempdir");
        make(dir.path(), 2, 2).await;
        add_frames(dir.path(), 1).await;
        let mut m = mod_req(1, 0);
        m.offset_y = Some(100);
        ops::set_frame_mod(dir.path(), m).await.expect("mod");
        ops::tween_frames(dir.path(), tween_req(0, 1, 1, easing)).await.expect("tween");
        assert_eq!(load(dir.path()).frames[1].layer_mods[0].offset_y, expected, "{easing}");
    }
}

#[tokio::test]
async fn adv_tween_frames_rejects_bad_steps_easing_and_frames() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 2, 2).await;
    add_frames(dir.path(), 1).await;
    assert_bad(ops::tween_frames(dir.path(), tween_req(0, 1, 0, "linear")).await);
    assert_bad(ops::tween_frames(dir.path(), tween_req(0, 1, 257, "linear")).await);
    assert_bad(ops::tween_frames(dir.path(), tween_req(0, 1, u32::MAX, "linear")).await);
    assert_bad(ops::tween_frames(dir.path(), tween_req(0, 1, 1, "bounce")).await);
    assert_bad(ops::tween_frames(dir.path(), tween_req(0, 1, 1, "EASE_IN")).await);
    assert_bad(ops::tween_frames(dir.path(), tween_req(1, 1, 1, "linear")).await);
    assert_bad(ops::tween_frames(dir.path(), tween_req(0, 2, 1, "linear")).await);
    assert_bad(ops::tween_frames(dir.path(), tween_req(usize::MAX, 0, 1, "linear")).await);
    assert_eq!(load(dir.path()).frames.len(), 2);
}

#[tokio::test]
async fn adv_tween_frames_rejects_exceeding_max_frames() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 1, 1).await;
    add_frames(dir.path(), 1).await;
    for steps in [256, 256, 256, 250] {
        ops::tween_frames(dir.path(), tween_req(0, 1, steps, "linear")).await.expect("bulk");
    }
    assert_eq!(load(dir.path()).frames.len(), 1020);
    assert_bad(ops::tween_frames(dir.path(), tween_req(0, 1, 5, "linear")).await);
    ops::tween_frames(dir.path(), tween_req(0, 1, 4, "linear")).await.expect("exactly full");
    assert_eq!(load(dir.path()).frames.len(), 1024);
}
