//! Export, pipeline, and Bevy pass-through tests: validation vs adversarial
//! per tool. Validation tests prove the tool does its job on honest input;
//! adversarial tests prove it fails closed (or degrades as documented) on
//! hostile or malformed input. The split is tallied in each section header.
//!
//! No test mutates process-global state: the project root is a tempdir and
//! every BRP mock binds an ephemeral loopback port.

use std::path::Path;

use image::{ImageBuffer, Rgba, RgbaImage};
use lumen::LumenError;
use lumen::bevy::{BevyCallRequest, BevyStatusRequest, bevy_call, bevy_status};
use lumen::doc::{
    BlendMode, Frame, Layer, LayerMod, SpriteDoc, Tag, identity_mod, load_doc, new_doc, save_doc,
};
use lumen::export::{
    ExportSheetRequest, ExportSpriteRequest, ExportTagRequest, ExportTic80Request,
    ExportWasm4Request, ImportLayerRequest, export_sheet, export_sprite, export_tag, export_tic80,
    export_wasm4, import_layer,
};
use lumen::pipeline::{
    BuildFullbodySheetRequest, ContactSheetRequest, GenerateMatureVariantRequest,
    build_fullbody_sheet, contact_sheet, generate_mature_variant,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const RED: Rgba<u8> = Rgba([255, 0, 0, 255]);
const WHITE: Rgba<u8> = Rgba([255, 255, 255, 255]);
const CLEAR: Rgba<u8> = Rgba([0, 0, 0, 0]);

/// A `w`x`h` doc with one red pixel at (0,0) on its only layer. Frame `i`
/// lasts `100 + i` ms and offsets the layer `i` px right, so every frame's
/// render is distinguishable by where the red pixel lands.
fn write_doc(root: &Path, w: u32, h: u32, frames: u32, tags: &[(&str, u32, u32)]) -> SpriteDoc {
    let mut doc = new_doc(w, h).expect("new doc");
    doc.layers[0].image.put_pixel(0, 0, RED);
    doc.frames = (0..frames)
        .map(|i| Frame {
            duration_ms: 100 + i,
            layer_mods: vec![LayerMod {
                offset_x: i as i32,
                ..identity_mod()
            }],
        })
        .collect();
    doc.tags = tags
        .iter()
        .map(|(name, from, to)| Tag {
            name: name.to_string(),
            from_frame: *from,
            to_frame: *to,
        })
        .collect();
    save_doc(root, &doc, "a.lumen.json").expect("save doc");
    doc
}

fn read_png(path: &Path) -> RgbaImage {
    image::open(path).expect("open png").to_rgba8()
}

fn read_json(path: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(path).expect("read json")).expect("parse json")
}

/// Two unsigned fields of a JSON object as a pair (panics if absent).
fn xy(v: &serde_json::Value, a: &str, b: &str) -> (u64, u64) {
    (
        v[a].as_u64().expect("first field"),
        v[b].as_u64().expect("second field"),
    )
}

fn is_bad_param(err: &LumenError) -> bool {
    matches!(err, LumenError::BadParam(_))
}

// ---------------------------------------------------------------------------
// export_sprite — 1 validation, 2 adversarial
// ---------------------------------------------------------------------------

#[tokio::test]
async fn export_sprite_renders_requested_frame() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_doc(root, 4, 2, 3, &[]);
    let req = ExportSpriteRequest {
        doc: "a.lumen.json".into(),
        output: "out/f2.png".into(),
        frame: Some(2),
    };
    let saved = export_sprite(root, req).await.expect("export");
    assert_eq!((saved.width, saved.height), (4, 2));
    let img = read_png(&root.join("out/f2.png"));
    assert_eq!(*img.get_pixel(2, 0), RED, "frame 2 shifts the pixel 2 px");
    assert_eq!(*img.get_pixel(0, 0), CLEAR);
}

#[tokio::test]
async fn export_sprite_rejects_frame_out_of_range() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_doc(root, 4, 2, 3, &[]);
    let req = ExportSpriteRequest {
        doc: "a.lumen.json".into(),
        output: "f.png".into(),
        frame: Some(3),
    };
    let err = export_sprite(root, req).await.unwrap_err();
    assert!(is_bad_param(&err), "{err}");
    assert!(err.to_string().contains("out of range"));
    assert!(!root.join("f.png").exists());
}

#[tokio::test]
async fn export_sprite_rejects_non_png_output_and_traversal() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_doc(root, 4, 2, 1, &[]);
    let wrong_ext = ExportSpriteRequest {
        doc: "a.lumen.json".into(),
        output: "f.json".into(),
        frame: None,
    };
    let err = export_sprite(root, wrong_ext).await.unwrap_err();
    assert!(is_bad_param(&err), "{err}");
    let escape = ExportSpriteRequest {
        doc: "a.lumen.json".into(),
        output: "../escaped.png".into(),
        frame: None,
    };
    let err = export_sprite(root, escape).await.unwrap_err();
    assert!(err.to_string().contains("escapes"), "{err}");
}

// ---------------------------------------------------------------------------
// export_sheet — 2 validation, 3 adversarial
// ---------------------------------------------------------------------------

fn sheet_req(columns: u32, tag: Option<&str>, padding: u32, meta: &str) -> ExportSheetRequest {
    ExportSheetRequest {
        doc: "a.lumen.json".into(),
        output: "sheet.png".into(),
        meta_output: meta.into(),
        columns,
        tag: tag.map(str::to_string),
        padding,
    }
}

#[tokio::test]
async fn export_sheet_lays_out_grid_with_padding_and_sidecar() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_doc(root, 4, 2, 3, &[("idle", 0, 1), ("walk", 1, 2)]);
    let saved = export_sheet(root, sheet_req(2, None, 1, "sheet.json"))
        .await
        .expect("export");
    // 2 columns x 2 rows of 4x2 cells with a 1 px gap.
    assert_eq!((saved.width, saved.height), (9, 5));
    let img = read_png(&root.join("sheet.png"));
    assert_eq!(*img.get_pixel(0, 0), RED, "frame 0 cell at (0,0)");
    assert_eq!(*img.get_pixel(6, 0), RED, "frame 1 cell at (5,0), pixel +1");
    assert_eq!(*img.get_pixel(2, 3), RED, "frame 2 cell at (0,3), pixel +2");
    assert_eq!(*img.get_pixel(4, 0), CLEAR, "gap column is transparent");
    let meta = read_json(&root.join("sheet.json"));
    assert_eq!(meta["image"], "sheet.png");
    assert_eq!(meta["columns"], 2);
    assert_eq!(xy(&meta, "cell_w", "cell_h"), (4, 2));
    let cells = meta["frames"].as_array().expect("frames");
    assert_eq!(cells.len(), 3);
    assert_eq!(xy(&cells[1], "x", "y"), (5, 0));
    assert_eq!(xy(&cells[2], "x", "y"), (0, 3));
    assert_eq!(cells[1]["tag"], "idle", "first covering tag wins");
    assert_eq!(cells[2]["tag"], "walk");
    assert_eq!(cells[2]["duration_ms"], 102);
}

#[tokio::test]
async fn export_sheet_with_tag_exports_only_that_range() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_doc(root, 4, 2, 4, &[("walk", 2, 3)]);
    let saved = export_sheet(root, sheet_req(8, Some("walk"), 0, "m.json"))
        .await
        .expect("export");
    // Columns clamp to the 2 tagged frames.
    assert_eq!((saved.width, saved.height), (8, 2));
    let meta = read_json(&root.join("m.json"));
    let frames: Vec<u64> = meta["frames"]
        .as_array()
        .expect("frames")
        .iter()
        .map(|c| c["frame"].as_u64().expect("frame"))
        .collect();
    assert_eq!(frames, vec![2, 3]);
    assert_eq!(meta["columns"], 2);
}

#[tokio::test]
async fn export_sheet_rejects_zero_and_oversized_columns() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_doc(root, 4, 2, 3, &[]);
    for columns in [0, 65] {
        let err = export_sheet(root, sheet_req(columns, None, 0, "m.json"))
            .await
            .unwrap_err();
        assert!(is_bad_param(&err), "columns {columns}: {err}");
    }
    let err = export_sheet(root, sheet_req(1, None, 65, "m.json"))
        .await
        .unwrap_err();
    assert!(is_bad_param(&err), "padding 65: {err}");
    assert!(!root.join("sheet.png").exists());
}

#[tokio::test]
async fn export_sheet_unknown_tag_lists_valid_tags() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_doc(root, 4, 2, 3, &[("idle", 0, 1)]);
    let err = export_sheet(root, sheet_req(2, Some("run"), 0, "m.json"))
        .await
        .unwrap_err();
    assert!(is_bad_param(&err), "{err}");
    assert!(err.to_string().contains("\"idle\""), "{err}");
}

#[tokio::test]
async fn export_sheet_rejects_png_meta_output_before_writing_anything() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_doc(root, 4, 2, 3, &[]);
    let err = export_sheet(root, sheet_req(2, None, 0, "meta.png"))
        .await
        .unwrap_err();
    assert!(is_bad_param(&err), "{err}");
    assert!(!root.join("sheet.png").exists(), "no half-finished export");
    assert!(!root.join("meta.png").exists());
}

// ---------------------------------------------------------------------------
// export_tag — 2 validation, 2 adversarial
// ---------------------------------------------------------------------------

fn tag_req(tag: &str, horizontal: bool) -> ExportTagRequest {
    ExportTagRequest {
        doc: "a.lumen.json".into(),
        tag: tag.into(),
        output: "strip.png".into(),
        horizontal,
    }
}

#[tokio::test]
async fn export_tag_horizontal_filmstrip() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_doc(root, 4, 2, 4, &[("walk", 1, 3)]);
    let saved = export_tag(root, tag_req("walk", true))
        .await
        .expect("export");
    assert_eq!((saved.width, saved.height), (12, 2));
    let img = read_png(&root.join("strip.png"));
    assert_eq!(*img.get_pixel(1, 0), RED, "frame 1 first, pixel +1");
    assert_eq!(*img.get_pixel(4 + 2, 0), RED, "frame 2 second, pixel +2");
    assert_eq!(*img.get_pixel(8 + 3, 0), RED, "frame 3 third, pixel +3");
}

#[tokio::test]
async fn export_tag_vertical_filmstrip() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_doc(root, 4, 2, 4, &[("walk", 1, 3)]);
    let saved = export_tag(root, tag_req("walk", false))
        .await
        .expect("export");
    assert_eq!((saved.width, saved.height), (4, 6));
    let img = read_png(&root.join("strip.png"));
    assert_eq!(*img.get_pixel(3, 4), RED, "frame 3: third row, +3 px");
}

#[tokio::test]
async fn export_tag_rejects_missing_tag() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_doc(root, 4, 2, 2, &[]);
    let err = export_tag(root, tag_req("walk", true)).await.unwrap_err();
    assert!(is_bad_param(&err), "{err}");
    assert!(!root.join("strip.png").exists());
}

#[tokio::test]
async fn export_tag_rejects_horizontal_strip_that_would_wrap() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    // 65 one-pixel frames: fits 4096 px but exceeds the 64-column grid cap.
    write_doc(root, 1, 1, 65, &[("long", 0, 64)]);
    let err = export_tag(root, tag_req("long", true)).await.unwrap_err();
    assert!(is_bad_param(&err), "{err}");
    assert!(!root.join("strip.png").exists());
}

// ---------------------------------------------------------------------------
// import_layer — 1 validation, 4 adversarial
// ---------------------------------------------------------------------------

fn write_png(root: &Path, name: &str, w: u32, h: u32, fill: Rgba<u8>) {
    let img: RgbaImage = ImageBuffer::from_pixel(w, h, fill);
    img.save(root.join(name)).expect("save png");
}

fn import_req(png: &str, name: Option<&str>) -> ImportLayerRequest {
    ImportLayerRequest {
        doc: "a.lumen.json".into(),
        output: None,
        png: png.into(),
        name: name.map(str::to_string),
    }
}

#[tokio::test]
async fn import_layer_appends_layer_in_place_and_syncs_frames() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_doc(root, 4, 2, 3, &[]);
    write_png(root, "overlay.png", 4, 2, Rgba([0, 255, 0, 255]));
    let saved = import_layer(root, import_req("overlay.png", None))
        .await
        .expect("import");
    assert_eq!((saved.layers, saved.frames), (2, 3));
    let doc = load_doc(root, "a.lumen.json").expect("reload in-place output");
    assert_eq!(doc.layers[1].name, "imported");
    assert_eq!(*doc.layers[1].image.get_pixel(3, 1), Rgba([0, 255, 0, 255]));
    assert!(doc.frames.iter().all(|f| f.layer_mods.len() == 2));
}

#[tokio::test]
async fn import_layer_rejects_dimension_mismatch_with_both_sizes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_doc(root, 4, 2, 1, &[]);
    write_png(root, "big.png", 8, 8, RED);
    let err = import_layer(root, import_req("big.png", None))
        .await
        .unwrap_err();
    assert!(is_bad_param(&err), "{err}");
    let msg = err.to_string();
    assert!(msg.contains("8x8") && msg.contains("4x2"), "{msg}");
    assert_eq!(load_doc(root, "a.lumen.json").expect("doc").layers.len(), 1);
}

#[tokio::test]
async fn import_layer_rejects_garbage_bytes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_doc(root, 4, 2, 1, &[]);
    std::fs::write(root.join("junk.png"), b"\x89PNG\r\n\x1a\nnot really").expect("write");
    let err = import_layer(root, import_req("junk.png", None))
        .await
        .unwrap_err();
    assert!(matches!(err, LumenError::BadPng(_)), "{err}");
}

#[tokio::test]
async fn import_layer_rejects_duplicate_and_empty_names() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_doc(root, 4, 2, 1, &[]);
    write_png(root, "o.png", 4, 2, RED);
    for name in ["Background", ""] {
        let err = import_layer(root, import_req("o.png", Some(name)))
            .await
            .unwrap_err();
        assert!(is_bad_param(&err), "name {name:?}: {err}");
    }
}

#[tokio::test]
async fn import_layer_rejects_png_outside_root() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("project");
    std::fs::create_dir(&root).expect("mkdir");
    write_doc(&root, 4, 2, 1, &[]);
    write_png(dir.path(), "outside.png", 4, 2, RED);
    let err = import_layer(&root, import_req("../outside.png", None))
        .await
        .unwrap_err();
    assert!(matches!(err, LumenError::PathRejected(_)), "{err}");
}

// ---------------------------------------------------------------------------
// build_fullbody_sheet — 1 validation, 2 adversarial
// ---------------------------------------------------------------------------

fn fullbody_req(columns: u32) -> BuildFullbodySheetRequest {
    BuildFullbodySheetRequest {
        doc: "a.lumen.json".into(),
        output: "full.png".into(),
        meta_output: "full.json".into(),
        columns,
    }
}

#[tokio::test]
async fn build_fullbody_sheet_grid_rows_and_overlapping_tags() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_doc(root, 4, 2, 4, &[("idle", 0, 1), ("walk", 1, 3)]);
    let saved = build_fullbody_sheet(root, fullbody_req(2))
        .await
        .expect("build");
    assert_eq!((saved.width, saved.height), (8, 4));
    let img = read_png(&root.join("full.png"));
    assert_eq!(*img.get_pixel(4 + 3, 2), RED, "frame 3 at row 1 col 1");
    let meta = read_json(&root.join("full.json"));
    assert_eq!(xy(&meta, "columns", "rows"), (2, 2));
    let cells = meta["frames"].as_array().expect("frames");
    assert_eq!(cells[0]["tags"], serde_json::json!(["idle"]));
    assert_eq!(cells[1]["tags"], serde_json::json!(["idle", "walk"]));
    assert_eq!(xy(&cells[3], "x", "y"), (4, 2));
}

#[tokio::test]
async fn build_fullbody_sheet_single_frame_doc_clamps_columns() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_doc(root, 4, 2, 1, &[]);
    let saved = build_fullbody_sheet(root, fullbody_req(64))
        .await
        .expect("build");
    assert_eq!((saved.width, saved.height), (4, 2), "no empty columns");
    let meta = read_json(&root.join("full.json"));
    assert_eq!(xy(&meta, "columns", "rows"), (1, 1));
    assert_eq!(meta["frames"][0]["tags"], serde_json::json!([]));
}

#[tokio::test]
async fn build_fullbody_sheet_rejects_oversized_sheet_before_writing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_doc(root, 4096, 1, 2, &[]);
    let err = build_fullbody_sheet(root, fullbody_req(2))
        .await
        .unwrap_err();
    assert!(is_bad_param(&err), "{err}");
    assert!(err.to_string().contains("8192x1"), "{err}");
    assert!(!root.join("full.png").exists());
    assert!(!root.join("full.json").exists());
}

// ---------------------------------------------------------------------------
// generate_mature_variant — 2 validation, 3 adversarial
// ---------------------------------------------------------------------------

const SOLID: Rgba<u8> = Rgba([10, 20, 30, 128]);

/// 4x4 doc, every pixel the same semi-transparent color.
fn write_solid_doc(root: &Path) {
    let mut doc = new_doc(4, 4).expect("new doc");
    doc.layers[0].image = ImageBuffer::from_pixel(4, 4, SOLID);
    save_doc(root, &doc, "a.lumen.json").expect("save doc");
}

fn mature_req(head_scale: f32) -> GenerateMatureVariantRequest {
    GenerateMatureVariantRequest {
        doc: "a.lumen.json".into(),
        output: Some("mature.lumen.json".into()),
        head_scale,
    }
}

#[tokio::test]
async fn generate_mature_variant_shrinks_about_center_preserving_alpha() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_solid_doc(root);
    generate_mature_variant(root, mature_req(0.5))
        .await
        .expect("variant");
    let doc = load_doc(root, "mature.lumen.json").expect("load variant");
    let img = &doc.layers[0].image;
    for y in 0..4 {
        for x in 0..4 {
            let inside = (1..=2).contains(&x) && (1..=2).contains(&y);
            let want = if inside { SOLID } else { CLEAR };
            assert_eq!(*img.get_pixel(x, y), want, "pixel ({x},{y})");
        }
    }
    let src = load_doc(root, "a.lumen.json").expect("source untouched");
    assert_eq!(*src.layers[0].image.get_pixel(0, 0), SOLID);
}

#[tokio::test]
async fn generate_mature_variant_defaults_to_055_and_identity_at_one() {
    let req: GenerateMatureVariantRequest =
        serde_json::from_value(serde_json::json!({"doc": "a.lumen.json"})).expect("parse");
    assert_eq!(req.head_scale, 0.55);
    assert!(req.output.is_none());
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_solid_doc(root);
    generate_mature_variant(root, mature_req(1.0))
        .await
        .expect("variant");
    let doc = load_doc(root, "mature.lumen.json").expect("load");
    assert!(doc.layers[0].image.pixels().all(|p| *p == SOLID));
}

#[tokio::test]
async fn generate_mature_variant_rejects_scale_two() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_solid_doc(root);
    let err = generate_mature_variant(root, mature_req(2.0))
        .await
        .unwrap_err();
    assert!(is_bad_param(&err), "{err}");
    assert!(!root.join("mature.lumen.json").exists());
}

#[tokio::test]
async fn generate_mature_variant_rejects_nan_and_infinity() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_solid_doc(root);
    for scale in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let err = generate_mature_variant(root, mature_req(scale))
            .await
            .unwrap_err();
        assert!(is_bad_param(&err), "scale {scale}: {err}");
    }
    assert!(!root.join("mature.lumen.json").exists());
}

#[tokio::test]
async fn generate_mature_variant_rejects_scale_below_minimum() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_solid_doc(root);
    let err = generate_mature_variant(root, mature_req(0.1))
        .await
        .unwrap_err();
    assert!(is_bad_param(&err), "{err}");
}

// ---------------------------------------------------------------------------
// contact_sheet — 2 validation, 3 adversarial
// ---------------------------------------------------------------------------

fn contact_req(columns: u32, label: bool) -> ContactSheetRequest {
    ContactSheetRequest {
        doc: "a.lumen.json".into(),
        output: "contact.png".into(),
        columns,
        label,
    }
}

#[tokio::test]
async fn contact_sheet_labels_frames_with_private_digits() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_doc(root, 8, 8, 3, &[]);
    let saved = contact_sheet(root, contact_req(2, true))
        .await
        .expect("contact");
    // 8 px border + 2 cells of 8 + one 8 px gap, both axes.
    assert_eq!((saved.width, saved.height), (40, 40));
    let img = read_png(&root.join("contact.png"));
    assert_eq!(*img.get_pixel(8, 8), RED, "frame 0, outside inset");
    assert_eq!(*img.get_pixel(9, 9), WHITE, "'0' top row, 1 px inset");
    assert_eq!(*img.get_pixel(10, 11), CLEAR, "'0' is hollow");
    assert_eq!(*img.get_pixel(26, 9), WHITE, "'1' top row is 010");
    assert_eq!(*img.get_pixel(25, 9), CLEAR);
    assert_eq!(*img.get_pixel(4, 4), CLEAR, "border stays transparent");
}

#[tokio::test]
async fn contact_sheet_without_labels_has_no_white() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_doc(root, 8, 8, 3, &[]);
    contact_sheet(root, contact_req(3, false))
        .await
        .expect("contact");
    let img = read_png(&root.join("contact.png"));
    assert_eq!(img.dimensions(), (8 * 2 + 3 * 8 + 2 * 8, 8 * 2 + 8));
    assert!(img.pixels().all(|p| *p != WHITE));
}

#[tokio::test]
async fn contact_sheet_rejects_zero_columns() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_doc(root, 8, 8, 3, &[]);
    let err = contact_sheet(root, contact_req(0, true)).await.unwrap_err();
    assert!(is_bad_param(&err), "{err}");
    assert!(!root.join("contact.png").exists());
}

#[tokio::test]
async fn contact_sheet_clamps_columns_above_frame_count() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_doc(root, 8, 8, 3, &[]);
    let saved = contact_sheet(root, contact_req(64, false))
        .await
        .expect("contact");
    assert_eq!((saved.width, saved.height), (8 * 2 + 3 * 8 + 2 * 8, 24));
}

#[tokio::test]
async fn contact_sheet_clips_label_to_tiny_cell() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_doc(root, 2, 2, 1, &[]);
    contact_sheet(root, contact_req(1, true))
        .await
        .expect("contact");
    let img = read_png(&root.join("contact.png"));
    assert_eq!(img.dimensions(), (18, 18));
    assert_eq!(*img.get_pixel(9, 9), WHITE, "only in-cell label px");
    assert_eq!(*img.get_pixel(10, 9), CLEAR, "clipped, no bleed");
    assert_eq!(*img.get_pixel(9, 10), CLEAR);
}

// ---------------------------------------------------------------------------
// BRP mock server
// ---------------------------------------------------------------------------

/// Serve exactly one HTTP request on an ephemeral loopback port with a
/// fixed JSON body. The task yields the raw request (headers + body) it saw,
/// read until Content-Length is satisfied (bounded: 64 KiB, 64 reads).
async fn mock_brp(reply: &'static str) -> (String, tokio::task::JoinHandle<String>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let handle = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.expect("accept");
        let mut raw = Vec::new();
        let mut buf = [0u8; 4096];
        for _ in 0..64 {
            let n = sock.read(&mut buf).await.expect("read");
            raw.extend_from_slice(&buf[..n]);
            if n == 0 || raw.len() > 64 * 1024 || request_complete(&raw) {
                break;
            }
        }
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{reply}",
            reply.len()
        );
        sock.write_all(resp.as_bytes()).await.expect("write");
        String::from_utf8_lossy(&raw).into_owned()
    });
    (format!("http://127.0.0.1:{port}"), handle)
}

fn request_complete(raw: &[u8]) -> bool {
    let text = String::from_utf8_lossy(raw);
    let Some(split) = text.find("\r\n\r\n") else {
        return false;
    };
    let body_len = text[..split]
        .lines()
        .find_map(|l| {
            let (k, v) = l.split_once(':')?;
            k.eq_ignore_ascii_case("content-length")
                .then(|| v.trim().parse::<usize>().ok())
                .flatten()
        })
        .unwrap_or(0);
    text.len() - split - 4 >= body_len
}

fn call_req(endpoint: Option<String>, method: &str) -> BevyCallRequest {
    BevyCallRequest {
        endpoint,
        method: method.into(),
        params: None,
    }
}

/// An endpoint guaranteed closed: bind an ephemeral port, then release it.
async fn closed_endpoint() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("addr").port();
    drop(listener);
    format!("http://127.0.0.1:{port}")
}

// ---------------------------------------------------------------------------
// bevy_call — 2 validation, 5 adversarial
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bevy_call_returns_result_and_forwards_method_and_params() {
    let (endpoint, server) = mock_brp(r#"{"jsonrpc":"2.0","id":1,"result":[42]}"#).await;
    let req = BevyCallRequest {
        endpoint: Some(endpoint),
        method: "world.query".into(),
        params: Some(serde_json::json!({"data": {"components": ["Transform"]}})),
    };
    let out = bevy_call(req).await.expect("call");
    assert!(out.ok);
    assert_eq!(out.http_status, 200);
    assert_eq!(out.result, Some(serde_json::json!([42])));
    assert!(out.error.is_none());
    let seen = server.await.expect("server task");
    assert!(seen.contains("\"method\":\"world.query\""), "{seen}");
    assert!(seen.contains("Transform"), "{seen}");
}

#[tokio::test]
async fn bevy_call_omitted_params_send_null() {
    let (endpoint, server) = mock_brp(r#"{"jsonrpc":"2.0","id":1,"result":null}"#).await;
    let out = bevy_call(call_req(Some(endpoint), "bevy/list"))
        .await
        .expect("call");
    // `result: null` is still a result: the key is present.
    assert!(out.ok);
    let seen = server.await.expect("server task");
    assert!(seen.contains("\"params\":null"), "{seen}");
}

#[tokio::test]
async fn bevy_call_jsonrpc_error_is_a_result_not_an_err() {
    let (endpoint, server) = mock_brp(
        r#"{"jsonrpc":"2.0","id":1,"error":{"code":-32601,"message":"Method not found"}}"#,
    )
    .await;
    let out = bevy_call(call_req(Some(endpoint), "no/such"))
        .await
        .expect("a JSON-RPC error must come back as Ok");
    assert!(!out.ok);
    assert!(out.result.is_none());
    assert_eq!(out.error.expect("error object")["code"], -32601);
    server.await.expect("server task");
}

#[tokio::test]
async fn bevy_call_rejects_empty_and_blank_method() {
    for method in ["", "   \t\n"] {
        let err = bevy_call(call_req(None, method)).await.unwrap_err();
        assert!(is_bad_param(&err), "method {method:?}: {err}");
    }
}

#[tokio::test]
async fn bevy_call_rejects_overlong_method_and_params() {
    let long = "m".repeat(129);
    let err = bevy_call(call_req(None, &long)).await.unwrap_err();
    assert!(is_bad_param(&err), "{err}");
    let req = BevyCallRequest {
        endpoint: None,
        method: "bevy/spawn".into(),
        params: Some(serde_json::Value::String("x".repeat(1024 * 1024 + 1))),
    };
    let err = bevy_call(req).await.unwrap_err();
    assert!(is_bad_param(&err), "{err}");
}

#[tokio::test]
async fn bevy_call_rejects_remote_endpoint_before_connecting() {
    let err = bevy_call(call_req(Some("http://example.com/".into()), "bevy/list"))
        .await
        .unwrap_err();
    assert!(matches!(err, LumenError::Brp(_)), "{err}");
    assert!(err.to_string().contains("non-loopback"), "{err}");
}

#[tokio::test]
async fn bevy_call_closed_port_is_classified_unreachable() {
    let endpoint = closed_endpoint().await;
    let err = bevy_call(call_req(Some(endpoint), "bevy/list"))
        .await
        .unwrap_err();
    assert!(matches!(err, LumenError::BrpUnreachable(_)), "{err}");
}

// ---------------------------------------------------------------------------
// bevy_status — 1 validation, 2 adversarial
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bevy_status_confirms_mock_brp() {
    let (endpoint, server) = mock_brp(r#"{"jsonrpc":"2.0","id":1,"result":[]}"#).await;
    let status = bevy_status(BevyStatusRequest {
        endpoint: Some(endpoint),
    })
    .await
    .expect("status");
    assert!(status.reachable && status.speaks_brp, "{}", status.detail);
    server.await.expect("server task");
}

#[tokio::test]
async fn bevy_status_closed_port_is_a_clean_report() {
    let endpoint = closed_endpoint().await;
    let status = bevy_status(BevyStatusRequest {
        endpoint: Some(endpoint.clone()),
    })
    .await
    .expect("unreachable must be a report, never an Err");
    assert!(!status.reachable);
    assert!(!status.speaks_brp);
    assert_eq!(status.endpoint, endpoint);
}

#[tokio::test]
async fn bevy_status_rejects_remote_endpoint() {
    let err = bevy_status(BevyStatusRequest {
        endpoint: Some("http://example.com/".into()),
    })
    .await
    .unwrap_err();
    assert!(err.to_string().contains("non-loopback"), "{err}");
}

// ---------------------------------------------------------------------------
// Console backends (export_tic80, export_wasm4) — 10 ok_*, 10 adv_*
//
// The decoders below are written independently of the encoders in
// src/export.rs, from the formats themselves: TIC-80 chunk headers
// (type | bank<<5, u16 LE size, reserved) and 4bpp tiles (left pixel in the
// low nibble); WASM-4 MSB-first bit packing with no row padding.
// ---------------------------------------------------------------------------

const GREEN: Rgba<u8> = Rgba([0, 255, 0, 255]);
const BLUE: Rgba<u8> = Rgba([0, 0, 255, 255]);

/// A doc whose frame `i` shows exactly `images[i]`: one layer per frame,
/// visible only in its own frame (opacity_mult 0 elsewhere). Durations are
/// `100 + i` ms.
fn frames_doc(root: &Path, images: &[RgbaImage], palette: &[[u8; 4]]) {
    let (w, h) = images[0].dimensions();
    let mut doc = new_doc(w, h).expect("new doc");
    doc.layers = (0..images.len())
        .map(|i| Layer {
            name: format!("f{i}"),
            visible: true,
            opacity: 1.0,
            blend: BlendMode::Normal,
            image: images[i].clone(),
        })
        .collect();
    doc.frames = (0..images.len())
        .map(|i| Frame {
            duration_ms: 100 + i as u32,
            layer_mods: (0..images.len())
                .map(|j| LayerMod {
                    opacity_mult: if i == j { 1.0 } else { 0.0 },
                    ..identity_mod()
                })
                .collect(),
        })
        .collect();
    doc.palette = palette.to_vec();
    save_doc(root, &doc, "c.lumen.json").expect("save doc");
}

/// Deterministic test image: pixel (x,y) takes `colors[(x + 2y + seed) % len]`.
fn pattern(w: u32, h: u32, colors: &[Rgba<u8>], seed: usize) -> RgbaImage {
    ImageBuffer::from_fn(w, h, |x, y| {
        colors[(x as usize + 2 * y as usize + seed) % colors.len()]
    })
}

fn tic_req(output: &str, tag: Option<&str>) -> ExportTic80Request {
    ExportTic80Request {
        doc: "c.lumen.json".into(),
        output: output.into(),
        meta_output: "out/cart.json".into(),
        tag: tag.map(str::to_string),
    }
}

fn w4_req(output: &str, bpp: u8, name: Option<&str>) -> ExportWasm4Request {
    ExportWasm4Request {
        doc: "c.lumen.json".into(),
        output: output.into(),
        meta_output: "out/sprite.json".into(),
        tag: None,
        bpp,
        name: name.map(str::to_string),
    }
}

/// Walk a .tic file chunk by chunk; panics on any malformed header or a
/// trailing partial chunk. Returns (type, bank, data) in file order.
fn tic_chunks(cart: &[u8]) -> Vec<(u8, u8, Vec<u8>)> {
    let mut chunks = Vec::new();
    let mut at = 0;
    while at < cart.len() {
        assert!(at + 4 <= cart.len(), "truncated chunk header at {at}");
        let size = usize::from(u16::from_le_bytes([cart[at + 1], cart[at + 2]]));
        assert!(size > 0, "zero size means 64 KiB to TIC-80");
        assert_eq!(cart[at + 3], 0, "reserved byte");
        assert!(at + 4 + size <= cart.len(), "chunk overruns the file");
        chunks.push((
            cart[at] & 0x1f,
            cart[at] >> 5,
            cart[at + 4..at + 4 + size].to_vec(),
        ));
        at += 4 + size;
    }
    chunks
}

/// The combined 512-tile sheet (TILES then SPRITES), the 16-entry palette,
/// and the Lua code of a cart.
fn tic_decode(cart: &[u8]) -> (Vec<u8>, Vec<[u8; 3]>, String) {
    let chunks = tic_chunks(cart);
    let get = |t: u8| chunks.iter().find(|c| c.0 == t).map(|c| c.2.clone());
    let mut sheet = get(1).expect("TILES chunk");
    sheet.resize(8192, 0);
    sheet.extend(get(2).unwrap_or_default());
    sheet.resize(16384, 0);
    let pal = get(12).expect("PALETTE chunk");
    let palette = pal.chunks(3).map(|c| [c[0], c[1], c[2]]).collect();
    let code = String::from_utf8(get(5).expect("CODE chunk")).expect("utf8 code");
    (sheet, palette, code)
}

/// Palette index of pixel (x,y) of the block whose top-left sprite is `id`.
fn tic_pixel(sheet: &[u8], id: u64, x: u32, y: u32) -> u8 {
    let tile = id as usize + (y / 8) as usize * 16 + (x / 8) as usize;
    let byte = sheet[tile * 32 + (y % 8) as usize * 4 + (x % 8) as usize / 2];
    if x.is_multiple_of(2) {
        byte & 0x0f
    } else {
        byte >> 4
    }
}

/// Assert every frame decodes back to its source image: transparent pixels
/// hit the reported transparent index, opaque ones a palette slot whose RGB
/// equals the source.
fn assert_tic_roundtrip(root: &Path, cart: &str, images: &[RgbaImage]) {
    let (sheet, palette, _) = tic_decode(&std::fs::read(root.join(cart)).expect("cart"));
    let meta = read_json(&root.join("out/cart.json"));
    let key = meta["transparent_index"].as_u64();
    for (frame, img) in meta["frames"]
        .as_array()
        .expect("frames")
        .iter()
        .zip(images)
    {
        let id = frame["sprite_id"].as_u64().expect("id");
        for (x, y, px) in img.enumerate_pixels() {
            let idx = tic_pixel(&sheet, id, x, y);
            if px[3] == 0 {
                assert_eq!(Some(u64::from(idx)), key, "transparent pixel ({x},{y})");
            } else {
                assert_ne!(
                    Some(u64::from(idx)),
                    key,
                    "opaque pixel ({x},{y}) keyed out"
                );
                assert_eq!(
                    palette[usize::from(idx)],
                    [px[0], px[1], px[2]],
                    "({x},{y})"
                );
            }
        }
    }
}

/// The byte array of a generated WASM-4 Rust source.
fn w4_bytes(source: &str) -> Vec<u8> {
    let start = source.rfind("= [\n").expect("array start") + 4;
    let end = source.rfind("];").expect("array end");
    source[start..end]
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| u8::from_str_radix(s.trim_start_matches("0x"), 16).expect("hex byte"))
        .collect()
}

/// Sprite value of pixel `i` (row-major across stacked frames).
fn w4_pixel(blob: &[u8], bpp: usize, i: usize) -> u8 {
    let bit = i * bpp;
    (blob[bit / 8] >> (8 - bpp - bit % 8)) & ((1 << bpp) - 1)
}

/// Decode every pixel through PALETTE + DRAW_COLORS exactly as the console
/// would and compare with the source images.
fn assert_w4_roundtrip(root: &Path, source: &str, images: &[RgbaImage]) {
    let blob = w4_bytes(&std::fs::read_to_string(root.join(source)).expect("source"));
    let meta = read_json(&root.join("out/sprite.json"));
    let bpp = meta["bpp"].as_u64().expect("bpp") as usize;
    let draw_colors = meta["draw_colors"].as_u64().expect("draw colors");
    assert_eq!(blob.len() as u64, meta["bytes"].as_u64().expect("bytes"));
    let palette: Vec<String> = serde_json::from_value(meta["palette"].clone()).expect("palette");
    let per_frame = images[0].width() as usize * images[0].height() as usize;
    for (f, img) in images.iter().enumerate() {
        for (i, px) in img.pixels().enumerate() {
            let value = w4_pixel(&blob, bpp, f * per_frame + i);
            let nibble = (draw_colors >> (4 * u64::from(value))) & 0xf;
            if px[3] == 0 {
                assert_eq!(nibble, 0, "frame {f} pixel {i} must draw transparent");
            } else {
                let hex = format!("#{:02X}{:02X}{:02X}", px[0], px[1], px[2]);
                assert!(nibble >= 1, "frame {f} pixel {i} drawn transparent");
                assert_eq!(palette[nibble as usize - 1], hex, "frame {f} pixel {i}");
            }
        }
    }
}

#[tokio::test]
async fn ok_tic80_roundtrip_two_frames_with_transparency() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let images = [
        pattern(16, 8, &[RED, GREEN, CLEAR], 0),
        pattern(16, 8, &[BLUE, CLEAR, RED], 1),
    ];
    frames_doc(root, &images, &[]);
    let saved = export_tic80(root, tic_req("out/x.tic", None))
        .await
        .expect("export");
    assert_eq!((saved.width, saved.height), (16, 8));
    let cart = std::fs::read(root.join("out/x.tic")).expect("cart");
    let types: Vec<(u8, u8)> = tic_chunks(&cart).iter().map(|c| (c.0, c.1)).collect();
    assert_eq!(
        types,
        [(1, 0), (2, 0), (12, 0), (5, 0)],
        "TILES SPRITES PALETTE CODE, bank 0"
    );
    let meta = read_json(&root.join("out/cart.json"));
    assert_eq!(meta["transparent_index"], 0);
    assert_eq!(xy(&meta, "tiles_w", "tiles_h"), (2, 1));
    assert_eq!(
        meta["palette"].as_array().expect("palette").len(),
        4,
        "clear + 3 colors"
    );
    assert_tic_roundtrip(root, "out/x.tic", &images);
}

#[tokio::test]
async fn ok_tic80_bound_palette_order_is_preserved() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let images = [pattern(8, 8, &[RED, BLUE], 0)];
    // The bound palette lists an unused color first and duplicates blue.
    let bound = [
        [9, 9, 9, 255],
        [0, 0, 255, 255],
        [255, 0, 0, 255],
        [0, 0, 255, 255],
    ];
    frames_doc(root, &images, &bound);
    export_tic80(root, tic_req("x.tic", None))
        .await
        .expect("export");
    let meta = read_json(&root.join("out/cart.json"));
    let palette: Vec<&str> = meta["palette"]
        .as_array()
        .expect("palette")
        .iter()
        .map(|v| v.as_str().expect("hex"))
        .collect();
    assert_eq!(
        palette,
        ["#090909", "#0000FF", "#FF0000"],
        "bound order, deduplicated"
    );
    assert!(meta["transparent_index"].is_null());
    assert_tic_roundtrip(root, "x.tic", &images);
}

#[tokio::test]
async fn ok_tic80_tag_range_packs_blocks_row_major() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    // 64x8 frames are 8x1 tiles: two blocks per 16-tile row.
    let images: Vec<RgbaImage> = (0..5)
        .map(|s| pattern(64, 8, &[RED, GREEN, BLUE], s))
        .collect();
    frames_doc(root, &images, &[]);
    let mut doc = load_doc(root, "c.lumen.json").expect("load");
    doc.tags = vec![Tag {
        name: "walk".into(),
        from_frame: 1,
        to_frame: 3,
    }];
    save_doc(root, &doc, "c.lumen.json").expect("save");
    export_tic80(root, tic_req("x.tic", Some("walk")))
        .await
        .expect("export");
    let meta = read_json(&root.join("out/cart.json"));
    let cells: Vec<(u64, u64, u64)> = meta["frames"]
        .as_array()
        .expect("frames")
        .iter()
        .map(|c| {
            (
                c["frame"].as_u64().unwrap(),
                c["sprite_id"].as_u64().unwrap(),
                c["duration_ms"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(cells, [(1, 0, 101), (2, 8, 102), (3, 16, 103)]);
    assert_tic_roundtrip(root, "x.tic", &images[1..4]);
}

#[tokio::test]
async fn ok_tic80_sixteen_opaque_colors_fill_the_palette() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let colors: Vec<Rgba<u8>> = (0..16u8)
        .map(|i| Rgba([i * 16, 255 - i * 16, i, 255]))
        .collect();
    let images = [pattern(16, 8, &colors, 0)];
    frames_doc(root, &images, &[]);
    export_tic80(root, tic_req("x.tic", None))
        .await
        .expect("export");
    let (_, palette, code) = tic_decode(&std::fs::read(root.join("x.tic")).expect("cart"));
    assert_eq!(palette.len(), 16);
    assert!(
        code.contains(",-1,1,0,0,2,1)"),
        "no colorkey when nothing is transparent:\n{code}"
    );
    assert_tic_roundtrip(root, "x.tic", &images);
}

#[tokio::test]
async fn ok_tic80_fills_both_banks_to_sprite_511() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    // 512 one-tile frames; frame i moves the red pixel to x = i % 8.
    let mut doc = new_doc(8, 8).expect("new doc");
    doc.layers[0].image.put_pixel(0, 0, RED);
    doc.frames = (0..512)
        .map(|i| Frame {
            duration_ms: 50,
            layer_mods: vec![LayerMod {
                offset_x: i % 8,
                ..identity_mod()
            }],
        })
        .collect();
    save_doc(root, &doc, "c.lumen.json").expect("save");
    export_tic80(root, tic_req("x.tic", None))
        .await
        .expect("export");
    let (sheet, palette, _) = tic_decode(&std::fs::read(root.join("x.tic")).expect("cart"));
    let meta = read_json(&root.join("out/cart.json"));
    assert_eq!(meta["frames"][511]["sprite_id"], 511);
    assert_eq!(
        palette[usize::from(tic_pixel(&sheet, 511, 7, 0))],
        [255, 0, 0]
    );
    assert_eq!(
        tic_pixel(&sheet, 511, 0, 0),
        0,
        "frame 511 is transparent at x=0"
    );
    assert_eq!(
        palette[usize::from(tic_pixel(&sheet, 256, 0, 0))],
        [255, 0, 0],
        "SPRITES bank"
    );
}

#[tokio::test]
async fn ok_tic80_code_chunk_loops_ids_and_durations() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let images = [
        pattern(8, 8, &[RED, CLEAR], 0),
        pattern(8, 8, &[GREEN, CLEAR], 0),
    ];
    frames_doc(root, &images, &[]);
    export_tic80(root, tic_req("x.tic", None))
        .await
        .expect("export");
    let (_, _, code) = tic_decode(&std::fs::read(root.join("x.tic")).expect("cart"));
    assert!(
        code.starts_with("-- title:  lumen export\n-- script: lua\n"),
        "{code}"
    );
    assert!(
        code.contains("local F={0,1}\nlocal D={100,101}\n"),
        "{code}"
    );
    assert!(code.contains("function TIC()") && code.contains("spr(F[i],116,64,0,1,0,0,1,1)"));
}

#[tokio::test]
async fn ok_wasm4_2bpp_roundtrip_unaligned_frames() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    // 5x3 = 15 px = 30 bits per frame: frame 1 starts mid-byte.
    let images = [
        pattern(5, 3, &[RED, GREEN, BLUE, CLEAR], 0),
        pattern(5, 3, &[CLEAR, BLUE, RED], 2),
    ];
    frames_doc(root, &images, &[]);
    let saved = export_wasm4(root, w4_req("out/hero.rs", 2, Some("HERO")))
        .await
        .expect("export");
    assert_eq!((saved.width, saved.height), (5, 3));
    let meta = read_json(&root.join("out/sprite.json"));
    assert_eq!(meta["flags"], 1, "BLIT_2BPP");
    assert_eq!(
        meta["draw_colors"], 0x4320,
        "value 0 transparent, 1..3 -> colors 2..4"
    );
    assert_eq!(meta["bytes"], 8, "60 bits round up to 8 bytes");
    assert_eq!(meta["frames"][1]["src_y"], 3);
    assert_w4_roundtrip(root, "out/hero.rs", &images);
}

#[tokio::test]
async fn ok_wasm4_1bpp_opaque_two_colors() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let images = [pattern(8, 2, &[WHITE, RED], 0)];
    frames_doc(root, &images, &[]);
    export_wasm4(root, w4_req("s.rs", 1, None))
        .await
        .expect("export");
    let meta = read_json(&root.join("out/sprite.json"));
    assert_eq!(
        (meta["flags"].as_u64(), meta["draw_colors"].as_u64()),
        (Some(0), Some(0x21))
    );
    assert!(meta["transparent_index"].is_null());
    // Each row alternates WHITE (value 0), RED (value 1), MSB first.
    assert_eq!(
        w4_bytes(&std::fs::read_to_string(root.join("s.rs")).unwrap()),
        [0x55, 0x55]
    );
    assert_w4_roundtrip(root, "s.rs", &images);
}

#[tokio::test]
async fn ok_wasm4_1bpp_one_color_plus_transparency() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let images = [pattern(3, 3, &[BLUE, CLEAR], 0)];
    frames_doc(root, &images, &[]);
    export_wasm4(root, w4_req("s.rs", 1, None))
        .await
        .expect("export");
    let meta = read_json(&root.join("out/sprite.json"));
    assert_eq!(meta["draw_colors"], 0x20);
    assert_eq!(
        meta["palette"],
        serde_json::json!(["#000000", "#0000FF", "#000000", "#000000"])
    );
    assert_w4_roundtrip(root, "s.rs", &images);
}

#[tokio::test]
async fn ok_wasm4_source_declares_consistent_constants() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let images: Vec<RgbaImage> = (0..3)
        .map(|s| pattern(16, 16, &[RED, GREEN, CLEAR], s))
        .collect();
    frames_doc(root, &images, &[]);
    export_wasm4(root, w4_req("src/hero.rs", 2, Some("HERO_2")))
        .await
        .expect("export");
    let source = std::fs::read_to_string(root.join("src/hero.rs")).expect("source");
    for line in [
        "// Generated by lumen: WASM-4 sprite ASSET SOURCE (not a cart).",
        "pub const HERO_2_WIDTH: u32 = 16;",
        "pub const HERO_2_HEIGHT: u32 = 16;",
        "pub const HERO_2_FRAMES: u32 = 3;",
        "pub const HERO_2_FLAGS: u32 = 1; // BLIT_2BPP",
        "pub const HERO_2_PALETTE: [u32; 4] = [0x000000, 0xff0000, 0x00ff00, 0x000000];",
        "pub const HERO_2_DRAW_COLORS: u16 = 0x4320;",
        "pub const HERO_2: [u8; 192] = [",
    ] {
        assert!(source.contains(line), "missing {line:?}");
    }
    assert_eq!(w4_bytes(&source).len(), 192, "16*16*3 px * 2 bits / 8");
    assert_w4_roundtrip(root, "src/hero.rs", &images);
}

/// Neither output nor sidecar exists after a rejected export.
fn assert_nothing_written(root: &Path, outputs: &[&str]) {
    for out in outputs {
        assert!(!root.join(out).exists(), "{out} must not be written");
    }
}

#[tokio::test]
async fn adv_tic80_rejects_dimensions_not_tile_aligned() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    frames_doc(root, &[pattern(10, 8, &[RED], 0)], &[]);
    let err = export_tic80(root, tic_req("x.tic", None))
        .await
        .unwrap_err();
    assert!(
        is_bad_param(&err) && err.to_string().contains("multiples of 8"),
        "{err}"
    );
    frames_doc(root, &[pattern(8, 12, &[RED], 0)], &[]);
    let err = export_tic80(root, tic_req("x.tic", None))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("multiples of 8"), "{err}");
    assert_nothing_written(root, &["x.tic", "out/cart.json"]);
}

#[tokio::test]
async fn adv_tic80_rejects_frames_beyond_sheet_capacity() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    frames_doc(root, &[pattern(136, 8, &[RED], 0)], &[]);
    let err = export_tic80(root, tic_req("x.tic", None))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("128x256"), "too wide: {err}");
    // 128x136 px blocks: only one fits the 128x256 sheet.
    frames_doc(
        root,
        &[pattern(128, 136, &[RED], 0), pattern(128, 136, &[RED], 1)],
        &[],
    );
    let err = export_tic80(root, tic_req("x.tic", None))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("holds 1..=1"), "{err}");
    let mut doc = new_doc(8, 8).expect("new doc");
    doc.layers[0].image.put_pixel(0, 0, RED);
    doc.frames = vec![doc.frames[0].clone(); 513];
    save_doc(root, &doc, "c.lumen.json").expect("save");
    let err = export_tic80(root, tic_req("x.tic", None))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("513 frames") && err.to_string().contains("1..=512"));
    assert_nothing_written(root, &["x.tic", "out/cart.json"]);
}

#[tokio::test]
async fn adv_tic80_rejects_palette_overflow() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let colors: Vec<Rgba<u8>> = (0..17u8).map(|i| Rgba([i, 0, 0, 255])).collect();
    frames_doc(root, &[pattern(32, 8, &colors, 0)], &[]);
    let err = export_tic80(root, tic_req("x.tic", None))
        .await
        .unwrap_err();
    assert!(
        is_bad_param(&err) && err.to_string().contains("quantize"),
        "17 colors: {err}"
    );
    // 16 colors fit alone, but not with a transparent slot on top.
    let mut sixteen: Vec<Rgba<u8>> = colors[..16].to_vec();
    sixteen.push(CLEAR);
    frames_doc(root, &[pattern(32, 8, &sixteen, 0)], &[]);
    let err = export_tic80(root, tic_req("x.tic", None))
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("16 colors + 1 transparent slot"),
        "{err}"
    );
    assert_nothing_written(root, &["x.tic", "out/cart.json"]);
}

#[tokio::test]
async fn adv_tic80_rejects_bound_palette_violations() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    frames_doc(
        root,
        &[pattern(8, 8, &[RED, GREEN], 0)],
        &[[255, 0, 0, 255], [0, 0, 255, 255]],
    );
    let err = export_tic80(root, tic_req("x.tic", None))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("#00FF00") && err.to_string().contains("palette_apply"));
    let bound: Vec<[u8; 4]> = (0..17u8).map(|i| [i, 0, 0, 255]).collect();
    frames_doc(root, &[pattern(8, 8, &[Rgba([0, 0, 0, 255])], 0)], &bound);
    let err = export_tic80(root, tic_req("x.tic", None))
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("needs 17 colors"),
        "oversized bound palette: {err}"
    );
    assert_nothing_written(root, &["x.tic", "out/cart.json"]);
}

#[tokio::test]
async fn adv_console_exports_reject_empty_sprite() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    frames_doc(root, &[pattern(8, 8, &[CLEAR], 0)], &[]);
    let err = export_tic80(root, tic_req("x.tic", None))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("fully transparent"), "{err}");
    let err = export_wasm4(root, w4_req("s.rs", 2, None))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("fully transparent"), "{err}");
    assert_nothing_written(root, &["x.tic", "s.rs", "out/cart.json", "out/sprite.json"]);
}

#[tokio::test]
async fn adv_console_exports_reject_partial_alpha() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    frames_doc(
        root,
        &[pattern(8, 8, &[RED, Rgba([0, 255, 0, 128])], 0)],
        &[],
    );
    let err = export_tic80(root, tic_req("x.tic", None))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("alpha 128"), "{err}");
    let err = export_wasm4(root, w4_req("s.rs", 2, None))
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("pixel (1,0) has alpha 128"),
        "{err}"
    );
    assert_nothing_written(root, &["x.tic", "s.rs", "out/cart.json", "out/sprite.json"]);
}

#[tokio::test]
async fn adv_console_exports_reject_path_escape_and_wrong_suffix() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    frames_doc(root, &[pattern(8, 8, &[RED], 0)], &[]);
    let err = export_tic80(root, tic_req("../escaped.tic", None))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("escapes"), "{err}");
    let err = export_wasm4(root, w4_req("../escaped.rs", 1, None))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("escapes"), "{err}");
    let mut req = tic_req("x.tic", None);
    req.meta_output = "../escaped.json".into();
    let err = export_tic80(root, req).await.unwrap_err();
    assert!(err.to_string().contains("escapes"), "meta escape: {err}");
    let err = export_tic80(root, tic_req("x.png", None))
        .await
        .unwrap_err();
    assert!(is_bad_param(&err), "{err}");
    let err = export_wasm4(root, w4_req("s.wasm", 1, None))
        .await
        .unwrap_err();
    assert!(is_bad_param(&err), "{err}");
    let parent = root.parent().expect("tempdir parent");
    assert_nothing_written(parent, &["escaped.tic", "escaped.rs", "escaped.json"]);
    assert_nothing_written(root, &["x.tic", "x.png", "s.wasm"]);
}

#[tokio::test]
async fn adv_wasm4_rejects_bit_depth_violations() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    frames_doc(root, &[pattern(8, 8, &[RED, GREEN, BLUE], 0)], &[]);
    for bpp in [0, 3, 4, 8] {
        let err = export_wasm4(root, w4_req("s.rs", bpp, None))
            .await
            .unwrap_err();
        assert!(err.to_string().contains("1 or 2 bpp"), "bpp {bpp}: {err}");
    }
    // Three colors cannot be 1 bpp; four colors + transparency cannot be 2.
    let err = export_wasm4(root, w4_req("s.rs", 1, None))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("2 palette slots"), "{err}");
    frames_doc(
        root,
        &[pattern(8, 8, &[RED, GREEN, BLUE, WHITE, CLEAR], 0)],
        &[],
    );
    let err = export_wasm4(root, w4_req("s.rs", 2, None))
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("4 colors + 1 transparent slot"),
        "{err}"
    );
    assert_nothing_written(root, &["s.rs", "out/sprite.json"]);
}

#[tokio::test]
async fn adv_wasm4_rejects_code_injection_in_name() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    frames_doc(root, &[pattern(8, 8, &[RED], 0)], &[]);
    let long = "A".repeat(33);
    for name in [
        "",
        "hero",
        "1HERO",
        "HERO: u8 = 0; fn evil() {} const X",
        "A\nB",
        "HÉRO",
        &long,
    ] {
        let err = export_wasm4(root, w4_req("s.rs", 1, Some(name)))
            .await
            .unwrap_err();
        assert!(
            is_bad_param(&err) && err.to_string().contains("[A-Z]"),
            "{name:?}: {err}"
        );
    }
    assert_nothing_written(root, &["s.rs", "out/sprite.json"]);
}

#[tokio::test]
async fn adv_wasm4_rejects_oversized_frames_and_blob() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    frames_doc(root, &[pattern(161, 1, &[RED], 0)], &[]);
    let err = export_wasm4(root, w4_req("s.rs", 1, None))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("160x160 screen"), "{err}");
    // 160x160 at 2 bpp is 6400 bytes per frame; six frames exceed 32 KiB.
    let mut doc = new_doc(160, 160).expect("new doc");
    doc.layers[0].image.put_pixel(0, 0, RED);
    doc.frames = vec![doc.frames[0].clone(); 6];
    save_doc(root, &doc, "c.lumen.json").expect("save");
    let err = export_wasm4(root, w4_req("s.rs", 2, None))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("38400 bytes"), "{err}");
    assert_nothing_written(root, &["s.rs", "out/sprite.json"]);
}
