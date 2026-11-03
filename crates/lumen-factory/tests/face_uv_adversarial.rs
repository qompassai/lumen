//! Adversarial tests for the face-UV contract census (T-11):
//! refusal without a layer, and each contract violation refused
//! under its own check name.

use lumen_factory::face_uv::{FaceLandmarks, FaceUvData, assess_face_uv};

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
        mirror_pairs: vec![([0.30, 0.62], [0.70, 0.62]), ([0.44, 0.45], [0.56, 0.45])],
        uv_layer: Some("FaceUV".to_string()),
    }
}

fn check(report: &lumen_factory::face_uv::FaceUvReport, name: &str) -> bool {
    report
        .checks
        .iter()
        .find(|candidate| candidate.name == name)
        .map(|candidate| candidate.pass)
        .unwrap_or_else(|| panic!("check {name} missing"))
}

#[test]
fn no_uv_layer_refuses_the_contract_outright() {
    let mut data = conforming_face();
    data.uv_layer = None;
    let report = assess_face_uv(&data).expect("refusal is a report, not an error");
    assert!(!report.pass);
    assert!(report.checks.iter().all(|check| !check.pass));
    assert!(!check(&report, "uv_layer_present"));
}

#[test]
fn leaning_or_flat_face_fails_face_vertical() {
    let mut leaning = conforming_face();
    leaning.chin = [0.44, 0.08];
    leaning.forehead = [0.56, 0.92];
    let report = assess_face_uv(&leaning).expect("face assesses");
    assert!(!report.pass);
    assert!(!check(&report, "face_vertical"));

    let mut flat = conforming_face();
    flat.forehead = [0.5, 0.14];
    let report = assess_face_uv(&flat).expect("face assesses");
    assert!(!check(&report, "face_vertical"));
}

#[test]
fn off_centre_face_fails_the_centre_edge_check() {
    let mut data = conforming_face();
    data.chin = [0.40, 0.08];
    data.forehead = [0.40, 0.92];
    let report = assess_face_uv(&data).expect("face assesses");
    assert!(!report.pass);
    assert!(!check(&report, "centre_edge_on_uv_centre"));
    // The axis itself is still vertical and unleaned.
    assert!(check(&report, "face_vertical"));
}

#[test]
fn asymmetric_pairs_fail_symmetry() {
    let mut data = conforming_face();
    data.mirror_pairs = vec![([0.30, 0.62], [0.66, 0.62])];
    let report = assess_face_uv(&data).expect("face assesses");
    assert!(!report.pass);
    assert!(!check(&report, "left_right_symmetry"));

    let mut empty = conforming_face();
    empty.mirror_pairs = Vec::new();
    let report = assess_face_uv(&empty).expect("face assesses");
    assert!(!check(&report, "left_right_symmetry"));
}

#[test]
fn collapsed_landmarks_fail_the_landmark_check() {
    let mut collapsed = conforming_face();
    collapsed.landmarks.under_eye_left = [[0.36, 0.60], [0.36, 0.60], [0.36, 0.60]];
    let report = assess_face_uv(&collapsed).expect("face assesses");
    assert!(!report.pass);
    assert!(!check(&report, "landmarks_present"));

    let mut same_side = conforming_face();
    same_side.landmarks.nose_right_flank = [0.47, 0.45];
    let report = assess_face_uv(&same_side).expect("face assesses");
    assert!(!check(&report, "landmarks_present"));
}

#[test]
fn corrupt_uvs_are_typed_errors_not_failed_checks() {
    let mut out_of_range = conforming_face();
    out_of_range.chin = [0.5, 1.4];
    assert!(assess_face_uv(&out_of_range).is_err());

    let mut non_finite = conforming_face();
    non_finite.mirror_pairs = vec![([f32::NAN, 0.5], [0.5, 0.5])];
    assert!(assess_face_uv(&non_finite).is_err());

    let mut empty_name = conforming_face();
    empty_name.uv_layer = Some(String::new());
    assert!(assess_face_uv(&empty_name).is_err());
}
