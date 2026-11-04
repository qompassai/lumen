//! lumen-engine-pack: packaging backends for the lumen
//! engine (SPEC section 4.1).
//!
//! The packaging contract is determinism (SPEC section
//! 4.4): a release tree goes in, a byte-reproducible
//! `.tar.gz` and a release record come out. The tar layer
//! is a hand-rolled ustar writer with every variable
//! field pinned (entries sorted by path, mode 0644,
//! uid/gid 0, mtime 0, empty uname/gname), and the gzip
//! layer is written with a zeroed header mtime — so the
//! same tree packages to the same archive bytes on any
//! machine, and the archive's SHA-256 is a meaningful
//! release identifier.
//!
//! The release record is the read-back contract: it
//! names the archive (file, size, SHA-256) and every
//! member (path, size, SHA-256). [`verify_archive`]
//! re-reads an archive and checks it against a record;
//! any disagreement is reported as a named discrepancy,
//! never absorbed.
//!
//! Nothing here shells out, opens a GUI, or reads a
//! clock. Android packaging (the owned-Gradle path,
//! D-829) is Stage 3 scope in the SPEC's plan; this
//! crate is the desktop backend plus the record law the
//! Android backend will emit into.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::Path;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Release-record format tag.
pub const RELEASE_FORMAT: &str = "lumen-engine-release/1";

/// SHA-256 of `bytes`, lowercase hex.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(64);
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

/// One file inside a release tree / archive.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct MemberRecord {
    /// Archive-relative path, `/`-separated, sorted.
    pub path: String,
    /// Size in bytes.
    pub size: u64,
    /// SHA-256 of the file's bytes, lowercase hex.
    pub sha256: String,
}

/// The release record: the archive's identity plus its
/// full member manifest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseRecord {
    /// Format tag; must equal [`RELEASE_FORMAT`].
    pub format: String,
    /// Project the release belongs to.
    pub project: String,
    /// Archive file name (basename only).
    pub archive_file: String,
    /// Archive size in bytes.
    pub archive_size: u64,
    /// SHA-256 of the archive bytes.
    pub archive_sha256: String,
    /// Every member, sorted by path.
    pub members: Vec<MemberRecord>,
}

/// A packaging failure. Messages name the path or field
/// at fault; packaging never guesses.
#[derive(Debug)]
pub struct PackError(pub String);

impl std::fmt::Display for PackError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::error::Error for PackError {}

type PackResult<T> = Result<T, PackError>;

/// Collect a directory tree into sorted `(path, bytes)`
/// members. Paths are relative to `root`, `/`-separated.
/// Symlinks are refused: a release tree is plain files.
pub fn collect_tree(root: &Path) -> PackResult<Vec<(String, Vec<u8>)>> {
    let mut out = Vec::new();
    collect_into(root, root, &mut out)?;
    out.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(out)
}

fn collect_into(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) -> PackResult<()> {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .map_err(|e| PackError(format!("read_dir {}: {e}", dir.display())))?
        .collect::<Result<_, _>>()
        .map_err(|e| PackError(format!("read_dir entry under {}: {e}", dir.display())))?;
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let path = entry.path();
        let file_type = entry
            .file_type()
            .map_err(|e| PackError(format!("file_type {}: {e}", path.display())))?;
        if file_type.is_symlink() {
            return Err(PackError(format!(
                "release tree contains a symlink: {}",
                path.display()
            )));
        }
        if file_type.is_dir() {
            collect_into(root, &path, out)?;
        } else if file_type.is_file() {
            let rel = path
                .strip_prefix(root)
                .map_err(|_| PackError(format!("path escapes root: {}", path.display())))?;
            let rel = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            let bytes = std::fs::read(&path)
                .map_err(|e| PackError(format!("read {}: {e}", path.display())))?;
            out.push((rel, bytes));
        }
    }
    Ok(())
}

/// The member manifest of a member set.
pub fn manifest_of(members: &[(String, Vec<u8>)]) -> Vec<MemberRecord> {
    members
        .iter()
        .map(|(path, bytes)| MemberRecord {
            path: path.clone(),
            size: bytes.len() as u64,
            sha256: sha256_hex(bytes),
        })
        .collect()
}

/* ------------------------------------------------------------------ */
/* Deterministic ustar                                                 */
/* ------------------------------------------------------------------ */

const BLOCK: usize = 512;

fn ustar_header(path: &str, size: usize) -> PackResult<[u8; BLOCK]> {
    if path.len() > 100 {
        return Err(PackError(format!(
            "path too long for ustar ({} > 100): {path}",
            path.len()
        )));
    }
    let mut h = [0u8; BLOCK];
    h[..path.len()].copy_from_slice(path.as_bytes());
    // mode 0644, uid 0, gid 0, mtime 0 — pinned for
    // byte-reproducibility.
    h[100..108].copy_from_slice(b"0000644\0");
    h[108..116].copy_from_slice(b"0000000\0");
    h[116..124].copy_from_slice(b"0000000\0");
    h[124..136].copy_from_slice(format!("{size:011o}\0").as_bytes());
    h[136..148].copy_from_slice(b"00000000000\0");
    h[156] = b'0'; // regular file
    h[257..263].copy_from_slice(b"ustar\0");
    h[263..265].copy_from_slice(b"00");
    // Checksum: sum of all header bytes with the
    // checksum field itself read as spaces.
    h[148..156].copy_from_slice(b"        ");
    let sum: u32 = h.iter().map(|&b| u32::from(b)).sum();
    h[148..156].copy_from_slice(format!("{sum:06o}\0 ").as_bytes());
    Ok(h)
}

/// Write members as an uncompressed ustar archive.
pub fn write_tar(members: &[(String, Vec<u8>)]) -> PackResult<Vec<u8>> {
    let mut out = Vec::new();
    for (path, bytes) in members {
        out.extend_from_slice(&ustar_header(path, bytes.len())?);
        out.extend_from_slice(bytes);
        let pad = (BLOCK - (bytes.len() % BLOCK)) % BLOCK;
        out.extend(std::iter::repeat_n(0u8, pad));
    }
    out.extend(std::iter::repeat_n(0u8, BLOCK * 2));
    Ok(out)
}

/// Read an uncompressed ustar archive back into members.
/// Refuses truncated archives and bad checksums.
pub fn read_tar(bytes: &[u8]) -> PackResult<Vec<(String, Vec<u8>)>> {
    if !bytes.len().is_multiple_of(BLOCK) {
        return Err(PackError(format!(
            "tar length {} is not a multiple of {BLOCK}",
            bytes.len()
        )));
    }
    let mut out = Vec::new();
    let mut offset = 0usize;
    while offset + BLOCK <= bytes.len() {
        let header = &bytes[offset..offset + BLOCK];
        if header.iter().all(|&b| b == 0) {
            break; // end-of-archive zero blocks
        }
        let stored_sum = parse_octal(&header[148..156]).unwrap_or(u32::MAX);
        let mut sum: u32 = 0;
        for (i, &b) in header.iter().enumerate() {
            sum += u32::from(if (148..156).contains(&i) { b' ' } else { b });
        }
        if sum != stored_sum {
            return Err(PackError(format!(
                "tar header checksum mismatch at block {}",
                offset / BLOCK
            )));
        }
        let name_end = header[..100].iter().position(|&b| b == 0).unwrap_or(100);
        let path = String::from_utf8_lossy(&header[..name_end]).into_owned();
        let size = parse_octal(&header[124..136])
            .map_err(|_| PackError(format!("tar size field unparseable for {path}")))?
            as usize;
        let data_start = offset + BLOCK;
        let data_end = data_start
            .checked_add(size)
            .filter(|&e| e <= bytes.len())
            .ok_or_else(|| PackError(format!("tar member {path} overruns the archive")))?;
        out.push((path, bytes[data_start..data_end].to_vec()));
        offset = data_start + size.div_ceil(BLOCK) * BLOCK;
    }
    Ok(out)
}

fn parse_octal(field: &[u8]) -> Result<u32, std::num::ParseIntError> {
    let text: String = field
        .iter()
        .take_while(|&&b| b != 0 && b != b' ')
        .map(|&b| b as char)
        .collect();
    u32::from_str_radix(text.trim(), 8)
}

/* ------------------------------------------------------------------ */
/* Package / record / verify                                           */
/* ------------------------------------------------------------------ */

/// gzip with a pinned header (mtime 0, no filename), so
/// identical tar bytes compress to identical archives.
fn gzip(bytes: &[u8]) -> PackResult<Vec<u8>> {
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder
        .write_all(bytes)
        .map_err(|e| PackError(format!("gzip write: {e}")))?;
    encoder
        .finish()
        .map_err(|e| PackError(format!("gzip finish: {e}")))
}

fn gunzip(bytes: &[u8]) -> PackResult<Vec<u8>> {
    let mut decoder = flate2::read::GzDecoder::new(bytes);
    let mut out = Vec::new();
    decoder
        .read_to_end(&mut out)
        .map_err(|e| PackError(format!("gunzip: {e}")))?;
    Ok(out)
}

/// Package a directory tree into a deterministic
/// `.tar.gz` plus its release record.
pub fn package_tree(
    root: &Path,
    project: &str,
    archive_file: &str,
) -> PackResult<(Vec<u8>, ReleaseRecord)> {
    let members = collect_tree(root)?;
    if members.is_empty() {
        return Err(PackError(format!(
            "release tree is empty: {}",
            root.display()
        )));
    }
    let record_members = manifest_of(&members);
    let tar = write_tar(&members)?;
    let archive = gzip(&tar)?;
    let record = ReleaseRecord {
        format: RELEASE_FORMAT.to_string(),
        project: project.to_string(),
        archive_file: archive_file.to_string(),
        archive_size: archive.len() as u64,
        archive_sha256: sha256_hex(&archive),
        members: record_members,
    };
    Ok((archive, record))
}

/// Package an in-memory member set (the same law,
/// without a filesystem tree behind it).
pub fn package_members(
    members: &[(String, Vec<u8>)],
    project: &str,
    archive_file: &str,
) -> PackResult<(Vec<u8>, ReleaseRecord)> {
    let mut sorted = members.to_vec();
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let tar = write_tar(&sorted)?;
    let archive = gzip(&tar)?;
    let record = ReleaseRecord {
        format: RELEASE_FORMAT.to_string(),
        project: project.to_string(),
        archive_file: archive_file.to_string(),
        archive_size: archive.len() as u64,
        archive_sha256: sha256_hex(&archive),
        members: manifest_of(&sorted),
    };
    Ok((archive, record))
}

/// Read an archive's members without a record — the
/// parse half of verification and record emission.
pub fn archive_members(archive: &[u8]) -> PackResult<Vec<(String, Vec<u8>)>> {
    let tar = gunzip(archive)?;
    read_tar(&tar)
}

/// Verify an archive against its release record.
/// Returns the list of discrepancies; empty means the
/// archive is exactly what the record says it is.
pub fn verify_archive(archive: &[u8], record: &ReleaseRecord) -> PackResult<Vec<String>> {
    let mut problems = Vec::new();
    if record.format != RELEASE_FORMAT {
        problems.push(format!(
            "record format {:?} is not {RELEASE_FORMAT}",
            record.format
        ));
    }
    if archive.len() as u64 != record.archive_size {
        problems.push(format!(
            "archive size {} != record {}",
            archive.len(),
            record.archive_size
        ));
    }
    let archive_hash = sha256_hex(archive);
    if archive_hash != record.archive_sha256 {
        problems.push(format!(
            "archive sha256 {archive_hash} != record {}",
            record.archive_sha256
        ));
    }
    let members = archive_members(archive)?;
    let actual = manifest_of(&members);
    let expected: BTreeMap<&str, &MemberRecord> = record
        .members
        .iter()
        .map(|m| (m.path.as_str(), m))
        .collect();
    let actual_map: BTreeMap<&str, &MemberRecord> =
        actual.iter().map(|m| (m.path.as_str(), m)).collect();
    for (path, want) in &expected {
        match actual_map.get(path) {
            None => problems.push(format!("member missing from archive: {path}")),
            Some(got) => {
                if got.size != want.size {
                    problems.push(format!(
                        "member {path}: size {} != record {}",
                        got.size, want.size
                    ));
                }
                if got.sha256 != want.sha256 {
                    problems.push(format!("member {path}: sha256 mismatch"));
                }
            }
        }
    }
    for path in actual_map.keys() {
        if !expected.contains_key(path) {
            problems.push(format!("member in archive but not in record: {path}"));
        }
    }
    Ok(problems)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_members() -> Vec<(String, Vec<u8>)> {
        vec![
            (
                "scenes/lift.scene.ron".to_string(),
                b"(version: 1)".to_vec(),
            ),
            ("bin/host".to_string(), vec![7u8; 1025]),
            ("README.md".to_string(), b"release".to_vec()),
        ]
    }

    #[test]
    fn packaging_is_byte_deterministic() {
        let members = sample_members();
        let (a, _) = package_members(&members, "demo", "demo.tar.gz").unwrap();
        let (b, _) = package_members(&members, "demo", "demo.tar.gz").unwrap();
        assert_eq!(a, b, "same members must package to identical bytes");
    }

    #[test]
    fn round_trip_verifies_clean() {
        let members = sample_members();
        let (archive, record) = package_members(&members, "demo", "demo.tar.gz").unwrap();
        let problems = verify_archive(&archive, &record).unwrap();
        assert!(problems.is_empty(), "unexpected problems: {problems:?}");
        assert_eq!(record.members.len(), 3);
    }

    #[test]
    fn tampered_archive_is_named() {
        let members = sample_members();
        let (mut archive, record) = package_members(&members, "demo", "demo.tar.gz").unwrap();
        let mid = archive.len() / 2;
        archive[mid] ^= 0xFF;
        // Verification either parses and reports the hash
        // disagreement, or the gzip layer refuses — both
        // are named failures, never silent passes.
        if let Ok(problems) = verify_archive(&archive, &record) {
            assert!(!problems.is_empty());
        }
    }

    #[test]
    fn record_mismatch_is_named() {
        let members = sample_members();
        let (archive, mut record) = package_members(&members, "demo", "demo.tar.gz").unwrap();
        record.members[0].sha256 = "0".repeat(64);
        let problems = verify_archive(&archive, &record).unwrap();
        assert!(
            problems.iter().any(|p| p.contains("sha256")),
            "problems: {problems:?}"
        );
    }

    #[test]
    fn tar_round_trip_preserves_members() {
        let members = sample_members();
        let tar = write_tar(&members).unwrap();
        let back = read_tar(&tar).unwrap();
        assert_eq!(back, members);
    }
}
