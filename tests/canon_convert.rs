//! Canon convert tests (R-607): 4 validation, 6 adversarial.
//!
//! Validation proves the handled conversion consumes the
//! record's palette and gates before writing. Adversarial
//! proves the lane law and the gate hold: cross-lane attempts,
//! unanchored conversion, fabricated anchors, stale records,
//! and drifted artifacts are refused and write nothing.

use std::collections::BTreeMap;
use std::path::Path;

use canon_record::{Measurements, Refusal};
use image::{ImageBuffer, Rgba};
use lumen::canon::{ConvertRequest, canon_convert};

const ANCHOR_SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const AUTHORITY_SHA: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn record_json(id: &str, lane: &str, label: &str) -> serde_json::Value {
    serde_json::json!({
        "format": "canon-record/1",
        "id": id,
        "version": 1,
        "origin_lane": lane,
        "content_label": label,
        "identity_invariants": [
            {"name": "hair", "fact": "black bob", "check": {"kind": "eye-only"}}
        ],
        "anchors": [{
            "role": "primary", "handle": "artifact://test/anchor.png",
            "sha256": ANCHOR_SHA, "adopted_by": "Owner",
            "adopted_at": "2026-10-28", "decision_ref": "D-TEST"
        }],
        "style_fingerprint": {
            "proportion_bands": [
                {"metric": "face_width_height_ratio", "target": 0.86, "tolerance": 0.12, "method": "landmarks"}
            ],
            "register_bands": [
                {"metric": "ink_colour_distance", "target": 0.02, "tolerance": 0.03, "method": "ink cluster"},
                {"metric": "shadow_edge_hardness", "target": 0.9, "tolerance": 0.1, "method": "gradient"},
                {"metric": "blush_coverage", "target": 0.12, "tolerance": 0.05, "method": "blush share"}
            ],
            "rendering_markers": ["flat cel"],
            "palette": [
                {"role": "ink", "hex": "#160804", "tolerance": 0.05},
                {"role": "paper", "hex": "#F5EFE4", "tolerance": 0.05}
            ]
        },
        "authority_refs": [{
            "handle": "artifact://test/authority.png", "sha256": AUTHORITY_SHA
        }],
        "keeper_chain": []
    })
}

fn store(root: &Path, lane: &str, character: &str, record: &serde_json::Value) -> String {
    let dir = root.join("canon").join(lane);
    std::fs::create_dir_all(&dir).expect("store dir");
    let bytes = serde_json::to_vec(record).expect("serialize");
    std::fs::write(dir.join(format!("{character}.json")), &bytes).expect("write record");
    canon_record::sha256_hex(&bytes)
}

fn sfw_store(root: &Path) -> String {
    store(
        root,
        "qompass",
        "testchar",
        &record_json("qompass/testchar", "qompass", "sfw"),
    )
}

fn good_measurements() -> Measurements {
    let mut m = BTreeMap::new();
    m.insert("face_width_height_ratio".into(), 0.86);
    m.insert("ink_colour_distance".into(), 0.03);
    m.insert("shadow_edge_hardness".into(), 0.88);
    m.insert("blush_coverage".into(), 0.13);
    Measurements {
        measurements: m,
        source: None,
    }
}

fn write_input(root: &Path) {
    // 4x2: near-ink, near-paper, far-from-palette green, transparent.
    let mut img: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::new(4, 2);
    img.put_pixel(0, 0, Rgba([24, 10, 6, 255])); // near #160804
    img.put_pixel(1, 0, Rgba([240, 232, 220, 255])); // near #F5EFE4
    img.put_pixel(2, 0, Rgba([0, 200, 0, 255])); // outside palette
    img.put_pixel(3, 0, Rgba([24, 10, 6, 0])); // transparent: untouched
    img.put_pixel(0, 1, Rgba([22, 8, 4, 255])); // exact ink
    img.put_pixel(1, 1, Rgba([245, 239, 228, 255])); // exact paper
    img.put_pixel(2, 1, Rgba([0, 200, 0, 255]));
    img.put_pixel(3, 1, Rgba([0, 200, 0, 255]));
    img.save(root.join("in.png")).expect("write input");
}

fn request(root: &Path, pin: &str) -> ConvertRequest {
    let _ = root;
    ConvertRequest {
        handle: "canon://qompass/testchar@1".into(),
        record_sha256: pin.into(),
        caller_lane: "qompass".into(),
        anchor_sha256: Some(ANCHOR_SHA.into()),
        input_path: "in.png".into(),
        output_path: "out.png".into(),
        measurements: good_measurements(),
    }
}

// ----------------------------------------------------------------
// Validation
// ----------------------------------------------------------------

#[test]
fn convert_snaps_pixels_to_the_record_palette() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let pin = sfw_store(root);
    write_input(root);
    let report = canon_convert(root, &request(root, &pin)).expect("converts");
    assert_eq!(report.palette_entries, 2);
    assert_eq!(report.pixels_total, 7);
    assert_eq!(report.pixels_outside_palette, 3);
    assert_eq!(report.gate, "pass");
    let out = image::open(root.join("out.png"))
        .expect("output exists")
        .to_rgba8();
    assert_eq!(out.get_pixel(0, 0), &Rgba([22, 8, 4, 255]));
    assert_eq!(out.get_pixel(1, 0), &Rgba([245, 239, 228, 255]));
    // Outside tolerance: colour kept, honestly counted.
    assert_eq!(out.get_pixel(2, 0), &Rgba([0, 200, 0, 255]));
    // Transparent pixels are never touched.
    assert_eq!(out.get_pixel(3, 0), &Rgba([24, 10, 6, 0]));
}

#[test]
fn convert_is_deterministic() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let pin = sfw_store(root);
    write_input(root);
    let a = canon_convert(root, &request(root, &pin)).expect("runs");
    let bytes_a = std::fs::read(root.join("out.png")).expect("read a");
    let mut req = request(root, &pin);
    req.output_path = "out2.png".into();
    let b = canon_convert(root, &req).expect("runs");
    let bytes_b = std::fs::read(root.join("out2.png")).expect("read b");
    assert_eq!(a.pixels_remapped, b.pixels_remapped);
    assert_eq!(bytes_a, bytes_b);
}

#[test]
fn ntranch_caller_may_convert_an_nsfw_record() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let pin = store(
        root,
        "ntranch",
        "opal",
        &record_json("ntranch/opal", "ntranch", "nsfw"),
    );
    write_input(root);
    let mut req = request(root, &pin);
    req.handle = "canon://ntranch/opal@1".into();
    req.caller_lane = "ntranch".into();
    let report = canon_convert(root, &req).expect("same-lane conversion proceeds");
    assert_eq!(report.gate, "pass");
}

#[test]
fn convert_output_lands_under_the_project_root() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let pin = sfw_store(root);
    write_input(root);
    let mut req = request(root, &pin);
    req.output_path = "nested/converted.png".into();
    let report = canon_convert(root, &req).expect("converts");
    assert!(root.join("nested/converted.png").exists());
    assert_eq!(report.output_path, "nested/converted.png");
}

// ----------------------------------------------------------------
// Adversarial
// ----------------------------------------------------------------

#[test]
fn cross_lane_conversion_is_refused_and_writes_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let pin = store(
        root,
        "ntranch",
        "opal",
        &record_json("ntranch/opal", "ntranch", "nsfw"),
    );
    write_input(root);
    let mut req = request(root, &pin);
    req.handle = "canon://ntranch/opal@1".into();
    req.caller_lane = "qompass".into(); // SFW caller, nsfw record
    let err = canon_convert(root, &req).unwrap_err();
    assert!(matches!(err, Refusal::LaneMismatch { .. }), "{err:?}");
    assert!(!root.join("out.png").exists());
}

#[test]
fn unclassified_record_defaults_to_nsfw_and_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let mut record = record_json("ntranch/opal", "ntranch", "nsfw");
    record
        .as_object_mut()
        .expect("object")
        .remove("content_label");
    let pin = store(root, "ntranch", "opal", &record);
    write_input(root);
    let mut req = request(root, &pin);
    req.handle = "canon://ntranch/opal@1".into();
    let err = canon_convert(root, &req).unwrap_err();
    assert!(matches!(err, Refusal::LaneMismatch { .. }), "{err:?}");
    assert!(!root.join("out.png").exists());
}

#[test]
fn unanchored_conversion_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let pin = sfw_store(root);
    write_input(root);
    let mut req = request(root, &pin);
    req.anchor_sha256 = None;
    let err = canon_convert(root, &req).unwrap_err();
    assert!(matches!(err, Refusal::UnanchoredLineage), "{err:?}");
    assert!(!root.join("out.png").exists());
}

#[test]
fn fabricated_anchor_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let pin = sfw_store(root);
    write_input(root);
    let mut req = request(root, &pin);
    req.anchor_sha256 = Some("f".repeat(64));
    let err = canon_convert(root, &req).unwrap_err();
    assert!(matches!(err, Refusal::AnchorNotAdopted { .. }), "{err:?}");
    assert!(!root.join("out.png").exists());
}

#[test]
fn stale_record_pin_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let _pin = sfw_store(root);
    write_input(root);
    let err = canon_convert(root, &request(root, &"0".repeat(64))).unwrap_err();
    assert!(matches!(err, Refusal::RecordStale { .. }), "{err:?}");
    assert!(!root.join("out.png").exists());
}

#[test]
fn drifted_measurements_are_refused_and_write_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path();
    let pin = sfw_store(root);
    write_input(root);
    let mut req = request(root, &pin);
    let mut m = BTreeMap::new();
    m.insert("face_width_height_ratio".into(), 0.30); // far outside band
    m.insert("ink_colour_distance".into(), 0.03);
    m.insert("shadow_edge_hardness".into(), 0.88);
    m.insert("blush_coverage".into(), 0.13);
    req.measurements = Measurements {
        measurements: m,
        source: None,
    };
    let err = canon_convert(root, &req).unwrap_err();
    assert!(matches!(err, Refusal::DriftDetected { .. }), "{err:?}");
    assert!(!root.join("out.png").exists());
}
