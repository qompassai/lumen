//! Milestone 2 Phase B interop gate: lumen against REAL external systems.
//!
//! - `*_live_*` tests are `#[ignore]`d: they need `/usr/bin/aseprite` and
//!   the bevy_mcp `example_app` (Bevy 0.18, BRP on 127.0.0.1:15702). Run them
//!   through `scripts/interop_check.sh`, which starts or verifies both first.
//! - Every other test uses only in-process loopback servers and tempdirs, so
//!   it runs in CI.
//!
//! Naming is the classification: `ok_*` validation, `adv_*` adversarial.
//! No test mutates process-global state; project roots are fresh tempdirs.

use std::path::Path;
use std::time::Duration;

use image::RgbaImage;
use lumen::LumenError;
use lumen::bevy::{BevyCallRequest, BevyStatusRequest, bevy_call, bevy_status};
use lumen::doc::load_doc;
use lumen::export::{
    ExportSheetRequest, ExportSpriteRequest, ImportLayerRequest, export_sheet, export_sprite,
    import_layer,
};
use lumen::sprite_ops::{
    self as ops, AddFrameRequest, AddLayerRequest, AddTagRequest, FillRectRequest,
    NewSpriteRequest, SetFrameModRequest, SetPixelRequest,
};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

const ASEPRITE_BIN: &str = "/usr/bin/aseprite";
/// Wall-clock budget for one Aseprite batch invocation.
const ASEPRITE_DEADLINE: Duration = Duration::from_secs(60);
/// The example app's BRP endpoint (bevy_remote's default port).
const LIVE_BRP: &str = "http://127.0.0.1:15702";
/// bevy_remote 0.18 `error_codes`.
const JSONRPC_METHOD_NOT_FOUND: i64 = -32601;
const JSONRPC_INVALID_PARAMS: i64 = -32602;
/// Aseprite file magic: u16 0xA5E0 little-endian at byte offset 4.
const ASEPRITE_MAGIC: [u8; 2] = [0xE0, 0xA5];
/// Aseprite's fixed file-header size, in bytes.
const ASEPRITE_HEADER_BYTES: usize = 128;

const DOC: &str = "s.lumen.json";
const SHEET_COLUMNS: u32 = 3;
const FRAME_DURATIONS_MS: [u32; 4] = [100, 90, 240, 180];
/// (name, from_frame, to_frame), inclusive, 0-based.
const TAGS: [(&str, u32, u32); 2] = [("walk", 0, 1), ("idle", 2, 3)];

/// Builds an `.aseprite` from lumen's per-frame PNGs `f<i>.png`, with lumen's
/// durations and tags. Params: dir, frames, durations ("a,b,.."),
/// tags ("name:from:to;.."), out.
const BUILD_SCRIPT: &str = r#"
local p = app.params
local count = tonumber(p.frames)
local first = Image{ fromFile = p.dir .. "/f0.png" }
local spr = Sprite(first.width, first.height, ColorMode.RGB)
local durations = {}
for d in string.gmatch(p.durations, "%d+") do durations[#durations + 1] = tonumber(d) end
for i = 0, count - 1 do
  if i > 0 then spr:newEmptyFrame(i + 1) end
  local img = Image{ fromFile = p.dir .. "/f" .. i .. ".png" }
  spr:newCel(spr.layers[1], i + 1, img, Point(0, 0))
  spr.frames[i + 1].duration = durations[i + 1] / 1000
end
for name, from, to in string.gmatch(p.tags, "([^:;]+):(%d+):(%d+)") do
  local tag = spr:newTag(tonumber(from) + 1, tonumber(to) + 1)
  tag.name = name
end
spr:saveAs(p.out)
"#;

/// Draws two known pixels into a fresh 8x6 sprite and saves it. Param: out.
const DRAW_SCRIPT: &str = r#"
local p = app.params
local spr = Sprite(8, 6, ColorMode.RGB)
local img = Image(8, 6, ColorMode.RGB)
img:drawPixel(1, 2, app.pixelColor.rgba(10, 200, 30, 255))
img:drawPixel(7, 5, app.pixelColor.rgba(250, 5, 5, 128))
spr:newCel(spr.layers[1], 1, img, Point(0, 0))
spr:saveAs(p.out)
"#;
const DRAWN: [(u32, u32, [u8; 4]); 2] = [(1, 2, [10, 200, 30, 255]), (7, 5, [250, 5, 5, 128])];

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn s(text: &str) -> String {
    text.to_string()
}

/// Run Aseprite in batch mode with a deadline; the child dies with the future.
async fn aseprite(args: &[&str]) -> std::process::Output {
    let child = tokio::process::Command::new(ASEPRITE_BIN)
        .args(args)
        .stdin(std::process::Stdio::null())
        .kill_on_drop(true)
        .output();
    tokio::time::timeout(ASEPRITE_DEADLINE, child)
        .await
        .expect("aseprite exceeded its deadline")
        .expect("spawn aseprite")
}

async fn run_script(root: &Path, script: &str, params: &[(&str, String)]) {
    let script_path = root.join("script.lua");
    std::fs::write(&script_path, script).expect("write lua");
    let mut args: Vec<String> = vec![s("-b")];
    for (key, value) in params {
        args.push(s("--script-param"));
        args.push(format!("{key}={value}"));
    }
    args.push(s("--script"));
    args.push(script_path.display().to_string());
    let argv: Vec<&str> = args.iter().map(String::as_str).collect();
    let out = aseprite(&argv).await;
    assert!(out.status.success(), "aseprite script failed: {out:?}");
}

fn read_png(path: &Path) -> RgbaImage {
    image::open(path).expect("decode png").to_rgba8()
}

fn read_json(path: &Path) -> Value {
    serde_json::from_slice(&std::fs::read(path).expect("read json")).expect("parse json")
}

/// Fully transparent pixels compare equal whatever their RGB bits hold.
fn norm(px: [u8; 4]) -> [u8; 4] {
    if px[3] == 0 { [0, 0, 0, 0] } else { px }
}

#[track_caller]
fn assert_same_pixels(left: &RgbaImage, right: &RgbaImage) {
    assert_eq!(left.dimensions(), right.dimensions(), "dimensions");
    for (x, y, px) in left.enumerate_pixels() {
        let other = right.get_pixel(x, y);
        assert_eq!(norm(px.0), norm(other.0), "pixel ({x},{y})");
    }
}

/// A 2-layer, 4-frame document whose frames all differ, with an alpha-128
/// pixel and two tags. Returns nothing; the doc is `DOC` under `root`.
async fn build_doc(root: &Path) {
    let new = NewSpriteRequest {
        width: 16,
        height: 12,
        background: s("#00000000"),
        output: s(DOC),
    };
    ops::new_sprite(root, new).await.expect("new_sprite");
    let rect = FillRectRequest {
        doc: s(DOC),
        output: None,
        layer: 0,
        x: 2,
        y: 2,
        w: 5,
        h: 4,
        color: s("#d02020ff"),
    };
    ops::fill_rect(root, rect).await.expect("fill_rect");
    let pixel = SetPixelRequest {
        doc: s(DOC),
        output: None,
        layer: 0,
        x: 0,
        y: 0,
        color: s("#2040ff80"),
    };
    ops::set_pixel(root, pixel).await.expect("set_pixel");
    let layer = AddLayerRequest {
        doc: s(DOC),
        output: None,
        name: s("fx"),
        index: None,
        blend: None,
        opacity: None,
        fill: None,
    };
    ops::add_layer(root, layer).await.expect("add_layer");
    let fx = FillRectRequest {
        doc: s(DOC),
        output: None,
        layer: 1,
        x: 9,
        y: 6,
        w: 3,
        h: 3,
        color: s("#20e040ff"),
    };
    ops::fill_rect(root, fx).await.expect("fill fx");
    build_frames(root).await;
}

async fn build_frames(root: &Path) {
    for &duration_ms in &FRAME_DURATIONS_MS[1..] {
        let frame = AddFrameRequest {
            doc: s(DOC),
            output: None,
            duration_ms,
            at: None,
        };
        ops::add_frame(root, frame).await.expect("add_frame");
    }
    let mods = [
        (1, 0, Some(3), None, None),
        (2, 1, None, Some(-2), None),
        (3, 1, None, None, Some(0.5)),
    ];
    for (frame, layer, offset_x, offset_y, opacity_mult) in mods {
        let req = SetFrameModRequest {
            doc: s(DOC),
            output: None,
            frame,
            layer,
            offset_x,
            offset_y,
            opacity_mult,
            scale: None,
        };
        ops::set_frame_mod(root, req).await.expect("set_frame_mod");
    }
    for (name, from_frame, to_frame) in TAGS {
        let tag = AddTagRequest {
            doc: s(DOC),
            output: None,
            name: s(name),
            from_frame,
            to_frame,
        };
        ops::add_tag(root, tag).await.expect("add_tag");
    }
}

/// Export lumen's sheet + sidecar and one PNG per frame (`f<i>.png`).
async fn export_all(root: &Path) {
    let sheet = ExportSheetRequest {
        doc: s(DOC),
        output: s("lumen_sheet.png"),
        meta_output: s("lumen_sheet.json"),
        columns: SHEET_COLUMNS,
        tag: None,
        padding: 0,
    };
    export_sheet(root, sheet).await.expect("export_sheet");
    for frame in 0..FRAME_DURATIONS_MS.len() {
        let req = ExportSpriteRequest {
            doc: s(DOC),
            output: format!("f{frame}.png"),
            frame: Some(frame),
        };
        export_sprite(root, req).await.expect("export_sprite");
    }
}

/// Serve exactly one HTTP request on an ephemeral loopback port. Yields the
/// raw request it saw (bounded: 64 KiB, 64 reads).
async fn serve_once(
    content_type: &'static str,
    body: String,
) -> (String, tokio::task::JoinHandle<String>) {
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
            let text = String::from_utf8_lossy(&raw);
            if n == 0 || raw.len() > 64 * 1024 || text.trim_end().ends_with('}') {
                break;
            }
        }
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: {content_type}\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        sock.write_all(resp.as_bytes()).await.expect("write");
        String::from_utf8_lossy(&raw).into_owned()
    });
    (format!("http://127.0.0.1:{port}"), handle)
}

fn status_req(endpoint: Option<&str>) -> BevyStatusRequest {
    BevyStatusRequest {
        endpoint: endpoint.map(str::to_string),
    }
}

fn call_req(endpoint: &str, method: &str, params: Option<Value>) -> BevyCallRequest {
    BevyCallRequest {
        endpoint: Some(s(endpoint)),
        method: s(method),
        params,
    }
}

fn error_code(error: &Option<Value>) -> Option<i64> {
    error.as_ref()?.get("code")?.as_i64()
}

// ---------------------------------------------------------------------------
// Offline (CI) — validation
// ---------------------------------------------------------------------------

/// The precondition of the Aseprite comparison: lumen's own sheet is exactly
/// its per-frame exports laid out at the sidecar's coordinates.
#[tokio::test]
async fn ok_lumen_sheet_cells_equal_per_frame_exports() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    build_doc(root).await;
    export_all(root).await;
    let sheet = read_png(&root.join("lumen_sheet.png"));
    let meta = read_json(&root.join("lumen_sheet.json"));
    assert_eq!(sheet.dimensions(), (16 * SHEET_COLUMNS, 12 * 2));
    let cells = meta["frames"].as_array().expect("frames");
    assert_eq!(cells.len(), FRAME_DURATIONS_MS.len());
    for (frame, cell) in cells.iter().enumerate() {
        let frame_png = read_png(&root.join(format!("f{frame}.png")));
        let x = u32::try_from(cell["x"].as_u64().expect("x")).expect("x fits");
        let y = u32::try_from(cell["y"].as_u64().expect("y")).expect("y fits");
        let view = image::imageops::crop_imm(&sheet, x, y, 16, 12).to_image();
        assert_same_pixels(&view, &frame_png);
        assert_eq!(cell["duration_ms"], FRAME_DURATIONS_MS[frame]);
    }
    // Frames really differ, so a frame-order bug cannot pass unnoticed.
    assert_ne!(
        read_png(&root.join("f0.png")),
        read_png(&root.join("f1.png"))
    );
}

/// A server shaped like Bevy 0.18's `rpc.discover` reply is confirmed, and
/// the probe really is `rpc.discover` (Bevy 0.18 has no `bevy/list`).
#[tokio::test]
async fn ok_status_confirms_bevy_shaped_openrpc_document() {
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "result": {
            "openrpc": "1.3.2",
            "info": {"title": "Bevy Remote Protocol", "version": "0.18.0"},
            "methods": [{"name": "world.query"}, {"name": "registry.schema"},
                        {"name": "rpc.discover"}],
        },
    });
    let (endpoint, server) = serve_once("application/json", body.to_string()).await;
    let status = bevy_status(status_req(Some(&endpoint)))
        .await
        .expect("status");
    assert!(status.reachable && status.speaks_brp, "{}", status.detail);
    assert!(
        status.detail.contains("listed 3 methods"),
        "{}",
        status.detail
    );
    let seen = server.await.expect("server task");
    assert!(seen.contains("\"method\":\"rpc.discover\""), "{seen}");
}

// ---------------------------------------------------------------------------
// Offline (CI) — adversarial
// ---------------------------------------------------------------------------

/// Regression for the live finding: a generic JSON-RPC server (not Bevy)
/// answers any unknown method with -32601. That must NOT read as BRP.
#[tokio::test]
async fn adv_status_foreign_jsonrpc_server_is_not_brp() {
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "error": {"code": JSONRPC_METHOD_NOT_FOUND, "message": "x".repeat(4096)},
    });
    let (endpoint, server) = serve_once("application/json", body.to_string()).await;
    let status = bevy_status(status_req(Some(&endpoint)))
        .await
        .expect("status");
    assert!(status.reachable);
    assert!(!status.speaks_brp, "{}", status.detail);
    assert!(status.detail.contains("-32601"), "{}", status.detail);
    // The server-supplied message is never echoed back.
    assert!(status.detail.len() < 256, "detail must stay bounded");
    server.await.expect("server task");
    // A non-integer code is not JSON-RPC, and is not echoed either.
    let body = json!({"jsonrpc": "2.0", "id": 1, "error": {"code": "y".repeat(4096)}});
    let (endpoint, server) = serve_once("application/json", body.to_string()).await;
    let status = bevy_status(status_req(Some(&endpoint)))
        .await
        .expect("status");
    assert!(status.reachable && !status.speaks_brp, "{}", status.detail);
    assert!(status.detail.len() < 256, "detail must stay bounded");
    server.await.expect("server task");
}

/// A foreign plain HTTP server squatting on the port is reachable but not BRP.
#[tokio::test]
async fn adv_status_foreign_html_server_is_not_brp() {
    let body = s("<!doctype html><html><body>{not json}</body></html>");
    let (endpoint, server) = serve_once("text/html", body).await;
    let status = bevy_status(status_req(Some(&endpoint)))
        .await
        .expect("status");
    assert!(status.reachable);
    assert!(!status.speaks_brp, "{}", status.detail);
    assert!(status.detail.contains("non-JSON-RPC"), "{}", status.detail);
    server.await.expect("server task");
}

/// Without `LUMEN_ALLOW_REMOTE_BRP=1`, every non-loopback (or loopback-
/// lookalike) target is refused before any socket opens.
#[tokio::test]
async fn adv_remote_brp_refused_without_opt_in() {
    assert!(
        std::env::var("LUMEN_ALLOW_REMOTE_BRP").is_err(),
        "unset LUMEN_ALLOW_REMOTE_BRP before running this gate"
    );
    let hostile = [
        "http://example.com:15702",
        "http://10.0.0.5:15702",
        "http://0.0.0.0:15702",
        "http://127.0.0.1.nip.io:15702",
        "http://localhost.attacker.test:15702",
        "http://[::ffff:7f00:1]:15702",
    ];
    for endpoint in hostile {
        let err = bevy_status(status_req(Some(endpoint))).await.unwrap_err();
        assert!(
            err.to_string().contains("non-loopback"),
            "{endpoint}: {err}"
        );
        let err = bevy_call(call_req(endpoint, "world.query", None))
            .await
            .unwrap_err();
        assert!(
            err.to_string().contains("non-loopback"),
            "{endpoint}: {err}"
        );
    }
    let err = bevy_status(status_req(Some("https://127.0.0.1:15702")))
        .await
        .unwrap_err();
    assert!(matches!(err, LumenError::Brp(_)), "{err}");
}

/// lumen has no `.aseprite` reader: a truncated Aseprite file is refused by
/// every lumen loader with a typed error, whatever name it hides behind.
#[tokio::test]
async fn adv_truncated_aseprite_bytes_refused_by_lumen_loaders() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let mut bytes = vec![0u8; ASEPRITE_HEADER_BYTES / 2];
    bytes[4..6].copy_from_slice(&ASEPRITE_MAGIC);
    for name in ["bad.aseprite", "bad.png", "bad.lumen.json"] {
        std::fs::write(root.join(name), &bytes).expect("write");
    }
    let new = NewSpriteRequest {
        width: 8,
        height: 6,
        background: s("#00000000"),
        output: s(DOC),
    };
    ops::new_sprite(root, new).await.expect("new_sprite");
    let before = std::fs::read(root.join(DOC)).expect("doc bytes");
    for (png, expect_png_error) in [("bad.aseprite", false), ("bad.png", true)] {
        let req = ImportLayerRequest {
            doc: s(DOC),
            output: None,
            png: s(png),
            name: Some(s("x")),
        };
        let err = import_layer(root, req).await.unwrap_err();
        match (expect_png_error, &err) {
            (false, LumenError::PathRejected(_)) | (true, LumenError::BadPng(_)) => {}
            _ => panic!("{png}: unexpected error {err:?}"),
        }
    }
    assert!(matches!(
        load_doc(root, "bad.lumen.json"),
        Err(LumenError::DocInvalid(_))
    ));
    assert_eq!(
        std::fs::read(root.join(DOC)).expect("doc bytes"),
        before,
        "doc untouched"
    );
}

// ---------------------------------------------------------------------------
// Live Aseprite (ignored; scripts/interop_check.sh)
// ---------------------------------------------------------------------------

/// lumen doc -> lumen frame PNGs -> real Aseprite `.aseprite` (frames,
/// durations, tags) -> real Aseprite sheet export. Aseprite's sheet must
/// match lumen's sheet pixel for pixel, and its JSON must match lumen's
/// sidecar: cell rects, durations, and tags.
#[tokio::test]
#[ignore = "needs /usr/bin/aseprite; run scripts/interop_check.sh"]
async fn ok_live_aseprite_roundtrip_matches_lumen_sheet() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    build_doc(root).await;
    export_all(root).await;
    let durations: Vec<String> = FRAME_DURATIONS_MS.iter().map(u32::to_string).collect();
    let tags: Vec<String> = TAGS
        .iter()
        .map(|(n, f, t)| format!("{n}:{f}:{t}"))
        .collect();
    let ase = root.join("roundtrip.aseprite");
    let params = [
        ("dir", root.display().to_string()),
        ("frames", FRAME_DURATIONS_MS.len().to_string()),
        ("durations", durations.join(",")),
        ("tags", tags.join(";")),
        ("out", ase.display().to_string()),
    ];
    run_script(root, BUILD_SCRIPT, &params).await;
    let header = std::fs::read(&ase).expect("aseprite wrote the file");
    assert_eq!(header[4..6], ASEPRITE_MAGIC, "real .aseprite magic");
    let (sheet, data) = (root.join("ase_sheet.png"), root.join("ase_sheet.json"));
    let columns = SHEET_COLUMNS.to_string();
    let out = aseprite(&[
        "-b",
        &ase.display().to_string(),
        "--sheet",
        &sheet.display().to_string(),
        "--sheet-type",
        "rows",
        "--sheet-columns",
        &columns,
        "--format",
        "json-array",
        "--list-tags",
        "--data",
        &data.display().to_string(),
    ])
    .await;
    assert!(out.status.success(), "sheet export failed: {out:?}");
    assert_same_pixels(&read_png(&sheet), &read_png(&root.join("lumen_sheet.png")));
    assert_sheet_data_matches(
        &read_json(&data),
        &read_json(&root.join("lumen_sheet.json")),
    );
}

#[track_caller]
fn assert_sheet_data_matches(ase: &Value, lumen: &Value) {
    let ase_frames = ase["frames"].as_array().expect("aseprite frames");
    let lumen_cells = lumen["frames"].as_array().expect("lumen cells");
    assert_eq!(ase_frames.len(), lumen_cells.len(), "frame count");
    for (ase_frame, cell) in ase_frames.iter().zip(lumen_cells) {
        let rect = &ase_frame["frame"];
        assert_eq!(rect["x"], cell["x"], "{ase_frame}");
        assert_eq!(rect["y"], cell["y"], "{ase_frame}");
        assert_eq!(rect["w"], lumen["cell_w"], "{ase_frame}");
        assert_eq!(rect["h"], lumen["cell_h"], "{ase_frame}");
        assert_eq!(ase_frame["duration"], cell["duration_ms"], "{ase_frame}");
    }
    let ase_tags = ase["meta"]["frameTags"].as_array().expect("frameTags");
    assert_eq!(ase_tags.len(), TAGS.len());
    for (tag, (name, from, to)) in ase_tags.iter().zip(TAGS) {
        assert_eq!(tag["name"], name);
        assert_eq!(tag["from"], from);
        assert_eq!(tag["to"], to);
    }
}

/// Aseprite draws -> Aseprite writes PNG -> lumen imports it as a layer and
/// re-exports: lumen sees exactly the pixels Aseprite drew (incl. alpha 128).
#[tokio::test]
#[ignore = "needs /usr/bin/aseprite; run scripts/interop_check.sh"]
async fn ok_live_aseprite_drawn_png_imports_into_lumen() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let ase = root.join("drawn.aseprite");
    run_script(root, DRAW_SCRIPT, &[("out", ase.display().to_string())]).await;
    let png = root.join("drawn.png");
    let ase_arg = ase.display().to_string();
    let png_arg = png.display().to_string();
    let out = aseprite(&["-b", &ase_arg, "--save-as", &png_arg]).await;
    assert!(out.status.success(), "save-as failed: {out:?}");
    let new = NewSpriteRequest {
        width: 8,
        height: 6,
        background: s("#00000000"),
        output: s(DOC),
    };
    ops::new_sprite(root, new).await.expect("new_sprite");
    let import = ImportLayerRequest {
        doc: s(DOC),
        output: None,
        png: s("drawn.png"),
        name: Some(s("aseprite")),
    };
    import_layer(root, import).await.expect("import_layer");
    let export = ExportSpriteRequest {
        doc: s(DOC),
        output: s("out.png"),
        frame: None,
    };
    export_sprite(root, export).await.expect("export_sprite");
    let img = read_png(&root.join("out.png"));
    assert_eq!(img.dimensions(), (8, 6));
    for (x, y, px) in img.enumerate_pixels() {
        let want = DRAWN
            .iter()
            .find(|(dx, dy, _)| (*dx, *dy) == (x, y))
            .map_or([0, 0, 0, 0], |d| d.2);
        assert_eq!(norm(px.0), want, "pixel ({x},{y})");
    }
}

/// A real `.aseprite` truncated inside its 128-byte header: Aseprite
/// RECOVERS it (writes a sheet from the partial data) instead of rejecting
/// it — that is Aseprite 1.3.18.6-dev's real behavior, not a lumen bug.
/// Lumen must still refuse it outright: `import_layer` takes a PNG, so a
/// truncated `.aseprite` is rejected (`PathRejected`: only .png sprites
/// are accepted), never silently accepted.
#[tokio::test]
#[ignore = "needs /usr/bin/aseprite; run scripts/interop_check.sh"]
async fn adv_live_aseprite_truncated_file_is_recovered_but_lumen_refuses_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let ase = root.join("t.aseprite");
    run_script(root, DRAW_SCRIPT, &[("out", ase.display().to_string())]).await;
    let full = std::fs::read(&ase).expect("aseprite wrote the file");
    assert!(
        full.len() > ASEPRITE_HEADER_BYTES,
        "real file is longer than its header"
    );
    std::fs::write(&ase, &full[..ASEPRITE_HEADER_BYTES / 2]).expect("truncate");
    let sheet = root.join("t_sheet.png");
    let ase_arg = ase.display().to_string();
    let sheet_arg = sheet.display().to_string();
    let out = aseprite(&["-b", &ase_arg, "--sheet", &sheet_arg]).await;
    // Aseprite recovers truncated files instead of rejecting them: it exits
    // 0 and writes a sheet from whatever it could parse. Document, don't
    // fight: the contract is that LUMEN refuses, not that Aseprite does.
    assert!(
        out.status.success(),
        "aseprite unexpectedly failed on a truncated file: {out:?}"
    );
    let new = NewSpriteRequest {
        width: 8,
        height: 6,
        background: s("#00000000"),
        output: s(DOC),
    };
    ops::new_sprite(root, new).await.expect("new_sprite");
    let req = ImportLayerRequest {
        doc: s(DOC),
        output: None,
        png: s("t.aseprite"),
        name: None,
    };
    let err = import_layer(root, req).await.unwrap_err();
    assert!(
        matches!(err, LumenError::PathRejected(_)),
        "lumen must refuse a non-PNG sprite path, got: {err:?}"
    );
}

// ---------------------------------------------------------------------------
// Live BRP against bevy_mcp example_app (ignored; scripts/interop_check.sh)
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "needs bevy_mcp example_app on 127.0.0.1:15702; run scripts/interop_check.sh"]
async fn ok_live_bevy_status_confirms_example_app() {
    // `None` exercises lumen's default endpoint, which must be the app's.
    let status = bevy_status(status_req(None)).await.expect("status");
    assert_eq!(status.endpoint, "http://127.0.0.1:15702");
    assert!(status.reachable && status.speaks_brp, "{}", status.detail);
    assert!(
        status.detail.starts_with("BRP confirmed"),
        "{}",
        status.detail
    );
    let discover = bevy_call(call_req(LIVE_BRP, "rpc.discover", None))
        .await
        .expect("call");
    let methods = discover.result.as_ref().expect("result")["methods"]
        .as_array()
        .expect("methods array")
        .iter()
        .filter_map(|m| m["name"].as_str())
        .collect::<Vec<_>>();
    for method in ["world.query", "registry.schema", "rpc.discover"] {
        assert!(
            methods.contains(&method),
            "{method} missing from {methods:?}"
        );
    }
    assert!(
        !methods.contains(&"bevy/list"),
        "Bevy 0.18 should not serve bevy/list"
    );
}

/// `world.query` for `Name` returns the example app's five named cubes.
#[tokio::test]
#[ignore = "needs bevy_mcp example_app on 127.0.0.1:15702; run scripts/interop_check.sh"]
async fn ok_live_bevy_query_returns_named_cubes() {
    let name_path = "bevy_ecs::name::Name";
    let params = json!({"data": {"components": [name_path]}});
    let out = bevy_call(call_req(LIVE_BRP, "world.query", Some(params)))
        .await
        .expect("call");
    assert!(out.ok && out.error.is_none(), "{out:?}");
    assert_eq!(out.http_status, 200);
    let rows = out
        .result
        .as_ref()
        .and_then(Value::as_array)
        .expect("rows array");
    let mut cubes = Vec::new();
    for row in rows {
        assert!(row["entity"].is_u64(), "entity id is a u64: {row}");
        let name = row["components"][name_path].to_string();
        if name.contains("Cube ") {
            cubes.push(name.trim_matches('"').to_string());
        }
    }
    cubes.sort();
    assert_eq!(cubes, ["Cube A", "Cube B", "Cube C", "Cube D", "Cube E"]);
}

/// `registry.schema` (filtered to one crate to bound the reply) describes
/// the `Name` component the query above relied on.
///
/// Bevy 0.18's `with_crates` is NOT a strict prefix filter: it only applies
/// to types with a known crate name, so primitives, tuples, and composite
/// types without one pass through unfiltered. The contract is that
/// `bevy_ecs::name::Name` is present, not that every key starts with
/// `bevy_ecs::`.
#[tokio::test]
#[ignore = "needs bevy_mcp example_app on 127.0.0.1:15702; run scripts/interop_check.sh"]
async fn ok_live_bevy_registry_schema_lists_name() {
    let params = json!({"with_crates": ["bevy_ecs"]});
    let out = bevy_call(call_req(LIVE_BRP, "registry.schema", Some(params)))
        .await
        .expect("call");
    assert!(out.ok, "{out:?}");
    let schema = out
        .result
        .as_ref()
        .and_then(Value::as_object)
        .expect("schema map");
    assert!(
        schema.contains_key("bevy_ecs::name::Name"),
        "{:?}",
        schema.keys()
    );
    // Bevy's filter lets crate-less types (primitives, tuples) through;
    // assert presence of the wanted type, not absence of everything else.
    assert!(
        schema.keys().any(|k| k.starts_with("bevy_ecs::")),
        "expected bevy_ecs types in filtered schema, got: {:?}",
        schema.keys().take(5).collect::<Vec<_>>()
    );
}

#[tokio::test]
#[ignore = "needs bevy_mcp example_app on 127.0.0.1:15702; run scripts/interop_check.sh"]
async fn adv_live_bevy_unknown_and_legacy_methods_are_jsonrpc_errors() {
    for method in ["lumen.no_such_method", "bevy/list"] {
        let out = bevy_call(call_req(LIVE_BRP, method, None))
            .await
            .expect("Ok, not Err");
        assert!(!out.ok && out.result.is_none(), "{method}: {out:?}");
        assert_eq!(
            error_code(&out.error),
            Some(JSONRPC_METHOD_NOT_FOUND),
            "{method}"
        );
    }
}

/// Malformed `world.query` params: Bevy 0.18 is lenient where JSON-RPC
/// permits it and strict where it must be.
///
/// - `null` params are VALID JSON-RPC (params are optional): Bevy treats them
///   as "no filter" and returns all entities with HTTP 200. That is correct
///   behavior, not an error.
/// - Structurally invalid params (`data` not an object, unknown component
///   with `strict: true`) are JSON-RPC errors.
#[tokio::test]
#[ignore = "needs bevy_mcp example_app on 127.0.0.1:15702; run scripts/interop_check.sh"]
async fn adv_live_bevy_malformed_params_are_jsonrpc_errors() {
    // Null params = omitted params: Bevy returns everything, HTTP 200.
    // Lumen relays this faithfully; it is not a lumen bug.
    let out = bevy_call(call_req(LIVE_BRP, "world.query", Some(Value::Null)))
        .await
        .expect("Ok, not Err");
    assert!(out.ok && out.error.is_none(), "null params: {out:?}");
    assert_eq!(out.http_status, 200);
    assert!(
        out.result.as_ref().and_then(Value::as_array).is_some(),
        "null params return the full entity list: {out:?}"
    );

    // Structurally invalid params are real errors.
    let bad = [
        json!({"data": "not an object"}),
        json!({"data": {"components": ["no::such::Component"]}, "strict": true}),
    ];
    for params in bad {
        let out = bevy_call(call_req(LIVE_BRP, "world.query", Some(params.clone())))
            .await
            .expect("Ok, not Err");
        assert!(!out.ok, "{params}: {out:?}");
        assert!(error_code(&out.error).is_some(), "{params}: {out:?}");
    }
    let out = bevy_call(call_req(LIVE_BRP, "world.query", Some(json!({"nope": 1}))))
        .await
        .expect("Ok, not Err");
    assert_eq!(
        error_code(&out.error),
        Some(JSONRPC_INVALID_PARAMS),
        "{out:?}"
    );
}
