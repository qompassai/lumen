//! Adversarial tests for Phase 5: invalid toon configurations are
//! rejected by validation and by the GLB writer, fail-closed.

use lumen_factory::base::build_base_mesh;
use lumen_factory::gltf::write_glb_with_toon;
use lumen_factory::toon::ToonConfig;

fn config_with(thresholds: [f32; 2], rim: f32, shadow: &str, highlight: &str) -> ToonConfig {
    ToonConfig {
        highlight_tint: highlight.to_string(),
        ramp_thresholds: thresholds,
        rim_strength: rim,
        shadow_tint: shadow.to_string(),
    }
}

#[test]
fn reversed_or_equal_thresholds_rejected() {
    assert!(
        config_with([0.70, 0.35], 0.35, "#2a2438", "#ffffff")
            .validate()
            .is_err()
    );
    assert!(
        config_with([0.50, 0.50], 0.35, "#2a2438", "#ffffff")
            .validate()
            .is_err()
    );
}

#[test]
fn out_of_range_or_nonfinite_thresholds_rejected() {
    assert!(
        config_with([-0.1, 0.5], 0.35, "#2a2438", "#ffffff")
            .validate()
            .is_err()
    );
    assert!(
        config_with([0.5, 1.1], 0.35, "#2a2438", "#ffffff")
            .validate()
            .is_err()
    );
    assert!(
        config_with([f32::NAN, 0.5], 0.35, "#2a2438", "#ffffff")
            .validate()
            .is_err()
    );
    assert!(
        config_with([0.1, f32::INFINITY], 0.35, "#2a2438", "#ffffff")
            .validate()
            .is_err()
    );
}

#[test]
fn out_of_range_rim_rejected() {
    assert!(
        config_with([0.35, 0.70], -0.1, "#2a2438", "#ffffff")
            .validate()
            .is_err()
    );
    assert!(
        config_with([0.35, 0.70], 1.5, "#2a2438", "#ffffff")
            .validate()
            .is_err()
    );
    assert!(
        config_with([0.35, 0.70], f32::NAN, "#2a2438", "#ffffff")
            .validate()
            .is_err()
    );
}

#[test]
fn malformed_tints_rejected() {
    for bad in ["red", "#12345", "#1234567", "#gggggg", "2a2438", ""] {
        assert!(
            config_with([0.35, 0.70], 0.35, bad, "#ffffff")
                .validate()
                .is_err(),
            "shadow tint {bad:?} must fail"
        );
        assert!(
            config_with([0.35, 0.70], 0.35, "#2a2438", bad)
                .validate()
                .is_err(),
            "highlight tint {bad:?} must fail"
        );
    }
}

#[test]
fn writer_rejects_invalid_config() {
    let mesh = build_base_mesh().unwrap();
    let bad = config_with([0.9, 0.1], 0.35, "#2a2438", "#ffffff");
    assert!(write_glb_with_toon(&mesh, &[], None, Some(&bad)).is_err());
}

#[test]
fn from_json_rejects_invalid_payload() {
    let payload = serde_json::json!({
        "highlight_tint": "#ffffff",
        "ramp_thresholds": [0.8, 0.2],
        "rim_strength": 0.35,
        "shadow_tint": "#2a2438"
    });
    assert!(ToonConfig::from_json(&payload).is_err());
    let missing = serde_json::json!({ "ramp_thresholds": [0.35, 0.70] });
    assert!(ToonConfig::from_json(&missing).is_err());
}
