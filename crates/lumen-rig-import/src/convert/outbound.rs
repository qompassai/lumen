//! Native model -> moc3 mapping: the outbound direction of
//! the converter (exporter phase E1, lumen R5c).
//!
//! The mapping is MAPPING.md sections 3-8, implemented row by
//! row. The discipline mirrors the inbound converter: every
//! native construct either maps exactly, maps under a named
//! loss recorded in the [`ExportReport`], or fails the export
//! with a typed [`ConvertError::Export`]. Nothing is silently
//! dropped, clamped, or substituted. The spec decisions taken
//! in MAPPING.md are binding here:
//! - D-1: a Combined parameter fails the export (no target
//!   construct computes one parameter from others).
//! - D-2: draw order is one flat group over all meshes, depths
//!   rescaled `(draw_order + 10000) / 20` into [0, 1000].
//! - D-3: a rotation form whose scale is non-uniform
//!   (sx != sy) fails the export (moc3 carries a single scale).
//! - D-4: a blend mode outside {Normal, LinearDodge, Multiply}
//!   fails the export (the V3_0 render-config byte encodes
//!   exactly those three).
//! - D-5/D-6 concern the physics3 sidecar (phase E3); the
//!   binary carries no physics, and each physics group is
//!   named in the report as not carried (see below).
//!
//! Canvas is an exporter input ([`CanvasSpec`], MAPPING.md
//! question Q1 resolved as option (a)): the native model has
//! no canvas fields, so the writer invents none.
//!
//! Form order (MAPPING.md section 3.3): native lattices store
//! forms in canonical ascending coordinate order (the last
//! axis varies fastest); moc3 stores keyforms with the first
//! axis varying fastest. [`moc3_form_order`] performs exactly
//! that permutation for 2-axis lattices; 1-axis and unbound
//! objects are unaffected.

use std::collections::HashMap;

use lumen_rig_core::{
    BlendMode, Deformer, Lattice, Mesh, MeshForm, Model, Parameter, RotationForm, WarpForm,
};
use serde::Serialize;

use crate::convert::ReportEntry;
use crate::error::ConvertError;
use crate::moc3_write::{
    BindingSetData, CanvasSpec, DeformerData, DrawGroupData, DrawItemData, MeshData, Moc3Data,
    Moc3Version, ParamData, ParamMapData, PartData, RotationData, RotationFormData, WarpData,
    write_moc3,
};

/// Native draw order domain bound (rig-core SPEC section 3.5).
const DRAW_ORDER_BOUND: i32 = 10_000;
/// moc3 stores vertex indices as u16 bit patterns, so a mesh
/// may address at most 65536 vertices (MAPPING.md section 6).
const MESH_VERTICES_MAX: usize = 65_536;
/// Parameter snap type "Normal" (MAPPING.md section 3.1: the
/// one value consistent with native semantics).
const SNAP_NORMAL: u32 = 3;

/// What a successful export produced.
#[derive(Debug)]
pub struct ExportOutcome {
    /// The complete `.moc3` byte stream.
    pub bytes: Vec<u8>,
    /// The export report accounting for every lossy / no-map
    /// construct (MAPPING.md section 15).
    pub report: ExportReport,
}

/// The export report: the outbound mirror of
/// [`crate::ConversionReport`].
#[derive(Debug, Clone, Serialize)]
pub struct ExportReport {
    pub format: &'static str,
    pub format_version: u32,
    pub model_id: String,
    pub moc3_version: String,
    pub counts: ExportCounts,
    /// Constructs written with information or behavior given
    /// up; the mechanism is named per entry.
    pub lossy: Vec<ReportEntry>,
    /// Constructs with no target at V3_0, not written.
    pub no_map: Vec<ReportEntry>,
    pub notes: Vec<String>,
}

/// The count info table the writer stamped, echoed.
#[derive(Debug, Clone, Serialize)]
pub struct ExportCounts {
    pub parts: usize,
    pub deformers: usize,
    pub warp_deformers: usize,
    pub rotation_deformers: usize,
    pub art_meshes: usize,
    pub parameters: usize,
    pub part_keyforms: usize,
    pub warp_keyforms: usize,
    pub rotation_keyforms: usize,
    pub art_mesh_keyforms: usize,
    pub keyform_positions: usize,
    pub parameter_binding_index: usize,
    pub keyform_bindings: usize,
    pub parameter_bindings: usize,
    pub keys: usize,
    pub uvs: usize,
    pub position_indices: usize,
    pub drawable_masks: usize,
    pub draw_order_groups: usize,
    pub draw_order_group_objects: usize,
    pub glue: usize,
}

impl ExportReport {
    /// Serialize in the house canonical form (2-space indent,
    /// one trailing newline), mirroring the native emitters.
    pub fn to_canonical_json(&self) -> String {
        let mut text =
            serde_json::to_string_pretty(self).expect("plain-data serialization is infallible");
        text.push('\n');
        text
    }
}

fn export_error(message: String) -> ConvertError {
    ConvertError::Export { message }
}

/// The lattice form payloads all carry their coordinate in an
/// `at` field; this trait lets the permutation be written once.
trait LatticeForm {
    fn at(&self) -> &[u32];
}

impl LatticeForm for RotationForm {
    fn at(&self) -> &[u32] {
        &self.at
    }
}

impl LatticeForm for WarpForm {
    fn at(&self) -> &[u32] {
        &self.at
    }
}

impl LatticeForm for MeshForm {
    fn at(&self) -> &[u32] {
        &self.at
    }
}

/// Resolve a lattice's axes to (parameter index, key list)
/// pairs, validating the key lists. Axis parameters must be
/// scalar parameters of the model.
fn lattice_axes<F>(
    lattice: &Lattice<F>,
    param_index: &HashMap<&str, usize>,
    what: &str,
) -> Result<Vec<(usize, Vec<f32>)>, ConvertError> {
    if lattice.axes.len() > 2 {
        return Err(export_error(format!(
            "{what}: lattice has {} axes; moc3 keyform bindings carry at most two",
            lattice.axes.len()
        )));
    }
    if lattice.keys.len() != lattice.axes.len() {
        return Err(export_error(format!(
            "{what}: lattice declares {} axes but {} key lists",
            lattice.axes.len(),
            lattice.keys.len()
        )));
    }
    let mut out = Vec::with_capacity(lattice.axes.len());
    for (axis, id) in lattice.axes.iter().enumerate() {
        let parameter = param_index.get(id.as_str()).copied().ok_or_else(|| {
            export_error(format!(
                "{what}: lattice axis names unknown or non-scalar parameter {id:?}"
            ))
        })?;
        let keys = &lattice.keys[axis];
        if keys.is_empty() {
            return Err(export_error(format!(
                "{what}: axis for parameter {id:?} has no keys"
            )));
        }
        for key in keys {
            if !key.is_finite() {
                return Err(export_error(format!(
                    "{what}: axis key {key} for parameter {id:?} is not finite"
                )));
            }
        }
        if keys.windows(2).any(|pair| pair[0] >= pair[1]) {
            return Err(export_error(format!(
                "{what}: axis keys for parameter {id:?} are not strictly increasing"
            )));
        }
        out.push((parameter, keys.clone()));
    }
    Ok(out)
}

/// The form indices of a lattice in moc3 storage order (first
/// axis fastest — MAPPING.md section 3.3). Validates that the
/// lattice is a complete grid: form count equals the axis
/// product, every form's coordinate is in range, and no
/// coordinate repeats. A sparse lattice is a typed error —
/// moc3 has no sparse-form encoding.
fn moc3_form_order<F: LatticeForm>(
    lattice: &Lattice<F>,
    what: &str,
) -> Result<Vec<usize>, ConvertError> {
    let axes = lattice.axes.len();
    let mut product: usize = 1;
    for keys in &lattice.keys {
        product = product
            .checked_mul(keys.len())
            .ok_or_else(|| export_error(format!("{what}: lattice axis product overflows")))?;
    }
    if lattice.forms.len() != product {
        return Err(export_error(format!(
            "{what}: incomplete lattice: {} forms for an axis product of {product} (moc3 has no sparse-form encoding)",
            lattice.forms.len()
        )));
    }
    let mut by_coord: HashMap<Vec<u32>, usize> = HashMap::with_capacity(lattice.forms.len());
    for (index, form) in lattice.forms.iter().enumerate() {
        let at = form.at();
        if at.len() != axes {
            return Err(export_error(format!(
                "{what}: form {index} coordinate has {} components for a {axes}-axis lattice",
                at.len()
            )));
        }
        for (axis, component) in at.iter().enumerate() {
            if *component as usize >= lattice.keys[axis].len() {
                return Err(export_error(format!(
                    "{what}: form {index} coordinate {at:?} is outside the lattice grid"
                )));
            }
        }
        if by_coord.insert(at.to_vec(), index).is_some() {
            return Err(export_error(format!(
                "{what}: two forms share lattice coordinate {at:?}"
            )));
        }
    }
    let form_at = |coord: &[u32]| {
        by_coord.get(coord).copied().ok_or_else(|| {
            export_error(format!("{what}: lattice coordinate {coord:?} has no form"))
        })
    };
    let mut order = Vec::with_capacity(product);
    match axes {
        0 => order.push(form_at(&[])?),
        1 => {
            for i0 in 0..lattice.keys[0].len() {
                order.push(form_at(&[i0 as u32])?);
            }
        }
        _ => {
            let n0 = lattice.keys[0].len();
            for ordinal in 0..product {
                let i0 = (ordinal % n0) as u32;
                let i1 = (ordinal / n0) as u32;
                order.push(form_at(&[i0, i1])?);
            }
        }
    }
    Ok(order)
}

/// The parameter-binding architecture (MAPPING.md section
/// 3.3): parameter maps are deduplicated per (parameter, key
/// list) and created grouped by parameter so each parameter's
/// maps stay contiguous; binding sets are deduplicated by
/// their reference lists. Creation order is first-use order,
/// gathered by walking the objects in native declaration
/// order. Hash maps are used as dictionaries only — iteration
/// order never reaches the output, so export is
/// deterministic.
struct BindingBuilder {
    /// Per parameter: the distinct key lists used, first-use
    /// order.
    declared: Vec<Vec<Vec<f32>>>,
    maps: Vec<ParamMapData>,
    keys: Vec<f32>,
    map_lookup: HashMap<(usize, Vec<u32>), usize>,
    param_ranges: Vec<(usize, usize)>,
    refs: Vec<usize>,
    sets: Vec<BindingSetData>,
    set_lookup: HashMap<Vec<usize>, usize>,
}

impl BindingBuilder {
    fn new(param_count: usize) -> BindingBuilder {
        let mut set_lookup = HashMap::new();
        set_lookup.insert(Vec::new(), 0usize);
        BindingBuilder {
            declared: vec![Vec::new(); param_count],
            maps: Vec::new(),
            keys: Vec::new(),
            map_lookup: HashMap::new(),
            param_ranges: vec![(0, 0); param_count],
            refs: Vec::new(),
            sets: vec![BindingSetData {
                ref_begin: 0,
                ref_count: 0,
            }],
            set_lookup,
        }
    }

    /// The shared empty binding set (index 0), referenced by
    /// every unbound object — the shape the sample files use.
    fn empty_set(&self) -> usize {
        0
    }

    /// Record one axis use (pass 1).
    fn register(&mut self, parameter: usize, keys: &[f32]) {
        let bits: Vec<u32> = keys.iter().map(|key| key.to_bits()).collect();
        let lists = &mut self.declared[parameter];
        let known = lists.iter().any(|existing| {
            existing
                .iter()
                .map(|key| key.to_bits())
                .collect::<Vec<u32>>()
                == bits
        });
        if !known {
            lists.push(keys.to_vec());
        }
    }

    /// Create the parameter maps, grouped by parameter
    /// (pass 2).
    fn build_maps(&mut self) {
        for parameter in 0..self.declared.len() {
            let begin = self.maps.len();
            for keys in &self.declared[parameter] {
                let index = self.maps.len();
                self.map_lookup.insert(
                    (parameter, keys.iter().map(|key| key.to_bits()).collect()),
                    index,
                );
                self.maps.push(ParamMapData {
                    key_begin: self.keys.len(),
                    key_count: keys.len(),
                });
                self.keys.extend_from_slice(keys);
            }
            self.param_ranges[parameter] = (begin, self.maps.len() - begin);
        }
    }

    /// The binding set for one object's axes, deduplicated.
    fn set_for(&mut self, axes: &[(usize, Vec<f32>)]) -> usize {
        let refs: Vec<usize> = axes
            .iter()
            .map(|(parameter, keys)| {
                self.map_lookup[&(
                    *parameter,
                    keys.iter().map(|key| key.to_bits()).collect::<Vec<u32>>(),
                )]
            })
            .collect();
        if let Some(index) = self.set_lookup.get(&refs) {
            return *index;
        }
        let index = self.sets.len();
        self.sets.push(BindingSetData {
            ref_begin: self.refs.len(),
            ref_count: refs.len(),
        });
        self.refs.extend_from_slice(&refs);
        self.set_lookup.insert(refs, index);
        index
    }
}

/// The V3_0 render-config blend bits for a native blend mode,
/// or the D-4 typed error (MAPPING.md section 6.1).
fn blend_bits(mesh: &Mesh) -> Result<u8, ConvertError> {
    match mesh.blend_mode {
        BlendMode::Normal => Ok(0),
        BlendMode::LinearDodge => Ok(1),
        BlendMode::Multiply => Ok(2),
        other => Err(export_error(format!(
            "art mesh {:?}: blend mode {} has no V3_0 encoding (only Normal, LinearDodge, and Multiply exist in the V3_0 render-config byte; the rest belong to the V5_3 blend word) — export refused (MAPPING D-4)",
            mesh.id,
            format!("{other:?}").to_lowercase()
        ))),
    }
}

/// Export a validated native model as a `.moc3` byte stream
/// plus its export report. `canvas` is an exporter input
/// (Q1 option (a)); `version` selects the writer layout —
/// only [`Moc3Version::V3_0`] is implemented.
pub fn export_model(
    model: &Model,
    canvas: &CanvasSpec,
    version: Moc3Version,
) -> Result<ExportOutcome, ConvertError> {
    if version != Moc3Version::V3_0 {
        return Err(export_error(format!(
            "moc3 version {} is not implemented by the exporter (V3_0 only in this slice)",
            version.label()
        )));
    }
    let mut lossy: Vec<ReportEntry> = Vec::new();
    let mut no_map: Vec<ReportEntry> = Vec::new();
    let mut notes: Vec<String> = Vec::new();

    // -- Parameters (MAPPING section 3). Combined fails (D-1).
    let mut param_index: HashMap<&str, usize> = HashMap::new();
    for (index, parameter) in model.parameters.iter().enumerate() {
        match parameter {
            Parameter::Scalar(_) => {
                param_index.insert(parameter.id(), index);
            }
            Parameter::Combined(combined) => {
                return Err(export_error(format!(
                    "parameter {:?} is a Combined (derived) parameter; moc3 parameters are plain scalar ranges and nothing in the format computes one parameter from others — export refused (MAPPING D-1)",
                    combined.id
                )));
            }
        }
    }

    // -- Index maps for cross-references.
    let part_index: HashMap<&str, usize> = model
        .parts
        .iter()
        .enumerate()
        .map(|(index, part)| (part.id.as_str(), index))
        .collect();
    let deformer_index: HashMap<&str, usize> = model
        .deformers
        .iter()
        .enumerate()
        .map(|(index, deformer)| (deformer.id(), index))
        .collect();
    let mesh_index: HashMap<&str, usize> = model
        .meshes
        .iter()
        .enumerate()
        .map(|(index, mesh)| (mesh.id.as_str(), index))
        .collect();
    let texture_index: HashMap<&str, usize> = model
        .textures
        .iter()
        .enumerate()
        .map(|(index, texture)| (texture.id.as_str(), index))
        .collect();

    // -- Bindings, pass 1: register every lattice's axes in
    // native declaration order (deformers, then meshes).
    let mut bindings = BindingBuilder::new(model.parameters.len());
    let mut deformer_axes: Vec<Vec<(usize, Vec<f32>)>> = Vec::with_capacity(model.deformers.len());
    for deformer in &model.deformers {
        let axes = match deformer {
            Deformer::Rotation(rotation) => match &rotation.keyforms {
                Some(lattice) => lattice_axes(
                    lattice,
                    &param_index,
                    &format!("rotation deformer {:?}", rotation.id),
                )?,
                None => Vec::new(),
            },
            Deformer::Warp(warp) => match &warp.keyforms {
                Some(lattice) => lattice_axes(
                    lattice,
                    &param_index,
                    &format!("warp deformer {:?}", warp.id),
                )?,
                None => Vec::new(),
            },
        };
        for (parameter, keys) in &axes {
            bindings.register(*parameter, keys);
        }
        deformer_axes.push(axes);
    }
    let mut mesh_axes: Vec<Vec<(usize, Vec<f32>)>> = Vec::with_capacity(model.meshes.len());
    for mesh in &model.meshes {
        let axes = match &mesh.keyforms {
            Some(lattice) => {
                lattice_axes(lattice, &param_index, &format!("art mesh {:?}", mesh.id))?
            }
            None => Vec::new(),
        };
        for (parameter, keys) in &axes {
            bindings.register(*parameter, keys);
        }
        mesh_axes.push(axes);
    }
    bindings.build_maps();

    // -- Parameters (writer records).
    let parameters: Vec<ParamData> = model
        .parameters
        .iter()
        .enumerate()
        .map(|(index, parameter)| match parameter {
            Parameter::Scalar(scalar) => {
                let (map_begin, map_count) = bindings.param_ranges[index];
                ParamData {
                    id: scalar.id.clone(),
                    min: scalar.min,
                    max: scalar.max,
                    default: scalar.default,
                    repeat: false,
                    snap_type: SNAP_NORMAL,
                    map_begin,
                    map_count,
                }
            }
            Parameter::Combined(_) => unreachable!("Combined parameters refused above"),
        })
        .collect();

    // -- Parts (section 4). Exactly one form each, depth 0.
    let mut parts: Vec<PartData> = Vec::with_capacity(model.parts.len());
    for part in &model.parts {
        let parent = match &part.parent {
            None => -1,
            Some(id) => {
                let index = part_index.get(id.as_str()).copied().ok_or_else(|| {
                    export_error(format!(
                        "part {:?} names unknown parent part {id:?}",
                        part.id
                    ))
                })?;
                i32::try_from(index).map_err(|_| export_error("part index overflow".to_string()))?
            }
        };
        if part.opacity != 1.0 {
            no_map.push(ReportEntry {
                item: "part opacity".to_string(),
                detail: format!(
                    "part {:?} has opacity {}; the V3_0 part schema carries no per-part opacity (PartForm holds depth only), so the value is not written",
                    part.id, part.opacity
                ),
            });
        }
        parts.push(PartData {
            id: part.id.clone(),
            parent,
            binding_set: bindings.empty_set(),
            visible_art_meshes: true,
            visible_deformers: true,
        });
    }
    let part_form_depths = vec![0.0; model.parts.len()];

    // -- Deformers (section 5) and their form pools.
    let mut deformers: Vec<DeformerData> = Vec::with_capacity(model.deformers.len());
    let mut warps: Vec<WarpData> = Vec::new();
    let mut warp_form_opacities: Vec<f32> = Vec::new();
    let mut warp_form_position_begins: Vec<usize> = Vec::new();
    let mut rotations: Vec<RotationData> = Vec::new();
    let mut rotation_forms: Vec<RotationFormData> = Vec::new();
    let mut keyform_positions: Vec<f32> = Vec::new();

    for (index, deformer) in model.deformers.iter().enumerate() {
        let what = format!("deformer {:?}", deformer.id());
        let parent = match deformer.parent() {
            None => -1,
            Some(id) => {
                let found = deformer_index.get(id).copied().ok_or_else(|| {
                    export_error(format!("{what} names unknown parent deformer {id:?}"))
                })?;
                i32::try_from(found)
                    .map_err(|_| export_error("deformer index overflow".to_string()))?
            }
        };
        let binding_set = bindings.set_for(&deformer_axes[index]);
        match deformer {
            Deformer::Warp(warp) => {
                let vertex_count = ((warp.columns as u64 + 1) * (warp.rows as u64 + 1)) as usize;
                if warp.rest_positions.len() != vertex_count {
                    return Err(export_error(format!(
                        "{what}: rest grid has {} points for a {}x{} cell grid ({vertex_count} expected)",
                        warp.rest_positions.len(),
                        warp.columns,
                        warp.rows
                    )));
                }
                let form_begin = warp_form_opacities.len();
                let form_count = match &warp.keyforms {
                    None => {
                        warp_form_position_begins.push(keyform_positions.len());
                        push_positions(&mut keyform_positions, &warp.rest_positions);
                        warp_form_opacities.push(1.0);
                        1
                    }
                    Some(lattice) => {
                        let order = moc3_form_order(lattice, &what)?;
                        for form_index in &order {
                            let form = &lattice.forms[*form_index];
                            if form.positions.len() != vertex_count {
                                return Err(export_error(format!(
                                    "{what}: keyform at {:?} has {} grid points, {vertex_count} expected",
                                    form.at,
                                    form.positions.len()
                                )));
                            }
                            warp_form_position_begins.push(keyform_positions.len());
                            push_positions(&mut keyform_positions, &form.positions);
                            warp_form_opacities.push(1.0);
                        }
                        order.len()
                    }
                };
                warps.push(WarpData {
                    binding_set,
                    form_begin,
                    form_count,
                    vertex_count,
                    rows: warp.rows as usize,
                    columns: warp.columns as usize,
                });
                deformers.push(DeformerData {
                    id: warp.id.clone(),
                    binding_set,
                    unknown_flag: true,
                    visible: true,
                    parent_part: -1,
                    parent,
                    kind: 0,
                    typed_index: warps.len() - 1,
                });
                lossy.push(ReportEntry {
                    item: "warp evaluation semantics".to_string(),
                    detail: format!(
                        "warp deformer {:?}: V3_0 has no bilinear-interpolation flag (it arrives at V3_3), so pre-V3_3 runtimes evaluate this grid with the triangle-split rule; the grid data itself is written exactly",
                        warp.id
                    ),
                });
            }
            Deformer::Rotation(rotation) => {
                let form_begin = rotation_forms.len();
                let form_count = match &rotation.keyforms {
                    None => {
                        rotation_forms.push(RotationFormData {
                            opacity: 1.0,
                            angle: 0.0,
                            origin: rotation.origin,
                            scale: 1.0,
                            reflect_x: false,
                            reflect_y: false,
                        });
                        1
                    }
                    Some(lattice) => {
                        let order = moc3_form_order(lattice, &what)?;
                        for form_index in &order {
                            let form = &lattice.forms[*form_index];
                            if form.scale[0] != form.scale[1] {
                                return Err(export_error(format!(
                                    "rotation deformer {:?} keyform at {:?} has non-uniform scale {:?}; moc3 rotation forms carry a single scale plus reflect flags, and native scales are positive (never mirrored) — export refused (MAPPING D-3)",
                                    rotation.id, form.at, form.scale
                                )));
                            }
                            rotation_forms.push(RotationFormData {
                                opacity: 1.0,
                                angle: form.angle_degrees,
                                origin: rotation.origin,
                                scale: form.scale[0],
                                reflect_x: false,
                                reflect_y: false,
                            });
                        }
                        order.len()
                    }
                };
                rotations.push(RotationData {
                    binding_set,
                    form_begin,
                    form_count,
                    base_angle: rotation.base_angle_degrees,
                });
                deformers.push(DeformerData {
                    id: rotation.id.clone(),
                    binding_set,
                    unknown_flag: true,
                    visible: true,
                    parent_part: -1,
                    parent,
                    kind: 1,
                    typed_index: rotations.len() - 1,
                });
            }
        }
    }

    // -- Art meshes (section 6) and their form pools.
    let mut meshes: Vec<MeshData> = Vec::with_capacity(model.meshes.len());
    let mut mesh_form_opacities: Vec<f32> = Vec::new();
    let mut mesh_form_depths: Vec<f32> = Vec::new();
    let mut mesh_form_position_begins: Vec<usize> = Vec::new();
    let mut uvs: Vec<f32> = Vec::new();
    let mut position_indices: Vec<u16> = Vec::new();
    let mut drawable_masks: Vec<i32> = Vec::new();

    for (index, mesh) in model.meshes.iter().enumerate() {
        let what = format!("art mesh {:?}", mesh.id);
        let vertex_count = mesh.vertices.len();
        if vertex_count > MESH_VERTICES_MAX {
            return Err(export_error(format!(
                "{what}: {vertex_count} vertices exceed the 65536 a u16 index pool can address — export refused (MAPPING section 6)"
            )));
        }
        if mesh.uvs.len() != vertex_count {
            return Err(export_error(format!(
                "{what}: {} uvs for {vertex_count} vertices",
                mesh.uvs.len()
            )));
        }
        if !(-DRAW_ORDER_BOUND..=DRAW_ORDER_BOUND).contains(&mesh.draw_order) {
            return Err(export_error(format!(
                "{what}: draw order {} is outside the native domain [-10000, 10000] the depth rescale is defined over",
                mesh.draw_order
            )));
        }
        let parent_part = {
            let found = part_index.get(mesh.part.as_str()).copied().ok_or_else(|| {
                export_error(format!("{what} names unknown part {:?}", mesh.part))
            })?;
            i32::try_from(found).map_err(|_| export_error("part index overflow".to_string()))?
        };
        let parent_deformer = match &mesh.deformer {
            None => -1,
            Some(id) => {
                let found = deformer_index
                    .get(id.as_str())
                    .copied()
                    .ok_or_else(|| export_error(format!("{what} names unknown deformer {id:?}")))?;
                i32::try_from(found)
                    .map_err(|_| export_error("deformer index overflow".to_string()))?
            }
        };
        let texture_number = texture_index
            .get(mesh.texture.as_str())
            .copied()
            .ok_or_else(|| {
                export_error(format!("{what} names unknown texture {:?}", mesh.texture))
            })?;
        // Double-sided bit set (culling off): the native format
        // has no culling concept, so rendering both sides is the
        // only emission consistent with the source (section 6.1).
        let render_config = blend_bits(mesh)? | 0x04;
        let depth = (mesh.draw_order + DRAW_ORDER_BOUND) as f32 / 20.0;
        lossy.push(ReportEntry {
            item: "draw order rescale".to_string(),
            detail: format!(
                "art mesh {:?}: native draw order {} written as moc3 depth {depth} ((d + 10000) / 20 into [0, 1000]); ordering is preserved exactly, the value's scale is not (MAPPING section 7)",
                mesh.id, mesh.draw_order
            ),
        });

        let form_begin = mesh_form_opacities.len();
        let form_count = match &mesh.keyforms {
            None => {
                mesh_form_position_begins.push(keyform_positions.len());
                push_positions(&mut keyform_positions, &mesh.vertices);
                mesh_form_opacities.push(mesh.opacity);
                mesh_form_depths.push(depth);
                1
            }
            Some(lattice) => {
                let order = moc3_form_order(lattice, &what)?;
                for form_index in &order {
                    let form = &lattice.forms[*form_index];
                    if form.vertices.len() != vertex_count {
                        return Err(export_error(format!(
                            "{what}: keyform at {:?} has {} vertices, {vertex_count} expected",
                            form.at,
                            form.vertices.len()
                        )));
                    }
                    mesh_form_position_begins.push(keyform_positions.len());
                    push_positions(&mut keyform_positions, &form.vertices);
                    mesh_form_opacities.push(mesh.opacity);
                    mesh_form_depths.push(depth);
                }
                order.len()
            }
        };
        let uv_begin = uvs.len();
        for uv in &mesh.uvs {
            uvs.extend_from_slice(uv);
        }
        let index_begin = position_indices.len();
        for vertex_index in &mesh.indices {
            let narrowed = u16::try_from(*vertex_index).map_err(|_| {
                export_error(format!(
                    "{what}: vertex index {vertex_index} does not fit the u16 index pool"
                ))
            })?;
            if (*vertex_index as usize) >= vertex_count {
                return Err(export_error(format!(
                    "{what}: vertex index {vertex_index} is outside its {vertex_count} vertices"
                )));
            }
            position_indices.push(narrowed);
        }
        let mask_begin = drawable_masks.len();
        for mask_id in &mesh.clipping_mask_ids {
            let found = mesh_index.get(mask_id.as_str()).copied().ok_or_else(|| {
                export_error(format!(
                    "{what} names unknown clipping mask mesh {mask_id:?}"
                ))
            })?;
            drawable_masks.push(
                i32::try_from(found)
                    .map_err(|_| export_error("mesh index overflow".to_string()))?,
            );
        }
        meshes.push(MeshData {
            id: mesh.id.clone(),
            binding_set: bindings.set_for(&mesh_axes[index]),
            form_begin,
            form_count,
            unknown_flag: true,
            visible: mesh.rendered,
            parent_part,
            parent_deformer,
            texture_number,
            render_config,
            vertex_count,
            uv_begin,
            index_begin,
            index_count: mesh.indices.len(),
            mask_begin,
            mask_count: mesh.clipping_mask_ids.len(),
        });
    }

    // -- Draw order (section 7, D-2): one flat group.
    let draw_items: Vec<DrawItemData> = (0..model.meshes.len())
        .map(|child| DrawItemData {
            item_type: 0,
            child,
            draw_group: -1,
        })
        .collect();
    let draw_groups = vec![DrawGroupData {
        item_begin: 0,
        item_count: draw_items.len(),
        total_art_meshes: model.meshes.len(),
        max_depth: crate::moc3_write::DRAW_DEPTH_MAX,
        min_depth: 0.0,
    }];

    // -- Physics: sidecar-only in the target ecosystem; the
    // binary carries none of it. Named per group so the E1
    // report is never silent about it (the physics3 mapping
    // itself is phase E3, MAPPING section 9).
    for group in &model.physics {
        no_map.push(ReportEntry {
            item: "physics group".to_string(),
            detail: format!(
                "physics group {:?} ({} strand(s)) is not carried in the .moc3 binary — physics is the physics3.json sidecar in the target ecosystem, written by exporter phase E3",
                group.id,
                group.strands.len()
            ),
        });
    }

    notes.push(
        "canvas values are the caller's exporter input (MAPPING Q1 option (a)); the native model carries no canvas and none was derived"
            .to_string(),
    );
    notes.push(
        "glue: counts 0, no sections — the native format has no glue construct (MAPPING section 8)"
            .to_string(),
    );
    notes.push(format!(
        "draw order: one flat group over all {} art mesh(es), group range [0, 1000] (MAPPING D-2)",
        model.meshes.len()
    ));
    if !model.physics.is_empty() {
        notes.push(
            "physics3 sidecar (MAPPING section 9, incl. its D-5/D-6 decisions and per-field NO-MAP rows) is phase E3 scope; E1 reports the groups as not carried rather than omitting them silently"
                .to_string(),
        );
    }

    let counts = ExportCounts {
        parts: parts.len(),
        deformers: deformers.len(),
        warp_deformers: warps.len(),
        rotation_deformers: rotations.len(),
        art_meshes: meshes.len(),
        parameters: parameters.len(),
        part_keyforms: part_form_depths.len(),
        warp_keyforms: warp_form_opacities.len(),
        rotation_keyforms: rotation_forms.len(),
        art_mesh_keyforms: mesh_form_opacities.len(),
        keyform_positions: keyform_positions.len(),
        parameter_binding_index: bindings.refs.len(),
        keyform_bindings: bindings.sets.len(),
        parameter_bindings: bindings.maps.len(),
        keys: bindings.keys.len(),
        uvs: uvs.len(),
        position_indices: position_indices.len(),
        drawable_masks: drawable_masks.len(),
        draw_order_groups: draw_groups.len(),
        draw_order_group_objects: draw_items.len(),
        glue: 0,
    };

    let data = Moc3Data {
        version,
        canvas: *canvas,
        parts,
        part_form_depths,
        deformers,
        warps,
        warp_form_opacities,
        warp_form_position_begins,
        rotations,
        rotation_forms,
        meshes,
        mesh_form_opacities,
        mesh_form_depths,
        mesh_form_position_begins,
        keyform_positions,
        parameters,
        parameter_maps: bindings.maps,
        binding_refs: bindings.refs,
        binding_sets: bindings.sets,
        keys: bindings.keys,
        uvs,
        position_indices,
        drawable_masks,
        draw_groups,
        draw_items,
    };
    let bytes = write_moc3(&data)?;
    Ok(ExportOutcome {
        bytes,
        report: ExportReport {
            format: "lumen-rig-export-report",
            format_version: 1,
            model_id: model.id.clone(),
            moc3_version: version.label().to_string(),
            counts,
            lossy,
            no_map,
            notes,
        },
    })
}

/// Append a form's [x, y] pairs to the shared position pool.
fn push_positions(pool: &mut Vec<f32>, points: &[[f32; 2]]) {
    for point in points {
        pool.extend_from_slice(point);
    }
}
