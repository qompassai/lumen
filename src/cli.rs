//! CLI batch commands: `split`, `pack`, `clean`, `watch`.
//!
//! The MCP server remains the primary surface; these subcommands expose the
//! same pipeline functions for terminal/Neovim use. Library functions stay
//! pure and progress-bar-free — all `indicatif`/`rayon` orchestration lives
//! here, so tests exercise logic without terminal widgets.
//!
//! Contract summary:
//! - All batch inputs are bounded: `MAX_BATCH_FILES` files, each within
//!   `MAX_SPRITE_BYTES`, dimensions within `MAX_IMAGE_DIMENSION`.
//! - Outputs are written atomically (temp file + rename); a failed batch
//!   never leaves a half-written PNG behind.
//! - `watch` debounces per path (`WATCH_DEBOUNCE_MS`) so editor save bursts
//!   trigger one pipeline run, not five.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use indicatif::{ProgressBar, ProgressStyle};
use rayon::prelude::*;

use crate::animation::{
    self, IDLE_GIF_HEIGHT, IDLE_GIF_WIDTH, clean_transparency_cc, expand_tag_frames,
    sanitize_tag_name,
};
use crate::aseprite::{self, TagDirection};
use crate::pack::{self, PackRequest};
use crate::{LumenError, MAX_SPRITE_BYTES};

/// Upper bound on files processed per batch command.
pub const MAX_BATCH_FILES: usize = 4096;
/// Sanity bound on image dimensions for batch inputs.
pub const MAX_IMAGE_DIMENSION: u32 = 8192;
/// Per-path debounce window for the watcher: editor save bursts collapse.
pub const WATCH_DEBOUNCE_MS: u64 = 750;


// ---------------------------------------------------------------------------
// split-tags
// ---------------------------------------------------------------------------

/// One exported tag's report.
#[derive(Debug, Clone)]
pub struct SplitTagsReport {
    /// Sanitized tag name (deduplicated).
    pub name: String,
    /// Loop direction from the Aseprite tag.
    pub direction: TagDirection,
    /// Frame indices in export order.
    pub frames: Vec<u32>,
    /// Output directory (png mode) or file (gif mode).
    pub output: PathBuf,
}

/// Export one animation per tag from a `.aseprite` file.
///
/// Contract:
/// - `format`: `"png"` writes `<out>/<name>/frame_*.png` + JSON sidecar;
///   `"gif"` writes `<out>/<name>.gif`.
/// - Tag names are sanitized; duplicates get `-2`, `-3` suffixes.
/// - `prefix` is prepended to every output name.
/// - Refuses to write into a non-empty `out_dir` unless `force`.
/// - Frame durations come from the Aseprite file; GIFs honor them.
/// - Fails closed on corrupt tag ranges, missing tags, I/O errors.
pub fn run_split_tags(
    input: &Path,
    out_dir: &Path,
    format: &str,
    prefix: &str,
    force: bool,
) -> Result<Vec<SplitTagsReport>, LumenError> {
    if format != "png" && format != "gif" {
        return Err(LumenError::BadParam(format!(
            "format must be \"png\" or \"gif\", got \"{format}\""
        )));
    }
    if prefix.len() > 64 {
        return Err(LumenError::BadParam(format!(
            "prefix too long ({} > 64 chars)",
            prefix.len()
        )));
    }
    // Validate prefix characters (same rules as tag sanitization).
    if !prefix
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(LumenError::BadParam(format!(
            "prefix contains invalid characters: \"{prefix}\""
        )));
    }

    let doc = aseprite::load_aseprite(input)?;
    let summary = aseprite::summarize(&doc)?;
    if summary.tags.is_empty() {
        return Err(LumenError::BadParam(format!(
            "no tags in {}: use `split` for untagged strips",
            input.display()
        )));
    }

    // Refuse non-empty output dir unless forced.
    if out_dir.exists() {
        let is_empty = std::fs::read_dir(out_dir)
            .map_err(|e| LumenError::Io(format!("reading {}: {e}", out_dir.display())))?
            .next()
            .is_none();
        if !is_empty && !force {
            return Err(LumenError::BadParam(format!(
                "{} is not empty: pass --force to overwrite",
                out_dir.display()
            )));
        }
    }
    std::fs::create_dir_all(out_dir)
        .map_err(|e| LumenError::Io(format!("creating {}: {e}", out_dir.display())))?;

    // Deduplicate sanitized names.
    let mut seen: HashMap<String, u32> = HashMap::new();
    let mut reports = Vec::with_capacity(summary.tags.len());
    let pb = progress_bar(summary.tags.len() as u64, "split-tags");

    for tag in &summary.tags {
        // Validate range against actual frame count (fail closed on corrupt files).
        if tag.to_frame >= summary.frames {
            return Err(LumenError::DocInvalid(format!(
                "tag \"{}\" range {}..={} exceeds {} frames",
                tag.name, tag.from_frame, tag.to_frame, summary.frames
            )));
        }
        let frame_indices =
            expand_tag_frames(tag.from_frame, tag.to_frame, tag.direction)?;

        // Sanitize + deduplicate name.
        let base = sanitize_tag_name(&tag.name);
        let count = seen.entry(base.clone()).or_insert(0);
        *count += 1;
        let deduped = if *count == 1 {
            base
        } else {
            format!("{base}-{}", *count)
        };
        let out_name = format!("{prefix}{deduped}");

        // Render frames (parallel; order preserved by index).
        let rendered: Result<Vec<(u32, image::RgbaImage)>, LumenError> = frame_indices
            .par_iter()
            .map(|&f| {
                let img = aseprite::frame_image(&doc, f)?;
                let duration_ms = doc.frame(f).duration();
                Ok((duration_ms, img))
            })
            .collect();
        let rendered = rendered?;

        let output = if format == "png" {
            let tag_dir = out_dir.join(&out_name);
            std::fs::create_dir_all(&tag_dir).map_err(|e| {
                LumenError::Io(format!("creating {}: {e}", tag_dir.display()))
            })?;
            for (i, (_, img)) in rendered.iter().enumerate() {
                let path = tag_dir.join(format!("frame_{i:03}.png"));
                write_png_atomic(&path, img)?;
            }
            // JSON sidecar with durations.
            let sidecar: Vec<serde_json::Value> = rendered
                .iter()
                .enumerate()
                .map(|(i, (dur, _))| {
                    serde_json::json!({
                        "file": format!("frame_{i:03}.png"),
                        "duration_ms": dur,
                    })
                })
                .collect();
            let meta = serde_json::json!({
                "name": tag.name,
                "direction": format!("{:?}", tag.direction).to_lowercase(),
                "frames": sidecar,
            });
            let sidecar_path = out_dir.join(format!("{out_name}.json"));
            let tmp = sidecar_path.with_extension("json.tmp");
            std::fs::write(&tmp, serde_json::to_string_pretty(&meta).unwrap())
                .map_err(|e| {
                    LumenError::Io(format!("writing {}: {e}", tmp.display()))
                })?;
            std::fs::rename(&tmp, &sidecar_path).map_err(|e| {
                LumenError::Io(format!("renaming {}: {e}", tmp.display()))
            })?;
            tag_dir
        } else {
            // GIF mode: per-frame durations honored.
            let gif_path = out_dir.join(format!("{out_name}.gif"));
            write_gif_with_delays(
                &gif_path,
                &rendered.iter().map(|(_, img)| img.clone()).collect::<Vec<_>>(),
                &rendered.iter().map(|(d, _)| *d).collect::<Vec<_>>(),
            )?;
            gif_path
        };

        reports.push(SplitTagsReport {
            name: out_name,
            direction: tag.direction,
            frames: frame_indices,
            output,
        });
        pb.inc(1);
    }
    pb.finish_and_clear();
    Ok(reports)
}

/// Write a GIF with per-frame durations (ms).
fn write_gif_with_delays(
    path: &Path,
    frames: &[image::RgbaImage],
    durations_ms: &[u32],
) -> Result<(), LumenError> {
    use image::codecs::gif::{GifEncoder, Repeat};
    use image::{Delay, ImageBuffer};

    if frames.is_empty() {
        return Err(LumenError::BadParam("no frames for GIF".into()));
    }
    let (w, h) = (frames[0].width(), frames[0].height());
    // Atomic write: temp file + rename.
    let tmp = path.with_extension("gif.tmp");
    let file = std::fs::File::create(&tmp)
        .map_err(|e| LumenError::Io(format!("creating {}: {e}", tmp.display())))?;
    let mut encoder = GifEncoder::new(file);
    encoder
        .set_repeat(Repeat::Infinite)
        .map_err(|e| LumenError::Io(format!("setting GIF repeat: {e}")))?;
    for (frame, &dur) in frames.iter().zip(durations_ms.iter()) {
        if frame.width() != w || frame.height() != h {
            return Err(LumenError::BadParam("GIF frame size mismatch".into()));
        }
        let delay = Delay::from_numer_denom_ms(dur.max(20), 1);
        let buffer: ImageBuffer<image::Rgba<u8>, Vec<u8>> =
            ImageBuffer::from_fn(w, h, |x, y| *frame.get_pixel(x, y));
        let gif_frame = image::Frame::from_parts(buffer, 0, 0, delay);
        encoder
            .encode_frame(gif_frame)
            .map_err(|e| LumenError::Io(format!("encoding GIF frame: {e}")))?;
    }
    drop(encoder);
    std::fs::rename(&tmp, path)
        .map_err(|e| LumenError::Io(format!("renaming {}: {e}", tmp.display())))?;
    Ok(())
}

/// What the watcher should do when a path changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatchAction {
    /// Clean transparency on a PNG, mirroring `clean`.
    CleanImage,
    /// Extract frames from an `.aseprite` file.
    ExtractAseprite,
    /// Not a pipeline input; ignore.
    Ignore,
}

/// Pure classification for watcher events: PNG -> clean, `.aseprite` ->
/// extract, anything else -> ignore. Case-insensitive extensions.
pub fn classify_watch_event(path: &Path) -> WatchAction {
    match path.extension().and_then(|e| e.to_str()) {
        Some(ext) if ext.eq_ignore_ascii_case("png") => WatchAction::CleanImage,
        Some(ext) if ext.eq_ignore_ascii_case("aseprite") => WatchAction::ExtractAseprite,
        _ => WatchAction::Ignore,
    }
}

/// Pure debounce decision: process when no previous run, or the window elapsed.
pub fn should_process(last: Option<Instant>, now: Instant) -> bool {
    match last {
        None => true,
        Some(t) => now.duration_since(t) >= Duration::from_millis(WATCH_DEBOUNCE_MS),
    }
}

/// Write a PNG atomically: temp file in the same directory, then rename.
/// A crash mid-write leaves the temp file, never a half-written output.
///
/// Uses the PNG encoder directly because the `.tmp` suffix defeats
/// `image::save`'s extension-based format detection.
fn write_png_atomic(path: &Path, img: &image::RgbaImage) -> Result<(), LumenError> {
    use image::ImageEncoder;
    use image::codecs::png::PngEncoder;

    let tmp = PathBuf::from(format!("{}.tmp", path.display()));
    let file = std::fs::File::create(&tmp)
        .map_err(|e| LumenError::Io(format!("creating {}: {e}", tmp.display())))?;
    PngEncoder::new(file)
        .write_image(
            img.as_raw(),
            img.width(),
            img.height(),
            image::ExtendedColorType::Rgba8,
        )
        .map_err(|e| LumenError::Io(format!("encoding {}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        LumenError::Io(format!("publishing {}: {e}", path.display()))
    })
}

/// Write bytes atomically (JSON sidecars).
fn write_bytes_atomic(path: &Path, bytes: &[u8]) -> Result<(), LumenError> {
    let tmp = PathBuf::from(format!("{}.tmp", path.display()));
    std::fs::write(&tmp, bytes)
        .map_err(|e| LumenError::Io(format!("writing {}: {e}", tmp.display())))?;
    std::fs::rename(&tmp, path).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        LumenError::Io(format!("publishing {}: {e}", path.display()))
    })
}

/// Load an image with size/dimension bounds checked before decode.
fn load_image_bounded(path: &Path) -> Result<image::RgbaImage, LumenError> {
    let meta = std::fs::metadata(path)
        .map_err(|e| LumenError::Io(format!("reading {}: {e}", path.display())))?;
    if meta.len() > MAX_SPRITE_BYTES {
        return Err(LumenError::BadParam(format!(
            "{} exceeds {MAX_SPRITE_BYTES} bytes",
            path.display()
        )));
    }
    let img = image::open(path)
        .map_err(|e| LumenError::BadParam(format!("decoding {}: {e}", path.display())))?;
    let rgba = img.to_rgba8();
    if rgba.width() > MAX_IMAGE_DIMENSION || rgba.height() > MAX_IMAGE_DIMENSION {
        return Err(LumenError::BadParam(format!(
            "{} exceeds {MAX_IMAGE_DIMENSION}px bound",
            path.display()
        )));
    }
    Ok(rgba)
}

/// Sorted PNG files in a directory, bounded. Deterministic order.
fn png_files_sorted(dir: &Path) -> Result<Vec<PathBuf>, LumenError> {
    let meta = std::fs::metadata(dir)
        .map_err(|e| LumenError::Io(format!("reading {}: {e}", dir.display())))?;
    if !meta.is_dir() {
        return Err(LumenError::BadParam(format!(
            "{} is not a directory",
            dir.display()
        )));
    }
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map_err(|e| LumenError::Io(format!("listing {}: {e}", dir.display())))?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.is_file()
                && p.extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| e.eq_ignore_ascii_case("png"))
        })
        .collect();
    files.sort();
    if files.len() > MAX_BATCH_FILES {
        return Err(LumenError::BadParam(format!(
            "too many files in {}: {} > {MAX_BATCH_FILES}",
            dir.display(),
            files.len()
        )));
    }
    Ok(files)
}

fn progress_bar(len: u64, what: &str) -> ProgressBar {
    let pb = ProgressBar::new(len);
    let style = ProgressStyle::with_template(
        "{spinner:.green} [{elapsed_precise}] [{bar:40.cyan/blue}] {pos}/{len} {msg}",
    )
    .unwrap_or_else(|_| ProgressStyle::default_bar());
    pb.set_style(style);
    pb.set_message(what.to_string());
    pb
}

/// `lumen split`: strip -> cleaned, centered frame PNGs in `out_dir`.
pub fn run_split(
    input: &Path,
    frames: u32,
    out_dir: &Path,
    width: u32,
    height: u32,
) -> Result<Vec<PathBuf>, LumenError> {
    if !(1..=64).contains(&frames) {
        return Err(LumenError::BadParam(format!(
            "frames must be 1..=64, got {frames}"
        )));
    }
    if width == 0 || width > MAX_IMAGE_DIMENSION || height == 0 || height > MAX_IMAGE_DIMENSION {
        return Err(LumenError::BadParam(format!(
            "target size {width}x{height} out of bounds"
        )));
    }
    let strip = load_image_bounded(input)?;
    // rayon inside split_strip parallelizes frames; order is preserved.
    let split = animation::split_strip(&strip, frames, width, height)?;
    std::fs::create_dir_all(out_dir)
        .map_err(|e| LumenError::Io(format!("creating {}: {e}", out_dir.display())))?;
    let pb = progress_bar(split.len() as u64, "split");
    let mut written = Vec::with_capacity(split.len());
    for (i, frame) in split.iter().enumerate() {
        let path = out_dir.join(format!("frame_{i:03}.png"));
        write_png_atomic(&path, frame)?;
        written.push(path);
        pb.inc(1);
    }
    pb.finish_and_clear();
    Ok(written)
}

/// Report from `lumen pack`.
#[derive(Debug, Clone)]
pub struct PackReport {
    /// Sheet PNG path.
    pub sheet: PathBuf,
    /// JSON sidecar path.
    pub sidecar: PathBuf,
    /// Bin size actually used (grows by doubling when needed).
    pub bin_size: u32,
    /// Frames packed, in input order.
    pub frames: Vec<PackReportFrame>,
}

/// One packed frame's placement.
#[derive(Debug, Clone)]
pub struct PackReportFrame {
    pub name: String,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

/// `lumen pack`: PNGs in `dir` -> sheet PNG + JSON sidecar at `out`.
///
/// `out` names the sheet (`out.png`); the sidecar is written alongside as
/// `out.json`. The bin starts at `bin_size` and doubles until everything
/// fits (capped at `pack::MAX_BIN_DIMENSION`).
pub fn run_pack(dir: &Path, out: &Path, bin_size: u32) -> Result<PackReport, LumenError> {
    if bin_size == 0 || bin_size > pack::MAX_BIN_DIMENSION {
        return Err(LumenError::BadParam(format!(
            "bin_size {bin_size} out of bounds"
        )));
    }
    let files = png_files_sorted(dir)?;
    if files.is_empty() {
        return Err(LumenError::BadParam(format!(
            "no PNGs in {}",
            dir.display()
        )));
    }
    let pb = progress_bar(files.len() as u64, "load");
    // Parallel load; sort by name afterwards for determinism.
    let mut named: Vec<(String, image::RgbaImage)> = files
        .par_iter()
        .map(|path| {
            let img = load_image_bounded(path)?;
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("frame")
                .to_string();
            pb.inc(1);
            Ok::<_, LumenError>((name, img))
        })
        .collect::<Result<Vec<_>, _>>()?;
    pb.finish_and_clear();
    named.sort_by(|a, b| a.0.cmp(&b.0));

    let requests: Vec<PackRequest> = named
        .iter()
        .enumerate()
        .map(|(i, (_, img))| PackRequest {
            id: i,
            width: img.width(),
            height: img.height(),
        })
        .collect();
    let (placements, used_bin) =
        pack::pack_sprites_grow(&requests, bin_size, pack::MAX_BIN_DIMENSION)?;

    let mut sheet = image::RgbaImage::new(used_bin, used_bin);
    let mut frames = Vec::with_capacity(named.len());
    for p in &placements {
        let (name, img) = &named[p.id];
        image::imageops::overlay(&mut sheet, img, p.x as i64, p.y as i64);
        frames.push(PackReportFrame {
            name: name.clone(),
            x: p.x,
            y: p.y,
            width: p.width,
            height: p.height,
        });
    }
    let sheet_path = out.with_extension("png");
    let sidecar_path = out.with_extension("json");
    if let Some(parent) = sheet_path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)
            .map_err(|e| LumenError::Io(format!("creating {}: {e}", parent.display())))?;
    }
    write_png_atomic(&sheet_path, &sheet)?;
    let sidecar = serde_json::json!({
        "sheet": sheet_path.file_name().and_then(|n| n.to_str()).unwrap_or("sheet.png"),
        "bin": {"width": used_bin, "height": used_bin},
        "frames": frames.iter().map(|f| serde_json::json!({
            "name": f.name, "x": f.x, "y": f.y, "w": f.width, "h": f.height,
        })).collect::<Vec<_>>(),
    });
    let sidecar_bytes = serde_json::to_string_pretty(&sidecar)
        .map_err(|e| LumenError::Io(format!("serializing sidecar: {e}")))?;
    write_bytes_atomic(&sidecar_path, sidecar_bytes.as_bytes())?;
    Ok(PackReport {
        sheet: sheet_path,
        sidecar: sidecar_path,
        bin_size: used_bin,
        frames,
    })
}

/// `lumen clean`: transparency cleanup on every PNG in `dir`, into `out_dir`.
///
/// Always applies `clean_transparency`; `use_cc` additionally keys the
/// border-connected background via connected components (for white panels).
pub fn run_clean(
    dir: &Path,
    out_dir: &Path,
    use_cc: bool,
    use_magenta: bool,
) -> Result<Vec<PathBuf>, LumenError> {
    let files = png_files_sorted(dir)?;
    std::fs::create_dir_all(out_dir)
        .map_err(|e| LumenError::Io(format!("creating {}: {e}", out_dir.display())))?;
    let pb = progress_bar(files.len() as u64, "clean");
    let written: Vec<PathBuf> = files
        .par_iter()
        .map(|path| {
            let mut img = load_image_bounded(path)?;
            if use_magenta {
                img = animation::remove_magenta_background(&img);
            } else {
                animation::clean_transparency(&mut img);
                if use_cc {
                    clean_transparency_cc(&mut img);
                }
            }
            let name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("out.png");
            let dest = out_dir.join(name);
            write_png_atomic(&dest, &img)?;
            pb.inc(1);
            Ok::<_, LumenError>(dest)
        })
        .collect::<Result<Vec<_>, _>>()?;
    pb.finish_and_clear();
    let mut sorted = written;
    sorted.sort();
    Ok(sorted)
}

/// Extract every frame of an `.aseprite` file as PNGs into `out_dir`.
/// Returns the written frame paths in order.
pub fn run_extract_aseprite(input: &Path, out_dir: &Path) -> Result<Vec<PathBuf>, LumenError> {
    let doc = aseprite::load_aseprite(input)?;
    let summary = aseprite::summarize(&doc)?;
    std::fs::create_dir_all(out_dir)
        .map_err(|e| LumenError::Io(format!("creating {}: {e}", out_dir.display())))?;
    let pb = progress_bar(u64::from(summary.frames), "extract");
    let mut written = Vec::with_capacity(summary.frames as usize);
    for f in 0..summary.frames {
        let img = aseprite::frame_image(&doc, f)?;
        let path = out_dir.join(format!("frame_{f:03}.png"));
        write_png_atomic(&path, &img)?;
        written.push(path);
        pb.inc(1);
    }
    pb.finish_and_clear();
    Ok(written)
}

/// `lumen watch`: block forever watching `dir`; on PNG change run the clean
/// pipeline into `<dir>/cleaned/`, on `.aseprite` change extract frames into
/// `<dir>/frames/<stem>/`. Per-path debounced. Runs until killed.
pub fn run_watch(dir: &Path) -> Result<(), LumenError> {
    use notify::{Config, EventKind, RecommendedWatcher, RecursiveMode, Watcher};

    let meta = std::fs::metadata(dir)
        .map_err(|e| LumenError::Io(format!("reading {}: {e}", dir.display())))?;
    if !meta.is_dir() {
        return Err(LumenError::BadParam(format!(
            "{} is not a directory",
            dir.display()
        )));
    }
    let (tx, rx) = std::sync::mpsc::channel();
    let mut watcher: RecommendedWatcher = RecommendedWatcher::new(tx, Config::default())
        .map_err(|e| LumenError::Io(format!("creating watcher: {e}")))?;
    watcher
        .watch(dir, RecursiveMode::Recursive)
        .map_err(|e| LumenError::Io(format!("watching {}: {e}", dir.display())))?;
    eprintln!("watching {} (Ctrl-C to stop)", dir.display());

    let mut last_run: HashMap<PathBuf, Instant> = HashMap::new();
    for event in rx {
        let event = match event {
            Ok(e) => e,
            Err(e) => {
                eprintln!("watch error: {e}");
                continue;
            }
        };
        let relevant = matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_));
        if !relevant {
            continue;
        }
        for path in event.paths {
            let now = Instant::now();
            let last = last_run.get(&path).copied();
            if !should_process(last, now) {
                continue;
            }
            last_run.insert(path.clone(), now);
            match classify_watch_event(&path) {
                WatchAction::Ignore => {}
                WatchAction::CleanImage => {
                    let dest_dir = dir.join("cleaned");
                    match run_clean(path.parent().unwrap_or(dir), &dest_dir, false, false) {
                        Ok(_) => eprintln!("cleaned -> {}", dest_dir.display()),
                        Err(e) => eprintln!("clean failed for {}: {e}", path.display()),
                    }
                }
                WatchAction::ExtractAseprite => {
                    let stem = path
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("frames");
                    let dest_dir = dir.join("frames").join(stem);
                    match run_extract_aseprite(&path, &dest_dir) {
                        Ok(frames) => eprintln!(
                            "extracted {} frames -> {}",
                            frames.len(),
                            dest_dir.display()
                        ),
                        Err(e) => eprintln!("extract failed for {}: {e}", path.display()),
                    }
                }
            }
        }
    }
    Ok(())
}

// Re-export the idle defaults so CLI help stays in sync with the library.
pub const CLI_DEFAULT_WIDTH: u32 = IDLE_GIF_WIDTH;
pub const CLI_DEFAULT_HEIGHT: u32 = IDLE_GIF_HEIGHT;

#[cfg(test)]
mod cli_tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    fn write_png(dir: &Path, name: &str, img: &RgbaImage) {
        img.save(dir.join(name)).unwrap();
    }

    fn temp_case(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lumen-cli-tests-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    // Validation: split writes one PNG per frame
    #[test]
    fn split_writes_frames() {
        let dir = temp_case("split");
        let strip = RgbaImage::from_pixel(120, 30, Rgba([255, 0, 0, 255]));
        let input = dir.join("strip.png");
        strip.save(&input).unwrap();
        let out = dir.join("frames");
        let written = run_split(&input, 4, &out, 144, 288).unwrap();
        assert_eq!(written.len(), 4);
        for p in &written {
            assert!(p.exists());
        }
    }

    // Validation: clean copies with transparency applied
    #[test]
    fn clean_writes_cleaned() {
        let dir = temp_case("clean");
        let src = dir.join("src");
        std::fs::create_dir_all(&src).unwrap();
        let mut img = RgbaImage::from_pixel(8, 8, Rgba([255, 0, 0, 255]));
        img.put_pixel(0, 0, Rgba([255, 255, 255, 10])); // faint trace
        write_png(&src, "a.png", &img);
        let out = dir.join("out");
        let written = run_clean(&src, &out, false, false).unwrap();
        assert_eq!(written.len(), 1);
        let back = image::open(&written[0]).unwrap().to_rgba8();
        assert_eq!(back.get_pixel(0, 0)[3], 0); // trace killed
        assert_eq!(back.get_pixel(4, 4)[3], 255); // solid kept
    }

    // Validation: pack produces sheet + sidecar with placements
    #[test]
    fn pack_produces_sheet_and_sidecar() {
        let dir = temp_case("pack");
        let src = dir.join("src");
        std::fs::create_dir_all(&src).unwrap();
        write_png(
            &src,
            "a.png",
            &RgbaImage::from_pixel(32, 32, Rgba([255, 0, 0, 255])),
        );
        write_png(
            &src,
            "b.png",
            &RgbaImage::from_pixel(16, 48, Rgba([0, 255, 0, 255])),
        );
        let report = run_pack(&src, &dir.join("sheet"), 128).unwrap();
        assert!(report.sheet.exists());
        assert!(report.sidecar.exists());
        assert_eq!(report.frames.len(), 2);
        let sidecar: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&report.sidecar).unwrap()).unwrap();
        assert_eq!(sidecar["frames"].as_array().unwrap().len(), 2);
        assert_eq!(sidecar["frames"][0]["name"], "a.png");
    }

    // Validation: pack grows the bin when the start size is too small
    #[test]
    fn pack_grows_bin() {
        let dir = temp_case("pack-grow");
        let src = dir.join("src");
        std::fs::create_dir_all(&src).unwrap();
        write_png(
            &src,
            "a.png",
            &RgbaImage::from_pixel(100, 100, Rgba([255, 0, 0, 255])),
        );
        let report = run_pack(&src, &dir.join("sheet"), 64).unwrap();
        assert!(report.bin_size >= 128);
        assert_eq!(report.frames.len(), 1);
    }

    // Adversarial: split rejects bad frame counts
    #[test]
    fn split_rejects_bad_frames() {
        let dir = temp_case("split-bad");
        let input = dir.join("s.png");
        RgbaImage::from_pixel(10, 10, Rgba([0, 0, 0, 255]))
            .save(&input)
            .unwrap();
        assert!(run_split(&input, 0, &dir.join("o"), 144, 288).is_err());
        assert!(run_split(&input, 65, &dir.join("o"), 144, 288).is_err());
    }

    // Adversarial: split on missing input is an error, not a panic
    #[test]
    fn split_missing_input() {
        let dir = temp_case("split-missing");
        assert!(run_split(&dir.join("nope.png"), 4, &dir.join("o"), 144, 288).is_err());
    }

    // Adversarial: pack on empty dir rejected
    #[test]
    fn pack_empty_dir_rejected() {
        let dir = temp_case("pack-empty");
        let src = dir.join("src");
        std::fs::create_dir_all(&src).unwrap();
        assert!(run_pack(&src, &dir.join("sheet"), 128).is_err());
    }

    // Adversarial: pack on missing dir rejected
    #[test]
    fn pack_missing_dir_rejected() {
        let dir = temp_case("pack-missing");
        assert!(run_pack(&dir.join("nope"), &dir.join("sheet"), 128).is_err());
    }

    // Validation: watch classification
    #[test]
    fn classify_events() {
        assert_eq!(
            classify_watch_event(Path::new("a.png")),
            WatchAction::CleanImage
        );
        assert_eq!(
            classify_watch_event(Path::new("DIR/B.ASEPRITE")),
            WatchAction::ExtractAseprite
        );
        assert_eq!(
            classify_watch_event(Path::new("notes.txt")),
            WatchAction::Ignore
        );
        assert_eq!(
            classify_watch_event(Path::new("noext")),
            WatchAction::Ignore
        );
    }

    // Validation: debounce passes first event, blocks rapid repeats
    #[test]
    fn debounce_windows() {
        let now = Instant::now();
        assert!(should_process(None, now));
        assert!(!should_process(Some(now), now));
        assert!(should_process(
            Some(now - Duration::from_millis(WATCH_DEBOUNCE_MS + 1)),
            now
        ));
    }

    // Adversarial: watch on missing dir rejected (does not block forever)
    #[test]
    fn watch_missing_dir_rejected() {
        let dir = temp_case("watch-missing");
        assert!(run_watch(&dir.join("nope")).is_err());
    }
}
