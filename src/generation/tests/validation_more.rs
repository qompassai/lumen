//! Validation tests, second set: contract pieces used as intended.

use serde_json::json;

use super::fixture::{
    Behavior, FakeGpu, StartMode, Tree, atelier, img_request, test_png, txt_request,
};
use crate::generation::admission::{Lane, sha256_bytes, sha256_file};
use crate::generation::backend::{QueueInfo, parse_system_info};
use crate::generation::config::{Profile, overlaps, resolve_path};
use crate::generation::error::{AtelierError, ErrorClass, ErrorCode};
use crate::generation::lifecycle::parse_nvidia_smi;
use crate::generation::paths::{
    admit_source, create_job_dir, decode_png, safe_output_name, write_new,
};
use crate::generation::service::{ModelsRequest, img2img_params};
use crate::generation::template::{IRIS_IMG2IMG, Template};

#[test]
fn resolve_path_prefers_env() {
    let path = resolve_path(Some("/etc/x.toml"), Some("/home/u")).expect("path");
    assert_eq!(path, std::path::PathBuf::from("/etc/x.toml"));
}

#[test]
fn resolve_path_defaults_under_home() {
    let path = resolve_path(None, Some("/home/u")).expect("path");
    assert_eq!(
        path,
        std::path::PathBuf::from("/home/u/.config/atelier/atelier.toml")
    );
}

#[test]
fn profiles_parse_with_fixed_classes() {
    assert_eq!(Profile::parse("qompass").expect("q").content_class(), "sfw");
    assert_eq!(
        Profile::parse("ntranch").expect("n").content_class(),
        "adult"
    );
}

#[test]
fn sibling_roots_do_not_overlap() {
    assert!(!overlaps("/a/qompass".as_ref(), "/a/ntranch".as_ref()));
    assert!(!overlaps("/a/out".as_ref(), "/a/outer".as_ref()));
}

#[test]
fn error_codes_are_unique_and_classed() {
    let codes = [
        ErrorCode::ConfigMissing,
        ErrorCode::ProfileRootOverlap,
        ErrorCode::CheckpointHashMismatch,
        ErrorCode::SourceOutsideRoots,
        ErrorCode::EngineUnownedPresent,
        ErrorCode::TemplateNotApiFormat,
        ErrorCode::ExecutionFailed,
        ErrorCode::OutputEscapeRefused,
        ErrorCode::CleanupFailed,
        ErrorCode::ArgumentInvalid,
    ];
    let mut names: Vec<_> = codes.iter().map(|code| code.as_str()).collect();
    names.dedup();
    assert_eq!(names.len(), codes.len());
    assert_eq!(ErrorCode::CleanupFailed.class(), ErrorClass::Cleanup);
    assert_eq!(ErrorCode::OutputEscapeRefused.class(), ErrorClass::Output);
}

#[test]
fn error_json_shape_and_bound() {
    let error = AtelierError::new(ErrorCode::ExecutionFailed, "x".repeat(10_000));
    assert!(error.message.len() <= crate::generation::error::ERROR_MESSAGE_BYTES_MAX + 4);
    let value = error.to_json();
    assert_eq!(value["error"]["code"], "execution_failed");
    assert_eq!(value["error"]["class"], "execution");
}

#[test]
fn file_hash_matches_bytes_hash() {
    let tree = Tree::new();
    let path = tree.path("h.bin");
    std::fs::write(&path, b"abc").expect("write");
    assert_eq!(sha256_file(&path, 16).expect("hash"), sha256_bytes(b"abc"));
    assert_eq!(
        sha256_bytes(b"abc"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
}

#[test]
fn lanes_parse() {
    assert_eq!(Lane::parse("image").ok(), Some(Lane::Image));
    assert_eq!(Lane::parse("video").ok(), Some(Lane::Video));
}

#[test]
fn img2img_template_node_classes() {
    let classes = Template::embedded(&IRIS_IMG2IMG).node_classes();
    for class in [
        "LoadImage",
        "VAEEncode",
        "ImageScale",
        "KSampler",
        "SaveImage",
    ] {
        assert!(classes.iter().any(|name| name == class), "{class}");
    }
}

#[test]
fn img2img_default_denoise_comes_from_template() {
    let tree = Tree::new();
    let config = tree.config("ntranch");
    let template = Template::embedded(&IRIS_IMG2IMG);
    let mut request = img_request("/x");
    request.denoise = None;
    request.width = None;
    request.height = None;
    let params = img2img_params(&config, &template, &request).expect("params");
    assert_eq!(params.denoise, 0.6);
    assert_eq!((params.width, params.height), (1280, 720));
}

#[test]
fn system_info_parses_device_and_version() {
    let info = parse_system_info(&json!({"system": {"comfyui_version": "0.39.0"},
        "devices": [{"name": "cuda:0 NVIDIA RTX A6000 : cudaMallocAsync", "vram_total": 10, "vram_free": 5}]}))
    .expect("info");
    assert_eq!(info.engine_version.as_deref(), Some("0.39.0"));
    assert_eq!(info.devices[0].vram_free_bytes, 5);
}

#[test]
fn nvidia_smi_line_parses() {
    let snapshot = parse_nvidia_smi(1, "NVIDIA RTX A6000, 7, 45533\n").expect("parse");
    assert_eq!(snapshot.name, "NVIDIA RTX A6000");
    assert_eq!(
        (snapshot.memory_used_mib, snapshot.memory_free_mib),
        (7, 45533)
    );
}

#[test]
fn queue_contains_running_and_pending() {
    let queue = QueueInfo {
        running: vec!["a".into()],
        pending: vec!["b".into()],
    };
    assert!(queue.contains("a") && queue.contains("b") && !queue.contains("c"));
}

#[test]
fn decode_reports_dimensions_and_blank_measure() {
    let blank = decode_png(&test_png(16, 8, false)).expect("blank");
    assert_eq!((blank.width, blank.height), (16, 8));
    assert_eq!(blank.pixel_stddev, 0.0);
    assert!(
        decode_png(&test_png(16, 8, true))
            .expect("varied")
            .pixel_stddev
            > 1.0
    );
}

#[test]
fn comfy_style_output_names_accepted() {
    for name in ["atelier-j1_00001_.png", "ComfyUI_00042_.png", "a b (1).png"] {
        assert_eq!(safe_output_name(name, "").expect("ok"), name);
    }
}

#[test]
fn job_dir_and_new_file_land_inside_root() {
    let tree = Tree::new();
    let root = tree.path("qompass/out").canonicalize().expect("c");
    let dir = create_job_dir(&root, "j-ok-1").expect("dir");
    let path = write_new(&root, &dir, "x.png", b"bytes").expect("write");
    assert!(path.starts_with(&root));
    assert_eq!(std::fs::read(path).expect("read"), b"bytes");
}

#[test]
fn source_admission_labels_root_and_reads_dims() {
    let tree = Tree::new();
    std::fs::create_dir_all(tree.path("ntranch/src/scene")).expect("mkdir");
    let bytes = test_png(24, 16, true);
    std::fs::write(tree.path("ntranch/src/scene/a.png"), &bytes).expect("write");
    let config = tree.config("ntranch");
    let source = admit_source(
        &tree.p("ntranch/src/scene/a.png"),
        &config.ntranch.source_roots,
        1 << 20,
    )
    .expect("admit");
    assert_eq!(source.label, "source_root[0]:scene/a.png");
    assert_eq!((source.width, source.height), (24, 16));
    assert_eq!(source.sha256, sha256_bytes(&bytes));
}

#[tokio::test]
async fn models_lane_music_lists_only_aria() {
    let tree = Tree::new();
    let (core, _) = atelier(
        &tree,
        "ntranch",
        Behavior::default(),
        StartMode::Normal,
        FakeGpu::default(),
    );
    let music = core
        .models(&ModelsRequest {
            lane: Some("music".into()),
        })
        .expect("music");
    assert_eq!(music["profile"], "ntranch");
    assert_eq!(music["models"].as_array().map(Vec::len), Some(0));
    assert_eq!(music["unimplemented"][0]["model"], "aria");
    assert_eq!(music["unimplemented"].as_array().map(Vec::len), Some(1));
}

#[tokio::test]
async fn status_reports_last_job_after_success() {
    let tree = Tree::new();
    let (core, _) = atelier(
        &tree,
        "qompass",
        Behavior::default(),
        StartMode::Normal,
        FakeGpu::default(),
    );
    let result = core.txt2img(txt_request()).await.expect("job");
    let status = core.status().await;
    assert_eq!(status["last_job"]["job_id"], result.job_id);
    assert_eq!(status["last_job"]["ok"], true);
    assert_eq!(status["queue_depth"], 0);
    assert_eq!(
        status["last_engine_evidence"]["engine_version"],
        "0.39.0-fake"
    );
}

#[test]
fn rfc3339_epoch_zero() {
    assert_eq!(
        crate::generation::log::rfc3339(0),
        "1970-01-01T00:00:00.000Z"
    );
}
