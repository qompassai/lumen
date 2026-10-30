//! Adversarial tests: broken bytes, hostile counts, cyclic
//! structures, unrepresentable constructs, and license refusals
//! all fail with typed errors (or convert only through a recorded
//! approximation) — never a panic, never a silent degradation.

mod common;

use common::{FixtureOptions, LICENSE_OK, TestDir, fixture_moc3, fixture_model3, fixture_motion3};
use lumen_rig_import::{ConversionInputs, ConvertError, convert};
use std::path::PathBuf;

fn attempt(
    test_dir: &TestDir,
    moc3: &[u8],
    model3: &str,
    motion3: &str,
    license: &str,
) -> Result<lumen_rig_import::ConversionOutcome, ConvertError> {
    let (model_dir, license_path, out_dir) = test_dir.write_model(moc3, model3, motion3, license);
    convert(&ConversionInputs {
        model_dir,
        license_path,
        out_dir,
    })
}

fn attempt_default(
    test_dir: &TestDir,
    options: &FixtureOptions,
) -> Result<lumen_rig_import::ConversionOutcome, ConvertError> {
    attempt(
        test_dir,
        &fixture_moc3(options),
        &fixture_model3(false),
        &fixture_motion3(30.0),
        LICENSE_OK,
    )
}

#[test]
fn truncated_moc3_fails_at_every_cut() {
    let full = fixture_moc3(&FixtureOptions::default());
    // Note: the cut set targets live sections. A cut inside the
    // draw-order self-index array (the file's last bytes) is not
    // in the set: the converter never reads that array (it is
    // redundant with the object order), so its tail is not load-
    // bearing — the same reason real files may carry slack.
    for cut in [0usize, 3, 10, 100, full.len() / 2, full.len() - 100] {
        let test_dir = TestDir::new("truncated");
        let result = attempt(
            &test_dir,
            &full[..cut],
            &fixture_model3(false),
            &fixture_motion3(30.0),
            LICENSE_OK,
        );
        assert!(
            result.is_err(),
            "cut at {cut} bytes must fail, got {result:?}"
        );
    }
}

#[test]
fn bad_magic_fails() {
    let mut moc3 = fixture_moc3(&FixtureOptions::default());
    moc3[0..4].copy_from_slice(b"XXXX");
    let test_dir = TestDir::new("badmagic");
    let result = attempt(
        &test_dir,
        &moc3,
        &fixture_model3(false),
        &fixture_motion3(30.0),
        LICENSE_OK,
    );
    assert!(
        matches!(result, Err(ConvertError::Moc3 { .. })),
        "got {result:?}"
    );
}

#[test]
fn unsupported_version_bytes_fail() {
    for version in [0u8, 7u8] {
        let test_dir = TestDir::new("version");
        let result = attempt_default(
            &test_dir,
            &FixtureOptions {
                version: Some(version),
                ..Default::default()
            },
        );
        assert!(
            matches!(result, Err(ConvertError::UnsupportedMoc3Version { found }) if found == version),
            "version {version}: got {result:?}"
        );
    }
}

#[test]
fn count_overflow_fails_in_the_battery() {
    let test_dir = TestDir::new("overflow");
    let result = attempt_default(
        &test_dir,
        &FixtureOptions {
            art_mesh_count_override: Some(i32::MAX),
            ..Default::default()
        },
    );
    assert!(
        result.is_err(),
        "an i32::MAX art mesh count must fail, got {result:?}"
    );
}

#[test]
fn cyclic_deformer_parent_fails_native_validation() {
    let test_dir = TestDir::new("defcycle");
    let result = attempt_default(
        &test_dir,
        &FixtureOptions {
            deformer_parent_cycle: true,
            ..Default::default()
        },
    );
    // LRF-204 is lumen-rig-core's reference-cycle code: it covers
    // part, deformer, and mask parent cycles uniformly.
    match result {
        Err(ConvertError::NativeValidation { code, .. }) => assert_eq!(code, "LRF-204"),
        other => panic!("expected LRF-204 native validation, got {other:?}"),
    }
}

#[test]
fn cyclic_clipping_masks_fail_native_validation() {
    let test_dir = TestDir::new("maskcycle");
    let result = attempt_default(
        &test_dir,
        &FixtureOptions {
            mask_cycle: true,
            ..Default::default()
        },
    );
    // LRF-204: lumen-rig-core's reference-cycle code.
    match result {
        Err(ConvertError::NativeValidation { code, .. }) => assert_eq!(code, "LRF-204"),
        other => panic!("expected LRF-204 native validation, got {other:?}"),
    }
}

#[test]
fn self_clipping_mask_fails_native_validation() {
    let test_dir = TestDir::new("selfmask");
    let result = attempt_default(
        &test_dir,
        &FixtureOptions {
            mesh_self_mask: true,
            ..Default::default()
        },
    );
    // LRF-204: lumen-rig-core's reference-cycle code (a self-mask
    // is a cycle of length one).
    match result {
        Err(ConvertError::NativeValidation { code, .. }) => assert_eq!(code, "LRF-204"),
        other => panic!("expected LRF-204 native validation, got {other:?}"),
    }
}

#[test]
fn mirrored_rotation_keyform_fails() {
    let test_dir = TestDir::new("reflect");
    let result = attempt_default(
        &test_dir,
        &FixtureOptions {
            rotation_reflect: true,
            ..Default::default()
        },
    );
    assert!(
        matches!(result, Err(ConvertError::Mapping { .. })),
        "a mirrored rotation keyform must fail as unrepresentable, got {result:?}"
    );
}

#[test]
fn inverted_mask_converts_only_as_a_recorded_approximation() {
    let test_dir = TestDir::new("invertmask");
    let outcome = attempt_default(
        &test_dir,
        &FixtureOptions {
            invert_mask: true,
            ..Default::default()
        },
    )
    .expect("inverted mask converts via the recorded approximation");
    assert!(
        outcome
            .report
            .approximated
            .iter()
            .any(|entry| entry.item == "inverted clipping mask"),
        "the inversion must be recorded, got {:?}",
        outcome.report.approximated
    );
    // The mesh's mask list was dropped: it renders unclipped.
    let model_bytes = std::fs::read(PathBuf::from(&test_dir.root).join("out/fixture.model.json"))
        .expect("model file");
    let model = lumen_rig_core::parse_model(&model_bytes).expect("model validates");
    let quad = model
        .meshes
        .iter()
        .find(|mesh| mesh.id == "MeshQuad")
        .expect("quad");
    assert!(quad.clipping_mask_ids.is_empty());
}

#[test]
fn license_without_game_use_is_refused() {
    let license = LICENSE_OK.replace("\"game_use\": true", "\"game_use\": false");
    let test_dir = TestDir::new("licensegame");
    let result = attempt(
        &test_dir,
        &fixture_moc3(&FixtureOptions::default()),
        &fixture_model3(false),
        &fixture_motion3(30.0),
        &license,
    );
    assert!(
        matches!(result, Err(ConvertError::LicenseDenied { .. })),
        "got {result:?}"
    );
}

#[test]
fn license_without_conversion_permission_is_refused() {
    let license = LICENSE_OK.replace(
        "\"conversion_allowed\": true",
        "\"conversion_allowed\": false",
    );
    let test_dir = TestDir::new("licenseconv");
    let result = attempt(
        &test_dir,
        &fixture_moc3(&FixtureOptions::default()),
        &fixture_model3(false),
        &fixture_motion3(30.0),
        &license,
    );
    assert!(
        matches!(result, Err(ConvertError::LicenseDenied { .. })),
        "got {result:?}"
    );
}

#[test]
fn missing_license_file_is_refused() {
    let test_dir = TestDir::new("licensemissing");
    let (model_dir, _, out_dir) = test_dir.write_model(
        &fixture_moc3(&FixtureOptions::default()),
        &fixture_model3(false),
        &fixture_motion3(30.0),
        LICENSE_OK,
    );
    let result = convert(&ConversionInputs {
        model_dir,
        license_path: test_dir.root.join("no-such-license.json"),
        out_dir,
    });
    assert!(
        matches!(result, Err(ConvertError::LicenseUnreadable { .. })),
        "a missing license must fail closed, got {result:?}"
    );
}

#[test]
fn malformed_license_is_refused() {
    let test_dir = TestDir::new("licensebad");
    let result = attempt(
        &test_dir,
        &fixture_moc3(&FixtureOptions::default()),
        &fixture_model3(false),
        &fixture_motion3(30.0),
        "{ not json",
    );
    assert!(
        matches!(result, Err(ConvertError::LicenseUnreadable { .. })),
        "got {result:?}"
    );
}

#[test]
fn motion_value_outside_parameter_range_fails() {
    let test_dir = TestDir::new("motionrange");
    let result = attempt(
        &test_dir,
        &fixture_moc3(&FixtureOptions::default()),
        &fixture_model3(false),
        &fixture_motion3(45.0),
        LICENSE_OK,
    );
    assert!(
        matches!(result, Err(ConvertError::MotionValidation { .. })),
        "a keyframe at 45 on a [-30, 30] parameter must fail, got {result:?}"
    );
}

#[test]
fn duplicate_motion_ids_fail() {
    let test_dir = TestDir::new("dupmotion");
    let result = attempt(
        &test_dir,
        &fixture_moc3(&FixtureOptions::default()),
        &fixture_model3(true),
        &fixture_motion3(30.0),
        LICENSE_OK,
    );
    assert!(
        matches!(result, Err(ConvertError::Mapping { .. })),
        "the same motion file under two groups must fail, got {result:?}"
    );
}
