//! Era gates (WS2, D-113): the migrated server speaks the modern
//! MCP era only (2026-07-28). Legacy contact is refused with the
//! designed typed errors and the server survives every refusal;
//! framing failures follow the fleet guard contract (see
//! `lumen::guard`). Frames are raw strings and responses are
//! correlated by JSON-RPC id (rmcp dispatches pipelined requests
//! concurrently).

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

const META: &str = r#""_meta": {"io.modelcontextprotocol/protocolVersion": "2026-07-28", "io.modelcontextprotocol/clientInfo": {"name": "era-test", "version": "0"}, "io.modelcontextprotocol/clientCapabilities": {}}"#;
const LEGACY_META: &str = r#""_meta": {"io.modelcontextprotocol/protocolVersion": "2025-06-18", "io.modelcontextprotocol/clientInfo": {"name": "era-test", "version": "0"}, "io.modelcontextprotocol/clientCapabilities": {}}"#;

fn lumen_bin() -> &'static str {
    env!("CARGO_BIN_EXE_lumen-mcp-server")
}

fn temp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "lumen-era-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

struct RawServer {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl RawServer {
    fn spawn(project: &std::path::Path) -> RawServer {
        let mut child = Command::new(lumen_bin())
            .env_remove("LUMEN_POLICY")
            .env_remove("LUMEN_ALLOW_REMOTE_BRP")
            // The folded generation lane reads a default config from
            // $HOME; keep stdio spawns hermetic (atelier fold, D-143).
            .env_remove("ATELIER_CONFIG")
            .env("HOME", project)
            .env("LUMEN_PROJECT_ROOT", project)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn lumen server");
        let stdin = child.stdin.take().expect("server stdin");
        let stdout = BufReader::new(child.stdout.take().expect("server stdout"));
        RawServer {
            child,
            stdin,
            stdout,
        }
    }

    /// Send raw lines; read exactly `expect` frames. Returns them
    /// in arrival order (callers correlate by id).
    fn exchange(&mut self, lines: &[String], expect: usize) -> Vec<serde_json::Value> {
        for line in lines {
            self.stdin.write_all(line.as_bytes()).expect("write frame");
            self.stdin.write_all(b"\n").expect("write newline");
        }
        self.stdin.flush().expect("flush");
        let mut frames = Vec::new();
        for _ in 0..expect {
            let mut line = String::new();
            let read = self.stdout.read_line(&mut line).expect("read frame");
            assert!(read > 0, "server closed stdout unexpectedly");
            frames.push(serde_json::from_str(&line).expect("server frame is JSON"));
        }
        frames
    }

    fn shutdown(mut self) {
        drop(self.stdin);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            match self.child.try_wait() {
                Ok(Some(_)) => return,
                Ok(None) => {
                    assert!(
                        std::time::Instant::now() < deadline,
                        "server did not exit within 20 s of transport EOF"
                    );
                    std::thread::sleep(std::time::Duration::from_millis(25));
                }
                Err(error) => panic!("waiting on server: {error}"),
            }
        }
    }
}

fn by_id(frames: &[serde_json::Value], id: i64) -> &serde_json::Value {
    frames
        .iter()
        .find(|f| f.get("id").and_then(serde_json::Value::as_i64) == Some(id))
        .unwrap_or_else(|| panic!("no frame with id {id} in {frames:?}"))
}

fn discover(id: i64) -> String {
    format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"server/discover","params":{{{META}}}}}"#)
}

fn initialize(id: i64, version: &str) -> String {
    format!(
        r#"{{"jsonrpc":"2.0","id":{id},"method":"initialize","params":{{"protocolVersion":"{version}","clientInfo":{{"name":"legacy-probe","version":"0"}},"capabilities":{{}}}}}}"#
    )
}

#[test]
fn discover_advertises_only_the_modern_era() {
    let project = temp_dir("discover");
    let mut server = RawServer::spawn(&project);
    let frames = server.exchange(&[discover(1)], 1);
    let result = &by_id(&frames, 1)["result"];
    assert_eq!(
        result["supportedVersions"],
        serde_json::json!(["2026-07-28"]),
        "discover result: {result}"
    );
    assert_eq!(
        result["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
        "lumen"
    );
    server.shutdown();
}

#[test]
fn legacy_initialize_is_refused_and_the_server_survives() {
    let project = temp_dir("legacy");
    let mut server = RawServer::spawn(&project);
    for (index, version) in ["2024-11-05", "2025-06-18", "2025-11-25", "2026-07-28"]
        .iter()
        .enumerate()
    {
        let id = i64::try_from(index).expect("small") + 1;
        let frames = server.exchange(&[initialize(id, version)], 1);
        let error = &by_id(&frames, id)["error"];
        assert_eq!(error["code"], -32022, "initialize {version}: {error}");
        assert_eq!(error["data"]["requested"], *version);
        assert_eq!(
            error["data"]["supported"],
            serde_json::json!(["2026-07-28"])
        );
    }
    // After every refusal the same process still serves the
    // modern era.
    let frames = server.exchange(&[discover(9)], 1);
    assert!(
        by_id(&frames, 9).get("result").is_some(),
        "discover after refusals: {frames:?}"
    );
    server.shutdown();
}

#[test]
fn tools_list_requires_modern_meta() {
    let project = temp_dir("meta");
    let mut server = RawServer::spawn(&project);
    let no_meta = r#"{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}"#.to_string();
    let legacy_meta =
        format!(r#"{{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{{{LEGACY_META}}}}}"#);
    let frames = server.exchange(&[no_meta, legacy_meta], 2);
    // A first request without _meta meets the SDK's metadata gate.
    assert_eq!(by_id(&frames, 1)["error"]["code"], -32602, "{frames:?}");
    // A legacy-version _meta meets the version refusal.
    assert_eq!(by_id(&frames, 2)["error"]["code"], -32022, "{frames:?}");
    server.shutdown();
}

#[test]
fn framing_errors_follow_the_guard_contract() {
    let project = temp_dir("framing");
    let mut server = RawServer::spawn(&project);
    let frames = server.exchange(
        &[
            "this is not json".to_string(),
            "[1,2,3]".to_string(),
            discover(3),
        ],
        3,
    );
    // The parse error answers first (id null), then the
    // non-object refusal (id null), then the discovery — the
    // guard answers in request order.
    assert_eq!(frames[0]["error"]["code"], -32700, "{frames:?}");
    assert_eq!(frames[0]["id"], serde_json::Value::Null);
    let message = frames[0]["error"]["message"].as_str().expect("message");
    assert!(message.starts_with("parse error: "), "{message}");
    assert_eq!(frames[1]["error"]["code"], -32600, "{frames:?}");
    assert!(by_id(&frames, 3).get("result").is_some(), "{frames:?}");
    server.shutdown();
}

#[test]
fn notifications_are_dropped_unexecuted() {
    let project = temp_dir("notifications");
    let mut server = RawServer::spawn(&project);
    let frames = server.exchange(
        &[
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#.to_string(),
            r#"{"jsonrpc":"2.0","method":"tools/call","params":{"name":"style_list","arguments":{}}}"#
                .to_string(),
            discover(1),
        ],
        1,
    );
    // The only frame back answers the discovery: neither
    // notification produced output (or execution evidence).
    assert_eq!(frames.len(), 1);
    assert!(by_id(&frames, 1).get("result").is_some(), "{frames:?}");
    server.shutdown();
}
