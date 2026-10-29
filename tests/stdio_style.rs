//! Stdio MCP surface tests for the style ops (D2, 2026-10-18).
//!
//! `style_list` / `style_apply` are registered on BOTH the A2A registry
//! and this stdio MCP server, wrapping the same library functions. These
//! tests drive the real `lumen` executable over stdio: the tool list must
//! include both ops, `style_list` must report all 15 presets (including
//! the demo_toon variants), and `style_apply` must grade end-to-end.
//! Roughly 50/50 validation/adversarial, per house test discipline.

use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use image::Rgba;
use lumen::doc::{load_doc, new_doc, save_doc};
use serde_json::{Value, json};

const STDIO_REPLY_TIMEOUT: Duration = Duration::from_secs(20);
const DOC: &str = "s.lumen.json";

/// The modern era's per-request metadata (protocol 2026-07-28):
/// every request carries its version and client identity in
/// `_meta`; sessions open with `server/discover`, never
/// `initialize` (D-113 migration, 2026-10-27).
fn modern_meta() -> Value {
    json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientInfo": {"name": "stdio-style-test", "version": "0"},
        "io.modelcontextprotocol/clientCapabilities": {}
    })
}

/// Minimal MCP stdio client: `server/discover`, then send each
/// (method, params) request in order with `_meta` stamped into
/// its params, returning the JSON-RPC replies in order.
fn stdio_requests(project: &std::path::Path, requests: &[(&str, Value)]) -> Vec<Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_lumen-mcp-server"))
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

fn write_fixture_doc(root: &std::path::Path) {
    let mut doc = new_doc(2, 1).expect("new doc");
    doc.layers[0]
        .image
        .put_pixel(0, 0, Rgba([200, 40, 90, 255]));
    doc.layers[0]
        .image
        .put_pixel(1, 0, Rgba([128, 128, 128, 255]));
    save_doc(root, &doc, DOC).expect("save fixture");
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

#[test]
fn stdio_tools_list_includes_style_ops() {
    let project = tempfile::tempdir().expect("tempdir");
    let replies = stdio_requests(project.path(), &[("tools/list", json!({}))]);
    let tools = replies[0]["result"]["tools"]
        .as_array()
        .expect("tools array");
    let mut names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "atelier_img2img",
            "atelier_models",
            "atelier_status",
            "atelier_txt2img",
            "bevy_discover",
            "canon_convert",
            "canon_gate",
            "canon_generate",
            "canon_resolve",
            "sprite_info",
            "style_apply",
            "style_list",
            "validate_atlas",
            "vrm_grade",
            "vrm_inspect"
        ]
    );
}

#[test]
fn stdio_style_list_reports_all_16_presets() {
    let project = tempfile::tempdir().expect("tempdir");
    let replies = stdio_requests(
        project.path(),
        &[("tools/call", json!({"name": "style_list", "arguments": {}}))],
    );
    let text = reply_text(&replies[0]);
    assert!(!text.contains("policy denied"), "{}", replies[0]);
    let body: Value = serde_json::from_str(&text).expect("style_list returns JSON");
    let styles = body["styles"].as_array().expect("styles array");
    assert_eq!(styles.len(), 16);
    let names: Vec<&str> = styles.iter().filter_map(|s| s["name"].as_str()).collect();
    for expected in [
        "demo_toon",
        "demo_toon_fire",
        "demo_toon_portrait",
        "ukiyo_e",
    ] {
        assert!(names.contains(&expected), "missing {expected}: {names:?}");
    }
}

#[test]
fn stdio_style_apply_demo_toon_grades_end_to_end() {
    let project = tempfile::tempdir().expect("tempdir");
    write_fixture_doc(project.path());
    let replies = stdio_requests(
        project.path(),
        &[(
            "tools/call",
            json!({"name": "style_apply", "arguments": {"doc": DOC, "style": "demo_toon", "strength": 1.0}}),
        )],
    );
    let text = reply_text(&replies[0]);
    assert!(!text.contains("error"), "apply failed: {}", replies[0]);
    let after = load_doc(project.path(), DOC).expect("load graded doc");
    let img = &after.layers[0].image;
    // Same pinned values as the library-level variant test.
    assert_eq!(img.get_pixel(0, 0), &Rgba([180, 24, 62, 255]));
    assert_eq!(img.get_pixel(1, 0), &Rgba([153, 153, 153, 255]));
}

// ---------------------------------------------------------------------------
// Adversarial
// ---------------------------------------------------------------------------

#[test]
fn stdio_style_apply_unknown_style_fails_closed() {
    let project = tempfile::tempdir().expect("tempdir");
    write_fixture_doc(project.path());
    let before = std::fs::read(project.path().join(DOC)).expect("read fixture");
    // An unregistered style name must fail closed through the stdio
    // surface, listing the valid names, and write nothing. (The probe was
    // demo_toon_fire until that preset landed on Matt's 2026-10-18 ruling.)
    let replies = stdio_requests(
        project.path(),
        &[(
            "tools/call",
            json!({"name": "style_apply", "arguments": {"doc": DOC, "style": "no_such_style", "strength": 1.0}}),
        )],
    );
    let text = reply_text(&replies[0]);
    assert!(text.contains("unknown style"), "{}", replies[0]);
    assert!(text.contains("demo_toon_portrait"), "{}", replies[0]);
    let after = std::fs::read(project.path().join(DOC)).expect("read after");
    assert_eq!(after, before, "rejected call must not write");
}

#[test]
fn stdio_style_apply_missing_doc_reports_error() {
    let project = tempfile::tempdir().expect("tempdir");
    let replies = stdio_requests(
        project.path(),
        &[(
            "tools/call",
            json!({"name": "style_apply", "arguments": {"doc": "nope.lumen.json", "style": "demo_toon", "strength": 1.0}}),
        )],
    );
    let text = reply_text(&replies[0]);
    assert!(!text.contains("policy denied"), "{}", replies[0]);
    assert!(text.contains("error"), "{}", replies[0]);
}
