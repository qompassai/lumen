//! Adversarial tests: hostile config, arguments, paths, templates, engine
//! responses, and lifecycle faults. Every one must refuse with a named code.

use std::sync::atomic::Ordering;

use serde_json::json;

use super::fixture::{
    Behavior, FakeGpu, HistoryMode, StartMode, Tree, atelier, img_request, spawn_server, test_png,
    txt_request,
};
use crate::generation::admission::{admit as admit_model, admit_all};
use crate::generation::config::Config;
use crate::generation::error::ErrorCode;
use crate::generation::lifecycle::EngineState;
use crate::generation::service::{
    Img2ImgRequest, ModelsRequest, Txt2ImgRequest, img2img_params, txt2img_params,
};
use crate::generation::template::{IRIS_TXT2IMG, Template, validate};

fn code_of<T: std::fmt::Debug>(result: crate::generation::error::Result<T>) -> ErrorCode {
    result.expect_err("must refuse").code
}

// ---- configuration and profiles -------------------------------------------

#[test]
fn missing_config_file_refused() {
    let tree = Tree::new();
    assert_eq!(
        code_of(Config::load(&tree.path("absent.toml"))),
        ErrorCode::ConfigMissing
    );
}

#[test]
fn malformed_toml_refused() {
    assert_eq!(
        code_of(Config::from_toml_str("profile = \"qompass")),
        ErrorCode::ConfigInvalid
    );
}

#[test]
fn unknown_key_refused() {
    let tree = Tree::new();
    let text = format!("surprise = 1\n{}", tree.toml("qompass"));
    assert_eq!(
        code_of(Config::from_toml_str(&text)),
        ErrorCode::ConfigInvalid
    );
}

#[test]
fn unknown_profile_refused() {
    let tree = Tree::new();
    let text = tree
        .toml("qompass")
        .replacen("profile = \"qompass\"", "profile = \"shared\"", 1);
    assert_eq!(
        code_of(Config::from_toml_str(&text)),
        ErrorCode::ProfileUnknown
    );
}

#[test]
fn missing_profile_refused() {
    let tree = Tree::new();
    let text = tree
        .toml("qompass")
        .replacen("profile = \"qompass\"\n", "", 1);
    assert_eq!(
        code_of(Config::from_toml_str(&text)),
        ErrorCode::ProfileMissing
    );
}

#[test]
fn missing_profile_block_refused() {
    let tree = Tree::new();
    let text = format!(
        "profile = \"qompass\"\nstate_root = \"{}\"\n{}{}",
        tree.p("state"),
        tree.profile_block("qompass"),
        tree.tail()
    );
    assert_eq!(
        code_of(Config::from_toml_str(&text)),
        ErrorCode::ProfileBlockMissing
    );
}

fn with_ntranch_output(tree: &Tree, output: &str) -> String {
    let ntranch = tree
        .profile_block("ntranch")
        .replace(&tree.p("ntranch/out"), output);
    format!(
        "profile = \"ntranch\"\nstate_root = \"{}\"\n{}{}{}",
        tree.p("state"),
        tree.profile_block("qompass"),
        ntranch,
        tree.tail()
    )
}

#[test]
fn equal_output_roots_refused() {
    let tree = Tree::new();
    let text = with_ntranch_output(&tree, &tree.p("qompass/out"));
    assert_eq!(
        code_of(Config::from_toml_str(&text)),
        ErrorCode::ProfileRootOverlap
    );
}

#[test]
fn nested_output_roots_refused() {
    let tree = Tree::new();
    std::fs::create_dir_all(tree.path("qompass/out/nested")).expect("mkdir");
    let text = with_ntranch_output(&tree, &tree.p("qompass/out/nested"));
    assert_eq!(
        code_of(Config::from_toml_str(&text)),
        ErrorCode::ProfileRootOverlap
    );
}

#[test]
fn dotdot_overlap_refused() {
    let tree = Tree::new();
    let text = with_ntranch_output(&tree, &tree.p("ntranch/../qompass/out"));
    assert_eq!(
        code_of(Config::from_toml_str(&text)),
        ErrorCode::ProfileRootOverlap
    );
}

#[test]
fn symlink_overlap_refused() {
    let tree = Tree::new();
    std::os::unix::fs::symlink(tree.path("qompass/out"), tree.path("sneaky")).expect("symlink");
    let text = with_ntranch_output(&tree, &tree.p("sneaky"));
    assert_eq!(
        code_of(Config::from_toml_str(&text)),
        ErrorCode::ProfileRootOverlap
    );
}

#[test]
fn cross_kind_overlap_refused() {
    // ntranch log root inside qompass output root.
    let tree = Tree::new();
    std::fs::create_dir_all(tree.path("qompass/out/logs")).expect("mkdir");
    let ntranch = tree
        .profile_block("ntranch")
        .replace(&tree.p("ntranch/log"), &tree.p("qompass/out/logs"));
    let text = format!(
        "profile = \"ntranch\"\nstate_root = \"{}\"\n{}{}{}",
        tree.p("state"),
        tree.profile_block("qompass"),
        ntranch,
        tree.tail()
    );
    assert_eq!(
        code_of(Config::from_toml_str(&text)),
        ErrorCode::ProfileRootOverlap
    );
}

#[test]
fn non_loopback_engine_refused() {
    let tree = Tree::new();
    let text = tree
        .toml("qompass")
        .replace("host = \"127.0.0.1\"", "host = \"192.168.1.20\"");
    assert_eq!(
        code_of(Config::from_toml_str(&text)),
        ErrorCode::EngineAddressNotLoopback
    );
}

#[test]
fn abbreviated_hash_refused() {
    let tree = Tree::new();
    let text = tree
        .toml("qompass")
        .replace(&tree.checkpoint_sha256, "536863e9");
    assert_eq!(
        code_of(Config::from_toml_str(&text)),
        ErrorCode::ConfigInvalid
    );
}

// ---- tool schema: no profile selection --------------------------------------

#[test]
fn tool_call_cannot_select_profile() {
    let extra = json!({"model": "iris", "prompt": "x", "profile": "ntranch"});
    assert!(serde_json::from_value::<Txt2ImgRequest>(extra).is_err());
    let extra = json!({"model": "iris", "prompt": "x", "source_path": "/a", "output_root": "/tmp"});
    assert!(serde_json::from_value::<Img2ImgRequest>(extra).is_err());
    let schema = serde_json::to_value(schemars::schema_for!(Txt2ImgRequest)).expect("schema");
    assert_eq!(schema["additionalProperties"], false);
    assert!(schema["properties"].get("profile").is_none());
}

// ---- model admission ---------------------------------------------------------

#[test]
fn checkpoint_missing_not_admitted() {
    let tree = Tree::new();
    let mut config = tree.config("qompass");
    let iris = config.models.get_mut("iris").expect("iris registered");
    iris.checkpoint_path = tree.path("models/absent.safetensors");
    let admission = admit_model("iris", iris);
    assert!(!admission.admitted);
    assert_eq!(admission.refusal_code, Some("checkpoint_missing"));
    assert_eq!(
        admission.require().expect_err("refuse").code,
        ErrorCode::ModelNotAdmitted
    );
}

#[test]
fn wrong_hash_not_admitted() {
    let tree = Tree::new();
    let mut config = tree.config("qompass");
    let iris = config.models.get_mut("iris").expect("iris registered");
    iris.sha256 = "0".repeat(64);
    let admission = admit_model("iris", iris);
    assert!(!admission.admitted);
    assert_eq!(admission.refusal_code, Some("checkpoint_hash_mismatch"));
}

#[test]
fn different_engine_filename_not_admitted() {
    let tree = Tree::new();
    std::fs::write(tree.path("models/Other.safetensors"), b"other").expect("write");
    let mut config = tree.config("qompass");
    let iris = config.models.get_mut("iris").expect("iris registered");
    iris.engine_name = "Other.safetensors".into();
    let admission = admit_model("iris", iris);
    assert_eq!(admission.refusal_code, Some("checkpoint_name_mismatch"));
}

#[tokio::test]
async fn unadmitted_iris_is_not_listed_and_refuses_generation() {
    let tree = Tree::new();
    let text = tree
        .toml("qompass")
        .replace(&tree.checkpoint_sha256, &"a".repeat(64));
    std::fs::write(tree.path("bad.toml"), text).expect("write");
    let config = Config::load(&tree.path("bad.toml")).expect("config is valid; admission is not");
    let admissions = admit_all(&config);
    let backend =
        crate::generation::backend::ComfyBackend::new(config.engine.base_url()).expect("client");
    let seen = std::sync::Arc::new(super::fixture::Seen::default());
    let launcher = super::fixture::FakeLauncher {
        behavior: Behavior::default(),
        seen: seen.clone(),
        mode: StartMode::Normal,
    };
    let core = crate::generation::service::Atelier::new(
        config,
        admissions,
        backend,
        launcher,
        FakeGpu::default(),
    );
    let models = core.models(&ModelsRequest { lane: None }).expect("models");
    // Only iris's pin was corrupted: wai stays admitted, iris is listed
    // under not_admitted.
    assert_eq!(models["models"].as_array().map(Vec::len), Some(1));
    assert_eq!(models["models"][0]["model"], "wai");
    assert_eq!(models["not_admitted"][0]["model"], "iris");
    assert_eq!(
        code_of(core.txt2img(txt_request()).await),
        ErrorCode::ModelNotAdmitted
    );
    assert_eq!(seen.launches.load(Ordering::SeqCst), 0);
}

// ---- templates ---------------------------------------------------------------

#[test]
fn ui_format_workflow_refused() {
    let ui = r#"{"last_node_id": 9, "nodes": [{"id": 3, "type": "KSampler"}], "links": [[1, 4, 0, 3, 0, "MODEL"]]}"#;
    assert_eq!(
        code_of(validate(&IRIS_TXT2IMG, ui)),
        ErrorCode::TemplateNotApiFormat
    );
}

#[test]
fn unexpected_node_class_refused() {
    let text = IRIS_TXT2IMG
        .text
        .replace("\"SaveImage\"", "\"ExecuteShell\"");
    assert_eq!(
        code_of(validate(&IRIS_TXT2IMG, &text)),
        ErrorCode::TemplateUnexpectedNode
    );
}

#[test]
fn missing_patch_point_refused() {
    let text = IRIS_TXT2IMG.text.replace("\"seed\": 20261020,", "");
    assert_eq!(
        code_of(validate(&IRIS_TXT2IMG, &text)),
        ErrorCode::TemplatePatchPointMissing
    );
}

#[test]
fn link_outside_node_map_refused() {
    let text = IRIS_TXT2IMG
        .text
        .replace("\"samples\": [\"3\", 0]", "\"samples\": [\"99\", 0]");
    assert_eq!(
        code_of(validate(&IRIS_TXT2IMG, &text)),
        ErrorCode::TemplateLinkInvalid
    );
}

// ---- arguments ---------------------------------------------------------------

fn txt_with(patch: serde_json::Value) -> Txt2ImgRequest {
    let mut base = json!({"model": "iris", "prompt": "stage"});
    for (key, value) in patch.as_object().expect("obj") {
        base[key] = value.clone();
    }
    serde_json::from_value(base).expect("request")
}

#[test]
fn bad_dimensions_refused() {
    let tree = Tree::new();
    let config = tree.config("qompass");
    let template = Template::embedded(&IRIS_TXT2IMG);
    for patch in [
        json!({"width": 0}),
        json!({"width": 1001}),
        json!({"height": 4096}),
        json!({"width": 2048, "height": 2048}),
    ] {
        let code = code_of(txt2img_params(&config, &template, &txt_with(patch.clone())));
        assert_eq!(code, ErrorCode::ArgumentInvalid, "{patch}");
    }
    assert!(
        serde_json::from_value::<Txt2ImgRequest>(
            json!({"model": "iris", "prompt": "x", "width": -8})
        )
        .is_err()
    );
}

#[test]
fn bad_sampler_scheduler_steps_cfg_model_prompt_refused() {
    let tree = Tree::new();
    let config = tree.config("qompass");
    let template = Template::embedded(&IRIS_TXT2IMG);
    for patch in [
        json!({"sampler_name": "rm -rf"}),
        json!({"scheduler": "chaos"}),
        json!({"steps": 0}),
        json!({"steps": 61}),
        json!({"cfg": 99.0}),
        json!({"prompt": "x".repeat(4097)}),
        json!({"prompt": "   "}),
    ] {
        let code = code_of(txt2img_params(&config, &template, &txt_with(patch.clone())));
        assert_eq!(code, ErrorCode::ArgumentInvalid, "{patch}");
    }
    let code = code_of(txt2img_params(
        &config,
        &template,
        &txt_with(json!({"model": "vera"})),
    ));
    assert_eq!(code, ErrorCode::ModelUnknown);
}

#[test]
fn bad_denoise_refused() {
    let tree = Tree::new();
    let config = tree.config("ntranch");
    let template = Template::embedded(&crate::generation::template::IRIS_IMG2IMG);
    for denoise in [-0.1, 1.5, 0.0, f64::NAN, f64::INFINITY] {
        let mut request = img_request("/x");
        request.denoise = Some(denoise);
        let code = code_of(img2img_params(&config, &template, &request));
        assert_eq!(code, ErrorCode::ArgumentInvalid, "{denoise}");
    }
}

#[test]
fn unknown_lane_refused() {
    let tree = Tree::new();
    let config = tree.config("qompass");
    let _ = config;
    assert_eq!(
        code_of(crate::generation::admission::Lane::parse("smell")),
        ErrorCode::ArgumentInvalid
    );
}

// ---- source paths -----------------------------------------------------------

fn admit(tree: &Tree, raw: &str) -> ErrorCode {
    let config = tree.config("ntranch");
    code_of(crate::generation::paths::admit_source(
        raw,
        &config.ntranch.source_roots,
        config.limits.source_bytes_max,
    ))
}

#[test]
fn source_traversal_refused() {
    let tree = Tree::new();
    std::fs::write(tree.path("qompass/src/q.png"), test_png(8, 8, true)).expect("write");
    let raw = format!("{}/../../qompass/src/q.png", tree.p("ntranch/src"));
    assert_eq!(admit(&tree, &raw), ErrorCode::SourceOutsideRoots);
    assert_eq!(
        admit(&tree, "ntranch/src/../x.png"),
        ErrorCode::SourceOutsideRoots
    );
}

#[test]
fn source_absolute_outside_refused() {
    let tree = Tree::new();
    assert_eq!(admit(&tree, "/etc/passwd"), ErrorCode::SourceOutsideRoots);
}

#[test]
fn source_symlink_escape_refused() {
    let tree = Tree::new();
    std::fs::write(tree.path("qompass/src/q.png"), test_png(8, 8, true)).expect("write");
    std::os::unix::fs::symlink(
        tree.path("qompass/src/q.png"),
        tree.path("ntranch/src/link.png"),
    )
    .expect("ln");
    assert_eq!(
        admit(&tree, &tree.p("ntranch/src/link.png")),
        ErrorCode::SourceOutsideRoots
    );
}

#[test]
fn source_directory_refused() {
    let tree = Tree::new();
    std::fs::create_dir(tree.path("ntranch/src/dir.png")).expect("mkdir");
    assert_eq!(
        admit(&tree, &tree.p("ntranch/src/dir.png")),
        ErrorCode::SourceNotRegularFile
    );
}

#[test]
fn source_unsupported_format_refused() {
    let tree = Tree::new();
    std::fs::write(tree.path("ntranch/src/fake.png"), b"GIF89a not a png").expect("write");
    assert_eq!(
        admit(&tree, &tree.p("ntranch/src/fake.png")),
        ErrorCode::SourceUnsupportedFormat
    );
}

#[test]
fn source_oversized_refused() {
    let tree = Tree::new();
    let mut bytes = test_png(8, 8, true);
    bytes.resize(1_048_577, 0);
    std::fs::write(tree.path("ntranch/src/big.png"), bytes).expect("write");
    assert_eq!(
        admit(&tree, &tree.p("ntranch/src/big.png")),
        ErrorCode::SourceTooLarge
    );
}

// ---- engine responses --------------------------------------------------------

async fn run_with(behavior: Behavior) -> (ErrorCode, Tree, std::sync::Arc<super::fixture::Seen>) {
    let tree = Tree::new();
    let (core, seen) = atelier(
        &tree,
        "qompass",
        behavior,
        StartMode::Normal,
        FakeGpu::default(),
    );
    let code = code_of(core.txt2img(txt_request()).await);
    let status = core.status().await;
    assert_eq!(
        status["engine"]["state"],
        json!(EngineState::Stopped),
        "cleaned up after {code:?}"
    );
    (code, tree, seen)
}

#[tokio::test]
async fn comfy_validation_failure_classified() {
    let behavior = Behavior {
        submit_validation_error: true,
        ..Behavior::default()
    };
    assert_eq!(run_with(behavior).await.0, ErrorCode::ComfyValidationFailed);
}

#[tokio::test]
async fn execution_failure_classified() {
    let behavior = Behavior {
        history: HistoryMode::ExecutionError,
        ..Behavior::default()
    };
    assert_eq!(run_with(behavior).await.0, ErrorCode::ExecutionFailed);
}

#[tokio::test]
async fn empty_history_not_queued_classified() {
    let behavior = Behavior {
        history: HistoryMode::EmptyNotQueued,
        ..Behavior::default()
    };
    assert_eq!(run_with(behavior).await.0, ErrorCode::HistoryMissing);
}

#[tokio::test]
async fn wrong_prompt_history_classified() {
    let behavior = Behavior {
        history: HistoryMode::WrongPrompt,
        ..Behavior::default()
    };
    assert_eq!(run_with(behavior).await.0, ErrorCode::HistoryWrongPrompt);
}

#[tokio::test]
async fn missing_output_descriptor_classified() {
    let behavior = Behavior {
        history: HistoryMode::NoImages,
        ..Behavior::default()
    };
    assert_eq!(
        run_with(behavior).await.0,
        ErrorCode::OutputDescriptorMissing
    );
}

#[tokio::test]
async fn view_404_classified() {
    let behavior = Behavior {
        view_status: 404,
        ..Behavior::default()
    };
    assert_eq!(run_with(behavior).await.0, ErrorCode::OutputFetchFailed);
}

#[tokio::test]
async fn malformed_history_json_classified() {
    let behavior = Behavior {
        history: HistoryMode::Malformed,
        ..Behavior::default()
    };
    assert_eq!(run_with(behavior).await.0, ErrorCode::BackendProtocol);
}

#[tokio::test]
async fn blank_or_corrupt_output_classified() {
    let behavior = Behavior {
        view_bytes: b"\x89PNG\r\n\x1a\ngarbage".to_vec(),
        ..Behavior::default()
    };
    assert_eq!(run_with(behavior).await.0, ErrorCode::OutputInvalidImage);
}

#[tokio::test]
async fn checkpoint_not_visible_to_engine() {
    let behavior = Behavior {
        checkpoints: vec!["SomethingElse.safetensors".into()],
        ..Behavior::default()
    };
    assert_eq!(run_with(behavior).await.0, ErrorCode::CheckpointNotVisible);
}

#[tokio::test]
async fn missing_node_class_classified() {
    let behavior = Behavior {
        missing_class: Some("KSampler".into()),
        ..Behavior::default()
    };
    assert_eq!(run_with(behavior).await.0, ErrorCode::NodeClassMissing);
}

fn escape_behavior(filename: &str, subfolder: &str) -> Behavior {
    Behavior {
        history: HistoryMode::Success {
            filename: filename.into(),
            subfolder: subfolder.into(),
            kind: "output".into(),
        },
        ..Behavior::default()
    }
}

fn files_outside_out(tree: &Tree) -> usize {
    // Anything written anywhere except qompass/out, the log, and state.
    let mut count = 0;
    for dir in [
        "qompass/src",
        "qompass/staged",
        "ntranch/out",
        "ntranch/staged",
        "ntranch/src",
    ] {
        count += std::fs::read_dir(tree.path(dir)).expect("dir").count();
    }
    count + usize::from(tree.path("escaped.png").exists())
}

#[tokio::test]
async fn output_dotdot_refused_without_writing() {
    let (code, tree, _) = run_with(escape_behavior("../../escaped.png", "")).await;
    assert_eq!(code, ErrorCode::OutputEscapeRefused);
    assert_eq!(files_outside_out(&tree), 0);
}

#[tokio::test]
async fn output_absolute_path_refused() {
    let (code, tree, _) = run_with(escape_behavior("/tmp/escaped.png", "")).await;
    assert_eq!(code, ErrorCode::OutputEscapeRefused);
    assert_eq!(files_outside_out(&tree), 0);
}

#[tokio::test]
async fn output_subfolder_escape_refused() {
    let (code, tree, _) = run_with(escape_behavior("a.png", "../../../ntranch/out")).await;
    assert_eq!(code, ErrorCode::OutputEscapeRefused);
    assert_eq!(files_outside_out(&tree), 0);
}

#[test]
fn output_symlink_target_refused() {
    // A pre-planted symlink at the destination is never followed.
    let tree = Tree::new();
    let root = tree.path("qompass/out").canonicalize().expect("c");
    let dir = crate::generation::paths::create_job_dir(&root, "jx").expect("dir");
    std::os::unix::fs::symlink(tree.path("escaped.png"), dir.join("a.png")).expect("ln");
    let code = code_of(crate::generation::paths::write_new(
        &root, &dir, "a.png", b"bytes",
    ));
    assert_eq!(code, ErrorCode::OutputWriteFailed);
    assert!(!tree.path("escaped.png").exists());
    // A job dir swapped for a symlink out of the root is refused.
    std::fs::create_dir(tree.path("outside")).expect("mkdir");
    std::os::unix::fs::symlink(tree.path("outside"), root.join("jy")).expect("ln");
    let code = code_of(crate::generation::paths::write_new(
        &root,
        &root.join("jy"),
        "a.png",
        b"bytes",
    ));
    assert_eq!(code, ErrorCode::OutputEscapeRefused);
    assert_eq!(
        std::fs::read_dir(tree.path("outside")).expect("d").count(),
        0
    );
}

// ---- lifecycle ---------------------------------------------------------------

#[tokio::test]
async fn unowned_engine_is_refused_and_left_running() {
    let tree = Tree::new();
    let foreign_seen = std::sync::Arc::new(super::fixture::Seen::default());
    spawn_server(tree.port, Behavior::default(), foreign_seen.clone(), None);
    let (core, seen) = atelier(
        &tree,
        "qompass",
        Behavior::default(),
        StartMode::Normal,
        FakeGpu::default(),
    );
    assert_eq!(
        core.status().await["engine"]["state"],
        json!(EngineState::UnownedPresent)
    );
    assert_eq!(
        code_of(core.txt2img(txt_request()).await),
        ErrorCode::EngineUnownedPresent
    );
    assert_eq!(seen.launches.load(Ordering::SeqCst), 0);
    assert!(
        !foreign_seen.saw("/prompt"),
        "nothing submitted to a foreign engine"
    );
    assert!(crate::generation::lifecycle::port_listening(([127, 0, 0, 1], tree.port).into()).await);
}

#[tokio::test]
async fn wrong_gpu_identity_refused_before_launch() {
    let tree = Tree::new();
    let gpu = FakeGpu {
        name: "NVIDIA GeForce RTX 4090".into(),
        ..FakeGpu::default()
    };
    let (core, seen) = atelier(
        &tree,
        "qompass",
        Behavior::default(),
        StartMode::Normal,
        gpu,
    );
    assert_eq!(
        code_of(core.txt2img(txt_request()).await),
        ErrorCode::GpuMismatch
    );
    assert_eq!(seen.launches.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn wrong_engine_device_refused_and_stopped() {
    let tree = Tree::new();
    let behavior = Behavior {
        device_name: "cuda:0 NVIDIA GeForce RTX 4090".into(),
        ..Behavior::default()
    };
    let (core, seen) = atelier(
        &tree,
        "qompass",
        behavior,
        StartMode::Normal,
        FakeGpu::default(),
    );
    assert_eq!(
        code_of(core.txt2img(txt_request()).await),
        ErrorCode::GpuMismatch
    );
    assert!(!seen.saw("POST /prompt"));
    assert_eq!(
        core.status().await["engine"]["state"],
        json!(EngineState::Stopped)
    );
}

#[tokio::test]
async fn low_vram_refused_without_launch() {
    let tree = Tree::new();
    let gpu = FakeGpu {
        free_mib: 100,
        ..FakeGpu::default()
    };
    let (core, seen) = atelier(
        &tree,
        "qompass",
        Behavior::default(),
        StartMode::Normal,
        gpu,
    );
    assert_eq!(
        code_of(core.txt2img(txt_request()).await),
        ErrorCode::VramInsufficient
    );
    assert_eq!(seen.launches.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn engine_exit_during_startup_classified() {
    let tree = Tree::new();
    let (core, _) = atelier(
        &tree,
        "qompass",
        Behavior::default(),
        StartMode::ExitImmediately,
        FakeGpu::default(),
    );
    assert_eq!(
        code_of(core.txt2img(txt_request()).await),
        ErrorCode::EngineExited
    );
}

#[tokio::test]
async fn engine_exit_mid_job_classified() {
    let tree = Tree::new();
    let behavior = Behavior {
        history: HistoryMode::PendingQueued,
        ..Behavior::default()
    };
    let (core, _) = atelier(
        &tree,
        "qompass",
        behavior,
        StartMode::DieAfter(2),
        FakeGpu::default(),
    );
    assert_eq!(
        code_of(core.txt2img(txt_request()).await),
        ErrorCode::EngineExited
    );
}

#[tokio::test]
async fn job_timeout_interrupts_and_cleans_up() {
    let tree = Tree::new();
    let behavior = Behavior {
        history: HistoryMode::PendingQueued,
        ..Behavior::default()
    };
    let (core, seen) = atelier(
        &tree,
        "qompass",
        behavior,
        StartMode::Normal,
        FakeGpu::default(),
    );
    assert_eq!(
        code_of(core.txt2img(txt_request()).await),
        ErrorCode::JobTimeout
    );
    assert!(seen.saw("POST /interrupt"));
    assert_eq!(
        core.status().await["engine"]["state"],
        json!(EngineState::Stopped)
    );
}

#[tokio::test]
async fn cleanup_failure_is_degraded_not_success() {
    let tree = Tree::new();
    let behavior = Behavior {
        ignore_child_exit: true,
        ..Behavior::default()
    };
    let (core, _) = atelier(
        &tree,
        "qompass",
        behavior,
        StartMode::Normal,
        FakeGpu::default(),
    );
    assert_eq!(
        code_of(core.txt2img(txt_request()).await),
        ErrorCode::CleanupFailed
    );
    let status = core.status().await;
    assert_eq!(status["engine"]["state"], json!(EngineState::Degraded));
    assert_eq!(status["last_cleanup"]["ok"], false);
    assert_eq!(status["last_job"]["code"], "cleanup_failed");
    // Degraded refuses further starts rather than stacking engines.
    assert_eq!(
        code_of(core.txt2img(txt_request()).await),
        ErrorCode::CleanupFailed
    );
}

#[tokio::test]
async fn queue_full_is_named_busy() {
    let tree = Tree::new();
    let text = tree
        .toml("qompass")
        .replace("queue_depth_max = 2", "queue_depth_max = 1");
    let config = Config::from_toml_str(&text).expect("config");
    let admissions = admit_all(&config);
    let backend =
        crate::generation::backend::ComfyBackend::new(config.engine.base_url()).expect("client");
    let seen = std::sync::Arc::new(super::fixture::Seen::default());
    let behavior = Behavior {
        history: HistoryMode::PendingQueued,
        ..Behavior::default()
    };
    let launcher = super::fixture::FakeLauncher {
        behavior,
        seen,
        mode: StartMode::Normal,
    };
    let core = crate::generation::service::Atelier::new(
        config,
        admissions,
        backend,
        launcher,
        FakeGpu::default(),
    );
    let (first, second) = tokio::join!(core.txt2img(txt_request()), async {
        tokio::time::sleep(std::time::Duration::from_millis(300)).await;
        core.txt2img(txt_request()).await
    });
    assert_eq!(first.expect_err("timeout").code, ErrorCode::JobTimeout);
    assert_eq!(second.expect_err("busy").code, ErrorCode::EngineBusy);
}
