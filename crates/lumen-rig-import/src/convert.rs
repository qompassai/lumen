//! Conversion orchestration: license gate, file discovery, the
//! moc3 -> native model mapping, and the conversion report.
//!
//! Mapping principles (the loud-error law applied to conversion):
//! - Every source construct either maps faithfully, maps under a
//!   documented approximation recorded in the report, drops with
//!   a recorded reason, or fails the conversion with a typed
//!   error. Nothing is silently lost.
//! - The converted model is emitted through `lumen_rig_core`'s
//!   canonical form and re-parsed by its validator before any
//!   file is written; a mapping bug therefore surfaces as a loud
//!   native validation error, never as a bad asset on disk.

use std::path::{Path, PathBuf};

use lumen_rig_core::{
    BlendMode, Deformer, Lattice, Mesh, MeshForm, Model, Parameter, Part, PhysicsGroup,
    RotationDeformer, RotationForm, ScalarParameter, Texture, WarpDeformer, WarpForm, parse_model,
};
use serde::Serialize;

use crate::cubism::{Cdi3, Model3, Physics3};
use crate::error::{ConvertError, bounded_quote};
use crate::license::LicenseRecord;
use crate::moc3::{BindingAxis, DeformerKind, Moc3File, RawMesh, RawRotation, RawWarp, axes_for};
use crate::native::{ExpressionAsset, MotionAsset};

pub mod inp;
mod motion;
pub mod outbound;
mod physics;

/// Generator string stamped into converted model assets.
pub const GENERATOR: &str = "lumen-rig-import 0.1.0";

/// Maximum size of any single source file the converter reads.
const SOURCE_BYTES_MAX: usize = 64 * 1024 * 1024;

/// Cubism's default expression fade, in seconds, applied when an
/// exp3 document omits its fades.
const EXPRESSION_FADE_DEFAULT_SECONDS: f32 = 0.5;

/// Tolerance for repairing float-dust opacity overshoot at the
/// [0, 1] boundary during conversion (observed overshoot in real
/// exports: ~5e-5). Recorded as an approximation, never silent.
const OPACITY_DUST_EPSILON: f32 = 1e-4;

/// Inputs for one conversion run.
pub struct ConversionInputs {
    pub model_dir: PathBuf,
    pub license_path: PathBuf,
    pub out_dir: PathBuf,
}

/// What a successful conversion produced.
#[derive(Debug)]
pub struct ConversionOutcome {
    pub report: ConversionReport,
    pub model_id: String,
    pub files_written: Vec<PathBuf>,
}

/// One report line: a construct that was approximated or dropped.
#[derive(Debug, Clone, Serialize)]
pub struct ReportEntry {
    pub item: String,
    pub detail: String,
}

/// Source-file summary carried in the conversion report.
#[derive(Debug, Clone, Serialize)]
pub struct SourceSummary {
    pub moc3_file: String,
    pub moc3_version_byte: u8,
    pub moc3_version_label: String,
    pub canvas_pixels_per_unit: f32,
    pub canvas_origin: [f32; 2],
    pub canvas_size: [f32; 2],
    pub counts: CountsSummary,
    pub keys_total: usize,
    pub keys_referenced: usize,
    pub keyform_positions_total: usize,
    pub keyform_positions_referenced_end: usize,
}

/// The count info table, echoed into the report.
#[derive(Debug, Clone, Serialize)]
pub struct CountsSummary {
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
    pub keyform_bindings: usize,
    pub parameter_bindings: usize,
    pub keys: usize,
    pub uvs: usize,
    pub position_indices: usize,
    pub drawable_masks: usize,
    pub draw_order_groups: usize,
    pub glue: usize,
    pub glue_info: usize,
    pub glue_keyforms: usize,
}

/// The license record, echoed into the report (it records grants,
/// never secrets).
#[derive(Debug, Clone, Serialize)]
pub struct LicenseEcho {
    pub source: String,
    pub license_name: String,
    pub license_url: Option<String>,
    pub recorded_by: String,
    pub date: String,
}

/// Output tallies carried in the conversion report.
#[derive(Debug, Clone, Default, Serialize)]
pub struct ConvertedCounts {
    pub parameters: usize,
    pub parts: usize,
    pub textures: usize,
    pub deformers: usize,
    pub meshes: usize,
    pub physics_groups: usize,
    pub physics_strands: usize,
    pub motions: usize,
    pub motion_curves: usize,
    pub expressions: usize,
}

/// The conversion report, emitted as `conversion-report.json`.
#[derive(Debug, Clone, Serialize)]
pub struct ConversionReport {
    pub format: &'static str,
    pub format_version: u32,
    pub model_id: String,
    pub generator: &'static str,
    pub source: SourceSummary,
    pub license: LicenseEcho,
    pub outputs: Vec<String>,
    pub converted: ConvertedCounts,
    pub approximated: Vec<ReportEntry>,
    pub dropped: Vec<ReportEntry>,
    pub notes: Vec<String>,
}

impl ConversionReport {
    /// Serialize in canonical form (2-space indent, one trailing
    /// newline).
    pub fn to_canonical_json(&self) -> String {
        let mut text =
            serde_json::to_string_pretty(self).expect("plain-data serialization is infallible");
        text.push('\n');
        text
    }
}

/// Mutable report state threaded through the mapping.
#[derive(Default)]
pub(crate) struct ReportBuilder {
    pub approximated: Vec<ReportEntry>,
    pub dropped: Vec<ReportEntry>,
    pub notes: Vec<String>,
    pub converted: ConvertedCounts,
    pub keyform_positions_referenced_end: usize,
    /// Mesh base opacities repaired from float dust outside
    /// [0, 1] (see `convert_mesh`); summarized into one report
    /// entry when the model is assembled.
    pub opacity_repairs: usize,
    /// Meshes carrying the drawable culling flag (consumer
    /// render state); summarized into one report entry.
    pub culling_flags: usize,
    /// Meshes whose source enabled toggle is off; summarized
    /// into one report entry.
    pub disabled_meshes: usize,
}

impl ReportBuilder {
    pub(crate) fn approximated(&mut self, item: &str, detail: String) {
        self.approximated.push(ReportEntry {
            item: item.to_string(),
            detail,
        });
    }

    pub(crate) fn dropped(&mut self, item: &str, detail: String) {
        self.dropped.push(ReportEntry {
            item: item.to_string(),
            detail,
        });
    }
}

/// Run one conversion end to end.
pub fn convert(inputs: &ConversionInputs) -> Result<ConversionOutcome, ConvertError> {
    // 1. License gate first: nothing about the model is read
    //    before the gate passes.
    let license_text = read_text(&inputs.license_path).map_err(|error| match error {
        ConvertError::Io { message, .. } => ConvertError::LicenseUnreadable { message },
        other => other,
    })?;
    let license = LicenseRecord::parse(&license_text)?;
    license.check()?;

    // 2. Locate and parse the model3 manifest.
    let model3_path = locate_model3(&inputs.model_dir)?;
    let model3: Model3 = read_json(&model3_path)?;
    let model_dir = inputs.model_dir.clone();

    // 3. Parse and layout-verify the moc3.
    let moc_path = model_dir.join(&model3.file_references.moc);
    let moc_bytes = read_bytes(&moc_path)?;
    let moc = Moc3File::parse(&moc_bytes)?;
    let model_id = model_id_from_moc_name(&model3.file_references.moc)?;

    // 4. Optional display info (cross-checks only).
    let mut report = ReportBuilder::default();
    if let Some(display_info) = &model3.file_references.display_info {
        let cdi3: Cdi3 = read_json(&model_dir.join(display_info))?;
        cross_check_cdi3(&cdi3, &moc, &mut report);
    } else {
        report
            .notes
            .push("no cdi3 display-info file referenced".to_string());
    }

    // 5. Convert the model (physics included), then prove the
    //    result against the native validator before writing.
    let physics_groups = match &model3.file_references.physics {
        Some(physics_file) => {
            let physics3: Physics3 = read_json(&model_dir.join(physics_file))?;
            physics::convert_physics(&physics3, &moc, &mut report)?
        }
        None => Vec::new(),
    };
    let model = build_model(&model3, &moc, &model_id, physics_groups, &mut report)?;
    let canonical = model.to_canonical_json();
    let validated = parse_model(canonical.as_bytes())?;
    debug_assert_eq!(validated.id, model.id);

    // 6. Motions and expressions, validated against the model.
    //    Curves targeting physics-output parameters are dropped
    //    first (recorded, never silent): the native format
    //    forbids them (LRF-602), and in the source runtime the
    //    physics pass runs after motions and overwrites those
    //    parameters, so the curves are dead there too.
    let physics_outputs: std::collections::BTreeSet<&str> = validated
        .physics
        .iter()
        .flat_map(|group| &group.strands)
        .filter_map(|strand| {
            strand
                .output
                .as_ref()
                .map(|output| output.parameter.as_str())
        })
        .collect();
    let mut motions: Vec<MotionAsset> = Vec::new();
    for (group_name, refs) in &model3.file_references.motions {
        let _ = group_name; // group names are consumer labels; see the dropped entry below
        for motion_ref in refs {
            let motion3 = read_json(&model_dir.join(&motion_ref.file))?;
            let mut motion = motion::convert_motion(&motion3, motion_ref, &validated, &mut report)?;
            let dropped_curves =
                drop_physics_output_curves(&mut motion, &physics_outputs, &mut report);
            report.converted.motion_curves -= dropped_curves;
            crate::native::validate_motion(&motion, &validated)?;
            if motions
                .iter()
                .any(|existing: &MotionAsset| existing.id == motion.id)
            {
                return Err(ConvertError::Mapping {
                    message: format!("duplicate motion id {:?} across motion groups", motion.id),
                });
            }
            motions.push(motion);
        }
    }
    if !model3.file_references.motions.is_empty() {
        report.dropped(
            "motion group names",
            "model3 motion group names (e.g. Idle, TapBody) are consumer labels; native motions are flat assets addressed by id".to_string(),
        );
    }
    let mut expressions: Vec<ExpressionAsset> = Vec::new();
    let mut expressions_defaulted_fades = 0usize;
    for expression_ref in &model3.file_references.expressions {
        let exp3 = read_json(&model_dir.join(&expression_ref.file))?;
        let (mut expression, defaulted) =
            motion::convert_expression(&exp3, expression_ref, &validated)?;
        if defaulted {
            expressions_defaulted_fades += 1;
        }
        drop_out_of_range_expression_entries(&mut expression, &validated, &mut report);
        crate::native::validate_expression(&expression, &validated)?;
        expressions.push(expression);
    }
    if expressions_defaulted_fades > 0 {
        report.approximated(
            "expression fade defaults",
            format!(
                "{expressions_defaulted_fades} expression(s) omit FadeInTime/FadeOutTime; the Cubism default of {EXPRESSION_FADE_DEFAULT_SECONDS} s was written explicitly (the native default is 0.3 s)"
            ),
        );
    }
    report.converted.expressions = expressions.len();
    note_unreferenced_assets(&model_dir, &model3, &mut report);

    // 7. Inventory the manifest-level constructs with no native
    //    counterpart.
    inventory_model3_extras(&model3, &moc, &mut report);

    // 8. Assemble the report and write everything. Outputs are
    //    written only after every validation above has passed.
    let mut outputs = vec![format!("{model_id}.model.json")];
    for motion in &motions {
        outputs.push(format!("motions/{}.motion.json", motion.id));
    }
    for expression in &expressions {
        outputs.push(format!("expressions/{}.expression.json", expression.id));
    }
    outputs.push("conversion-report.json".to_string());
    let final_report = ConversionReport {
        format: "lumen-rig-conversion-report",
        format_version: 1,
        model_id: model_id.clone(),
        generator: GENERATOR,
        source: summarize_source(&model3, &moc, report.keyform_positions_referenced_end),
        license: LicenseEcho {
            source: license.source.clone(),
            license_name: license.license_name.clone(),
            license_url: license.license_url.clone(),
            recorded_by: license.recorded_by.clone(),
            date: license.date.clone(),
        },
        outputs: outputs.clone(),
        converted: report.converted,
        approximated: report.approximated,
        dropped: report.dropped,
        notes: report.notes,
    };
    let mut files_written = Vec::new();
    std::fs::create_dir_all(inputs.out_dir.join("motions")).map_err(io_error(&inputs.out_dir))?;
    std::fs::create_dir_all(inputs.out_dir.join("expressions"))
        .map_err(io_error(&inputs.out_dir))?;
    let model_path = inputs.out_dir.join(format!("{model_id}.model.json"));
    write_file(&model_path, &canonical)?;
    files_written.push(model_path);
    for motion in &motions {
        let path = inputs
            .out_dir
            .join("motions")
            .join(format!("{}.motion.json", motion.id));
        write_file(&path, &motion.to_canonical_json())?;
        files_written.push(path);
    }
    for expression in &expressions {
        let path = inputs
            .out_dir
            .join("expressions")
            .join(format!("{}.expression.json", expression.id));
        write_file(&path, &expression.to_canonical_json())?;
        files_written.push(path);
    }
    let report_path = inputs.out_dir.join("conversion-report.json");
    write_file(&report_path, &final_report.to_canonical_json())?;
    files_written.push(report_path);

    Ok(ConversionOutcome {
        report: final_report,
        model_id,
        files_written,
    })
}

/// Inventory motion/expression files present in the model
/// directory but not referenced by the model3 manifest. They are
/// not converted (the manifest is the model's table of contents)
/// and the report says so, so a model dir is fully accounted for.
fn note_unreferenced_assets(model_dir: &Path, model3: &Model3, report: &mut ReportBuilder) {
    let mut referenced: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for refs in model3.file_references.motions.values() {
        for motion_ref in refs {
            referenced.insert(motion_ref.file.replace('\\', "/"));
        }
    }
    for expression_ref in &model3.file_references.expressions {
        referenced.insert(expression_ref.file.replace('\\', "/"));
    }
    let mut found: Vec<String> = Vec::new();
    collect_asset_files(model_dir, model_dir, 0, &mut found);
    found.sort();
    let unreferenced: Vec<&String> = found
        .iter()
        .filter(|path| !referenced.contains(*path))
        .collect();
    if !unreferenced.is_empty() {
        report.notes.push(format!(
            "{} motion/expression file(s) exist in the model directory but are not referenced by the model3 manifest and were not converted: {}",
            unreferenced.len(),
            unreferenced
                .iter()
                .map(|path| path.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
}

/// Recursively collect `*.motion3.json` / `*.exp3.json` paths
/// relative to `root`, bounded in depth and count.
fn collect_asset_files(root: &Path, dir: &Path, depth: usize, out: &mut Vec<String>) {
    const MAX_DEPTH: usize = 4;
    const MAX_FILES: usize = 4096;
    if depth > MAX_DEPTH || out.len() >= MAX_FILES {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .collect();
    entries.sort();
    for path in entries {
        if path.is_dir() {
            collect_asset_files(root, &path, depth + 1, out);
        } else if let Some(name) = path.file_name().and_then(|name| name.to_str())
            && (name.ends_with(".motion3.json") || name.ends_with(".exp3.json"))
            && let Ok(relative) = path.strip_prefix(root)
        {
            out.push(relative.to_string_lossy().replace('\\', "/"));
        }
    }
}

// ---------------------------------------------------------------------
// File helpers
// ---------------------------------------------------------------------

fn io_error(path: &Path) -> impl Fn(std::io::Error) -> ConvertError + '_ {
    move |error| ConvertError::Io {
        path: path.to_path_buf(),
        message: error.to_string(),
    }
}

fn read_bytes(path: &Path) -> Result<Vec<u8>, ConvertError> {
    let bytes = std::fs::read(path).map_err(io_error(path))?;
    if bytes.len() > SOURCE_BYTES_MAX {
        return Err(ConvertError::Moc3 {
            message: format!(
                "{} is {} bytes, over the {} byte source cap",
                path.display(),
                bytes.len(),
                SOURCE_BYTES_MAX
            ),
        });
    }
    Ok(bytes)
}

fn read_text(path: &Path) -> Result<String, ConvertError> {
    let bytes = read_bytes(path)?;
    String::from_utf8(bytes).map_err(|_| ConvertError::Io {
        path: path.to_path_buf(),
        message: "file is not valid UTF-8".to_string(),
    })
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, ConvertError> {
    let text = read_text(path)?;
    serde_json::from_str(&text).map_err(|error| ConvertError::Json {
        file: path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| path.display().to_string()),
        message: error.to_string(),
    })
}

fn write_file(path: &Path, contents: &str) -> Result<(), ConvertError> {
    std::fs::write(path, contents).map_err(io_error(path))
}

/// Remove curves that target physics-output parameters from a
/// converted motion, recording each removal. Returns how many
/// curves were removed.
fn drop_physics_output_curves(
    motion: &mut MotionAsset,
    physics_outputs: &std::collections::BTreeSet<&str>,
    report: &mut ReportBuilder,
) -> usize {
    let before = motion.curves.len();
    let mut dropped: Vec<String> = Vec::new();
    motion.curves.retain(|curve| {
        if physics_outputs.contains(curve.parameter.as_str()) {
            dropped.push(curve.parameter.clone());
            false
        } else {
            true
        }
    });
    if !dropped.is_empty() {
        report.dropped(
            "motion curves on physics-output parameters",
            format!(
                "motion {:?}: curve(s) for {} target physics-output parameters; native motions may not curve them (LRF-602), and in the source runtime the physics pass evaluates after motions and overwrites those parameters, so the curves do not drive the model there either",
                motion.id,
                dropped.join(", ")
            ),
        );
    }
    before - motion.curves.len()
}

/// Remove expression entries whose value lies outside the
/// parameter's range. Cubism `Add` entries are deltas and
/// `Multiply` entries are factors, and both may legitimately lie
/// outside the parameter's range in the source; the native
/// format requires every entry value in range (SPEC section
/// 4.5, LRF-303), so out-of-range entries have no native
/// counterpart and drop with a recorded reason.
fn drop_out_of_range_expression_entries(
    expression: &mut ExpressionAsset,
    model: &Model,
    report: &mut ReportBuilder,
) {
    let mut dropped: Vec<String> = Vec::new();
    expression.entries.retain(|entry| {
        let range = model
            .parameters
            .iter()
            .find(|parameter| parameter.id() == entry.parameter)
            .map(|parameter| match parameter {
                Parameter::Scalar(scalar) => (scalar.min, scalar.max),
                Parameter::Combined(combined) => (combined.min, combined.max),
            });
        match range {
            Some((min, max)) if entry.value < min || entry.value > max => {
                dropped.push(format!(
                    "{} = {} (blend {}, range [{min}, {max}])",
                    entry.parameter, entry.value, entry.blend
                ));
                false
            }
            _ => true,
        }
    });
    if !dropped.is_empty() {
        report.dropped(
            "expression entries outside the native range rule",
            format!(
                "expression {:?}: entr(ies) {} lie outside their parameters' ranges; Cubism add/multiply entries are deltas/factors and may do so, but the native format requires entry values in range (LRF-303), so they are not carried",
                expression.id,
                dropped.join(", ")
            ),
        );
    }
}

/// Find the single `*.model3.json` manifest in the model dir.
fn locate_model3(model_dir: &Path) -> Result<PathBuf, ConvertError> {
    let entries = std::fs::read_dir(model_dir).map_err(io_error(model_dir))?;
    let mut found: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let entry = entry.map_err(io_error(model_dir))?;
        let name = entry.file_name();
        if name.to_string_lossy().ends_with(".model3.json") {
            found.push(entry.path());
        }
    }
    found.sort();
    match found.len() {
        1 => Ok(found.into_iter().next().expect("one element")),
        0 => Err(ConvertError::Cli {
            message: format!("no *.model3.json manifest in {}", model_dir.display()),
        }),
        _ => Err(ConvertError::Cli {
            message: format!(
                "multiple *.model3.json manifests in {}; ambiguous model dir",
                model_dir.display()
            ),
        }),
    }
}

/// The native model id is the moc file's stem (`Haru.moc3` ->
/// `Haru`), validated against the native id rule.
fn model_id_from_moc_name(moc_name: &str) -> Result<String, ConvertError> {
    let stem = Path::new(moc_name)
        .file_stem()
        .and_then(|stem| stem.to_str())
        .ok_or_else(|| ConvertError::Mapping {
            message: format!("moc file name {moc_name:?} has no usable stem"),
        })?;
    check_native_id(stem, "model id")?;
    Ok(stem.to_string())
}

/// Validate an id against the native rule (SPEC section 1.3):
/// 1..=64 bytes of `[A-Za-z0-9_-]`. Ids are never rewritten; an
/// unconvertible id is a loud error.
pub(crate) fn check_native_id(id: &str, what: &str) -> Result<(), ConvertError> {
    let ok = !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-');
    if !ok {
        return Err(ConvertError::Mapping {
            message: format!(
                "{what} {:?} violates the native id rule (1..=64 bytes of [A-Za-z0-9_-])",
                bounded_quote(id)
            ),
        });
    }
    Ok(())
}

// ---------------------------------------------------------------------
// cdi3 cross-checks (notes only; the moc3 is the authority)
// ---------------------------------------------------------------------

fn cross_check_cdi3(cdi3: &Cdi3, moc: &Moc3File, report: &mut ReportBuilder) {
    let moc_parameters: std::collections::BTreeSet<&str> = moc
        .parameters
        .iter()
        .map(|parameter| parameter.id.as_str())
        .collect();
    let cdi_parameters: std::collections::BTreeSet<&str> = cdi3
        .parameters
        .iter()
        .map(|parameter| parameter.id.as_str())
        .collect();
    for id in cdi_parameters.difference(&moc_parameters) {
        report.notes.push(format!(
            "cdi3 lists parameter {id:?} which the moc3 does not contain; ignored (moc3 is authoritative)"
        ));
    }
    for id in moc_parameters.difference(&cdi_parameters) {
        report.notes.push(format!(
            "moc3 parameter {id:?} has no cdi3 entry; its display name is unknown"
        ));
    }
    let moc_parts: std::collections::BTreeSet<&str> =
        moc.parts.iter().map(|part| part.id.as_str()).collect();
    let cdi_parts: std::collections::BTreeSet<&str> =
        cdi3.parts.iter().map(|part| part.id.as_str()).collect();
    for id in cdi_parts.difference(&moc_parts) {
        report.dropped(
            "cdi3-only part",
            format!(
                "cdi3 lists part {id:?} with no moc3 counterpart (display-info only, no geometry); not carried"
            ),
        );
    }
    report.notes.push(
        "cdi3 display names and parameter groups are editor UI data; the native format carries ids only, so names were read for cross-checks and not carried".to_string(),
    );
}

// ---------------------------------------------------------------------
// Manifest-level inventory of unconvertible constructs
// ---------------------------------------------------------------------

fn inventory_model3_extras(model3: &Model3, moc: &Moc3File, report: &mut ReportBuilder) {
    let references = &model3.file_references;
    if !model3.hit_areas.is_empty() {
        report.dropped(
            "hit areas",
            format!(
                "{} hit area(s) declared in model3; hit areas are consumer-layer data, not animation data (SPEC section 12.4)",
                model3.hit_areas.len()
            ),
        );
    }
    if references.pose.is_some() {
        report.dropped(
            "pose file",
            "pose3.json part-visibility poses are stage R5b scope, not converted in R5a"
                .to_string(),
        );
    }
    if references.user_data.is_some() {
        report.dropped(
            "user data file",
            "userdata3.json art-mesh user data is consumer-layer data (SPEC section 12.4 class), not converted".to_string(),
        );
    }
    if !model3.groups.is_empty() {
        report.dropped(
            "model3 groups",
            format!(
                "{} group(s) (EyeBlink/LipSync style parameter and part groupings) are runtime hints for consumers, not asset data; not carried",
                model3.groups.len()
            ),
        );
    }
    for key in references.extra.keys() {
        report.dropped(
            "unknown file reference",
            format!("model3 FileReferences key {key:?} is not a known reference kind; its target was not read"),
        );
    }
    if moc.counts.glue > 0 {
        report.dropped(
            "glue",
            format!(
                "{} glue binding(s) ({} glue info entries, {} glue keyforms) join art meshes at shared vertices; the native format omits glue in v1 (SPEC section 12.1)",
                moc.counts.glue, moc.counts.glue_info, moc.counts.glue_keyforms
            ),
        );
    }
    if moc.counts.part_keyforms > 0 {
        report.dropped(
            "part keyform draw orders",
            format!(
                "{} part keyform(s) carry per-pose part draw orders; native parts have no draw order (meshes carry the total order)",
                moc.counts.part_keyforms
            ),
        );
    }
    let repeat_count = moc
        .parameters
        .iter()
        .filter(|parameter| parameter.is_repeat)
        .count();
    if repeat_count > 0 {
        report.dropped(
            "parameter repeat flags",
            format!(
                "{repeat_count} parameter(s) carry the Cubism repeat (wrap-around) flag; the native format has no repeat semantics"
            ),
        );
    }
    if moc.keys_referenced_end < moc.counts.keys {
        report.notes.push(format!(
            "the keys array carries {} value(s) beyond the last referenced binding key ({} of {} referenced); newer exporters store additional key sets there (the SDK 5 blend-shape key storage is the suspected tenant); unreferenced data is not carried",
            moc.counts.keys - moc.keys_referenced_end,
            moc.keys_referenced_end,
            moc.counts.keys
        ));
    }
    report.notes.push(format!(
        "canvas: {}x{} px at {} px/unit, origin ({}, {}); model coordinates are carried verbatim from the moc3 (no re-origin, no rescale)",
        moc.canvas.width, moc.canvas.height, moc.canvas.pixels_per_unit, moc.canvas.origin_x, moc.canvas.origin_y
    ));
}

fn summarize_source(
    model3: &Model3,
    moc: &Moc3File,
    keyform_positions_referenced_end: usize,
) -> SourceSummary {
    let counts = &moc.counts;
    SourceSummary {
        moc3_file: model3.file_references.moc.clone(),
        moc3_version_byte: moc.version,
        moc3_version_label: version_label(moc.version).to_string(),
        canvas_pixels_per_unit: moc.canvas.pixels_per_unit,
        canvas_origin: [moc.canvas.origin_x, moc.canvas.origin_y],
        canvas_size: [moc.canvas.width, moc.canvas.height],
        counts: CountsSummary {
            parts: counts.parts,
            deformers: counts.deformers,
            warp_deformers: counts.warp_deformers,
            rotation_deformers: counts.rotation_deformers,
            art_meshes: counts.art_meshes,
            parameters: counts.parameters,
            part_keyforms: counts.part_keyforms,
            warp_keyforms: counts.warp_keyforms,
            rotation_keyforms: counts.rotation_keyforms,
            art_mesh_keyforms: counts.art_mesh_keyforms,
            keyform_positions: counts.keyform_positions,
            keyform_bindings: counts.keyform_bindings,
            parameter_bindings: counts.parameter_bindings,
            keys: counts.keys,
            uvs: counts.uvs,
            position_indices: counts.position_indices,
            drawable_masks: counts.drawable_masks,
            draw_order_groups: counts.draw_order_groups,
            glue: counts.glue,
            glue_info: counts.glue_info,
            glue_keyforms: counts.glue_keyforms,
        },
        keys_total: counts.keys,
        keys_referenced: moc.keys_referenced_end,
        keyform_positions_total: counts.keyform_positions,
        keyform_positions_referenced_end,
    }
}

/// Marketing names for the moc3 version bytes, where known.
fn version_label(version: u8) -> &'static str {
    match version {
        1 => "V3_00_00",
        2 => "V3_03_00",
        3 => "V4_00_00",
        4 => "V4_02_00",
        5 => "V5_00_00",
        _ => "post-V5 (unnamed in the community spec)",
    }
}

// ---------------------------------------------------------------------
// The model mapping
// ---------------------------------------------------------------------

/// Factor a stored keyform ordinal into per-axis key indices.
/// Stored order varies the first-listed axis fastest (verified
/// against real models; see the moc3 module docs).
pub(crate) fn form_at(axes: &[BindingAxis], mut ordinal: usize) -> Vec<u32> {
    let mut at = Vec::with_capacity(axes.len());
    for axis in axes {
        let count = axis.keys.len();
        at.push((ordinal % count) as u32);
        ordinal /= count;
    }
    at
}

/// The ordinal of the default (rest) form: the form whose axis
/// keys equal the parameters' default values. Fails loudly when
/// no axis key equals a default — Cubism authors defaults on the
/// key grid, so a miss means the file is not what it claims.
pub(crate) fn default_form_ordinal(
    axes: &[BindingAxis],
    moc: &Moc3File,
    what: &str,
) -> Result<usize, ConvertError> {
    let mut ordinal = 0usize;
    // Horner from the last axis: ordinal = i0 + n0*(i1 + n1*(...))
    for axis in axes.iter().rev() {
        let default = moc.parameters[axis.parameter].default;
        let index = axis
            .keys
            .iter()
            .position(|key| *key == default)
            .ok_or_else(|| ConvertError::Mapping {
                message: format!(
                    "{what}: parameter {:?} default {default} is not one of its binding keys {:?}",
                    moc.parameters[axis.parameter].id, axis.keys
                ),
            })?;
        ordinal = ordinal * axis.keys.len() + index;
    }
    Ok(ordinal)
}

/// A lattice form payload: the native form types all carry
/// their lattice coordinate in an `at` field.
pub(crate) trait LatticeForm {
    /// The form's lattice coordinate.
    fn at(&self) -> &[u32];
}

impl LatticeForm for MeshForm {
    fn at(&self) -> &[u32] {
        &self.at
    }
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

/// Build a native lattice from forms whose payloads already
/// carry their `at` coordinates, sorted into the canonical
/// ascending `at` order the native validator requires. Returns
/// `None` for an unbound object (no axes, a single rest form).
pub(crate) fn build_lattice<F: LatticeForm>(
    axes: &[BindingAxis],
    moc: &Moc3File,
    mut forms: Vec<F>,
) -> Option<Lattice<F>> {
    if axes.is_empty() {
        return None;
    }
    forms.sort_by(|left, right| left.at().cmp(right.at()));
    Some(Lattice {
        axes: axes
            .iter()
            .map(|axis| moc.parameters[axis.parameter].id.clone())
            .collect(),
        keys: axes.iter().map(|axis| axis.keys.clone()).collect(),
        forms,
    })
}

/// A lattice construction plan: which axes survive as native
/// lattice axes (the native format allows at most two), which
/// stored forms feed the lattice, and which axes were baked.
pub(crate) struct LatticePlan {
    /// The axes the native lattice keeps (at most two).
    pub kept_axes: Vec<BindingAxis>,
    /// `(at, stored ordinal)` pairs in kept-axis stored order.
    pub entries: Vec<(Vec<u32>, usize)>,
}

/// Plan a lattice for an object whose binding may exceed the
/// native two-axis limit. Axes beyond the first two are baked at
/// the key nearest the parameter default (Cubism authors defaults
/// on the key grid, so the baked key is the default key itself);
/// that is an approximation and is recorded in the report.
pub(crate) fn plan_lattice(
    axes: &[BindingAxis],
    moc: &Moc3File,
    what: &str,
    report: &mut ReportBuilder,
) -> Result<LatticePlan, ConvertError> {
    let kept_count = axes.len().min(2);
    let kept_axes: Vec<BindingAxis> = axes[..kept_count].to_vec();
    let mut baked_indices: Vec<usize> = Vec::new();
    let mut baked: Vec<(String, f32)> = Vec::new();
    for axis in &axes[kept_count..] {
        let default = moc.parameters[axis.parameter].default;
        let index = axis
            .keys
            .iter()
            .position(|key| *key == default)
            .ok_or_else(|| ConvertError::Mapping {
                message: format!(
                    "{what}: parameter {:?} default {default} is not one of its binding keys {:?}",
                    moc.parameters[axis.parameter].id, axis.keys
                ),
            })?;
        baked_indices.push(index);
        baked.push((moc.parameters[axis.parameter].id.clone(), axis.keys[index]));
    }
    if !baked.is_empty() {
        report.approximated(
            "keyform binding over the native axis limit",
            format!(
                "{what} is bound to {} parameters; the native lattice allows two axes, so {} were baked at their default keys ({}) and the object no longer responds to them",
                axes.len(),
                baked.len(),
                baked
                    .iter()
                    .map(|(id, value)| format!("{id} = {value}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        );
    }
    // Strides in stored (first-axis-fastest) order over ALL axes.
    let mut strides: Vec<usize> = Vec::with_capacity(axes.len());
    let mut stride = 1usize;
    for axis in axes {
        strides.push(stride);
        stride *= axis.keys.len();
    }
    let mut entries = Vec::new();
    let kept_total: usize = kept_axes.iter().map(|axis| axis.keys.len()).product();
    for kept_ordinal in 0..kept_total {
        let at = form_at(&kept_axes, kept_ordinal);
        let mut full_ordinal = 0usize;
        for (position, index) in at.iter().enumerate() {
            full_ordinal += (*index as usize) * strides[position];
        }
        for (position, index) in baked_indices.iter().enumerate() {
            full_ordinal += index * strides[kept_count + position];
        }
        entries.push((at, full_ordinal));
    }
    Ok(LatticePlan { kept_axes, entries })
}

/// Read one keyform's positions (vertex_count points) from the
/// global keyform positions array.
pub(crate) fn positions_at(
    moc: &Moc3File,
    begin: i32,
    vertex_count: i32,
    report: &mut ReportBuilder,
) -> Result<Vec<[f32; 2]>, ConvertError> {
    let begin = crate::moc3::checked_index(begin, "keyform position begin")?;
    let count = crate::moc3::checked_index(vertex_count, "vertex count")?;
    let end = begin
        .checked_add(count * 2)
        .ok_or_else(|| ConvertError::Mapping {
            message: "keyform position range overflows".to_string(),
        })?;
    report.keyform_positions_referenced_end = report.keyform_positions_referenced_end.max(end);
    let slice = moc
        .keyform_positions
        .get(begin..end)
        .ok_or_else(|| ConvertError::Mapping {
            message: "keyform position range out of bounds".to_string(),
        })?;
    Ok(slice
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| [pair[0], pair[1]])
        .collect())
}

/// Assemble the native model.
fn build_model(
    model3: &Model3,
    moc: &Moc3File,
    model_id: &str,
    physics: Vec<PhysicsGroup>,
    report: &mut ReportBuilder,
) -> Result<Model, ConvertError> {
    let parameters: Vec<Parameter> = moc
        .parameters
        .iter()
        .map(|parameter| {
            check_native_id(&parameter.id, "parameter id")?;
            Ok(Parameter::Scalar(ScalarParameter {
                id: parameter.id.clone(),
                min: parameter.min,
                max: parameter.max,
                default: parameter.default,
            }))
        })
        .collect::<Result<_, ConvertError>>()?;

    let parts: Vec<Part> = moc
        .parts
        .iter()
        .map(|part| {
            check_native_id(&part.id, "part id")?;
            Ok(Part {
                id: part.id.clone(),
                parent: if part.parent >= 0 {
                    Some(moc.parts[part.parent as usize].id.clone())
                } else {
                    None
                },
                opacity: 1.0,
                translation_keyforms: None,
            })
        })
        .collect::<Result<_, ConvertError>>()?;

    let textures: Vec<Texture> = model3
        .file_references
        .textures
        .iter()
        .enumerate()
        .map(|(index, source)| Texture {
            id: format!("texture_{index:02}"),
            source: source.clone(),
        })
        .collect();

    let mut deformers = Vec::with_capacity(moc.deformers.len());
    let mut deformer_part_links = 0usize;
    let mut deformer_keyform_opacity_losses = 0usize;
    for raw in &moc.deformers {
        check_native_id(&raw.id, "deformer id")?;
        if raw.parent_part >= 0 {
            deformer_part_links += 1;
        }
        let parent = if raw.parent_deformer >= 0 {
            Some(moc.deformers[raw.parent_deformer as usize].id.clone())
        } else {
            None
        };
        let deformer = match raw.kind {
            DeformerKind::Warp(index) => {
                let warp = &moc.warps[index];
                deformer_keyform_opacity_losses += count_opacity_losses(&warp.keyform_opacities);
                Deformer::Warp(convert_warp(warp, &raw.id, moc, parent, report)?)
            }
            DeformerKind::Rotation(index) => {
                let rotation = &moc.rotations[index];
                deformer_keyform_opacity_losses += rotation
                    .keyforms
                    .iter()
                    .filter(|keyform| keyform.opacity != 1.0)
                    .count();
                Deformer::Rotation(convert_rotation(rotation, &raw.id, moc, parent, report)?)
            }
        };
        deformers.push(deformer);
    }
    if deformer_part_links > 0 {
        report.dropped(
            "deformer part membership",
            format!(
                "{deformer_part_links} deformer(s) name a parent part in the moc3; native deformers form their own forest and carry no part link (part membership lives on meshes)"
            ),
        );
    }
    if deformer_keyform_opacity_losses > 0 {
        report.dropped(
            "deformer keyform opacities",
            format!(
                "{deformer_keyform_opacity_losses} deformer keyform(s) carry opacity != 1.0; native deformers have no opacity (opacity lives on parts and meshes)"
            ),
        );
    }

    let mut meshes = Vec::with_capacity(moc.meshes.len());
    let mut empty_meshes: Vec<String> = Vec::new();
    for raw in &moc.meshes {
        // A mesh with no indices (or no vertices) renders no
        // triangles in the source either, and the native format
        // cannot represent it (LRF-402 requires >= 3 indices).
        // Observed in real exports as a repeated placeholder
        // pattern; dropped with a recorded reason. If the
        // version-4+ offscreen tables (R5b scope) reference such
        // a mesh as a mask source, that use is lost with it.
        if raw.position_index_count == 0 || raw.vertex_count == 0 {
            empty_meshes.push(raw.id.clone());
            continue;
        }
        meshes.push(convert_mesh(raw, model3, moc, report)?);
    }
    if !empty_meshes.is_empty() {
        report.dropped(
            "zero-geometry art meshes",
            format!(
                "{} art mesh(es) ({}) have no position indices and render no triangles in the source; the native format requires at least 3 indices per mesh (LRF-402), so they are not carried",
                empty_meshes.len(),
                empty_meshes.join(", ")
            ),
        );
    }
    if report.opacity_repairs > 0 {
        report.approximated(
            "opacity float-dust repair",
            format!(
                "{} art mesh(es) carry a default-form opacity within {OPACITY_DUST_EPSILON} outside [0, 1] (float dust in the source export, e.g. 1.00005); the value was repaired to the boundary — converters are where source repair belongs (SPEC section 10), and the repair is recorded here rather than applied silently",
                report.opacity_repairs
            ),
        );
    }
    if report.culling_flags > 0 {
        report.dropped(
            "art mesh culling flags",
            format!(
                "{} art mesh(es) enable backface culling; culling is consumer render state, not asset data",
                report.culling_flags
            ),
        );
    }
    if report.disabled_meshes > 0 {
        report.dropped(
            "art mesh enabled toggles",
            format!(
                "{} art mesh(es) are disabled in the source (an editor interaction toggle); they convert with rendered taken from their visibility toggles",
                report.disabled_meshes
            ),
        );
    }

    report.converted.parameters = parameters.len();
    report.converted.parts = parts.len();
    report.converted.textures = textures.len();
    report.converted.deformers = deformers.len();
    report.converted.meshes = meshes.len();
    report.converted.physics_groups = physics.len();
    report.converted.physics_strands = physics.iter().map(|group| group.strands.len()).sum();

    Ok(Model {
        id: model_id.to_string(),
        generator: Some(GENERATOR.to_string()),
        parameters,
        parts,
        textures,
        deformers,
        meshes,
        physics,
    })
}

fn count_opacity_losses(opacities: &[f32]) -> usize {
    opacities.iter().filter(|opacity| **opacity != 1.0).count()
}

/// Convert one warp deformer.
fn convert_warp(
    warp: &RawWarp,
    id: &str,
    moc: &Moc3File,
    parent: Option<String>,
    report: &mut ReportBuilder,
) -> Result<WarpDeformer, ConvertError> {
    let axes = axes_for(&moc.bindings, warp.binding)?;
    let default_ordinal = default_form_ordinal(axes, moc, "warp deformer")?;
    let mut payloads = Vec::with_capacity(warp.keyform_position_begins.len());
    for begin in &warp.keyform_position_begins {
        payloads.push(positions_at(moc, *begin, warp.vertex_count, report)?);
    }
    let rest_positions = payloads[default_ordinal].clone();
    let plan = plan_lattice(axes, moc, &format!("warp deformer {id:?}"), report)?;
    let forms: Vec<WarpForm> = plan
        .entries
        .iter()
        .map(|(at, ordinal)| WarpForm {
            at: at.clone(),
            positions: payloads[*ordinal].clone(),
        })
        .collect();
    let keyforms = build_lattice(&plan.kept_axes, moc, forms);
    Ok(WarpDeformer {
        id: id.to_string(),
        parent,
        columns: warp.columns as u32,
        rows: warp.rows as u32,
        rest_positions,
        keyforms,
    })
}

/// Convert one rotation deformer.
fn convert_rotation(
    rotation: &RawRotation,
    id: &str,
    moc: &Moc3File,
    parent: Option<String>,
    _report: &mut ReportBuilder,
) -> Result<RotationDeformer, ConvertError> {
    let axes = axes_for(&moc.bindings, rotation.binding)?;
    for keyform in &rotation.keyforms {
        if keyform.reflect_x || keyform.reflect_y {
            return Err(ConvertError::Mapping {
                message: format!(
                    "rotation deformer {id:?} has a mirrored keyform (reflect x={} y={}); native rotation scales must be positive, so the mirror is unrepresentable",
                    keyform.reflect_x, keyform.reflect_y
                ),
            });
        }
    }
    let default_ordinal = default_form_ordinal(axes, moc, "rotation deformer")?;
    let default_form = &rotation.keyforms[default_ordinal];
    let plan = plan_lattice(axes, moc, &format!("rotation deformer {id:?}"), _report)?;
    let forms: Vec<RotationForm> = plan
        .entries
        .iter()
        .map(|(at, ordinal)| {
            let keyform = &rotation.keyforms[*ordinal];
            RotationForm {
                at: at.clone(),
                angle_degrees: keyform.angle,
                scale: [keyform.scale, keyform.scale],
            }
        })
        .collect();
    let keyforms = build_lattice(&plan.kept_axes, moc, forms);
    Ok(RotationDeformer {
        id: id.to_string(),
        parent,
        origin: default_form.origin,
        base_angle_degrees: rotation.base_angle,
        keyforms,
    })
}

/// Convert one art mesh.
fn convert_mesh(
    raw: &RawMesh,
    model3: &Model3,
    moc: &Moc3File,
    report: &mut ReportBuilder,
) -> Result<Mesh, ConvertError> {
    check_native_id(&raw.id, "art mesh id")?;
    let axes = axes_for(&moc.bindings, raw.binding)?;
    let default_ordinal = default_form_ordinal(axes, moc, "art mesh")?;
    let vertex_count = crate::moc3::checked_index(raw.vertex_count, "art mesh vertex count")?;

    let mut payloads = Vec::with_capacity(raw.keyform_position_begins.len());
    for begin in &raw.keyform_position_begins {
        payloads.push(positions_at(moc, *begin, raw.vertex_count, report)?);
    }
    let vertices = payloads[default_ordinal].clone();
    let plan = plan_lattice(axes, moc, &format!("art mesh {:?}", raw.id), report)?;
    let forms: Vec<MeshForm> = plan
        .entries
        .iter()
        .map(|(at, ordinal)| MeshForm {
            at: at.clone(),
            vertices: payloads[*ordinal].clone(),
        })
        .collect();
    let keyforms = build_lattice(&plan.kept_axes, moc, forms);

    // Keyform opacity / draw-order variation has no native
    // counterpart (native mesh keyforms carry vertices only).
    // The base opacity may carry float dust just outside [0, 1]
    // (observed in real exports: 1.00005). Within a hair of the
    // boundary it is repaired to the boundary and counted for
    // the report's approximation entry; genuinely out-of-range
    // opacity is a loud error.
    let raw_opacity = raw.keyform_opacities[default_ordinal];
    let base_opacity = if (0.0..=1.0).contains(&raw_opacity) {
        raw_opacity
    } else if raw_opacity > 1.0 && raw_opacity <= 1.0 + OPACITY_DUST_EPSILON {
        report.opacity_repairs += 1;
        1.0
    } else if (-OPACITY_DUST_EPSILON..0.0).contains(&raw_opacity) {
        report.opacity_repairs += 1;
        0.0
    } else {
        return Err(ConvertError::Mapping {
            message: format!(
                "art mesh {:?} default-form opacity {raw_opacity} is outside [0, 1] by more than float dust",
                raw.id
            ),
        });
    };
    let base_draw_order = raw.keyform_draw_orders[default_ordinal];
    let varies = raw
        .keyform_opacities
        .iter()
        .zip(&raw.keyform_draw_orders)
        .any(|(opacity, draw_order)| *opacity != base_opacity || *draw_order != base_draw_order);
    if varies {
        report.dropped(
            "art mesh keyform opacity/draw order",
            format!(
                "art mesh {:?} varies opacity or draw order across its keyforms; native mesh keyforms carry vertices only, so the default form's opacity ({base_opacity}) and draw order ({base_draw_order}) became the mesh's static values",
                raw.id
            ),
        );
    }
    if !raw.is_enabled {
        // An editor interaction toggle; counted here and
        // summarized into one report entry. The mesh converts
        // with `rendered` taken from its visibility toggle.
        report.disabled_meshes += 1;
    }

    let uv_begin = crate::moc3::checked_index(raw.uv_begin, "art mesh uv begin")?;
    let uv_end = uv_begin + vertex_count * 2;
    let uvs: Vec<[f32; 2]> = moc
        .uvs
        .get(uv_begin..uv_end)
        .ok_or_else(|| ConvertError::Mapping {
            message: format!("art mesh {:?} uv range out of bounds", raw.id),
        })?
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| [pair[0], pair[1]])
        .collect();

    let index_begin = crate::moc3::checked_index(raw.position_index_begin, "position index begin")?;
    let index_count = crate::moc3::checked_index(raw.position_index_count, "position index count")?;
    let indices: Vec<u32> = moc
        .position_indices
        .get(index_begin..index_begin + index_count)
        .ok_or_else(|| ConvertError::Mapping {
            message: format!("art mesh {:?} position index range out of bounds", raw.id),
        })?
        .iter()
        // Cubism stores indices as i16 bit patterns of u16 values.
        .map(|index| (*index as u16) as u32)
        .collect();

    let mask_begin = crate::moc3::checked_index(raw.mask_begin, "mask begin")?;
    let mask_count = crate::moc3::checked_index(raw.mask_count, "mask count")?;
    let mut clipping_mask_ids: Vec<String> = moc
        .drawable_masks
        .get(mask_begin..mask_begin + mask_count)
        .ok_or_else(|| ConvertError::Mapping {
            message: format!("art mesh {:?} mask range out of bounds", raw.id),
        })?
        .iter()
        .map(|mask| moc.meshes[*mask as usize].id.clone())
        .collect();
    // Inverted clipping (v4+ flag 0x8) has no native counterpart:
    // the mask list converts as dropped and the mesh renders
    // unclipped. Recorded as an approximation per mesh — the
    // alternative (failing the whole model) would refuse models
    // over one mesh's mask polarity.
    if raw.drawable_flag & 0x08 != 0 && !clipping_mask_ids.is_empty() {
        report.approximated(
            "inverted clipping mask",
            format!(
                "art mesh {:?} inverts its clipping mask ({} mask(s)); the native format has no inverted masks, so the mask list was dropped and the mesh renders unclipped",
                raw.id,
                clipping_mask_ids.len()
            ),
        );
        clipping_mask_ids.clear();
    }

    let blend_mode = convert_drawable_flag(raw, !clipping_mask_ids.is_empty(), report)?;
    let draw_order = convert_draw_order(raw, base_draw_order, moc, report)?;
    let texture_index = crate::moc3::checked_index(raw.texture_number, "texture number")?;
    if texture_index >= model3.file_references.textures.len() {
        return Err(ConvertError::Mapping {
            message: format!(
                "art mesh {:?} names texture {texture_index} but model3 declares {} texture(s)",
                raw.id,
                model3.file_references.textures.len()
            ),
        });
    }

    Ok(Mesh {
        id: raw.id.clone(),
        part: moc.parts[raw.parent_part as usize].id.clone(),
        texture: format!("texture_{texture_index:02}"),
        deformer: if raw.parent_deformer >= 0 {
            Some(moc.deformers[raw.parent_deformer as usize].id.clone())
        } else {
            None
        },
        vertices,
        uvs,
        indices,
        opacity: base_opacity,
        blend_mode,
        draw_order,
        rendered: raw.is_visible,
        clipping_mask_ids,
        keyforms,
    })
}

/// The drawable flag byte: blend bits 0x1 (additive) / 0x2
/// (multiply), culling bit 0x4, inverted-mask bit 0x8 (v4+).
fn convert_drawable_flag(
    raw: &RawMesh,
    has_masks: bool,
    report: &mut ReportBuilder,
) -> Result<BlendMode, ConvertError> {
    let flag = raw.drawable_flag;
    if flag & !0x0F != 0 {
        return Err(ConvertError::Mapping {
            message: format!(
                "art mesh {:?} drawable flag {flag:#04x} has bits outside the known 0x0F mask",
                raw.id
            ),
        });
    }
    if flag & 0x04 != 0 {
        // Consumer render state; counted here and summarized
        // into one report entry when the model is assembled.
        report.culling_flags += 1;
    }
    if flag & 0x08 != 0 && !has_masks {
        report.dropped(
            "art mesh inverted-mask flag without masks",
            format!(
                "art mesh {:?} sets the inverted-clipping flag but has no clipping masks; the flag has no effect and is not carried",
                raw.id
            ),
        );
    }
    match flag & 0x03 {
        0 => Ok(BlendMode::Normal),
        1 => Ok(BlendMode::LinearDodge),
        2 => Ok(BlendMode::Multiply),
        _ => Err(ConvertError::Mapping {
            message: format!(
                "art mesh {:?} drawable flag {flag:#04x} sets both blend bits",
                raw.id
            ),
        }),
    }
}

/// Base draw order: the default form's keyform draw order. With
/// more than one draw order group, the value is remapped from
/// the Cubism 0..=1000 range into the mesh's group range, which
/// is an approximation of the runtime's group evaluation.
fn convert_draw_order(
    raw: &RawMesh,
    base_draw_order: f32,
    moc: &Moc3File,
    report: &mut ReportBuilder,
) -> Result<i32, ConvertError> {
    if moc.draw_order_groups.len() <= 1 {
        return Ok(base_draw_order.round() as i32);
    }
    let mesh_index = moc
        .meshes
        .iter()
        .position(|mesh| mesh.id == raw.id)
        .expect("mesh comes from the same table");
    for group in &moc.draw_order_groups {
        if group
            .objects
            .iter()
            .any(|(kind, index)| *kind == 0 && *index as usize == mesh_index)
        {
            let span = (group.maximum - group.minimum) as f32;
            let mapped = group.minimum as f32 + (base_draw_order / 1000.0) * span;
            report.approximated(
                "draw order group remap",
                format!(
                    "art mesh {:?}: the model has {} draw order groups; its keyform draw order {base_draw_order} was remapped into its group's [{}, {}] range as {} (the native total order cannot express group-relative evaluation)",
                    raw.id,
                    moc.draw_order_groups.len(),
                    group.minimum,
                    group.maximum,
                    mapped.round() as i32
                ),
            );
            return Ok(mapped.round() as i32);
        }
    }
    Err(ConvertError::Mapping {
        message: format!(
            "art mesh {:?} belongs to no draw order group in a {}-group model",
            raw.id,
            moc.draw_order_groups.len()
        ),
    })
}
