//! Validation tests: the contract works when used as intended.

use std::sync::atomic::Ordering;

use serde_json::json;

use super::fixture::{
    Behavior, FakeGpu, StartMode, Tree, atelier, img_request, test_png, txt_request,
};
use crate::generation::admission::{Lane, admit};
use crate::generation::backend::{HistoryState, parse_history};
use crate::generation::config::{Config, Profile};
use crate::generation::lifecycle::EngineState;
use crate::generation::service::{
    ModelsRequest, STYLE_NEGATIVE, STYLE_POSITIVE, build_graph, img2img_params, style_negative,
    style_positive, txt2img_params,
};
use crate::generation::template::{IRIS_IMG2IMG, IRIS_TXT2IMG, Template};

#[test]
fn config_loads_for_qompass() {
    let tree = Tree::new();
    let config = tree.config("qompass");
    assert_eq!(config.profile, Profile::Qompass);
    assert_eq!(config.profile.content_class(), "sfw");
    assert!(config.active_roots().output_root.ends_with("qompass/out"));
}

#[test]
fn config_loads_for_ntranch() {
    let tree = Tree::new();
    let config = tree.config("ntranch");
    assert_eq!(config.profile, Profile::Ntranch);
    assert_eq!(config.profile.content_class(), "adult");
    assert_ne!(
        config.qompass.log_file(Profile::Qompass),
        config.ntranch.log_file(Profile::Ntranch)
    );
}

#[test]
fn config_load_reads_file() {
    let tree = Tree::new();
    let path = tree.path("atelier.toml");
    std::fs::write(&path, tree.toml("qompass")).expect("write");
    assert_eq!(Config::load(&path).expect("load").profile, Profile::Qompass);
}

#[test]
fn admission_admits_correct_hash() {
    let tree = Tree::new();
    let admission = admit("iris", &tree.config("qompass").models["iris"]);
    assert!(admission.admitted, "{admission:?}");
    assert_eq!(
        admission.observed_sha256.as_deref(),
        Some(tree.checkpoint_sha256.as_str())
    );
    assert!(!admission.license_alert);
}

#[test]
fn unrecorded_license_admits_with_alert() {
    let tree = Tree::new();
    let mut config = tree.config("qompass");
    let iris = config.models.get_mut("iris").expect("iris registered");
    iris.license = None;
    let admission = admit("iris", iris);
    assert!(admission.admitted);
    assert!(admission.license_alert);
    assert_eq!(admission.license, "UNRECORDED");
}

#[tokio::test]
async fn models_reports_iris_and_unimplemented_lanes() {
    let tree = Tree::new();
    let (core, _) = atelier(
        &tree,
        "qompass",
        Behavior::default(),
        StartMode::Normal,
        FakeGpu::default(),
    );
    let all = core.models(&ModelsRequest { lane: None }).expect("models");
    assert_eq!(all["profile"], "qompass");
    assert_eq!(all["models"].as_array().map(Vec::len), Some(2));
    assert_eq!(all["models"][0]["model"], "iris");
    assert_eq!(all["models"][1]["model"], "wai");
    let names: Vec<_> = all["unimplemented"]
        .as_array()
        .expect("arr")
        .iter()
        .map(|e| e["model"].clone())
        .collect();
    assert_eq!(names, vec![json!("vera"), json!("aria")]);
    let video = core
        .models(&ModelsRequest {
            lane: Some("video".into()),
        })
        .expect("video");
    assert_eq!(video["models"].as_array().map(Vec::len), Some(0));
    assert_eq!(Lane::parse("music").ok(), Some(Lane::Music));
}

#[test]
fn txt2img_patches_only_named_inputs_and_keeps_links() {
    let template = Template::embedded(&IRIS_TXT2IMG);
    let tree = Tree::new();
    let config = tree.config("qompass");
    let params = txt2img_params(&config, &template, &txt_request()).expect("params");
    let graph = build_graph(&template, &params, "Iris.safetensors", "j1", None);
    assert_eq!(
        graph["6"]["inputs"]["text"],
        format!("{STYLE_POSITIVE}, an empty concert stage")
    );
    assert_eq!(params.prompt, graph["6"]["inputs"]["text"]);
    assert_eq!(graph["7"]["inputs"]["text"], STYLE_NEGATIVE);
    assert_eq!(graph["5"]["inputs"]["width"], 64);
    assert_eq!(graph["5"]["inputs"]["batch_size"], 1);
    assert_eq!(graph["3"]["inputs"]["model"], json!(["4", 0]));
    assert_eq!(graph["3"]["inputs"]["latent_image"], json!(["5", 0]));
    assert_eq!(graph["8"]["inputs"]["vae"], json!(["4", 2]));
    assert_eq!(graph["9"]["inputs"]["filename_prefix"], "atelier-j1");
    // Defaults come from the template and are echoed.
    assert_eq!(params.steps, 30);
    assert_eq!(params.sampler_name, "dpmpp_2m");
    assert_eq!(params.seed, 20261020);
    let keys: Vec<_> = graph.keys().cloned().collect();
    let original: Vec<_> = template.graph.keys().cloned().collect();
    assert_eq!(keys, original);
}

#[test]
fn img2img_patches_source_and_denoise() {
    let template = Template::embedded(&IRIS_IMG2IMG);
    let tree = Tree::new();
    let config = tree.config("ntranch");
    let params = img2img_params(&config, &template, &img_request("/x")).expect("params");
    let graph = build_graph(
        &template,
        &params,
        "Iris.safetensors",
        "j2",
        Some("server name.png"),
    );
    assert_eq!(graph["10"]["inputs"]["image"], "server name.png");
    assert_eq!(graph["3"]["inputs"]["denoise"], 0.6);
    assert_eq!(graph["3"]["inputs"]["latent_image"], json!(["11", 0]));
    assert_eq!(graph["12"]["inputs"]["width"], 64);
    assert_eq!(graph["11"]["inputs"]["pixels"], json!(["12", 0]));
}

#[test]
fn template_identity_is_stable() {
    let first = Template::embedded(&IRIS_TXT2IMG).identity();
    let second = Template::embedded(&IRIS_TXT2IMG).identity();
    assert_eq!(first, second);
    assert_eq!(first.sha256.len(), 64);
    assert_ne!(
        first.sha256,
        Template::embedded(&IRIS_IMG2IMG).identity().sha256
    );
}

#[test]
fn history_success_yields_descriptors() {
    let value = json!({"p1": {"outputs": {"9": {"images": [{"filename": "a.png", "subfolder": "", "type": "output"}]}},
        "status": {"status_str": "success", "completed": true, "messages": []}}});
    let HistoryState::Success(descriptors) = parse_history(&value, "p1", "9").expect("parse")
    else {
        panic!("expected success");
    };
    assert_eq!(descriptors[0].filename, "a.png");
    assert_eq!(
        parse_history(&json!({}), "p1", "9").expect("pending"),
        HistoryState::Pending
    );
}

#[test]
fn safe_output_name_accepts_plain_component() {
    assert_eq!(
        crate::generation::paths::safe_output_name("atelier-j1_00001_.png", "").expect("ok"),
        "atelier-j1_00001_.png"
    );
}

#[test]
fn log_lines_carry_profile_and_fields() {
    let tree = Tree::new();
    let path = tree.path("ntranch/log/atelier-ntranch.jsonl");
    let log = crate::generation::log::EventLog::open(path.clone(), Profile::Ntranch);
    log.event(
        "info",
        "job_start",
        json!({"job_id": "j9", "profile": "qompass"}),
    );
    let text = std::fs::read_to_string(path).expect("log");
    let line: serde_json::Value = serde_json::from_str(text.trim()).expect("json");
    assert_eq!(
        line["profile"], "ntranch",
        "a field cannot override the stamp"
    );
    assert_eq!(line["job_id"], "j9");
    assert!(line["ts"].as_str().is_some_and(|ts| ts.ends_with('Z')));
}

#[test]
fn rfc3339_known_instant() {
    assert_eq!(
        crate::generation::log::rfc3339(1_760_918_400_123),
        "2025-10-20T00:00:00.123Z"
    );
}

#[tokio::test]
async fn status_and_models_do_not_start_engine() {
    let tree = Tree::new();
    let (core, seen) = atelier(
        &tree,
        "qompass",
        Behavior::default(),
        StartMode::Normal,
        FakeGpu::default(),
    );
    let status = core.status().await;
    assert_eq!(status["profile"], "qompass");
    assert_eq!(status["engine"]["state"], json!(EngineState::Stopped));
    assert_eq!(status["server"]["name"], "atelier");
    core.models(&ModelsRequest {
        lane: Some("image".into()),
    })
    .expect("models");
    assert_eq!(seen.launches.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn txt2img_end_to_end_through_fake_comfy() {
    let tree = Tree::new();
    let (core, seen) = atelier(
        &tree,
        "qompass",
        Behavior::default(),
        StartMode::Normal,
        FakeGpu::default(),
    );
    let result = core.txt2img(txt_request()).await.expect("generation");
    assert_eq!(result.profile, "qompass");
    assert_eq!(result.prompt_id, "fake-prompt-1");
    assert!(
        result
            .output
            .path
            .starts_with(tree.path("qompass/out").canonicalize().expect("c"))
    );
    assert_eq!(
        (result.output.image.width, result.output.image.height),
        (64, 48)
    );
    assert!(result.output.image.pixel_stddev > 1.0);
    assert!(seen.saw("GET /view?filename=atelier_00001_.png"));
    assert!(seen.saw("GET /history/fake-prompt-1"));
    assert!(result.cleanup.ok, "{:?}", result.cleanup);
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&result.manifest_path).expect("manifest"))
            .expect("json");
    assert_eq!(manifest["profile"], "qompass");
    assert_eq!(manifest["model"]["sha256"], tree.checkpoint_sha256);
    assert_eq!(manifest["template"]["name"], "iris_txt2img");
    assert_eq!(
        manifest["template"]["sha256"].as_str().map(str::len),
        Some(64)
    );
    assert_eq!(manifest["parameters"]["seed"], 20261020);
    assert_eq!(manifest["output"]["sha256"], result.output.sha256);
    assert_eq!(manifest["lifecycle"]["cleanup"]["ok"], true);
    // Nothing landed under the other profile.
    assert_eq!(
        std::fs::read_dir(tree.path("ntranch/out"))
            .expect("dir")
            .count(),
        0
    );
}

#[tokio::test]
async fn img2img_records_server_upload_name_and_source_hash() {
    let tree = Tree::new();
    let source = tree.path("ntranch/src/scene.png");
    let bytes = test_png(32, 32, true);
    std::fs::write(&source, &bytes).expect("write source");
    let (core, seen) = atelier(
        &tree,
        "ntranch",
        Behavior::default(),
        StartMode::Normal,
        FakeGpu::default(),
    );
    let result = core
        .img2img(img_request(&source.display().to_string()))
        .await
        .expect("img2img");
    let record = result.source.as_ref().expect("source record");
    assert_eq!(
        record.sha256,
        crate::generation::admission::sha256_bytes(&bytes)
    );
    assert_eq!(record.engine_upload.name, "uploaded-by-server (1).png");
    assert_eq!(record.reference, "source_root[0]:scene.png");
    let prompts = seen.prompts.lock().expect("prompts");
    assert_eq!(
        prompts[0]["prompt"]["10"]["inputs"]["image"],
        "uploaded-by-server (1).png"
    );
    assert!(seen.uploads.lock().expect("u")[0].contains("name=\"image\"; filename=\"atelier-"));
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&result.manifest_path).expect("m")).expect("json");
    assert_eq!(manifest["profile"], "ntranch");
    assert_eq!(manifest["source"]["sha256"], record.sha256);
    assert_eq!(manifest["parameters"]["denoise"], 0.6);
    assert_eq!(
        std::fs::read(&source).expect("source"),
        bytes,
        "source unchanged"
    );
}

#[tokio::test]
async fn two_jobs_serialize_and_stop_only_owned_child() {
    let tree = Tree::new();
    let bystander = tokio::process::Command::new("sleep")
        .arg("30")
        .kill_on_drop(true)
        .spawn()
        .expect("bystander");
    let bystander_pid = bystander.id().expect("pid");
    let (core, seen) = atelier(
        &tree,
        "qompass",
        Behavior::default(),
        StartMode::Normal,
        FakeGpu::default(),
    );
    let core = std::sync::Arc::new(core);
    let (first, second) = tokio::join!(core.txt2img(txt_request()), core.txt2img(txt_request()));
    assert!(first.is_ok() && second.is_ok(), "{first:?} {second:?}");
    assert_eq!(seen.launches.load(Ordering::SeqCst), 2);
    assert_eq!(
        seen.max_alive.load(Ordering::SeqCst),
        1,
        "engines never overlapped"
    );
    assert!(std::path::Path::new(&format!("/proc/{bystander_pid}")).exists());
    let status = core.status().await;
    assert_eq!(status["engine"]["state"], json!(EngineState::Stopped));
    assert_eq!(status["last_cleanup"]["ok"], true);
}

#[tokio::test]
async fn log_file_is_profile_scoped() {
    let tree = Tree::new();
    let (core, _) = atelier(
        &tree,
        "ntranch",
        Behavior::default(),
        StartMode::Normal,
        FakeGpu::default(),
    );
    core.txt2img(txt_request()).await.expect("ok");
    let text =
        std::fs::read_to_string(tree.path("ntranch/log/atelier-ntranch.jsonl")).expect("log");
    assert!(text.lines().count() >= 4);
    for line in text.lines() {
        let value: serde_json::Value = serde_json::from_str(line).expect("json line");
        assert_eq!(value["profile"], "ntranch");
    }
    assert_eq!(
        std::fs::read_dir(tree.path("qompass/log"))
            .expect("dir")
            .count(),
        0
    );
}

#[test]
fn style_positive_prepends_default_tags() {
    assert_eq!(
        style_positive("1girl, solo"),
        format!("{STYLE_POSITIVE}, 1girl, solo")
    );
}

#[test]
fn style_positive_skips_tags_the_caller_already_has() {
    assert_eq!(
        style_positive("Cel Shading, 1girl"),
        "anime coloring, flat color, sharp lineart, vibrant colors, \
         detailed eyes, shiny hair, Cel Shading, 1girl"
    );
}

#[test]
fn style_negative_appends_only_missing_exclusions() {
    assert_eq!(style_negative(""), STYLE_NEGATIVE);
    assert_eq!(
        style_negative("blur, Painterly"),
        "blur, Painterly, oil_painting, watercolor, soft_shading, \
         semi-realistic, muted_colors, dull_colors, airbrushed"
    );
}

#[test]
fn style_merge_is_idempotent() {
    let once = style_positive("1girl");
    assert_eq!(style_positive(&once), once);
    let once = style_negative("lowres");
    assert_eq!(style_negative(&once), once);
}
