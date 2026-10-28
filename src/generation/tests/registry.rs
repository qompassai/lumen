//! Model registry tests: half validation (the map works as intended), half
//! adversarial (hostile ids, modalities, defaults, and missing checkpoints
//! must refuse with a named code and never fall back to another model).

use std::sync::atomic::Ordering;

use serde_json::json;

use super::fixture::{Behavior, FakeGpu, StartMode, Tree, WAI_ENGINE_NAME, atelier, txt_request};
use crate::generation::admission::admit_all;
use crate::generation::config::{Config, Modality};
use crate::generation::error::ErrorCode;
use crate::generation::service::{
    ModelsRequest, STYLE_POSITIVE, Txt2ImgRequest, select_model, txt2img_params,
};
use crate::generation::template::{IRIS_TXT2IMG, Template, TemplateKind};

fn code_of<T: std::fmt::Debug>(result: crate::generation::error::Result<T>) -> ErrorCode {
    result.expect_err("must refuse").code
}

fn txt(value: serde_json::Value) -> Txt2ImgRequest {
    serde_json::from_value(value).expect("request")
}

// ---- validation ---------------------------------------------------------------

#[test]
fn registry_parses_both_checkpoints() {
    let tree = Tree::new();
    let config = tree.config("qompass");
    let ids: Vec<&str> = config.models.keys().map(String::as_str).collect();
    assert_eq!(ids, ["iris", "wai"]);
    let wai = config.model("wai").expect("wai registered");
    assert_eq!(wai.modality, Modality::Image);
    assert_eq!(
        wai.checkpoint_path,
        tree.path("models").join(WAI_ENGINE_NAME)
    );
    assert_eq!(wai.engine_name, WAI_ENGINE_NAME);
    assert_eq!(wai.notes.as_deref(), Some("txt2img candidate"));
    assert_eq!(
        config.model("iris").expect("iris").modality,
        Modality::Image
    );
    let admissions = admit_all(&config);
    assert!(admissions["iris"].admitted, "{:?}", admissions["iris"]);
    assert!(admissions["wai"].admitted, "{:?}", admissions["wai"]);
    assert_eq!(
        admissions["wai"].observed_sha256.as_deref(),
        Some(tree.wai_sha256.as_str())
    );
}

#[test]
fn img2img_default_still_iris_and_txt2img_default_is_one_config_line() {
    let tree = Tree::new();
    let config = tree.config("qompass");
    assert_eq!(
        select_model(&config, TemplateKind::Img2Img, None).ok(),
        Some("iris")
    );
    assert_eq!(
        select_model(&config, TemplateKind::Txt2Img, None).ok(),
        Some("iris")
    );
    let text = tree
        .toml("qompass")
        .replacen("txt2img = \"iris\"", "txt2img = \"wai\"", 1);
    let config = Config::from_toml_str(&text).expect("config");
    assert_eq!(
        select_model(&config, TemplateKind::Txt2Img, None).ok(),
        Some("wai")
    );
    assert_eq!(
        select_model(&config, TemplateKind::Img2Img, None).ok(),
        Some("iris")
    );
    // An explicit id always wins over the default.
    assert_eq!(
        select_model(&config, TemplateKind::Txt2Img, Some("iris")).ok(),
        Some("iris")
    );
}

#[tokio::test]
async fn models_lists_every_registered_model_with_modality_and_availability() {
    let tree = Tree::new();
    let (core, _) = atelier(
        &tree,
        "qompass",
        Behavior::default(),
        StartMode::Normal,
        FakeGpu::default(),
    );
    let all = core.models(&ModelsRequest { lane: None }).expect("models");
    for (index, id) in ["iris", "wai"].into_iter().enumerate() {
        let entry = &all["models"][index];
        assert_eq!(entry["model"], id);
        assert_eq!(entry["modality"], "image");
        assert_eq!(entry["available"], true);
    }
    assert_eq!(all["models"][1]["license_alert"], true);
    assert_eq!(
        all["defaults"],
        json!({"txt2img": "iris", "img2img": "iris"})
    );
    let status = core.status().await;
    assert_eq!(status["admitted_models"], json!(["iris", "wai"]));
}

#[tokio::test]
async fn txt2img_with_wai_loads_wai_and_stamps_manifest() {
    let tree = Tree::new();
    let (core, seen) = atelier(
        &tree,
        "qompass",
        Behavior::default(),
        StartMode::Normal,
        FakeGpu::default(),
    );
    let result = core
        .txt2img(txt(
            json!({"model": "wai", "prompt": "an empty concert stage",
            "width": 64, "height": 48}),
        ))
        .await
        .expect("generation");
    assert_eq!(result.model, "wai");
    // The baked-in style block is unchanged and still applied.
    assert!(
        result
            .parameters
            .prompt
            .starts_with("anime coloring, cel shading")
    );
    assert!(STYLE_POSITIVE.starts_with("anime coloring, cel shading"));
    let prompts = seen.prompts.lock().expect("prompts");
    assert_eq!(
        prompts[0]["prompt"]["4"]["inputs"]["ckpt_name"],
        WAI_ENGINE_NAME
    );
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&result.manifest_path).expect("manifest"))
            .expect("json");
    assert_eq!(manifest["model"]["key"], "wai");
    assert_eq!(manifest["model"]["sha256"], tree.wai_sha256);
    assert_eq!(manifest["model"]["engine_name"], WAI_ENGINE_NAME);
    assert_eq!(manifest["model"]["license"], "UNRECORDED");
    assert_eq!(manifest["model"]["license_alert"], true);
    assert_eq!(manifest["model"]["cleared_for_production"], false);
}

#[tokio::test]
async fn omitted_model_uses_default_iris() {
    let tree = Tree::new();
    let (core, seen) = atelier(
        &tree,
        "qompass",
        Behavior::default(),
        StartMode::Normal,
        FakeGpu::default(),
    );
    let result = core
        .txt2img(txt(json!({"prompt": "stage", "width": 64, "height": 48})))
        .await
        .expect("generation");
    assert_eq!(result.model, "iris");
    let prompts = seen.prompts.lock().expect("prompts");
    assert_eq!(
        prompts[0]["prompt"]["4"]["inputs"]["ckpt_name"],
        "Iris.safetensors"
    );
}

// ---- adversarial --------------------------------------------------------------

#[tokio::test]
async fn unknown_model_id_refused_naming_registered_ids() {
    let tree = Tree::new();
    let config = tree.config("qompass");
    let template = Template::embedded(&IRIS_TXT2IMG);
    for id in ["vera", "IRIS", "wai ", "", "../iris"] {
        let error = txt2img_params(
            &config,
            &template,
            &txt(json!({"model": id, "prompt": "x"})),
        )
        .expect_err("must refuse");
        assert_eq!(error.code, ErrorCode::ModelUnknown, "{id:?}");
        assert!(
            error.message.contains("registered: iris, wai"),
            "{}",
            error.message
        );
    }
    let (core, seen) = atelier(
        &tree,
        "qompass",
        Behavior::default(),
        StartMode::Normal,
        FakeGpu::default(),
    );
    let mut request = txt_request();
    request.model = Some("kokoro".into());
    assert_eq!(
        code_of(core.txt2img(request).await),
        ErrorCode::ModelUnknown
    );
    assert_eq!(seen.launches.load(Ordering::SeqCst), 0, "no engine start");
}

#[test]
fn default_naming_unregistered_model_refused() {
    let tree = Tree::new();
    for (from, to) in [
        ("img2img = \"iris\"", "img2img = \"ghost\""),
        ("txt2img = \"iris\"", "txt2img = \"\""),
    ] {
        let text = tree.toml("qompass").replacen(from, to, 1);
        let error = Config::from_toml_str(&text).expect_err("must refuse");
        assert_eq!(error.code, ErrorCode::ConfigInvalid, "{to}");
        assert!(
            error.message.contains("registered: iris, wai"),
            "{}",
            error.message
        );
    }
    let missing = tree.toml("qompass").replacen(
        "[defaults]\ntxt2img = \"iris\"\nimg2img = \"iris\"\n",
        "",
        1,
    );
    assert!(!missing.contains("[defaults]"));
    assert_eq!(
        code_of(Config::from_toml_str(&missing)),
        ErrorCode::ConfigInvalid
    );
}

#[test]
fn unimplemented_or_missing_modality_refused() {
    let tree = Tree::new();
    for to in [
        "modality = \"audio\"",
        "modality = \"video\"",
        "modality = \"\"",
        "",
    ] {
        let text = tree.toml("qompass").replacen("modality = \"image\"", to, 1);
        assert_eq!(
            code_of(Config::from_toml_str(&text)),
            ErrorCode::ConfigInvalid,
            "{to:?}"
        );
    }
}

#[test]
fn hostile_model_ids_refused() {
    let tree = Tree::new();
    let long = format!("[models.{}]", "w".repeat(33));
    for header in [
        "[models.WAI]",
        "[models.\"../wai\"]",
        "[models.\"w a i\"]",
        "[models.\"\"]",
        &long,
    ] {
        let text = tree.toml("qompass").replacen("[models.wai]", header, 1);
        assert_eq!(
            code_of(Config::from_toml_str(&text)),
            ErrorCode::ConfigInvalid,
            "{header}"
        );
    }
    // A duplicate table is a TOML error, not a silent override.
    let text = format!(
        "{}\n[models.wai]\nmodality = \"image\"\n",
        tree.toml("qompass")
    );
    assert_eq!(
        code_of(Config::from_toml_str(&text)),
        ErrorCode::ConfigInvalid
    );
}

#[tokio::test]
async fn missing_wai_checkpoint_is_listed_unavailable_and_never_falls_back() {
    let tree = Tree::new();
    std::fs::remove_file(tree.path("models").join(WAI_ENGINE_NAME)).expect("remove wai");
    let text = tree
        .toml("qompass")
        .replacen("txt2img = \"iris\"", "txt2img = \"wai\"", 1);
    let config = Config::from_toml_str(&text).expect("config is valid; wai admission is not");
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
    assert_eq!(models["models"].as_array().map(Vec::len), Some(1));
    let refused = &models["not_admitted"][0];
    assert_eq!(refused["model"], "wai");
    assert_eq!(refused["modality"], "image");
    assert_eq!(refused["available"], false);
    assert_eq!(refused["refusal_code"], "checkpoint_missing");
    // The default is wai: omitting the model must refuse, not use iris.
    let omitted = txt(json!({"prompt": "stage", "width": 64, "height": 48}));
    assert_eq!(
        code_of(core.txt2img(omitted).await),
        ErrorCode::ModelNotAdmitted
    );
    let mut explicit = txt_request();
    explicit.model = Some("wai".into());
    assert_eq!(
        code_of(core.txt2img(explicit).await),
        ErrorCode::ModelNotAdmitted
    );
    assert_eq!(seen.launches.load(Ordering::SeqCst), 0, "no engine start");
}
