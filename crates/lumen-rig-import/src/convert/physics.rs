//! physics3.json -> native physics groups (SPEC section 3.6).
//!
//! The source model (wave-4 physics annex, from the official
//! Cubism documentation): a setting reads parameters through
//! per-type normalization windows (Position X or Angle), weighted
//! and optionally reflected; the summed input tilts a pendulum
//! chain; each output writes a parameter from the pendulum angle
//! (in radians) at one vertex of the chain, scaled.
//!
//! The native model differs in three structural ways, and the
//! report records each as an approximation rather than hiding it:
//! - Native physics inputs rotate gravity by
//!   `scale_degrees * normalized(parameter)` with the parameter
//!   normalized over its own [min, max]; the source normalizes
//!   through a default-centered window. Magnitudes at the
//!   parameter's maximum agree; the resting point differs.
//! - Source "Position X" inputs shift the pendulum's anchor; the
//!   native format has only angle inputs, so an X input is
//!   carried as an angle input over its position window.
//! - A native strand has exactly one output and reads the angle
//!   at its tip (the LRF-601 one-writer rule); a source setting
//!   has one chain with many outputs. The converter emits one
//!   strand per output, carrying the chain prefix up to that
//!   output's vertex — the chains share vertices in the source
//!   and are independent strands in the native asset, which
//!   evaluate identically until a host couples them.
//!
//! Per-vertex `Acceleration` and the editor-side pendulum tuning
//! (duration, ease, convergence) are not exported concepts in the
//! native format and are dropped with a recorded reason; the
//! native `damping` is written 0.0 (the source has no per-vertex
//! damping; SPEC section 5 evaluates (1 - damping) as the Verlet
//! retention factor).

use std::collections::BTreeMap;

use lumen_rig_core::{
    Anchor, AnchorKind, Particle, PhysicsGroup, PhysicsInput, PhysicsOutput, Strand,
};

use crate::cubism::{Physics3, PhysicsSetting};
use crate::error::ConvertError;
use crate::moc3::{Moc3File, axes_for};

use super::{ReportBuilder, check_native_id};

/// Cubism effective forces are unitless direction vectors
/// (gravity conventionally (0, -1)); native accelerations are in
/// model units per second squared with a (0, -9.8) default. The
/// factor maps the conventional source gravity onto the native
/// default exactly.
const FORCE_SCALE: f32 = 9.8;

/// Radians-to-degrees conversion for output scales: a source
/// output writes `angle_radians * scale`; a native output writes
/// `offset + scale * angle_degrees`.
const RADIANS_TO_DEGREES: f32 = 180.0 / std::f32::consts::PI;

/// Convert all settings of a physics3 document.
pub(crate) fn convert_physics(
    physics3: &Physics3,
    moc: &Moc3File,
    report: &mut ReportBuilder,
) -> Result<Vec<PhysicsGroup>, ConvertError> {
    let forces = &physics3.meta.effective_forces;
    report.approximated(
        "physics effective forces",
        format!(
            "source gravity ({}, {}) and wind ({}, {}) are unitless direction vectors; they were scaled by {FORCE_SCALE} into native accelerations (the conventional (0, -1) gravity lands exactly on the native default (0, -9.8))",
            forces.gravity.x, forces.gravity.y, forces.wind.x, forces.wind.y
        ),
    );
    report.dropped(
        "physics vertex acceleration",
        "per-vertex Acceleration is Cubism pendulum response tuning with no native field (SPEC section 3.6, annex item T-12); values were read and not carried, and native damping is written 0.0 for every particle".to_string(),
    );
    let mut groups = Vec::with_capacity(physics3.physics_settings.len());
    // One-writer rule (LRF-601) checked across all settings here,
    // with a message that names the collision in source terms.
    let mut writers: BTreeMap<String, String> = BTreeMap::new();
    for setting in &physics3.physics_settings {
        groups.push(convert_setting(
            setting,
            physics3,
            moc,
            report,
            &mut writers,
        )?);
    }
    Ok(groups)
}

/// Convert one physics setting into one native group.
fn convert_setting(
    setting: &PhysicsSetting,
    physics3: &Physics3,
    moc: &Moc3File,
    report: &mut ReportBuilder,
    writers: &mut BTreeMap<String, String>,
) -> Result<PhysicsGroup, ConvertError> {
    check_native_id(&setting.id, "physics setting id")?;
    let mut inputs = Vec::with_capacity(setting.input.len());
    let mut x_inputs = 0usize;
    for input in &setting.input {
        if input.source.target != "Parameter" {
            return Err(ConvertError::Mapping {
                message: format!(
                    "physics setting {:?} input targets {:?}, not a parameter",
                    setting.id, input.source.target
                ),
            });
        }
        if !moc
            .parameters
            .iter()
            .any(|parameter| parameter.id == input.source.id)
        {
            return Err(ConvertError::Mapping {
                message: format!(
                    "physics setting {:?} input names unknown parameter {:?}",
                    setting.id, input.source.id
                ),
            });
        }
        let window = match input.input_type.as_str() {
            "X" => {
                x_inputs += 1;
                &setting.normalization.position
            }
            "Angle" => &setting.normalization.angle,
            other => {
                return Err(ConvertError::Mapping {
                    message: format!(
                        "physics setting {:?} input has unknown type {other:?} (expected \"X\" or \"Angle\")",
                        setting.id
                    ),
                });
            }
        };
        let mut scale_degrees = window.maximum * (input.weight / 100.0);
        if input.reflect {
            scale_degrees = -scale_degrees;
        }
        inputs.push(PhysicsInput {
            parameter: input.source.id.clone(),
            scale_degrees,
        });
    }
    if !inputs.is_empty() {
        report.approximated(
            "physics input normalization",
            format!(
                "physics setting {:?}: {} input(s) ({} Position X) map source window-weighted inputs onto native scale_degrees; the source normalizes default-centered through its windows while the native input normalizes over the parameter's own [min, max], so magnitudes agree at the parameter maximum and the resting point differs",
                setting.id,
                inputs.len(),
                x_inputs
            ),
        );
    }

    let mut strands = Vec::with_capacity(setting.output.len());
    for (output_index, output) in setting.output.iter().enumerate() {
        if output.destination.target != "Parameter" {
            return Err(ConvertError::Mapping {
                message: format!(
                    "physics setting {:?} output targets {:?}, not a parameter",
                    setting.id, output.destination.target
                ),
            });
        }
        let parameter = &output.destination.id;
        if !moc
            .parameters
            .iter()
            .any(|candidate| &candidate.id == parameter)
        {
            return Err(ConvertError::Mapping {
                message: format!(
                    "physics setting {:?} output names unknown parameter {parameter:?}",
                    setting.id
                ),
            });
        }
        if let Some(first) = writers.get(parameter) {
            return Err(ConvertError::Mapping {
                message: format!(
                    "parameter {parameter:?} is written by physics settings {first:?} and {:?}; the native format enforces one writer per parameter (LRF-601) and the converter does not merge writers",
                    setting.id
                ),
            });
        }
        writers.insert(parameter.clone(), setting.id.clone());
        strands.push(convert_output_strand(
            setting,
            output_index,
            output,
            moc,
            report,
        )?);
    }

    let forces = &physics3.meta.effective_forces;
    Ok(PhysicsGroup {
        id: setting.id.clone(),
        gravity: [
            forces.gravity.x * FORCE_SCALE,
            forces.gravity.y * FORCE_SCALE,
        ],
        wind: [forces.wind.x * FORCE_SCALE, forces.wind.y * FORCE_SCALE],
        turbulence_amplitude: 0.0,
        inputs,
        strands,
    })
}

/// Convert one output into a strand carrying the chain prefix up
/// to the output's vertex.
fn convert_output_strand(
    setting: &PhysicsSetting,
    output_index: usize,
    output: &crate::cubism::PhysicsOutput,
    moc: &Moc3File,
    report: &mut ReportBuilder,
) -> Result<Strand, ConvertError> {
    let vertex_index = output.vertex_index as usize;
    if vertex_index == 0 {
        return Err(ConvertError::Mapping {
            message: format!(
                "physics setting {:?} output {} writes from the chain root (VertexIndex 0); a native strand needs at least one segment",
                setting.id, output_index
            ),
        });
    }
    if vertex_index >= setting.vertices.len() {
        return Err(ConvertError::Mapping {
            message: format!(
                "physics setting {:?} output {} VertexIndex {vertex_index} exceeds the {}-vertex chain",
                setting.id,
                output_index,
                setting.vertices.len()
            ),
        });
    }
    let mut particles = Vec::with_capacity(vertex_index);
    let mut field_repairs: Vec<String> = Vec::new();
    for segment in 0..vertex_index {
        let from = &setting.vertices[segment];
        let to = &setting.vertices[segment + 1];
        let dx = to.position.x - from.position.x;
        let dy = to.position.y - from.position.y;
        // Native particle fields are range-bounded (LRF-305):
        // mobility and delay in [0, 1], radius >= 0. Real
        // exports exceed them (observed: delay 1.3 in an SDK 5
        // model — the source delay is a time factor, not a
        // fraction). Out-of-range values are clamped to the
        // boundary and recorded; delay is carried-but-inert in
        // native v1 (SPEC section 12.2), so the repair costs no
        // behavior until a delay consumption law lands.
        let mobility = clamp_particle_field(
            to.mobility,
            0.0,
            1.0,
            "mobility",
            segment + 1,
            &mut field_repairs,
        );
        let delay =
            clamp_particle_field(to.delay, 0.0, 1.0, "delay", segment + 1, &mut field_repairs);
        let radius = clamp_particle_field(
            to.radius,
            0.0,
            f32::MAX,
            "radius",
            segment + 1,
            &mut field_repairs,
        );
        particles.push(Particle {
            length: (dx * dx + dy * dy).sqrt(),
            radius,
            mobility,
            delay,
            damping: 0.0,
        });
    }
    if !field_repairs.is_empty() {
        report.approximated(
            "physics particle field range repair",
            format!(
                "physics setting {:?} output {}: {} clamped to the native ranges (delay is inert in native v1, SPEC section 12.2, so no behavior changes today; the clamp is recorded for the day a delay law lands)",
                setting.id,
                output_index + 1,
                field_repairs.join(", ")
            ),
        );
    }
    let mut scale = output.scale * (output.weight / 100.0) * RADIANS_TO_DEGREES;
    if output.reflect {
        scale = -scale;
    }
    let anchor = resolve_anchor(setting, output, moc, report)?;
    let root = &setting.vertices[0];
    Ok(Strand {
        id: format!("{}_out_{}", setting.id, output_index + 1),
        anchor: Anchor {
            kind: anchor.0,
            id: anchor.1,
            offset: [root.position.x, root.position.y],
        },
        particles,
        output: Some(PhysicsOutput {
            parameter: output.destination.id.clone(),
            scale,
            offset: 0.0,
        }),
    })
}

/// Clamp one particle field to its native range, recording the
/// repair when the value actually changes.
fn clamp_particle_field(
    value: f32,
    minimum: f32,
    maximum: f32,
    field: &str,
    vertex: usize,
    repairs: &mut Vec<String>,
) -> f32 {
    let clamped = value.clamp(minimum, maximum);
    if clamped != value {
        repairs.push(format!("vertex {vertex} {field} {value} -> {clamped}"));
    }
    clamped
}

/// The native anchor for a physics strand: the first deformer
/// (in moc3 declaration order) whose keyform binding drives the
/// output parameter, else the first art mesh that does, with the
/// chain root's authored position as the offset. Documented as
/// an approximation in the build report: Cubism chains hang from
/// a point in model space, native strands hang from an object.
///
/// Fallback: an output parameter may drive nothing in the
/// sections this converter parses — on SDK 5 exports it likely
/// drives blend-shape keyforms, which live in tail sections that
/// are R5b scope. The strand then anchors to the model's first
/// deformer (or first mesh) and the report records the fallback;
/// the strand's output (a parameter write) is unaffected.
fn resolve_anchor(
    setting: &PhysicsSetting,
    output: &crate::cubism::PhysicsOutput,
    moc: &Moc3File,
    report: &mut ReportBuilder,
) -> Result<(AnchorKind, String), ConvertError> {
    let parameter_id = &output.destination.id;
    let parameter_index = moc
        .parameters
        .iter()
        .position(|parameter| &parameter.id == parameter_id)
        .expect("parameter existence checked by the caller");
    for deformer in &moc.deformers {
        let binding = deformer.binding;
        if binding >= 0 {
            let axes = axes_for(&moc.bindings, binding)?;
            if axes.iter().any(|axis| axis.parameter == parameter_index) {
                return Ok((AnchorKind::Deformer, deformer.id.clone()));
            }
        }
    }
    for mesh in &moc.meshes {
        if mesh.binding >= 0 {
            let axes = axes_for(&moc.bindings, mesh.binding)?;
            if axes.iter().any(|axis| axis.parameter == parameter_index) {
                return Ok((AnchorKind::Mesh, mesh.id.clone()));
            }
        }
    }
    let fallback = if let Some(deformer) = moc.deformers.first() {
        (AnchorKind::Deformer, deformer.id.clone())
    } else if let Some(mesh) = moc.meshes.first() {
        (AnchorKind::Mesh, mesh.id.clone())
    } else {
        return Err(ConvertError::Mapping {
            message: format!(
                "physics setting {:?} output parameter {parameter_id:?} drives no deformer or art mesh, and the model has no object to anchor the strand to",
                setting.id
            ),
        });
    };
    report.approximated(
        "physics anchor fallback",
        format!(
            "physics setting {:?} output parameter {parameter_id:?} drives no deformer or art mesh in the parsed sections (on newer exports it likely drives blend-shape keyforms in the tail sections, R5b scope); its strand was anchored to {:?} {:?} so the parameter write survives",
            setting.id, fallback.0, fallback.1
        ),
    );
    Ok(fallback)
}
