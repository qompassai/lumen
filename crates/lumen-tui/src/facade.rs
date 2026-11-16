//! The narrow lumen surface the TUI programs against (DESIGN §D2).
//!
//! Two implementations: [`FakeFacade`] (in-memory, scripted, records
//! every call — used by all tests) and [`McpStdioFacade`] (the real
//! wiring: a JSON-RPC client over the stdio of a spawned
//! `lumen-mcp-server`). Filesystem-only surfaces (identity records,
//! factory status, media listing) are read directly by the real facade
//! because the MCP server exposes no tool for them; each result says so.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use serde_json::{Value, json};
use thiserror::Error;

/// Maximum bytes in one MCP stdio line, either direction.
pub const MCP_LINE_BYTES_MAX: usize = 8 * 1024 * 1024;
/// Deadline for one MCP request round-trip.
pub const MCP_DEADLINE_MS: u64 = 30_000;
/// Maximum identity records listed from one project root scan.
pub const IDENTITY_RECORDS_MAX: usize = 256;
/// Maximum directory entries scanned while looking for identity records.
pub const SCAN_ENTRIES_MAX: usize = 4096;

/// Typed facade failures. A dead or malformed server is a value, never
/// a panic.
#[derive(Debug, Error)]
pub enum FacadeError {
    /// The server process could not be spawned or died.
    #[error("server unavailable: {0}")]
    Unavailable(String),
    /// The server answered outside the expected protocol shape.
    #[error("malformed tool result: {0}")]
    Malformed(String),
    /// The round-trip exceeded [`MCP_DEADLINE_MS`].
    #[error("server call timed out after {MCP_DEADLINE_MS} ms")]
    Timeout,
    /// A filesystem surface failed.
    #[error("io error: {0}")]
    Io(String),
}

/// The operations the screens need. All results are JSON values so the
/// UI renders member data without owning member types.
pub trait LumenFacade {
    /// Member status: version, tool count, generation lane state.
    fn status(&mut self) -> Result<Value, FacadeError>;
    /// The member's MCP tool names.
    fn tools(&mut self) -> Result<Vec<String>, FacadeError>;
    /// Call one MCP tool with JSON arguments.
    fn call(&mut self, tool: &str, args: Value) -> Result<Value, FacadeError>;
    /// Identity records found under the project root (filesystem surface).
    fn identity_records(&mut self) -> Result<Vec<Value>, FacadeError>;
    /// Factory status (filesystem surface over the factory crates).
    fn factory_status(&mut self) -> Result<Value, FacadeError>;
}

/// Validate a tool result's shape before the UI trusts it: results must
/// be JSON objects or carry an `error` string. Anything else is a typed
/// [`FacadeError::Malformed`], which the app renders instead of crashing.
pub fn validate_tool_result(value: &Value) -> Result<Value, FacadeError> {
    match value {
        Value::Object(_) => Ok(value.clone()),
        Value::String(text) => match serde_json::from_str::<Value>(text) {
            Ok(parsed @ Value::Object(_)) => Ok(parsed),
            _ => Err(FacadeError::Malformed(format!(
                "expected JSON object, got string of {} chars",
                text.chars().count()
            ))),
        },
        other => Err(FacadeError::Malformed(format!(
            "expected JSON object, got {other}"
        ))),
    }
}

/// In-memory facade for tests: scripted results, recorded calls.
#[derive(Debug, Default)]
pub struct FakeFacade {
    /// Value returned by `status`.
    pub status_result: Value,
    /// Tool names returned by `tools`.
    pub tools_result: Vec<String>,
    /// Results returned by successive `call`s; empty queue = echo object.
    pub call_results: VecDeque<Result<Value, FacadeError>>,
    /// Identity records returned by `identity_records`.
    pub records_result: Vec<Value>,
    /// Value returned by `factory_status`.
    pub factory_result: Value,
    /// Every `(tool, args)` passed to `call`, in order.
    pub calls: Vec<(String, Value)>,
}

impl FakeFacade {
    /// A fake with benign defaults.
    pub fn new() -> FakeFacade {
        FakeFacade {
            status_result: json!({"version": "2.0.2", "generation": "ready"}),
            tools_result: vec!["lumen_status".to_string(), "lumen_txt2img".to_string()],
            factory_result: json!({"crates": ["lumen-factory", "lumen-rig-core"]}),
            ..FakeFacade::default()
        }
    }

    /// Queue the result of the next `call`.
    pub fn queue_call(&mut self, result: Result<Value, FacadeError>) {
        self.call_results.push_back(result);
    }
}

impl LumenFacade for FakeFacade {
    fn status(&mut self) -> Result<Value, FacadeError> {
        Ok(self.status_result.clone())
    }

    fn tools(&mut self) -> Result<Vec<String>, FacadeError> {
        Ok(self.tools_result.clone())
    }

    fn call(&mut self, tool: &str, args: Value) -> Result<Value, FacadeError> {
        self.calls.push((tool.to_string(), args.clone()));
        match self.call_results.pop_front() {
            Some(result) => result.and_then(|v| validate_tool_result(&v)),
            None => Ok(json!({"status": "ok", "tool": tool, "echo": args})),
        }
    }

    fn identity_records(&mut self) -> Result<Vec<Value>, FacadeError> {
        Ok(self.records_result.clone())
    }

    fn factory_status(&mut self) -> Result<Value, FacadeError> {
        Ok(self.factory_result.clone())
    }
}

/// Real facade: JSON-RPC over the stdio of a spawned lumen server.
///
/// The child is owned here and killed on drop. Reads run on one helper
/// thread feeding a channel so a wedged server hits the deadline
/// instead of hanging the UI.
pub struct McpStdioFacade {
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    lines: mpsc::Receiver<String>,
    next_id: u64,
    project_root: PathBuf,
    server_bin: PathBuf,
}

impl McpStdioFacade {
    /// Spawn `server_bin` with `project_root` as its working directory
    /// and perform the MCP initialize handshake.
    pub fn spawn(server_bin: &Path, project_root: &Path) -> Result<McpStdioFacade, FacadeError> {
        let mut child = Command::new(server_bin)
            .current_dir(project_root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| {
                FacadeError::Unavailable(format!("spawn {}: {e}", server_bin.display()))
            })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| FacadeError::Unavailable("server stdin missing".to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| FacadeError::Unavailable("server stdout missing".to_string()))?;
        let (tx, rx) = mpsc::channel::<String>();
        std::thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                match line {
                    Ok(text) => {
                        if tx.send(text).is_err() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
        });
        let mut facade = McpStdioFacade {
            child: Some(child),
            stdin: Some(stdin),
            lines: rx,
            next_id: 0,
            project_root: project_root.to_path_buf(),
            server_bin: server_bin.to_path_buf(),
        };
        // Modern-era handshake (fleet D-113; lumen src/guard.rs): there
        // is no `initialize` — it is refused by design (-32022). The
        // client opens with `server/discover`, and every request
        // carries the protocol version in its `_meta` (see `request`).
        facade.request("server/discover", json!({}))?;
        Ok(facade)
    }

    fn send(&mut self, payload: &Value) -> Result<(), FacadeError> {
        let text = payload.to_string();
        if text.len() > MCP_LINE_BYTES_MAX {
            return Err(FacadeError::Malformed("outgoing line over bound".into()));
        }
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| FacadeError::Unavailable("server stdin closed".into()))?;
        stdin
            .write_all(text.as_bytes())
            .and_then(|()| stdin.write_all(b"\n"))
            .and_then(|()| stdin.flush())
            .map_err(|e| FacadeError::Unavailable(format!("write to server: {e}")))
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value, FacadeError> {
        self.next_id += 1;
        let id = self.next_id;
        // Modern-era gate: every request's params must carry `_meta`
        // with the protocol version and client capabilities, or the
        // server answers -32602 before routing.
        let mut params = params;
        if !params.is_object() {
            params = json!({});
        }
        params["_meta"] = json!({
            "io.modelcontextprotocol/protocolVersion": "2026-07-28",
            "io.modelcontextprotocol/clientCapabilities": {},
            "io.modelcontextprotocol/clientInfo": {
                "name": "lumen-tui",
                "version": crate::TUI_VERSION
            }
        });
        self.send(&json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        let deadline = Duration::from_millis(MCP_DEADLINE_MS);
        loop {
            let line = self
                .lines
                .recv_timeout(deadline)
                .map_err(|_| FacadeError::Timeout)?;
            if line.len() > MCP_LINE_BYTES_MAX {
                return Err(FacadeError::Malformed("incoming line over bound".into()));
            }
            let msg: Value = serde_json::from_str(&line)
                .map_err(|e| FacadeError::Malformed(format!("non-JSON server line: {e}")))?;
            if msg.get("id").and_then(Value::as_u64) != Some(id) {
                continue; // A notification or a stale reply; keep waiting.
            }
            if let Some(err) = msg.get("error") {
                return Err(FacadeError::Malformed(format!("server error: {err}")));
            }
            return Ok(msg.get("result").cloned().unwrap_or(Value::Null));
        }
    }

    /// Extract the text payload of a tools/call result as JSON.
    fn call_result_value(result: &Value) -> Result<Value, FacadeError> {
        let text = result
            .pointer("/content/0/text")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                FacadeError::Malformed("tools/call result has no text content".into())
            })?;
        let parsed: Value = serde_json::from_str(text)
            .map_err(|e| FacadeError::Malformed(format!("tool text is not JSON: {e}")))?;
        validate_tool_result(&parsed)
    }
}

impl Drop for McpStdioFacade {
    fn drop(&mut self) {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

impl LumenFacade for McpStdioFacade {
    fn status(&mut self) -> Result<Value, FacadeError> {
        let tools = self.tools()?;
        let lane = self
            .call("lumen_status", json!({}))
            .unwrap_or_else(|e| json!({"generation_lane_error": e.to_string()}));
        Ok(json!({
            "member": crate::MEMBER_NAME,
            "version": crate::MEMBER_VERSION,
            "server_bin": self.server_bin.display().to_string(),
            "tool_count": tools.len(),
            "generation": lane,
        }))
    }

    fn tools(&mut self) -> Result<Vec<String>, FacadeError> {
        let result = self.request("tools/list", json!({}))?;
        let names = result
            .get("tools")
            .and_then(Value::as_array)
            .ok_or_else(|| FacadeError::Malformed("tools/list result has no tools array".into()))?
            .iter()
            .filter_map(|t| t.get("name").and_then(Value::as_str).map(str::to_string))
            .collect();
        Ok(names)
    }

    fn call(&mut self, tool: &str, args: Value) -> Result<Value, FacadeError> {
        let result = self.request("tools/call", json!({"name": tool, "arguments": args}))?;
        Self::call_result_value(&result)
    }

    fn identity_records(&mut self) -> Result<Vec<Value>, FacadeError> {
        scan_identity_records(&self.project_root)
    }

    fn factory_status(&mut self) -> Result<Value, FacadeError> {
        let records = scan_identity_records(&self.project_root)?;
        Ok(json!({
            "source": "filesystem (no MCP tool exposes factory state)",
            "identity_records": records.len(),
            "crates": ["lumen-canon", "lumen-factory", "lumen-rig-core"],
        }))
    }
}

/// Scan `root` (depth at most 2, bounded entries) for `*.identity.json`
/// files and summarize each as JSON. Read-only; unparseable files are
/// reported as entries with an `error` field, never a failure of the scan.
pub fn scan_identity_records(root: &Path) -> Result<Vec<Value>, FacadeError> {
    let mut records = Vec::new();
    let mut budget = SCAN_ENTRIES_MAX;
    let mut stack: Vec<(PathBuf, u32)> = vec![(root.to_path_buf(), 0)];
    while let Some((dir, depth)) = stack.pop() {
        let entries = match std::fs::read_dir(&dir) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in entries.flatten() {
            if budget == 0 || records.len() >= IDENTITY_RECORDS_MAX {
                return Ok(records);
            }
            budget -= 1;
            let path = entry.path();
            if path.is_dir() {
                if depth < 2 {
                    stack.push((path, depth + 1));
                }
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if name.ends_with(".identity.json") {
                records.push(summarize_identity(&path, &name));
            }
        }
    }
    records.sort_by_key(|r| {
        r.get("path")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    });
    Ok(records)
}

fn summarize_identity(path: &Path, name: &str) -> Value {
    let base = json!({"file": name, "path": path.display().to_string()});
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) => return merge_error(base, &format!("read: {e}")),
    };
    match serde_json::from_str::<Value>(&text) {
        Ok(parsed) => {
            let mut out = base;
            for key in ["name", "companion", "id", "character"] {
                if let Some(v) = parsed.get(key) {
                    out[key] = v.clone();
                }
            }
            out
        }
        Err(e) => merge_error(base, &format!("parse: {e}")),
    }
}

fn merge_error(mut base: Value, error: &str) -> Value {
    base["error"] = Value::String(error.to_string());
    base
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_accepts_objects() {
        assert!(validate_tool_result(&json!({"a": 1})).is_ok());
    }

    #[test]
    fn validate_parses_json_object_strings() {
        assert!(validate_tool_result(&Value::String("{\"a\":1}".into())).is_ok());
    }

    #[test]
    fn validate_rejects_malformed_results() {
        assert!(matches!(
            validate_tool_result(&json!([1, 2, 3])),
            Err(FacadeError::Malformed(_))
        ));
        assert!(matches!(
            validate_tool_result(&Value::String("not json".into())),
            Err(FacadeError::Malformed(_))
        ));
        assert!(matches!(
            validate_tool_result(&Value::Null),
            Err(FacadeError::Malformed(_))
        ));
    }

    #[test]
    fn fake_records_calls_in_order() {
        let mut fake = FakeFacade::new();
        let _ = fake.call("sprite_info", json!({"path": "a.png"}));
        let _ = fake.call("lumen_status", json!({}));
        assert_eq!(fake.calls.len(), 2);
        assert_eq!(fake.calls[0].0, "sprite_info");
        assert_eq!(fake.calls[1].0, "lumen_status");
    }

    #[test]
    fn fake_surfaces_scripted_malformed_result_as_error() {
        let mut fake = FakeFacade::new();
        fake.queue_call(Ok(json!("a bare string, not an object")));
        assert!(matches!(
            fake.call("sprite_info", json!({})),
            Err(FacadeError::Malformed(_))
        ));
    }

    #[test]
    fn scan_finds_identity_records_and_tolerates_bad_ones() {
        let dir = std::env::temp_dir().join(format!("lumen-tui-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("mkdir");
        std::fs::write(dir.join("lana.identity.json"), "{\"name\":\"Lana\"}").expect("write");
        std::fs::write(dir.join("broken.identity.json"), "{nope").expect("write");
        std::fs::write(dir.join("other.json"), "{}").expect("write");
        let records = scan_identity_records(&dir).expect("scan");
        assert_eq!(records.len(), 2);
        assert!(records.iter().any(|r| r["name"] == "Lana"));
        assert!(records.iter().any(|r| r.get("error").is_some()));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
