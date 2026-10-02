//! Milestone 2 Phase B gate tests for the A2A agent layer.
//!
//! Count: 32 tests -- 16 validation (`ok_*`), 16 adversarial (`adv_*`).
//! Validation proves the binding does its job for honest clients (including
//! diver's exact wire shape). Adversarial proves it fails closed: malformed
//! JSON-RPC, unknown methods, oversized and smuggling-shaped HTTP, cancel of
//! missing/settled tasks, path escapes, and illegal state transitions.
//!
//! Slow steps come from a loopback mock BRP endpoint driven through the real
//! `bevy_status` operation, so cancellation and deadlines are exercised on
//! the same operation core MCP uses. No test mutates process-global state.

use std::net::SocketAddr;
use std::path::Path;
use std::time::Duration;

use lumen::a2a::{self, A2aConfig, Agent, SKILLS, TaskState, codes, find_skill};
use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn agent_with(root: &Path, deadline: Duration) -> (Agent, SocketAddr) {
    let mut config = A2aConfig::loopback(0, root.to_path_buf());
    config.task_deadline = deadline;
    let server = a2a::bind(config).await.expect("bind loopback");
    let (agent, addr) = (server.agent(), server.local_addr());
    tokio::spawn(server.serve());
    (agent, addr)
}

async fn agent(root: &Path) -> (Agent, SocketAddr) {
    agent_with(root, a2a::TASK_DEADLINE_DEFAULT).await
}

async fn rpc(agent: &Agent, method: &str, params: Value) -> Value {
    let body = json!({"jsonrpc": "2.0", "id": 7, "method": method, "params": params});
    agent.handle_rpc(body.to_string().as_bytes()).await
}

fn data_message(payload: Value) -> Value {
    json!({
        "kind": "message",
        "messageId": "test-msg",
        "role": "user",
        "parts": [{"kind": "data", "data": payload}],
    })
}

async fn send(agent: &Agent, payload: Value) -> Value {
    rpc(agent, "message/send", json!({"message": data_message(payload)})).await
}

async fn send_nonblocking(agent: &Agent, payload: Value) -> Value {
    let params = json!({"message": data_message(payload), "configuration": {"blocking": false}});
    rpc(agent, "message/send", params).await
}

async fn reply(agent: &Agent, task_id: &str, decision: Value) -> Value {
    let mut message = data_message(decision);
    message["taskId"] = json!(task_id);
    rpc(agent, "message/send", json!({"message": message})).await
}

fn state(envelope: &Value) -> &str {
    envelope.pointer("/result/status/state").and_then(Value::as_str).unwrap_or("<none>")
}

fn task_id(envelope: &Value) -> String {
    envelope.pointer("/result/id").and_then(Value::as_str).expect("task id").to_string()
}

fn error_code(envelope: &Value) -> i64 {
    envelope.pointer("/error/code").and_then(Value::as_i64).unwrap_or(0)
}

fn status_error(envelope: &Value) -> Value {
    let parts = envelope.pointer("/result/status/message/parts").and_then(Value::as_array);
    parts
        .into_iter()
        .flatten()
        .find_map(|p| p.pointer("/data/error").cloned())
        .unwrap_or(Value::Null)
}

fn new_sprite(output: &str) -> Value {
    json!({"skill": "new_sprite", "arguments": {
        "width": 8, "height": 8, "background": "#00000000", "output": output}})
}

/// A BRP endpoint that answers every request after `delay`.
async fn slow_brp(delay: Duration) -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("mock bind");
    let addr = listener.local_addr().expect("mock addr");
    tokio::spawn(async move {
        while let Ok((mut stream, _)) = listener.accept().await {
            tokio::spawn(async move {
                let mut buf = [0u8; 4096];
                let _ = stream.read(&mut buf).await;
                tokio::time::sleep(delay).await;
                let body = r#"{"jsonrpc":"2.0","id":1,"result":[]}"#;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\n\
                     Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes()).await;
            });
        }
    });
    format!("http://{addr}")
}

async fn wait_for_state(agent: &Agent, id: &str, want: &str) -> Value {
    for _ in 0..200 {
        let got = rpc(agent, "tasks/get", json!({"id": id})).await;
        if state(&got) == want {
            return got;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("task {id} never reached {want}");
}

/// Raw HTTP round trip. Returns (status, body).
async fn http_raw(addr: SocketAddr, request: &[u8]) -> (u16, String) {
    let mut stream = TcpStream::connect(addr).await.expect("connect");
    stream.write_all(request).await.expect("write");
    let mut out = Vec::new();
    let _ = tokio::time::timeout(Duration::from_secs(15), stream.read_to_end(&mut out)).await;
    let text = String::from_utf8_lossy(&out).to_string();
    let status = text.split(' ').nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let body = text.split_once("\r\n\r\n").map(|(_, b)| b.to_string()).unwrap_or_default();
    (status, body)
}

async fn http_post(addr: SocketAddr, body: &str) -> (u16, String) {
    let request = format!(
        "POST / HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\n\r\n{body}",
        body.len()
    );
    http_raw(addr, request.as_bytes()).await
}

// ---------------------------------------------------------------------------
// Validation (16)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn ok_card_is_served_and_generated_from_registry() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (_agent, addr) = agent(dir.path()).await;
    let request = "GET /.well-known/agent-card.json HTTP/1.1\r\nHost: localhost\r\n\r\n";
    let (status, body) = http_raw(addr, request.as_bytes()).await;
    assert_eq!(status, 200);
    assert!(body.len() < 256 * 1024, "diver's CARD_MAX_BYTES bound");
    let card: Value = serde_json::from_str(&body).expect("card json");
    assert_eq!(card["name"], "lumen");
    assert_eq!(card["url"], format!("http://{addr}/"));
    assert_eq!(card["supportedInterfaces"][0]["protocolBinding"], "JSONRPC");
    assert_eq!(card["capabilities"]["streaming"], true);
    let skills = card["skills"].as_array().expect("skills");
    assert_eq!(skills.len(), SKILLS.len());
    for skill in skills {
        let id = skill["id"].as_str().expect("id");
        assert!(find_skill(id).is_some(), "card skill {id} must dispatch");
    }
    let mut tags: Vec<&str> = skills.iter().filter_map(|s| s["tags"][0].as_str()).collect();
    tags.sort_unstable();
    tags.dedup();
    let mut want = vec![
        "sprite-core",
        "lightshow-pipeline",
        "fidelity",
        "styles",
        "dream-automation",
        "export-backends",
        "bevy-bridge",
    ];
    want.sort_unstable();
    assert_eq!(tags, want);
}

#[tokio::test(flavor = "multi_thread")]
async fn ok_message_send_runs_core_operation_and_completes() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (agent, _) = agent(dir.path()).await;
    let out = send(&agent, new_sprite("art/hero.lumen.json")).await;
    assert_eq!(state(&out), "completed", "{out}");
    let artifact = &out["result"]["artifacts"][0];
    assert_eq!(artifact["name"], "new_sprite");
    assert_eq!(artifact["parts"][0]["kind"], "data");
    assert_eq!(artifact["parts"][0]["data"]["path"], "art/hero.lumen.json");
    assert_eq!(artifact["parts"][1]["kind"], "file");
    assert_eq!(artifact["parts"][1]["file"]["uri"], "art/hero.lumen.json");
    // Same core as MCP: the document loads through the library.
    let doc = lumen::doc::load_doc(dir.path(), "art/hero.lumen.json").expect("load");
    assert_eq!((doc.width, doc.height), (8, 8));
}

#[tokio::test(flavor = "multi_thread")]
async fn ok_diver_text_part_wire_shape_is_accepted() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (agent, _) = agent(dir.path()).await;
    let message = json!({
        "kind": "message",
        "messageId": "diver-msg-1",
        "role": "user",
        "parts": [{"kind": "text", "text": new_sprite("d.lumen.json").to_string()}],
    });
    let out = rpc(&agent, "message/send", json!({"message": message})).await;
    assert_eq!(state(&out), "completed", "{out}");
    assert_eq!(out["result"]["kind"], "task");
    assert!(dir.path().join("d.lumen.json").is_file());
}

#[tokio::test(flavor = "multi_thread")]
async fn ok_multi_step_task_yields_one_artifact_per_step() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (agent, _) = agent(dir.path()).await;
    let payload = json!({"steps": [
        new_sprite("m.lumen.json"),
        {"skill": "fill_rect", "arguments": {"doc": "m.lumen.json", "layer": 0,
            "x": 0, "y": 0, "w": 4, "h": 4, "color": "#ff0000"}},
    ]});
    let out = send(&agent, payload).await;
    assert_eq!(state(&out), "completed", "{out}");
    let artifacts = out["result"]["artifacts"].as_array().expect("artifacts");
    assert_eq!(artifacts.len(), 2);
    assert!(artifacts[0]["artifactId"].as_str().expect("id").ends_with("-step-0"));
    assert!(artifacts[1]["artifactId"].as_str().expect("id").ends_with("-step-1"));
}

#[tokio::test(flavor = "multi_thread")]
async fn ok_tasks_get_returns_the_settled_task() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (agent, _) = agent(dir.path()).await;
    let out = send(&agent, json!({"skill": "style_list"})).await;
    let id = task_id(&out);
    let got = rpc(&agent, "tasks/get", json!({"id": id})).await;
    assert_eq!(got["result"], out["result"]);
}

#[tokio::test(flavor = "multi_thread")]
async fn ok_review_pauses_then_approval_resumes_the_same_task() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (agent, _) = agent(dir.path()).await;
    let payload = json!({"steps": [new_sprite("trial.lumen.json"), new_sprite("fleet.lumen.json")],
        "review_after": 0});
    let paused = send(&agent, payload).await;
    assert_eq!(state(&paused), "input-required", "{paused}");
    assert_eq!(paused["result"]["artifacts"].as_array().map(Vec::len), Some(1));
    assert!(!dir.path().join("fleet.lumen.json").exists(), "fleet waits for approval");
    let id = task_id(&paused);
    let done = reply(&agent, &id, json!({"decision": "approve"})).await;
    assert_eq!(state(&done), "completed", "{done}");
    assert_eq!(task_id(&done), id);
    assert_eq!(done["result"]["artifacts"].as_array().map(Vec::len), Some(2));
    assert!(dir.path().join("fleet.lumen.json").is_file());
}

#[tokio::test(flavor = "multi_thread")]
async fn ok_review_rejection_is_structured_and_keeps_trial_artifacts() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (agent, _) = agent(dir.path()).await;
    let payload = json!({"steps": [new_sprite("trial.lumen.json"), new_sprite("fleet.lumen.json")],
        "review_after": 0});
    let id = task_id(&send(&agent, payload).await);
    let reasons = json!({"decision": "reject", "reasons": ["eyes too large"]});
    let out = reply(&agent, &id, reasons).await;
    assert_eq!(state(&out), "rejected", "{out}");
    let err = status_error(&out);
    assert_eq!(err["code"], "review_rejected");
    assert_eq!(err["reasons"][0], "eyes too large");
    assert_eq!(out["result"]["artifacts"].as_array().map(Vec::len), Some(1));
    assert!(dir.path().join("trial.lumen.json").is_file(), "trial artifacts are kept");
    assert!(!dir.path().join("fleet.lumen.json").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn ok_cancel_lands_at_the_next_step_boundary() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (agent, _) = agent(dir.path()).await;
    let endpoint = slow_brp(Duration::from_millis(800)).await;
    let payload = json!({"steps": [
        {"skill": "bevy_status", "arguments": {"endpoint": endpoint}},
        new_sprite("never.lumen.json"),
    ]});
    let id = task_id(&send_nonblocking(&agent, payload).await);
    wait_for_state(&agent, &id, "working").await;
    let out = rpc(&agent, "tasks/cancel", json!({"id": id})).await;
    assert_eq!(state(&out), "canceled", "{out}");
    assert_eq!(status_error(&out)["code"], "canceled");
    // The in-flight step finished and kept its artifact; the next never ran.
    assert_eq!(out["result"]["artifacts"].as_array().map(Vec::len), Some(1));
    assert!(!dir.path().join("never.lumen.json").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn ok_cancel_of_a_paused_review_is_immediate() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (agent, _) = agent(dir.path()).await;
    let payload = json!({"steps": [{"skill": "style_list"}], "review_after": 0});
    let id = task_id(&send(&agent, payload).await);
    let out = rpc(&agent, "tasks/cancel", json!({"id": id})).await;
    assert_eq!(state(&out), "canceled", "{out}");
    assert_eq!(status_error(&out)["code"], "canceled");
}

#[tokio::test(flavor = "multi_thread")]
async fn ok_message_stream_emits_sse_progress_then_final() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (_agent, addr) = agent(dir.path()).await;
    let body = json!({"jsonrpc": "2.0", "id": "s1", "method": "message/stream",
        "params": {"message": data_message(new_sprite("s.lumen.json"))}});
    let (status, sse) = http_post(addr, &body.to_string()).await;
    assert_eq!(status, 200);
    let events: Vec<Value> = sse
        .split("\n\n")
        .filter_map(|e| e.strip_prefix("data: "))
        .map(|d| serde_json::from_str(d).expect("event json"))
        .collect();
    assert!(events.len() >= 2, "{sse}");
    assert!(events.iter().all(|e| e["id"] == "s1"));
    assert_eq!(events[0]["result"]["kind"], "task");
    let kinds: Vec<&str> = events.iter().filter_map(|e| e["result"]["kind"].as_str()).collect();
    assert!(kinds.contains(&"artifact-update"), "{kinds:?}");
    let last = events.last().expect("final");
    assert_eq!(last["result"]["kind"], "status-update");
    assert_eq!(last["result"]["final"], true);
    assert_eq!(last["result"]["status"]["state"], "completed");
}

#[tokio::test(flavor = "multi_thread")]
async fn ok_nonblocking_send_returns_early_and_settles_later() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (agent, _) = agent(dir.path()).await;
    let endpoint = slow_brp(Duration::from_millis(300)).await;
    let payload = json!({"skill": "bevy_status", "arguments": {"endpoint": endpoint}});
    let first = send_nonblocking(&agent, payload).await;
    assert!(matches!(state(&first), "submitted" | "working"), "{first}");
    let done = wait_for_state(&agent, &task_id(&first), "completed").await;
    assert_eq!(done["result"]["artifacts"][0]["parts"][0]["data"]["speaks_brp"], true);
}

#[tokio::test(flavor = "multi_thread")]
async fn ok_failed_task_carries_a_structured_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (agent, _) = agent(dir.path()).await;
    let payload = json!({"skill": "fill_rect", "arguments": {"doc": "missing.lumen.json",
        "layer": 0, "x": 0, "y": 0, "w": 1, "h": 1, "color": "#fff"}});
    let out = send(&agent, payload).await;
    assert_eq!(state(&out), "failed");
    let err = status_error(&out);
    assert_eq!(err["code"], "operation_failed");
    assert_eq!(err["step"], 0);
    assert_eq!(err["skill"], "fill_rect");
    assert!(err["message"].as_str().is_some_and(|m| !m.is_empty()));
}

#[tokio::test(flavor = "multi_thread")]
async fn ok_task_deadline_fails_a_slow_step_with_timeout() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (agent, _) = agent_with(dir.path(), Duration::from_millis(200)).await;
    let endpoint = slow_brp(Duration::from_secs(3)).await;
    let out = send(&agent, json!({"skill": "bevy_status", "arguments": {"endpoint": endpoint}}))
        .await;
    assert_eq!(state(&out), "failed", "{out}");
    assert_eq!(status_error(&out)["code"], "timeout");
}

#[test]
fn ok_state_machine_accepts_every_documented_edge() {
    use TaskState::*;
    let legal = [
        (Submitted, Working),
        (Submitted, Canceled),
        (Submitted, Failed),
        (Working, InputRequired),
        (Working, Completed),
        (Working, Failed),
        (Working, Canceled),
        (InputRequired, Working),
        (InputRequired, Completed),
        (InputRequired, Rejected),
        (InputRequired, Canceled),
        (InputRequired, Failed),
    ];
    for (from, to) in legal {
        assert!(from.can_transition_to(to), "{} -> {}", from.as_str(), to.as_str());
    }
    assert_eq!(InputRequired.as_str(), "input-required");
}

#[test]
fn ok_artifact_paths_are_project_relative() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("a/b")).expect("mkdir");
    std::fs::write(dir.path().join("a/b/x.png"), b"x").expect("write");
    let abs = dir.path().join("a/b/x.png");
    assert_eq!(a2a::artifact_relative_path(dir.path(), &abs).expect("rel"), "a/b/x.png");
    let rel = Path::new("a/./b/x.png");
    assert_eq!(a2a::artifact_relative_path(dir.path(), rel).expect("rel"), "a/b/x.png");
}

#[tokio::test(flavor = "multi_thread")]
async fn ok_context_id_is_echoed_and_kebab_states_on_the_wire() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (agent, _) = agent(dir.path()).await;
    let mut message = data_message(json!({"skill": "style_list"}));
    message["contextId"] = json!("ondine-trial");
    let out = rpc(&agent, "message/send", json!({"message": message})).await;
    assert_eq!(out["result"]["contextId"], "ondine-trial");
    assert_eq!(out["jsonrpc"], "2.0");
    assert_eq!(out["id"], 7);
    assert_eq!(state(&out), "completed");
}

// ---------------------------------------------------------------------------
// Adversarial (16)
// ---------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn adv_malformed_json_is_a_parse_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (agent, _) = agent(dir.path()).await;
    for body in [&b"{not json"[..], b"", b"\xff\xfe", &[b'['; 10_000][..]] {
        let out = agent.handle_rpc(body).await;
        assert_eq!(error_code(&out), codes::PARSE_ERROR, "{out}");
        assert_eq!(out["id"], Value::Null);
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn adv_invalid_envelopes_are_invalid_requests() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (agent, _) = agent(dir.path()).await;
    let long_id = "x".repeat(4096);
    let bad = [
        json!([{"jsonrpc": "2.0", "id": 1, "method": "tasks/get"}]),
        json!({"id": 1, "method": "tasks/get"}),
        json!({"jsonrpc": "1.0", "id": 1, "method": "tasks/get"}),
        json!({"jsonrpc": "2.0", "method": "tasks/get"}),
        json!({"jsonrpc": "2.0", "id": {"a": 1}, "method": "tasks/get"}),
        json!({"jsonrpc": "2.0", "id": long_id, "method": "tasks/get"}),
        json!({"jsonrpc": "2.0", "id": 1, "method": 5}),
        json!("tasks/get"),
    ];
    for body in bad {
        let out = agent.handle_rpc(body.to_string().as_bytes()).await;
        assert_eq!(error_code(&out), codes::INVALID_REQUEST, "{body} -> {out}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn adv_unknown_and_unsupported_methods_are_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (agent, _) = agent(dir.path()).await;
    let out = rpc(&agent, "tasks/explode", json!({})).await;
    assert_eq!(error_code(&out), codes::METHOD_NOT_FOUND);
    let out = rpc(&agent, "tasks/pushNotificationConfig/set", json!({})).await;
    assert_eq!(error_code(&out), codes::PUSH_NOT_SUPPORTED);
    let out = rpc(&agent, "tasks/resubscribe", json!({"id": "lumen-task-0"})).await;
    assert_eq!(error_code(&out), codes::UNSUPPORTED_OPERATION);
    // Raw Lua is not reachable over A2A, by any name.
    assert!(find_skill("run_lua_script").is_none());
    let out = send(&agent, json!({"skill": "run_lua_script",
        "arguments": {"doc": "x.lumen.json", "script": "", "opt_in": true}})).await;
    assert_eq!(error_code(&out), codes::INVALID_PARAMS);
}

#[tokio::test(flavor = "multi_thread")]
async fn adv_oversized_body_is_refused_before_it_is_read() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (_agent, addr) = agent(dir.path()).await;
    let head = "POST / HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\n\
                Content-Length: 10485760\r\n\r\n";
    let started = std::time::Instant::now();
    let (status, _) = http_raw(addr, head.as_bytes()).await;
    assert_eq!(status, 413);
    assert!(started.elapsed() < Duration::from_secs(5), "no wait for the body");
    let huge = format!("GET / HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Pad: {}", "a".repeat(20_000));
    let (status, _) = http_raw(addr, huge.as_bytes()).await;
    assert_eq!(status, 431);
}

#[tokio::test(flavor = "multi_thread")]
async fn adv_smuggling_shaped_requests_are_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (_agent, addr) = agent(dir.path()).await;
    let cases: [(&str, u16); 5] = [
        ("POST / HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\n\
          Transfer-Encoding: chunked\r\n\r\n0\r\n\r\n", 501),
        ("POST / HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\n\
          Content-Length: 2\r\nContent-Length: 2\r\n\r\n{}", 400),
        ("POST / HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\n\r\n", 411),
        ("POST / HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\n\
          Content-Length: -1\r\n\r\n", 400),
        ("POST / HTTP/2\r\nHost: 127.0.0.1\r\n\r\n", 505),
    ];
    for (request, want) in cases {
        let (status, _) = http_raw(addr, request.as_bytes()).await;
        assert_eq!(status, want, "{request:?}");
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn adv_browser_shaped_and_rebinding_requests_are_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (_agent, addr) = agent(dir.path()).await;
    let body = json!({"jsonrpc": "2.0", "id": 1, "method": "message/send",
        "params": {"message": data_message(new_sprite("pwn.lumen.json"))}})
    .to_string();
    // DNS rebinding: a loopback socket reached under an attacker's name.
    let rebound = format!(
        "POST / HTTP/1.1\r\nHost: evil.example:80\r\nContent-Type: application/json\r\n\
         Content-Length: {}\r\n\r\n{body}",
        body.len()
    );
    assert_eq!(http_raw(addr, rebound.as_bytes()).await.0, 403);
    // Cross-origin "simple request" (no preflight): text/plain is refused.
    let simple = format!(
        "POST / HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: text/plain\r\n\
         Content-Length: {}\r\n\r\n{body}",
        body.len()
    );
    assert_eq!(http_raw(addr, simple.as_bytes()).await.0, 415);
    let preflight = "OPTIONS / HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n";
    assert_eq!(http_raw(addr, preflight.as_bytes()).await.0, 405);
    assert!(!dir.path().join("pwn.lumen.json").exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn adv_cancel_or_get_of_a_nonexistent_task_is_not_found() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (agent, _) = agent(dir.path()).await;
    let overflow = format!("lumen-task-{}", "9".repeat(25));
    let ids = ["lumen-task-999", "lumen-task-", "lumen-task-01x", "../../etc", &overflow];
    for id in ids {
        let out = rpc(&agent, "tasks/cancel", json!({"id": id})).await;
        assert_eq!(error_code(&out), codes::TASK_NOT_FOUND, "{id}");
        let out = rpc(&agent, "tasks/get", json!({"id": id})).await;
        assert_eq!(error_code(&out), codes::TASK_NOT_FOUND, "{id}");
    }
    let out = rpc(&agent, "tasks/cancel", json!({"id": 5})).await;
    assert_eq!(error_code(&out), codes::INVALID_PARAMS);
    let out = rpc(&agent, "tasks/cancel", json!({})).await;
    assert_eq!(error_code(&out), codes::INVALID_PARAMS);
}

#[tokio::test(flavor = "multi_thread")]
async fn adv_cancel_of_a_settled_task_is_not_cancelable() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (agent, _) = agent(dir.path()).await;
    let done = send(&agent, json!({"skill": "style_list"})).await;
    let id = task_id(&done);
    let out = rpc(&agent, "tasks/cancel", json!({"id": id})).await;
    assert_eq!(error_code(&out), codes::TASK_NOT_CANCELABLE, "{out}");
    let again = rpc(&agent, "tasks/get", json!({"id": id})).await;
    assert_eq!(state(&again), "completed", "a refused cancel changes nothing");
}

#[tokio::test(flavor = "multi_thread")]
async fn adv_path_escape_in_task_arguments_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("project");
    std::fs::create_dir_all(&root).expect("mkdir");
    let (agent, _) = agent(&root).await;
    let outside = dir.path().join("escape.lumen.json");
    let abs = outside.to_str().expect("utf8").to_string();
    for output in ["../escape.lumen.json", abs.as_str(), "a/../../escape.lumen.json"] {
        let out = send(&agent, new_sprite(output)).await;
        assert_eq!(state(&out), "failed", "{output}: {out}");
        assert_eq!(status_error(&out)["code"], "operation_failed");
        assert_eq!(out["result"]["artifacts"].as_array().map(Vec::len), Some(0));
    }
    assert!(!outside.exists(), "nothing written outside the root");
}

#[tokio::test(flavor = "multi_thread")]
async fn adv_symlink_escape_is_refused_for_writes_and_artifacts() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("project");
    let outside = dir.path().join("outside");
    std::fs::create_dir_all(&root).expect("mkdir root");
    std::fs::create_dir_all(&outside).expect("mkdir outside");
    std::os::unix::fs::symlink(&outside, root.join("link")).expect("symlink");
    let (agent, _) = agent(&root).await;
    let out = send(&agent, new_sprite("link/x.lumen.json")).await;
    assert_eq!(state(&out), "failed", "{out}");
    assert!(!outside.join("x.lumen.json").exists());
    // The artifact bound itself also refuses escapes and the root.
    std::fs::write(outside.join("secret.png"), b"s").expect("write");
    let err = a2a::artifact_relative_path(&root, &outside.join("secret.png")).unwrap_err();
    assert!(err.to_string().contains("escapes"), "{err}");
    assert!(a2a::artifact_relative_path(&root, Path::new("link/secret.png")).is_err());
    assert!(a2a::artifact_relative_path(&root, Path::new("..")).is_err());
    assert!(a2a::artifact_relative_path(&root, &root).is_err());
}

#[test]
fn adv_illegal_state_transitions_are_refused() {
    use TaskState::*;
    let all = [Submitted, Working, InputRequired, Completed, Failed, Canceled, Rejected];
    for terminal in [Completed, Failed, Canceled, Rejected] {
        assert!(terminal.is_terminal());
        for next in all {
            assert!(!terminal.can_transition_to(next), "{} is terminal", terminal.as_str());
        }
    }
    let illegal = [
        (Submitted, Completed),
        (Submitted, Rejected),
        (Submitted, InputRequired),
        (Submitted, Submitted),
        (Working, Submitted),
        (Working, Working),
        (Working, Rejected),
        (InputRequired, Submitted),
        (InputRequired, InputRequired),
    ];
    for (from, to) in illegal {
        assert!(!from.can_transition_to(to), "{} -> {}", from.as_str(), to.as_str());
    }
}

#[tokio::test(flavor = "multi_thread")]
async fn adv_review_reply_to_a_task_not_awaiting_input_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (agent, _) = agent(dir.path()).await;
    let id = task_id(&send(&agent, json!({"skill": "style_list"})).await);
    let out = reply(&agent, &id, json!({"decision": "approve"})).await;
    assert_eq!(error_code(&out), codes::INVALID_PARAMS, "{out}");
    let still = rpc(&agent, "tasks/get", json!({"id": id})).await;
    assert_eq!(state(&still), "completed");
    let out = reply(&agent, "lumen-task-4242", json!({"decision": "approve"})).await;
    assert_eq!(error_code(&out), codes::TASK_NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread")]
async fn adv_bad_task_payloads_are_refused_without_creating_tasks() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (agent, _) = agent(dir.path()).await;
    let seventeen: Vec<Value> = (0..17).map(|_| json!({"skill": "style_list"})).collect();
    let payloads = [
        json!({"skill": "no_such_skill"}),
        json!({"skill": "style_list", "steps": [{"skill": "style_list"}]}),
        json!({"steps": []}),
        json!({"steps": seventeen}),
        json!({"steps": [{"skill": "style_list"}], "review_after": 1}),
        json!({"skill": "style_list", "arguments": [1, 2]}),
        json!({"skill": "style_list", "surprise": true}),
    ];
    for payload in payloads {
        let out = send(&agent, payload.clone()).await;
        assert_eq!(error_code(&out), codes::INVALID_PARAMS, "{payload} -> {out}");
    }
    let messages = [
        json!({"role": "agent", "parts": [{"kind": "data", "data": {"skill": "style_list"}}]}),
        json!({"role": "USER", "parts": [{"kind": "data", "data": {"skill": "style_list"}}]}),
        json!({"role": "user", "parts": [{"kind": "text", "text": "draw me a dragon"}]}),
        json!({"role": "user", "parts": [{"kind": "file", "file": {"uri": "file:///etc/passwd"}}]}),
        json!({"role": "user", "parts": []}),
        json!({"role": "user", "parts": [{"kind": "text", "text": "{}"}, {"kind": "text",
            "text": "{}"}]}),
        json!({"role": "user", "kind": "task", "parts": [{"kind": "text", "text": "{}"}]}),
    ];
    for message in messages {
        let out = rpc(&agent, "message/send", json!({"message": message})).await;
        assert_eq!(error_code(&out), codes::INVALID_PARAMS, "{message} -> {out}");
    }
    let out = rpc(&agent, "tasks/get", json!({"id": "lumen-task-0"})).await;
    assert_eq!(error_code(&out), codes::TASK_NOT_FOUND, "no task was created");
}

#[tokio::test(flavor = "multi_thread")]
async fn adv_live_task_bound_is_enforced_and_frees_on_settle() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (agent, _) = agent(dir.path()).await;
    let paused = json!({"steps": [{"skill": "style_list"}], "review_after": 0});
    let mut ids = Vec::new();
    for _ in 0..16 {
        let out = send(&agent, paused.clone()).await;
        assert_eq!(state(&out), "input-required", "{out}");
        ids.push(task_id(&out));
    }
    let out = send(&agent, paused.clone()).await;
    assert_eq!(error_code(&out), codes::SERVER_BUSY, "{out}");
    let canceled = rpc(&agent, "tasks/cancel", json!({"id": ids[0]})).await;
    assert_eq!(state(&canceled), "canceled");
    let out = send(&agent, paused).await;
    assert_eq!(state(&out), "input-required", "a settled task frees a slot");
}

#[tokio::test(flavor = "multi_thread")]
async fn adv_bind_refuses_non_loopback_and_bad_deadlines() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut open = A2aConfig::loopback(0, dir.path().to_path_buf());
    open.bind = "0.0.0.0:0".parse().expect("addr");
    assert!(a2a::bind(open).await.is_err(), "0.0.0.0 must be refused");
    let mut zero = A2aConfig::loopback(0, dir.path().to_path_buf());
    zero.task_deadline = Duration::ZERO;
    assert!(a2a::bind(zero).await.is_err());
    let mut huge = A2aConfig::loopback(0, dir.path().to_path_buf());
    huge.task_deadline = a2a::TASK_DEADLINE_MAX + Duration::from_secs(1);
    assert!(a2a::bind(huge).await.is_err());
    let missing = A2aConfig::loopback(0, dir.path().join("does-not-exist"));
    assert!(a2a::bind(missing).await.is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn adv_decision_payload_bounds_are_enforced() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (agent, _) = agent(dir.path()).await;
    let paused = json!({"steps": [{"skill": "style_list"}], "review_after": 0});
    let id = task_id(&send(&agent, paused).await);
    let many: Vec<String> = (0..17).map(|i| format!("r{i}")).collect();
    let bad = [
        json!({"decision": "reject", "reasons": many}),
        json!({"decision": "reject", "reasons": ["x".repeat(2000)]}),
        json!({"decision": "approve", "reasons": ["sneaky"]}),
        json!({"decision": "APPROVE"}),
        json!({"decision": "approve", "skill": "new_sprite"}),
    ];
    for decision in bad {
        let out = reply(&agent, &id, decision.clone()).await;
        assert_eq!(error_code(&out), codes::INVALID_PARAMS, "{decision} -> {out}");
    }
    let still = rpc(&agent, "tasks/get", json!({"id": id})).await;
    assert_eq!(state(&still), "input-required", "refused replies leave the task paused");
}
