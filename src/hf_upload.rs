//! Hugging Face dataset upload: the generation finishing step.
//!
//! The generated-media backup rule sends finished generated media to a
//! Hugging Face dataset immediately on completion, labelled
//! `candidate`, before any verdict. That rule was served by
//! per-programme Python uploaders (strobe `upload_pack.py`, MoonDeep
//! `upload_tranche.py`, the Wan rerun uploader); the Wan one broke
//! when `huggingface_hub` removed `HfApi.hf_hub_url`, stranding
//! staged files behind an UPLOAD-PENDING flag. This module is the
//! Rust replacement, owned by lumen because lumen owns estate
//! generation (D-1268): dataset repo, paths, and metadata rows are
//! parameters, so the tool is lane-neutral machinery and no content
//! lives in it.
//!
//! Contract (extracted from the Python uploaders, which agree on it):
//!
//! 1. The HF token comes from a primo `pass` entry at call time only.
//!    It is never logged, never placed in argv, and never persisted.
//!    A locked agent or a missing entry is a structured
//!    `secret_unavailable` result, never a crash and never a retry.
//! 2. A manifest names every file: source path, destination path,
//!    and the sha256 the source must hash to. Every source is hashed
//!    and asserted locally *before* the token is read and before any
//!    network call, so a wrong hash aborts offline.
//! 3. A destination already in the dataset with the identical sha256
//!    is skipped (idempotent re-runs). A destination present with a
//!    *different* sha256 is a hard stop, never an overwrite.
//! 4. Metadata rows merge into the dataset's metadata file in the
//!    same commit as the files: existing lines are preserved
//!    verbatim, new rows append, and a new row whose `file_name`
//!    already exists remotely is a hard stop (no double rows).
//! 5. After the commit, every destination is verified by LFS sha256
//!    read-back (`paths-info`), and the metadata file is re-read and
//!    checked. A caller-named UPLOAD-PENDING flag file is removed
//!    only after verification passes.
//!
//! Wire protocol: the documented Hub endpoints (`preupload`, the
//! Git LFS batch API with the `basic` transfer, the NDJSON `commit`
//! endpoint, `paths-info`), implemented over the reqwest/sha2/base64
//! dependencies lumen already pins. The `hf-hub` crate (1.0) does
//! offer uploads, but its high-level path routes through the Xet
//! storage protocol and does not expose this contract's semantics
//! (sha-verified skip/refuse, same-commit metadata merge,
//! paths-info verification, pass-held token custody), and adding it
//! would break the workspace's pinned-dependency discipline for one
//! tool. The protocol surface needed here is small and stable.
//!
//! All request paths (manifest, sources, the pending flag) resolve
//! relative to `base_dir` — the server process working directory,
//! passed explicitly like every other filesystem root in this
//! library. Absolute paths in a call are the policy layer's business
//! (they label `restricted`, which never flows to the external
//! sink), so the relative-path contract is also what the shipped
//! policy admits.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Default Hub endpoint. Tests point the context at a local mock.
pub const DEFAULT_ENDPOINT: &str = "https://huggingface.co";
/// The one revision this tool writes to.
pub const REVISION: &str = "main";

/// Maximum files in one upload call.
pub const FILES_MAX: usize = 256;
/// Maximum bytes in one source file (single-PUT `basic` LFS transfer;
/// larger media is out of scope for this tool and errors explicitly).
pub const FILE_BYTES_MAX: u64 = 64 * 1024 * 1024;
/// Maximum summed source bytes in one upload call.
pub const TOTAL_BYTES_MAX: u64 = 512 * 1024 * 1024;
/// Maximum bytes carried inline (base64) in a commit: the ceiling
/// for `regular`-mode files and for the metadata download/compare.
pub const INLINE_BYTES_MAX: u64 = 10 * 1024 * 1024;
/// Maximum bytes of manifest JSON (file or inline).
pub const MANIFEST_BYTES_MAX: u64 = 1024 * 1024;
/// Bytes of file head sent to `preupload` as the sample.
pub const SAMPLE_BYTES: usize = 512;
/// Deadline for one `pass show` invocation (a locked agent can block
/// on pinentry; the call must fail structured, never hang).
pub const PASS_TIMEOUT: Duration = Duration::from_secs(15);
/// Maximum bytes read from `pass` stdout; a token entry is one line.
pub const TOKEN_BYTES_MAX: usize = 4096;
/// Per-request HTTP timeout (LFS PUTs of tens of MiB included).
pub const HTTP_TIMEOUT: Duration = Duration::from_secs(180);
/// Cap on Hub error text quoted into an error result.
pub const ERROR_BODY_MAX: usize = 300;

/// One file entry in an upload manifest.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct HfManifestFile {
    /// Filesystem path of the local file. Relative paths resolve
    /// against the manifest file's directory (or the server working
    /// directory for an inline manifest). When omitted, the staged
    /// layout applies: `<manifest dir>/files/<dest>`, which is the
    /// shape the per-programme staging scripts write.
    #[serde(default)]
    pub source: Option<String>,
    /// Destination path inside the dataset repo (relative, no
    /// traversal, at most 1000 characters).
    pub dest: String,
    /// Expected sha256 of the source file, 64 hex characters. The
    /// source is hashed locally and must match before any upload.
    pub sha256: String,
    /// Expected size in bytes; asserted against the source when set
    /// (the staging scripts always record it).
    #[serde(default)]
    pub bytes: Option<u64>,
}

/// An upload manifest: files plus the metadata rows that ride in the
/// same commit. Matches the staged manifest shape the per-programme
/// scripts write (extra fields there are ignored).
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct HfManifest {
    /// Files to upload, in any order; the tool sorts by destination
    /// for a deterministic commit.
    pub files: Vec<HfManifestFile>,
    /// Dataset-relative path of the metadata JSONL file the rows
    /// merge into (e.g. `torogao/metadata.jsonl`). Required when
    /// `metadata_rows` is set.
    #[serde(default)]
    pub metadata_path: Option<String>,
    /// Rows appended to the metadata file. Each row must be a JSON
    /// object with a string `file_name`; rows carry `status:
    /// candidate` by the caller's construction (the backup rule's
    /// label), the tool transports them verbatim.
    #[serde(default)]
    pub metadata_rows: Option<Vec<serde_json::Value>>,
    /// Dataset repo id recorded by a staging script. When set it
    /// must equal the request's `dataset` — a mismatch fails closed
    /// rather than silently overriding either side.
    #[serde(default)]
    pub repo: Option<String>,
}

/// Request for the `hf_dataset_upload` tool.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct HfDatasetUploadRequest {
    /// Dataset repo id, `namespace/name` (e.g.
    /// `NiceThoughtsRanch/ntranch-game-assets`).
    pub dataset: String,
    /// Path to a manifest JSON file, relative to the server working
    /// directory. Exactly one of `manifest_path` / `manifest`.
    #[serde(default)]
    pub manifest_path: Option<String>,
    /// The manifest, inline. Exactly one of `manifest_path` /
    /// `manifest`.
    #[serde(default)]
    pub manifest: Option<HfManifest>,
    /// Name of the primo `pass` entry holding the Hugging Face
    /// token (e.g. `hf/ntranch`). Read at call time only.
    pub pass_entry: String,
    /// Plan only: validate the manifest and hash every source
    /// locally, then report what would happen. No token is read and
    /// no network call is made; the pending flag is never cleared.
    #[serde(default)]
    pub dry_run: bool,
    /// Commit summary line. Defaults to a deterministic generated
    /// line naming the file count.
    #[serde(default)]
    pub commit_summary: Option<String>,
    /// Path of an UPLOAD-PENDING flag file, relative to the server
    /// working directory. Removed only after the upload verifies;
    /// never removed on a dry run or a failed call.
    #[serde(default)]
    pub pending_flag_path: Option<String>,
}

/// Where the upload token comes from. The MCP tool always uses
/// `Pass`; `EnvVar` exists for drivers (the Python uploaders'
/// `HF_TOKEN` convention) and `Literal` for tests — the MCP server
/// never constructs either of those two.
#[derive(Debug, Clone)]
pub enum TokenSource {
    /// A primo `pass` entry name, read via `pass show` at call time.
    Pass {
        /// Pass entry path, e.g. `hf/ntranch`.
        entry: String,
    },
    /// An environment variable naming the token.
    EnvVar {
        /// Environment variable name, e.g. `HF_TOKEN`.
        name: String,
    },
    /// The token held directly by the caller (tests only).
    Literal(String),
}

/// Call context: Hub endpoint plus token source. Production uses
/// [`HfUploadContext::production`]; tests substitute a local mock
/// endpoint and an environment token.
#[derive(Debug, Clone)]
pub struct HfUploadContext {
    /// Hub base URL, no trailing slash.
    pub endpoint: String,
    /// Token source for this call.
    pub token_source: TokenSource,
}

impl HfUploadContext {
    /// Production context: the real Hub, token from `pass`.
    pub fn production(pass_entry: &str) -> Self {
        Self {
            endpoint: DEFAULT_ENDPOINT.to_string(),
            token_source: TokenSource::Pass {
                entry: pass_entry.to_string(),
            },
        }
    }
}

/// Per-file outcome in the upload report.
#[derive(Debug, Clone, Serialize)]
pub struct HfFileReport {
    /// Destination path inside the dataset.
    pub dest: String,
    /// sha256 the destination was verified at.
    pub sha256: String,
    /// Source size in bytes.
    pub bytes: u64,
    /// `uploaded` by this call, or `already_present` with the
    /// identical hash (skipped, still verified).
    pub status: String,
}

/// The upload report returned by a successful call.
#[derive(Debug, Clone, Serialize)]
pub struct HfUploadReport {
    /// Dataset repo id uploaded to.
    pub dataset: String,
    /// True when this was a plan-only call.
    pub dry_run: bool,
    /// Commit created by this call, or null when nothing changed.
    pub commit_oid: Option<String>,
    /// Dataset head after the call.
    pub new_head: Option<String>,
    /// True when every file was already present and no metadata row
    /// was added, so no commit was needed.
    pub already_up_to_date: bool,
    /// Per-file outcomes, sorted by destination.
    pub files: Vec<HfFileReport>,
    /// Metadata file the rows merged into, when any.
    pub metadata_path: Option<String>,
    /// Metadata rows appended by this call.
    pub metadata_rows_added: usize,
    /// True when every destination was verified by read-back after
    /// the commit (always false on a dry run, which verifies
    /// nothing remotely).
    pub verified: bool,
    /// True when the named pending flag file was removed.
    pub pending_flag_cleared: bool,
}

/// Typed failures for the upload tool. `kind()` is the stable
/// machine-readable class the MCP result carries beside the message;
/// secret material never appears in any variant.
#[derive(Debug)]
pub enum HfUploadError {
    /// The request or manifest is malformed or out of bounds.
    InvalidInput(String),
    /// The manifest file could not be read.
    ManifestIo {
        /// Manifest path as given.
        path: String,
        /// Underlying I/O detail.
        detail: String,
    },
    /// A manifest source file does not exist.
    MissingFile {
        /// Source path as resolved.
        path: String,
    },
    /// A source file's size differs from the manifest's record.
    SizeMismatch {
        /// Destination the entry names.
        dest: String,
        /// Manifest-recorded bytes.
        expected: u64,
        /// Actual bytes on disk.
        actual: u64,
    },
    /// A source file's sha256 differs from the manifest's record.
    /// Raised locally, before the token is read or any network call.
    HashMismatch {
        /// Destination the entry names.
        dest: String,
        /// Manifest-recorded sha256.
        expected: String,
        /// Computed sha256.
        actual: String,
    },
    /// The token could not be obtained (locked agent, missing
    /// entry, missing variable). A result, never a crash; zero
    /// network calls have been made when this is returned.
    SecretUnavailable(String),
    /// The destination already exists in the dataset with different
    /// content. The tool never overwrites.
    HashConflict {
        /// Destination path in the dataset.
        dest: String,
        /// sha256 recorded remotely (or `non-lfs` when the remote
        /// file is not LFS-tracked and could not be compared).
        remote_sha256: String,
        /// sha256 the manifest wants there.
        local_sha256: String,
    },
    /// A metadata row's `file_name` already exists in the dataset's
    /// metadata file. The tool never double-rows.
    DuplicateMetadataRow {
        /// The colliding `file_name`.
        file_name: String,
    },
    /// A Hub HTTP call returned a non-success status.
    Http {
        /// Operation name (`preupload`, `commit`, ...).
        operation: &'static str,
        /// HTTP status code.
        status: u16,
        /// Bounded excerpt of the response body.
        body: String,
    },
    /// A Hub response could not be understood, or the Hub reported
    /// an application-level error.
    Api {
        /// Operation name.
        operation: &'static str,
        /// Detail, bounded, secret-free.
        detail: String,
    },
    /// The Hub assigned a transfer or mode this tool does not
    /// implement (multipart LFS, oversized inline file, ignored
    /// path). Explicit, never silent.
    UnsupportedMode {
        /// Destination path concerned.
        dest: String,
        /// What the Hub asked for.
        detail: String,
    },
    /// Post-commit read-back did not match what was committed.
    Verification {
        /// What failed to verify.
        detail: String,
    },
    /// The upload verified but the pending flag could not be
    /// removed; the flag remains and the caller is told.
    FlagClear {
        /// Flag path as given.
        path: String,
        /// Underlying I/O detail.
        detail: String,
    },
}

impl HfUploadError {
    /// Stable machine-readable error class for the MCP result.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::InvalidInput(_) => "invalid_input",
            Self::ManifestIo { .. } => "manifest_io",
            Self::MissingFile { .. } => "missing_file",
            Self::SizeMismatch { .. } => "size_mismatch",
            Self::HashMismatch { .. } => "hash_mismatch",
            Self::SecretUnavailable(_) => "secret_unavailable",
            Self::HashConflict { .. } => "hash_conflict",
            Self::DuplicateMetadataRow { .. } => "duplicate_metadata_row",
            Self::Http { .. } => "http_error",
            Self::Api { .. } => "api_error",
            Self::UnsupportedMode { .. } => "unsupported_mode",
            Self::Verification { .. } => "verification_failed",
            Self::FlagClear { .. } => "flag_clear_failed",
        }
    }
}

impl fmt::Display for HfUploadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidInput(msg) => write!(f, "invalid input: {msg}"),
            Self::ManifestIo { path, detail } => {
                write!(f, "cannot read manifest {path}: {detail}")
            }
            Self::MissingFile { path } => write!(f, "source file missing: {path}"),
            Self::SizeMismatch {
                dest,
                expected,
                actual,
            } => write!(
                f,
                "size mismatch for {dest}: manifest records {expected} bytes, source has {actual}"
            ),
            Self::HashMismatch {
                dest,
                expected,
                actual,
            } => write!(
                f,
                "sha256 mismatch for {dest}: manifest records {expected}, source hashes to {actual}"
            ),
            Self::SecretUnavailable(msg) => write!(f, "secret unavailable: {msg}"),
            Self::HashConflict {
                dest,
                remote_sha256,
                local_sha256,
            } => write!(
                f,
                "destination {dest} already exists with different content \
                 (remote {remote_sha256}, local {local_sha256}); refusing to overwrite"
            ),
            Self::DuplicateMetadataRow { file_name } => write!(
                f,
                "metadata already contains a row for {file_name}; refusing to double-row"
            ),
            Self::Http {
                operation,
                status,
                body,
            } => write!(f, "hub {operation} failed with HTTP {status}: {body}"),
            Self::Api { operation, detail } => {
                write!(f, "hub {operation} failed: {detail}")
            }
            Self::UnsupportedMode { dest, detail } => {
                write!(f, "unsupported hub mode for {dest}: {detail}")
            }
            Self::Verification { detail } => write!(f, "verification failed: {detail}"),
            Self::FlagClear { path, detail } => {
                write!(
                    f,
                    "upload verified but pending flag {path} not removed: {detail}"
                )
            }
        }
    }
}

impl std::error::Error for HfUploadError {}

/// A manifest file after local validation and hashing: the source
/// is known to exist and to hash to the manifest's sha256.
struct PreparedFile {
    source_path: PathBuf,
    dest: String,
    sha256: String,
    bytes: u64,
    sample_b64: String,
}

/// Validate a dataset repo id: exactly `namespace/name`, each part
/// a non-empty run of `[A-Za-z0-9._-]` not starting with `.` or `-`.
fn validate_dataset(dataset: &str) -> Result<(), HfUploadError> {
    let invalid = |why: &str| HfUploadError::InvalidInput(format!("dataset `{dataset}`: {why}"));
    let mut parts = dataset.split('/');
    let (Some(ns), Some(name), None) = (parts.next(), parts.next(), parts.next()) else {
        return Err(invalid("expected namespace/name"));
    };
    for part in [ns, name] {
        if part.is_empty()
            || part.starts_with(['.', '-'])
            || !part
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
        {
            return Err(invalid("bad namespace or name characters"));
        }
    }
    Ok(())
}

/// Validate a dataset-relative path (destination or metadata
/// path): relative, `/`-separated, no empty / `.` / `..` segments,
/// conservative characters, at most 1000 characters.
fn validate_repo_path(path: &str, what: &str) -> Result<(), HfUploadError> {
    let invalid = |why: &str| HfUploadError::InvalidInput(format!("{what} `{path}`: {why}"));
    if path.is_empty() || path.len() > 1000 {
        return Err(invalid("empty or over 1000 characters"));
    }
    if path.starts_with('/') || path.contains('\\') {
        return Err(invalid("must be a relative forward-slash path"));
    }
    for segment in path.split('/') {
        if segment.is_empty() || segment == "." || segment == ".." {
            return Err(invalid("empty, dot, or traversal segment"));
        }
        if !segment
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ' '))
        {
            return Err(invalid("unsupported characters in a segment"));
        }
    }
    Ok(())
}

/// Normalize a manifest sha256: must be 64 hex characters; the
/// lowercase form is returned (hash comparison is case-insensitive
/// on input, exact internally).
fn normalize_sha256(raw: &str, dest: &str) -> Result<String, HfUploadError> {
    let lower = raw.to_ascii_lowercase();
    if lower.len() != 64 || !lower.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(HfUploadError::InvalidInput(format!(
            "sha256 for {dest} is not 64 hex characters"
        )));
    }
    Ok(lower)
}

/// Load the manifest from the request: exactly one of a path or an
/// inline value. Returns the manifest and the directory sources
/// resolve against (the manifest file's directory, or `base_dir`
/// for an inline manifest).
fn load_manifest(
    base_dir: &Path,
    req: &HfDatasetUploadRequest,
) -> Result<(HfManifest, PathBuf), HfUploadError> {
    match (&req.manifest_path, &req.manifest) {
        (Some(_), Some(_)) => Err(HfUploadError::InvalidInput(
            "give exactly one of manifest_path and manifest, not both".to_string(),
        )),
        (None, None) => Err(HfUploadError::InvalidInput(
            "one of manifest_path or manifest is required".to_string(),
        )),
        (None, Some(manifest)) => Ok((manifest.clone(), base_dir.to_path_buf())),
        (Some(rel), None) => {
            let path = base_dir.join(rel);
            let data = std::fs::read(&path).map_err(|e| HfUploadError::ManifestIo {
                path: rel.clone(),
                detail: e.to_string(),
            })?;
            if data.len() as u64 > MANIFEST_BYTES_MAX {
                return Err(HfUploadError::InvalidInput(format!(
                    "manifest {rel} exceeds {MANIFEST_BYTES_MAX} bytes"
                )));
            }
            let manifest: HfManifest = serde_json::from_slice(&data).map_err(|e| {
                HfUploadError::InvalidInput(format!(
                    "manifest {rel} is not valid manifest JSON: {e}"
                ))
            })?;
            let dir = path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| base_dir.to_path_buf());
            Ok((manifest, dir))
        }
    }
}

/// Hash one file with sha256, streaming in 1 MiB chunks; also
/// captures the head sample `preupload` asks for. Returns the
/// lowercase hex digest, the byte count, and the base64 sample.
fn hash_file(path: &Path) -> Result<(String, u64, String), HfUploadError> {
    use std::io::Read as _;
    let display = path.display().to_string();
    let mut file = std::fs::File::open(path).map_err(|_| HfUploadError::MissingFile {
        path: display.clone(),
    })?;
    let mut hasher = Sha256::new();
    let mut total: u64 = 0;
    let mut sample: Vec<u8> = Vec::new();
    let mut buffer = vec![0u8; 1024 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|e| HfUploadError::Api {
            operation: "read_source",
            detail: format!("reading {display}: {e}"),
        })?;
        if read == 0 {
            break;
        }
        let chunk = &buffer[..read];
        if sample.len() < SAMPLE_BYTES {
            let take = (SAMPLE_BYTES - sample.len()).min(chunk.len());
            sample.extend_from_slice(&chunk[..take]);
        }
        hasher.update(chunk);
        total = total.saturating_add(read as u64);
        if total > FILE_BYTES_MAX {
            return Err(HfUploadError::InvalidInput(format!(
                "source {display} exceeds the per-file cap of {FILE_BYTES_MAX} bytes"
            )));
        }
    }
    let digest = format!("{:x}", hasher.finalize());
    Ok((digest, total, BASE64.encode(&sample)))
}

/// Validate the manifest and hash every source locally. This is the
/// whole offline phase: on success the plan is known-good, and any
/// failure here precedes token acquisition and all network I/O.
fn prepare_files(
    manifest_dir: &Path,
    manifest: &HfManifest,
) -> Result<Vec<PreparedFile>, HfUploadError> {
    if manifest.files.is_empty() || manifest.files.len() > FILES_MAX {
        return Err(HfUploadError::InvalidInput(format!(
            "manifest must name between 1 and {FILES_MAX} files"
        )));
    }
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut total: u64 = 0;
    let mut prepared = Vec::with_capacity(manifest.files.len());
    for entry in &manifest.files {
        validate_repo_path(&entry.dest, "dest")?;
        if !seen.insert(entry.dest.clone()) {
            return Err(HfUploadError::InvalidInput(format!(
                "duplicate dest in manifest: {}",
                entry.dest
            )));
        }
        let sha256 = normalize_sha256(&entry.sha256, &entry.dest)?;
        let source_path = match &entry.source {
            Some(source) => manifest_dir.join(source),
            None => manifest_dir.join("files").join(&entry.dest),
        };
        let (actual_sha, actual_bytes, sample_b64) = hash_file(&source_path)?;
        if let Some(expected_bytes) = entry.bytes
            && expected_bytes != actual_bytes
        {
            return Err(HfUploadError::SizeMismatch {
                dest: entry.dest.clone(),
                expected: expected_bytes,
                actual: actual_bytes,
            });
        }
        if actual_sha != sha256 {
            return Err(HfUploadError::HashMismatch {
                dest: entry.dest.clone(),
                expected: sha256,
                actual: actual_sha,
            });
        }
        total = total.saturating_add(actual_bytes);
        if total > TOTAL_BYTES_MAX {
            return Err(HfUploadError::InvalidInput(format!(
                "manifest exceeds the per-call cap of {TOTAL_BYTES_MAX} bytes"
            )));
        }
        prepared.push(PreparedFile {
            source_path,
            dest: entry.dest.clone(),
            sha256,
            bytes: actual_bytes,
            sample_b64,
        });
    }
    prepared.sort_by(|a, b| a.dest.cmp(&b.dest));
    Ok(prepared)
}

/// Validate the metadata side of the manifest: a `metadata_path`
/// must accompany rows, the path must be a clean repo path distinct
/// from every payload dest, and every row must be an object with a
/// unique string `file_name`.
fn prepare_metadata(
    manifest: &HfManifest,
    files: &[PreparedFile],
) -> Result<(Option<String>, Vec<serde_json::Value>), HfUploadError> {
    let rows = manifest.metadata_rows.clone().unwrap_or_default();
    let Some(path) = &manifest.metadata_path else {
        if !rows.is_empty() {
            return Err(HfUploadError::InvalidInput(
                "metadata_rows given without metadata_path".to_string(),
            ));
        }
        return Ok((None, rows));
    };
    validate_repo_path(path, "metadata_path")?;
    if files.iter().any(|f| &f.dest == path) {
        return Err(HfUploadError::InvalidInput(format!(
            "metadata_path {path} collides with a payload dest"
        )));
    }
    let mut seen: BTreeSet<String> = BTreeSet::new();
    for row in &rows {
        let Some(name) = row
            .as_object()
            .and_then(|obj| obj.get("file_name"))
            .and_then(serde_json::Value::as_str)
        else {
            return Err(HfUploadError::InvalidInput(
                "every metadata row must be an object with a string file_name".to_string(),
            ));
        };
        if name.is_empty() || !seen.insert(name.to_string()) {
            return Err(HfUploadError::InvalidInput(format!(
                "metadata row file_name `{name}` is empty or duplicated in the manifest"
            )));
        }
    }
    Ok((Some(path.clone()), rows))
}

/// Resolve the token without ever logging or persisting it. A
/// locked `pass` agent can block on pinentry, so the read is
/// deadline-bounded and the child is killed on drop; any failure —
/// spawn, timeout, non-zero exit, empty output — is the structured
/// `secret_unavailable` result.
async fn acquire_token(source: &TokenSource) -> Result<String, HfUploadError> {
    match source {
        TokenSource::Literal(raw) => {
            let token = raw.trim().to_string();
            if token.is_empty() {
                Err(HfUploadError::SecretUnavailable(
                    "literal token is empty".to_string(),
                ))
            } else {
                Ok(token)
            }
        }
        TokenSource::EnvVar { name } => match std::env::var(name) {
            Ok(value) if !value.trim().is_empty() => Ok(value.trim().to_string()),
            _ => Err(HfUploadError::SecretUnavailable(format!(
                "environment variable {name} is unset or empty"
            ))),
        },
        TokenSource::Pass { entry } => {
            if entry.is_empty()
                || entry.starts_with('-')
                || !entry
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '/' | '.' | '_' | '-'))
            {
                return Err(HfUploadError::InvalidInput(format!(
                    "pass entry `{entry}` is not a plain pass path"
                )));
            }
            acquire_token_from_pass(entry).await
        }
    }
}

/// The `pass show` half of [`acquire_token`], split out for size.
async fn acquire_token_from_pass(entry: &str) -> Result<String, HfUploadError> {
    use tokio::io::AsyncReadExt as _;
    let unavailable = |why: String| {
        HfUploadError::SecretUnavailable(format!(
            "pass entry `{entry}` unreadable ({why}); the GPG agent may be locked"
        ))
    };
    let mut child = tokio::process::Command::new("pass")
        .arg("show")
        .arg(entry)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| unavailable(format!("spawn failed: {e}")))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| unavailable("no stdout pipe".to_string()))?;
    let read = async {
        let mut buf = Vec::new();
        (&mut stdout)
            .take(TOKEN_BYTES_MAX as u64)
            .read_to_end(&mut buf)
            .await?;
        Ok::<Vec<u8>, std::io::Error>(buf)
    };
    let output = tokio::time::timeout(PASS_TIMEOUT, read)
        .await
        .map_err(|_| unavailable("timed out".to_string()))?
        .map_err(|e| unavailable(format!("read failed: {e}")))?;
    let status = tokio::time::timeout(PASS_TIMEOUT, child.wait())
        .await
        .map_err(|_| unavailable("timed out waiting for exit".to_string()))?
        .map_err(|e| unavailable(format!("wait failed: {e}")))?;
    if !status.success() {
        return Err(unavailable(format!("pass exited with {status}")));
    }
    let text = String::from_utf8_lossy(&output);
    let token = text.lines().next().unwrap_or("").trim().to_string();
    if token.is_empty() {
        return Err(unavailable("entry read back empty".to_string()));
    }
    Ok(token)
}

/// Remote existence state for one destination, from `paths-info`.
#[derive(Debug, Clone, PartialEq, Eq)]
enum RemoteState {
    /// Present as an LFS file with this sha256. (Absence is
    /// represented by the path missing from the map entirely.)
    Lfs(String),
    /// Present as a regular (non-LFS) file of this size; content
    /// comparison needs a download.
    Regular(u64),
}

/// A thin Hub client: one reqwest client, the dataset, and the
/// bearer token. The token is held only in this struct's memory for
/// the duration of one call and is never logged or formatted.
struct HubClient {
    http: reqwest::Client,
    endpoint: String,
    dataset: String,
    token: String,
}

impl HubClient {
    /// Build the client; a construction failure is an `api_error`,
    /// never a panic.
    fn new(ctx: &HfUploadContext, dataset: &str, token: String) -> Result<Self, HfUploadError> {
        let http = reqwest::Client::builder()
            .timeout(HTTP_TIMEOUT)
            .connect_timeout(Duration::from_secs(15))
            .user_agent(concat!("lumen-hf-upload/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| HfUploadError::Api {
                operation: "client_build",
                detail: e.to_string(),
            })?;
        Ok(Self {
            http,
            endpoint: ctx.endpoint.trim_end_matches('/').to_string(),
            dataset: dataset.to_string(),
            token,
        })
    }

    /// API base for the dataset (`{endpoint}/api/datasets/{id}`).
    fn api_base(&self) -> String {
        format!("{}/api/datasets/{}", self.endpoint, self.dataset)
    }

    /// Attach the bearer token to a request builder.
    fn authed(&self, rb: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        rb.bearer_auth(&self.token)
    }

    /// Send a request and classify the outcome: success → response;
    /// non-success → [`HfUploadError::Http`] with a bounded,
    /// secret-free body excerpt (Hub error bodies name paths and
    /// rules, never credentials).
    async fn send(
        &self,
        operation: &'static str,
        rb: reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, HfUploadError> {
        let resp = rb.send().await.map_err(|e| HfUploadError::Api {
            operation,
            detail: format!("request failed: {}", sanitize_reqwest_error(&e)),
        })?;
        let status = resp.status();
        if status.is_success() {
            return Ok(resp);
        }
        let body = resp.text().await.unwrap_or_default();
        Err(HfUploadError::Http {
            operation,
            status: status.as_u16(),
            body: excerpt(&body),
        })
    }

    /// Current dataset head sha (`GET /api/datasets/{id}`).
    async fn head_sha(&self) -> Result<String, HfUploadError> {
        let url = format!("{}/api/datasets/{}", self.endpoint, self.dataset);
        let resp = self
            .send("dataset_info", self.authed(self.http.get(url)))
            .await?;
        let value: serde_json::Value = resp.json().await.map_err(|e| HfUploadError::Api {
            operation: "dataset_info",
            detail: format!("unparseable response: {e}"),
        })?;
        value
            .get("sha")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string)
            .ok_or_else(|| HfUploadError::Api {
                operation: "dataset_info",
                detail: "response carries no sha".to_string(),
            })
    }

    /// `paths-info` for a batch of paths: destination → state.
    /// Paths the Hub does not return are simply absent remotely.
    async fn paths_info(
        &self,
        paths: &[String],
    ) -> Result<BTreeMap<String, RemoteState>, HfUploadError> {
        let url = format!("{}/paths-info/{REVISION}", self.api_base());
        let body = serde_json::json!({ "paths": paths });
        let resp = self
            .send("paths_info", self.authed(self.http.post(url)).json(&body))
            .await?;
        let entries: Vec<serde_json::Value> =
            resp.json().await.map_err(|e| HfUploadError::Api {
                operation: "paths_info",
                detail: format!("unparseable response: {e}"),
            })?;
        let mut states = BTreeMap::new();
        for entry in entries {
            let Some(path) = entry.get("path").and_then(serde_json::Value::as_str) else {
                continue;
            };
            let state = match entry.get("lfs") {
                Some(lfs) if !lfs.is_null() => {
                    let oid = lfs
                        .get("oid")
                        .or_else(|| lfs.get("sha256"))
                        .and_then(serde_json::Value::as_str)
                        .unwrap_or_default()
                        .to_ascii_lowercase();
                    RemoteState::Lfs(oid)
                }
                _ => RemoteState::Regular(
                    entry
                        .get("size")
                        .and_then(serde_json::Value::as_u64)
                        .unwrap_or(0),
                ),
            };
            states.insert(path.to_string(), state);
        }
        Ok(states)
    }

    /// Download a dataset file's content via `resolve`, bounded.
    /// `Ok(None)` is a 404 (file not in the dataset).
    async fn fetch_file(
        &self,
        path: &str,
        max_bytes: u64,
    ) -> Result<Option<Vec<u8>>, HfUploadError> {
        let url = format!(
            "{}/datasets/{}/resolve/{REVISION}/{path}",
            self.endpoint, self.dataset
        );
        let resp =
            self.authed(self.http.get(url))
                .send()
                .await
                .map_err(|e| HfUploadError::Api {
                    operation: "fetch_file",
                    detail: format!("request failed: {}", sanitize_reqwest_error(&e)),
                })?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(HfUploadError::Http {
                operation: "fetch_file",
                status,
                body: excerpt(&body),
            });
        }
        let bytes = resp.bytes().await.map_err(|e| HfUploadError::Api {
            operation: "fetch_file",
            detail: format!("read failed: {}", sanitize_reqwest_error(&e)),
        })?;
        if bytes.len() as u64 > max_bytes {
            return Err(HfUploadError::Api {
                operation: "fetch_file",
                detail: format!("{path} exceeds the {max_bytes}-byte fetch bound"),
            });
        }
        Ok(Some(bytes.to_vec()))
    }
}

/// Bound and sanitize an error body excerpt for display.
fn excerpt(body: &str) -> String {
    let trimmed = body.trim();
    let mut out: String = trimmed.chars().take(ERROR_BODY_MAX).collect();
    if trimmed.chars().count() > ERROR_BODY_MAX {
        out.push_str("...");
    }
    out
}

/// Format a reqwest error without its URL (URLs here never carry
/// the token, but the habit is cheaper than the audit).
fn sanitize_reqwest_error(e: &reqwest::Error) -> String {
    match e.status() {
        Some(status) => format!("http status {status}"),
        None => "connection or read failure".to_string(),
    }
}

/// Parse a metadata JSONL body into its verbatim non-blank lines
/// plus the set of `file_name`s present. A line that is not a JSON
/// object is remote corruption and fails the call before any write.
fn parse_metadata_lines(
    body: &[u8],
    path: &str,
) -> Result<(Vec<String>, BTreeSet<String>), HfUploadError> {
    let text = String::from_utf8(body.to_vec()).map_err(|_| HfUploadError::Api {
        operation: "fetch_file",
        detail: format!("existing {path} is not UTF-8"),
    })?;
    let mut lines = Vec::new();
    let mut names = BTreeSet::new();
    for (idx, line) in text.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let value: serde_json::Value =
            serde_json::from_str(line).map_err(|e| HfUploadError::Api {
                operation: "fetch_file",
                detail: format!("existing {path} line {} is not valid JSON: {e}", idx + 1),
            })?;
        if let Some(name) = value
            .as_object()
            .and_then(|obj| obj.get("file_name"))
            .and_then(serde_json::Value::as_str)
        {
            names.insert(name.to_string());
        }
        lines.push(line.to_string());
    }
    Ok((lines, names))
}

/// One file staged for the commit: either a payload file (content
/// read from `source_path` on demand) or the merged metadata file
/// (content held in memory).
struct CommitItem {
    dest: String,
    sha256: String,
    bytes: u64,
    sample_b64: String,
    content: Option<Vec<u8>>,
    source_path: Option<PathBuf>,
}

/// Ask `preupload` how each staged item must travel. Returns
/// dest → is-LFS. A `shouldIgnore` verdict or an unknown mode is an
/// explicit error: silently dropping a file is the one outcome this
/// tool must never produce.
async fn preupload_modes(
    hub: &HubClient,
    items: &[CommitItem],
) -> Result<BTreeMap<String, bool>, HfUploadError> {
    let url = format!("{}/preupload/{REVISION}", hub.api_base());
    let files: Vec<serde_json::Value> = items
        .iter()
        .map(|item| {
            serde_json::json!({
                "path": item.dest,
                "size": item.bytes,
                "sample": item.sample_b64,
            })
        })
        .collect();
    let body = serde_json::json!({ "files": files });
    let resp = hub
        .send("preupload", hub.authed(hub.http.post(url)).json(&body))
        .await?;
    let value: serde_json::Value = resp.json().await.map_err(|e| HfUploadError::Api {
        operation: "preupload",
        detail: format!("unparseable response: {e}"),
    })?;
    let mut modes = BTreeMap::new();
    let entries = value
        .get("files")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| HfUploadError::Api {
            operation: "preupload",
            detail: "response carries no files array".to_string(),
        })?;
    for entry in entries {
        let Some(path) = entry.get("path").and_then(serde_json::Value::as_str) else {
            continue;
        };
        if entry
            .get("shouldIgnore")
            .and_then(serde_json::Value::as_bool)
            .unwrap_or(false)
        {
            return Err(HfUploadError::UnsupportedMode {
                dest: path.to_string(),
                detail: "the dataset's ignore rules exclude this path".to_string(),
            });
        }
        let is_lfs = match entry.get("uploadMode").and_then(serde_json::Value::as_str) {
            Some("lfs") => true,
            Some("regular") => false,
            other => {
                return Err(HfUploadError::Api {
                    operation: "preupload",
                    detail: format!("unknown uploadMode {other:?} for {path}"),
                });
            }
        };
        modes.insert(path.to_string(), is_lfs);
    }
    for item in items {
        if !modes.contains_key(&item.dest) {
            return Err(HfUploadError::Api {
                operation: "preupload",
                detail: format!("no upload mode returned for {}", item.dest),
            });
        }
    }
    Ok(modes)
}

/// Upload one LFS object's bytes: batch-negotiate a `basic`
/// transfer, PUT the content, then run the batch's verify action
/// when one is offered. An object the batch returns without actions
/// is already in storage and counts as uploaded.
async fn upload_lfs_object(
    hub: &HubClient,
    item: &CommitItem,
    content: &[u8],
) -> Result<(), HfUploadError> {
    let url = format!(
        "{}/datasets/{}.git/info/lfs/objects/batch",
        hub.endpoint, hub.dataset
    );
    let body = serde_json::json!({
        "operation": "upload",
        "transfers": ["basic"],
        "objects": [{ "oid": item.sha256, "size": item.bytes }],
        "hash_algo": "sha256",
    });
    let resp = hub
        .send(
            "lfs_batch",
            hub.authed(hub.http.post(url))
                .header("Accept", "application/vnd.git-lfs+json")
                .header("Content-Type", "application/vnd.git-lfs+json")
                .json(&body),
        )
        .await?;
    let value: serde_json::Value = resp.json().await.map_err(|e| HfUploadError::Api {
        operation: "lfs_batch",
        detail: format!("unparseable response: {e}"),
    })?;
    let object = value
        .get("objects")
        .and_then(serde_json::Value::as_array)
        .and_then(|objects| objects.first())
        .ok_or_else(|| HfUploadError::Api {
            operation: "lfs_batch",
            detail: format!("no object in batch response for {}", item.dest),
        })?;
    if let Some(error) = object.get("error") {
        let message = error
            .get("message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("object refused");
        return Err(HfUploadError::Api {
            operation: "lfs_batch",
            detail: format!("{}: {}", item.dest, excerpt(message)),
        });
    }
    let Some(actions) = object.get("actions") else {
        return Ok(()); // Already in storage.
    };
    let Some(upload) = actions.get("upload") else {
        return Ok(());
    };
    let href = upload
        .get("href")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| HfUploadError::Api {
            operation: "lfs_batch",
            detail: format!("upload action without href for {}", item.dest),
        })?;
    let mut put = hub.http.put(href);
    if let Some(header) = upload.get("header").and_then(serde_json::Value::as_object) {
        if header.contains_key("chunk_size") {
            return Err(HfUploadError::UnsupportedMode {
                dest: item.dest.clone(),
                detail: "hub asked for a multipart LFS transfer; this tool \
                         implements the basic transfer only"
                    .to_string(),
            });
        }
        for (name, value) in header {
            if let Some(value) = value.as_str() {
                put = put.header(name.as_str(), value);
            }
        }
    }
    // The PUT goes to pre-signed storage: no bearer token here, the
    // batch action's own headers carry the authorization.
    hub.send("lfs_put", put.body(content.to_vec())).await?;
    if let Some(verify) = actions.get("verify")
        && let Some(verify_href) = verify.get("href").and_then(serde_json::Value::as_str)
    {
        let mut post = hub.http.post(verify_href);
        if let Some(header) = verify.get("header").and_then(serde_json::Value::as_object) {
            for (name, value) in header {
                if let Some(value) = value.as_str() {
                    post = post.header(name.as_str(), value);
                }
            }
        }
        let body = serde_json::json!({ "oid": item.sha256, "size": item.bytes });
        hub.send("lfs_verify", post.json(&body)).await?;
    }
    Ok(())
}

/// Read a staged item's content: from memory for the metadata
/// pseudo-file, from disk for payload files.
fn item_content(item: &CommitItem) -> Result<Vec<u8>, HfUploadError> {
    if let Some(content) = &item.content {
        return Ok(content.clone());
    }
    let path = item
        .source_path
        .as_ref()
        .expect("payload items always carry a source path");
    std::fs::read(path).map_err(|e| HfUploadError::Api {
        operation: "read_source",
        detail: format!("reading {}: {e}", path.display()),
    })
}

/// Build the NDJSON commit body: header line first (with the
/// parent commit, so a moved head fails the commit instead of
/// racing), then one operation per item sorted by destination.
fn build_commit_body(
    summary: &str,
    parent: &str,
    items: &[CommitItem],
    modes: &BTreeMap<String, bool>,
) -> Result<String, HfUploadError> {
    let mut lines = Vec::with_capacity(items.len() + 1);
    lines.push(
        serde_json::json!({
            "key": "header",
            "value": { "summary": summary, "parentCommit": parent },
        })
        .to_string(),
    );
    for item in items {
        let is_lfs = modes[&item.dest];
        let line = if is_lfs {
            serde_json::json!({
                "key": "lfsFile",
                "value": {
                    "path": item.dest,
                    "algo": "sha256",
                    "oid": item.sha256,
                    "size": item.bytes,
                },
            })
        } else {
            if item.bytes > INLINE_BYTES_MAX {
                return Err(HfUploadError::UnsupportedMode {
                    dest: item.dest.clone(),
                    detail: format!(
                        "hub assigned the inline (regular) mode to a {}-byte file; \
                         the inline cap is {INLINE_BYTES_MAX} bytes",
                        item.bytes
                    ),
                });
            }
            let content = item_content(item)?;
            serde_json::json!({
                "key": "file",
                "value": {
                    "path": item.dest,
                    "content": BASE64.encode(&content),
                    "encoding": "base64",
                },
            })
        };
        lines.push(line.to_string());
    }
    Ok(lines.join("\n") + "\n")
}

/// POST the commit; returns the new commit oid.
async fn post_commit(hub: &HubClient, body: String) -> Result<String, HfUploadError> {
    let url = format!("{}/commit/{REVISION}", hub.api_base());
    let resp = hub
        .send(
            "commit",
            hub.authed(hub.http.post(url))
                .header("Content-Type", "application/x-ndjson")
                .body(body),
        )
        .await?;
    let value: serde_json::Value = resp.json().await.map_err(|e| HfUploadError::Api {
        operation: "commit",
        detail: format!("unparseable response: {e}"),
    })?;
    value
        .get("commitOid")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| HfUploadError::Api {
            operation: "commit",
            detail: "response carries no commitOid".to_string(),
        })
}

/// Compare one destination against its remote state. Returns true
/// when the file is already present with identical content (an LFS
/// sha match, or a regular file whose downloaded bytes hash to the
/// manifest sha). A different remote hash is the hard stop.
async fn already_present(
    hub: &HubClient,
    file: &PreparedFile,
    state: Option<&RemoteState>,
) -> Result<bool, HfUploadError> {
    match state {
        None => Ok(false),
        Some(RemoteState::Lfs(remote_sha)) => {
            if remote_sha == &file.sha256 {
                Ok(true)
            } else {
                Err(HfUploadError::HashConflict {
                    dest: file.dest.clone(),
                    remote_sha256: remote_sha.clone(),
                    local_sha256: file.sha256.clone(),
                })
            }
        }
        Some(RemoteState::Regular(size)) => {
            if *size > INLINE_BYTES_MAX {
                return Err(HfUploadError::HashConflict {
                    dest: file.dest.clone(),
                    remote_sha256: "non-lfs (too large to compare)".to_string(),
                    local_sha256: file.sha256.clone(),
                });
            }
            let Some(bytes) = hub.fetch_file(&file.dest, INLINE_BYTES_MAX).await? else {
                return Ok(false);
            };
            let digest = format!("{:x}", Sha256::digest(&bytes));
            if digest == file.sha256 {
                Ok(true)
            } else {
                Err(HfUploadError::HashConflict {
                    dest: file.dest.clone(),
                    remote_sha256: "non-lfs (content differs)".to_string(),
                    local_sha256: file.sha256.clone(),
                })
            }
        }
    }
}

/// Post-commit verification of one payload file: LFS destinations
/// must read back at the manifest sha; inline destinations are
/// re-downloaded and re-hashed.
async fn verify_file(
    hub: &HubClient,
    file: &PreparedFile,
    state: Option<&RemoteState>,
    was_lfs: bool,
) -> Result<(), HfUploadError> {
    let fail = |detail: String| HfUploadError::Verification { detail };
    match state {
        Some(RemoteState::Lfs(remote_sha)) if was_lfs => {
            if remote_sha == &file.sha256 {
                Ok(())
            } else {
                Err(fail(format!(
                    "{} reads back at {remote_sha}, expected {}",
                    file.dest, file.sha256
                )))
            }
        }
        _ => {
            let Some(bytes) = hub.fetch_file(&file.dest, INLINE_BYTES_MAX).await? else {
                return Err(fail(format!("{} missing after commit", file.dest)));
            };
            let digest = format!("{:x}", Sha256::digest(&bytes));
            if digest == file.sha256 {
                Ok(())
            } else {
                Err(fail(format!(
                    "{} content hashes to {digest} after commit, expected {}",
                    file.dest, file.sha256
                )))
            }
        }
    }
}

/// Upload a manifest of generated media to a Hugging Face dataset,
/// per the contract in the module docs. `base_dir` is the server
/// working directory all request paths resolve against.
///
/// State transitions: validate (offline) → plan (dry run stops
/// here) → token → remote pre-check (conflicts stop before any
/// write) → LFS upload → commit → verify → flag clear. Any failure
/// before the commit leaves the dataset untouched; a failure after
/// it leaves the flag in place and says exactly what did not
/// verify, and the call is safe to re-run (identical destinations
/// are skipped).
pub async fn hf_dataset_upload(
    base_dir: &Path,
    req: &HfDatasetUploadRequest,
    ctx: &HfUploadContext,
) -> Result<HfUploadReport, HfUploadError> {
    validate_dataset(&req.dataset)?;
    if let Some(summary) = &req.commit_summary
        && summary.len() > 200
    {
        return Err(HfUploadError::InvalidInput(
            "commit_summary exceeds 200 characters".to_string(),
        ));
    }
    let (manifest, manifest_dir) = load_manifest(base_dir, req)?;
    if let Some(repo) = &manifest.repo
        && repo != &req.dataset
    {
        return Err(HfUploadError::InvalidInput(format!(
            "manifest names repo {repo} but the request names {}; refusing to guess",
            req.dataset
        )));
    }
    let files = prepare_files(&manifest_dir, &manifest)?;
    let (metadata_path, new_rows) = prepare_metadata(&manifest, &files)?;

    let mut report = HfUploadReport {
        dataset: req.dataset.clone(),
        dry_run: req.dry_run,
        commit_oid: None,
        new_head: None,
        already_up_to_date: false,
        files: files
            .iter()
            .map(|f| HfFileReport {
                dest: f.dest.clone(),
                sha256: f.sha256.clone(),
                bytes: f.bytes,
                status: "planned".to_string(),
            })
            .collect(),
        metadata_path: metadata_path.clone(),
        metadata_rows_added: 0,
        verified: false,
        pending_flag_cleared: false,
    };
    if req.dry_run {
        return Ok(report);
    }

    // Token before network: a secret failure is a zero-network
    // result by construction.
    let token = acquire_token(&ctx.token_source).await?;
    let hub = HubClient::new(ctx, &req.dataset, token)?;
    let head = hub.head_sha().await?;

    // Remote pre-check: classify every destination before writing
    // anything anywhere.
    let mut dests: Vec<String> = files.iter().map(|f| f.dest.clone()).collect();
    if let Some(path) = &metadata_path {
        dests.push(path.clone());
    }
    let states = hub.paths_info(&dests).await?;
    let mut present: BTreeSet<String> = BTreeSet::new();
    for file in &files {
        if already_present(&hub, file, states.get(&file.dest)).await? {
            present.insert(file.dest.clone());
        }
    }

    // Metadata merge: existing lines verbatim, new rows appended;
    // a file_name already present remotely is a hard stop.
    let mut metadata_item: Option<CommitItem> = None;
    if let Some(path) = &metadata_path {
        let existing_body = hub.fetch_file(path, INLINE_BYTES_MAX).await?;
        let (lines, names) = match &existing_body {
            Some(body) => parse_metadata_lines(body, path)?,
            None => (Vec::new(), BTreeSet::new()),
        };
        for row in &new_rows {
            let name = row
                .get("file_name")
                .and_then(serde_json::Value::as_str)
                .expect("rows validated in prepare_metadata");
            if names.contains(name) {
                return Err(HfUploadError::DuplicateMetadataRow {
                    file_name: name.to_string(),
                });
            }
        }
        if !new_rows.is_empty() {
            let mut merged = lines;
            for row in &new_rows {
                merged.push(serde_json::to_string(row).expect("row serializes"));
            }
            let content = (merged.join("\n") + "\n").into_bytes();
            if content.len() as u64 > INLINE_BYTES_MAX {
                return Err(HfUploadError::InvalidInput(format!(
                    "merged {path} exceeds the {INLINE_BYTES_MAX}-byte metadata bound"
                )));
            }
            let sha256 = format!("{:x}", Sha256::digest(&content));
            let sample_b64 = BASE64.encode(&content[..SAMPLE_BYTES.min(content.len())]);
            metadata_item = Some(CommitItem {
                dest: path.clone(),
                sha256,
                bytes: content.len() as u64,
                sample_b64,
                content: Some(content),
                source_path: None,
            });
            report.metadata_rows_added = new_rows.len();
        }
    }

    // Stage the commit: pending payload files plus the metadata
    // pseudo-file when rows were added.
    let mut items: Vec<CommitItem> = files
        .iter()
        .filter(|f| !present.contains(&f.dest))
        .map(|f| CommitItem {
            dest: f.dest.clone(),
            sha256: f.sha256.clone(),
            bytes: f.bytes,
            sample_b64: f.sample_b64.clone(),
            content: None,
            source_path: Some(f.source_path.clone()),
        })
        .collect();
    if let Some(item) = metadata_item {
        items.push(item);
    }
    items.sort_by(|a, b| a.dest.cmp(&b.dest));

    let mut modes: BTreeMap<String, bool> = BTreeMap::new();
    if !items.is_empty() {
        modes = preupload_modes(&hub, &items).await?;
        for item in &items {
            if modes[&item.dest] {
                let content = item_content(item)?;
                upload_lfs_object(&hub, item, &content).await?;
            }
        }
        let summary = req.commit_summary.clone().unwrap_or_else(|| {
            format!(
                "Add {} file(s) via lumen hf_dataset_upload (generated-media backup rule)",
                items.len()
            )
        });
        let body = build_commit_body(&summary, &head, &items, &modes)?;
        let commit_oid = post_commit(&hub, body).await?;
        report.new_head = Some(commit_oid.clone());
        report.commit_oid = Some(commit_oid);
    } else {
        report.new_head = Some(head);
        report.already_up_to_date = true;
    }
    for file_report in &mut report.files {
        if present.contains(&file_report.dest) {
            file_report.status = "already_present".to_string();
        } else {
            file_report.status = "uploaded".to_string();
        }
    }

    // Verification: every destination reads back at its manifest
    // sha, and the metadata file carries each new row exactly once.
    let after = hub.paths_info(&dests).await?;
    for file in &files {
        let was_lfs = modes.get(&file.dest).copied().unwrap_or(false)
            || matches!(after.get(&file.dest), Some(RemoteState::Lfs(_)));
        verify_file(&hub, file, after.get(&file.dest), was_lfs).await?;
    }
    if report.metadata_rows_added > 0
        && let Some(path) = &metadata_path
    {
        let Some(body) = hub.fetch_file(path, INLINE_BYTES_MAX).await? else {
            return Err(HfUploadError::Verification {
                detail: format!("{path} missing after commit"),
            });
        };
        let (_, names) = parse_metadata_lines(&body, path)?;
        for row in &new_rows {
            let name = row
                .get("file_name")
                .and_then(serde_json::Value::as_str)
                .expect("rows validated in prepare_metadata");
            if !names.contains(name) {
                return Err(HfUploadError::Verification {
                    detail: format!("metadata row {name} missing after commit"),
                });
            }
        }
    }
    report.verified = true;

    // The flag clears only now: verified upload, nothing pending.
    if let Some(flag) = &req.pending_flag_path {
        let flag_path = base_dir.join(flag);
        match std::fs::remove_file(&flag_path) {
            Ok(()) => report.pending_flag_cleared = true,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                report.pending_flag_cleared = false;
            }
            Err(e) => {
                return Err(HfUploadError::FlagClear {
                    path: flag.clone(),
                    detail: e.to_string(),
                });
            }
        }
    }
    Ok(report)
}
