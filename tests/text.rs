//! Milestone 2 Phase A gate tests for `lumen::text`.
//!
//! `ok_<tool>_*` tests are validation; `adv_<tool>_*` tests are adversarial.
//! Each project root is a fresh tempdir passed explicitly.

use std::path::Path;

use lumen::LumenError;
use lumen::doc::{SpriteDoc, load_doc};
use lumen::sprite_ops::{self as ops, AddLayerRequest, NewSpriteRequest};
use lumen::text::{TextMeasureRequest, TextRasterizeRequest, text_measure, text_rasterize};

const DOC: &str = "t.lumen.json";
const CLEAR: [u8; 4] = [0, 0, 0, 0];
const INK: [u8; 4] = [255, 255, 255, 255];

async fn make(root: &Path, width: u32, height: u32) {
    let req = NewSpriteRequest {
        width,
        height,
        background: "#00000000".into(),
        output: DOC.into(),
    };
    ops::new_sprite(root, req).await.expect("new_sprite");
}

fn load(root: &Path) -> SpriteDoc {
    load_doc(root, DOC).expect("load doc")
}

fn raster_req(text: &str, scale: u32, x: i32, y: i32) -> TextRasterizeRequest {
    TextRasterizeRequest {
        doc: DOC.into(),
        output: None,
        layer: None,
        text: text.into(),
        fg: "#FFFFFF".into(),
        scale,
        x,
        y,
    }
}

fn measure(text: &str, scale: u32) -> Result<(u32, u32), LumenError> {
    let size = block_on(text_measure(TextMeasureRequest { text: text.into(), scale }))?;
    Ok((size.width, size.height))
}

/// text_measure does no I/O and never awaits, so a tiny runtime suffices.
fn block_on<F: std::future::Future>(future: F) -> F::Output {
    tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("runtime")
        .block_on(future)
}

fn ink_count(doc: &SpriteDoc, layer: usize) -> usize {
    doc.layers[layer].image.pixels().filter(|p| p.0 == INK).count()
}

#[track_caller]
fn assert_bad<T: std::fmt::Debug>(result: Result<T, LumenError>) {
    match result {
        Err(LumenError::BadParam(_)) => {}
        other => panic!("expected BadParam, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// text_rasterize
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ok_text_rasterize_new_layer_draws_exact_glyph() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 8, 8).await;
    let saved = text_rasterize(dir.path(), raster_req("!", 1, 1, 0)).await.expect("raster");
    assert_eq!(saved.layers, 2);
    let doc = load(dir.path());
    assert_eq!(doc.layers[1].name, "text");
    assert_eq!(doc.frames[0].layer_mods.len(), 2);
    // '!' is column 2 of the glyph, rows 0..=4 and row 6.
    for y in [0, 1, 2, 3, 4, 6] {
        assert_eq!(doc.layers[1].image.get_pixel(3, y).0, INK, "row {y}");
    }
    assert_eq!(doc.layers[1].image.get_pixel(3, 5).0, CLEAR);
    assert_eq!(ink_count(&doc, 1), 6);
    assert_eq!(ink_count(&doc, 0), 0, "background untouched");
}

#[tokio::test]
async fn ok_text_rasterize_scale_newline_and_existing_layer() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 16, 32).await;
    let mut req = raster_req("I\nI", 2, 0, 0);
    req.layer = Some(0);
    text_rasterize(dir.path(), req).await.expect("raster");
    let doc = load(dir.path());
    assert_eq!(doc.layers.len(), 1, "existing layer reused");
    // 'I' has 11 inked cells; scale 2 => 44 px; two lines => 88.
    assert_eq!(ink_count(&doc, 0), 88);
    // Second line starts at y = 8 * 2 = 16; 'I' top bar spans columns 1..=3.
    assert_eq!(doc.layers[0].image.get_pixel(2, 16).0, INK);
    assert_eq!(doc.layers[0].image.get_pixel(2, 15).0, CLEAR);
    // A second call without layer adds a uniquely named layer.
    text_rasterize(dir.path(), raster_req("A", 1, 0, 0)).await.expect("one");
    text_rasterize(dir.path(), raster_req("A", 1, 0, 0)).await.expect("two");
    let doc = load(dir.path());
    assert_eq!((doc.layers[1].name.as_str(), doc.layers[2].name.as_str()), ("text", "text (2)"));
}

#[tokio::test]
async fn ok_text_rasterize_every_printable_char_renders() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 600, 8).await;
    let all: String = (' '..='~').collect();
    text_rasterize(dir.path(), raster_req(&all, 1, 0, 0)).await.expect("raster");
    let doc = load(dir.path());
    // Every glyph except space must ink at least one pixel in its cell.
    for (i, c) in all.chars().enumerate().skip(1) {
        let x0 = i as u32 * 6;
        let inked = (x0..x0 + 5)
            .any(|x| (0..7).any(|y| doc.layers[1].image.get_pixel(x, y).0 == INK));
        assert!(inked, "glyph {c:?} is blank");
    }
}

#[tokio::test]
async fn adv_text_rasterize_rejects_non_ascii_and_control_chars() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 8, 8).await;
    for text in ["hi 🦀", "caf\u{e9}", "tab\there", "cr\r\n", "nul\0", "\u{7f}", ""] {
        assert_bad(text_rasterize(dir.path(), raster_req(text, 1, 0, 0)).await);
    }
    let err = text_rasterize(dir.path(), raster_req("ok 🦀", 1, 0, 0)).await.unwrap_err();
    assert!(err.to_string().contains("U+1F980"), "must name the char: {err}");
    assert_eq!(load(dir.path()).layers.len(), 1, "nothing saved");
}

#[tokio::test]
async fn adv_text_rasterize_rejects_bad_scale_length_layer_and_color() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 8, 8).await;
    assert_bad(text_rasterize(dir.path(), raster_req("A", 0, 0, 0)).await);
    assert_bad(text_rasterize(dir.path(), raster_req("A", 17, 0, 0)).await);
    assert_bad(text_rasterize(dir.path(), raster_req(&"A".repeat(513), 1, 0, 0)).await);
    let mut req = raster_req("A", 1, 0, 0);
    req.layer = Some(1);
    assert_bad(text_rasterize(dir.path(), req).await);
    let mut req = raster_req("A", 1, 0, 0);
    req.fg = "white".into();
    assert_bad(text_rasterize(dir.path(), req).await);
    let mut req = raster_req("A", 1, 0, 0);
    req.output = Some("x.json".into());
    assert_bad(text_rasterize(dir.path(), req).await);
}

#[tokio::test]
async fn adv_text_rasterize_clips_extreme_origins_without_panic() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 8, 8).await;
    let long = "W".repeat(512);
    for (x, y) in [(i32::MAX, i32::MAX), (i32::MIN, i32::MIN), (-3, -3), (i32::MAX - 1, 0)] {
        let mut req = raster_req(&long, 16, x, y);
        req.layer = Some(0);
        text_rasterize(dir.path(), req).await.expect("clipped");
    }
    // Only the (-3, -3) pass can reach the canvas: 'W' at scale 16 shifted
    // up-left by 3 px still covers the top-left corner pixel.
    assert_eq!(load(dir.path()).layers[0].image.get_pixel(0, 0).0, INK);
}

#[tokio::test]
async fn adv_text_rasterize_refuses_past_layer_cap() {
    let dir = tempfile::tempdir().expect("tempdir");
    make(dir.path(), 1, 1).await;
    for i in 1..64 {
        let req = AddLayerRequest {
            doc: DOC.into(),
            output: None,
            name: format!("l{i}"),
            index: None,
            blend: None,
            opacity: None,
            fill: None,
        };
        ops::add_layer(dir.path(), req).await.expect("under cap");
    }
    assert_bad(text_rasterize(dir.path(), raster_req("A", 1, 0, 0)).await);
}

// ---------------------------------------------------------------------------
// text_measure
// ---------------------------------------------------------------------------

#[test]
fn ok_text_measure_single_and_multi_line() {
    assert_eq!(measure("A", 1).expect("A"), (5, 7));
    assert_eq!(measure("AB", 1).expect("AB"), (11, 7));
    assert_eq!(measure("Hello", 2).expect("Hello"), (58, 14));
    assert_eq!(measure("ab\nabcd\n", 1).expect("multi"), (23, 23));
    assert_eq!(measure("\n", 3).expect("newline only"), (0, 45));
}

#[test]
fn ok_text_measure_max_text_at_max_scale() {
    let long = "M".repeat(512);
    assert_eq!(measure(&long, 16).expect("max"), ((512 * 6 - 1) * 16, 7 * 16));
}

#[test]
fn adv_text_measure_rejects_bad_text_and_scale() {
    assert_bad(measure("A", 0));
    assert_bad(measure("A", 17));
    assert_bad(measure("A", u32::MAX));
    assert_bad(measure("", 1));
    assert_bad(measure(&"A".repeat(513), 1));
    assert_bad(measure(&"\n".repeat(513), 1));
    assert_bad(measure("emoji 😀", 1));
    assert_bad(measure("\u{200b}", 1));
}
