//! Stdio MCP surface tests for the generation lane folded from
//! atelier (D-143, 2026-10-27).
//!
//! The fold's parity contract, proven against the real `lumen`
//! executable over stdio:
//! - tools/list carries all 11 tools, and every tool object is
//!   semantically identical to the one its standalone server served
//!   (lumen's seven from the migration baseline, atelier's four from
//!   atelier @ 11f027a) — fixtures in
//!   `tests/fixtures/generation-baseline-tools.json`, captured by the
//!   probes in `~/workspace/mcp-atelier-fold-20261027/`.
//! - Under a fixture atelier config, `atelier_models` /
//!   `atelier_status` answer with atelier's shapes, the lane profiles
//!   and roster (vera/aria unimplemented) intact.
//! - Refusals keep atelier's typed error shapes (`model_unknown`,
//!   `argument_invalid`, the SDK deserialization refusal).
//! - With no config anywhere, the lane refuses per call with
//!   `config_missing` while lumen's own tools serve normally.
//!
//! Validation/adversarial split is roughly even, per house discipline.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use lumen::generation::admission::sha256_bytes;
use serde_json::{Value, json};

const STDIO_REPLY_TIMEOUT: Duration = Duration::from_secs(20);

fn modern_meta() -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientInfo": {"name": "generation-test", "version": "0"},
        "io.modelcontextprotocol/clientCapabilities": {}
    })
}

/// Spawn the server with a fully explicit environment and drive a
/// modern session: `server/discover`, then each request in order
/// with `_meta` stamped in. Returns the JSON-RPC replies in order.
fn stdio_session(env: &[(&str, &std::ffi::OsStr)], requests: &[(&str, Value)]) -> Vec<Value> {
    let mut command = Command::new(env!("CARGO_BIN_EXE_lumen"));
    command
        .env_clear()
        .env("PATH", std::env::var("PATH").unwrap_or_default())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    for (key, value) in env {
        command.env(key, value);
    }
    let mut child = command.spawn().expect("spawn lumen server");
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
                .recv_timeout(STDIO_REPLY_TIMEOUT)
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
    for (index, (method, params)) in requests.iter().enumerate() {
        let id = u64::try_from(index).expect("small") + 1;
        let mut params = params.clone();
        if let Some(object) = params.as_object_mut() {
            object.insert("_meta".to_string(), modern_meta());
        }
        send(json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}));
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

fn reply_is_error(reply: &Value) -> bool {
    reply["result"]["isError"].as_bool().unwrap_or(false)
}

/// A complete fixture atelier tree under `root`: both profiles'
/// roots, two dummy checkpoints (iris, wai) whose SHA-256 the config
/// pins, engine-visible names as symlinks (admission compares
/// canonical paths). Returns the config path.
fn write_atelier_fixture(root: &Path) -> PathBuf {
    for profile in ["qompass", "ntranch"] {
        for sub in ["src", "staged", "out", "log"] {
            std::fs::create_dir_all(root.join(profile).join(sub)).expect("profile roots");
        }
    }
    std::fs::create_dir_all(root.join("state")).expect("state root");
    std::fs::create_dir_all(root.join("models")).expect("models dir");
    std::fs::create_dir_all(root.join("engine")).expect("engine dir");
    let mut hashes = Vec::new();
    for (name, blob) in [
        ("iris", b"fixture-iris-checkpoint".as_slice()),
        ("wai", b"fixture-wai-checkpoint".as_slice()),
    ] {
        let checkpoint = root.join("models").join(format!("{name}.safetensors"));
        std::fs::write(&checkpoint, blob).expect("write checkpoint");
        std::os::unix::fs::symlink(
            &checkpoint,
            root.join("models")
                .join(format!("{name}-engine.safetensors")),
        )
        .expect("engine symlink");
        hashes.push((name, sha256_bytes(blob)));
    }
    let root = root.display();
    let config = format!(
        r#"profile = "qompass"
state_root = "{root}/state"

[profiles.qompass]
source_roots = ["{root}/qompass/src"]
staged_input_root = "{root}/qompass/staged"
output_root = "{root}/qompass/out"
log_root = "{root}/qompass/log"

[profiles.ntranch]
source_roots = ["{root}/ntranch/src"]
staged_input_root = "{root}/ntranch/staged"
output_root = "{root}/ntranch/out"
log_root = "{root}/ntranch/log"

[engine]
host = "127.0.0.1"
port = 8188
program = "/bin/true"
args = []
working_directory = "{root}/engine"
cuda_device_order = "PCI_BUS_ID"
cuda_visible_devices = "1"
gpu_index = 1
expected_gpu_name = "NVIDIA RTX A6000"
vram_free_mib_min = 1
startup_timeout_s = 30
job_timeout_s = 60

[models.iris]
modality = "image"
checkpoint_path = "{root}/models/iris.safetensors"
sha256 = "{iris_hash}"
engine_name = "iris-engine.safetensors"
engine_model_dir = "{root}/models"
display_name = "Fixture Iris"
source = "fixture"

[models.wai]
modality = "image"
checkpoint_path = "{root}/models/wai.safetensors"
sha256 = "{wai_hash}"
engine_name = "wai-engine.safetensors"
engine_model_dir = "{root}/models"
display_name = "Fixture WAI"
source = "fixture"

[defaults]
txt2img = "wai"
img2img = "iris"

[limits]
axis_pixels_max = 2048
total_pixels_max = 2097152
steps_max = 150
prompt_bytes_max = 16384
source_bytes_max = 67108864
queue_depth_max = 4
"#,
        iris_hash = hashes[0].1,
        wai_hash = hashes[1].1,
    );
    let path = PathBuf::from(format!("{root}/config.toml"));
    std::fs::write(&path, config).expect("write config");
    path
}

/// One session under the fixture config: project tempdir + atelier
/// fixture tempdir, both kept alive by the caller.
fn configured_session(project: &Path, config: &Path, requests: &[(&str, Value)]) -> Vec<Value> {
    stdio_session(
        &[
            ("LUMEN_PROJECT_ROOT", project.as_os_str()),
            ("ATELIER_CONFIG", config.as_os_str()),
            ("HOME", project.as_os_str()),
        ],
        requests,
    )
}

fn call(name: &str, arguments: Value) -> (&'static str, Value) {
    ("tools/call", json!({"name": name, "arguments": arguments}))
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

#[test]
fn fold_tools_list_matches_both_baselines() {
    let project = tempfile::tempdir().expect("tempdir");
    let fixture = tempfile::tempdir().expect("tempdir");
    let config = write_atelier_fixture(fixture.path());
    let replies = configured_session(project.path(), &config, &[("tools/list", json!({}))]);
    let tools = replies[0]["result"]["tools"]
        .as_array()
        .expect("tools array")
        .clone();
    assert_eq!(tools.len(), 11, "folded surface is 7 + 4 tools");
    let baseline: Value =
        serde_json::from_str(include_str!("fixtures/generation-baseline-tools.json"))
            .expect("baseline fixture parses");
    let expected: Vec<Value> = baseline["tools"].as_array().expect("tools").clone();
    assert_eq!(expected.len(), 11);
    for want in &expected {
        let name = want["name"].as_str().expect("tool name");
        let got = tools
            .iter()
            .find(|t| t["name"] == want["name"])
            .unwrap_or_else(|| panic!("folded server is missing {name}"));
        assert_eq!(
            got, want,
            "tool {name} differs from its standalone baseline"
        );
    }
}

#[test]
fn fold_configured_status_and_models_keep_atelier_shape() {
    let project = tempfile::tempdir().expect("tempdir");
    let fixture = tempfile::tempdir().expect("tempdir");
    let config = write_atelier_fixture(fixture.path());
    let replies = configured_session(
        project.path(),
        &config,
        &[
            call("atelier_status", json!({})),
            call("atelier_models", json!({})),
        ],
    );
    let status: Value =
        serde_json::from_str(&reply_text(&replies[0])).expect("status returns JSON");
    assert_eq!(status["profile"], "qompass");
    assert_eq!(status["content_class"], "sfw");
    assert_eq!(status["server"]["name"], "atelier");
    assert_eq!(status["admitted_models"], json!(["iris", "wai"]));
    assert_eq!(status["backend"], "comfyui");

    let models: Value =
        serde_json::from_str(&reply_text(&replies[1])).expect("models returns JSON");
    assert_eq!(models["profile"], "qompass");
    assert_eq!(models["defaults"]["txt2img"], "wai");
    assert_eq!(models["defaults"]["img2img"], "iris");
    let admitted: Vec<&str> = models["models"]
        .as_array()
        .expect("models array")
        .iter()
        .filter_map(|m| m["model"].as_str())
        .collect();
    assert_eq!(admitted, ["iris", "wai"]);
    // The roster names survive the fold as data (D-144/D-145: no
    // video/music tools exist; these are not tools).
    let unimplemented = models["unimplemented"].as_array().expect("unimplemented");
    assert_eq!(unimplemented.len(), 2);
    assert!(
        unimplemented
            .iter()
            .any(|u| u["model"] == "vera" && u["lane"] == "video")
    );
    assert!(
        unimplemented
            .iter()
            .any(|u| u["model"] == "aria" && u["lane"] == "music")
    );
}

#[test]
fn fold_unconfigured_lane_refuses_but_lumen_serves() {
    let project = tempfile::tempdir().expect("tempdir");
    let home = tempfile::tempdir().expect("tempdir");
    let replies = stdio_session(
        &[
            ("LUMEN_PROJECT_ROOT", project.path().as_os_str()),
            ("HOME", home.path().as_os_str()),
        ],
        &[
            call("atelier_status", json!({})),
            call("atelier_models", json!({})),
            call("style_list", json!({})),
        ],
    );
    for reply in &replies[..2] {
        assert!(
            reply_is_error(reply),
            "unconfigured call must refuse: {reply}"
        );
        let text = reply_text(reply);
        assert!(text.contains("\"config_missing\""), "{text}");
    }
    assert!(!reply_is_error(&replies[2]), "lumen tools must still serve");
    let styles: Value =
        serde_json::from_str(&reply_text(&replies[2])).expect("style_list returns JSON");
    assert_eq!(styles["styles"].as_array().expect("styles").len(), 16);
}

// ---------------------------------------------------------------------------
// Adversarial
// ---------------------------------------------------------------------------

#[test]
fn fold_txt2img_unknown_model_refused_with_atelier_shape() {
    let project = tempfile::tempdir().expect("tempdir");
    let fixture = tempfile::tempdir().expect("tempdir");
    let config = write_atelier_fixture(fixture.path());
    let replies = configured_session(
        project.path(),
        &config,
        &[call(
            "atelier_txt2img",
            json!({"model": "nope", "prompt": "a cat"}),
        )],
    );
    assert!(reply_is_error(&replies[0]));
    let body: Value = serde_json::from_str(&reply_text(&replies[0])).expect("refusal is JSON");
    assert_eq!(body["error"]["code"], "model_unknown");
    assert_eq!(body["error"]["class"], "model_admission");
    assert_eq!(body["profile"], "qompass");
}

#[test]
fn fold_models_bad_lane_refused() {
    let project = tempfile::tempdir().expect("tempdir");
    let fixture = tempfile::tempdir().expect("tempdir");
    let config = write_atelier_fixture(fixture.path());
    let replies = configured_session(
        project.path(),
        &config,
        &[call("atelier_models", json!({"lane": "sound"}))],
    );
    assert!(reply_is_error(&replies[0]));
    let body: Value = serde_json::from_str(&reply_text(&replies[0])).expect("refusal is JSON");
    assert_eq!(body["error"]["code"], "argument_invalid");
    assert_eq!(body["error"]["class"], "argument");
}

#[test]
fn fold_txt2img_unknown_field_refused() {
    let project = tempfile::tempdir().expect("tempdir");
    let fixture = tempfile::tempdir().expect("tempdir");
    let config = write_atelier_fixture(fixture.path());
    let replies = configured_session(
        project.path(),
        &config,
        &[call(
            "atelier_txt2img",
            json!({"prompt": "a cat", "profile": "ntranch"}),
        )],
    );
    assert!(reply_is_error(&replies[0]));
    let text = reply_text(&replies[0]);
    assert!(
        text.contains("unknown field `profile`"),
        "callers must not switch profiles per request: {text}"
    );
}
