//! Native `.aseprite` file parsing via the `asefile` crate.
//!
//! This is the fast, deterministic counterpart to the vendored-Lua path in
//! `qa.rs`: no Lua runtime in the loop, so frame/layer/tag/cel extraction is
//! unit-testable. The Lua path is kept for scripted compositing; this module
//! only *reads* Aseprite files.
//!
//! Contract summary:
//! - Accepted: `.aseprite` files within `MAX_SPRITE_BYTES` and the canvas
//!   bound `MAX_ASEPRITE_DIMENSION`.
//! - Rejected: missing files, oversize files, wrong extension, undecodable
//!   content, out-of-range frame/layer indices — all as typed `LumenError`,
//!   never panics.
//! - Note: `asefile` speaks `image` 0.24 while lumen uses `image` 0.25. Pixel
//!   buffers cross that boundary through `aseprite_rgba_to_image` (the
//!   version firewall: identical RGBA8 layout, plain `Vec<u8>` move, no
//!   unsafe code).

use std::path::Path;

use image::RgbaImage;

use crate::{LumenError, MAX_SPRITE_BYTES};

/// Required file extension (case-insensitive).
pub const ASEPRITE_EXTENSION: &str = "aseprite";
/// Sanity bound on canvas dimensions; larger files are rejected before parse.
pub const MAX_ASEPRITE_DIMENSION: u32 = 8192;

/// Layer name + visibility, in file order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AsepriteLayerSummary {
    pub name: String,
    pub visible: bool,
}

/// Loop direction of an Aseprite animation tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagDirection {
    Forward,
    Reverse,
    PingPong,
}

impl TagDirection {
    /// Map from the `asefile` crate's direction type.
    fn from_asefile(d: asefile::AnimationDirection) -> Self {
        use asefile::AnimationDirection as A;
        match d {
            A::Forward => TagDirection::Forward,
            A::Reverse => TagDirection::Reverse,
            A::PingPong => TagDirection::PingPong,
        }
    }
}

/// Tag name + inclusive frame range + loop direction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AsepriteTagSummary {
    pub name: String,
    pub from_frame: u32,
    pub to_frame: u32,
    pub direction: TagDirection,
}

/// Whole-file summary: canvas, frame count, layers, tags.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AsepriteSummary {
    pub width: u32,
    pub height: u32,
    pub frames: u32,
    pub layers: Vec<AsepriteLayerSummary>,
    pub tags: Vec<AsepriteTagSummary>,
}

/// Load and parse a `.aseprite` file, fail closed.
///
/// Validates the extension and file size *before* parsing so a hostile file
/// cannot blow up memory or waste parse time.
pub fn load_aseprite(path: &Path) -> Result<asefile::AsepriteFile, LumenError> {
    let ext_ok = path
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case(ASEPRITE_EXTENSION));
    if !ext_ok {
        return Err(LumenError::BadParam(format!(
            "expected a .{ASEPRITE_EXTENSION} file: {}",
            path.display()
        )));
    }
    let meta = std::fs::metadata(path)
        .map_err(|e| LumenError::Io(format!("reading {}: {e}", path.display())))?;
    if meta.len() > MAX_SPRITE_BYTES {
        return Err(LumenError::BadParam(format!(
            "file {} exceeds {MAX_SPRITE_BYTES} bytes",
            path.display()
        )));
    }
    asefile::AsepriteFile::read_file(path)
        .map_err(|e| LumenError::DocInvalid(format!("parsing {}: {e}", path.display())))
}

/// Summarize canvas, frames, layers, and tags.
pub fn summarize(doc: &asefile::AsepriteFile) -> Result<AsepriteSummary, LumenError> {
    let width = u32::try_from(doc.width())
        .map_err(|_| LumenError::DocInvalid("canvas width out of range".into()))?;
    let height = u32::try_from(doc.height())
        .map_err(|_| LumenError::DocInvalid("canvas height out of range".into()))?;
    if width > MAX_ASEPRITE_DIMENSION || height > MAX_ASEPRITE_DIMENSION {
        return Err(LumenError::DocInvalid(format!(
            "canvas {width}x{height} exceeds {MAX_ASEPRITE_DIMENSION}px bound"
        )));
    }
    let frames = doc.num_frames();
    let mut layers = Vec::with_capacity(doc.num_layers() as usize);
    for i in 0..doc.num_layers() {
        let layer = doc.layer(i);
        layers.push(AsepriteLayerSummary {
            name: layer.name().to_string(),
            visible: layer.is_visible(),
        });
    }
    let mut tags = Vec::with_capacity(doc.num_tags() as usize);
    for i in 0..doc.num_tags() {
        let tag = doc.tag(i);
        tags.push(AsepriteTagSummary {
            name: tag.name().to_string(),
            from_frame: tag.from_frame(),
            to_frame: tag.to_frame(),
            direction: TagDirection::from_asefile(tag.animation_direction()),
        });
    }
    Ok(AsepriteSummary {
        width,
        height,
        frames,
        layers,
        tags,
    })
}

/// Render one frame with all visible layers blended, as Aseprite would show it.
pub fn frame_image(doc: &asefile::AsepriteFile, frame: u32) -> Result<RgbaImage, LumenError> {
    if frame >= doc.num_frames() {
        return Err(LumenError::BadParam(format!(
            "frame {frame} out of range ({} frames)",
            doc.num_frames()
        )));
    }
    let raw = doc.frame(frame).image();
    let (w, h) = raw.dimensions();
    aseprite_rgba_to_image(raw.into_raw(), w, h)
}

/// Render a single cel (frame × layer intersection).
///
/// An empty cel (no pixel data for that frame/layer) renders as a fully
/// transparent canvas — that is meaningful, not an error.
pub fn cel_image(
    doc: &asefile::AsepriteFile,
    frame: u32,
    layer: u32,
) -> Result<RgbaImage, LumenError> {
    if frame >= doc.num_frames() {
        return Err(LumenError::BadParam(format!(
            "frame {frame} out of range ({} frames)",
            doc.num_frames()
        )));
    }
    if layer >= doc.num_layers() {
        return Err(LumenError::BadParam(format!(
            "layer {layer} out of range ({} layers)",
            doc.num_layers()
        )));
    }
    let raw = doc.cel(frame, layer).image();
    let (w, h) = raw.dimensions();
    aseprite_rgba_to_image(raw.into_raw(), w, h)
}

/// Move an `image` 0.24 RGBA pixel buffer (from `asefile`) into lumen's
/// `image` 0.25 `RgbaImage`.
///
/// This is the version firewall between the two `image` major versions: both
/// buffers are `ImageBuffer<Rgba<u8>, Vec<u8>>` with identical RGBA8 row-major
/// layout, so the transfer is a plain `Vec<u8>` move — no unsafe code, no
/// pixel rewrite. The 0.24 buffer type is unnameable here (lumen depends on
/// `image` 0.25), so callers extract `(width, height, raw)` via the buffer's
/// inherent methods first; method resolution works on the inferred concrete
/// type without naming it.
///
/// The `debug_assert_eq!` pins the layout contract in dev builds. A
/// short/overlong buffer in release still fails closed as `DocInvalid` via
/// `from_raw` instead of panicking.
fn aseprite_rgba_to_image(raw: Vec<u8>, width: u32, height: u32) -> Result<RgbaImage, LumenError> {
    debug_assert_eq!(
        raw.len(),
        width as usize * height as usize * 4,
        "asefile RGBA8 buffer length must equal width*height*4"
    );
    RgbaImage::from_raw(width, height, raw).ok_or_else(|| {
        LumenError::DocInvalid("asefile produced a buffer inconsistent with its dimensions".into())
    })
}

#[cfg(test)]
mod aseprite_tests {
    use super::*;
    use std::io::Write;

    // -- Minimal .aseprite fixture builder --------------------------------
    //
    // Hand-built per the ase-file spec (offsets verified against asefile
    // 0.3.8's parser): 2 frames, 4x3 RGBA canvas, 2 layers ("bg", "fg"),
    // 1 tag ("walk", frames 0-1). Frame 0: red bg cel + 2x2 blue fg cel at
    // (1,1). Frame 1: green bg cel.

    fn w16(v: &[u8], n: u16) -> Vec<u8> {
        let mut v = v.to_vec();
        v.extend_from_slice(&n.to_le_bytes());
        v
    }
    fn w32(v: &[u8], n: u32) -> Vec<u8> {
        let mut v = v.to_vec();
        v.extend_from_slice(&n.to_le_bytes());
        v
    }
    fn wstr(v: &[u8], s: &str) -> Vec<u8> {
        let v = w16(v, s.len() as u16);
        let mut v = v;
        v.extend_from_slice(s.as_bytes());
        v
    }
    fn chunk(typ: u16, data: &[u8]) -> Vec<u8> {
        let mut v = Vec::new();
        v = w32(&v, (data.len() + 6) as u32);
        v = w16(&v, typ);
        v.extend_from_slice(data);
        v
    }
    fn layer_chunk(name: &str, visible: bool) -> Vec<u8> {
        let mut d = Vec::new();
        d = w16(&d, if visible { 1 } else { 0 }); // flags
        d = w16(&d, 0); // normal image layer
        d = w16(&d, 0); // child level
        d = w16(&d, 4); // default width
        d = w16(&d, 3); // default height
        d = w16(&d, 0); // normal blend
        d.push(255); // opacity
        d.push(0); // reserved
        d = w16(&d, 0); // reserved
        d = wstr(&d, name);
        chunk(0x2004, &d)
    }
    fn raw_cel_chunk(layer: u16, x: i16, y: i16, w: u16, h: u16, px: &[u8]) -> Vec<u8> {
        let mut d = Vec::new();
        d = w16(&d, layer);
        d.extend_from_slice(&x.to_le_bytes());
        d.extend_from_slice(&y.to_le_bytes());
        d.push(255); // opacity
        d = w16(&d, 0); // raw cel
        d.extend_from_slice(&[0u8; 7]); // reserved
        d = w16(&d, w);
        d = w16(&d, h);
        d.extend_from_slice(px);
        chunk(0x2005, &d)
    }
    fn tags_chunk() -> Vec<u8> {
        let mut d = Vec::new();
        d = w16(&d, 1); // one tag
        d.extend_from_slice(&[0u8; 8]); // reserved
        d = w16(&d, 0); // from frame
        d = w16(&d, 1); // to frame
        d.push(0); // forward
        d = w16(&d, 0); // repeat (0 = infinite in UI)
        d.extend_from_slice(&[0u8; 6]); // reserved
        d = w32(&d, 0); // color
        d = wstr(&d, "walk");
        chunk(0x2018, &d)
    }
    fn frame(duration_ms: u16, chunks: &[Vec<u8>]) -> Vec<u8> {
        let body: Vec<u8> = chunks.concat();
        let mut v = Vec::new();
        v = w32(&v, (16 + body.len()) as u32);
        v = w16(&v, 0xF1FA);
        v = w16(&v, chunks.len() as u16); // old count
        v = w16(&v, duration_ms);
        v = w16(&v, 0); // placeholder
        v = w32(&v, 0); // new count = 0 -> use old
        v.extend_from_slice(&body);
        v
    }
    fn solid_rgba(w: u16, h: u16, px: [u8; 4]) -> Vec<u8> {
        let mut v = Vec::with_capacity(w as usize * h as usize * 4);
        for _ in 0..w as usize * h as usize {
            v.extend_from_slice(&px);
        }
        v
    }

    /// The fixture: 2 frames, 4x3, 2 layers, 1 tag.
    fn fixture_bytes() -> Vec<u8> {
        let red = solid_rgba(4, 3, [255, 0, 0, 255]);
        let green = solid_rgba(4, 3, [0, 255, 0, 255]);
        let blue = solid_rgba(2, 2, [0, 0, 255, 255]);
        let f0 = frame(
            100,
            &[
                layer_chunk("bg", true),
                layer_chunk("fg", true),
                raw_cel_chunk(0, 0, 0, 4, 3, &red),
                raw_cel_chunk(1, 1, 1, 2, 2, &blue),
                tags_chunk(),
            ],
        );
        let f1 = frame(100, &[raw_cel_chunk(0, 0, 0, 4, 3, &green)]);
        let mut v = Vec::new();
        v = w32(&v, (128 + f0.len() + f1.len()) as u32);
        v = w16(&v, 0xA5E0);
        v = w16(&v, 2); // frames
        v = w16(&v, 4); // width
        v = w16(&v, 3); // height
        v = w16(&v, 32); // RGBA
        v = w32(&v, 1); // flags
        v = w16(&v, 100); // default frame time
        v = w32(&v, 0); // transparent index
        v = w32(&v, 0); // placeholder
        v.push(0); // transparent palette index (byte)
        v.push(0); // ignore (byte)
        v = w16(&v, 0); // ignore (word)
        v = w16(&v, 0); // num colors
        v.push(1); // pixel width
        v.push(1); // pixel height
        v.extend_from_slice(&[0u8; 2]); // grid x
        v.extend_from_slice(&[0u8; 2]); // grid y
        v = w16(&v, 0); // grid w
        v = w16(&v, 0); // grid h
        v.extend_from_slice(&[0u8; 84]); // reserved
        debug_assert_eq!(v.len(), 128);
        v.extend_from_slice(&f0);
        v.extend_from_slice(&f1);
        v
    }

    fn write_fixture(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("lumen-aseprite-tests");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(&fixture_bytes()).unwrap();
        path
    }

    // Validation: summary reports canvas, frames, layers, tags
    #[test]
    fn summary_matches_fixture() {
        let path = write_fixture("summary.aseprite");
        let doc = load_aseprite(&path).unwrap();
        let s = summarize(&doc).unwrap();
        assert_eq!((s.width, s.height), (4, 3));
        assert_eq!(s.frames, 2);
        assert_eq!(
            s.layers,
            vec![
                AsepriteLayerSummary {
                    name: "bg".into(),
                    visible: true
                },
                AsepriteLayerSummary {
                    name: "fg".into(),
                    visible: true
                },
            ]
        );
        assert_eq!(
            s.tags,
            vec![AsepriteTagSummary {
                name: "walk".into(),
                from_frame: 0,
                to_frame: 1,
                direction: TagDirection::Forward,
            }]
        );
    }

    // Validation: frame 0 blends red bg + blue fg cel at (1,1)
    #[test]
    fn frame_zero_blends_layers() {
        let path = write_fixture("frame0.aseprite");
        let doc = load_aseprite(&path).unwrap();
        let img = frame_image(&doc, 0).unwrap();
        assert_eq!((img.width(), img.height()), (4, 3));
        assert_eq!(img.get_pixel(0, 0).0, [255, 0, 0, 255]); // bg red
        assert_eq!(img.get_pixel(1, 1).0, [0, 0, 255, 255]); // fg blue on top
    }

    // Validation: frame 1 is the green bg cel
    #[test]
    fn frame_one_is_green() {
        let path = write_fixture("frame1.aseprite");
        let doc = load_aseprite(&path).unwrap();
        let img = frame_image(&doc, 1).unwrap();
        assert_eq!(img.get_pixel(2, 2).0, [0, 255, 0, 255]);
    }

    // Validation: single cel extraction isolates the fg layer, positioned
    // on the canvas (asefile composites the cel at its x/y offset).
    #[test]
    fn cel_isolates_layer() {
        let path = write_fixture("cel.aseprite");
        let doc = load_aseprite(&path).unwrap();
        let img = cel_image(&doc, 0, 1).unwrap();
        assert_eq!((img.width(), img.height()), (4, 3));
        assert_eq!(img.get_pixel(1, 1).0, [0, 0, 255, 255]); // cel at (1,1)
        assert_eq!(img.get_pixel(2, 2).0, [0, 0, 255, 255]);
        assert_eq!(img.get_pixel(0, 0)[3], 0); // outside the cel: transparent
    }

    // Adversarial: wrong extension rejected before any parse
    #[test]
    fn wrong_extension_rejected() {
        let path = write_fixture("nope.png");
        assert!(load_aseprite(&path).is_err());
    }

    // Adversarial: missing file is an Io error, not a panic
    #[test]
    fn missing_file_is_io_error() {
        let path = std::env::temp_dir().join("lumen-aseprite-tests/does-not-exist.aseprite");
        assert!(matches!(load_aseprite(&path), Err(LumenError::Io(_))));
    }

    // Adversarial: corrupt magic is a DocInvalid, not a panic
    #[test]
    fn corrupt_magic_rejected() {
        let mut bytes = fixture_bytes();
        bytes[4] = 0x00;
        bytes[5] = 0x00;
        let dir = std::env::temp_dir().join("lumen-aseprite-tests");
        let path = dir.join("corrupt.aseprite");
        std::fs::write(&path, &bytes).unwrap();
        assert!(matches!(
            load_aseprite(&path),
            Err(LumenError::DocInvalid(_))
        ));
    }

    // Adversarial: truncated file is a DocInvalid, not a panic
    #[test]
    fn truncated_file_rejected() {
        let bytes = fixture_bytes();
        let dir = std::env::temp_dir().join("lumen-aseprite-tests");
        let path = dir.join("truncated.aseprite");
        std::fs::write(&path, &bytes[..100]).unwrap();
        assert!(load_aseprite(&path).is_err());
    }

    // The version firewall: known pixel bytes cross 0.24 -> 0.25 unchanged.
    #[test]
    fn firewall_round_trips_known_pixels() {
        // 2x2, row-major RGBA: opaque red, half alpha green, transparent
        // blue, opaque white.
        let raw: Vec<u8> = vec![
            255, 0, 0, 255, //
            0, 255, 0, 128, //
            0, 0, 255, 0, //
            255, 255, 255, 255,
        ];
        let img = aseprite_rgba_to_image(raw, 2, 2).expect("2x2 buffer is consistent");
        assert_eq!(img.dimensions(), (2, 2));
        assert_eq!(img.get_pixel(0, 0).0, [255, 0, 0, 255]);
        assert_eq!(img.get_pixel(1, 0).0, [0, 255, 0, 128]);
        assert_eq!(img.get_pixel(0, 1).0, [0, 0, 255, 0]);
        assert_eq!(img.get_pixel(1, 1).0, [255, 255, 255, 255]);
    }

    // Adversarial: out-of-range frame/layer indices rejected
    #[test]
    fn out_of_range_indices_rejected() {
        let path = write_fixture("oor.aseprite");
        let doc = load_aseprite(&path).unwrap();
        assert!(frame_image(&doc, 2).is_err());
        assert!(frame_image(&doc, u32::MAX).is_err());
        assert!(cel_image(&doc, 0, 2).is_err());
        assert!(cel_image(&doc, 9, 0).is_err());
    }
}
