//! The strict face-UV contract census (T-11 lumen half, D-740).
//!
//! SDF face shading samples a painted-terminator texture in face
//! UV space, which only works when the face's UV layout obeys a
//! strict contract. This census is the gate: a head either
//! satisfies the contract on a named UV layer or it is **refused**
//! the SDF step — there is no partial credit, because a
//! half-conforming layout shades wrong silently.
//!
//! The contract (`lumen.face-uv-contract` v1), checked on the
//! dedicated face UV layer (not the factory's zoned atlas UVs):
//! - `uv_layer_present`: a face UV layer is named at all. Without
//!   one the head is refused outright and no other check is
//!   assessed.
//! - `face_vertical`: the face runs vertically in UV space — the
//!   forehead landmark sits above the chin landmark by at least
//!   [`FACE_VERTICAL_MIN_SPAN`], on a near-common U (the axis does
//!   not lean).
//! - `centre_edge_on_uv_centre`: the face's centre edge — the
//!   vertical seam between the mirrored halves, sampled at the
//!   chin and forehead landmarks — lies on the UV centre line
//!   U = 0.5 within [`UV_CENTRE_TOLERANCE`].
//! - `left_right_symmetry`: every supplied mirror pair mirrors
//!   across U = 0.5: `u_left + u_right = 1` and `v_left = v_right`,
//!   each within [`UV_SYMMETRY_TOLERANCE`].
//! - `landmarks_present`: the SDF landmarks exist as real UV
//!   geometry — both under-eye triangles have area at least
//!   [`LANDMARK_MIN_AREA`], and the nose flanks straddle the
//!   centre line (left flank U < 0.5 < right flank U).
//!
//! UV coordinates are validated fail-closed: finite and inside
//! `[0, 1]`; anything else is a corrupt extraction, not a failed
//! check.

#![forbid(unsafe_code)]

use serde::Serialize;

use crate::FactoryError;
use crate::validate::ValidationCheck;

/// Schema identifier for the face-UV contract census.
pub const FACE_UV_SCHEMA: &str = "lumen.face-uv-contract";
/// Schema version for the face-UV contract census.
pub const FACE_UV_VERSION: u32 = 1;
/// Minimum forehead-above-chin V span for `face_vertical`.
pub const FACE_VERTICAL_MIN_SPAN: f32 = 0.1;
/// Minimum under-eye triangle area (UV units squared) for a
/// landmark to count as present.
pub const LANDMARK_MIN_AREA: f32 = 1e-4;
/// How close the centre edge must sit to U = 0.5, and how far the
/// face axis may lean (chin/forehead U difference).
pub const UV_CENTRE_TOLERANCE: f32 = 0.02;
/// How closely a mirror pair must actually mirror.
pub const UV_SYMMETRY_TOLERANCE: f32 = 0.02;

/// The landmark UVs the contract is assessed from, sampled on the
/// face UV layer named in [`FaceUvData::uv_layer`].
#[derive(Clone, Debug, PartialEq)]
pub struct FaceLandmarks {
    /// Nose flank on the character's left (U < 0.5 side).
    pub nose_left_flank: [f32; 2],
    /// Nose flank on the character's right (U > 0.5 side).
    pub nose_right_flank: [f32; 2],
    /// The left under-eye landmark triangle (3 UV points).
    pub under_eye_left: [[f32; 2]; 3],
    /// The right under-eye landmark triangle (3 UV points).
    pub under_eye_right: [[f32; 2]; 3],
}

/// One head's face-UV evidence: the layer under assessment, the
/// centre-edge landmarks, the SDF landmarks, and the mirror pairs.
#[derive(Clone, Debug, PartialEq)]
pub struct FaceUvData {
    /// Chin landmark UV (on the face's centre edge).
    pub chin: [f32; 2],
    /// Forehead landmark UV (on the face's centre edge).
    pub forehead: [f32; 2],
    /// The SDF landmark set.
    pub landmarks: FaceLandmarks,
    /// (left, right) landmark UV pairs that must mirror.
    pub mirror_pairs: Vec<([f32; 2], [f32; 2])>,
    /// The face UV layer under assessment; `None` refuses the
    /// contract outright.
    pub uv_layer: Option<String>,
}

/// The face-UV contract census report: the five named checks in
/// contract order. `pass` is the SDF step's admission ticket.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct FaceUvReport {
    /// The contract checks, in contract order.
    pub checks: Vec<ValidationCheck>,
    /// True iff every check passed — the head may consume the
    /// SDF face step.
    pub pass: bool,
    /// Schema identifier, always [`FACE_UV_SCHEMA`].
    pub schema: String,
    /// The layer that was assessed (`None` when refused outright).
    pub uv_layer: Option<String>,
    /// Schema version, always [`FACE_UV_VERSION`].
    pub version: u32,
}

fn validate_uv(point: [f32; 2], what: &str) -> Result<(), FactoryError> {
    if point
        .iter()
        .any(|component| !component.is_finite() || !(0.0..=1.0).contains(component))
    {
        return Err(FactoryError::BadParam(format!(
            "{what} UV {point:?} is non-finite or outside [0, 1]"
        )));
    }
    Ok(())
}

fn validate_face_uv_data(data: &FaceUvData) -> Result<(), FactoryError> {
    validate_uv(data.chin, "chin")?;
    validate_uv(data.forehead, "forehead")?;
    validate_uv(data.landmarks.nose_left_flank, "nose_left_flank")?;
    validate_uv(data.landmarks.nose_right_flank, "nose_right_flank")?;
    for (side, triangle) in [
        ("under_eye_left", data.landmarks.under_eye_left),
        ("under_eye_right", data.landmarks.under_eye_right),
    ] {
        for point in triangle {
            validate_uv(point, side)?;
        }
    }
    for (left, right) in &data.mirror_pairs {
        validate_uv(*left, "mirror_pair left")?;
        validate_uv(*right, "mirror_pair right")?;
    }
    if let Some(name) = &data.uv_layer
        && name.is_empty()
    {
        return Err(FactoryError::BadParam(
            "a UV layer with an empty name is not a layer".to_string(),
        ));
    }
    Ok(())
}

fn triangle_area(triangle: &[[f32; 2]; 3]) -> f32 {
    let [a, b, c] = triangle;
    0.5 * ((b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1])).abs()
}

fn refused_report(data: &FaceUvData) -> FaceUvReport {
    let names = [
        "uv_layer_present",
        "face_vertical",
        "centre_edge_on_uv_centre",
        "left_right_symmetry",
        "landmarks_present",
    ];
    FaceUvReport {
        checks: names
            .iter()
            .enumerate()
            .map(|(index, name)| ValidationCheck {
                detail: if index == 0 {
                    "no face UV layer named; the head is refused the SDF step".to_string()
                } else {
                    "not assessed: no UV layer".to_string()
                },
                name: name.to_string(),
                pass: false,
            })
            .collect(),
        pass: false,
        schema: FACE_UV_SCHEMA.to_string(),
        uv_layer: data.uv_layer.clone(),
        version: FACE_UV_VERSION,
    }
}

/// Assess one head against the face-UV contract. Structural
/// corruption (non-finite or out-of-range UVs) is a typed error;
/// contract violations are report content — with `pass: false`
/// the head is refused the SDF step (D-740).
pub fn assess_face_uv(data: &FaceUvData) -> Result<FaceUvReport, FactoryError> {
    validate_face_uv_data(data)?;
    if data.uv_layer.is_none() {
        return Ok(refused_report(data));
    }
    let span = data.forehead[1] - data.chin[1];
    let lean = (data.forehead[0] - data.chin[0]).abs();
    let vertical = span >= FACE_VERTICAL_MIN_SPAN && lean <= UV_CENTRE_TOLERANCE;
    let centre = (data.chin[0] - 0.5).abs() <= UV_CENTRE_TOLERANCE
        && (data.forehead[0] - 0.5).abs() <= UV_CENTRE_TOLERANCE;
    let symmetry = !data.mirror_pairs.is_empty()
        && data.mirror_pairs.iter().all(|(left, right)| {
            (left[0] + right[0] - 1.0).abs() <= UV_SYMMETRY_TOLERANCE
                && (left[1] - right[1]).abs() <= UV_SYMMETRY_TOLERANCE
        });
    let left_area = triangle_area(&data.landmarks.under_eye_left);
    let right_area = triangle_area(&data.landmarks.under_eye_right);
    let landmarks = left_area >= LANDMARK_MIN_AREA
        && right_area >= LANDMARK_MIN_AREA
        && data.landmarks.nose_left_flank[0] < 0.5
        && data.landmarks.nose_right_flank[0] > 0.5;
    let checks = vec![
        ValidationCheck {
            detail: format!(
                "face UV layer '{}' under assessment",
                data.uv_layer.as_deref().unwrap_or_default()
            ),
            name: "uv_layer_present".to_string(),
            pass: true,
        },
        ValidationCheck {
            detail: format!(
                "forehead-chin V span {span:.3} (min {FACE_VERTICAL_MIN_SPAN}), axis lean {lean:.3} (max {UV_CENTRE_TOLERANCE})"
            ),
            name: "face_vertical".to_string(),
            pass: vertical,
        },
        ValidationCheck {
            detail: format!(
                "centre edge U at chin {:.3}, forehead {:.3} (centre 0.5 ± {UV_CENTRE_TOLERANCE})",
                data.chin[0], data.forehead[0]
            ),
            name: "centre_edge_on_uv_centre".to_string(),
            pass: centre,
        },
        ValidationCheck {
            detail: if data.mirror_pairs.is_empty() {
                "no mirror pairs supplied".to_string()
            } else {
                format!(
                    "{} mirror pairs within ±{UV_SYMMETRY_TOLERANCE}",
                    data.mirror_pairs.len()
                )
            },
            name: "left_right_symmetry".to_string(),
            pass: symmetry,
        },
        ValidationCheck {
            detail: format!(
                "under-eye areas {left_area:.5}/{right_area:.5} (min {LANDMARK_MIN_AREA}), nose flanks at U {:.3}/{:.3}",
                data.landmarks.nose_left_flank[0], data.landmarks.nose_right_flank[0]
            ),
            name: "landmarks_present".to_string(),
            pass: landmarks,
        },
    ];
    Ok(FaceUvReport {
        pass: checks.iter().all(|check| check.pass),
        checks,
        schema: FACE_UV_SCHEMA.to_string(),
        uv_layer: data.uv_layer.clone(),
        version: FACE_UV_VERSION,
    })
}
