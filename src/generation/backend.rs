//! The `MediaBackend` boundary and the S1 `ComfyBackend`.
//!
//! Contract: the backend owns transport only — every ComfyUI URL atelier
//! builds is built here. Policy (admission, bounds, paths, lifecycle,
//! manifests) lives above it. Every response body is size-bounded and parsed
//! defensively; engine-supplied text is truncated into the error message.
//! Output descriptors are returned as untrusted data for `paths` to judge.

use std::future::Future;
use std::time::Duration;

use serde::Serialize;
use serde_json::{Map, Value};

use crate::generation::error::{AtelierError, ErrorCode, Result, err};

/// Largest JSON body read from the engine, in bytes.
pub const JSON_BYTES_MAX: usize = 16 * 1024 * 1024;
/// Largest output image fetched through `/view`, in bytes.
pub const VIEW_BYTES_MAX: usize = 64 * 1024 * 1024;
/// Per-request transport timeout.
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

/// Engine system information (from `/system_stats`).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SystemInfo {
    pub engine_version: Option<String>,
    pub devices: Vec<DeviceInfo>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DeviceInfo {
    pub name: String,
    pub vram_total_bytes: u64,
    pub vram_free_bytes: u64,
}

/// What the engine says it stored for an upload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct UploadedImage {
    pub name: String,
    pub subfolder: String,
    pub kind: String,
}

/// One untrusted output descriptor from history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct OutputDescriptor {
    pub filename: String,
    pub subfolder: String,
    pub kind: String,
}

/// History state for one prompt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HistoryState {
    /// No record yet.
    Pending,
    /// Finished successfully with these descriptors for the requested node.
    Success(Vec<OutputDescriptor>),
}

/// Queue snapshot (prompt IDs only).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
pub struct QueueInfo {
    pub running: Vec<String>,
    pub pending: Vec<String>,
}

impl QueueInfo {
    pub fn contains(&self, prompt_id: &str) -> bool {
        self.running
            .iter()
            .chain(&self.pending)
            .any(|id| id == prompt_id)
    }
}

/// The narrow engine boundary.
pub trait MediaBackend: Send + Sync {
    fn kind(&self) -> &'static str;
    fn system_info(&self) -> impl Future<Output = Result<SystemInfo>> + Send;
    fn checkpoints(&self) -> impl Future<Output = Result<Vec<String>>> + Send;
    fn has_node_class(&self, class: &str) -> impl Future<Output = Result<bool>> + Send;
    fn upload_image(
        &self,
        name: &str,
        bytes: Vec<u8>,
    ) -> impl Future<Output = Result<UploadedImage>> + Send;
    fn submit(
        &self,
        graph: Map<String, Value>,
        client_id: &str,
    ) -> impl Future<Output = Result<String>> + Send;
    fn history(
        &self,
        prompt_id: &str,
        output_node: &str,
    ) -> impl Future<Output = Result<HistoryState>> + Send;
    fn view(&self, output: &OutputDescriptor) -> impl Future<Output = Result<Vec<u8>>> + Send;
    fn interrupt(&self) -> impl Future<Output = Result<()>> + Send;
    fn queue(&self) -> impl Future<Output = Result<QueueInfo>> + Send;
}

/// ComfyUI over loopback HTTP.
#[derive(Debug, Clone)]
pub struct ComfyBackend {
    base_url: String,
    client: reqwest::Client,
}

fn protocol(what: impl Into<String>) -> AtelierError {
    AtelierError::new(ErrorCode::BackendProtocol, what)
}

async fn read_bounded(mut response: reqwest::Response, bytes_max: usize) -> Result<Vec<u8>> {
    let mut body = Vec::new();
    loop {
        let chunk = response
            .chunk()
            .await
            .map_err(|cause| protocol(format!("body read: {cause}")))?;
        let Some(chunk) = chunk else {
            return Ok(body);
        };
        if body.len().saturating_add(chunk.len()) > bytes_max {
            return Err(protocol(format!("response exceeds {bytes_max} bytes")));
        }
        body.extend_from_slice(&chunk);
    }
}

async fn json_of(response: reqwest::Response) -> Result<Value> {
    let body = read_bounded(response, JSON_BYTES_MAX).await?;
    serde_json::from_slice(&body).map_err(|cause| protocol(format!("malformed JSON: {cause}")))
}

fn str_field(value: &Value, key: &str) -> Result<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| protocol(format!("missing string field `{key}`")))
}

impl ComfyBackend {
    pub fn new(base_url: String) -> Result<ComfyBackend> {
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .no_proxy()
            .build()
            .map_err(|cause| protocol(format!("http client: {cause}")))?;
        Ok(ComfyBackend { base_url, client })
    }

    fn url(&self, path: &str) -> String {
        assert!(path.starts_with('/'));
        format!("{}{path}", self.base_url)
    }

    async fn get(&self, path: &str) -> Result<reqwest::Response> {
        self.client
            .get(self.url(path))
            .send()
            .await
            .map_err(|cause| AtelierError::new(ErrorCode::EngineNotReady, cause.to_string()))
    }

    async fn get_json(&self, path: &str) -> Result<Value> {
        let response = self.get(path).await?;
        if !response.status().is_success() {
            return Err(protocol(format!("GET {path}: HTTP {}", response.status())));
        }
        json_of(response).await
    }
}

/// Parse `/system_stats`.
pub fn parse_system_info(value: &Value) -> Result<SystemInfo> {
    let devices = value
        .get("devices")
        .and_then(Value::as_array)
        .ok_or_else(|| protocol("system_stats lacks devices"))?;
    let devices = devices
        .iter()
        .map(|device| DeviceInfo {
            name: device["name"].as_str().unwrap_or_default().to_string(),
            vram_total_bytes: device["vram_total"].as_u64().unwrap_or(0),
            vram_free_bytes: device["vram_free"].as_u64().unwrap_or(0),
        })
        .collect();
    Ok(SystemInfo {
        engine_version: value["system"]["comfyui_version"]
            .as_str()
            .map(str::to_string),
        devices,
    })
}

/// Parse `/history/{prompt_id}` for `prompt_id`, reading `output_node`.
pub fn parse_history(value: &Value, prompt_id: &str, output_node: &str) -> Result<HistoryState> {
    let Some(records) = value.as_object() else {
        return Err(protocol("history is not an object"));
    };
    if records.is_empty() {
        return Ok(HistoryState::Pending);
    }
    let Some(record) = records.get(prompt_id) else {
        return err(
            ErrorCode::HistoryWrongPrompt,
            format!("history answered for another prompt, not {prompt_id}"),
        );
    };
    let status = record["status"]["status_str"].as_str().unwrap_or_default();
    if status == "error" {
        let detail = record["status"]["messages"]
            .as_array()
            .and_then(|messages| {
                messages
                    .iter()
                    .find(|message| message[0] == "execution_error")
            })
            .map(|message| message[1]["exception_message"].to_string())
            .unwrap_or_else(|| "engine reported status error".to_string());
        return err(ErrorCode::ExecutionFailed, detail);
    }
    let completed = record["status"]["completed"].as_bool().unwrap_or(false);
    if !completed || status != "success" {
        return Ok(HistoryState::Pending);
    }
    let images = record["outputs"][output_node]["images"].as_array();
    let Some(images) = images.filter(|images| !images.is_empty()) else {
        return err(
            ErrorCode::OutputDescriptorMissing,
            format!("history has no images for node {output_node}"),
        );
    };
    let mut descriptors = Vec::with_capacity(images.len());
    for image in images {
        descriptors.push(OutputDescriptor {
            filename: str_field(image, "filename")?,
            subfolder: image["subfolder"].as_str().unwrap_or_default().to_string(),
            kind: image["type"].as_str().unwrap_or_default().to_string(),
        });
    }
    Ok(HistoryState::Success(descriptors))
}

fn queue_ids(value: &Value, key: &str) -> Vec<String> {
    value[key]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry.get(1).and_then(Value::as_str))
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

impl MediaBackend for ComfyBackend {
    fn kind(&self) -> &'static str {
        "comfyui"
    }

    async fn system_info(&self) -> Result<SystemInfo> {
        parse_system_info(&self.get_json("/system_stats").await?)
    }

    async fn checkpoints(&self) -> Result<Vec<String>> {
        let value = self.get_json("/models/checkpoints").await?;
        let names = value
            .as_array()
            .ok_or_else(|| protocol("/models/checkpoints is not an array"))?;
        Ok(names
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect())
    }

    async fn has_node_class(&self, class: &str) -> Result<bool> {
        assert!(
            class
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
        );
        let value = self.get_json(&format!("/object_info/{class}")).await?;
        Ok(value.get(class).is_some())
    }

    async fn upload_image(&self, name: &str, bytes: Vec<u8>) -> Result<UploadedImage> {
        // Contract proven against /opt/comfyui/server.py (0.39.0): multipart
        // field `image` carries the file (filename required); `type` selects
        // input/temp/output; `overwrite` "true"/"1" replaces a same-named
        // file, else the server may rename to "stem (N).ext". The response is
        // {"name","subfolder","type"[,"asset"]}; the returned name is the one
        // LoadImage must reference.
        let part = reqwest::multipart::Part::bytes(bytes)
            .file_name(name.to_string())
            .mime_str("image/png")
            .map_err(|cause| protocol(cause.to_string()))?;
        let form = reqwest::multipart::Form::new()
            .part("image", part)
            .text("type", "input")
            .text("overwrite", "false");
        let response = self
            .client
            .post(self.url("/upload/image"))
            .multipart(form)
            .send()
            .await
            .map_err(|cause| AtelierError::new(ErrorCode::EngineNotReady, cause.to_string()))?;
        if !response.status().is_success() {
            return Err(protocol(format!("upload: HTTP {}", response.status())));
        }
        let value = json_of(response).await?;
        Ok(UploadedImage {
            name: str_field(&value, "name")?,
            subfolder: value["subfolder"].as_str().unwrap_or_default().to_string(),
            kind: str_field(&value, "type")?,
        })
    }

    async fn submit(&self, graph: Map<String, Value>, client_id: &str) -> Result<String> {
        let body = serde_json::json!({ "prompt": graph, "client_id": client_id });
        let response = self
            .client
            .post(self.url("/prompt"))
            .json(&body)
            .send()
            .await
            .map_err(|cause| AtelierError::new(ErrorCode::EngineNotReady, cause.to_string()))?;
        let status = response.status();
        let value = json_of(response).await?;
        if value.get("error").is_some() || !status.is_success() {
            return err(
                ErrorCode::ComfyValidationFailed,
                format!(
                    "HTTP {status}: error={} node_errors={}",
                    value["error"], value["node_errors"]
                ),
            );
        }
        str_field(&value, "prompt_id")
    }

    async fn history(&self, prompt_id: &str, output_node: &str) -> Result<HistoryState> {
        let value = self.get_json(&format!("/history/{prompt_id}")).await?;
        parse_history(&value, prompt_id, output_node)
    }

    async fn view(&self, output: &OutputDescriptor) -> Result<Vec<u8>> {
        let response = self
            .client
            .get(self.url("/view"))
            .query(&[
                ("filename", output.filename.as_str()),
                ("subfolder", output.subfolder.as_str()),
                ("type", output.kind.as_str()),
            ])
            .send()
            .await
            .map_err(|cause| AtelierError::new(ErrorCode::OutputFetchFailed, cause.to_string()))?;
        if !response.status().is_success() {
            return err(
                ErrorCode::OutputFetchFailed,
                format!("/view: HTTP {}", response.status()),
            );
        }
        read_bounded(response, VIEW_BYTES_MAX).await
    }

    async fn interrupt(&self) -> Result<()> {
        let response = self
            .client
            .post(self.url("/interrupt"))
            .send()
            .await
            .map_err(|cause| AtelierError::new(ErrorCode::EngineNotReady, cause.to_string()))?;
        if response.status().is_success() {
            return Ok(());
        }
        Err(protocol(format!("interrupt: HTTP {}", response.status())))
    }

    async fn queue(&self) -> Result<QueueInfo> {
        let value = self.get_json("/queue").await?;
        Ok(QueueInfo {
            running: queue_ids(&value, "queue_running"),
            pending: queue_ids(&value, "queue_pending"),
        })
    }
}
