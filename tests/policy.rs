//! Policy layer tests: 12 validation (`ok_*`), 12 adversarial (`adv_*`).
//!
//! Binary-level tests drive the real `lumen` executable: the policy CLI
//! subcommands, and the MCP stdio server (proving enforcement happens in the
//! router wrapper, before the tool runs).

use std::collections::BTreeSet;
use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

use lumen::policy::{
    self, CallEvent, DEFAULT_POLICY_TOML, Decision, Enforcer, LIBRARY_TOOLS, POLICY_BYTES_MAX,
    Policy, PolicyError, RULE_DENY_BY_DEFAULT, SCENARIOS, Sensitivity, Session, Trust,
    classify_brp_endpoint,
};
use serde_json::{Map, Value, json};

const REGISTERED: &[&str] = &["sprite_info", "validate_atlas", "bevy_discover"];
const STDIO_REPLY_TIMEOUT: Duration = Duration::from_secs(20);

fn default_policy() -> Policy {
    Policy::from_toml_str(DEFAULT_POLICY_TOML).expect("default policy loads")
}

fn args(v: Value) -> Map<String, Value> {
    match v {
        Value::Object(m) => m,
        _ => panic!("test args must be an object"),
    }
}

fn denied_rule(r: Result<CallEvent, policy::Denial>) -> String {
    r.expect_err("expected a denial").rule
}

fn policy_without_sprite_info_labels() -> String {
    let block = "[tools.sprite_info]\nreads = [\"low\"]\nsink = \"trusted\"\n";
    assert!(DEFAULT_POLICY_TOML.contains(block), "fixture drifted from lumen-policy.toml");
    DEFAULT_POLICY_TOML.replace(block, "")
}

/// R1 loosened: high may now reach public/external through the deny list.
fn loosened_policy() -> String {
    let r1_sinks = "sinks = [\"public\", \"external\"]";
    assert_eq!(DEFAULT_POLICY_TOML.matches(r1_sinks).count(), 1, "fixture drifted");
    DEFAULT_POLICY_TOML.replace(r1_sinks, "sinks = [\"remote\"]")
}

fn run_cli(subcommand: &str, policy_file: Option<&std::path::Path>) -> (i32, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_lumen"));
    cmd.args(["policy", subcommand]).env_remove("LUMEN_POLICY");
    if let Some(path) = policy_file {
        cmd.env("LUMEN_POLICY", path);
    }
    let out = cmd.output().expect("spawn lumen");
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    println!("--- lumen policy {subcommand} ---\n{stdout}");
    (out.status.code().unwrap_or(-1), stdout)
}

fn write_policy(dir: &tempfile::TempDir, text: &str) -> std::path::PathBuf {
    let path = dir.path().join("lumen-policy.toml");
    std::fs::write(&path, text).expect("write policy");
    path
}

/// Minimal MCP stdio client: initialize, then one tools/call per item.
/// Returns the JSON-RPC responses for the tool calls, in order.
fn stdio_session(project: &std::path::Path, calls: &[(&str, Value)]) -> Vec<Value> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_lumen"))
        .env_remove("LUMEN_POLICY")
        .env_remove("LUMEN_ALLOW_REMOTE_BRP")
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
            let line = rx.recv_timeout(STDIO_REPLY_TIMEOUT).expect("server reply in time");
            let v: Value = serde_json::from_str(&line).expect("server speaks json");
            if v.get("id") == Some(&json!(id)) {
                return v;
            }
        }
    };
    send(json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{
        "protocolVersion":"2025-06-18","capabilities":{},
        "clientInfo":{"name":"policy-test","version":"0"}}}));
    let init = recv_id(0);
    assert!(init.get("result").is_some(), "initialize failed: {init}");
    send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    let mut replies = Vec::new();
    for (index, (tool, arguments)) in calls.iter().enumerate() {
        let id = u64::try_from(index).expect("small") + 1;
        send(json!({"jsonrpc":"2.0","id":id,"method":"tools/call",
            "params":{"name":tool,"arguments":arguments}}));
        replies.push(recv_id(id));
    }
    drop(send);
    drop(stdin);
    // Best-effort teardown: the server may already have exited on stdin EOF.
    let _ = child.kill();
    let _ = child.wait();
    replies
}

fn reply_text(reply: &Value) -> String {
    reply["result"]["content"][0]["text"].as_str().unwrap_or_default().to_string()
}

// ---------------------------------------------------------------------------
// Validation
// ---------------------------------------------------------------------------

#[test]
fn ok_default_policy_passes_check_for_registered_and_library_tools() {
    let report = policy::check(&default_policy(), REGISTERED);
    assert!(report.problems.is_empty(), "{:?}", report.problems);
    assert_eq!(report.required_tools, REGISTERED.len() + LIBRARY_TOOLS.len());
    assert_eq!(report.labeled_tools, report.required_tools);
    assert_eq!((report.deny_rules, report.allow_rules), (3, 4));
}

#[test]
fn ok_replay_suite_passes_and_is_balanced() {
    let outcomes = policy::replay(&default_policy(), SCENARIOS);
    let failed: Vec<_> = outcomes.iter().filter(|o| !o.passed).collect();
    assert!(failed.is_empty(), "{failed:?}");
    let ok = SCENARIOS.iter().filter(|s| s.id.starts_with("ok_")).count();
    let adv = SCENARIOS.iter().filter(|s| s.id.starts_with("adv_")).count();
    assert_eq!(ok + adv, SCENARIOS.len(), "every scenario is ok_ or adv_");
    assert_eq!(ok, adv, "50/50 split");
    let ids: BTreeSet<_> = SCENARIOS.iter().map(|s| s.id).collect();
    assert_eq!(ids.len(), SCENARIOS.len(), "scenario ids are unique");
}

#[test]
fn ok_decisions_are_deterministic_across_parses_and_runs() {
    let a = default_policy();
    let b = default_policy();
    assert_eq!(a, b);
    let event = CallEvent {
        tool: "cart_export_wasm4".to_string(),
        labels: [Sensitivity::Low, Sensitivity::High, Sensitivity::Untrusted].into(),
        sink: Trust::Public,
    };
    let first = policy::decide(&a, &event);
    for _ in 0..1000 {
        assert_eq!(policy::decide(&b, &event), first);
    }
    // Most severe label is named; deny rules beat allow rules.
    let Decision::Deny(d) = first else { panic!("high to public must deny") };
    assert_eq!((d.rule.as_str(), d.label), ("R1-high-never-public", Some(Sensitivity::High)));
}

#[test]
fn ok_loopback_endpoint_forms_classify_loopback() {
    for endpoint in [
        "http://127.0.0.1:15702",
        "http://localhost:15702",
        "http://LocalHost:15702/",
        "http://[::1]:15702",
        "http://127.1:15702",
        "http://127.8.9.10:15702",
    ] {
        assert_eq!(classify_brp_endpoint(endpoint), Ok(Trust::Loopback), "{endpoint}");
    }
}

#[test]
fn ok_denial_json_names_rule_label_and_sink() {
    let policy = default_policy();
    let mut session = Session::new();
    session
        .admit(&policy, "sprite_info", Some(&args(json!({"path":"refs/restricted/v.png"}))))
        .expect("local read allowed");
    let denial = session
        .admit(&policy, "bevy_call", Some(&args(json!({"endpoint":"http://10.1.2.3:15702","method":"m"}))))
        .expect_err("restricted to remote");
    let body: Value = serde_json::from_str(&denial.to_json()).expect("json");
    assert_eq!(body["error"], "policy denied");
    assert_eq!(body["policy"]["rule"], "R3-restricted-loopback-brp-only");
    assert_eq!(body["policy"]["label"], "restricted");
    assert_eq!(body["policy"]["sink"], "remote");
    assert_eq!(body["policy"]["tool"], "bevy_call");
    assert!(body["policy"]["reason"].as_str().is_some_and(|r| !r.is_empty()));
}

#[test]
fn ok_lua_rule_is_scoped_to_run_lua_script() {
    let policy = default_policy();
    let mut session = Session::new();
    session
        .admit(&policy, "sprite_info", Some(&args(json!({"path":"refs/web/x.png"}))))
        .expect("untrusted read is allowed locally");
    for tool in ["export_sprite", "set_pixel", "compare_frames"] {
        let a = args(json!({"doc":"sprites/a.lumen.json"}));
        assert!(session.admit(&policy, tool, Some(&a)).is_ok(), "{tool} unaffected by R2");
    }
}

#[test]
fn ok_session_labels_grow_only_on_allow() {
    let enforcer = Enforcer::new(default_policy());
    assert!(enforcer.session_labels().expect("labels").is_empty());
    enforcer
        .admit("sprite_info", Some(&args(json!({"path":"refs/licensed/a.png"}))))
        .expect("allowed");
    let after_allow = enforcer.session_labels().expect("labels");
    assert_eq!(after_allow, [Sensitivity::Low, Sensitivity::High].into());
    // A denied call (unbounded args) carrying a restricted path adds nothing.
    let deep = json!({"p":[[[[[[[[[["/etc/passwd"]]]]]]]]]]});
    assert!(enforcer.admit("sprite_info", Some(&args(deep))).is_err());
    assert_eq!(enforcer.session_labels().expect("labels"), after_allow);
}

#[test]
fn ok_plain_strings_and_lookalikes_stay_low() {
    let policy = default_policy();
    let session = Session::new();
    for value in [
        "hello world",
        "refs proprietary",
        "sprites/refs/proprietary/a.png",
        "refs/proprietary-fan/a.png",
        "refs/proprietaryX",
        "...",
    ] {
        let a = args(json!({"text": value}));
        let event = session.label(&policy, "text_measure", Some(&a)).expect("labels");
        assert_eq!(event.labels, [Sensitivity::Low].into(), "{value}");
    }
}

#[test]
fn ok_load_file_roundtrip_matches_builtin() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_policy(&dir, DEFAULT_POLICY_TOML);
    assert_eq!(Policy::load_file(&path).expect("loads"), default_policy());
}

#[test]
fn ok_binary_policy_check_passes() {
    let (code, out) = run_cli("check", None);
    assert_eq!(code, 0, "{out}");
    assert!(out.contains("PASS every required tool is labeled"));
    assert!(out.contains("registered tools: 3"));
}

#[test]
fn ok_binary_policy_replay_passes() {
    let (code, out) = run_cli("replay", None);
    assert_eq!(code, 0, "{out}");
    let summary = format!("{} passed, 0 failed", SCENARIOS.len());
    assert!(out.contains(&summary), "{out}");
}

#[test]
fn ok_stdio_server_runs_allowed_call() {
    let project = tempfile::tempdir().expect("tempdir");
    let replies = stdio_session(project.path(), &[("sprite_info", json!({"path":"missing.png"}))]);
    let text = reply_text(&replies[0]);
    // Allowed by policy, so the tool itself ran and reported its own error.
    assert!(text.contains("path rejected"), "{}", replies[0]);
    assert!(!text.contains("policy denied"));
}

// ---------------------------------------------------------------------------
// Adversarial
// ---------------------------------------------------------------------------

#[test]
fn adv_check_fails_on_registered_tool_without_labels() {
    let policy = Policy::from_toml_str(&policy_without_sprite_info_labels()).expect("loads");
    let report = policy::check(&policy, REGISTERED);
    assert_eq!(report.problems, vec!["tool `sprite_info` has no labels".to_string()]);
    // And at runtime the unlabeled tool is denied, not waved through.
    let r = Session::new().admit(&policy, "sprite_info", Some(&args(json!({"path":"a.png"}))));
    assert_eq!(denied_rule(r), policy::RULE_UNLABELED_TOOL);
    // A newly registered tool with no labels also fails check.
    let mut registered = REGISTERED.to_vec();
    registered.push("export_wasm4_cart");
    let report = policy::check(&default_policy(), &registered);
    assert_eq!(report.problems, vec!["tool `export_wasm4_cart` has no labels".to_string()]);
}

#[test]
fn adv_check_flags_unknown_tool_labels_and_rule_typos() {
    let text = DEFAULT_POLICY_TOML
        .replace("tools = [\"run_lua_script\"]", "tools = [\"run_lua_scirpt\"]")
        + "\n[tools.sprite_infoo]\nreads = [\"low\"]\nsink = \"trusted\"\n";
    let report = policy::check(&Policy::from_toml_str(&text).expect("loads"), REGISTERED);
    assert_eq!(
        report.problems,
        vec![
            "labels for unknown tool `sprite_infoo`".to_string(),
            "rule `R2-lua-refuses-untrusted` names unknown tool `run_lua_scirpt`".to_string(),
        ]
    );
}

#[test]
fn adv_loader_rejects_default_allow_unknown_keys_and_bad_version() {
    let allow = DEFAULT_POLICY_TOML.replace("default = \"deny\"", "default = \"allow\"");
    assert!(matches!(Policy::from_toml_str(&allow), Err(PolicyError::Invalid(_))));
    let unknown = format!("{DEFAULT_POLICY_TOML}\nbypass = true\n");
    assert!(matches!(Policy::from_toml_str(&unknown), Err(PolicyError::Parse(_))));
    let unknown_rule_key =
        DEFAULT_POLICY_TOML.replace("id = \"A1-low-anywhere\"", "id = \"A1-low-anywhere\"\nunless = []");
    assert!(matches!(Policy::from_toml_str(&unknown_rule_key), Err(PolicyError::Parse(_))));
    let bad_label = DEFAULT_POLICY_TOML.replace("label = \"untrusted\"", "label = \"secret\"");
    assert!(matches!(Policy::from_toml_str(&bad_label), Err(PolicyError::Parse(_))));
    let version = DEFAULT_POLICY_TOML.replace("version = 1", "version = 2");
    assert!(matches!(Policy::from_toml_str(&version), Err(PolicyError::Invalid(_))));
    assert!(matches!(Policy::from_toml_str("not = [toml"), Err(PolicyError::Parse(_))));
}

#[test]
fn adv_loader_rejects_reserved_duplicate_and_empty_rules() {
    let cases = [
        DEFAULT_POLICY_TOML.replace("A1-low-anywhere", "R4-deny-by-default"),
        DEFAULT_POLICY_TOML.replace("A1-low-anywhere", "R0-anything"),
        DEFAULT_POLICY_TOML.replace("A1-low-anywhere", "R1-high-never-public"),
        DEFAULT_POLICY_TOML.replace("id = \"A1-low-anywhere\"", "id = \"\""),
        DEFAULT_POLICY_TOML.replace("labels = [\"low\"]", "labels = []"),
        DEFAULT_POLICY_TOML.replace("prefix = \"refs/web\"", "prefix = \"../refs/web\""),
        DEFAULT_POLICY_TOML.replace("prefix = \"refs/web\"", "prefix = \"/refs/web\""),
        DEFAULT_POLICY_TOML.replace("prefix = \"refs/web\"", "prefix = \"./\""),
        DEFAULT_POLICY_TOML.replacen("reads = [\"low\"]", "reads = []", 1),
    ];
    for (index, text) in cases.iter().enumerate() {
        assert_ne!(text, DEFAULT_POLICY_TOML, "case {index} fixture drifted");
        let r = Policy::from_toml_str(text);
        assert!(matches!(r, Err(PolicyError::Invalid(_))), "case {index}: {r:?}");
    }
}

#[test]
fn adv_loader_rejects_oversized_policy() {
    let limit = usize::try_from(POLICY_BYTES_MAX).expect("fits");
    let big = format!("{DEFAULT_POLICY_TOML}\n#{}", "x".repeat(limit));
    assert!(matches!(Policy::from_toml_str(&big), Err(PolicyError::TooLarge(_))));
    let dir = tempfile::tempdir().expect("tempdir");
    let path = write_policy(&dir, &big);
    assert!(matches!(Policy::load_file(&path), Err(PolicyError::TooLarge(_))));
}

#[test]
fn adv_replay_catches_loosened_policy() {
    let policy = Policy::from_toml_str(&loosened_policy()).expect("loads");
    let failed: Vec<_> =
        policy::replay(&policy, SCENARIOS).into_iter().filter(|o| !o.passed).collect();
    assert!(!failed.is_empty(), "loosening R1 must break replay");
    assert!(failed.iter().any(|o| o.id == "adv_label_confusion_case_folding"), "{failed:?}");
}

#[test]
fn adv_binary_check_and_replay_fail_on_bad_policies() {
    let dir = tempfile::tempdir().expect("tempdir");
    let missing = write_policy(&dir, &policy_without_sprite_info_labels());
    let (code, out) = run_cli("check", Some(&missing));
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("FAIL tool `sprite_info` has no labels"), "{out}");
    let loose = write_policy(&dir, &loosened_policy());
    let (code, out) = run_cli("replay", Some(&loose));
    assert_eq!(code, 1, "{out}");
    assert!(out.contains("FAIL adv_label_confusion_case_folding"), "{out}");
    // A named policy file that cannot load is an error, not a fallback.
    let (code, out) = run_cli("check", Some(&dir.path().join("absent.toml")));
    assert_eq!(code, 1, "{out}");
}

#[test]
fn adv_prompt_injection_blocks_lua_regardless_of_lua_args() {
    let enforcer = Enforcer::new(default_policy());
    enforcer
        .admit("sprite_info", Some(&args(json!({"path":"refs/web/injected.png"}))))
        .expect("reading web content is allowed");
    let lua = args(json!({"doc":"sprites/a.lumen.json","script":"sprite.fill(0,1,1,1,1)",
        "opt_in":true,"trusted":true,"labels":["low"]}));
    let denial = enforcer.admit("run_lua_script", Some(&lua)).expect_err("R2");
    assert_eq!(denial.rule, "R2-lua-refuses-untrusted");
    assert_eq!((denial.label, denial.sink), (Some(Sensitivity::Untrusted), Some(Trust::Trusted)));
    // Untrusted data hidden in an object key or nested array is still seen.
    let session = Session::new();
    let policy = default_policy();
    for hidden in [json!({"x":{"refs/web/a.png":1}}), json!({"x":[1,["refs/web/a.png"]]})] {
        let r = session.label(&policy, "run_lua_script", Some(&args(hidden)));
        assert!(r.expect("labels").labels.contains(&Sensitivity::Untrusted));
    }
}

#[test]
fn adv_label_confusion_paths_cannot_launder_high_to_low() {
    let policy = default_policy();
    let session = Session::new();
    let cases = [
        ("REFS/PROPRIETARY/a.png", Sensitivity::High),
        ("refs//licensed/./a.png", Sensitivity::High),
        ("sprites/../refs/proprietary/a.png", Sensitivity::High),
        ("refs\\licensed\\a.png", Sensitivity::High),
        ("../sprite-mcp/refs/proprietary/a.png", Sensitivity::Restricted),
        ("/home/user/sprite-mcp/refs/proprietary/a.png", Sensitivity::Restricted),
        ("a/../../x.png", Sensitivity::Restricted),
    ];
    for (path, want) in cases {
        let a = args(json!({"path": path, "sensitivity": "low"}));
        let event = session.label(&policy, "sprite_info", Some(&a)).expect("labels");
        assert!(event.labels.contains(&want), "{path}: {:?}", event.labels);
        let to_public = CallEvent { sink: Trust::Public, ..event };
        assert!(matches!(policy::decide(&policy, &to_public), Decision::Deny(_)), "{path}");
    }
}

#[test]
fn adv_spoofed_brp_hosts_classify_remote_or_unresolvable() {
    for endpoint in [
        "http://127.0.0.1.nip.io:15702",
        "http://localhost@evil.example:15702",
        "http://localhost.evil.example:15702",
        "http://localhost.:15702",
        "http://[::ffff:127.0.0.1]:15702",
        "http://0.0.0.0:15702",
        "http://evil.example/http://127.0.0.1:15702",
        "http://evil.example:15702/?h=localhost",
    ] {
        assert_eq!(classify_brp_endpoint(endpoint), Ok(Trust::Remote), "{endpoint}");
    }
    for endpoint in ["", "127.0.0.1:15702", "not a url", "file:///etc/passwd"] {
        assert!(classify_brp_endpoint(endpoint).is_err(), "{endpoint}");
    }
    // Restricted context + spoofed loopback = R3, before any socket opens.
    let policy = default_policy();
    let mut session = Session::new();
    session
        .admit(&policy, "sprite_info", Some(&args(json!({"path":"refs/restricted/a.png"}))))
        .expect("allowed");
    let spoof = args(json!({"endpoint":"http://localhost@10.0.0.1:15702"}));
    assert_eq!(denied_rule(session.admit(&policy, "bevy_discover", Some(&spoof))), "R3-restricted-loopback-brp-only");
}

#[test]
fn adv_restricted_and_untrusted_to_external_are_default_denied() {
    let policy = default_policy();
    for label in [Sensitivity::Restricted, Sensitivity::Untrusted] {
        let event = CallEvent {
            tool: "a2a_dispatch".to_string(),
            labels: [label].into(),
            sink: Trust::External,
        };
        let Decision::Deny(d) = policy::decide(&policy, &event) else {
            panic!("{label:?} to external must deny")
        };
        assert_eq!((d.rule.as_str(), d.label, d.sink), (RULE_DENY_BY_DEFAULT, Some(label), Some(Trust::External)));
    }
}

#[test]
fn adv_stdio_server_denies_before_execution() {
    let project = tempfile::tempdir().expect("tempdir");
    let replies = stdio_session(
        project.path(),
        &[
            ("sprite_info", json!({"path":"refs/restricted/vault.png"})),
            ("bevy_discover", json!({"endpoint":"http://10.255.255.1:15702"})),
            ("shadow_exec", json!({"cmd":"id"})),
        ],
    );
    // Step 1 was allowed (the tool ran and rejected the missing file).
    assert!(reply_text(&replies[0]).contains("path rejected"), "{}", replies[0]);
    // Step 2: the policy, not bevy_discover's own loopback guard, refused.
    let denied: Value = serde_json::from_str(&reply_text(&replies[1])).expect("denial json");
    assert_eq!(denied["policy"]["rule"], "R3-restricted-loopback-brp-only", "{}", replies[1]);
    assert_eq!(denied["policy"]["sink"], "remote");
    assert_eq!(replies[1]["result"]["isError"], json!(true));
    // Step 3: unknown tool is refused by policy before routing.
    let unknown: Value = serde_json::from_str(&reply_text(&replies[2])).expect("denial json");
    assert_eq!(unknown["policy"]["rule"], policy::RULE_UNLABELED_TOOL, "{}", replies[2]);
}
