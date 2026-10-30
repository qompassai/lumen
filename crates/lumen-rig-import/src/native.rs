//! Native motion and expression assets (SPEC sections 4.1-4.5),
//! plus the validation the converter performs on them.
//!
//! `lumen-rig-core` R1 parses only the model asset; its motion and
//! expression parsers land with R2. Until then the converter owns
//! these checks itself and applies the SPEC section 9 rules that
//! concern motions and expressions (LRF-202/303/501/502/602
//! classes), failing loudly exactly as a native validator would.
//! Emission follows the SPEC section 1.2 canonical discipline:
//! fields in spec order, 2-space indent, one trailing newline,
//! shortest-round-trip floats.

use std::collections::BTreeSet;

use lumen_rig_core::{Model, Parameter};
use serde::Serialize;

use crate::error::ConvertError;

/// SPEC section 11 motion bounds. `lumen-rig-core` R1 does not
/// parse motions, so its `limits` module omits these; the values
/// are the SPEC's, named here until the R2 parser lands.
pub const CURVES_PER_MOTION_MAX: usize = 512;
/// SPEC section 11: keyframes per curve bound.
pub const KEYFRAMES_PER_CURVE_MAX: usize = 4096;

/// Interpolation kinds of the native format (SPEC section 4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Interpolation {
    Linear,
    Stepped,
    Cubic,
}

impl Interpolation {
    /// The format string for this kind.
    pub fn as_str(&self) -> &'static str {
        match self {
            Interpolation::Linear => "linear",
            Interpolation::Stepped => "stepped",
            Interpolation::Cubic => "cubic",
        }
    }
}

/// One keyframe of a native motion curve.
#[derive(Debug, Clone)]
pub struct Keyframe {
    pub time: f32,
    pub value: f32,
    pub interpolation: Interpolation,
    pub tangent_in: Option<[f32; 2]>,
    pub tangent_out: Option<[f32; 2]>,
}

/// One curve of a native motion asset.
#[derive(Debug, Clone)]
pub struct MotionCurve {
    pub parameter: String,
    pub fade_in_seconds: f32,
    pub fade_out_seconds: f32,
    pub keyframes: Vec<Keyframe>,
}

/// A native motion asset (SPEC section 4).
#[derive(Debug, Clone)]
pub struct MotionAsset {
    pub id: String,
    pub model: String,
    pub duration_seconds: f32,
    pub loop_motion: bool,
    pub frames_per_second: u32,
    pub curves: Vec<MotionCurve>,
}

/// One entry of a native expression asset.
#[derive(Debug, Clone)]
pub struct ExpressionEntry {
    pub parameter: String,
    pub value: f32,
    /// One of `overwrite`, `add`, `multiply` (format strings).
    pub blend: &'static str,
}

/// A native expression asset (SPEC section 4.5).
#[derive(Debug, Clone)]
pub struct ExpressionAsset {
    pub id: String,
    pub model: String,
    pub fade_in_seconds: f32,
    pub fade_out_seconds: f32,
    pub entries: Vec<ExpressionEntry>,
}

// ---------------------------------------------------------------------
// Canonical emission (serde views in SPEC field order)
// ---------------------------------------------------------------------

#[derive(Serialize)]
struct MotionView<'a> {
    format: &'a str,
    format_version: u32,
    id: &'a str,
    model: &'a str,
    duration_seconds: f32,
    #[serde(rename = "loop")]
    loop_motion: bool,
    frames_per_second: u32,
    curves: Vec<CurveView<'a>>,
}

#[derive(Serialize)]
struct CurveView<'a> {
    parameter: &'a str,
    fade_in_seconds: f32,
    fade_out_seconds: f32,
    keyframes: Vec<KeyframeView>,
}

#[derive(Serialize)]
struct KeyframeView {
    time: f32,
    value: f32,
    interpolation: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    tangent_in: Option<[f32; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tangent_out: Option<[f32; 2]>,
}

#[derive(Serialize)]
struct ExpressionView<'a> {
    format: &'a str,
    format_version: u32,
    id: &'a str,
    model: &'a str,
    fade_in_seconds: f32,
    fade_out_seconds: f32,
    entries: Vec<EntryView<'a>>,
}

#[derive(Serialize)]
struct EntryView<'a> {
    parameter: &'a str,
    value: f32,
    blend: &'a str,
}

fn emit<T: Serialize>(view: &T) -> String {
    // Serializing plain data cannot fail; the expect documents a
    // derived-Serialize invariant, mirroring lumen-rig-core.
    let mut text =
        serde_json::to_string_pretty(view).expect("plain-data serialization is infallible");
    text.push('\n');
    text
}

impl MotionAsset {
    /// Serialize in the canonical emission form.
    pub fn to_canonical_json(&self) -> String {
        emit(&MotionView {
            format: "lumen-rig-motion",
            format_version: 1,
            id: &self.id,
            model: &self.model,
            duration_seconds: self.duration_seconds,
            loop_motion: self.loop_motion,
            frames_per_second: self.frames_per_second,
            curves: self
                .curves
                .iter()
                .map(|curve| CurveView {
                    parameter: &curve.parameter,
                    fade_in_seconds: curve.fade_in_seconds,
                    fade_out_seconds: curve.fade_out_seconds,
                    keyframes: curve
                        .keyframes
                        .iter()
                        .map(|keyframe| KeyframeView {
                            time: keyframe.time,
                            value: keyframe.value,
                            interpolation: keyframe.interpolation.as_str(),
                            tangent_in: keyframe.tangent_in,
                            tangent_out: keyframe.tangent_out,
                        })
                        .collect(),
                })
                .collect(),
        })
    }
}

impl ExpressionAsset {
    /// Serialize in the canonical emission form.
    pub fn to_canonical_json(&self) -> String {
        emit(&ExpressionView {
            format: "lumen-rig-expression",
            format_version: 1,
            id: &self.id,
            model: &self.model,
            fade_in_seconds: self.fade_in_seconds,
            fade_out_seconds: self.fade_out_seconds,
            entries: self
                .entries
                .iter()
                .map(|entry| EntryView {
                    parameter: &entry.parameter,
                    value: entry.value,
                    blend: entry.blend,
                })
                .collect(),
        })
    }
}

// ---------------------------------------------------------------------
// Validation (converter-side, pending the R2 native parsers)
// ---------------------------------------------------------------------

/// Parameter range lookup against the validated model.
fn parameter_range(model: &Model, id: &str) -> Option<(f32, f32, bool)> {
    model
        .parameters
        .iter()
        .find(|parameter| parameter.id() == id)
        .map(|parameter| match parameter {
            Parameter::Scalar(scalar) => (scalar.min, scalar.max, true),
            Parameter::Combined(combined) => (combined.min, combined.max, false),
        })
}

/// The set of parameters written by physics outputs (LRF-602:
/// motions may not curve them).
fn physics_output_parameters(model: &Model) -> BTreeSet<&str> {
    let mut out = BTreeSet::new();
    for group in &model.physics {
        for strand in &group.strands {
            if let Some(output) = &strand.output {
                out.insert(output.parameter.as_str());
            }
        }
    }
    out
}

fn motion_error(message: String) -> ConvertError {
    ConvertError::MotionValidation { message }
}

/// Validate a motion asset against the SPEC section 4 rules and
/// the validated target model.
pub fn validate_motion(motion: &MotionAsset, model: &Model) -> Result<(), ConvertError> {
    if motion.model != model.id {
        return Err(motion_error(format!(
            "motion {:?} targets model {:?}, not {:?}",
            motion.id, motion.model, model.id
        )));
    }
    if !motion.duration_seconds.is_finite() || motion.duration_seconds <= 0.0 {
        return Err(motion_error(format!(
            "motion {:?} duration {} is not positive and finite",
            motion.id, motion.duration_seconds
        )));
    }
    if !(1..=240).contains(&motion.frames_per_second) {
        return Err(motion_error(format!(
            "motion {:?} frames_per_second {} outside 1..=240",
            motion.id, motion.frames_per_second
        )));
    }
    if motion.curves.len() > CURVES_PER_MOTION_MAX {
        return Err(motion_error(format!(
            "motion {:?} has {} curves, max {}",
            motion.id,
            motion.curves.len(),
            CURVES_PER_MOTION_MAX
        )));
    }
    let physics_outputs = physics_output_parameters(model);
    let mut seen_parameters = BTreeSet::new();
    for curve in &motion.curves {
        if !seen_parameters.insert(curve.parameter.as_str()) {
            return Err(motion_error(format!(
                "motion {:?} has two curves for parameter {:?} (LRF-202)",
                motion.id, curve.parameter
            )));
        }
        let (min, max, _scalar) = parameter_range(model, &curve.parameter).ok_or_else(|| {
            motion_error(format!(
                "motion {:?} curves unknown parameter {:?} (LRF-203)",
                motion.id, curve.parameter
            ))
        })?;
        if physics_outputs.contains(curve.parameter.as_str()) {
            return Err(motion_error(format!(
                "motion {:?} curves physics-output parameter {:?} (LRF-602)",
                motion.id, curve.parameter
            )));
        }
        if curve.fade_in_seconds < 0.0 || !curve.fade_in_seconds.is_finite() {
            return Err(motion_error(format!(
                "motion {:?} curve {:?} fade_in_seconds invalid",
                motion.id, curve.parameter
            )));
        }
        if curve.fade_out_seconds < 0.0 || !curve.fade_out_seconds.is_finite() {
            return Err(motion_error(format!(
                "motion {:?} curve {:?} fade_out_seconds invalid",
                motion.id, curve.parameter
            )));
        }
        validate_keyframes(motion, curve, min, max)?;
    }
    Ok(())
}

/// Keyframe checks for one curve: LRF-501 (times), LRF-303
/// (values in range), LRF-502 (cubic tangents).
fn validate_keyframes(
    motion: &MotionAsset,
    curve: &MotionCurve,
    min: f32,
    max: f32,
) -> Result<(), ConvertError> {
    let keyframes = &curve.keyframes;
    if keyframes.is_empty() {
        return Err(motion_error(format!(
            "motion {:?} curve {:?} has no keyframes",
            motion.id, curve.parameter
        )));
    }
    if keyframes.len() > KEYFRAMES_PER_CURVE_MAX {
        return Err(motion_error(format!(
            "motion {:?} curve {:?} has {} keyframes, max {}",
            motion.id,
            curve.parameter,
            keyframes.len(),
            KEYFRAMES_PER_CURVE_MAX
        )));
    }
    for (index, keyframe) in keyframes.iter().enumerate() {
        if !keyframe.time.is_finite() || !keyframe.value.is_finite() {
            return Err(motion_error(format!(
                "motion {:?} curve {:?} keyframe {index} is not finite",
                motion.id, curve.parameter
            )));
        }
        if keyframe.time < 0.0 || keyframe.time > motion.duration_seconds {
            return Err(motion_error(format!(
                "motion {:?} curve {:?} keyframe {index} time {} outside [0, {}] (LRF-501)",
                motion.id, curve.parameter, keyframe.time, motion.duration_seconds
            )));
        }
        if index > 0 && keyframe.time <= keyframes[index - 1].time {
            return Err(motion_error(format!(
                "motion {:?} curve {:?} keyframe times not strictly increasing (LRF-501)",
                motion.id, curve.parameter
            )));
        }
        if keyframe.value < min || keyframe.value > max {
            return Err(motion_error(format!(
                "motion {:?} curve {:?} keyframe {index} value {} outside [{min}, {max}] (LRF-303)",
                motion.id, curve.parameter, keyframe.value
            )));
        }
        // Tangents: a tangent_out belongs to a cubic segment that
        // starts at this keyframe; a tangent_in belongs to a cubic
        // segment that ends here.
        if let Some([dt, _]) = keyframe.tangent_out {
            if keyframe.interpolation != Interpolation::Cubic {
                return Err(motion_error(format!(
                    "motion {:?} curve {:?} keyframe {index} carries tangent_out on a non-cubic key (LRF-502)",
                    motion.id, curve.parameter
                )));
            }
            if dt < 0.0 {
                return Err(motion_error(format!(
                    "motion {:?} curve {:?} keyframe {index} tangent_out dt negative (LRF-502)",
                    motion.id, curve.parameter
                )));
            }
            if let Some(next) = keyframes.get(index + 1)
                && keyframe.time + dt > next.time
            {
                return Err(motion_error(format!(
                    "motion {:?} curve {:?} keyframe {index} tangent_out overruns the next key (LRF-502)",
                    motion.id, curve.parameter
                )));
            }
        }
        if let Some([dt, _]) = keyframe.tangent_in {
            let previous_is_cubic =
                index > 0 && keyframes[index - 1].interpolation == Interpolation::Cubic;
            if !previous_is_cubic {
                return Err(motion_error(format!(
                    "motion {:?} curve {:?} keyframe {index} carries tangent_in without a preceding cubic segment (LRF-502)",
                    motion.id, curve.parameter
                )));
            }
            if dt < 0.0 || keyframe.time - dt < keyframes[index - 1].time {
                return Err(motion_error(format!(
                    "motion {:?} curve {:?} keyframe {index} tangent_in overruns the previous key (LRF-502)",
                    motion.id, curve.parameter
                )));
            }
        }
    }
    Ok(())
}

/// Validate an expression asset (SPEC section 4.5 rules).
pub fn validate_expression(
    expression: &ExpressionAsset,
    model: &Model,
) -> Result<(), ConvertError> {
    if expression.model != model.id {
        return Err(motion_error(format!(
            "expression {:?} targets model {:?}, not {:?}",
            expression.id, expression.model, model.id
        )));
    }
    for (field, value) in [
        ("fade_in_seconds", expression.fade_in_seconds),
        ("fade_out_seconds", expression.fade_out_seconds),
    ] {
        if !value.is_finite() || value < 0.0 {
            return Err(motion_error(format!(
                "expression {:?} {field} is invalid",
                expression.id
            )));
        }
    }
    let mut seen = BTreeSet::new();
    for entry in &expression.entries {
        if !seen.insert(entry.parameter.as_str()) {
            return Err(motion_error(format!(
                "expression {:?} assigns parameter {:?} twice (LRF-202)",
                expression.id, entry.parameter
            )));
        }
        let (min, max, scalar) = parameter_range(model, &entry.parameter).ok_or_else(|| {
            motion_error(format!(
                "expression {:?} assigns unknown parameter {:?} (LRF-203)",
                expression.id, entry.parameter
            ))
        })?;
        if !scalar {
            return Err(motion_error(format!(
                "expression {:?} assigns non-scalar parameter {:?} (LRF-207)",
                expression.id, entry.parameter
            )));
        }
        if !entry.value.is_finite() || entry.value < min || entry.value > max {
            return Err(motion_error(format!(
                "expression {:?} value {} for {:?} outside [{min}, {max}] (LRF-303)",
                expression.id, entry.value, entry.parameter
            )));
        }
    }
    Ok(())
}
