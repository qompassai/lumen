//! Milestone 1 gate tests: 50/50 validation/adversarial split per tool.
//!
//! Validation tests prove the tool does its job on honest input.
//! Adversarial tests prove it fails closed on hostile or malformed input.
//!
//! Count: 14 tests — 7 validation, 7 adversarial. The two extra adversarial
//! tests pin down the shared project-root path bound. No test mutates
//! process-global state: the project root is an explicit parameter.

use std::path::Path;

use image::{ImageBuffer, Rgba};
use lumen::{
    ATLAS_HEIGHT, ATLAS_WIDTH, bevy_discover, resolve_sprite_path, sprite_info, validate_atlas,
};

fn write_png(dir: &Path, name: &str, w: u32, h: u32, fill: Rgba<u8>) {
    let img: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::from_pixel(w, h, fill);
    img.save(dir.join(name)).expect("save png");
}

// ---------------------------------------------------------------------------
// sprite_info — 2 validation, 2 adversarial
// ---------------------------------------------------------------------------

#[test]
fn sprite_info_reports_dimensions_of_valid_png() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_png(root, "hero.png", 96, 192, Rgba([255, 0, 0, 255]));
    let info = sprite_info(root, "hero.png").expect("valid png must decode");
    assert_eq!(info.width, 96);
    assert_eq!(info.height, 192);
    assert!(info.has_alpha);
    assert!(info.file_bytes > 0);
}

#[test]
fn sprite_info_missing_file_is_an_error_not_a_panic() {
    let dir = tempfile::tempdir().expect("tempdir");
    let err = sprite_info(dir.path(), "does_not_exist.png").unwrap_err();
    assert!(err.to_string().contains("does not exist"));
}

#[test]
fn sprite_info_rejects_path_traversal() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    // Plant a decoy OUTSIDE the project root; the tool must not reach it.
    let outside = root.parent().unwrap().join("decoy.png");
    let img: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::from_pixel(8, 8, Rgba([1, 2, 3, 4]));
    img.save(&outside).expect("save decoy");
    let err = sprite_info(root, "../decoy.png").unwrap_err();
    assert!(err.to_string().contains("escapes"));
    std::fs::remove_file(&outside).ok();
}

#[test]
fn sprite_info_rejects_text_file_wearing_png_extension() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    std::fs::write(root.join("fake.png"), b"this is not a png").expect("write fake");
    let err = sprite_info(root, "fake.png").unwrap_err();
    assert!(err.to_string().contains("not a readable png"));
}

// ---------------------------------------------------------------------------
// validate_atlas — 2 validation, 2 adversarial
// ---------------------------------------------------------------------------

fn write_valid_atlas(dir: &Path) {
    // 384x1152, every cell gets at least one opaque pixel.
    let mut img: ImageBuffer<Rgba<u8>, Vec<u8>> =
        ImageBuffer::from_pixel(ATLAS_WIDTH, ATLAS_HEIGHT, Rgba([0, 0, 0, 0]));
    for row in 0..6u32 {
        for col in 0..4u32 {
            img.put_pixel(col * 96 + 3, row * 192 + 5, Rgba([200, 100, 50, 255]));
        }
    }
    img.save(dir.join("atlas.png")).expect("save atlas");
}

#[test]
fn validate_atlas_accepts_contract_atlas() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_valid_atlas(root);
    let report = validate_atlas(root, "atlas.png").expect("valid atlas must validate");
    assert!(report.valid, "failures: {:?}", report.failures);
    assert!(report.failures.is_empty());
    assert_eq!(report.cells_with_content, 24);
    assert!(report.empty_cells.is_empty());
}

#[test]
fn validate_atlas_flags_fully_transparent_cell() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let mut img: ImageBuffer<Rgba<u8>, Vec<u8>> =
        ImageBuffer::from_pixel(ATLAS_WIDTH, ATLAS_HEIGHT, Rgba([0, 0, 0, 0]));
    // Fill every cell except cell 7.
    for cell in 0..24u32 {
        if cell == 7 {
            continue;
        }
        let (col, row) = (cell % 4, cell / 4);
        img.put_pixel(col * 96 + 1, row * 192 + 1, Rgba([9, 9, 9, 255]));
    }
    img.save(root.join("sparse.png")).expect("save");
    let report = validate_atlas(root, "sparse.png").expect("must validate");
    assert!(report.valid);
    assert_eq!(report.empty_cells, vec![7]);
    assert_eq!(report.cells_with_content, 23);
}

#[test]
fn validate_atlas_rejects_wrong_dimensions() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    write_png(root, "small.png", 100, 100, Rgba([1, 1, 1, 255]));
    let report = validate_atlas(root, "small.png").expect("must produce a report");
    assert!(!report.valid);
    assert!(report.failures.iter().any(|f| f.contains("100x100")));
}

#[test]
fn validate_atlas_rejects_garbage_bytes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    std::fs::write(root.join("junk.png"), b"\x00\x01\x02junk").expect("write junk");
    let err = validate_atlas(root, "junk.png").unwrap_err();
    assert!(err.to_string().contains("not a readable png"));
}

// ---------------------------------------------------------------------------
// bevy_discover — 2 validation, 2 adversarial
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bevy_discover_reports_unreachable_endpoint_cleanly() {
    // Port 9 (discard) is virtually never bound; even if it were, the
    // classification must not panic.
    let d = bevy_discover(Some("http://127.0.0.1:9"))
        .await
        .expect("must classify, not fail");
    assert!(!d.reachable);
    assert!(!d.speaks_brp);
    assert!(d.detail.contains("unreachable"));
}

#[tokio::test]
async fn bevy_discover_confirms_mock_brp_server() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("addr").port();
    tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.expect("accept");
        let mut buf = vec![0u8; 4096];
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let n = sock.read(&mut buf).await.expect("read");
        let req = String::from_utf8_lossy(&buf[..n]);
        assert!(req.contains("bevy/list"), "probe must use bevy/list");
        let body = br#"{"jsonrpc":"2.0","id":1,"result":{"types":[]}}"#;
        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            body.len(),
            String::from_utf8_lossy(body)
        );
        sock.write_all(resp.as_bytes()).await.expect("write");
    });
    let d = bevy_discover(Some(&format!("http://127.0.0.1:{port}")))
        .await
        .expect("must classify");
    assert!(d.reachable);
    assert!(d.speaks_brp);
}

#[tokio::test]
async fn bevy_discover_rejects_malformed_url_without_network() {
    let err = bevy_discover(Some("not a url :::")).await.unwrap_err();
    assert!(err.to_string().contains("not a valid url"));
}

#[tokio::test]
async fn bevy_discover_rejects_non_loopback_by_default() {
    // Must fail closed BEFORE any socket is opened.
    let err = bevy_discover(Some("http://192.168.1.50:15702"))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("non-loopback"));
}

// ---------------------------------------------------------------------------
// resolve_sprite_path — shared bound, adversarial only (validation is covered
// implicitly by every passing tool test above)
// ---------------------------------------------------------------------------

#[test]
fn resolve_rejects_absolute_path() {
    let dir = tempfile::tempdir().expect("tempdir");
    let err = resolve_sprite_path("/etc/passwd", dir.path()).unwrap_err();
    assert!(err.to_string().contains("absolute paths are not accepted"));
}

#[test]
fn resolve_rejects_empty_path() {
    let dir = tempfile::tempdir().expect("tempdir");
    assert!(resolve_sprite_path("", dir.path()).is_err());
}
