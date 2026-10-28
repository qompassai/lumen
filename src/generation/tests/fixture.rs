//! Test fixtures: a minimal fake ComfyUI over loopback HTTP, a fake engine
//! launcher (a real owned `sleep` child plus the fake server), a fake GPU
//! probe, and a temp-dir config builder with a tiny hashed "checkpoint".

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::generation::admission::{admit_all, sha256_bytes};
use crate::generation::backend::ComfyBackend;
use crate::generation::config::{Config, EngineConfig};
use crate::generation::error::Result;
use crate::generation::lifecycle::{GpuProbe, GpuSnapshot, Launcher};
use crate::generation::service::Atelier;

pub const FAKE_GPU: &str = "NVIDIA RTX A6000";

#[derive(Debug, Clone, PartialEq)]
pub enum HistoryMode {
    Success {
        filename: String,
        subfolder: String,
        kind: String,
    },
    ExecutionError,
    EmptyNotQueued,
    WrongPrompt,
    Malformed,
    NoImages,
    PendingQueued,
}

#[derive(Debug, Clone)]
pub struct Behavior {
    pub device_name: String,
    pub checkpoints: Vec<String>,
    pub missing_class: Option<String>,
    pub submit_validation_error: bool,
    pub history: HistoryMode,
    pub view_status: u16,
    pub view_bytes: Vec<u8>,
    pub upload_name: String,
    /// Keep listening after the owning child dies (cleanup-failure fixture).
    pub ignore_child_exit: bool,
}

impl Default for Behavior {
    fn default() -> Self {
        Behavior {
            device_name: format!("cuda:0 {FAKE_GPU} : cudaMallocAsync"),
            checkpoints: vec!["Iris.safetensors".into(), WAI_ENGINE_NAME.into()],
            missing_class: None,
            submit_validation_error: false,
            history: HistoryMode::Success {
                filename: "atelier_00001_.png".into(),
                subfolder: String::new(),
                kind: "output".into(),
            },
            view_status: 200,
            view_bytes: test_png(64, 48, true),
            upload_name: "uploaded-by-server (1).png".into(),
            ignore_child_exit: false,
        }
    }
}

/// Shared record of what the fake saw.
#[derive(Debug, Default)]
pub struct Seen {
    pub requests: Mutex<Vec<String>>,
    pub uploads: Mutex<Vec<String>>,
    pub prompts: Mutex<Vec<Value>>,
    pub launches: AtomicUsize,
    pub alive: AtomicUsize,
    pub max_alive: AtomicUsize,
}

impl Seen {
    pub fn saw(&self, needle: &str) -> bool {
        self.requests
            .lock()
            .map(|requests| requests.iter().any(|request| request.contains(needle)))
            .unwrap_or(false)
    }
}

/// A PNG of `width`x`height`; `varied` gives nonzero pixel variance.
pub fn test_png(width: u32, height: u32, varied: bool) -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, width, height);
        encoder.set_color(png::ColorType::Rgb);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().expect("png header");
        let data: Vec<u8> = (0..width * height * 3)
            .map(|index| if varied { (index % 251) as u8 } else { 0 })
            .collect();
        writer.write_image_data(&data).expect("png data");
    }
    bytes
}

fn response(status: u16, content_type: &str, body: &[u8]) -> Vec<u8> {
    let mut out = format!(
        "HTTP/1.1 {status} X\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    out.extend_from_slice(body);
    out
}

fn json_response(status: u16, value: &Value) -> Vec<u8> {
    response(status, "application/json", value.to_string().as_bytes())
}

async fn read_request(stream: &mut tokio::net::TcpStream) -> Option<(String, Vec<u8>)> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 8192];
    let header_end = loop {
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
        if let Some(position) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break position + 4;
        }
        if buffer.len() > 1 << 20 {
            return None;
        }
    };
    let head = String::from_utf8_lossy(&buffer[..header_end]).to_string();
    let length = head
        .lines()
        .find_map(|line| {
            let lower = line.to_ascii_lowercase();
            lower
                .strip_prefix("content-length:")
                .map(|value| value.trim().parse::<usize>().unwrap_or(0))
        })
        .unwrap_or(0);
    while buffer.len() < header_end + length {
        let read = stream.read(&mut chunk).await.ok()?;
        if read == 0 {
            break;
        }
        buffer.extend_from_slice(&chunk[..read]);
    }
    let request_line = head.lines().next().unwrap_or_default().to_string();
    Some((request_line, buffer[header_end..].to_vec()))
}

fn route(behavior: &Behavior, seen: &Seen, request_line: &str, body: &[u8]) -> Vec<u8> {
    let mut parts = request_line.split_whitespace();
    let method = parts.next().unwrap_or_default();
    let target = parts.next().unwrap_or_default();
    let path = target.split('?').next().unwrap_or_default();
    if let Ok(mut requests) = seen.requests.lock() {
        requests.push(format!("{method} {target}"));
    }
    let prompt_id = "fake-prompt-1";
    match (method, path) {
        ("GET", "/system_stats") => json_response(
            200,
            &json!({
                "system": {"comfyui_version": "0.39.0-fake"},
                "devices": [{"name": behavior.device_name, "type": "cuda", "index": 0,
                    "vram_total": 51_527_024_640u64, "vram_free": 50_000_000_000u64}]
            }),
        ),
        ("GET", "/models/checkpoints") => json_response(200, &json!(behavior.checkpoints)),
        ("GET", path) if path.starts_with("/object_info/") => {
            let class = &path["/object_info/".len()..];
            if behavior.missing_class.as_deref() == Some(class) {
                json_response(200, &json!({}))
            } else {
                json_response(200, &json!({ class: {"input": {}} }))
            }
        }
        ("POST", "/upload/image") => {
            let text = String::from_utf8_lossy(body);
            let field_ok = text.contains("name=\"image\"; filename=");
            if let Ok(mut uploads) = seen.uploads.lock() {
                uploads.push(text.chars().take(400).collect());
            }
            if !field_ok {
                return response(400, "text/plain", b"");
            }
            json_response(
                200,
                &json!({"name": behavior.upload_name, "subfolder": "", "type": "input"}),
            )
        }
        ("POST", "/prompt") => {
            let value: Value = serde_json::from_slice(body).unwrap_or(Value::Null);
            if let Ok(mut prompts) = seen.prompts.lock() {
                prompts.push(value);
            }
            if behavior.submit_validation_error {
                return json_response(
                    400,
                    &json!({
                        "error": {"type": "prompt_outputs_failed_validation", "message": "bad"},
                        "node_errors": {"4": {"errors": [{"message": "ckpt not found"}]}}
                    }),
                );
            }
            json_response(
                200,
                &json!({"prompt_id": prompt_id, "number": 1, "node_errors": {}}),
            )
        }
        ("GET", "/queue") => {
            let running = if behavior.history == HistoryMode::PendingQueued {
                json!([[1, prompt_id, {}, {}, []]])
            } else {
                json!([])
            };
            json_response(200, &json!({"queue_running": running, "queue_pending": []}))
        }
        ("GET", path) if path.starts_with("/history/") => history(behavior, prompt_id),
        ("GET", "/view") => {
            if behavior.view_status != 200 {
                return response(behavior.view_status, "text/plain", b"not found");
            }
            response(200, "image/png", &behavior.view_bytes)
        }
        ("POST", "/interrupt") => response(200, "text/plain", b""),
        _ => response(404, "text/plain", b""),
    }
}

fn history(behavior: &Behavior, prompt_id: &str) -> Vec<u8> {
    let done = |outputs: Value, status: &str| {
        json!({ prompt_id: {"prompt": [], "outputs": outputs,
            "status": {"status_str": status, "completed": status == "success",
                "messages": [["execution_error", {"exception_message": "CUDA boom"}]]}}})
    };
    match &behavior.history {
        HistoryMode::Success {
            filename,
            subfolder,
            kind,
        } => json_response(
            200,
            &done(
                json!({"9": {"images": [{"filename": filename, "subfolder": subfolder, "type": kind}]}}),
                "success",
            ),
        ),
        HistoryMode::ExecutionError => json_response(200, &done(json!({}), "error")),
        HistoryMode::EmptyNotQueued | HistoryMode::PendingQueued => json_response(200, &json!({})),
        HistoryMode::WrongPrompt => json_response(200, &json!({"other-prompt": {"outputs": {}}})),
        HistoryMode::Malformed => response(200, "application/json", b"{not json"),
        HistoryMode::NoImages => json_response(200, &done(json!({"9": {"images": []}}), "success")),
    }
}

/// Serve the fake on `port` until `/proc/<pid>` disappears (unless the
/// behavior says to ignore child exit, or `pid` is `None`).
pub fn spawn_server(port: u16, behavior: Behavior, seen: Arc<Seen>, pid: Option<u32>) {
    let listener = std::net::TcpListener::bind(("127.0.0.1", port)).expect("bind fake comfy");
    listener.set_nonblocking(true).expect("nonblocking");
    let listener = tokio::net::TcpListener::from_std(listener).expect("tokio listener");
    tokio::spawn(async move {
        loop {
            let child_gone = pid.is_some_and(|pid| !Path::new(&format!("/proc/{pid}")).exists());
            if child_gone && !behavior.ignore_child_exit {
                return;
            }
            let accepted = tokio::time::timeout(Duration::from_millis(50), listener.accept()).await;
            let Ok(Ok((mut stream, _))) = accepted else {
                continue;
            };
            let behavior = behavior.clone();
            let seen = seen.clone();
            tokio::spawn(async move {
                if let Some((line, body)) = read_request(&mut stream).await {
                    let reply = route(&behavior, &seen, &line, &body);
                    let _ = stream.write_all(&reply).await;
                    let _ = stream.shutdown().await;
                }
            });
        }
    });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartMode {
    /// Owned `sleep` child plus the fake server.
    Normal,
    /// Child exits immediately and nothing listens.
    ExitImmediately,
    /// Child lives `seconds`; the server stays up while it lives.
    DieAfter(u32),
}

#[derive(Clone)]
pub struct FakeLauncher {
    pub behavior: Behavior,
    pub seen: Arc<Seen>,
    pub mode: StartMode,
}

impl Launcher for FakeLauncher {
    fn launch(&self, engine: &EngineConfig, _log: &Path) -> Result<tokio::process::Child> {
        self.seen.launches.fetch_add(1, Ordering::SeqCst);
        let (program, arg) = match self.mode {
            StartMode::Normal => ("sleep", "600".to_string()),
            StartMode::ExitImmediately => ("sleep", "0".to_string()),
            StartMode::DieAfter(seconds) => ("sleep", seconds.to_string()),
        };
        let child = tokio::process::Command::new(program)
            .arg(arg)
            .kill_on_drop(true)
            .spawn()
            .map_err(|cause| {
                crate::generation::error::AtelierError::new(
                    crate::generation::error::ErrorCode::EngineStartFailed,
                    cause.to_string(),
                )
            })?;
        let alive = self.seen.alive.fetch_add(1, Ordering::SeqCst) + 1;
        self.seen.max_alive.fetch_max(alive, Ordering::SeqCst);
        let seen = self.seen.clone();
        let pid = child.id();
        if self.mode != StartMode::ExitImmediately {
            spawn_server(engine.port, self.behavior.clone(), self.seen.clone(), pid);
        }
        tokio::spawn(async move {
            while pid.is_some_and(|pid| Path::new(&format!("/proc/{pid}")).exists()) {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
            seen.alive.fetch_sub(1, Ordering::SeqCst);
        });
        Ok(child)
    }
}

#[derive(Debug, Clone)]
pub struct FakeGpu {
    pub name: String,
    pub free_mib: u64,
}

impl Default for FakeGpu {
    fn default() -> Self {
        FakeGpu {
            name: FAKE_GPU.into(),
            free_mib: 48_000,
        }
    }
}

impl GpuProbe for FakeGpu {
    async fn snapshot(&self, index: u32) -> Result<GpuSnapshot> {
        Ok(GpuSnapshot {
            index,
            name: self.name.clone(),
            memory_used_mib: 7,
            memory_free_mib: self.free_mib,
        })
    }
}

/// A temp tree with both profiles' roots, a state root, and a checkpoint.
pub struct Tree {
    pub dir: tempfile::TempDir,
    pub port: u16,
    pub checkpoint_sha256: String,
    pub wai_sha256: String,
}

pub const CHECKPOINT_BYTES: &[u8] = b"fake iris checkpoint for atelier tests";
pub const WAI_BYTES: &[u8] = b"fake wai checkpoint for atelier tests";
/// WAI is registered by its real file name (no alias).
pub const WAI_ENGINE_NAME: &str = "waiIllustriousSDXL_v170.safetensors";

impl Tree {
    pub fn new() -> Tree {
        let dir = tempfile::tempdir().expect("tempdir");
        for profile in ["qompass", "ntranch"] {
            for leaf in ["src", "staged", "out", "log"] {
                std::fs::create_dir_all(dir.path().join(profile).join(leaf)).expect("mkdir");
            }
        }
        std::fs::create_dir_all(dir.path().join("state")).expect("mkdir state");
        std::fs::create_dir_all(dir.path().join("models")).expect("mkdir models");
        let checkpoint = dir.path().join("models/Illustrious-XL-v1.1.safetensors");
        std::fs::write(&checkpoint, CHECKPOINT_BYTES).expect("write ckpt");
        std::os::unix::fs::symlink(&checkpoint, dir.path().join("models/Iris.safetensors"))
            .expect("symlink");
        std::fs::write(dir.path().join("models").join(WAI_ENGINE_NAME), WAI_BYTES)
            .expect("write wai");
        let port = std::net::TcpListener::bind("127.0.0.1:0")
            .and_then(|listener| listener.local_addr())
            .map(|address| address.port())
            .expect("free port");
        Tree {
            dir,
            port,
            checkpoint_sha256: sha256_bytes(CHECKPOINT_BYTES),
            wai_sha256: sha256_bytes(WAI_BYTES),
        }
    }

    pub fn path(&self, relative: &str) -> PathBuf {
        self.dir.path().join(relative)
    }

    pub fn p(&self, relative: &str) -> String {
        self.path(relative).display().to_string()
    }

    pub fn profile_block(&self, name: &str) -> String {
        format!(
            "[profiles.{name}]\nsource_roots = [\"{}\"]\nstaged_input_root = \"{}\"\noutput_root = \"{}\"\nlog_root = \"{}\"\n",
            self.p(&format!("{name}/src")),
            self.p(&format!("{name}/staged")),
            self.p(&format!("{name}/out")),
            self.p(&format!("{name}/log")),
        )
    }

    /// The common non-profile sections.
    pub fn tail(&self) -> String {
        format!(
            r#"
[engine]
host = "127.0.0.1"
port = {port}
program = "/usr/bin/sleep"
args = ["600"]
working_directory = "{state}"
cuda_device_order = "PCI_BUS_ID"
cuda_visible_devices = "1"
gpu_index = 1
expected_gpu_name = "{FAKE_GPU}"
vram_free_mib_min = 9000
startup_timeout_s = 5
job_timeout_s = 5

[models.iris]
modality = "image"
checkpoint_path = "{models}/Illustrious-XL-v1.1.safetensors"
sha256 = "{sha}"
engine_name = "Iris.safetensors"
engine_model_dir = "{models}"
display_name = "Illustrious XL v1.1"
source = "test fixture"
license = "CreativeML Open RAIL++-M (test)"

[models.wai]
modality = "image"
checkpoint_path = "{models}/{wai_name}"
sha256 = "{wai_sha}"
engine_name = "{wai_name}"
engine_model_dir = "{models}"
display_name = "WAI-illustrious-SDXL v17"
source = "test fixture"
notes = "txt2img candidate"

[defaults]
txt2img = "iris"
img2img = "iris"

[limits]
axis_pixels_max = 2048
total_pixels_max = 2097152
steps_max = 60
prompt_bytes_max = 4096
source_bytes_max = 1048576
queue_depth_max = 2
"#,
            port = self.port,
            state = self.p("state"),
            models = self.p("models"),
            sha = self.checkpoint_sha256,
            wai_sha = self.wai_sha256,
            wai_name = WAI_ENGINE_NAME,
        )
    }

    pub fn toml(&self, profile: &str) -> String {
        format!(
            "profile = \"{profile}\"\nstate_root = \"{}\"\n{}{}{}",
            self.p("state"),
            self.profile_block("qompass"),
            self.profile_block("ntranch"),
            self.tail()
        )
    }

    pub fn config(&self, profile: &str) -> Config {
        Config::from_toml_str(&self.toml(profile)).expect("valid test config")
    }
}

pub type TestAtelier = Atelier<ComfyBackend, FakeLauncher, FakeGpu>;

pub fn atelier(
    tree: &Tree,
    profile: &str,
    behavior: Behavior,
    mode: StartMode,
    gpu: FakeGpu,
) -> (TestAtelier, Arc<Seen>) {
    let config = tree.config(profile);
    let admissions = admit_all(&config);
    let backend = ComfyBackend::new(config.engine.base_url()).expect("client");
    let seen = Arc::new(Seen::default());
    let launcher = FakeLauncher {
        behavior,
        seen: seen.clone(),
        mode,
    };
    (
        Atelier::new(config, admissions, backend, launcher, gpu),
        seen,
    )
}

pub fn txt_request() -> crate::generation::service::Txt2ImgRequest {
    serde_json::from_value(
        json!({"model": "iris", "prompt": "an empty concert stage", "width": 64, "height": 48}),
    )
    .expect("request")
}

pub fn img_request(source: &str) -> crate::generation::service::Img2ImgRequest {
    serde_json::from_value(
        json!({"model": "iris", "prompt": "a stage", "source_path": source,
        "denoise": 0.6, "width": 64, "height": 48}),
    )
    .expect("request")
}
