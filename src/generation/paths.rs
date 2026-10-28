//! Source admission and output placement — the profile wall on disk.
//!
//! Contract (sources): a caller path is a request, not authorization. It
//! must be absolute, canonicalize (resolving `..` and symlinks) to a regular
//! file under one of the active profile's canonical source roots, be at most
//! `source_bytes_max`, and be a PNG. The bytes are read once; everything
//! after (hash, staging, upload) uses those bytes.
//!
//! Contract (outputs): backend descriptors are untrusted. The filename must
//! be one plain path component and the subfolder must be empty; anything
//! else is `output_escape_refused`. Files are written only beneath
//! `output_root/<server job id>/`, created fresh with `create_new`, and
//! re-verified by canonicalization after the write.

use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use serde::Serialize;

use crate::generation::admission::sha256_bytes;
use crate::generation::error::{AtelierError, ErrorCode, Result, err};

pub const PNG_MAGIC: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
/// Longest output filename accepted from the backend, in bytes.
pub const OUTPUT_FILENAME_BYTES_MAX: usize = 128;
/// Largest PNG atelier will decode, in decoded bytes.
pub const DECODED_BYTES_MAX: usize = 256 * 1024 * 1024;

/// A validated source image, read once.
#[derive(Debug, Clone)]
pub struct AdmittedSource {
    pub canonical: PathBuf,
    /// Root-labelled reference, e.g. `source_root[0]:frames/x.png`.
    pub label: String,
    pub bytes: Vec<u8>,
    pub sha256: String,
    pub width: u32,
    pub height: u32,
}

pub fn admit_source(raw: &str, roots: &[PathBuf], bytes_max: u64) -> Result<AdmittedSource> {
    if raw.is_empty() || raw.contains('\0') || !Path::new(raw).is_absolute() {
        return err(
            ErrorCode::SourceOutsideRoots,
            "source_path must be a nonempty absolute path",
        );
    }
    let canonical = Path::new(raw).canonicalize().map_err(|cause| {
        AtelierError::new(ErrorCode::SourceUnreadable, format!("source_path: {cause}"))
    })?;
    let Some((index, root)) = roots
        .iter()
        .enumerate()
        .find(|(_, root)| canonical.starts_with(root))
    else {
        return err(
            ErrorCode::SourceOutsideRoots,
            "source resolves outside every allowed source root of the active profile",
        );
    };
    let metadata = std::fs::symlink_metadata(&canonical).map_err(|cause| {
        AtelierError::new(ErrorCode::SourceUnreadable, format!("source: {cause}"))
    })?;
    if !metadata.is_file() {
        return err(
            ErrorCode::SourceNotRegularFile,
            "source is not a regular file",
        );
    }
    if metadata.len() > bytes_max {
        return err(
            ErrorCode::SourceTooLarge,
            format!("source is {} bytes; limit {bytes_max}", metadata.len()),
        );
    }
    let bytes = read_bounded(&canonical, bytes_max)?;
    if !bytes.starts_with(&PNG_MAGIC) {
        return err(ErrorCode::SourceUnsupportedFormat, "source is not a PNG");
    }
    let info = decode_png(&bytes)
        .map_err(|error| AtelierError::new(ErrorCode::SourceUnsupportedFormat, error.message))?;
    let relative = canonical.strip_prefix(root).unwrap_or(&canonical);
    Ok(AdmittedSource {
        label: format!("source_root[{index}]:{}", relative.display()),
        sha256: sha256_bytes(&bytes),
        canonical,
        bytes,
        width: info.width,
        height: info.height,
    })
}

fn read_bounded(path: &Path, bytes_max: u64) -> Result<Vec<u8>> {
    let file = std::fs::File::open(path)
        .map_err(|cause| AtelierError::new(ErrorCode::SourceUnreadable, cause.to_string()))?;
    let mut bytes = Vec::new();
    file.take(bytes_max.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(|cause| AtelierError::new(ErrorCode::SourceUnreadable, cause.to_string()))?;
    if bytes.len() as u64 > bytes_max {
        return err(
            ErrorCode::SourceTooLarge,
            "source grew past the limit while reading",
        );
    }
    Ok(bytes)
}

/// Decoded-image facts recorded for every output (and checked for sources).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ImageInfo {
    pub format: &'static str,
    pub width: u32,
    pub height: u32,
    /// Population standard deviation over all 8-bit channel samples.
    pub pixel_stddev: f64,
}

pub fn decode_png(bytes: &[u8]) -> Result<ImageInfo> {
    let fail = |what: String| AtelierError::new(ErrorCode::OutputInvalidImage, what);
    let mut decoder = png::Decoder::new(bytes);
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder
        .read_info()
        .map_err(|cause| fail(format!("PNG header: {cause}")))?;
    let buffer_bytes = reader.output_buffer_size();
    if buffer_bytes > DECODED_BYTES_MAX {
        return Err(fail(format!("decoded size {buffer_bytes} exceeds limit")));
    }
    let mut buffer = vec![0u8; buffer_bytes];
    let frame = reader
        .next_frame(&mut buffer)
        .map_err(|cause| fail(format!("PNG data: {cause}")))?;
    let samples = &buffer[..frame.buffer_size()];
    if samples.is_empty() {
        return Err(fail("PNG has no pixels".into()));
    }
    let count = samples.len() as f64;
    let mean = samples.iter().map(|&value| f64::from(value)).sum::<f64>() / count;
    let variance = samples
        .iter()
        .map(|&value| (f64::from(value) - mean).powi(2))
        .sum::<f64>()
        / count;
    Ok(ImageInfo {
        format: "png",
        width: frame.width,
        height: frame.height,
        pixel_stddev: variance.sqrt(),
    })
}

/// Validate an untrusted backend output descriptor into a bare filename.
pub fn safe_output_name(filename: &str, subfolder: &str) -> Result<String> {
    let escape = |why: &str| {
        AtelierError::new(
            ErrorCode::OutputEscapeRefused,
            format!("backend output descriptor refused: {why}"),
        )
    };
    if !subfolder.is_empty() {
        return Err(escape("nonempty subfolder"));
    }
    if filename.is_empty() || filename.len() > OUTPUT_FILENAME_BYTES_MAX {
        return Err(escape("filename empty or too long"));
    }
    if filename.contains(['/', '\\', '\0']) || filename.starts_with('.') {
        return Err(escape("filename is not one plain component"));
    }
    let mut components = Path::new(filename).components();
    match (components.next(), components.next()) {
        (Some(Component::Normal(_)), None) => Ok(filename.to_string()),
        _ => Err(escape("filename is not one plain component")),
    }
}

/// Create `root/<job_id>` fresh (it must not pre-exist) and prove it resolves
/// inside `root`.
pub fn create_job_dir(root: &Path, job_id: &str) -> Result<PathBuf> {
    assert!(
        job_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-'),
        "job ids are server-generated"
    );
    let dir = root.join(job_id);
    std::fs::create_dir(&dir).map_err(|cause| {
        AtelierError::new(
            ErrorCode::OutputWriteFailed,
            format!("job dir {}: {cause}", dir.display()),
        )
    })?;
    ensure_inside(root, &dir)?;
    Ok(dir)
}

fn ensure_inside(root: &Path, path: &Path) -> Result<()> {
    let canonical = path.canonicalize().map_err(|cause| {
        AtelierError::new(ErrorCode::OutputWriteFailed, format!("verify: {cause}"))
    })?;
    if canonical.starts_with(root) {
        return Ok(());
    }
    err(
        ErrorCode::OutputEscapeRefused,
        format!("{} resolves outside the profile root", path.display()),
    )
}

/// Write `bytes` to `dir/name` (new file only; never follows an existing
/// symlink) and re-verify containment.
pub fn write_new(root: &Path, dir: &Path, name: &str, bytes: &[u8]) -> Result<PathBuf> {
    ensure_inside(root, dir)?;
    let path = dir.join(name);
    let write = || -> std::io::Result<()> {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        file.write_all(bytes)?;
        file.sync_all()
    };
    write().map_err(|cause| {
        AtelierError::new(
            ErrorCode::OutputWriteFailed,
            format!("{}: {cause}", path.display()),
        )
    })?;
    ensure_inside(root, &path)?;
    Ok(path)
}
