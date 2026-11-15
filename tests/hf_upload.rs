//! Integration gates for `lumen::hf_upload` (the `hf_dataset_upload`
//! tool): the generated-media backup contract against a mock Hub.
//!
//! Split, per the estate test doctrine: validation cases (the happy
//! paths of the contract — skip-identical, metadata merge, inline
//! mode, dry run) and adversarial cases (wrong hash, missing file,
//! unavailable token, traversal, remote conflict, double row,
//! lying read-back). The adversarial local cases assert the mock
//! saw ZERO requests: the contract's ordering (validate → token →
//! network) is itself under test.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use lumen::hf_upload::{
    HfDatasetUploadRequest, HfManifest, HfManifestFile, HfUploadContext, TokenSource,
    hf_dataset_upload,
};
use sha2::{Digest, Sha256};

const OLD_HEAD: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const NEW_HEAD: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const DATASET: &str = "test/ns";

// ---------------------------------------------------------------------------
// Mock Hub
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct MockFile {
    lfs_oid: Option<String>,
    content: Vec<u8>,
}

#[derive(Default)]
struct MockHub {
    head: String,
    files: BTreeMap<String, MockFile>,
    requests: Vec<String>,
    put_bodies: BTreeMap<String, Vec<u8>>,
    commits: Vec<String>,
    commit_count: usize,
    corrupt_verify: bool,
    base: String,
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn mock_json_path<'a>(path: &'a str, marker: &str) -> Option<&'a str> {
    path.split(marker).nth(1)
}

/// Route one request; returns (status, body).
fn route(state: &Arc<Mutex<MockHub>>, method: &str, path: &str, body: &[u8]) -> (u16, String) {
    let mut hub = state.lock().expect("mock lock");
    hub.requests.push(format!("{method} {path}"));
    if method == "GET" && path == format!("/api/datasets/{DATASET}") {
        return (
            200,
            serde_json::json!({ "sha": hub.head, "id": DATASET }).to_string(),
        );
    }
    if method == "POST" && path.ends_with("/paths-info/main") {
        let req: serde_json::Value = serde_json::from_slice(body).expect("paths-info body");
        let mut out = Vec::new();
        for p in req["paths"].as_array().expect("paths array") {
            let p = p.as_str().expect("path string");
            if let Some(file) = hub.files.get(p) {
                let oid = if hub.corrupt_verify && hub.commit_count > 0 {
                    "0".repeat(64)
                } else {
                    file.lfs_oid.clone().unwrap_or_default()
                };
                let mut entry = serde_json::json!({
                    "path": p, "size": file.content.len(),
                });
                if file.lfs_oid.is_some() {
                    entry["lfs"] = serde_json::json!({
                        "oid": oid, "size": file.content.len(), "pointerSize": 132,
                    });
                }
                out.push(entry);
            }
        }
        return (200, serde_json::Value::Array(out).to_string());
    }
    if method == "POST" && path.ends_with("/preupload/main") {
        let req: serde_json::Value = serde_json::from_slice(body).expect("preupload body");
        let files: Vec<serde_json::Value> = req["files"]
            .as_array()
            .expect("files array")
            .iter()
            .map(|f| {
                let p = f["path"].as_str().expect("path");
                let mode = if p.ends_with(".jsonl") || p.ends_with(".txt") {
                    "regular"
                } else {
                    "lfs"
                };
                serde_json::json!({ "path": p, "uploadMode": mode })
            })
            .collect();
        return (200, serde_json::json!({ "files": files }).to_string());
    }
    if method == "POST" && path.ends_with("/info/lfs/objects/batch") {
        let req: serde_json::Value = serde_json::from_slice(body).expect("batch body");
        let mut objects = Vec::new();
        for obj in req["objects"].as_array().expect("objects array") {
            let oid = obj["oid"].as_str().expect("oid").to_string();
            let known = hub.put_bodies.contains_key(&oid)
                || hub
                    .files
                    .values()
                    .any(|f| f.lfs_oid.as_deref() == Some(&oid));
            if known {
                objects.push(serde_json::json!({ "oid": oid, "size": obj["size"] }));
            } else {
                objects.push(serde_json::json!({
                    "oid": oid,
                    "size": obj["size"],
                    "actions": {
                        "upload": {
                            "href": format!("{}/lfs-put/{oid}", hub.base),
                            "header": {},
                        },
                        "verify": {
                            "href": format!("{}/lfs-verify", hub.base),
                            "header": {},
                        },
                    },
                }));
            }
        }
        return (
            200,
            serde_json::json!({ "transfer": "basic", "objects": objects }).to_string(),
        );
    }
    if method == "PUT" && path.starts_with("/lfs-put/") {
        let oid = path.trim_start_matches("/lfs-put/").to_string();
        hub.put_bodies.insert(oid, body.to_vec());
        return (200, "{}".to_string());
    }
    if method == "POST" && path == "/lfs-verify" {
        let req: serde_json::Value = serde_json::from_slice(body).expect("verify body");
        let oid = req["oid"].as_str().expect("oid");
        return if hub.put_bodies.contains_key(oid) {
            (200, "{}".to_string())
        } else {
            (404, "{}".to_string())
        };
    }
    if method == "GET" && path.contains("/resolve/main/") {
        let file_path = mock_json_path(path, "/resolve/main/").expect("resolve path");
        return match hub.files.get(file_path) {
            Some(file) => (200, String::from_utf8_lossy(&file.content).to_string()),
            None => (404, "not found".to_string()),
        };
    }
    if method == "POST" && path.ends_with("/commit/main") {
        let text = String::from_utf8(body.to_vec()).expect("commit body utf8");
        hub.commits.push(text.clone());
        for line in text.lines() {
            let op: serde_json::Value = serde_json::from_str(line).expect("commit line");
            match op["key"].as_str() {
                Some("file") => {
                    let p = op["value"]["path"].as_str().expect("file path").to_string();
                    let content = BASE64
                        .decode(op["value"]["content"].as_str().expect("content"))
                        .expect("base64 content");
                    hub.files.insert(
                        p,
                        MockFile {
                            lfs_oid: None,
                            content,
                        },
                    );
                }
                Some("lfsFile") => {
                    let p = op["value"]["path"].as_str().expect("lfs path").to_string();
                    let oid = op["value"]["oid"].as_str().expect("oid").to_string();
                    let content = hub.put_bodies.get(&oid).cloned().unwrap_or_default();
                    hub.files.insert(
                        p,
                        MockFile {
                            lfs_oid: Some(oid),
                            content,
                        },
                    );
                }
                _ => {}
            }
        }
        hub.commit_count += 1;
        hub.head = NEW_HEAD.to_string();
        return (
            200,
            serde_json::json!({ "commitOid": NEW_HEAD }).to_string(),
        );
    }
    (404, format!("mock: no route for {method} {path}"))
}

fn handle_conn(stream: std::net::TcpStream, state: Arc<Mutex<MockHub>>) {
    let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(10)));
    let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
    let mut writer = stream;
    loop {
        let mut request_line = String::new();
        match reader.read_line(&mut request_line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or("").to_string();
        let path = parts.next().unwrap_or("").to_string();
        if method.is_empty() {
            return;
        }
        let mut content_length = 0usize;
        loop {
            let mut line = String::new();
            match reader.read_line(&mut line) {
                Ok(0) | Err(_) => return,
                Ok(_) => {}
            }
            let line = line.trim();
            if line.is_empty() {
                break;
            }
            if let Some((name, value)) = line.split_once(':')
                && name.eq_ignore_ascii_case("content-length")
            {
                content_length = value.trim().parse().unwrap_or(0);
            }
        }
        let mut body = vec![0u8; content_length];
        if content_length > 0 && reader.read_exact(&mut body).is_err() {
            return;
        }
        let (status, text) = route(&state, &method, &path, &body);
        let response = format!(
            "HTTP/1.1 {status} OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{}",
            text.len(),
            text
        );
        if writer.write_all(response.as_bytes()).is_err() {
            return;
        }
    }
}

fn spawn_mock(configure: impl FnOnce(&mut MockHub)) -> (String, Arc<Mutex<MockHub>>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock");
    let port = listener.local_addr().expect("mock addr").port();
    let mut hub = MockHub {
        head: OLD_HEAD.to_string(),
        base: format!("http://127.0.0.1:{port}"),
        ..Default::default()
    };
    configure(&mut hub);
    let base = hub.base.clone();
    let state = Arc::new(Mutex::new(hub));
    let server_state = Arc::clone(&state);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            match stream {
                Ok(stream) => {
                    let conn_state = Arc::clone(&server_state);
                    std::thread::spawn(move || handle_conn(stream, conn_state));
                }
                Err(_) => break,
            }
        }
    });
    (base, state)
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn write_source(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, bytes).expect("write source");
    path
}

fn ctx(endpoint: &str, token_source: TokenSource) -> HfUploadContext {
    HfUploadContext {
        endpoint: endpoint.to_string(),
        token_source,
    }
}

fn env_ctx(endpoint: &str) -> HfUploadContext {
    ctx(endpoint, TokenSource::Literal("test-token".to_string()))
}

fn request(manifest: HfManifest) -> HfDatasetUploadRequest {
    HfDatasetUploadRequest {
        dataset: DATASET.to_string(),
        manifest_path: None,
        manifest: Some(manifest),
        pass_entry: "hf/test".to_string(),
        dry_run: false,
        commit_summary: Some("test commit".to_string()),
        pending_flag_path: None,
    }
}

fn clip_manifest(bytes: &[u8]) -> HfManifest {
    HfManifest {
        files: vec![HfManifestFile {
            source: Some("clip.mp4".to_string()),
            dest: "torogao/prima/video/clip.mp4".to_string(),
            sha256: sha256_hex(bytes),
            bytes: Some(bytes.len() as u64),
        }],
        metadata_path: Some("torogao/metadata.jsonl".to_string()),
        metadata_rows: Some(vec![serde_json::json!({
            "file_name": "prima/video/clip.mp4",
            "status": "candidate",
            "class": "video",
        })]),
        repo: None,
    }
}

const EXISTING_ROW: &str =
    "{\"file_name\":\"prima/video/old.mp4\",\"status\":\"candidate\",\"class\":\"video\"}";

fn seed_metadata(hub: &mut MockHub) {
    hub.files.insert(
        "torogao/metadata.jsonl".to_string(),
        MockFile {
            lfs_oid: None,
            content: format!("{EXISTING_ROW}\n").into_bytes(),
        },
    );
}

// ---------------------------------------------------------------------------
// Validation cases
// ---------------------------------------------------------------------------

#[tokio::test]
async fn happy_path_uploads_merges_and_clears_flag() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bytes: Vec<u8> = (0..4096u32).map(|i| (i % 251) as u8).collect();
    write_source(dir.path(), "clip.mp4", &bytes);
    let flag = write_source(dir.path(), "UPLOAD-PENDING", b"pending\n");
    let (endpoint, state) = spawn_mock(seed_metadata);

    let mut req = request(clip_manifest(&bytes));
    req.pending_flag_path = Some("UPLOAD-PENDING".to_string());
    let report = hf_dataset_upload(dir.path(), &req, &env_ctx(&endpoint))
        .await
        .expect("upload succeeds");

    assert!(report.verified);
    assert_eq!(report.commit_oid.as_deref(), Some(NEW_HEAD));
    assert_eq!(report.metadata_rows_added, 1);
    assert!(report.pending_flag_cleared);
    assert!(!flag.exists(), "flag removed after verification");
    assert_eq!(report.files[0].status, "uploaded");

    let hub = state.lock().expect("mock lock");
    assert_eq!(hub.commits.len(), 1);
    let commit = &hub.commits[0];
    let first: serde_json::Value =
        serde_json::from_str(commit.lines().next().expect("header line")).expect("header json");
    assert_eq!(first["key"], "header");
    assert_eq!(first["value"]["parentCommit"], OLD_HEAD);
    let lfs_line = commit
        .lines()
        .find(|l| l.contains("\"lfsFile\""))
        .expect("an lfsFile op");
    let lfs_op: serde_json::Value = serde_json::from_str(lfs_line).expect("lfs op json");
    assert_eq!(lfs_op["value"]["oid"], sha256_hex(&bytes));
    let meta_line = commit
        .lines()
        .find(|l| l.contains("metadata.jsonl"))
        .expect("a metadata file op");
    let meta_op: serde_json::Value = serde_json::from_str(meta_line).expect("meta op json");
    let merged = BASE64
        .decode(meta_op["value"]["content"].as_str().expect("meta content"))
        .expect("meta base64");
    let merged_text = String::from_utf8(merged).expect("meta utf8");
    assert!(
        merged_text.starts_with(EXISTING_ROW),
        "existing rows verbatim"
    );
    assert!(
        merged_text.contains("prima/video/clip.mp4"),
        "new row appended"
    );
    assert_eq!(
        hub.put_bodies.get(&sha256_hex(&bytes)),
        Some(&bytes),
        "LFS bytes arrived intact"
    );
}

#[tokio::test]
async fn already_present_file_is_skipped_and_still_verified() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bytes = b"already there".to_vec();
    write_source(dir.path(), "clip.mp4", &bytes);
    let sha = sha256_hex(&bytes);
    let (endpoint, state) = spawn_mock(|hub| {
        hub.files.insert(
            "torogao/prima/video/clip.mp4".to_string(),
            MockFile {
                lfs_oid: Some(sha.clone()),
                content: bytes.clone(),
            },
        );
    });
    let mut manifest = clip_manifest(&bytes);
    manifest.metadata_rows = Some(vec![]);
    let report = hf_dataset_upload(dir.path(), &request(manifest), &env_ctx(&endpoint))
        .await
        .expect("upload succeeds");
    assert!(report.already_up_to_date);
    assert_eq!(report.commit_oid, None);
    assert!(report.verified);
    assert_eq!(report.files[0].status, "already_present");
    let hub = state.lock().expect("mock lock");
    assert!(hub.commits.is_empty(), "no commit for a no-op");
    assert!(hub.put_bodies.is_empty(), "no LFS upload for a skip");
}

#[tokio::test]
async fn regular_mode_file_uploads_inline() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bytes = b"plain text payload".to_vec();
    write_source(dir.path(), "note.txt", &bytes);
    let (endpoint, state) = spawn_mock(|_| {});
    let manifest = HfManifest {
        files: vec![HfManifestFile {
            source: Some("note.txt".to_string()),
            dest: "notes/note.txt".to_string(),
            sha256: sha256_hex(&bytes),
            bytes: None,
        }],
        metadata_path: None,
        metadata_rows: None,
        repo: None,
    };
    let report = hf_dataset_upload(dir.path(), &request(manifest), &env_ctx(&endpoint))
        .await
        .expect("upload succeeds");
    assert!(report.verified);
    let hub = state.lock().expect("mock lock");
    assert_eq!(hub.commits.len(), 1);
    assert!(hub.commits[0].contains("\"file\""), "inline file op");
    assert!(hub.put_bodies.is_empty(), "no LFS traffic for inline");
}

#[tokio::test]
async fn dry_run_plans_without_token_or_network() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bytes = b"dry run bytes".to_vec();
    write_source(dir.path(), "clip.mp4", &bytes);
    let (endpoint, state) = spawn_mock(|_| {});
    let mut req = request(clip_manifest(&bytes));
    req.dry_run = true;
    // A pass source naming a bogus entry: any token read would fail,
    // so success here proves the dry run never touches the token.
    let context = ctx(
        &endpoint,
        TokenSource::Pass {
            entry: "lumen-test/no-such-entry".to_string(),
        },
    );
    let report = hf_dataset_upload(dir.path(), &req, &context)
        .await
        .expect("dry run succeeds");
    assert!(report.dry_run);
    assert!(!report.verified);
    assert_eq!(report.files[0].status, "planned");
    let hub = state.lock().expect("mock lock");
    assert!(
        hub.requests.is_empty(),
        "dry run made network calls: {:?}",
        hub.requests
    );
}

// ---------------------------------------------------------------------------
// Adversarial cases
// ---------------------------------------------------------------------------

#[tokio::test]
async fn wrong_sha_aborts_before_token_and_network() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bytes = b"real bytes".to_vec();
    write_source(dir.path(), "clip.mp4", &bytes);
    let (endpoint, state) = spawn_mock(|_| {});
    let mut manifest = clip_manifest(&bytes);
    manifest.files[0].sha256 = "0".repeat(64);
    let context = ctx(
        &endpoint,
        TokenSource::Pass {
            entry: "lumen-test/no-such-entry".to_string(),
        },
    );
    let err = hf_dataset_upload(dir.path(), &request(manifest), &context)
        .await
        .expect_err("wrong sha must fail");
    assert_eq!(err.kind(), "hash_mismatch");
    let hub = state.lock().expect("mock lock");
    assert!(
        hub.requests.is_empty(),
        "network touched: {:?}",
        hub.requests
    );
}

#[tokio::test]
async fn missing_file_aborts() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (endpoint, state) = spawn_mock(|_| {});
    let manifest = clip_manifest(b"never written");
    let err = hf_dataset_upload(dir.path(), &request(manifest), &env_ctx(&endpoint))
        .await
        .expect_err("missing source must fail");
    assert_eq!(err.kind(), "missing_file");
    let hub = state.lock().expect("mock lock");
    assert!(hub.requests.is_empty());
}

#[tokio::test]
async fn token_unavailable_is_structured_and_offline() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bytes = b"valid bytes".to_vec();
    write_source(dir.path(), "clip.mp4", &bytes);
    let (endpoint, state) = spawn_mock(|_| {});
    let context = ctx(
        &endpoint,
        TokenSource::Pass {
            entry: "lumen-test/no-such-entry".to_string(),
        },
    );
    let err = hf_dataset_upload(dir.path(), &request(clip_manifest(&bytes)), &context)
        .await
        .expect_err("unavailable token must fail");
    assert_eq!(err.kind(), "secret_unavailable");
    let hub = state.lock().expect("mock lock");
    assert!(
        hub.requests.is_empty(),
        "network touched: {:?}",
        hub.requests
    );
}

#[tokio::test]
async fn traversal_and_duplicate_dests_are_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bytes = b"bytes".to_vec();
    write_source(dir.path(), "clip.mp4", &bytes);
    for bad_dest in ["../evil.mp4", "/abs.mp4", "a/../../b.mp4", ""] {
        let mut manifest = clip_manifest(&bytes);
        manifest.files[0].dest = bad_dest.to_string();
        let err = hf_dataset_upload(
            dir.path(),
            &request(manifest),
            &env_ctx("http://127.0.0.1:1"),
        )
        .await
        .expect_err("bad dest must fail");
        assert_eq!(err.kind(), "invalid_input", "dest {bad_dest:?}");
    }
    let mut manifest = clip_manifest(&bytes);
    manifest.files.push(manifest.files[0].clone());
    let err = hf_dataset_upload(
        dir.path(),
        &request(manifest),
        &env_ctx("http://127.0.0.1:1"),
    )
    .await
    .expect_err("duplicate dest must fail");
    assert_eq!(err.kind(), "invalid_input");
}

#[tokio::test]
async fn remote_hash_conflict_aborts_before_any_write() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bytes = b"local bytes".to_vec();
    write_source(dir.path(), "clip.mp4", &bytes);
    let (endpoint, state) = spawn_mock(|hub| {
        hub.files.insert(
            "torogao/prima/video/clip.mp4".to_string(),
            MockFile {
                lfs_oid: Some("f".repeat(64)),
                content: b"someone else's bytes".to_vec(),
            },
        );
    });
    let err = hf_dataset_upload(
        dir.path(),
        &request(clip_manifest(&bytes)),
        &env_ctx(&endpoint),
    )
    .await
    .expect_err("conflict must fail");
    assert_eq!(err.kind(), "hash_conflict");
    let hub = state.lock().expect("mock lock");
    assert!(hub.commits.is_empty(), "a commit escaped the conflict");
    assert!(
        hub.put_bodies.is_empty(),
        "an LFS upload escaped the conflict"
    );
}

#[tokio::test]
async fn duplicate_metadata_row_aborts_before_commit() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bytes = b"clip bytes".to_vec();
    write_source(dir.path(), "clip.mp4", &bytes);
    let (endpoint, state) = spawn_mock(|hub| {
        hub.files.insert(
            "torogao/metadata.jsonl".to_string(),
            MockFile {
                lfs_oid: None,
                content: b"{\"file_name\":\"prima/video/clip.mp4\",\"status\":\"candidate\"}\n"
                    .to_vec(),
            },
        );
    });
    let err = hf_dataset_upload(
        dir.path(),
        &request(clip_manifest(&bytes)),
        &env_ctx(&endpoint),
    )
    .await
    .expect_err("duplicate row must fail");
    assert_eq!(err.kind(), "duplicate_metadata_row");
    let hub = state.lock().expect("mock lock");
    assert!(hub.commits.is_empty());
}

#[tokio::test]
async fn failed_verification_keeps_the_pending_flag() {
    let dir = tempfile::tempdir().expect("tempdir");
    let bytes = b"verify me".to_vec();
    write_source(dir.path(), "clip.mp4", &bytes);
    let flag = write_source(dir.path(), "UPLOAD-PENDING", b"pending\n");
    let (endpoint, _state) = spawn_mock(|hub| {
        hub.corrupt_verify = true;
    });
    let mut req = request(clip_manifest(&bytes));
    req.pending_flag_path = Some("UPLOAD-PENDING".to_string());
    let err = hf_dataset_upload(dir.path(), &req, &env_ctx(&endpoint))
        .await
        .expect_err("lying read-back must fail verification");
    assert_eq!(err.kind(), "verification_failed");
    assert!(flag.exists(), "flag must survive a failed verification");
}
