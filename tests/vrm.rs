//! VRM wiring gate tests (`src/vrm.rs`): VRoid Studio's export format at
//! the file layer. Fixtures are synthetic minimal GLBs built in-process
//! (no external models, no licenses to audit): a JSON chunk carrying the
//! VRM extension plus one embedded PNG texture in the binary chunk.
//!
//! Roughly 50/50 validation/adversarial. Validation proves inspect and
//! grade do their jobs and that grading preserves everything except the
//! replaced textures; adversarial proves malformed, non-VRM, external,
//! and hostile inputs fail closed with nothing written.

use std::io::{BufRead, BufReader, Cursor, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use image::{ExtendedColorType, ImageEncoder, ImageFormat, Rgba, RgbaImage};
use lumen::LumenError;
use lumen::vrm::{VrmGradeReq, VrmInspectReq, vrm_grade, vrm_inspect};
use serde_json::{Value, json};

// ---------------------------------------------------------------------------
// Fixture construction
// ---------------------------------------------------------------------------

/// The fixture texture: 2x1, pixels chosen so demo_toon's graded values
/// are pinned exactly by the style tests ((200,40,90) and mid-gray).
fn texture_png() -> Vec<u8> {
    let img = RgbaImage::from_raw(2, 1, vec![200, 40, 90, 255, 128, 128, 128, 255])
        .expect("fixture image");
    let mut cursor = Cursor::new(Vec::new());
    let encoder = image::codecs::png::PngEncoder::new(&mut cursor);
    encoder
        .write_image(
            img.as_raw(),
            img.width(),
            img.height(),
            ExtendedColorType::Rgba8,
        )
        .expect("encode fixture png");
    cursor.into_inner()
}

fn glb_bytes(document: &Value, bin: &[u8]) -> Vec<u8> {
    let json_bytes = serde_json::to_vec(document).expect("fixture json");
    let json_pad = (4 - json_bytes.len() % 4) % 4;
    let bin_pad = (4 - bin.len() % 4) % 4;
    let total = 12 + 8 + json_bytes.len() + json_pad + 8 + bin.len() + bin_pad;
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&((json_bytes.len() + json_pad) as u32).to_le_bytes());
    out.extend_from_slice(&0x4E4F_534Au32.to_le_bytes());
    out.extend_from_slice(&json_bytes);
    out.extend(std::iter::repeat_n(b' ', json_pad));
    out.extend_from_slice(&((bin.len() + bin_pad) as u32).to_le_bytes());
    out.extend_from_slice(&0x004E_4942u32.to_le_bytes());
    out.extend_from_slice(bin);
    out.extend(std::iter::repeat_n(0u8, bin_pad));
    out
}

fn fixture_document(extensions: Value, png_len: usize) -> Value {
    json!({
        "asset": {"version": "2.0", "generator": "lumen test fixture"},
        "extensions": extensions,
        "scene": 0,
        "scenes": [{"nodes": [0]}],
        "nodes": [{"name": "Root"}],
        "meshes": [],
        "images": [{"bufferView": 0, "mimeType": "image/png"}],
        "bufferViews": [{"buffer": 0, "byteOffset": 0, "byteLength": png_len}],
        "buffers": [{"byteLength": png_len}],
    })
}

fn vrm_1_0() -> Vec<u8> {
    let png = texture_png();
    let doc = fixture_document(
        json!({"VRMC_vrm": {
            "specVersion": "1.0",
            "meta": {"name": "Fixture Fox", "authors": ["lumen tests"]},
            "humanoid": {},
        }}),
        png.len(),
    );
    glb_bytes(&doc, &png)
}

fn vrm_0_x() -> Vec<u8> {
    let png = texture_png();
    let doc = fixture_document(
        json!({"VRM": {
            "exporterVersion": "UniVRM-0.128.0",
            "meta": {"title": "Legacy Fox", "author": "lumen tests"},
        }}),
        png.len(),
    );
    glb_bytes(&doc, &png)
}

fn write_fixture(root: &std::path::Path, name: &str, bytes: &[u8]) {
    std::fs::write(root.join(name), bytes).expect("write fixture");
}

/// Minimal GLB reader for assertions: (parsed JSON, binary chunk).
fn read_glb(bytes: &[u8]) -> (Value, Vec<u8>) {
    let json_len = u32::from_le_bytes(bytes[12..16].try_into().expect("len")) as usize;
    let doc: Value = serde_json::from_slice(&bytes[20..20 + json_len]).expect("json chunk");
    let bin_off = 20 + json_len + 8;
    let bin_len = u32::from_le_bytes(
        bytes[20 + json_len..20 + json_len + 4]
            .try_into()
            .expect("len"),
    ) as usize;
    (doc, bytes[bin_off..bin_off + bin_len].to_vec())
}

fn graded_texture(doc: &Value, bin: &[u8]) -> RgbaImage {
    let view_index = doc["images"][0]["bufferView"].as_u64().expect("view") as usize;
    let view = &doc["bufferViews"][view_index];
    let off = view["byteOffset"].as_u64().expect("offset") as usize;
    let len = view["byteLength"].as_u64().expect("length") as usize;
    image::load_from_memory_with_format(&bin[off..off + len], ImageFormat::Png)
        .expect("graded texture decodes")
        .to_rgba8()
}

fn inspect_req(vrm: &str) -> VrmInspectReq {
    VrmInspectReq {
        vrm: vrm.to_string(),
    }
}

fn grade_req(vrm: &str, output: Option<&str>, style: &str, strength: f32) -> VrmGradeReq {
    VrmGradeReq {
        vrm: vrm.to_string(),
        output: output.map(str::to_string),
        style: style.to_string(),
        strength,
    }
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

#[tokio::test]
async fn vrm_inspect_reports_1_0_fixture() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bytes = vrm_1_0();
    write_fixture(dir.path(), "fox.vrm", &bytes);
    let info = vrm_inspect(dir.path(), inspect_req("fox.vrm"))
        .await
        .expect("inspect");
    assert_eq!(info.flavor, "vrm-1.0");
    assert_eq!(info.spec_version.as_deref(), Some("1.0"));
    assert_eq!(info.name.as_deref(), Some("Fixture Fox"));
    assert_eq!(info.author.as_deref(), Some("lumen tests"));
    assert_eq!(info.node_count, 1);
    assert_eq!(info.mesh_count, 0);
    assert_eq!(info.byte_len, bytes.len() as u64);
    assert_eq!(info.textures.len(), 1);
    let tex = &info.textures[0];
    assert_eq!(tex.mime_type, "image/png");
    assert_eq!((tex.width, tex.height), (Some(2), Some(1)));
    assert_eq!(tex.byte_length, texture_png().len());
}

#[tokio::test]
async fn vrm_inspect_reports_0_x_fixture() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_fixture(dir.path(), "legacy.vrm", &vrm_0_x());
    let info = vrm_inspect(dir.path(), inspect_req("legacy.vrm"))
        .await
        .expect("inspect");
    assert_eq!(info.flavor, "vrm-0.x");
    assert_eq!(info.spec_version.as_deref(), Some("UniVRM-0.128.0"));
    assert_eq!(info.name.as_deref(), Some("Legacy Fox"));
    assert_eq!(info.author.as_deref(), Some("lumen tests"));
}

#[tokio::test]
async fn vrm_grade_demo_toon_rewrites_texture_and_preserves_meta() {
    let dir = tempfile::tempdir().expect("tempdir");
    let input = vrm_1_0();
    write_fixture(dir.path(), "fox.vrm", &input);
    let graded = vrm_grade(
        dir.path(),
        grade_req("fox.vrm", Some("fox_graded.vrm"), "demo_toon", 1.0),
    )
    .await
    .expect("grade");
    assert_eq!(graded.textures_graded, 1);
    let out_bytes = std::fs::read(dir.path().join("fox_graded.vrm")).expect("read output");
    assert_eq!(graded.bytes_written, out_bytes.len() as u64);
    let (in_doc, _) = read_glb(&input);
    let (out_doc, out_bin) = read_glb(&out_bytes);
    // The VRM extension subtree and every untouched key are preserved.
    assert_eq!(out_doc["extensions"], in_doc["extensions"]);
    assert_eq!(out_doc["nodes"], in_doc["nodes"]);
    assert_eq!(out_doc["scenes"], in_doc["scenes"]);
    assert_eq!(out_doc["asset"], in_doc["asset"]);
    // The image was repointed at an appended bufferView; the texture
    // pixels carry exactly demo_toon's pinned values.
    assert_eq!(out_doc["images"][0]["bufferView"], json!(1));
    let img = graded_texture(&out_doc, &out_bin);
    assert_eq!(img.get_pixel(0, 0), &Rgba([180, 24, 62, 255]));
    assert_eq!(img.get_pixel(1, 0), &Rgba([153, 153, 153, 255]));
    // And the output still inspects as the same avatar.
    let info = vrm_inspect(dir.path(), inspect_req("fox_graded.vrm"))
        .await
        .expect("re-inspect");
    assert_eq!(info.name.as_deref(), Some("Fixture Fox"));
}

#[tokio::test]
async fn vrm_grade_strength_zero_is_byte_identical() {
    let dir = tempfile::tempdir().expect("tempdir");
    let input = vrm_1_0();
    write_fixture(dir.path(), "fox.vrm", &input);
    let graded = vrm_grade(
        dir.path(),
        grade_req("fox.vrm", Some("fox_copy.vrm"), "demo_toon", 0.0),
    )
    .await
    .expect("grade");
    assert_eq!(graded.textures_graded, 0);
    let out_bytes = std::fs::read(dir.path().join("fox_copy.vrm")).expect("read output");
    assert_eq!(
        out_bytes, input,
        "strength 0 must pass the file through byte for byte"
    );
}

#[tokio::test]
async fn vrm_grade_in_place_overwrites_input() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_fixture(dir.path(), "fox.vrm", &vrm_1_0());
    let graded = vrm_grade(dir.path(), grade_req("fox.vrm", None, "demo_toon", 1.0))
        .await
        .expect("grade in place");
    assert_eq!(graded.path, "fox.vrm");
    let info = vrm_inspect(dir.path(), inspect_req("fox.vrm"))
        .await
        .expect("inspect after in-place grade");
    assert_eq!(info.flavor, "vrm-1.0");
}

// ---------------------------------------------------------------------------
// Adversarial
// ---------------------------------------------------------------------------

#[tokio::test]
async fn vrm_inspect_rejects_non_vrm_gltf() {
    let dir = tempfile::tempdir().expect("tempdir");
    let png = texture_png();
    let doc = fixture_document(json!({}), png.len());
    write_fixture(dir.path(), "plain.vrm", &glb_bytes(&doc, &png));
    let err = vrm_inspect(dir.path(), inspect_req("plain.vrm"))
        .await
        .expect_err("a glTF without a VRM extension must fail");
    assert!(matches!(err, LumenError::DocInvalid(_)), "got {err:?}");
    assert!(err.to_string().contains("not a VRM"), "got {err}");
}

#[tokio::test]
async fn vrm_inspect_rejects_bad_magic_and_truncation() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut bad = vrm_1_0();
    bad[0] = b'X';
    write_fixture(dir.path(), "bad.vrm", &bad);
    let err = vrm_inspect(dir.path(), inspect_req("bad.vrm"))
        .await
        .expect_err("bad magic must fail");
    assert!(matches!(err, LumenError::DocInvalid(_)), "got {err:?}");
    let full = vrm_1_0();
    write_fixture(dir.path(), "cut.vrm", &full[..full.len() / 2]);
    let err = vrm_inspect(dir.path(), inspect_req("cut.vrm"))
        .await
        .expect_err("truncated file must fail");
    assert!(matches!(err, LumenError::DocInvalid(_)), "got {err:?}");
}

#[tokio::test]
async fn vrm_grade_truncated_writes_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let full = vrm_1_0();
    write_fixture(dir.path(), "cut.vrm", &full[..full.len() / 2]);
    let err = vrm_grade(
        dir.path(),
        grade_req("cut.vrm", Some("out.vrm"), "demo_toon", 1.0),
    )
    .await
    .expect_err("truncated input must fail");
    assert!(matches!(err, LumenError::DocInvalid(_)), "got {err:?}");
    assert!(!dir.path().join("out.vrm").exists(), "no partial output");
}

#[tokio::test]
async fn vrm_grade_jpeg_texture_is_rejected_but_inspects() {
    let dir = tempfile::tempdir().expect("tempdir");
    let fake_jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 1, 2, 3, 4];
    let mut doc = fixture_document(
        json!({"VRMC_vrm": {"specVersion": "1.0", "meta": {"name": "Jpeg Fox"}}}),
        fake_jpeg.len(),
    );
    doc["images"][0]["mimeType"] = json!("image/jpeg");
    write_fixture(dir.path(), "jpeg.vrm", &glb_bytes(&doc, &fake_jpeg));
    // Inspect reports the structure honestly (dims undecodable here).
    let info = vrm_inspect(dir.path(), inspect_req("jpeg.vrm"))
        .await
        .expect("inspect");
    assert_eq!(info.textures[0].mime_type, "image/jpeg");
    assert_eq!(info.textures[0].width, None);
    // Grading fails closed: this build cannot decode JPEG.
    let err = vrm_grade(
        dir.path(),
        grade_req("jpeg.vrm", Some("out.vrm"), "demo_toon", 1.0),
    )
    .await
    .expect_err("JPEG grading must fail");
    assert!(matches!(err, LumenError::DocInvalid(_)), "got {err:?}");
    assert!(err.to_string().contains("image/jpeg"), "got {err}");
    assert!(!dir.path().join("out.vrm").exists());
}

#[tokio::test]
async fn vrm_grade_external_texture_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut doc = fixture_document(
        json!({"VRMC_vrm": {"specVersion": "1.0", "meta": {"name": "Ext Fox"}}}),
        0,
    );
    doc["images"] = json!([{"uri": "textures/face.png", "mimeType": "image/png"}]);
    doc["bufferViews"] = json!([]);
    doc["buffers"] = json!([{"byteLength": 0}]);
    write_fixture(dir.path(), "ext.vrm", &glb_bytes(&doc, &[]));
    let info = vrm_inspect(dir.path(), inspect_req("ext.vrm"))
        .await
        .expect("inspect");
    assert_eq!(info.textures[0].mime_type, "external");
    let err = vrm_grade(
        dir.path(),
        grade_req("ext.vrm", Some("out.vrm"), "demo_toon", 1.0),
    )
    .await
    .expect_err("external textures must fail");
    assert!(matches!(err, LumenError::DocInvalid(_)), "got {err:?}");
    assert!(!dir.path().join("out.vrm").exists());
}

#[tokio::test]
async fn vrm_grade_without_textures_is_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut doc = fixture_document(
        json!({"VRMC_vrm": {"specVersion": "1.0", "meta": {"name": "Bald Fox"}}}),
        0,
    );
    doc["images"] = json!([]);
    doc["bufferViews"] = json!([]);
    doc["buffers"] = json!([{"byteLength": 0}]);
    write_fixture(dir.path(), "bald.vrm", &glb_bytes(&doc, &[]));
    let err = vrm_grade(
        dir.path(),
        grade_req("bald.vrm", Some("out.vrm"), "demo_toon", 1.0),
    )
    .await
    .expect_err("nothing to grade must fail");
    assert!(matches!(err, LumenError::DocInvalid(_)), "got {err:?}");
}

#[tokio::test]
async fn vrm_grade_unknown_style_lists_presets() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_fixture(dir.path(), "fox.vrm", &vrm_1_0());
    let err = vrm_grade(
        dir.path(),
        grade_req("fox.vrm", Some("out.vrm"), "no_such_style", 1.0),
    )
    .await
    .expect_err("unknown style must fail");
    assert!(matches!(err, LumenError::BadParam(_)), "got {err:?}");
    assert!(err.to_string().contains("demo_toon_portrait"), "got {err}");
    assert!(!dir.path().join("out.vrm").exists());
}

#[tokio::test]
async fn vrm_paths_escape_root_and_wrong_extension_are_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_fixture(dir.path(), "fox.vrm", &vrm_1_0());
    let err = vrm_inspect(dir.path(), inspect_req("../outside.vrm"))
        .await
        .expect_err("escape must fail");
    assert!(matches!(err, LumenError::PathRejected(_)), "got {err:?}");
    let err = vrm_grade(
        dir.path(),
        grade_req("fox.vrm", Some("../evil.vrm"), "demo_toon", 1.0),
    )
    .await
    .expect_err("escaping output must fail");
    assert!(matches!(err, LumenError::PathRejected(_)), "got {err:?}");
    write_fixture(dir.path(), "fox.glb", &vrm_1_0());
    let err = vrm_inspect(dir.path(), inspect_req("fox.glb"))
        .await
        .expect_err("non-.vrm input must fail");
    assert!(matches!(err, LumenError::PathRejected(_)), "got {err:?}");
}

// ---------------------------------------------------------------------------
// Stdio surface: the same ops through the real binary
// ---------------------------------------------------------------------------

#[test]
fn stdio_vrm_inspect_and_grade_end_to_end() {
    let project = tempfile::tempdir().expect("tempdir");
    write_fixture(project.path(), "fox.vrm", &vrm_1_0());
    let replies = stdio_calls(
        project.path(),
        &[
            ("vrm_inspect", json!({"vrm": "fox.vrm"})),
            (
                "vrm_grade",
                json!({"vrm": "fox.vrm", "output": "graded.vrm", "style": "demo_toon_portrait", "strength": 1.0}),
            ),
        ],
    );
    let inspect_text = reply_text(&replies[0]);
    assert!(inspect_text.contains("Fixture Fox"), "{}", replies[0]);
    assert!(inspect_text.contains("vrm-1.0"), "{}", replies[0]);
    let grade_text = reply_text(&replies[1]);
    assert!(!grade_text.contains("error"), "{}", replies[1]);
    assert!(grade_text.contains("demo_toon_portrait"), "{}", replies[1]);
    assert!(project.path().join("graded.vrm").exists());
}

/// The modern era's per-request metadata (protocol 2026-07-28):
/// every request carries its version and client identity in
/// `_meta`; sessions open with `server/discover`, never
/// `initialize` (D-113 migration, 2026-10-27).
fn modern_meta() -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientInfo": {"name": "vrm-test", "version": "0"},
        "io.modelcontextprotocol/clientCapabilities": {}
    })
}

/// Minimal MCP stdio client (tools/call only), after tests/policy.rs.
fn stdio_calls(project: &std::path::Path, calls: &[(&str, Value)]) -> Vec<Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_lumen"))
        .env_remove("LUMEN_POLICY")
        .env_remove("LUMEN_ALLOW_REMOTE_BRP")
        // The folded generation lane reads a default config from
        // $HOME; keep stdio spawns hermetic (atelier fold, D-143).
        .env_remove("ATELIER_CONFIG")
        .env("HOME", project)
        .env("LUMEN_PROJECT_ROOT", project)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn lumen server");
    let mut stdin = child.stdin.take().expect("stdin");
    let stdout = child.stdout.take().expect("stdout");
    let (tx, rx) = mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { break };
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let mut send = |v: Value| {
        writeln!(stdin, "{v}").expect("write request");
        stdin.flush().expect("flush");
    };
    let recv_id = |id: u64| -> Value {
        loop {
            let line = rx
                .recv_timeout(Duration::from_secs(20))
                .expect("server reply in time");
            let v: Value = serde_json::from_str(&line).expect("server speaks json");
            if v.get("id") == Some(&json!(id)) {
                return v;
            }
        }
    };
    send(
        json!({"jsonrpc":"2.0","id":0,"method":"server/discover","params":{"_meta":modern_meta()}}),
    );
    let discovery = recv_id(0);
    assert!(
        discovery.get("result").is_some(),
        "server/discover failed: {discovery}"
    );
    let mut replies = Vec::new();
    for (index, (tool, arguments)) in calls.iter().enumerate() {
        let id = u64::try_from(index).expect("small") + 1;
        send(json!({"jsonrpc":"2.0","id":id,"method":"tools/call",
            "params":{"name":tool,"arguments":arguments,"_meta":modern_meta()}}));
        replies.push(recv_id(id));
    }
    drop(send);
    drop(stdin);
    let _ = child.kill();
    let _ = child.wait();
    replies
}

fn reply_text(reply: &Value) -> String {
    reply["result"]["content"][0]["text"]
        .as_str()
        .unwrap_or_default()
        .to_string()
}
