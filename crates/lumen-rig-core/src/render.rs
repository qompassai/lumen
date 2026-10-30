//! RENDER-1: the CPU software rasteriser. A lumen model,
//! posed by the pose evaluator, becomes a PNG-ready RGBA
//! frame — no GPU, no wgpu, nothing but the deformed mesh,
//! its textures, and the model's own draw order.
//!
//! The pipeline (SPEC.md section 3.5 made executable):
//!
//! - Meshes draw in `(draw_order, id)` order; `rendered:
//!   false` meshes never draw (they may still serve as
//!   clipping masks).
//! - Each triangle rasterises by barycentric coverage at
//!   pixel centres; UVs interpolate barycentrically and
//!   sample the mesh's texture bilinearly, clamped to the
//!   texture edge. UV space is the format's own: `u` across,
//!   `v` down the texture's rows (the same convention the
//!   estate's wgpu renderer passes to its sampler).
//! - Source alpha is `texel alpha * effective opacity`
//!   (section 3.2's product), further multiplied by the
//!   mesh's clipping-mask alpha where `clipping_mask_ids`
//!   is non-empty. A mesh's mask alpha at a pixel is the
//!   source-over accumulation of its mask meshes' own
//!   source alphas there (mask meshes evaluate without
//!   their own clipping — the relation is acyclic by
//!   validation, and one level is the documented R3 rule).
//! - Blending is the W3C Compositing and Blending Level 1
//!   formula set, all 18 modes of section 3.5, in f32
//!   sRGB-encoded space (the space the model's textures are
//!   authored in; R3 pins the working space to the texture
//!   space). No mode is aliased to normal: section 3.5
//!   forbids substitution, so every mode is implemented.
//!
//! Determinism: the frame is a pure function of the
//! evaluated meshes, the texture pixels, and the frame
//! mapping. Fixed iteration orders everywhere (draw sort,
//! triangle order, pixel scan); quantisation to RGBA8
//! happens once, at the end, by round-and-clamp.

use std::collections::HashMap;
use std::fmt;

use crate::model::BlendMode;
use crate::pose::EvaluatedMesh;

/// An RGBA8 image: row-major, top row first, four bytes per
/// pixel. Textures and finished frames share this shape.
#[derive(Debug, Clone, PartialEq)]
pub struct RgbaImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<[u8; 4]>,
}

impl RgbaImage {
    /// A fully transparent image of the given size.
    pub fn transparent(width: u32, height: u32) -> RgbaImage {
        RgbaImage {
            width,
            height,
            pixels: vec![[0, 0, 0, 0]; (width as usize) * (height as usize)],
        }
    }

    /// The texel at (x, y), in row-major top-first order.
    pub fn texel(&self, x: u32, y: u32) -> [u8; 4] {
        self.pixels[(y as usize) * (self.width as usize) + (x as usize)]
    }
}

/// Why a frame could not be rendered. Rendering fails
/// loudly (section 9.2's law, applied to the renderer): a
/// missing texture is never replaced by a stand-in.
#[derive(Debug, Clone, PartialEq)]
pub enum RenderError {
    /// A drawn (or mask) mesh names a texture the caller did
    /// not supply pixels for.
    MissingTexture { mesh: String, texture: String },
    /// No rendered mesh has any vertex, so no frame mapping
    /// exists.
    EmptyScene,
    /// The requested canvas has a zero dimension or exceeds
    /// the pixel bound.
    BadCanvas { width: u32, height: u32 },
    /// Texture bytes were supplied but are not a decodable
    /// PNG.
    UnreadableTexture { detail: String },
}

impl fmt::Display for RenderError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RenderError::MissingTexture { mesh, texture } => {
                write!(
                    formatter,
                    "mesh {mesh:?} names texture {texture:?}, for which no pixels were supplied"
                )
            }
            RenderError::EmptyScene => write!(formatter, "no rendered mesh has vertices"),
            RenderError::BadCanvas { width, height } => {
                write!(
                    formatter,
                    "canvas {width}x{height} is empty or beyond the pixel bound"
                )
            }
            RenderError::UnreadableTexture { detail } => {
                write!(formatter, "texture bytes are not a decodable PNG: {detail}")
            }
        }
    }
}

impl std::error::Error for RenderError {}

/// The largest canvas, in pixels, one frame may allocate
/// (bounds the framebuffer at 64 Mi pixels = 1 GiB of f32
/// working space worst case; batch frames are far smaller).
pub const FRAME_PIXELS_MAX: usize = 67_108_864;

/// The affine map from model space to pixel space for one
/// frame: `pixel = (model * scale + offset)`, with the y
/// axis flipped (model +y is up, pixel +y is down). Exposed
/// so a caller can localise model-space regions (a brow
/// bounding box, say) in the finished frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameMapping {
    pub width: u32,
    pub height: u32,
    pub scale: f64,
    pub offset_x: f64,
    pub offset_y: f64,
}

impl FrameMapping {
    /// Fit every rendered mesh's vertices into a
    /// `width` x `height` canvas, preserving aspect, with
    /// `margin_fraction` of the fitted extent left as margin
    /// on the limiting axis. `None` when nothing renders.
    pub fn fit(
        meshes: &[EvaluatedMesh],
        width: u32,
        height: u32,
        margin_fraction: f64,
    ) -> Option<FrameMapping> {
        let (mut min_x, mut min_y) = (f64::INFINITY, f64::INFINITY);
        let (mut max_x, mut max_y) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
        let mut any = false;
        for mesh in meshes.iter().filter(|mesh| mesh.rendered) {
            for vertex in &mesh.vertices {
                any = true;
                min_x = min_x.min(f64::from(vertex[0]));
                max_x = max_x.max(f64::from(vertex[0]));
                min_y = min_y.min(f64::from(vertex[1]));
                max_y = max_y.max(f64::from(vertex[1]));
            }
        }
        if !any {
            return None;
        }
        let extent_x = (max_x - min_x).max(1e-9);
        let extent_y = (max_y - min_y).max(1e-9);
        let usable = 1.0 - 2.0 * margin_fraction.clamp(0.0, 0.45);
        let scale =
            ((f64::from(width) * usable) / extent_x).min((f64::from(height) * usable) / extent_y);
        let centre_x = (min_x + max_x) / 2.0;
        let centre_y = (min_y + max_y) / 2.0;
        Some(FrameMapping {
            width,
            height,
            scale,
            offset_x: f64::from(width) / 2.0 - centre_x * scale,
            offset_y: f64::from(height) / 2.0 + centre_y * scale,
        })
    }

    /// Map one model-space point to pixel coordinates
    /// (fractional; pixel centres sit at half-integers).
    pub fn model_to_pixel(&self, point: [f32; 2]) -> [f64; 2] {
        [
            f64::from(point[0]) * self.scale + self.offset_x,
            -f64::from(point[1]) * self.scale + self.offset_y,
        ]
    }
}

/// Render one frame: `meshes` in any order (the draw sort
/// is applied here), textures by texture id, through an
/// explicit frame mapping.
pub fn render_frame(
    meshes: &[EvaluatedMesh],
    textures: &HashMap<String, RgbaImage>,
    mapping: &FrameMapping,
) -> Result<RgbaImage, RenderError> {
    let (width, height) = (mapping.width as usize, mapping.height as usize);
    if width == 0 || height == 0 || width.saturating_mul(height) > FRAME_PIXELS_MAX {
        return Err(RenderError::BadCanvas {
            width: mapping.width,
            height: mapping.height,
        });
    }
    let mut frame = vec![[0.0_f32; 4]; width * height];
    let mut draw: Vec<&EvaluatedMesh> = meshes.iter().filter(|mesh| mesh.rendered).collect();
    draw.sort_by(|a, b| (a.draw_order, a.id.as_str()).cmp(&(b.draw_order, b.id.as_str())));
    for mesh in draw {
        let texture = textures
            .get(&mesh.texture)
            .ok_or_else(|| RenderError::MissingTexture {
                mesh: mesh.id.clone(),
                texture: mesh.texture.clone(),
            })?;
        let mask = if mesh.clipping_mask_ids.is_empty() {
            None
        } else {
            Some(mask_alpha(mesh, meshes, textures, mapping)?)
        };
        rasterise_mesh(mesh, texture, mapping, mask.as_deref(), &mut frame);
    }
    let pixels: Vec<[u8; 4]> = frame
        .iter()
        .map(|pixel| {
            [
                quantise(pixel[0]),
                quantise(pixel[1]),
                quantise(pixel[2]),
                quantise(pixel[3]),
            ]
        })
        .collect();
    Ok(RgbaImage {
        width: mapping.width,
        height: mapping.height,
        pixels,
    })
}

/// The combined clipping-mask alpha for one mesh: the
/// source-over accumulation, per pixel, of every mask
/// mesh's source alpha (texture alpha at the interpolated
/// UV times the mask mesh's effective opacity).
fn mask_alpha(
    mesh: &EvaluatedMesh,
    meshes: &[EvaluatedMesh],
    textures: &HashMap<String, RgbaImage>,
    mapping: &FrameMapping,
) -> Result<Vec<f32>, RenderError> {
    let (width, height) = (mapping.width as usize, mapping.height as usize);
    let mut mask = vec![0.0_f32; width * height];
    for mask_id in &mesh.clipping_mask_ids {
        let mask_mesh = meshes
            .iter()
            .find(|candidate| &candidate.id == mask_id)
            .expect("a validated model resolves every mask id");
        let texture =
            textures
                .get(&mask_mesh.texture)
                .ok_or_else(|| RenderError::MissingTexture {
                    mesh: mask_mesh.id.clone(),
                    texture: mask_mesh.texture.clone(),
                })?;
        rasterise_alpha(mask_mesh, texture, mapping, &mut mask);
    }
    Ok(mask)
}

/// Rasterise one mesh's triangles into an alpha-only
/// buffer, source-over (used for clipping masks).
fn rasterise_alpha(
    mesh: &EvaluatedMesh,
    texture: &RgbaImage,
    mapping: &FrameMapping,
    out: &mut [f32],
) {
    for_each_covered_pixel(mesh, mapping, |index, uv| {
        let texel = sample_texture(texture, uv);
        let source_alpha = texel[3] * mesh.opacity;
        out[index] = source_alpha + out[index] * (1.0 - source_alpha);
    });
}

/// Rasterise one mesh into the colour frame: sample,
/// blend (W3C), composite source-over with blending.
fn rasterise_mesh(
    mesh: &EvaluatedMesh,
    texture: &RgbaImage,
    mapping: &FrameMapping,
    mask: Option<&[f32]>,
    frame: &mut [[f32; 4]],
) {
    for_each_covered_pixel(mesh, mapping, |index, uv| {
        let texel = sample_texture(texture, uv);
        let mut source_alpha = texel[3] * mesh.opacity;
        if let Some(mask) = mask {
            source_alpha *= mask[index];
        }
        if source_alpha <= 0.0 {
            return;
        }
        let source = [texel[0], texel[1], texel[2]];
        let backdrop = frame[index];
        frame[index] = composite(source, source_alpha, backdrop, mesh.blend_mode);
    });
}

/// Visit every pixel a mesh's triangles cover, with the
/// pixel's interpolated UV. Coverage is barycentric at
/// pixel centres; triangle iteration order is the mesh's
/// own index order, pixel scan is row-major — both fixed,
/// so overlapping coverage composes deterministically.
fn for_each_covered_pixel(
    mesh: &EvaluatedMesh,
    mapping: &FrameMapping,
    mut visit: impl FnMut(usize, [f32; 2]),
) {
    let (width, height) = (mapping.width as i64, mapping.height as i64);
    let points: Vec<[f64; 2]> = mesh
        .vertices
        .iter()
        .map(|vertex| mapping.model_to_pixel(*vertex))
        .collect();
    for triangle in mesh.indices.as_chunks::<3>().0 {
        let (ia, ib, ic) = (
            triangle[0] as usize,
            triangle[1] as usize,
            triangle[2] as usize,
        );
        let (a, b, c) = (points[ia], points[ib], points[ic]);
        let denominator = (b[1] - c[1]) * (a[0] - c[0]) + (c[0] - b[0]) * (a[1] - c[1]);
        if denominator.abs() <= 1e-12 {
            continue;
        }
        let min_x = a[0].min(b[0]).min(c[0]).floor() as i64;
        let max_x = a[0].max(b[0]).max(c[0]).ceil() as i64;
        let min_y = a[1].min(b[1]).min(c[1]).floor() as i64;
        let max_y = a[1].max(b[1]).max(c[1]).ceil() as i64;
        for py in min_y.max(0)..=max_y.min(height - 1) {
            for px in min_x.max(0)..=max_x.min(width - 1) {
                let point = [px as f64 + 0.5, py as f64 + 0.5];
                let wa = ((b[1] - c[1]) * (point[0] - c[0]) + (c[0] - b[0]) * (point[1] - c[1]))
                    / denominator;
                let wb = ((c[1] - a[1]) * (point[0] - c[0]) + (a[0] - c[0]) * (point[1] - c[1]))
                    / denominator;
                let wc = 1.0 - wa - wb;
                if wa < -1e-9 || wb < -1e-9 || wc < -1e-9 {
                    continue;
                }
                let uv = [
                    (wa * f64::from(mesh.uvs[ia][0])
                        + wb * f64::from(mesh.uvs[ib][0])
                        + wc * f64::from(mesh.uvs[ic][0])) as f32,
                    (wa * f64::from(mesh.uvs[ia][1])
                        + wb * f64::from(mesh.uvs[ib][1])
                        + wc * f64::from(mesh.uvs[ic][1])) as f32,
                ];
                visit((py as usize) * (width as usize) + (px as usize), uv);
            }
        }
    }
}

/// Bilinear texture sampling, clamped to the edge texels,
/// in f32 `[0, 1]` space. `v` indexes rows top-first (the
/// format's UV convention).
fn sample_texture(texture: &RgbaImage, uv: [f32; 2]) -> [f32; 4] {
    let (width, height) = (texture.width as f32, texture.height as f32);
    let x = (uv[0].clamp(0.0, 1.0) * (width - 1.0)).max(0.0);
    let y = (uv[1].clamp(0.0, 1.0) * (height - 1.0)).max(0.0);
    let (x0, y0) = (x.floor() as u32, y.floor() as u32);
    let (x1, y1) = (
        (x0 + 1).min(texture.width - 1),
        (y0 + 1).min(texture.height - 1),
    );
    let (fx, fy) = (x - x0 as f32, y - y0 as f32);
    let mut out = [0.0_f32; 4];
    for (dx, dy, weight) in [
        (x0, y0, (1.0 - fx) * (1.0 - fy)),
        (x1, y0, fx * (1.0 - fy)),
        (x0, y1, (1.0 - fx) * fy),
        (x1, y1, fx * fy),
    ] {
        let texel = texture.texel(dx, dy);
        for channel in 0..4 {
            out[channel] += weight * (f32::from(texel[channel]) / 255.0);
        }
    }
    out
}

/// The W3C composite: blend the source colour against the
/// backdrop colour, then source-over composite by alpha.
/// Colours are non-premultiplied f32; the result is too.
fn composite(source: [f32; 3], source_alpha: f32, backdrop: [f32; 4], mode: BlendMode) -> [f32; 4] {
    let backdrop_alpha = backdrop[3];
    let blended = blend(mode, [backdrop[0], backdrop[1], backdrop[2]], source);
    let alpha_out = source_alpha + backdrop_alpha * (1.0 - source_alpha);
    let mut out = [0.0_f32; 4];
    for channel in 0..3 {
        let colour = source_alpha * (1.0 - backdrop_alpha) * source[channel]
            + source_alpha * backdrop_alpha * blended[channel]
            + (1.0 - source_alpha) * backdrop_alpha * backdrop[channel];
        out[channel] = if alpha_out > 0.0 {
            colour / alpha_out
        } else {
            0.0
        };
    }
    out[3] = alpha_out;
    out
}

/// The blend function B(cb, cs) of W3C Compositing and
/// Blending Level 1, per mode. Separable modes apply per
/// channel; the four non-separable modes work on the
/// colour as a whole via the standard Lum/Sat helpers.
fn blend(mode: BlendMode, backdrop: [f32; 3], source: [f32; 3]) -> [f32; 3] {
    match mode {
        BlendMode::Normal => source,
        BlendMode::Hue => set_lum(set_sat(source, sat(backdrop)), lum(backdrop)),
        BlendMode::Saturation => set_lum(set_sat(backdrop, sat(source)), lum(backdrop)),
        BlendMode::Color => set_lum(source, lum(backdrop)),
        BlendMode::Luminosity => set_lum(backdrop, lum(source)),
        separable => {
            let mut out = [0.0_f32; 3];
            for channel in 0..3 {
                out[channel] = blend_channel(separable, backdrop[channel], source[channel]);
            }
            out
        }
    }
}

/// One channel of a separable blend mode.
fn blend_channel(mode: BlendMode, cb: f32, cs: f32) -> f32 {
    match mode {
        BlendMode::Multiply => cb * cs,
        BlendMode::Screen => cb + cs - cb * cs,
        BlendMode::Overlay => {
            if cb <= 0.5 {
                2.0 * cb * cs
            } else {
                1.0 - 2.0 * (1.0 - cb) * (1.0 - cs)
            }
        }
        BlendMode::Darken => cb.min(cs),
        BlendMode::Lighten => cb.max(cs),
        BlendMode::ColorDodge => {
            if cs >= 1.0 {
                1.0
            } else {
                (cb / (1.0 - cs)).min(1.0)
            }
        }
        BlendMode::ColorBurn => {
            if cs <= 0.0 {
                0.0
            } else {
                (1.0 - (1.0 - cb) / cs).max(0.0)
            }
        }
        BlendMode::HardLight => {
            if cs <= 0.5 {
                2.0 * cb * cs
            } else {
                1.0 - 2.0 * (1.0 - cb) * (1.0 - cs)
            }
        }
        BlendMode::SoftLight => {
            if cs <= 0.5 {
                cb - (1.0 - 2.0 * cs) * cb * cb * (1.0 - cb)
            } else {
                let d = if cb <= 0.25 {
                    ((16.0 * cb - 12.0) * cb + 4.0) * cb
                } else {
                    cb.sqrt()
                };
                cb + (2.0 * cs - 1.0) * (d - cb)
            }
        }
        BlendMode::Difference => (cb - cs).abs(),
        BlendMode::Exclusion => cb + cs - 2.0 * cb * cs,
        BlendMode::LinearDodge => (cb + cs).min(1.0),
        BlendMode::LinearBurn => (cb + cs - 1.0).max(0.0),
        // Non-separable modes never reach the per-channel
        // path (handled whole-colour in `blend`).
        _ => cs,
    }
}

/// W3C luminosity: `0.3 R + 0.59 G + 0.11 B`.
fn lum(colour: [f32; 3]) -> f32 {
    0.3 * colour[0] + 0.59 * colour[1] + 0.11 * colour[2]
}

/// W3C saturation: `max - min` over the channels.
fn sat(colour: [f32; 3]) -> f32 {
    colour.iter().copied().fold(f32::MIN, f32::max)
        - colour.iter().copied().fold(f32::MAX, f32::min)
}

/// W3C SetSat: scale the colour's channels so its
/// saturation becomes `s`, keeping channel order.
fn set_sat(colour: [f32; 3], s: f32) -> [f32; 3] {
    let mut order: [usize; 3] = [0, 1, 2];
    order.sort_by(|&a, &b| {
        colour[a]
            .partial_cmp(&colour[b])
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    let (min, mid, max) = (order[0], order[1], order[2]);
    let mut out = [0.0_f32; 3];
    if colour[max] > colour[min] {
        out[mid] = (colour[mid] - colour[min]) * s / (colour[max] - colour[min]);
        out[max] = s;
    }
    out[min] = 0.0;
    out
}

/// W3C SetLum: shift the colour so its luminosity becomes
/// `l`, then clip back into gamut.
fn set_lum(colour: [f32; 3], l: f32) -> [f32; 3] {
    let delta = l - lum(colour);
    clip_colour([colour[0] + delta, colour[1] + delta, colour[2] + delta])
}

/// W3C ClipColor: pull out-of-gamut channels back along
/// the luminosity-preserving line.
fn clip_colour(mut colour: [f32; 3]) -> [f32; 3] {
    let l = lum(colour);
    let min = colour.iter().copied().fold(f32::MAX, f32::min);
    let max = colour.iter().copied().fold(f32::MIN, f32::max);
    if min < 0.0 {
        for channel in colour.iter_mut() {
            *channel = l + (*channel - l) * l / (l - min);
        }
    }
    if max > 1.0 {
        for channel in colour.iter_mut() {
            *channel = l + (*channel - l) * (1.0 - l) / (max - l);
        }
    }
    colour
}

/// Round-and-clamp one f32 channel to RGBA8.
fn quantise(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

/// Encode a frame as PNG bytes with fixed settings
/// (RGBA8, no ancillary chunks, default compression): the
/// bytes are a pure function of the pixels, so two renders
/// of the same pose produce byte-identical files — the
/// raster-determinism gate hashes these bytes.
pub fn encode_png(image: &RgbaImage) -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, image.width, image.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .expect("writing a PNG header to memory cannot fail");
        let raw: Vec<u8> = image
            .pixels
            .iter()
            .flat_map(|pixel| pixel.iter().copied())
            .collect();
        writer
            .write_image_data(&raw)
            .expect("writing PNG data to memory cannot fail");
    }
    bytes
}

/// Decode PNG bytes into an [`RgbaImage`] (any PNG colour
/// type, converted to RGBA8). Textures enter the renderer
/// through here in the batch CLI; decode failure is a loud
/// error, never a substituted texture.
pub fn decode_png(bytes: &[u8]) -> Result<RgbaImage, RenderError> {
    let image = image::load_from_memory_with_format(bytes, image::ImageFormat::Png)
        .map_err(|error| RenderError::UnreadableTexture {
            detail: error.to_string(),
        })?
        .to_rgba8();
    let (width, height) = (image.width(), image.height());
    let pixels = image
        .pixels()
        .map(|pixel| [pixel[0], pixel[1], pixel[2], pixel[3]])
        .collect();
    Ok(RgbaImage {
        width,
        height,
        pixels,
    })
}
