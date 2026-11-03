//! Validation tests for the face-UV contract census (T-11 lumen
//! half): a contract-conforming face is admitted to the SDF step.

use lumen_factory::face_uv::{FaceLandmarks, FaceUvData, assess_face_uv};

/// A conforming face: centre edge on U = 0.5, forehead well above
/// chin, mirror pairs exact, landmarks real and straddling.
fn conforming_face() -> FaceUvData {
    FaceUvData {
        chin: [0.5, 0.08],
        forehead: [0.5, 0.92],
        landmarks: FaceLandmarks {
            nose_left_flank: [0.44, 0.45],
            nose_right_flank: [0.56, 0.45],
            under_eye_left: [[0.30, 0.62], [0.42, 0.62], [0.36, 0.56]],
            under_eye_right: [[0.58, 0.62], [0.70, 0.62], [0.64, 0.56]],
        },
        mirror_pairs: vec![
            ([0.30, 0.62], [0.70, 0.62]),
            ([0.44, 0.45], [0.56, 0.45]),
            ([0.22, 0.30], [0.78, 0.30]),
        ],
        uv_layer: Some("FaceUV".to_string()),
    }
}

#[test]
fn conforming_face_is_admitted_to_the_sdf_step() {
    let report = assess_face_uv(&conforming_face()).expect("face assesses");
    assert!(report.pass, "{report:?}");
    assert_eq!(report.checks.len(), 5);
    assert!(report.checks.iter().all(|check| check.pass));
    assert_eq!(report.schema, "lumen.face-uv-contract");
    assert_eq!(report.version, 1);
    assert_eq!(report.uv_layer.as_deref(), Some("FaceUV"));
}

#[test]
fn checks_come_in_contract_order() {
    let report = assess_face_uv(&conforming_face()).expect("face assesses");
    let names: Vec<&str> = report
        .checks
        .iter()
        .map(|check| check.name.as_str())
        .collect();
    assert_eq!(
        names,
        [
            "uv_layer_present",
            "face_vertical",
            "centre_edge_on_uv_centre",
            "left_right_symmetry",
            "landmarks_present",
        ]
    );
}

#[test]
fn report_serializes_with_schema() {
    let report = assess_face_uv(&conforming_face()).expect("face assesses");
    let json = serde_json::to_string(&report).expect("report serializes");
    assert!(
        json.contains("\"schema\":\"lumen.face-uv-contract\""),
        "{json}"
    );
    assert!(json.contains("\"pass\":true"), "{json}");
}
