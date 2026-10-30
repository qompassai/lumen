//! motion3.json -> native motion assets, exp3.json -> native
//! expression assets (SPEC sections 4.1-4.5).
//!
//! The Cubism segment stream decodes as: an initial point, then
//! per segment a type tag — 0 linear, 1 bezier (two control
//! points and an end point), 2 stepped, 3 inverse-stepped. A
//! bezier becomes a native cubic segment: the start key's
//! `tangent_out` is the first control point relative to the
//! start, the end key's `tangent_in` is the end point relative
//! to the second control point.
//!
//! Inverse-stepped segments (the value jumps to the segment's end
//! value at its start) have no native equivalent: a native key is
//! single-valued, so the jump cannot be represented at the shared
//! instant. They convert as stepped segments and are recorded as
//! approximated, per curve.
//!
//! Curves targeting `Model` (opacity) or `PartOpacity` have no
//! native curve target — native motions drive parameters only —
//! and drop with a recorded reason.

use std::path::Path;

use lumen_rig_core::Model;

use crate::cubism::{Exp3, ExpressionRef, Motion3, MotionCurve as CubismCurve, MotionRef};
use crate::error::ConvertError;
use crate::native::{
    ExpressionAsset, ExpressionEntry, Interpolation, Keyframe, MotionAsset, MotionCurve,
};

use super::{ReportBuilder, check_native_id};

/// Tolerance for the exporter's two-decimal Meta.Duration
/// rounding, in seconds.
const DURATION_ROUNDING_TOLERANCE_SECONDS: f32 = 0.01;

/// Convert one motion3 document. `motion_ref` is the model3
/// manifest entry that referenced the file (its fades are the
/// fallback when a curve omits its own).
pub(crate) fn convert_motion(
    motion3: &Motion3,
    motion_ref: &MotionRef,
    model: &Model,
    report: &mut ReportBuilder,
) -> Result<MotionAsset, ConvertError> {
    let id = motion_id_from_file(&motion_ref.file)?;
    let fps = motion3.meta.fps;
    if fps.fract() != 0.0 || !(1.0..=240.0).contains(&fps) {
        return Err(ConvertError::MotionValidation {
            message: format!(
                "motion {id:?} declares fps {fps}; the native format requires an integer in 1..=240"
            ),
        });
    }
    let mut curves = Vec::new();
    let mut used_ref_fades = false;
    let mut non_parameter_curves: Vec<String> = Vec::new();
    for curve in &motion3.curves {
        match curve.target.as_str() {
            "Parameter" => {
                let fade_in = curve.fade_in_time.or(motion_ref.fade_in_time);
                let fade_out = curve.fade_out_time.or(motion_ref.fade_out_time);
                if curve.fade_in_time.is_none() && motion_ref.fade_in_time.is_some() {
                    used_ref_fades = true;
                }
                if curve.fade_out_time.is_none() && motion_ref.fade_out_time.is_some() {
                    used_ref_fades = true;
                }
                curves.push(convert_curve(
                    curve,
                    &id,
                    fade_in.unwrap_or(0.0),
                    fade_out.unwrap_or(0.0),
                    report,
                )?);
            }
            "Model" | "PartOpacity" => {
                non_parameter_curves.push(format!("{} {:?}", curve.target, curve.id));
            }
            other => {
                return Err(ConvertError::MotionValidation {
                    message: format!("motion {id:?} curve has unknown target {other:?}"),
                });
            }
        }
    }
    if !non_parameter_curves.is_empty() {
        report.dropped(
            "non-parameter motion curves",
            format!(
                "motion {id:?}: {} curve(s) target Model opacity or part opacity ({}); native motions drive parameters only, so they are not carried",
                non_parameter_curves.len(),
                non_parameter_curves.join(", ")
            ),
        );
    }
    if used_ref_fades {
        report.notes.push(format!(
            "motion {id:?}: model3 manifest fades were applied to curves that omit their own FadeInTime/FadeOutTime"
        ));
    }
    // The exporter's Meta.Duration is rounded to two decimals
    // (observed: 5.33 for a motion whose last keyframe is at
    // 5.333). A native motion's keyframes must lie inside its
    // duration (LRF-501), so a duration that undershoots the
    // last keyframe by no more than the rounding tolerance is
    // repaired to the last keyframe time and recorded; a larger
    // overshoot is inconsistent data and fails loudly.
    let mut duration = motion3.meta.duration;
    let last_keyframe_time = curves
        .iter()
        .flat_map(|curve| curve.keyframes.iter().map(|keyframe| keyframe.time))
        .fold(0.0f32, f32::max);
    if last_keyframe_time > duration {
        let overshoot = last_keyframe_time - duration;
        if overshoot <= DURATION_ROUNDING_TOLERANCE_SECONDS {
            report.approximated(
                "motion duration rounding",
                format!(
                    "motion {id:?}: Meta.Duration {duration} undershoots the last keyframe at {last_keyframe_time} by {overshoot:.4} s (exporter two-decimal rounding); the native duration was set to the last keyframe time"
                ),
            );
            duration = last_keyframe_time;
        } else {
            return Err(ConvertError::MotionValidation {
                message: format!(
                    "motion {id:?} has a keyframe at {last_keyframe_time} but Meta.Duration is {duration} (overshoot {overshoot:.4} s exceeds the rounding tolerance)"
                ),
            });
        }
    }
    if !motion3.user_data.is_empty() {
        report.dropped(
            "motion user data",
            format!(
                "motion {id:?} carries {} user-data event(s); user data is consumer-layer data, not animation data",
                motion3.user_data.len()
            ),
        );
    }
    report.converted.motions += 1;
    report.converted.motion_curves += curves.len();
    Ok(MotionAsset {
        id,
        model: model.id.clone(),
        duration_seconds: duration,
        loop_motion: motion3.meta.loop_motion,
        frames_per_second: fps as u32,
        curves,
    })
}

/// The native motion id is the motion file's stem:
/// `motions/idle.motion3.json` -> `idle`.
fn motion_id_from_file(file: &str) -> Result<String, ConvertError> {
    let name = Path::new(file)
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| ConvertError::Mapping {
            message: format!("motion file path {file:?} has no file name"),
        })?;
    let stem = name
        .strip_suffix(".motion3.json")
        .or_else(|| name.strip_suffix(".json"))
        .unwrap_or(name);
    check_native_id(stem, "motion id")?;
    Ok(stem.to_string())
}

/// Decode one Cubism curve's segment stream into native
/// keyframes.
fn convert_curve(
    curve: &CubismCurve,
    motion_id: &str,
    fade_in_seconds: f32,
    fade_out_seconds: f32,
    report: &mut ReportBuilder,
) -> Result<MotionCurve, ConvertError> {
    let segments = &curve.segments;
    let invalid = |message: String| ConvertError::MotionValidation { message };
    if segments.len() < 2 {
        return Err(invalid(format!(
            "motion {motion_id:?} curve {:?} has {} segment value(s); a curve needs at least one point",
            curve.id,
            segments.len()
        )));
    }
    let mut keyframes: Vec<Keyframe> = vec![Keyframe {
        time: segments[0],
        value: segments[1],
        interpolation: Interpolation::Linear,
        tangent_in: None,
        tangent_out: None,
    }];
    let mut index = 2usize;
    let mut inverse_stepped_segments = 0usize;
    while index < segments.len() {
        let tag = segments[index];
        index += 1;
        if tag.fract() != 0.0 || !(0.0..=3.0).contains(&tag) {
            return Err(invalid(format!(
                "motion {motion_id:?} curve {:?} has segment type {tag}; expected an integer 0..=3",
                curve.id
            )));
        }
        let take = |index: usize, count: usize| -> Result<&[f32], ConvertError> {
            segments.get(index..index + count).ok_or_else(|| {
                invalid(format!(
                    "motion {motion_id:?} curve {:?} segment stream is truncated",
                    curve.id
                ))
            })
        };
        match tag as i32 {
            0 | 2 | 3 => {
                let point = take(index, 2)?;
                index += 2;
                let last = keyframes.last_mut().expect("one keyframe exists");
                match tag as i32 {
                    0 => last.interpolation = Interpolation::Linear,
                    2 => last.interpolation = Interpolation::Stepped,
                    _ => {
                        // Inverse-stepped: see the module docs.
                        last.interpolation = Interpolation::Stepped;
                        inverse_stepped_segments += 1;
                    }
                }
                keyframes.push(Keyframe {
                    time: point[0],
                    value: point[1],
                    interpolation: Interpolation::Linear,
                    tangent_in: None,
                    tangent_out: None,
                });
            }
            1 => {
                let points = take(index, 6)?;
                index += 6;
                let (c1, c2) = ([points[0], points[1]], [points[2], points[3]]);
                let (end_time, end_value) = (points[4], points[5]);
                let last = keyframes.last_mut().expect("one keyframe exists");
                last.interpolation = Interpolation::Cubic;
                last.tangent_out = Some([c1[0] - last.time, c1[1] - last.value]);
                keyframes.push(Keyframe {
                    time: end_time,
                    value: end_value,
                    interpolation: Interpolation::Linear,
                    tangent_in: Some([end_time - c2[0], end_value - c2[1]]),
                    tangent_out: None,
                });
            }
            _ => unreachable!("tag range checked above"),
        }
    }
    if inverse_stepped_segments > 0 {
        report.approximated(
            "inverse-stepped motion segments",
            format!(
                "motion {motion_id:?} curve {:?} has {inverse_stepped_segments} inverse-stepped segment(s); the native format has no inverse-stepped interpolation, so they convert as stepped (the value jump lands at the segment end instead of its start)",
                curve.id
            ),
        );
    }
    Ok(MotionCurve {
        parameter: curve.id.clone(),
        fade_in_seconds,
        fade_out_seconds,
        keyframes,
    })
}

/// Convert one exp3 document. Returns the asset and whether the
/// Cubism default fades had to be applied.
pub(crate) fn convert_expression(
    exp3: &Exp3,
    expression_ref: &ExpressionRef,
    model: &Model,
) -> Result<(ExpressionAsset, bool), ConvertError> {
    check_native_id(&expression_ref.name, "expression id")?;
    let defaulted = exp3.fade_in_time.is_none() || exp3.fade_out_time.is_none();
    let mut entries = Vec::with_capacity(exp3.parameters.len());
    for parameter in &exp3.parameters {
        let blend = match parameter.blend.as_deref() {
            None | Some("Overwrite") => "overwrite",
            Some("Add") => "add",
            Some("Multiply") => "multiply",
            Some(other) => {
                return Err(ConvertError::MotionValidation {
                    message: format!(
                        "expression {:?} parameter {:?} has unknown blend {other:?}",
                        expression_ref.name, parameter.id
                    ),
                });
            }
        };
        entries.push(ExpressionEntry {
            parameter: parameter.id.clone(),
            value: parameter.value,
            blend,
        });
    }
    Ok((
        ExpressionAsset {
            id: expression_ref.name.clone(),
            model: model.id.clone(),
            fade_in_seconds: exp3
                .fade_in_time
                .unwrap_or(super::EXPRESSION_FADE_DEFAULT_SECONDS),
            fade_out_seconds: exp3
                .fade_out_time
                .unwrap_or(super::EXPRESSION_FADE_DEFAULT_SECONDS),
            entries,
        },
        defaulted,
    ))
}
