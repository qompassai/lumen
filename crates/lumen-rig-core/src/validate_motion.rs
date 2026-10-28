//! The validating motion parser: bytes plus a validated
//! `Model` in, validated `Motion` out.
//!
//! Checks run in the order of SPEC.md section 8, adapted to the
//! motion asset, and stop at the first failure, so the reported
//! error is deterministic. The loader does not trust the
//! emitter: every rule the torogao-side bridge validates on
//! emission (per the R2 handoff) is re-checked here, plus the
//! model binding (`LRF-203`) and the one-writer rule
//! (`LRF-602`) that only a loader holding the model can check.
//! Nothing is repaired, dropped, or clamped — invalid input is
//! a typed `ParseError`, per the loud-error law.

use std::collections::{HashMap, HashSet};

use crate::error::{ParseError, bounded_quote};
use crate::limits::*;
use crate::model::{FORMAT_VERSION, Model, Parameter};
use crate::motion::*;
use crate::raw::*;
use crate::validate::{check_finite, check_id, map_serde_error, probe_header};

/// Parse and validate a motion asset against its target model.
///
/// Contract: `bytes` is the whole file, at most
/// `INPUT_BYTES_MAX`; `model` is the validated model the motion
/// will be applied to. The result is either a fully validated
/// `Motion` or the first `ParseError` in section-8 order. A
/// motion whose `model` field does not name `model.id` is
/// refused with `LRF-203` — motions bind by model id, never by
/// file proximity.
pub fn parse_motion(bytes: &[u8], model: &Model) -> Result<Motion, ParseError> {
    if bytes.len() > INPUT_BYTES_MAX {
        return Err(ParseError::InputTooLarge {
            bytes: bytes.len(),
            max: INPUT_BYTES_MAX,
        });
    }
    probe_header(bytes, "lumen-rig-motion")?;
    let raw: RawMotion = serde_json::from_slice(bytes).map_err(map_serde_error)?;
    validate_motion(raw, model)
}

/// The schema the fathom binding must name (SPEC.md
/// section 4.4): fathom's vocabulary, used verbatim.
const FATHOM_SCHEMA: &str = "fathom-motion-record/3";

/// Validate a raw motion into a `Motion`, against `model`.
/// Called by `parse_motion`; also usable directly on an
/// already-parsed raw document.
pub fn validate_motion(raw: RawMotion, model: &Model) -> Result<Motion, ParseError> {
    // The probe (parse_motion) reports header errors first;
    // this re-check keeps validate_motion sound when called
    // directly.
    if raw.format != "lumen-rig-motion" {
        return Err(ParseError::UnsupportedFormat {
            found: bounded_quote(&raw.format),
        });
    }
    if raw.format_version != FORMAT_VERSION {
        return Err(ParseError::UnsupportedVersion {
            found: raw.format_version,
        });
    }
    check_motion_counts(&raw)?;
    check_id(&raw.id)?;
    check_id(&raw.model)?;
    if raw.model != model.id {
        return Err(ParseError::DanglingReference {
            collection: "motions",
            from_id: raw.id.clone(),
            reference: raw.model.clone(),
        });
    }
    let duration_seconds = check_duration(raw.duration_seconds)?;
    if !(1..=240).contains(&raw.frames_per_second) {
        return Err(ParseError::FieldRange {
            context: format!("motion {:?} frames_per_second", raw.id),
        });
    }

    let params: HashMap<&str, &Parameter> = model
        .parameters
        .iter()
        .map(|param| (param.id(), param))
        .collect();
    let physics_outputs: HashSet<&str> = model
        .physics
        .iter()
        .flat_map(|group| group.strands.iter())
        .filter_map(|strand| strand.output.as_ref())
        .map(|output| output.parameter.as_str())
        .collect();
    let mut targeted: HashSet<&str> = HashSet::new();

    let mut curves = Vec::with_capacity(raw.curves.len());
    for raw_curve in &raw.curves {
        curves.push(convert_curve_head(
            &raw.id,
            &raw_curve.parameter,
            raw_curve.fade_in_seconds,
            raw_curve.fade_out_seconds,
            &params,
            &physics_outputs,
            &mut targeted,
        )?);
        let keyframes = convert_keyframes(
            &raw.id,
            &raw_curve.parameter,
            &raw_curve.keyframes,
            duration_seconds,
            &params,
        )?;
        curves.last_mut().expect("curve just pushed").keyframes = keyframes;
    }

    let mut baked_curves = Vec::new();
    if let Some(audio) = &raw.audio {
        for raw_set in &audio.baked {
            let mut curve = convert_curve_head(
                &raw.id,
                &raw_set.parameter,
                raw_set.fade_in_seconds,
                raw_set.fade_out_seconds,
                &params,
                &physics_outputs,
                &mut targeted,
            )?;
            curve.keyframes = convert_keyframes(
                &raw.id,
                &raw_set.parameter,
                &raw_set.keyframes,
                duration_seconds,
                &params,
            )?;
            let provenance = convert_provenance(&raw.id, &raw_set.provenance)?;
            baked_curves.push(BakedCurveSet {
                parameter: curve.parameter,
                fade_in_seconds: curve.fade_in_seconds,
                fade_out_seconds: curve.fade_out_seconds,
                keyframes: curve.keyframes,
                provenance,
            });
        }
    }

    let fathom = match &raw.fathom {
        Some(raw_fathom) => Some(convert_fathom(&raw.id, raw_fathom)?),
        None => None,
    };

    Ok(Motion {
        id: raw.id,
        model: raw.model,
        duration_seconds,
        looping: raw.loop_.unwrap_or(false),
        frames_per_second: raw.frames_per_second,
        curves,
        baked_curves,
        fathom,
    })
}

/// Section-8 step 4, motion collections: curves per motion and
/// keyframes per curve (SPEC.md section 11).
fn check_motion_counts(raw: &RawMotion) -> Result<(), ParseError> {
    if raw.curves.len() > CURVES_PER_MOTION_MAX {
        return Err(ParseError::CountExceeded {
            collection: "motion curves",
            count: raw.curves.len(),
            max: CURVES_PER_MOTION_MAX,
        });
    }
    let baked_count = raw.audio.as_ref().map_or(0, |audio| audio.baked.len());
    if baked_count > CURVES_PER_MOTION_MAX {
        return Err(ParseError::CountExceeded {
            collection: "motion baked curves",
            count: baked_count,
            max: CURVES_PER_MOTION_MAX,
        });
    }
    for curve in &raw.curves {
        check_keyframe_count(curve.keyframes.len())?;
    }
    if let Some(audio) = &raw.audio {
        for set in &audio.baked {
            check_keyframe_count(set.keyframes.len())?;
        }
    }
    Ok(())
}

fn check_keyframe_count(count: usize) -> Result<(), ParseError> {
    if count > KEYFRAMES_PER_CURVE_MAX {
        return Err(ParseError::CountExceeded {
            collection: "curve keyframes",
            count,
            max: KEYFRAMES_PER_CURVE_MAX,
        });
    }
    Ok(())
}

/// Duration is read as f64 in the raw layer so an f32 overflow
/// reports as the value error it is (`LRF-301`); the duration
/// itself must be positive (`LRF-305` class: a field-specific
/// range rule, like particle length).
fn check_duration(duration_seconds: f64) -> Result<f32, ParseError> {
    if !duration_seconds.is_finite() || duration_seconds > f64::from(f32::MAX) {
        return Err(ParseError::NonFinite {
            context: "motion duration_seconds".to_string(),
        });
    }
    let duration = duration_seconds as f32;
    if !duration.is_finite() {
        return Err(ParseError::NonFinite {
            context: "motion duration_seconds".to_string(),
        });
    }
    if duration <= 0.0 {
        return Err(ParseError::FieldRange {
            context: "motion duration_seconds must be > 0".to_string(),
        });
    }
    Ok(duration)
}

/// The declared `[min, max]` of a parameter, whichever kind it
/// is. Curves may target scalar or combined parameters
/// (SPEC.md section 4.1); both carry a declared range.
fn parameter_range(param: &Parameter) -> (f32, f32) {
    match param {
        Parameter::Scalar(scalar) => (scalar.min, scalar.max),
        Parameter::Combined(combined) => (combined.min, combined.max),
    }
}

/// Validate everything about a curve except its keyframes:
/// the target exists (`LRF-203`), is not a physics output
/// (`LRF-602`), is targeted at most once across the whole
/// motion (`LRF-202`), and the fades resolve. Returns the curve
/// with an empty keyframe list for the caller to fill.
fn convert_curve_head<'model, 'raw>(
    motion_id: &str,
    parameter: &'raw str,
    fade_in_seconds: Option<f32>,
    fade_out_seconds: Option<f32>,
    params: &HashMap<&'model str, &Parameter>,
    physics_outputs: &HashSet<&'model str>,
    targeted: &mut HashSet<&'raw str>,
) -> Result<Curve, ParseError> {
    if !params.contains_key(parameter) {
        return Err(ParseError::DanglingReference {
            collection: "motions",
            from_id: motion_id.to_string(),
            reference: parameter.to_string(),
        });
    }
    if physics_outputs.contains(parameter) {
        return Err(ParseError::PhysicsOutputTarget {
            motion_id: motion_id.to_string(),
            parameter: parameter.to_string(),
        });
    }
    if !targeted.insert(parameter) {
        return Err(ParseError::DuplicateId {
            collection: "motion curves",
            id: parameter.to_string(),
        });
    }
    Ok(Curve {
        parameter: parameter.to_string(),
        fade_in_seconds: convert_fade(motion_id, parameter, "fade_in_seconds", fade_in_seconds)?,
        fade_out_seconds: convert_fade(motion_id, parameter, "fade_out_seconds", fade_out_seconds)?,
        keyframes: Vec::new(),
    })
}

/// Resolve one fade field: absent means 0 (the spec's named
/// default); a stated fade must be finite and non-negative
/// (its declared range, `LRF-303`).
fn convert_fade(
    motion_id: &str,
    parameter: &str,
    field: &str,
    value: Option<f32>,
) -> Result<f32, ParseError> {
    let value = value.unwrap_or(0.0);
    check_finite(
        value,
        &format!("motion {motion_id:?} curve {parameter:?} {field}"),
    )?;
    if value < 0.0 {
        return Err(ParseError::OutOfRange {
            context: format!("motion {motion_id:?} curve {parameter:?} {field}"),
        });
    }
    Ok(value)
}

/// Validate one curve's keyframes: values in the parameter's
/// range (`LRF-303`), interpolation modes known, tangents only
/// on cubic keys and inside their segment (`LRF-502`), and
/// times strictly increasing inside `[0, duration]`
/// (`LRF-501`).
fn convert_keyframes(
    motion_id: &str,
    parameter: &str,
    raw_keyframes: &[RawKeyframe],
    duration_seconds: f32,
    params: &HashMap<&str, &Parameter>,
) -> Result<Vec<Keyframe>, ParseError> {
    let context = format!("motion {motion_id:?} curve {parameter:?}");
    if raw_keyframes.is_empty() {
        return Err(ParseError::Schema {
            message: format!("{context} has no keyframes"),
        });
    }
    let (min, max) = parameter_range(params[parameter]);
    let mut keyframes = Vec::with_capacity(raw_keyframes.len());
    for raw_key in raw_keyframes {
        check_finite(raw_key.time, &context)?;
        check_finite(raw_key.value, &context)?;
        if raw_key.value < min || raw_key.value > max {
            return Err(ParseError::OutOfRange {
                context: format!("{context} keyframe value"),
            });
        }
        let interpolation = match &raw_key.interpolation {
            None => Interpolation::Linear,
            Some(text) => {
                Interpolation::from_format_str(text).ok_or_else(|| ParseError::Schema {
                    message: format!("{context} unknown interpolation {text:?}"),
                })?
            }
        };
        let tangent_in = convert_tangent(&context, raw_key.tangent_in)?;
        let tangent_out = convert_tangent(&context, raw_key.tangent_out)?;
        keyframes.push(Keyframe {
            time: raw_key.time,
            value: raw_key.value,
            interpolation,
            tangent_in,
            tangent_out,
        });
    }
    check_keyframe_times(motion_id, parameter, &keyframes, duration_seconds)?;
    check_tangent_bounds(motion_id, parameter, &keyframes)?;
    Ok(keyframes)
}

/// A tangent pair: both components finite; the sign and reach
/// rules are segment-relative and checked in
/// `check_tangent_bounds`.
fn convert_tangent(
    context: &str,
    tangent: Option<[f32; 2]>,
) -> Result<Option<[f32; 2]>, ParseError> {
    match tangent {
        None => Ok(None),
        Some([dt, dv]) => {
            check_finite(dt, context)?;
            check_finite(dv, context)?;
            Ok(Some([dt, dv]))
        }
    }
}

/// Times strictly increasing, first at or after 0, last at or
/// before the duration (`LRF-501`).
fn check_keyframe_times(
    motion_id: &str,
    parameter: &str,
    keyframes: &[Keyframe],
    duration_seconds: f32,
) -> Result<(), ParseError> {
    let invalid = || ParseError::KeyframeTimes {
        motion_id: motion_id.to_string(),
        parameter: parameter.to_string(),
    };
    for (index, key) in keyframes.iter().enumerate() {
        if key.time < 0.0 || key.time > duration_seconds {
            return Err(invalid());
        }
        if index > 0 && key.time <= keyframes[index - 1].time {
            return Err(invalid());
        }
    }
    Ok(())
}

/// The tangent law (`LRF-502`), segment-relative as section
/// 4.2 reads it: a segment is cubic when its STARTING key says
/// so, and the tangents that segment consumes are the starting
/// key's `tangent_out` and the closing key's `tangent_in`. A
/// tangent no cubic segment would read is invalid — a
/// `tangent_out` on a non-cubic or last key, a `tangent_in` on
/// the first key or after a non-cubic segment. Per segment,
/// `dt >= 0` on both tangents and neither may reach past the
/// segment's far key, which keeps the Bézier's time component
/// monotonically non-decreasing.
fn check_tangent_bounds(
    motion_id: &str,
    parameter: &str,
    keyframes: &[Keyframe],
) -> Result<(), ParseError> {
    let context = format!("motion {motion_id:?} curve {parameter:?}");
    let invalid = |field: &str| ParseError::TangentInvalid {
        context: format!("{context} {field}"),
    };
    for (index, key) in keyframes.iter().enumerate() {
        if key.tangent_out.is_some()
            && (index + 1 == keyframes.len() || key.interpolation != Interpolation::Cubic)
        {
            return Err(invalid("tangent_out"));
        }
        if key.tangent_in.is_some()
            && (index == 0 || keyframes[index - 1].interpolation != Interpolation::Cubic)
        {
            return Err(invalid("tangent_in"));
        }
    }
    for segment in keyframes.windows(2) {
        let (from, to) = (&segment[0], &segment[1]);
        if from.interpolation != Interpolation::Cubic {
            continue;
        }
        if let Some([dt_out, _]) = from.tangent_out
            && (dt_out < 0.0 || from.time + dt_out > to.time)
        {
            return Err(invalid("tangent_out"));
        }
        if let Some([dt_in, _]) = to.tangent_in
            && (dt_in < 0.0 || to.time - dt_in < from.time)
        {
            return Err(invalid("tangent_in"));
        }
    }
    Ok(())
}

/// Validate baked-set provenance (SPEC.md section 6.1): the
/// kind is known; `manual` sets carry no analysis fields;
/// analyzed sets name their source audio by hash and carry
/// positive window/hop.
fn convert_provenance(
    motion_id: &str,
    raw: &RawBakedProvenance,
) -> Result<BakedProvenance, ParseError> {
    let context = format!("motion {motion_id:?} baked provenance");
    let kind = ProvenanceKind::from_format_str(&raw.kind).ok_or_else(|| ParseError::Schema {
        message: format!("{context} unknown kind {:?}", raw.kind),
    })?;
    for (field, value) in [("tool", &raw.tool), ("tool_version", &raw.tool_version)] {
        if value.len() > NAME_BYTES_MAX {
            return Err(ParseError::FieldRange {
                context: format!("{context} {field} exceeds NAME_BYTES_MAX"),
            });
        }
    }
    if let Some(sha) = &raw.source_audio_sha256
        && !is_sha256_hex(sha)
    {
        return Err(ParseError::Schema {
            message: format!("{context} source_audio_sha256 is not 64 lowercase hex chars"),
        });
    }
    match kind {
        ProvenanceKind::Manual => {
            if raw.source_audio_sha256.is_some()
                || raw.window_seconds.is_some()
                || raw.hop_seconds.is_some()
            {
                return Err(ParseError::Schema {
                    message: format!("{context} manual set carries analysis fields"),
                });
            }
        }
        ProvenanceKind::Envelope | ProvenanceKind::Onset => {
            if raw.source_audio_sha256.is_none() {
                return Err(ParseError::Schema {
                    message: format!("{context} analyzed set lacks source_audio_sha256"),
                });
            }
            for (field, value) in [
                ("window_seconds", raw.window_seconds),
                ("hop_seconds", raw.hop_seconds),
            ] {
                match value {
                    None => {
                        return Err(ParseError::Schema {
                            message: format!("{context} analyzed set lacks {field}"),
                        });
                    }
                    Some(seconds) => {
                        check_finite(seconds, &context)?;
                        if seconds <= 0.0 {
                            return Err(ParseError::FieldRange {
                                context: format!("{context} {field} must be > 0"),
                            });
                        }
                    }
                }
            }
        }
    }
    Ok(BakedProvenance {
        kind,
        tool: raw.tool.clone(),
        tool_version: raw.tool_version.clone(),
        source_audio_sha256: raw.source_audio_sha256.clone(),
        window_seconds: raw.window_seconds,
        hop_seconds: raw.hop_seconds,
    })
}

/// A sha256 hex digest: exactly 64 lowercase hex characters.
fn is_sha256_hex(text: &str) -> bool {
    text.len() == 64
        && text
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Validate the fathom binding (SPEC.md section 4.4): the
/// schema token is fathom's, phase provenance and arc kind are
/// fathom vocabulary, and every phase's frame range is ordered.
fn convert_fathom(motion_id: &str, raw: &RawFathom) -> Result<FathomBinding, ParseError> {
    let context = format!("motion {motion_id:?} fathom binding");
    if raw.schema != FATHOM_SCHEMA {
        return Err(ParseError::Schema {
            message: format!("{context} schema {:?} is not {FATHOM_SCHEMA:?}", raw.schema),
        });
    }
    let mut phases = Vec::with_capacity(raw.phases.len());
    for raw_phase in &raw.phases {
        if raw_phase.label.is_empty() || raw_phase.label.len() > NAME_BYTES_MAX {
            return Err(ParseError::FieldRange {
                context: format!("{context} phase label length"),
            });
        }
        if raw_phase.start_frame > raw_phase.end_frame {
            return Err(ParseError::FieldRange {
                context: format!("{context} phase {:?} start after end", raw_phase.label),
            });
        }
        let provenance =
            PhaseProvenance::from_format_str(&raw_phase.provenance).ok_or_else(|| {
                ParseError::Schema {
                    message: format!(
                        "{context} unknown phase provenance {:?}",
                        raw_phase.provenance
                    ),
                }
            })?;
        phases.push(FathomPhase {
            label: raw_phase.label.clone(),
            start_frame: raw_phase.start_frame,
            end_frame: raw_phase.end_frame,
            provenance,
            source: raw_phase.source.clone(),
        });
    }
    let kind = ArcKind::from_format_str(&raw.arc.kind).ok_or_else(|| ParseError::Schema {
        message: format!("{context} unknown arc kind {:?}", raw.arc.kind),
    })?;
    Ok(FathomBinding {
        schema: raw.schema.clone(),
        phases,
        arc: FathomArc { kind },
    })
}
