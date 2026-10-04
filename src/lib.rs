//! lumen — "Lux in motu": an MCP server for sprite artistry.
//!
//! This library holds the tool logic. The MCP transport wiring lives in
//! `main.rs`. Every public function fails closed: invalid input produces a
//! typed error, never a panic and never a partial side effect.
//!
//! The project root is an explicit `&Path` parameter on every function that
//! touches the filesystem. Nothing reads ambient configuration except the
//! thin `project_root()` helper used by the binary entry point, which keeps
//! the core testable without process-global mutation.

#![forbid(unsafe_code)]

pub mod a2a;
pub mod animation;
pub mod aseprite;
pub mod bevy;
pub mod cli;
pub mod doc;
pub mod dream;
pub mod export;
pub mod inspect;
pub mod pack;
pub mod upscale;
pub mod palette;
pub mod pipeline;
pub mod policy;
pub mod qa;
pub mod sprite_ops;
pub mod style;
pub mod text;

use std::path::{Path, PathBuf};

use thiserror::Error;

/// Upper bound on decoded sprite files. Anything larger is rejected before
/// decode so a hostile PNG cannot blow up memory.
pub const MAX_SPRITE_BYTES: u64 = 64 * 1024 * 1024;

/// Light Show atlas contract: 96x192 cells, 4 columns x 6 rows.
pub const ATLAS_WIDTH: u32 = 384;
pub const ATLAS_HEIGHT: u32 = 1152;
pub const ATLAS_COLS: u32 = 4;
pub const ATLAS_ROWS: u32 = 6;
pub const CELL_WIDTH: u32 = 96;
pub const CELL_HEIGHT: u32 = 192;
pub const ATLAS_FRAMES: u32 = 24;
pub const ATLAS_TAGS: u32 = 6;
pub const ATLAS_FRAME_MS: u32 = 180;

/// Default Bevy Remote Protocol endpoint (Nub/bevy_mcp uses the same).
pub const DEFAULT_BRP_ENDPOINT: &str = "http://127.0.0.1:15702";

/// Typed failures. Display strings are user-facing; no internal paths leak.
#[derive(Debug, Error)]
pub enum LumenError {
    /// Filesystem or decode failure.
    #[error("io error: {0}")]
    Io(String),
    /// The path was rejected by the project-root bound.
    #[error("path rejected: {0}")]
    PathRejected(String),
    /// The file is not a decodable PNG.
    #[error("not a readable png: {0}")]
    BadPng(String),
    /// The atlas violates the Light Show contract.
    #[error("atlas invalid: {0}")]
    AtlasInvalid(String),
    /// BRP discovery failed in a classified way.
    #[error("brp discovery: {0}")]
    Brp(String),
    /// A tool parameter failed validation.
    #[error("bad parameter: {0}")]
    BadParam(String),
    /// A sprite document failed structural validation.
    #[error("document invalid: {0}")]
    DocInvalid(String),
    /// A required external dependency is unavailable.
    #[error("unavailable: {0}")]
    Unavailable(String),
    /// BRP endpoint was unreachable (connection refused or timed out). This
    /// is a classified outcome, not a bug: callers map it into a structured
    /// report instead of raising it.
    #[error("brp unreachable: {0}")]
    BrpUnreachable(String),
}

impl From<std::io::Error> for LumenError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e.to_string())
    }
}

/// The effective project root: `LUMEN_PROJECT_ROOT` when set, else the
/// process working directory. Reading the environment is safe; only mutation
/// is (forbidden) unsafe, and this function never mutates.
pub fn project_root() -> Result<PathBuf, LumenError> {
    match std::env::var("LUMEN_PROJECT_ROOT") {
        Ok(v) if !v.is_empty() => Ok(PathBuf::from(v)),
        _ => std::env::current_dir().map_err(LumenError::from),
    }
}

/// Resolve a user-supplied path against an explicit project root, fail closed.
///
/// The target must exist, must canonicalize inside the root, and must carry
/// a `.png` extension. Anything else is rejected before any file is opened.
pub fn resolve_sprite_path(user_path: &str, root: &Path) -> Result<PathBuf, LumenError> {
    if user_path.is_empty() {
        return Err(LumenError::PathRejected("empty path".to_string()));
    }
    let root = root.canonicalize().map_err(|e| {
        LumenError::PathRejected(format!("project root not accessible: {e}"))
    })?;
    // Reject absolute paths outright: everything is relative to the root.
    if Path::new(user_path).is_absolute() {
        return Err(LumenError::PathRejected(
            "absolute paths are not accepted; use a project-relative path".to_string(),
        ));
    }
    let candidate = root.join(user_path);
    let canonical = candidate.canonicalize().map_err(|_| {
        LumenError::PathRejected("file does not exist inside the project".to_string())
    })?;
    if !canonical.starts_with(&root) {
        return Err(LumenError::PathRejected(
            "path escapes the project root".to_string(),
        ));
    }
    match canonical.extension().and_then(|e| e.to_str()) {
        Some(ext) if ext.eq_ignore_ascii_case("png") => Ok(canonical),
        _ => Err(LumenError::PathRejected(
            "only .png sprites are accepted".to_string(),
        )),
    }
}

/// Resolve a user-supplied *output* path against an explicit project root.
///
/// Unlike `resolve_sprite_path`, the target need not exist yet: this is for
/// tool outputs. The path must be relative, must stay inside the root, and
/// must carry one of `allowed_exts`. Missing parent directories are created
/// inside the root. Overwriting an existing file is allowed (documented per
/// tool); nothing outside the root is ever touched.
pub fn resolve_write_path(
    user_path: &str,
    root: &Path,
    allowed_exts: &[&str],
) -> Result<PathBuf, LumenError> {
    if user_path.is_empty() {
        return Err(LumenError::PathRejected("empty path".to_string()));
    }
    let root = root.canonicalize().map_err(|e| {
        LumenError::PathRejected(format!("project root not accessible: {e}"))
    })?;
    if Path::new(user_path).is_absolute() {
        return Err(LumenError::PathRejected(
            "absolute paths are not accepted; use a project-relative path".to_string(),
        ));
    }
    let candidate = root.join(user_path);
    let parent = candidate.parent().ok_or_else(|| {
        LumenError::PathRejected("path has no parent directory".to_string())
    })?;
    std::fs::create_dir_all(parent).map_err(LumenError::from)?;
    let canonical_parent = parent.canonicalize().map_err(|e| {
        LumenError::PathRejected(format!("output parent not accessible: {e}"))
    })?;
    if !canonical_parent.starts_with(&root) {
        return Err(LumenError::PathRejected(
            "path escapes the project root".to_string(),
        ));
    }
    let ext_ok = candidate
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| allowed_exts.iter().any(|a| ext.eq_ignore_ascii_case(a)));
    if !ext_ok {
        return Err(LumenError::PathRejected(format!(
            "output must end in one of: {}",
            allowed_exts.join(", ")
        )));
    }
    let file_name = candidate.file_name().ok_or_else(|| {
        LumenError::PathRejected("path has no file name".to_string())
    })?;
    Ok(canonical_parent.join(file_name))
}

/// What `sprite_info` reports about a PNG.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SpriteInfo {
    pub width: u32,
    pub height: u32,
    pub color_type: String,
    pub has_alpha: bool,
    pub file_bytes: u64,
}

/// Read PNG metadata. Fails closed on missing files, oversized files, and
/// undecodable content.
pub fn sprite_info(root: &Path, user_path: &str) -> Result<SpriteInfo, LumenError> {
    let path = resolve_sprite_path(user_path, root)?;
    let meta = std::fs::metadata(&path).map_err(LumenError::from)?;
    if meta.len() > MAX_SPRITE_BYTES {
        return Err(LumenError::PathRejected(format!(
            "file larger than {} bytes",
            MAX_SPRITE_BYTES
        )));
    }
    let reader =
        image::ImageReader::open(&path).map_err(|e| LumenError::Io(e.to_string()))?;
    let reader = reader
        .with_guessed_format()
        .map_err(|e| LumenError::BadPng(e.to_string()))?;
    // Decode headers only; full decode is unnecessary for metadata.
    let img = reader
        .decode()
        .map_err(|e| LumenError::BadPng(e.to_string()))?;
    let color = img.color();
    Ok(SpriteInfo {
        width: img.width(),
        height: img.height(),
        color_type: format!("{color:?}"),
        has_alpha: color.has_alpha(),
        file_bytes: meta.len(),
    })
}

/// What `validate_atlas` reports about a candidate Light Show atlas.
#[derive(Debug, Clone, serde::Serialize)]
pub struct AtlasReport {
    pub width: u32,
    pub height: u32,
    pub valid: bool,
    pub failures: Vec<String>,
    pub cells_with_content: u32,
    pub empty_cells: Vec<u32>,
}

/// Validate a PNG against the Light Show atlas contract.
///
/// Contract: 384x1152 RGBA, 4 columns x 6 rows of 96x192 cells, 24 frames,
/// 6 tags at 180ms. Frame/tag timing lives in the .aseprite sidecar; this
/// checks what the pixels can prove: dimensions, grid geometry, and per-cell
/// content. A fully transparent cell is reported, not fatal.
pub fn validate_atlas(root: &Path, user_path: &str) -> Result<AtlasReport, LumenError> {
    let path = resolve_sprite_path(user_path, root)?;
    let meta = std::fs::metadata(&path).map_err(LumenError::from)?;
    if meta.len() > MAX_SPRITE_BYTES {
        return Err(LumenError::PathRejected(format!(
            "file larger than {} bytes",
            MAX_SPRITE_BYTES
        )));
    }
    let img = image::ImageReader::open(&path)
        .map_err(|e| LumenError::Io(e.to_string()))?
        .with_guessed_format()
        .map_err(|e| LumenError::BadPng(e.to_string()))?
        .decode()
        .map_err(|e| LumenError::BadPng(e.to_string()))?;
    let (w, h) = (img.width(), img.height());
    let mut failures = Vec::new();
    if w != ATLAS_WIDTH || h != ATLAS_HEIGHT {
        failures.push(format!(
            "dimensions {w}x{h}, expected {ATLAS_WIDTH}x{ATLAS_HEIGHT}"
        ));
    }
    if !img.color().has_alpha() {
        failures.push("atlas has no alpha channel; RGBA is required".to_string());
    }
    let rgba = img.to_rgba8();
    let mut cells_with_content = 0u32;
    let mut empty_cells = Vec::new();
    if failures.is_empty() {
        for row in 0..ATLAS_ROWS {
            for col in 0..ATLAS_COLS {
                let cell = row * ATLAS_COLS + col;
                let mut content = false;
                for y in 0..CELL_HEIGHT {
                    for x in 0..CELL_WIDTH {
                        let px = rgba.get_pixel(col * CELL_WIDTH + x, row * CELL_HEIGHT + y);
                        if px[3] != 0 {
                            content = true;
                            break;
                        }
                    }
                    if content {
                        break;
                    }
                }
                if content {
                    cells_with_content += 1;
                } else {
                    empty_cells.push(cell);
                }
            }
        }
    }
    let valid = failures.is_empty();
    Ok(AtlasReport {
        width: w,
        height: h,
        valid,
        failures,
        cells_with_content,
        empty_cells,
    })
}

/// What `bevy_discover` reports about a BRP endpoint.
#[derive(Debug, Clone, serde::Serialize)]
pub struct BrpDiscovery {
    pub endpoint: String,
    pub reachable: bool,
    pub speaks_brp: bool,
    pub detail: String,
}

/// What a BRP call returns: the endpoint, the transport outcome, and the
/// classified JSON-RPC envelope. A JSON-RPC `error` object is a result
/// (`ok: false`), never a transport failure.
#[derive(Debug, Clone, serde::Serialize)]
pub struct BrpCallResult {
    pub endpoint: String,
    pub http_status: u16,
    pub ok: bool,
    pub result: Option<serde_json::Value>,
    pub error: Option<serde_json::Value>,
}

/// Validate a BRP endpoint URL. Loopback only unless
/// `LUMEN_ALLOW_REMOTE_BRP=1`. Fails closed before any socket opens.
fn check_brp_endpoint(endpoint: &str) -> Result<reqwest::Url, LumenError> {
    let url: reqwest::Url = endpoint
        .parse()
        .map_err(|_| LumenError::Brp("endpoint is not a valid url".to_string()))?;
    if url.scheme() != "http" {
        return Err(LumenError::Brp(
            "only http endpoints are accepted".to_string(),
        ));
    }
    let host = url.host_str().unwrap_or("").to_string();
    let loopback = host == "localhost" || host == "127.0.0.1" || host == "::1";
    let remote_ok = std::env::var("LUMEN_ALLOW_REMOTE_BRP").as_deref() == Ok("1");
    if !loopback && !remote_ok {
        return Err(LumenError::Brp(
            "non-loopback BRP targets are rejected; set LUMEN_ALLOW_REMOTE_BRP=1 to opt in"
                .to_string(),
        ));
    }
    Ok(url)
}

/// POST one JSON-RPC call to a BRP endpoint and classify the envelope.
///
/// Shared by `bevy_discover` and `bevy_call`: 5s timeout, no retries.
/// Connection refusal/timeout becomes `BrpUnreachable` (a classified
/// outcome); any other transport problem is `Brp`.
pub(crate) async fn brp_post(
    endpoint: &str,
    method: &str,
    params: serde_json::Value,
) -> Result<BrpCallResult, LumenError> {
    let url = check_brp_endpoint(endpoint)?;
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .map_err(|e| LumenError::Brp(format!("client build failed: {e}")))?;
    let body = serde_json::json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": method,
        "params": params,
    });
    let resp = match client.post(url).json(&body).send().await {
        Ok(r) => r,
        Err(e) if e.is_connect() || e.is_timeout() => {
            return Err(LumenError::BrpUnreachable(format!("unreachable: {e}")));
        }
        Err(e) => return Err(LumenError::Brp(format!("request failed: {e}"))),
    };
    let status = resp.status().as_u16();
    let text = resp
        .text()
        .await
        .map_err(|e| LumenError::Brp(format!("reading body failed: {e}")))?;
    // A non-JSON body is a classified result (not a BRP endpoint), not a
    // transport failure -- the caller decides what that means.
    let parsed: Option<serde_json::Value> = serde_json::from_str(&text).ok();
    let (ok, result, error) = match &parsed {
        Some(v) => (
            v.get("result").is_some(),
            v.get("result").cloned(),
            v.get("error").cloned(),
        ),
        None => (false, None, None),
    };
    Ok(BrpCallResult {
        endpoint: endpoint.to_string(),
        http_status: status,
        ok,
        result,
        error,
    })
}

/// Probe a Bevy Remote Protocol endpoint and classify the outcome.
///
/// Sends a JSON-RPC `bevy/list` call and classifies: result -> BRP
/// confirmed; JSON-RPC error object -> endpoint speaks BRP but rejected the
/// method; connection refused/timeout -> unreachable; non-JSON or HTTP
/// error -> not a BRP endpoint. Non-loopback hosts are rejected unless
/// `LUMEN_ALLOW_REMOTE_BRP=1`, so the tool cannot be used to scan a LAN.
pub async fn bevy_discover(endpoint: Option<&str>) -> Result<BrpDiscovery, LumenError> {
    let endpoint = endpoint.unwrap_or(DEFAULT_BRP_ENDPOINT).to_string();
    match brp_post(&endpoint, "bevy/list", serde_json::json!({})).await {
        Ok(call) if call.ok => Ok(BrpDiscovery {
            endpoint,
            reachable: true,
            speaks_brp: true,
            detail: "BRP confirmed: bevy/list returned a result".to_string(),
        }),
        Ok(call) if call.error.is_some() => Ok(BrpDiscovery {
            endpoint,
            reachable: true,
            speaks_brp: true,
            detail: format!(
                "endpoint speaks JSON-RPC/BRP but rejected the probe: {}",
                call.error.unwrap_or(serde_json::Value::Null)
            ),
        }),
        Ok(call) => Ok(BrpDiscovery {
            endpoint,
            reachable: true,
            speaks_brp: false,
            detail: format!(
                "http {} with a non-JSON-RPC body: not a BRP endpoint",
                call.http_status
            ),
        }),
        Err(LumenError::BrpUnreachable(detail)) => Ok(BrpDiscovery {
            endpoint,
            reachable: false,
            speaks_brp: false,
            detail,
        }),
        Err(e) => Err(e),
    }
}
