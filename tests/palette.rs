//! Palette tool tests: validation (honest input does the job) and adversarial
//! (hostile or malformed input fails closed with a typed error).
//!
//! Count: 21 tests — 11 validation, 10 adversarial.

use std::path::Path;

use image::{ImageBuffer, Rgba};
use lumen::LumenError;
use lumen::doc::{BlendMode, Layer, SpriteDoc, identity_mod, load_doc, new_doc, save_doc};
use lumen::palette::{
    PaletteApplyRequest, PaletteExtractRequest, PalettePresetsRequest, PaletteRampRequest,
    QuantizeRequest, palette_apply, palette_extract, palette_presets, palette_ramp, quantize,
};

/// 4x4 doc: one layer with the left half `left` and right half `right`.
fn two_tone_doc(root: &Path, name: &str, left: [u8; 4], right: [u8; 4]) -> SpriteDoc {
    let mut doc = new_doc(4, 4).expect("new doc");
    for (x, _, px) in doc.layers[0].image.enumerate_pixels_mut() {
        px.0 = if x < 2 { left } else { right };
    }
    save_doc(root, &doc, name).expect("save");
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

fn apply_req(doc: &str, palette: &str) -> PaletteApplyRequest {
    PaletteApplyRequest {
        doc: doc.to_string(),
        output: None,
        layer: None,
        palette: palette.to_string(),
        dither: false,
    }
}

fn assert_bad_param<T: std::fmt::Debug>(r: Result<T, LumenError>, needle: &str) {
    match r {
        Err(LumenError::BadParam(msg)) => assert!(msg.contains(needle), "message: {msg}"),
        other => panic!("expected BadParam containing {needle:?}, got {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// palette_presets — 2 validation, 1 adversarial
// ---------------------------------------------------------------------------

#[tokio::test]
async fn presets_list_has_expected_names_and_counts() {
    let dir = tempfile::tempdir().expect("tempdir");
    let list = palette_presets(dir.path(), PalettePresetsRequest {}).await.expect("presets");
    let got: Vec<(&str, usize)> =
        list.palettes.iter().map(|p| (p.name.as_str(), p.entries)).collect();
    assert_eq!(
        got,
        vec![("pico8", 16), ("sweetie16", 16), ("gameboy", 4), ("nes", 54), ("endesga32", 32)]
    );
    for p in &list.palettes {
        assert_eq!(p.colors.len(), p.entries);
    }
}

#[tokio::test]
async fn presets_known_anchor_colors() {
    let dir = tempfile::tempdir().expect("tempdir");
    let list = palette_presets(dir.path(), PalettePresetsRequest {}).await.expect("presets");
    let pico = &list.palettes[0];
    assert_eq!(pico.colors[0], "#000000");
    assert_eq!(pico.colors[8], "#FF004D");
    assert_eq!(list.palettes[2].colors[0], "#0F380F");
}

#[tokio::test]
async fn presets_have_no_duplicate_entries() {
    // Adversarial against my own data entry: a duplicated hex would silently
    // shrink a palette's effective size.
    let dir = tempfile::tempdir().expect("tempdir");
    let list = palette_presets(dir.path(), PalettePresetsRequest {}).await.expect("presets");
    for p in &list.palettes {
        let mut sorted = p.colors.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), p.entries, "duplicates in {}", p.name);
        assert!(p.colors.iter().all(|c| c.len() == 7 && c.starts_with('#')));
    }
}

// ---------------------------------------------------------------------------
// palette_apply — 3 validation, 3 adversarial
// ---------------------------------------------------------------------------

#[tokio::test]
async fn apply_custom_palette_maps_to_nearest_and_keeps_alpha() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    two_tone_doc(root, "a.lumen.json", [20, 20, 20, 255], [240, 230, 250, 128]);
    let mut req = apply_req("a.lumen.json", "#000000, #FFFFFF");
    req.output = Some("out/b.lumen.json".to_string());
    let saved = palette_apply(root, req).await.expect("apply");
    assert!(saved.path.ends_with("b.lumen.json"));
    let doc = load_doc(root, "out/b.lumen.json").expect("load");
    assert_eq!(doc.layers[0].image.get_pixel(0, 0).0, [0, 0, 0, 255]);
    assert_eq!(doc.layers[0].image.get_pixel(3, 3).0, [255, 255, 255, 128]);
    // Input untouched when output is given.
    let orig = load_doc(root, "a.lumen.json").expect("load");
    assert_eq!(orig.layers[0].image.get_pixel(0, 0).0, [20, 20, 20, 255]);
}

#[tokio::test]
async fn apply_preset_in_place_only_selected_layer() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let mut doc = two_tone_doc(root, "a.lumen.json", [10, 50, 10, 255], [10, 50, 10, 255]);
    add_layer(&mut doc, "Top", [10, 50, 10, 255]);
    save_doc(root, &doc, "a.lumen.json").expect("save");
    let mut req = apply_req("a.lumen.json", "gameboy");
    req.layer = Some(1);
    palette_apply(root, req).await.expect("apply");
    let doc = load_doc(root, "a.lumen.json").expect("load");
    assert_eq!(doc.layers[0].image.get_pixel(0, 0).0, [10, 50, 10, 255]);
    assert_eq!(doc.layers[1].image.get_pixel(0, 0).0, [0x0F, 0x38, 0x0F, 255]);
}

#[tokio::test]
async fn apply_dither_mixes_two_entries_on_midtone() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    two_tone_doc(root, "a.lumen.json", [128, 128, 128, 255], [128, 128, 128, 255]);
    let mut req = apply_req("a.lumen.json", "#000000,#FFFFFF");
    req.dither = true;
    palette_apply(root, req).await.expect("apply");
    let doc = load_doc(root, "a.lumen.json").expect("load");
    let blacks = doc.layers[0].image.pixels().filter(|p| p[0] == 0).count();
    assert!(blacks > 0 && blacks < 16, "ordered dither should mix, got {blacks} black");
    assert!(doc.layers[0].image.pixels().all(|p| p[0] == 0 || p[0] == 255));
}

#[tokio::test]
async fn apply_unknown_preset_lists_valid_presets() {
    let dir = tempfile::tempdir().expect("tempdir");
    two_tone_doc(dir.path(), "a.lumen.json", [1, 2, 3, 255], [4, 5, 6, 255]);
    let r = palette_apply(dir.path(), apply_req("a.lumen.json", "c64")).await;
    assert_bad_param(r, "pico8, sweetie16, gameboy, nes, endesga32");
}

#[tokio::test]
async fn apply_rejects_one_entry_and_malformed_custom_palettes() {
    let dir = tempfile::tempdir().expect("tempdir");
    two_tone_doc(dir.path(), "a.lumen.json", [1, 2, 3, 255], [4, 5, 6, 255]);
    let r = palette_apply(dir.path(), apply_req("a.lumen.json", "#FF0000")).await;
    assert_bad_param(r, "need 2..=256");
    let r = palette_apply(dir.path(), apply_req("a.lumen.json", "#FF0000,#GG0000")).await;
    assert_bad_param(r, "not #RRGGBB");
    // Multibyte UTF-8 where hex digits belong must not panic on slicing.
    let r = palette_apply(dir.path(), apply_req("a.lumen.json", "#ÿÿÿ,#000000")).await;
    assert_bad_param(r, "not #RRGGBB");
    let huge = vec!["#000000"; 257].join(",");
    let r = palette_apply(dir.path(), apply_req("a.lumen.json", &huge)).await;
    assert_bad_param(r, "need 2..=256");
}

#[tokio::test]
async fn apply_rejects_bad_layer_and_bad_output() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    two_tone_doc(root, "a.lumen.json", [1, 2, 3, 255], [4, 5, 6, 255]);
    let mut req = apply_req("a.lumen.json", "pico8");
    req.layer = Some(1);
    assert_bad_param(palette_apply(root, req).await, "out of range");
    let mut req = apply_req("a.lumen.json", "pico8");
    req.output = Some("evil.json".to_string());
    assert_bad_param(palette_apply(root, req).await, ".lumen.json");
    let mut req = apply_req("a.lumen.json", "pico8");
    req.output = Some("../escape.lumen.json".to_string());
    assert!(matches!(palette_apply(root, req).await, Err(LumenError::PathRejected(_))));
}

// ---------------------------------------------------------------------------
// palette_extract — 2 validation, 2 adversarial
// ---------------------------------------------------------------------------

#[tokio::test]
async fn extract_counts_sorted_desc_and_skips_transparent() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let mut doc = new_doc(4, 4).expect("new");
    for (x, y, px) in doc.layers[0].image.enumerate_pixels_mut() {
        px.0 = match (x, y) {
            (0, _) => [255, 0, 0, 255],
            (1, 0) => [0, 255, 0, 255],
            _ => [0, 0, 0, 0],
        };
    }
    save_doc(root, &doc, "a.lumen.json").expect("save");
    let req = PaletteExtractRequest { doc: "a.lumen.json".into(), layer: None, max_colors: 8 };
    let got = palette_extract(root, req).await.expect("extract");
    let pairs: Vec<(&str, u64)> =
        got.colors.iter().map(|c| (c.color.as_str(), c.count)).collect();
    assert_eq!(pairs, vec![("#FF0000FF", 4), ("#00FF00FF", 1)]);
}

#[tokio::test]
async fn extract_truncates_and_sums_across_layers() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let mut doc = two_tone_doc(root, "a.lumen.json", [9, 9, 9, 255], [7, 7, 7, 255]);
    add_layer(&mut doc, "Top", [9, 9, 9, 255]);
    save_doc(root, &doc, "a.lumen.json").expect("save");
    let req = PaletteExtractRequest { doc: "a.lumen.json".into(), layer: None, max_colors: 1 };
    let got = palette_extract(root, req).await.expect("extract");
    assert_eq!(got.colors.len(), 1);
    assert_eq!(got.colors[0].color, "#090909FF");
    assert_eq!(got.colors[0].count, 8 + 16);
}

#[tokio::test]
async fn extract_rejects_zero_and_oversized_max_colors() {
    let dir = tempfile::tempdir().expect("tempdir");
    two_tone_doc(dir.path(), "a.lumen.json", [1, 2, 3, 255], [4, 5, 6, 255]);
    for max_colors in [0, 1025] {
        let req = PaletteExtractRequest { doc: "a.lumen.json".into(), layer: None, max_colors };
        assert_bad_param(palette_extract(dir.path(), req).await, "max_colors");
    }
}

#[tokio::test]
async fn extract_rejects_missing_and_non_doc_paths() {
    let dir = tempfile::tempdir().expect("tempdir");
    let req = PaletteExtractRequest { doc: "nope.lumen.json".into(), layer: None, max_colors: 4 };
    assert!(matches!(palette_extract(dir.path(), req).await, Err(LumenError::PathRejected(_))));
    let req = PaletteExtractRequest { doc: "/etc/passwd".into(), layer: None, max_colors: 4 };
    assert!(matches!(palette_extract(dir.path(), req).await, Err(LumenError::PathRejected(_))));
}

// ---------------------------------------------------------------------------
// palette_ramp — 2 validation, 2 adversarial
// ---------------------------------------------------------------------------

#[tokio::test]
async fn ramp_black_to_white_five_steps() {
    let dir = tempfile::tempdir().expect("tempdir");
    let req = PaletteRampRequest { from: "#000000".into(), to: "#FFFFFF".into(), steps: 5 };
    let ramp = palette_ramp(dir.path(), req).await.expect("ramp");
    assert_eq!(ramp.colors, vec!["#000000", "#404040", "#808080", "#BFBFBF", "#FFFFFF"]);
}

#[tokio::test]
async fn ramp_endpoints_exact_at_max_steps_and_alpha_ignored() {
    let dir = tempfile::tempdir().expect("tempdir");
    let req = PaletteRampRequest { from: "#12345600".into(), to: "#abcdefff".into(), steps: 256 };
    let ramp = palette_ramp(dir.path(), req).await.expect("ramp");
    assert_eq!(ramp.colors.len(), 256);
    assert_eq!(ramp.colors[0], "#123456");
    assert_eq!(ramp.colors[255], "#ABCDEF");
}

#[tokio::test]
async fn ramp_rejects_out_of_range_steps() {
    let dir = tempfile::tempdir().expect("tempdir");
    for steps in [0, 1, 257, u32::MAX] {
        let req = PaletteRampRequest { from: "#000000".into(), to: "#FFFFFF".into(), steps };
        assert_bad_param(palette_ramp(dir.path(), req).await, "steps");
    }
}

#[tokio::test]
async fn ramp_rejects_malformed_colors() {
    let dir = tempfile::tempdir().expect("tempdir");
    for bad in ["000000", "#00000", "#0000000", "#00000G", "", "#", "#+12345"] {
        let req = PaletteRampRequest { from: bad.into(), to: "#FFFFFF".into(), steps: 3 };
        assert_bad_param(palette_ramp(dir.path(), req).await, "not #RRGGBB");
    }
}

// ---------------------------------------------------------------------------
// quantize — 2 validation, 2 adversarial
// ---------------------------------------------------------------------------

#[tokio::test]
async fn quantize_reduces_gradient_to_at_most_n_colors() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let mut doc = new_doc(16, 16).expect("new");
    for (x, y, px) in doc.layers[0].image.enumerate_pixels_mut() {
        px.0 = [(x * 16) as u8, (y * 16) as u8, 128, 255];
    }
    save_doc(root, &doc, "g.lumen.json").expect("save");
    let req = QuantizeRequest {
        doc: "g.lumen.json".into(),
        output: Some("q.lumen.json".into()),
        layer: None,
        max_colors: 4,
    };
    quantize(root, req).await.expect("quantize");
    let req = PaletteExtractRequest { doc: "q.lumen.json".into(), layer: None, max_colors: 1024 };
    let got = palette_extract(root, req).await.expect("extract");
    assert!(!got.colors.is_empty() && got.colors.len() <= 4, "got {}", got.colors.len());
    assert_eq!(got.colors.iter().map(|c| c.count).sum::<u64>(), 256);
}

#[tokio::test]
async fn quantize_preserves_alpha_and_transparency() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let mut doc = new_doc(4, 1).expect("new");
    let pixels = [[0, 0, 0, 0], [10, 10, 10, 200], [12, 12, 12, 255], [250, 250, 250, 255]];
    for (x, _, px) in doc.layers[0].image.enumerate_pixels_mut() {
        px.0 = pixels[x as usize];
    }
    save_doc(root, &doc, "a.lumen.json").expect("save");
    let req =
        QuantizeRequest { doc: "a.lumen.json".into(), output: None, layer: Some(0), max_colors: 2 };
    quantize(root, req).await.expect("quantize");
    let doc = load_doc(root, "a.lumen.json").expect("load");
    let img = &doc.layers[0].image;
    assert_eq!(img.get_pixel(0, 0).0, [0, 0, 0, 0]);
    assert_eq!(img.get_pixel(1, 0).0, [11, 11, 11, 200]);
    assert_eq!(img.get_pixel(2, 0).0, [11, 11, 11, 255]);
    assert_eq!(img.get_pixel(3, 0).0, [250, 250, 250, 255]);
}

#[tokio::test]
async fn quantize_one_color_layer_and_transparent_layer_are_noops() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let mut doc = two_tone_doc(root, "a.lumen.json", [33, 66, 99, 255], [33, 66, 99, 255]);
    add_layer(&mut doc, "Empty", [0, 0, 0, 0]);
    save_doc(root, &doc, "a.lumen.json").expect("save");
    let req =
        QuantizeRequest { doc: "a.lumen.json".into(), output: None, layer: None, max_colors: 256 };
    quantize(root, req).await.expect("quantize");
    let doc = load_doc(root, "a.lumen.json").expect("load");
    assert!(doc.layers[0].image.pixels().all(|p| p.0 == [33, 66, 99, 255]));
    assert!(doc.layers[1].image.pixels().all(|p| p.0 == [0, 0, 0, 0]));
}

#[tokio::test]
async fn quantize_rejects_bad_max_colors_and_layer() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    two_tone_doc(root, "a.lumen.json", [1, 2, 3, 255], [4, 5, 6, 255]);
    for max_colors in [0, 1, 257] {
        let req =
            QuantizeRequest { doc: "a.lumen.json".into(), output: None, layer: None, max_colors };
        assert_bad_param(quantize(root, req).await, "max_colors");
    }
    let req =
        QuantizeRequest { doc: "a.lumen.json".into(), output: None, layer: Some(9), max_colors: 2 };
    assert_bad_param(quantize(root, req).await, "out of range");
}
