//! Sprite->3D fidelity metric (G-L2, PARITY leg 2c): the first
//! deterministic measurement of how faithfully a factory GLB
//! reproduces its source sprite, at matched framings.
//!
//! Two legs, both computed from the artifacts themselves:
//!
//! - **Silhouette IoU.** The sprite's alpha silhouette and the
//!   GLB's front orthographic projection (the mesh faces +Z;
//!   positions are taken in accessor/model space, which is the
//!   space the factory writer emits) are each normalized to
//!   their own bounding box and rasterized onto the same
//!   `grid` x `grid` lattice. IoU = |intersection| / |union| of
//!   the two filled-cell sets. Matched framing = same lattice,
//!   same bbox normalization, front view on both sides.
//! - **Landmark delta.** Named landmarks measured on the sprite
//!   by [`crate::identity::measure_proportions`] (scalp, chin,
//!   neck, hip rows and the face-centre column, all segmented
//!   from the sprite itself) against the corresponding GLB
//!   landmarks: the named skeleton joints (`HeadTop_End` for
//!   the scalp/top, `Head` for the chin/jaw line — the Mixamo
//!   head joint sits at the skull base, `Neck`, `Hips`) with
//!   world positions accumulated through the node hierarchy,
//!   plus the head joint's X against the mesh width for the
//!   face centre. Every coordinate is normalized to a fraction
//!   of the figure's bounding-box height (Y landmarks, origin
//!   at the bottom) or width (the X landmark), so the delta is
//!   framing-independent. The head-joint-as-chin correspondence
//!   is a proxy and is named as such in the report; no facial
//!   feature detection is claimed.
//!
//! Everything fails closed with [`FactoryError`]: a transparent
//! or unsegmentable sprite, a GLB with no position/index data,
//! no named joints, non-finite geometry, or a degenerate
//! bounding box is an error, never a zero score.

use image::RgbaImage;
use serde::Serialize;
use serde_json::Value;

use crate::gltf::parse_glb_parts;
use crate::identity::measure_proportions;
use crate::{FactoryError, MAX_GLB_BYTES};

/// Default silhouette lattice edge (cells per side).
pub const DEFAULT_GRID: u32 = 256;
/// Largest accepted lattice edge (bounds the raster cost).
pub const MAX_GRID: u32 = 1024;
/// Alpha at or above this value counts as silhouette (the same
/// threshold the identity measurement uses, `ALPHA_OPAQUE`).
const ALPHA_OPAQUE: u8 = 128;

/// One landmark comparison: sprite fraction vs GLB fraction.
#[derive(Debug, Clone, Serialize)]
pub struct LandmarkDelta {
    /// Landmark name (see module docs for the correspondences).
    pub name: String,
    /// Sprite-side fraction (of bbox height, or width for X).
    pub sprite: f64,
    /// GLB-side fraction, same normalization.
    pub glb: f64,
    /// Absolute difference of the two fractions.
    pub delta: f64,
}

/// The fidelity report for one sprite/GLB pair.
#[derive(Debug, Clone, Serialize)]
pub struct FidelityReport {
    /// Where the sprite-side landmarks came from:
    /// `sprite_segmentation` (measured from the supplied sprite
    /// by the identity measurement) or `identity_record` (the
    /// record's stored sprite measurements, taken at ingest
    /// from the source sprite the GLB was built from).
    pub landmark_source: String,
    /// Lattice edge used for the silhouette leg.
    pub grid: u32,
    /// Silhouette intersection-over-union in [0, 1].
    pub silhouette_iou: f64,
    /// Filled lattice cells on the sprite side.
    pub sprite_cells: usize,
    /// Filled lattice cells on the GLB side.
    pub glb_cells: usize,
    /// GLB vertex count (all primitives).
    pub glb_vertices: usize,
    /// GLB triangle count (all primitives).
    pub glb_triangles: usize,
    /// Per-landmark deltas, in report order.
    pub landmarks: Vec<LandmarkDelta>,
    /// Mean of the landmark deltas.
    pub landmark_mean_delta: f64,
    /// Largest landmark delta.
    pub landmark_max_delta: f64,
}

fn hex_rgb(text: &str) -> Result<[u8; 3], FactoryError> {
    crate::params::hex_to_rgb(text)
}

/// Decode one accessor as tightly-interpreted elements from the
/// BIN chunk. Supports the component types the factory writer
/// emits (f32 5126, u16 5123, u32 5125, u8 5121); a strided
/// buffer view is honoured for the position path only via
/// `read_positions`, indices must be tightly packed.
fn accessor_raw(
    json: &Value,
    accessor_idx: usize,
) -> Result<(u64, usize, usize, usize), FactoryError> {
    // Returns (componentType, count, components, byte offset into bin).
    let accessors = json
        .get("accessors")
        .and_then(Value::as_array)
        .ok_or_else(|| FactoryError::InvalidGlb("glb has no accessors".to_string()))?;
    let views = json
        .get("bufferViews")
        .and_then(Value::as_array)
        .ok_or_else(|| FactoryError::InvalidGlb("glb has no bufferViews".to_string()))?;
    let accessor = accessors
        .get(accessor_idx)
        .ok_or_else(|| FactoryError::InvalidGlb("accessor index out of range".to_string()))?;
    let view_idx = accessor
        .get("bufferView")
        .and_then(Value::as_u64)
        .ok_or_else(|| FactoryError::InvalidGlb("accessor missing bufferView".to_string()))?
        as usize;
    let view = views
        .get(view_idx)
        .ok_or_else(|| FactoryError::InvalidGlb("bufferView index out of range".to_string()))?;
    if view.get("byteStride").is_some() {
        return Err(FactoryError::InvalidGlb(
            "interleaved (byteStride) accessors are not supported by the fidelity metric"
                .to_string(),
        ));
    }
    let view_offset = view.get("byteOffset").and_then(Value::as_u64).unwrap_or(0) as usize;
    let accessor_offset = accessor
        .get("byteOffset")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    let component_type = accessor
        .get("componentType")
        .and_then(Value::as_u64)
        .ok_or_else(|| FactoryError::InvalidGlb("accessor missing componentType".to_string()))?;
    let count = accessor.get("count").and_then(Value::as_u64).unwrap_or(0) as usize;
    let components = match accessor.get("type").and_then(Value::as_str) {
        Some("SCALAR") => 1usize,
        Some("VEC2") => 2,
        Some("VEC3") => 3,
        Some("VEC4") => 4,
        other => {
            return Err(FactoryError::InvalidGlb(format!(
                "unsupported accessor type {other:?}"
            )));
        }
    };
    Ok((
        component_type,
        count,
        components,
        view_offset + accessor_offset,
    ))
}

fn read_positions(
    json: &Value,
    bin: &[u8],
    accessor_idx: usize,
) -> Result<Vec<[f32; 3]>, FactoryError> {
    let (component_type, count, components, offset) = accessor_raw(json, accessor_idx)?;
    if component_type != 5126 || components != 3 {
        return Err(FactoryError::InvalidGlb(
            "POSITION accessor is not f32 VEC3".to_string(),
        ));
    }
    if count > crate::MAX_PART_VERTICES {
        return Err(FactoryError::InvalidGlb(format!(
            "position count {count} exceeds bound"
        )));
    }
    let bytes = count
        .checked_mul(12)
        .and_then(|n| offset.checked_add(n))
        .filter(|end| *end <= bin.len())
        .ok_or_else(|| FactoryError::InvalidGlb("POSITION accessor overruns BIN".to_string()))?;
    let slice = &bin[offset..bytes];
    let mut out = Vec::with_capacity(count);
    for chunk in slice.as_chunks::<4>().0.as_chunks::<3>().0 {
        let v = [
            f32::from_le_bytes(chunk[0]),
            f32::from_le_bytes(chunk[1]),
            f32::from_le_bytes(chunk[2]),
        ];
        if !v.iter().all(|c| c.is_finite()) {
            return Err(FactoryError::InvalidGlb(
                "non-finite vertex position".to_string(),
            ));
        }
        out.push(v);
    }
    Ok(out)
}

fn read_indices(json: &Value, bin: &[u8], accessor_idx: usize) -> Result<Vec<u32>, FactoryError> {
    let (component_type, count, components, offset) = accessor_raw(json, accessor_idx)?;
    if components != 1 {
        return Err(FactoryError::InvalidGlb(
            "index accessor is not SCALAR".to_string(),
        ));
    }
    let elem = match component_type {
        5121 => 1usize,
        5123 => 2,
        5125 => 4,
        other => {
            return Err(FactoryError::InvalidGlb(format!(
                "unsupported index componentType {other}"
            )));
        }
    };
    let end = count
        .checked_mul(elem)
        .and_then(|n| offset.checked_add(n))
        .filter(|end| *end <= bin.len())
        .ok_or_else(|| FactoryError::InvalidGlb("index accessor overruns BIN".to_string()))?;
    let slice = &bin[offset..end];
    let mut out = Vec::with_capacity(count);
    match elem {
        1 => out.extend(slice.iter().map(|b| u32::from(*b))),
        2 => {
            for chunk in slice.as_chunks::<2>().0 {
                out.push(u32::from(u16::from_le_bytes(*chunk)));
            }
        }
        _ => {
            for chunk in slice.as_chunks::<4>().0 {
                out.push(u32::from_le_bytes(*chunk));
            }
        }
    }
    Ok(out)
}

/// World translation of every node: local translations summed
/// through the parent chain (factory GLBs carry translation-only
/// joint nodes; a node with rotation/scale is refused rather
/// than approximated).
fn node_world_translations(json: &Value) -> Result<Vec<[f64; 3]>, FactoryError> {
    let nodes = json
        .get("nodes")
        .and_then(Value::as_array)
        .ok_or_else(|| FactoryError::InvalidGlb("glb has no nodes".to_string()))?;
    let mut parent: Vec<Option<usize>> = vec![None; nodes.len()];
    for (i, node) in nodes.iter().enumerate() {
        if node.get("rotation").is_some() || node.get("scale").is_some() {
            return Err(FactoryError::InvalidGlb(format!(
                "node {i} carries rotation/scale; fidelity landmarks require translation-only nodes"
            )));
        }
        if let Some(children) = node.get("children").and_then(Value::as_array) {
            for child in children {
                let c = child.as_u64().ok_or_else(|| {
                    FactoryError::InvalidGlb("node child is not an index".to_string())
                })? as usize;
                if c >= nodes.len() {
                    return Err(FactoryError::InvalidGlb(
                        "node child index out of range".to_string(),
                    ));
                }
                parent[c] = Some(i);
            }
        }
    }
    let mut world = vec![[0.0f64; 3]; nodes.len()];
    for (i, slot) in world.iter_mut().enumerate() {
        let mut sum = [0.0f64; 3];
        let mut cur = Some(i);
        let mut steps = 0usize;
        while let Some(n) = cur {
            steps += 1;
            if steps > nodes.len() {
                return Err(FactoryError::InvalidGlb(
                    "node hierarchy has a cycle".to_string(),
                ));
            }
            if let Some(t) = nodes[n].get("translation").and_then(Value::as_array) {
                for (axis, v) in t.iter().enumerate().take(3) {
                    sum[axis] += v.as_f64().unwrap_or(0.0);
                }
            }
            cur = parent[n];
        }
        *slot = sum;
    }
    Ok(world)
}

fn joint_world(json: &Value, world: &[[f64; 3]], name: &str) -> Result<[f64; 3], FactoryError> {
    let nodes = json
        .get("nodes")
        .and_then(Value::as_array)
        .unwrap_or(&Vec::new())
        .clone();
    for (i, node) in nodes.iter().enumerate() {
        if node.get("name").and_then(Value::as_str) == Some(name) {
            return Ok(world[i]);
        }
    }
    Err(FactoryError::InvalidGlb(format!(
        "glb has no node named {name:?}; fidelity landmarks require the canonical skeleton"
    )))
}

/// Rasterize the sprite's alpha silhouette onto the lattice,
/// normalized to its alpha bounding box.
fn sprite_cells(sprite: &RgbaImage, grid: u32) -> Result<Vec<bool>, FactoryError> {
    let (w, h) = (sprite.width(), sprite.height());
    let mut min_x = w;
    let mut min_y = h;
    let mut max_x = 0u32;
    let mut max_y = 0u32;
    let mut any = false;
    for y in 0..h {
        for x in 0..w {
            if sprite.get_pixel(x, y)[3] >= ALPHA_OPAQUE {
                any = true;
                min_x = min_x.min(x);
                min_y = min_y.min(y);
                max_x = max_x.max(x);
                max_y = max_y.max(y);
            }
        }
    }
    if !any {
        return Err(FactoryError::BadParam(
            "sprite is fully transparent; no silhouette".to_string(),
        ));
    }
    let (bw, bh) = (f64::from(max_x - min_x + 1), f64::from(max_y - min_y + 1));
    let g = grid as usize;
    let mut cells = vec![false; g * g];
    for gy in 0..grid {
        // Lattice row 0 is the TOP of the framing (image space).
        let sy = min_y + (((f64::from(gy) + 0.5) / f64::from(grid)) * bh) as u32;
        for gx in 0..grid {
            let sx = min_x + (((f64::from(gx) + 0.5) / f64::from(grid)) * bw) as u32;
            if sprite.get_pixel(sx.min(w - 1), sy.min(h - 1))[3] >= ALPHA_OPAQUE {
                cells[gy as usize * g + gx as usize] = true;
            }
        }
    }
    Ok(cells)
}

/// Rasterize the GLB front projection (filled triangles, X/Y)
/// onto the lattice, normalized to the mesh bounding box.
fn glb_cells(positions: &[[f32; 3]], triangles: &[[u32; 3]], grid: u32) -> Vec<bool> {
    let (mut min_x, mut min_y) = (f32::INFINITY, f32::INFINITY);
    let (mut max_x, mut max_y) = (f32::NEG_INFINITY, f32::NEG_INFINITY);
    for p in positions {
        min_x = min_x.min(p[0]);
        max_x = max_x.max(p[0]);
        min_y = min_y.min(p[1]);
        max_y = max_y.max(p[1]);
    }
    let (bw, bh) = (f64::from(max_x - min_x), f64::from(max_y - min_y));
    let g = grid as usize;
    let mut cells = vec![false; g * g];
    // Project to lattice space; lattice row 0 is the TOP (max Y).
    let proj = |p: [f32; 3]| -> (f64, f64) {
        let nx = (f64::from(p[0] - min_x) / bw) * f64::from(grid);
        let ny = (1.0 - (f64::from(p[1] - min_y) / bh)) * f64::from(grid);
        (nx, ny)
    };
    for tri in triangles {
        let a = proj(positions[tri[0] as usize]);
        let b = proj(positions[tri[1] as usize]);
        let c = proj(positions[tri[2] as usize]);
        let lo_x = a.0.min(b.0).min(c.0).floor().max(0.0) as i64;
        let hi_x = a.0.max(b.0).max(c.0).ceil().min(f64::from(grid)) as i64;
        let lo_y = a.1.min(b.1).min(c.1).floor().max(0.0) as i64;
        let hi_y = a.1.max(b.1).max(c.1).ceil().min(f64::from(grid)) as i64;
        let denom = (b.1 - c.1) * (a.0 - c.0) + (c.0 - b.0) * (a.1 - c.1);
        if denom.abs() < f64::EPSILON {
            continue; // degenerate in projection: no area to fill
        }
        for gy in lo_y..hi_y {
            for gx in lo_x..hi_x {
                let (px, py) = (gx as f64 + 0.5, gy as f64 + 0.5);
                let w0 = ((b.1 - c.1) * (px - c.0) + (c.0 - b.0) * (py - c.1)) / denom;
                let w1 = ((c.1 - a.1) * (px - c.0) + (a.0 - c.0) * (py - c.1)) / denom;
                let w2 = 1.0 - w0 - w1;
                if w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0 {
                    cells[gy as usize * g + gx as usize] = true;
                }
            }
        }
    }
    cells
}

/// Sprite-side landmark fractions from raw sprite measurements.
/// Y fractions are height-from-bottom over the figure height
/// (scalp row to sole row); the X fraction is over the alpha
/// bounding-box width (`bbox_left` / `bbox_width` locate the
/// face column inside it; for record-sourced measurements the
/// record's image width stands in for the bbox, disclosed in
/// the first-measurement record).
fn sprite_landmark_fractions(
    meas: &crate::identity::SpriteMeasurements,
    figure_top_row: f64,
    figure_bottom_row: f64,
    bbox_left: f64,
    bbox_width: f64,
) -> Vec<(&'static str, f64)> {
    let scalp_abs = figure_top_row + f64::from(meas.scalp_offset_px);
    let fig_h = figure_bottom_row - scalp_abs;
    let from_bottom = |abs_row: f64| (figure_bottom_row - abs_row) / fig_h;
    vec![
        ("scalp_top", from_bottom(scalp_abs)),
        (
            "chin",
            from_bottom(scalp_abs + f64::from(meas.skull_height_px)),
        ),
        ("neck", from_bottom(scalp_abs + f64::from(meas.neck_row))),
        ("hip", from_bottom(scalp_abs + f64::from(meas.hip_row))),
        (
            "face_centre_x",
            (f64::from(meas.face_center_x) - bbox_left) / bbox_width,
        ),
    ]
}

/// Measure one sprite/GLB pair. `skin_hex` / `hair_hex` are the
/// sprite-segmentation anchors the identity measurement needs
/// (the identity record's palette carries them).
pub fn measure_fidelity(
    sprite: &RgbaImage,
    glb_bytes: &[u8],
    skin_hex: &str,
    hair_hex: &str,
    grid: u32,
) -> Result<FidelityReport, FactoryError> {
    let skin = hex_rgb(skin_hex)?;
    let hair = hex_rgb(hair_hex)?;
    let (_props, sprite_meas) = measure_proportions(sprite, skin, hair)?;
    measure_fidelity_measured(sprite, glb_bytes, &sprite_meas, "sprite_segmentation", grid)
}

/// Measure one sprite/GLB pair with the sprite-side landmarks
/// supplied as stored [`crate::identity::SpriteMeasurements`]
/// (e.g. from an identity record) instead of re-segmenting the
/// sprite. The sprite image still supplies the silhouette leg.
/// The measurements' rows are relative to the source sprite's
/// scalp row and its figure runs scalp->sole over
/// `total_height_px`, so the figure rows are reconstructed in
/// the record's own frame and normalized there; only the
/// silhouette comes from the supplied image.
pub fn measure_fidelity_measured(
    sprite: &RgbaImage,
    glb_bytes: &[u8],
    sprite_meas: &crate::identity::SpriteMeasurements,
    landmark_source: &str,
    grid: u32,
) -> Result<FidelityReport, FactoryError> {
    if grid == 0 || grid > MAX_GRID {
        return Err(FactoryError::BadParam(format!(
            "grid {grid} outside 1..={MAX_GRID}"
        )));
    }
    if glb_bytes.len() > MAX_GLB_BYTES {
        return Err(FactoryError::InvalidGlb(
            "glb exceeds size bound".to_string(),
        ));
    }
    let s_cells = sprite_cells(sprite, grid)?;

    // GLB side: geometry + joints.
    let parts = parse_glb_parts(glb_bytes)?;
    let json = &parts.json;
    let meshes = json
        .get("meshes")
        .and_then(Value::as_array)
        .ok_or_else(|| FactoryError::InvalidGlb("glb has no meshes".to_string()))?;
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut triangles: Vec<[u32; 3]> = Vec::new();
    for mesh in meshes {
        let prims = mesh
            .get("primitives")
            .and_then(Value::as_array)
            .ok_or_else(|| FactoryError::InvalidGlb("mesh has no primitives".to_string()))?;
        for prim in prims {
            let pos_idx = prim
                .get("attributes")
                .and_then(|a| a.get("POSITION"))
                .and_then(Value::as_u64)
                .ok_or_else(|| FactoryError::InvalidGlb("primitive lacks POSITION".to_string()))?
                as usize;
            let base = positions.len() as u32;
            let prim_positions = read_positions(json, &parts.bin, pos_idx)?;
            positions.extend_from_slice(&prim_positions);
            if let Some(idx) = prim.get("indices").and_then(Value::as_u64) {
                let indices = read_indices(json, &parts.bin, idx as usize)?;
                for tri in indices.as_chunks::<3>().0 {
                    triangles.push([base + tri[0], base + tri[1], base + tri[2]]);
                }
            } else {
                let sequential: Vec<u32> = (0..prim_positions.len() as u32).collect();
                for tri in sequential.as_chunks::<3>().0 {
                    triangles.push([base + tri[0], base + tri[1], base + tri[2]]);
                }
            }
        }
    }
    if positions.is_empty() || triangles.is_empty() {
        return Err(FactoryError::InvalidGlb(
            "glb carries no triangles to project".to_string(),
        ));
    }
    let g_cells = glb_cells(&positions, &triangles, grid);

    // Silhouette IoU.
    let mut inter = 0usize;
    let mut union = 0usize;
    let mut s_count = 0usize;
    let mut g_count = 0usize;
    for (s, g) in s_cells.iter().zip(g_cells.iter()) {
        if *s {
            s_count += 1;
        }
        if *g {
            g_count += 1;
        }
        if *s && *g {
            inter += 1;
        }
        if *s || *g {
            union += 1;
        }
    }
    if union == 0 {
        return Err(FactoryError::InvalidGlb(
            "both silhouettes are empty; IoU undefined".to_string(),
        ));
    }
    let iou = inter as f64 / union as f64;

    // Landmarks. Sprite rows are relative to the scalp row;
    // absolute image row = alpha_top + scalp_offset + rel_row,
    // where alpha_top is the silhouette bbox top. Fractions are
    // height-from-bottom over the bbox height.
    let (mut top, mut bottom) = (sprite.height(), 0u32);
    let (mut left, mut right) = (sprite.width(), 0u32);
    for y in 0..sprite.height() {
        for x in 0..sprite.width() {
            if sprite.get_pixel(x, y)[3] >= ALPHA_OPAQUE {
                top = top.min(y);
                bottom = bottom.max(y);
                left = left.min(x);
                right = right.max(x);
            }
        }
    }
    let sprite_landmarks: Vec<(&str, f64)> = if landmark_source == "identity_record" {
        // Record frame: alpha top = row 0, figure runs
        // scalp_offset .. scalp_offset + total_height_px - 1,
        // face column over the record's image width.
        sprite_landmark_fractions(
            sprite_meas,
            0.0,
            f64::from(sprite_meas.scalp_offset_px + sprite_meas.total_height_px - 1),
            0.0,
            f64::from(sprite_meas.width_px),
        )
    } else {
        sprite_landmark_fractions(
            sprite_meas,
            f64::from(top),
            f64::from(bottom),
            f64::from(left),
            f64::from(right - left + 1),
        )
    };

    let world = node_world_translations(json)?;
    let (mut mesh_min_x, mut mesh_min_y) = (f64::INFINITY, f64::INFINITY);
    let (mut mesh_max_x, mut mesh_max_y) = (f64::NEG_INFINITY, f64::NEG_INFINITY);
    for p in &positions {
        mesh_min_x = mesh_min_x.min(f64::from(p[0]));
        mesh_max_x = mesh_max_x.max(f64::from(p[0]));
        mesh_min_y = mesh_min_y.min(f64::from(p[1]));
        mesh_max_y = mesh_max_y.max(f64::from(p[1]));
    }
    let mesh_h = mesh_max_y - mesh_min_y;
    let mesh_w = mesh_max_x - mesh_min_x;
    if !(mesh_h > 0.0 && mesh_w > 0.0) {
        return Err(FactoryError::InvalidGlb(
            "glb bounding box is degenerate".to_string(),
        ));
    }
    let head_top = joint_world(json, &world, "HeadTop_End")?;
    let head = joint_world(json, &world, "Head")?;
    let neck = joint_world(json, &world, "Neck")?;
    let hips = joint_world(json, &world, "Hips")?;
    let fy = |y: f64| (y - mesh_min_y) / mesh_h;
    let glb_landmarks: Vec<(&str, f64)> = vec![
        ("scalp_top", fy(head_top[1])),
        ("chin", fy(head[1])),
        ("neck", fy(neck[1])),
        ("hip", fy(hips[1])),
        ("face_centre_x", (head[0] - mesh_min_x) / mesh_w),
    ];

    let mut landmarks = Vec::with_capacity(sprite_landmarks.len());
    let mut sum = 0.0f64;
    let mut max = 0.0f64;
    for ((name, s), (_, g)) in sprite_landmarks.iter().zip(glb_landmarks.iter()) {
        let delta = (s - g).abs();
        sum += delta;
        max = max.max(delta);
        landmarks.push(LandmarkDelta {
            name: (*name).to_string(),
            sprite: *s,
            glb: *g,
            delta,
        });
    }
    let mean = sum / landmarks.len() as f64;

    Ok(FidelityReport {
        landmark_source: landmark_source.to_string(),
        grid,
        silhouette_iou: iou,
        sprite_cells: s_count,
        glb_cells: g_count,
        glb_vertices: positions.len(),
        glb_triangles: triangles.len(),
        landmarks,
        landmark_mean_delta: mean,
        landmark_max_delta: max,
    })
}
