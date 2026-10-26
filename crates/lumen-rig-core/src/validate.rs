//! The validating parser: bytes in, validated `Model` out.
//!
//! Checks run in the order of SPEC.md section 8 and stop at the
//! first failure, so the reported error is deterministic. Nothing
//! is repaired, dropped, or clamped here — invalid input is a
//! typed `ParseError`, per the loud-error law.

use std::collections::{HashMap, HashSet};

use crate::error::{ParseError, bounded_quote};
use crate::limits::*;
use crate::model::*;
use crate::raw::*;

/// Parse and validate a model asset.
///
/// Contract: `bytes` is the whole file, at most
/// `INPUT_BYTES_MAX`; the result is either a fully validated
/// `Model` or the first `ParseError` in section-8 order.
pub fn parse_model(bytes: &[u8]) -> Result<Model, ParseError> {
    if bytes.len() > INPUT_BYTES_MAX {
        return Err(ParseError::InputTooLarge {
            bytes: bytes.len(),
            max: INPUT_BYTES_MAX,
        });
    }
    probe_header(bytes)?;
    let raw: RawModel = serde_json::from_slice(bytes).map_err(map_serde_error)?;
    validate_model(raw)
}

/// Section-8 step 2: read the header first, so a wrong format or
/// version fails as itself. A header that cannot be read at all
/// defers to the full parse, which reports the precise error.
fn probe_header(bytes: &[u8]) -> Result<(), ParseError> {
    let header: RawHeader = match serde_json::from_slice(bytes) {
        Ok(header) => header,
        Err(_) => return Ok(()),
    };
    match header.format.as_deref() {
        None => {
            return Err(ParseError::UnsupportedFormat {
                found: "<missing>".to_string(),
            });
        }
        Some(found) if found != "lumen-rig-model" => {
            return Err(ParseError::UnsupportedFormat {
                found: bounded_quote(found),
            });
        }
        _ => {}
    }
    if let Some(version) = header.format_version
        && version != FORMAT_VERSION
    {
        return Err(ParseError::UnsupportedVersion { found: version });
    }
    Ok(())
}

/// Map a serde_json failure onto the loud-error table. Number
/// overflow is `LRF-301` even though serde reports it as a parse
/// failure: the value, not the grammar, is what is wrong.
fn map_serde_error(err: serde_json::Error) -> ParseError {
    let message = bounded_quote(&err.to_string());
    if message.contains("number out of range") {
        return ParseError::NonFinite {
            context: "document".to_string(),
        };
    }
    if err.is_eof() {
        return ParseError::Syntax {
            line: err.line(),
            column: err.column(),
            message,
        };
    }
    if err.is_data() {
        if message.contains("unknown field") {
            return ParseError::UnknownField { message };
        }
        return ParseError::Schema { message };
    }
    ParseError::Syntax {
        line: err.line(),
        column: err.column(),
        message,
    }
}

/// An id must be 1..=64 bytes of `[A-Za-z0-9_-]` (SPEC section 1.3).
fn check_id(id: &str) -> Result<(), ParseError> {
    let valid = !id.is_empty()
        && id.len() <= ID_BYTES_MAX
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-');
    if valid {
        Ok(())
    } else {
        Err(ParseError::InvalidId {
            id: bounded_quote(id),
        })
    }
}

/// Check id syntax for a whole collection, then reject duplicates,
/// in declaration order (so the first offender is reported).
fn collect_ids<'a>(
    collection: &'static str,
    ids: impl Iterator<Item = &'a str>,
) -> Result<HashSet<&'a str>, ParseError> {
    let mut seen = HashSet::new();
    for id in ids {
        check_id(id)?;
        if !seen.insert(id) {
            return Err(ParseError::DuplicateId {
                collection,
                id: id.to_string(),
            });
        }
    }
    Ok(seen)
}

/// A float field must be finite (no NaN, no infinity, no f32
/// overflow smuggled in as an f64 the parser accepted).
fn check_finite(value: f32, context: &str) -> Result<f32, ParseError> {
    if value.is_finite() {
        Ok(value)
    } else {
        Err(ParseError::NonFinite {
            context: context.to_string(),
        })
    }
}

/// A model-space point: finite, and inside the magnitude bound.
fn check_point(point: [f32; 2], context: &str) -> Result<[f32; 2], ParseError> {
    for component in point {
        check_finite(component, context)?;
        if component.abs() > COORD_MAGNITUDE_MAX {
            return Err(ParseError::FieldRange {
                context: context.to_string(),
            });
        }
    }
    Ok(point)
}

/// A texture path must satisfy SPEC.md section 3.3.
fn check_texture_path(texture_id: &str, source: &str) -> Result<(), ParseError> {
    let valid = !source.is_empty()
        && source.len() <= PATH_BYTES_MAX
        && !source.starts_with('/')
        && !source.contains('\\')
        && !source.contains('\0')
        && !source.split('/').any(|component| component == "..");
    if valid {
        Ok(())
    } else {
        Err(ParseError::InvalidTexturePath {
            texture_id: texture_id.to_string(),
            source: bounded_quote(source),
        })
    }
}

/// Walk every parent chain in a forest: parents must exist
/// (checked by the caller into `parents` construction — a missing
/// parent yields a `None` lookup here only if the caller skipped
/// the existence check, so callers check first), chains must be
/// acyclic and at most `TREE_DEPTH_MAX` deep.
fn check_parent_chains(
    collection: &'static str,
    parents: &HashMap<&str, Option<&str>>,
    order: &[&str],
) -> Result<(), ParseError> {
    for &start in order {
        let mut seen: HashSet<&str> = HashSet::new();
        let mut current = start;
        loop {
            if !seen.insert(current) {
                return Err(ParseError::ReferenceCycle {
                    collection,
                    id: start.to_string(),
                });
            }
            if seen.len() > TREE_DEPTH_MAX {
                return Err(ParseError::ReferenceCycle {
                    collection,
                    id: start.to_string(),
                });
            }
            match parents.get(current) {
                Some(Some(parent)) => current = parent,
                _ => break,
            }
        }
    }
    Ok(())
}

/// The outcome of checking a lattice header: resolved axes and
/// keys, plus the exact number of forms the lattice must carry.
struct LatticeHead {
    axes: Vec<String>,
    keys: Vec<Vec<f32>>,
    form_count: usize,
}

/// Validate the shared lattice shape (SPEC.md section 3.7):
/// axes exist and are scalar, keys are strictly increasing and
/// in range, and the `at` coordinates are complete, in-bounds,
/// and in canonical (ascending) order.
fn check_lattice_head(
    owner_collection: &'static str,
    owner_id: &str,
    axes: &[String],
    keys: &[Vec<f32>],
    ats: &[Vec<u32>],
    params: &HashMap<&str, &Parameter>,
) -> Result<LatticeHead, ParseError> {
    let context = format!("{owner_collection} {owner_id:?} keyforms");
    let invalid = |reason: &'static str| ParseError::LatticeInvalid {
        context: context.clone(),
        reason,
    };
    if axes.is_empty() || axes.len() > 2 {
        return Err(invalid("axis count must be 1 or 2"));
    }
    if keys.len() != axes.len() {
        return Err(invalid("keys must have one row per axis"));
    }
    if axes.len() == 2 && axes[0] == axes[1] {
        return Err(ParseError::WrongKind {
            context: context.clone(),
            id: axes[0].clone(),
        });
    }
    let mut form_count: usize = 1;
    for (axis_index, axis) in axes.iter().enumerate() {
        let param = params
            .get(axis.as_str())
            .ok_or_else(|| ParseError::DanglingReference {
                collection: owner_collection,
                from_id: owner_id.to_string(),
                reference: axis.clone(),
            })?;
        let scalar = match param {
            Parameter::Scalar(scalar) => scalar,
            Parameter::Combined(_) => {
                return Err(ParseError::WrongKind {
                    context: context.clone(),
                    id: axis.clone(),
                });
            }
        };
        let row = &keys[axis_index];
        if row.len() < 2 || row.len() > KEYS_PER_AXIS_MAX {
            return Err(invalid("key count per axis must be 2..=32"));
        }
        for (key_index, key) in row.iter().enumerate() {
            check_finite(*key, &context)?;
            if key_index > 0 && *key <= row[key_index - 1] {
                return Err(invalid("keys must be strictly increasing"));
            }
            if *key < scalar.min || *key > scalar.max {
                return Err(invalid("key outside axis parameter range"));
            }
        }
        form_count = form_count.saturating_mul(row.len());
    }
    if ats.len() != form_count {
        return Err(invalid("form count does not match the lattice size"));
    }
    let mut previous: Option<&Vec<u32>> = None;
    for at in ats {
        if at.len() != axes.len() {
            return Err(invalid("form coordinate rank differs from axis count"));
        }
        for (axis_index, index) in at.iter().enumerate() {
            if (*index as usize) >= keys[axis_index].len() {
                return Err(invalid("form coordinate out of range"));
            }
        }
        if let Some(prev) = previous
            && at <= prev
        {
            return Err(invalid("forms are not in canonical ascending order"));
        }
        previous = Some(at);
    }
    Ok(LatticeHead {
        axes: axes.to_vec(),
        keys: keys.to_vec(),
        form_count,
    })
}

/// Validate a raw model into a `Model`. Called by `parse_model`;
/// also usable directly on an already-parsed raw document.
pub fn validate_model(raw: RawModel) -> Result<Model, ParseError> {
    // The probe (parse_model) reports header errors first; this
    // re-check keeps validate_model sound when called directly.
    if raw.format != "lumen-rig-model" {
        return Err(ParseError::UnsupportedFormat {
            found: bounded_quote(&raw.format),
        });
    }
    if raw.format_version != FORMAT_VERSION {
        return Err(ParseError::UnsupportedVersion {
            found: raw.format_version,
        });
    }
    check_top_counts(&raw)?;
    check_id(&raw.id)?;
    if let Some(generator) = &raw.generator
        && generator.len() > NAME_BYTES_MAX
    {
        return Err(ParseError::FieldRange {
            context: "generator exceeds NAME_BYTES_MAX".to_string(),
        });
    }

    let param_ids = collect_ids("parameters", raw.parameters.iter().map(raw_parameter_id))?;
    let part_ids = collect_ids("parts", raw.parts.iter().map(|part| part.id.as_str()))?;
    let texture_ids = collect_ids(
        "textures",
        raw.textures.iter().map(|texture| texture.id.as_str()),
    )?;
    let deformer_ids = collect_ids("deformers", raw.deformers.iter().map(raw_deformer_id))?;
    let mesh_ids = collect_ids("meshes", raw.meshes.iter().map(|mesh| mesh.id.as_str()))?;
    let _group_ids = collect_ids(
        "physics groups",
        raw.physics.iter().map(|group| group.id.as_str()),
    )?;
    for group in &raw.physics {
        collect_ids(
            "strands",
            group.strands.iter().map(|strand| strand.id.as_str()),
        )?;
    }

    let parameters = convert_parameters(&raw.parameters, &param_ids)?;
    let param_map: HashMap<&str, &Parameter> =
        parameters.iter().map(|param| (param.id(), param)).collect();
    let parts = convert_parts(&raw.parts, &part_ids)?;
    let textures = convert_textures(&raw.textures)?;
    let deformers = convert_deformers(&raw.deformers, &deformer_ids, &param_map)?;
    let meshes = convert_meshes(
        &raw.meshes,
        &part_ids,
        &texture_ids,
        &deformer_ids,
        &mesh_ids,
        &param_map,
    )?;
    check_mask_cycles(&meshes)?;
    let physics = convert_physics(&raw.physics, &deformer_ids, &mesh_ids, &param_map)?;

    Ok(Model {
        id: raw.id,
        generator: raw.generator,
        parameters,
        parts,
        textures,
        deformers,
        meshes,
        physics,
    })
}

/// Section-8 step 4: top-level collection count limits.
fn check_top_counts(raw: &RawModel) -> Result<(), ParseError> {
    let counts: [(&'static str, usize, usize); 6] = [
        ("parameters", raw.parameters.len(), PARAMETERS_MAX),
        ("parts", raw.parts.len(), PARTS_MAX),
        ("textures", raw.textures.len(), TEXTURES_MAX),
        ("deformers", raw.deformers.len(), DEFORMERS_MAX),
        ("meshes", raw.meshes.len(), MESHES_MAX),
        ("physics groups", raw.physics.len(), PHYSICS_GROUPS_MAX),
    ];
    for (collection, count, max) in counts {
        if count > max {
            return Err(ParseError::CountExceeded {
                collection,
                count,
                max,
            });
        }
    }
    for group in &raw.physics {
        if group.strands.len() > STRANDS_PER_GROUP_MAX {
            return Err(ParseError::CountExceeded {
                collection: "strands",
                count: group.strands.len(),
                max: STRANDS_PER_GROUP_MAX,
            });
        }
    }
    Ok(())
}

fn raw_parameter_id(param: &RawParameter) -> &str {
    match param {
        RawParameter::Scalar { id, .. } => id,
        RawParameter::Combined { id, .. } => id,
    }
}

fn raw_deformer_id(deformer: &RawDeformer) -> &str {
    match deformer {
        RawDeformer::Rotation { id, .. } => id,
        RawDeformer::Warp { id, .. } => id,
    }
}

fn raw_deformer_parent(deformer: &RawDeformer) -> Option<&str> {
    match deformer {
        RawDeformer::Rotation { parent, .. } => parent.as_deref(),
        RawDeformer::Warp { parent, .. } => parent.as_deref(),
    }
}

/// Convert parameters and prove the combined-parameter laws:
/// ranges, defaults, term references, acyclicity, and
/// reachable-range containment (SPEC.md section 3.1).
fn convert_parameters(
    raw: &[RawParameter],
    param_ids: &HashSet<&str>,
) -> Result<Vec<Parameter>, ParseError> {
    let mut out: Vec<Parameter> = Vec::with_capacity(raw.len());
    for param in raw {
        match param {
            RawParameter::Scalar {
                id,
                min,
                max,
                default,
            } => {
                let context = format!("parameter {id:?}");
                check_finite(*min, &context)?;
                check_finite(*max, &context)?;
                check_finite(*default, &context)?;
                if min >= max {
                    return Err(ParseError::ParameterOrder {
                        id: id.clone(),
                        min: *min,
                        max: *max,
                    });
                }
                if default < min || default > max {
                    return Err(ParseError::OutOfRange {
                        context: format!("parameter {id:?} default"),
                    });
                }
                out.push(Parameter::Scalar(ScalarParameter {
                    id: id.clone(),
                    min: *min,
                    max: *max,
                    default: *default,
                }));
            }
            RawParameter::Combined {
                id,
                min,
                max,
                terms,
                offset,
            } => {
                let context = format!("combined parameter {id:?}");
                check_finite(*min, &context)?;
                check_finite(*max, &context)?;
                check_finite(*offset, &context)?;
                if min >= max {
                    return Err(ParseError::ParameterOrder {
                        id: id.clone(),
                        min: *min,
                        max: *max,
                    });
                }
                if terms.is_empty() {
                    return Err(ParseError::FieldRange {
                        context: format!("combined parameter {id:?} has no terms"),
                    });
                }
                if terms.len() > COMBINED_TERMS_MAX {
                    return Err(ParseError::CountExceeded {
                        collection: "combined terms",
                        count: terms.len(),
                        max: COMBINED_TERMS_MAX,
                    });
                }
                let mut converted_terms = Vec::with_capacity(terms.len());
                for term in terms {
                    check_finite(term.weight, &context)?;
                    if !param_ids.contains(term.parameter.as_str()) {
                        return Err(ParseError::DanglingReference {
                            collection: "parameters",
                            from_id: id.clone(),
                            reference: term.parameter.clone(),
                        });
                    }
                    converted_terms.push(CombinedTerm {
                        parameter: term.parameter.clone(),
                        weight: term.weight,
                    });
                }
                out.push(Parameter::Combined(CombinedParameter {
                    id: id.clone(),
                    min: *min,
                    max: *max,
                    terms: converted_terms,
                    offset: *offset,
                }));
            }
        }
    }
    check_combined_graph(&out)?;
    Ok(out)
}

/// The combined graph must be acyclic, and each combined
/// parameter's reachable range (f64 interval arithmetic over its
/// terms) must be contained in its declared range.
fn check_combined_graph(params: &[Parameter]) -> Result<(), ParseError> {
    let mut ranges: HashMap<&str, (f64, f64)> = HashMap::new();
    for param in params {
        if let Parameter::Scalar(scalar) = param {
            ranges.insert(
                scalar.id.as_str(),
                (f64::from(scalar.min), f64::from(scalar.max)),
            );
        }
    }
    let combined: Vec<&CombinedParameter> = params
        .iter()
        .filter_map(|param| match param {
            Parameter::Combined(combined) => Some(combined),
            Parameter::Scalar(_) => None,
        })
        .collect();
    loop {
        let mut progress = false;
        for combined_param in &combined {
            if ranges.contains_key(combined_param.id.as_str()) {
                continue;
            }
            let ready = combined_param
                .terms
                .iter()
                .all(|term| ranges.contains_key(term.parameter.as_str()));
            if !ready {
                continue;
            }
            let mut lo = f64::from(combined_param.offset);
            let mut hi = lo;
            for term in &combined_param.terms {
                let (term_lo, term_hi) = ranges[term.parameter.as_str()];
                let weight = f64::from(term.weight);
                lo += (weight * term_lo).min(weight * term_hi);
                hi += (weight * term_lo).max(weight * term_hi);
            }
            if f64::from(combined_param.min) > lo || hi > f64::from(combined_param.max) {
                return Err(ParseError::CombinedRangeNotContained {
                    id: combined_param.id.clone(),
                });
            }
            ranges.insert(combined_param.id.as_str(), (lo, hi));
            progress = true;
        }
        if !progress {
            break;
        }
    }
    for combined_param in &combined {
        if !ranges.contains_key(combined_param.id.as_str()) {
            return Err(ParseError::CombinedCycle {
                id: combined_param.id.clone(),
            });
        }
    }
    Ok(())
}

/// Convert parts: resolve opacity defaults, check opacity range,
/// parent existence, and forest acyclicity.
fn convert_parts(raw: &[RawPart], part_ids: &HashSet<&str>) -> Result<Vec<Part>, ParseError> {
    let mut out = Vec::with_capacity(raw.len());
    let mut parents: HashMap<&str, Option<&str>> = HashMap::new();
    for part in raw {
        let opacity = match part.opacity {
            Some(value) => {
                check_finite(value, &format!("part {:?} opacity", part.id))?;
                if !(0.0..=1.0).contains(&value) {
                    return Err(ParseError::OutOfRange {
                        context: format!("part {:?} opacity", part.id),
                    });
                }
                value
            }
            None => 1.0,
        };
        if let Some(parent) = &part.parent
            && !part_ids.contains(parent.as_str())
        {
            return Err(ParseError::DanglingReference {
                collection: "parts",
                from_id: part.id.clone(),
                reference: parent.clone(),
            });
        }
        parents.insert(part.id.as_str(), part.parent.as_deref());
        out.push(Part {
            id: part.id.clone(),
            parent: part.parent.clone(),
            opacity,
        });
    }
    let order: Vec<&str> = raw.iter().map(|part| part.id.as_str()).collect();
    check_parent_chains("parts", &parents, &order)?;
    Ok(out)
}

/// Convert textures: check each source path (section 3.3).
fn convert_textures(raw: &[RawTexture]) -> Result<Vec<Texture>, ParseError> {
    let mut out = Vec::with_capacity(raw.len());
    for texture in raw {
        check_texture_path(&texture.id, &texture.source)?;
        out.push(Texture {
            id: texture.id.clone(),
            source: texture.source.clone(),
        });
    }
    Ok(out)
}

/// Convert deformers: parent existence and acyclicity first,
/// then per-kind fields and lattices.
fn convert_deformers(
    raw: &[RawDeformer],
    deformer_ids: &HashSet<&str>,
    params: &HashMap<&str, &Parameter>,
) -> Result<Vec<Deformer>, ParseError> {
    let mut parents: HashMap<&str, Option<&str>> = HashMap::new();
    for deformer in raw {
        let id = raw_deformer_id(deformer);
        if let Some(parent) = raw_deformer_parent(deformer)
            && !deformer_ids.contains(parent)
        {
            return Err(ParseError::DanglingReference {
                collection: "deformers",
                from_id: id.to_string(),
                reference: parent.to_string(),
            });
        }
        parents.insert(id, raw_deformer_parent(deformer));
    }
    let order: Vec<&str> = raw.iter().map(raw_deformer_id).collect();
    check_parent_chains("deformers", &parents, &order)?;

    let mut out = Vec::with_capacity(raw.len());
    for deformer in raw {
        match deformer {
            RawDeformer::Rotation {
                id,
                parent,
                origin,
                base_angle_degrees,
                keyforms,
            } => {
                let context = format!("deformer {id:?}");
                check_point(*origin, &context)?;
                check_finite(*base_angle_degrees, &context)?;
                let converted = match keyforms {
                    Some(lattice) => Some(convert_lattice(
                        "deformers",
                        id,
                        lattice,
                        params,
                        |form: &RawRotationForm| {
                            check_finite(form.angle_degrees, &context)?;
                            check_finite(form.scale[0], &context)?;
                            check_finite(form.scale[1], &context)?;
                            if form.scale[0] <= 0.0 || form.scale[1] <= 0.0 {
                                return Err(ParseError::FieldRange {
                                    context: format!("{context} keyform scale must be > 0"),
                                });
                            }
                            Ok(RotationForm {
                                at: form.at.clone(),
                                angle_degrees: form.angle_degrees,
                                scale: form.scale,
                            })
                        },
                    )?),
                    None => None,
                };
                out.push(Deformer::Rotation(RotationDeformer {
                    id: id.clone(),
                    parent: parent.clone(),
                    origin: *origin,
                    base_angle_degrees: *base_angle_degrees,
                    keyforms: converted,
                }));
            }
            RawDeformer::Warp {
                id,
                parent,
                columns,
                rows,
                rest_positions,
                keyforms,
            } => {
                let context = format!("deformer {id:?}");
                if !(1..=32).contains(columns) || !(1..=32).contains(rows) {
                    return Err(ParseError::FieldRange {
                        context: format!("{context} grid must be within 1..=32 per side"),
                    });
                }
                let expected = ((*columns as usize) + 1) * ((*rows as usize) + 1);
                if rest_positions.len() != expected {
                    return Err(ParseError::LengthMismatch {
                        context: format!("{context} rest_positions"),
                    });
                }
                for point in rest_positions {
                    check_point(*point, &context)?;
                }
                let converted = match keyforms {
                    Some(lattice) => Some(convert_lattice(
                        "deformers",
                        id,
                        lattice,
                        params,
                        |form: &RawWarpForm| {
                            if form.positions.len() != expected {
                                return Err(ParseError::LengthMismatch {
                                    context: format!("{context} keyform positions"),
                                });
                            }
                            for point in &form.positions {
                                check_point(*point, &context)?;
                            }
                            Ok(WarpForm {
                                at: form.at.clone(),
                                positions: form.positions.clone(),
                            })
                        },
                    )?),
                    None => None,
                };
                out.push(Deformer::Warp(WarpDeformer {
                    id: id.clone(),
                    parent: parent.clone(),
                    columns: *columns,
                    rows: *rows,
                    rest_positions: rest_positions.clone(),
                    keyforms: converted,
                }));
            }
        }
    }
    Ok(out)
}

/// Convert one raw lattice: header law from `check_lattice_head`,
/// payload law from the caller's closure.
fn convert_lattice<R, F>(
    owner_collection: &'static str,
    owner_id: &str,
    raw: &RawLattice<R>,
    params: &HashMap<&str, &Parameter>,
    convert_form: impl Fn(&R) -> Result<F, ParseError>,
) -> Result<Lattice<F>, ParseError>
where
    R: LatticeForm,
{
    let ats: Vec<Vec<u32>> = raw.forms.iter().map(|form| form.at().to_vec()).collect();
    let head = check_lattice_head(
        owner_collection,
        owner_id,
        &raw.axes,
        &raw.keys,
        &ats,
        params,
    )?;
    let mut forms = Vec::with_capacity(head.form_count);
    for form in &raw.forms {
        forms.push(convert_form(form)?);
    }
    Ok(Lattice {
        axes: head.axes,
        keys: head.keys,
        forms,
    })
}

/// The `at` coordinate of a raw keyform, whichever payload it has.
trait LatticeForm {
    fn at(&self) -> &[u32];
}

impl LatticeForm for RawRotationForm {
    fn at(&self) -> &[u32] {
        &self.at
    }
}

impl LatticeForm for RawWarpForm {
    fn at(&self) -> &[u32] {
        &self.at
    }
}

impl LatticeForm for RawMeshForm {
    fn at(&self) -> &[u32] {
        &self.at
    }
}

/// Convert meshes: references, geometry, resolved defaults,
/// blend modes, and lattices. Mask-cycle checking is separate
/// (`check_mask_cycles`) since it spans the whole collection.
fn convert_meshes(
    raw: &[RawMesh],
    part_ids: &HashSet<&str>,
    texture_ids: &HashSet<&str>,
    deformer_ids: &HashSet<&str>,
    mesh_ids: &HashSet<&str>,
    params: &HashMap<&str, &Parameter>,
) -> Result<Vec<Mesh>, ParseError> {
    let mut out = Vec::with_capacity(raw.len());
    for mesh in raw {
        let context = format!("mesh {:?}", mesh.id);
        for (reference, set) in [(&mesh.part, part_ids), (&mesh.texture, texture_ids)] {
            if !set.contains(reference.as_str()) {
                return Err(ParseError::DanglingReference {
                    collection: "meshes",
                    from_id: mesh.id.clone(),
                    reference: reference.clone(),
                });
            }
        }
        if let Some(deformer) = &mesh.deformer
            && !deformer_ids.contains(deformer.as_str())
        {
            return Err(ParseError::DanglingReference {
                collection: "meshes",
                from_id: mesh.id.clone(),
                reference: deformer.clone(),
            });
        }
        let masks = mesh.clipping_mask_ids.clone().unwrap_or_default();
        for mask in &masks {
            if !mesh_ids.contains(mask.as_str()) {
                return Err(ParseError::DanglingReference {
                    collection: "meshes",
                    from_id: mesh.id.clone(),
                    reference: mask.clone(),
                });
            }
            if mask == &mesh.id {
                return Err(ParseError::ReferenceCycle {
                    collection: "meshes",
                    id: mesh.id.clone(),
                });
            }
        }
        if mesh.vertices.is_empty() {
            return Err(ParseError::FieldRange {
                context: format!("{context} has no vertices"),
            });
        }
        if mesh.vertices.len() > VERTICES_PER_MESH_MAX {
            return Err(ParseError::CountExceeded {
                collection: "mesh vertices",
                count: mesh.vertices.len(),
                max: VERTICES_PER_MESH_MAX,
            });
        }
        for point in &mesh.vertices {
            check_point(*point, &context)?;
        }
        if mesh.uvs.len() != mesh.vertices.len() {
            return Err(ParseError::LengthMismatch {
                context: format!("{context} uvs"),
            });
        }
        for uv in &mesh.uvs {
            check_finite(uv[0], &context)?;
            check_finite(uv[1], &context)?;
        }
        if mesh.indices.len() > INDICES_PER_MESH_MAX {
            return Err(ParseError::CountExceeded {
                collection: "mesh indices",
                count: mesh.indices.len(),
                max: INDICES_PER_MESH_MAX,
            });
        }
        if mesh.indices.len() < 3 || mesh.indices.len() % 3 != 0 {
            return Err(ParseError::IndexCountInvalid {
                mesh_id: mesh.id.clone(),
                count: mesh.indices.len(),
            });
        }
        for index in &mesh.indices {
            if (*index as usize) >= mesh.vertices.len() {
                return Err(ParseError::IndexOutOfBounds {
                    mesh_id: mesh.id.clone(),
                    index: *index,
                    vertex_count: mesh.vertices.len(),
                });
            }
        }
        let opacity = match mesh.opacity {
            Some(value) => {
                check_finite(value, &context)?;
                if !(0.0..=1.0).contains(&value) {
                    return Err(ParseError::OutOfRange {
                        context: format!("{context} opacity"),
                    });
                }
                value
            }
            None => 1.0,
        };
        let blend_mode = match &mesh.blend_mode {
            None => BlendMode::Normal,
            Some(text) => BlendMode::from_format_str(text).ok_or_else(|| {
                ParseError::UnsupportedBlendMode {
                    mesh_id: mesh.id.clone(),
                    value: bounded_quote(text),
                }
            })?,
        };
        let draw_order = mesh.draw_order.unwrap_or(0);
        if !(DRAW_ORDER_MIN..=DRAW_ORDER_MAX).contains(&draw_order) {
            return Err(ParseError::FieldRange {
                context: format!("{context} draw_order"),
            });
        }
        let vertex_count = mesh.vertices.len();
        let converted_keyforms = match &mesh.keyforms {
            Some(lattice) => Some(convert_lattice(
                "meshes",
                &mesh.id,
                lattice,
                params,
                |form: &RawMeshForm| {
                    if form.vertices.len() != vertex_count {
                        return Err(ParseError::LengthMismatch {
                            context: format!("{context} keyform vertices"),
                        });
                    }
                    for point in &form.vertices {
                        check_point(*point, &context)?;
                    }
                    Ok(MeshForm {
                        at: form.at.clone(),
                        vertices: form.vertices.clone(),
                    })
                },
            )?),
            None => None,
        };
        out.push(Mesh {
            id: mesh.id.clone(),
            part: mesh.part.clone(),
            texture: mesh.texture.clone(),
            deformer: mesh.deformer.clone(),
            vertices: mesh.vertices.clone(),
            uvs: mesh.uvs.clone(),
            indices: mesh.indices.clone(),
            opacity,
            blend_mode,
            draw_order,
            rendered: mesh.rendered.unwrap_or(true),
            clipping_mask_ids: masks,
            keyforms: converted_keyforms,
        });
    }
    Ok(out)
}

/// The clipping-mask relation over all meshes must be acyclic
/// (SPEC.md section 3.5). Iterative DFS with an explicit stack:
/// no recursion over input-shaped depth.
fn check_mask_cycles(meshes: &[Mesh]) -> Result<(), ParseError> {
    let index_of: HashMap<&str, usize> = meshes
        .iter()
        .enumerate()
        .map(|(index, mesh)| (mesh.id.as_str(), index))
        .collect();
    // 0 = unvisited, 1 = on the current DFS path, 2 = finished.
    // Enter/Exit steps on an explicit stack: no recursion over
    // input-shaped depth.
    enum Step {
        Enter(usize),
        Exit(usize),
    }
    let mut state = vec![0u8; meshes.len()];
    for start in 0..meshes.len() {
        if state[start] != 0 {
            continue;
        }
        let mut stack: Vec<Step> = vec![Step::Enter(start)];
        while let Some(step) = stack.pop() {
            match step {
                Step::Enter(node) => {
                    if state[node] != 0 {
                        continue;
                    }
                    state[node] = 1;
                    stack.push(Step::Exit(node));
                    for mask in &meshes[node].clipping_mask_ids {
                        let next = index_of[mask.as_str()];
                        if state[next] == 1 {
                            return Err(ParseError::ReferenceCycle {
                                collection: "meshes",
                                id: meshes[next].id.clone(),
                            });
                        }
                        if state[next] == 0 {
                            stack.push(Step::Enter(next));
                        }
                    }
                }
                Step::Exit(node) => state[node] = 2,
            }
        }
    }
    Ok(())
}

/// Convert physics groups: resolved defaults, references,
/// per-field ranges, and the one-writer rule (SPEC.md 3.6).
fn convert_physics<'a>(
    raw: &'a [RawPhysicsGroup],
    deformer_ids: &HashSet<&str>,
    mesh_ids: &HashSet<&str>,
    params: &HashMap<&str, &Parameter>,
) -> Result<Vec<PhysicsGroup>, ParseError> {
    let mut written: HashSet<&'a str> = HashSet::new();
    let mut out = Vec::with_capacity(raw.len());
    for group in raw {
        let context = format!("physics group {:?}", group.id);
        let gravity = resolve_accel(group.gravity, [0.0, -9.8], &context)?;
        let wind = resolve_accel(group.wind, [0.0, 0.0], &context)?;
        let turbulence_amplitude = match group.turbulence_amplitude {
            Some(value) => {
                check_finite(value, &context)?;
                if value < 0.0 {
                    return Err(ParseError::FieldRange {
                        context: format!("{context} turbulence_amplitude"),
                    });
                }
                value
            }
            None => 0.0,
        };
        let mut inputs = Vec::with_capacity(group.inputs.len());
        for input in &group.inputs {
            let param = params.get(input.parameter.as_str()).ok_or_else(|| {
                ParseError::DanglingReference {
                    collection: "physics groups",
                    from_id: group.id.clone(),
                    reference: input.parameter.clone(),
                }
            })?;
            if !param.is_scalar() {
                return Err(ParseError::WrongKind {
                    context: format!("{context} input"),
                    id: input.parameter.clone(),
                });
            }
            check_finite(input.scale_degrees, &context)?;
            if !(-360.0..=360.0).contains(&input.scale_degrees) {
                return Err(ParseError::FieldRange {
                    context: format!("{context} input scale_degrees"),
                });
            }
            inputs.push(PhysicsInput {
                parameter: input.parameter.clone(),
                scale_degrees: input.scale_degrees,
            });
        }
        let mut strands = Vec::with_capacity(group.strands.len());
        for strand in &group.strands {
            strands.push(convert_strand(
                strand,
                deformer_ids,
                mesh_ids,
                params,
                &mut written,
            )?);
        }
        out.push(PhysicsGroup {
            id: group.id.clone(),
            gravity,
            wind,
            turbulence_amplitude,
            inputs,
            strands,
        });
    }
    Ok(out)
}

/// Resolve an optional acceleration vector against its default,
/// checking finiteness and the magnitude bound.
fn resolve_accel(
    value: Option<[f32; 2]>,
    default: [f32; 2],
    context: &str,
) -> Result<[f32; 2], ParseError> {
    let vector = value.unwrap_or(default);
    for component in vector {
        check_finite(component, context)?;
        if component.abs() > ACCEL_MAGNITUDE_MAX {
            return Err(ParseError::FieldRange {
                context: context.to_string(),
            });
        }
    }
    Ok(vector)
}

/// Convert one strand: anchor reference, particle ranges, and
/// the output one-writer rule.
fn convert_strand<'a>(
    strand: &'a RawStrand,
    deformer_ids: &HashSet<&str>,
    mesh_ids: &HashSet<&str>,
    params: &HashMap<&str, &Parameter>,
    written: &mut HashSet<&'a str>,
) -> Result<Strand, ParseError> {
    let context = format!("strand {:?}", strand.id);
    let (kind, anchor_set) = match strand.anchor.kind {
        RawAnchorKind::Deformer => (AnchorKind::Deformer, deformer_ids),
        RawAnchorKind::Mesh => (AnchorKind::Mesh, mesh_ids),
    };
    if !anchor_set.contains(strand.anchor.id.as_str()) {
        return Err(ParseError::DanglingReference {
            collection: "strands",
            from_id: strand.id.clone(),
            reference: strand.anchor.id.clone(),
        });
    }
    check_point(strand.anchor.offset, &context)?;
    if strand.particles.is_empty() {
        return Err(ParseError::FieldRange {
            context: format!("{context} has no particles"),
        });
    }
    if strand.particles.len() > PARTICLES_PER_STRAND_MAX {
        return Err(ParseError::CountExceeded {
            collection: "strand particles",
            count: strand.particles.len(),
            max: PARTICLES_PER_STRAND_MAX,
        });
    }
    let mut particles = Vec::with_capacity(strand.particles.len());
    for particle in &strand.particles {
        for value in [
            particle.length,
            particle.radius,
            particle.mobility,
            particle.delay,
            particle.damping,
        ] {
            check_finite(value, &context)?;
        }
        let in_range = particle.length > 0.0
            && particle.radius >= 0.0
            && (0.0..=1.0).contains(&particle.mobility)
            && (0.0..=1.0).contains(&particle.delay)
            && (0.0..1.0).contains(&particle.damping);
        if !in_range {
            return Err(ParseError::FieldRange {
                context: format!("{context} particle field out of range"),
            });
        }
        particles.push(Particle {
            length: particle.length,
            radius: particle.radius,
            mobility: particle.mobility,
            delay: particle.delay,
            damping: particle.damping,
        });
    }
    let output = match &strand.output {
        Some(raw_output) => {
            let param = params.get(raw_output.parameter.as_str()).ok_or_else(|| {
                ParseError::DanglingReference {
                    collection: "strands",
                    from_id: strand.id.clone(),
                    reference: raw_output.parameter.clone(),
                }
            })?;
            if !param.is_scalar() {
                return Err(ParseError::WrongKind {
                    context: format!("{context} output"),
                    id: raw_output.parameter.clone(),
                });
            }
            check_finite(raw_output.scale, &context)?;
            check_finite(raw_output.offset, &context)?;
            if !written.insert(raw_output.parameter.as_str()) {
                return Err(ParseError::PhysicsOutputConflict {
                    parameter: raw_output.parameter.clone(),
                });
            }
            Some(PhysicsOutput {
                parameter: raw_output.parameter.clone(),
                scale: raw_output.scale,
                offset: raw_output.offset,
            })
        }
        None => None,
    };
    Ok(Strand {
        id: strand.id.clone(),
        anchor: Anchor {
            kind,
            id: strand.anchor.id.clone(),
            offset: strand.anchor.offset,
        },
        particles,
        output,
    })
}
