//! Validation tests: the synthetic fixture converts into native
//! assets with the expected structure, values, and report, and
//! the conversion is byte-for-byte deterministic.

mod common;

use common::{
    FixtureOptions, LICENSE_OK, TestDir, fixture_moc3, fixture_model3, fixture_motion3, read_tree,
};
use lumen_rig_core::{AnchorKind, Deformer, Parameter, parse_model};
use lumen_rig_import::{ConversionInputs, convert};
use std::path::Path;

fn convert_fixture(
    test_dir: &TestDir,
) -> (lumen_rig_import::ConversionOutcome, std::path::PathBuf) {
    let moc3 = fixture_moc3(&FixtureOptions::default());
    let (model_dir, license_path, out_dir) = test_dir.write_model(
        &moc3,
        &fixture_model3(false),
        &fixture_motion3(30.0),
        LICENSE_OK,
    );
    let outcome = convert(&ConversionInputs {
        model_dir,
        license_path,
        out_dir: out_dir.clone(),
    })
    .expect("fixture converts");
    (outcome, out_dir)
}

#[test]
fn fixture_converts_and_model_revalidates() {
    let test_dir = TestDir::new("happy");
    let (outcome, out_dir) = convert_fixture(&test_dir);
    let report = &outcome.report;
    assert_eq!(report.model_id, "fixture");
    assert_eq!(report.converted.parameters, 3);
    assert_eq!(report.converted.parts, 2);
    assert_eq!(report.converted.textures, 1);
    assert_eq!(report.converted.deformers, 2);
    assert_eq!(report.converted.meshes, 2);
    assert_eq!(report.converted.physics_groups, 1);
    assert_eq!(report.converted.physics_strands, 1);
    assert_eq!(report.converted.motions, 1);
    assert_eq!(report.converted.motion_curves, 2);
    assert_eq!(report.converted.expressions, 1);
    // The emitted model asset re-parses under lumen-rig-core.
    let model_bytes = std::fs::read(out_dir.join("fixture.model.json")).expect("model file");
    parse_model(&model_bytes).expect("emitted model passes lumen-rig-core validation");
}

#[test]
fn model_structure_matches_the_fixture() {
    let test_dir = TestDir::new("structure");
    let (_, out_dir) = convert_fixture(&test_dir);
    let model_bytes = std::fs::read(out_dir.join("fixture.model.json")).expect("model file");
    let model = parse_model(&model_bytes).expect("model parses");

    // Parameters: ids, ranges, defaults carried verbatim.
    assert_eq!(model.parameters.len(), 3);
    match &model.parameters[2] {
        Parameter::Scalar(scalar) => {
            assert_eq!(scalar.id, "ParamBlink");
            assert_eq!((scalar.min, scalar.max, scalar.default), (0.0, 1.0, 1.0));
        }
        other => panic!("ParamBlink should be scalar, got {other:?}"),
    }

    // Parts forest: child hangs under root.
    assert_eq!(model.parts[0].id, "PartRoot");
    assert_eq!(model.parts[0].parent, None);
    assert_eq!(model.parts[1].parent.as_deref(), Some("PartRoot"));

    // Rotation deformer: the default form (angle key 0) is the
    // base; the lattice keeps all three keyforms.
    let rotation = model
        .deformers
        .iter()
        .find_map(|deformer| match deformer {
            Deformer::Rotation(rotation) if rotation.id == "DeformerRot" => Some(rotation),
            _ => None,
        })
        .expect("rotation deformer present");
    assert_eq!(rotation.base_angle_degrees, 0.0);
    let lattice = rotation.keyforms.as_ref().expect("rotation lattice");
    assert_eq!(lattice.forms.len(), 3);
    assert_eq!(lattice.forms[0].angle_degrees, -10.0);
    assert_eq!(lattice.forms[2].angle_degrees, 10.0);

    // Warp deformer: rest positions come from the default form
    // (ordinal 4: axis keys at both parameters' defaults).
    let warp = model
        .deformers
        .iter()
        .find_map(|deformer| match deformer {
            Deformer::Warp(warp) if warp.id == "DeformerWarp" => Some(warp),
            _ => None,
        })
        .expect("warp deformer present");
    assert_eq!((warp.columns, warp.rows), (2, 2));
    assert_eq!(warp.rest_positions.len(), 9);
    // Form 4 shifts the grid by +0.04 on x.
    assert!((warp.rest_positions[0][0] - (-0.96)).abs() < 1e-6);
    assert!((warp.rest_positions[0][1] - (-1.0)).abs() < 1e-6);
    let warp_lattice = warp.keyforms.as_ref().expect("warp lattice");
    assert_eq!(warp_lattice.forms.len(), 9);
    assert_eq!(warp_lattice.axes.len(), 2);

    // Meshes: default-form vertices, draw order from the group,
    // the clipping relationship resolved by id.
    let quad = model
        .meshes
        .iter()
        .find(|mesh| mesh.id == "MeshQuad")
        .expect("quad");
    assert_eq!(quad.draw_order, 500);
    assert_eq!(quad.clipping_mask_ids, vec!["MeshMask".to_string()]);
    assert_eq!(quad.indices, vec![0, 1, 2, 0, 2, 3]);
    // Default form (ordinal 4) shifts x by +0.04.
    assert!((quad.vertices[0][0] - (-0.46)).abs() < 1e-6);
    assert!((quad.vertices[0][1] - (-0.5)).abs() < 1e-6);
    let mask = model
        .meshes
        .iter()
        .find(|mesh| mesh.id == "MeshMask")
        .expect("mask");
    assert_eq!(mask.draw_order, 400);
    assert!(mask.clipping_mask_ids.is_empty());
}

#[test]
fn physics_group_matches_hand_computed_values() {
    let test_dir = TestDir::new("physics");
    let (_, out_dir) = convert_fixture(&test_dir);
    let model_bytes = std::fs::read(out_dir.join("fixture.model.json")).expect("model file");
    let model = parse_model(&model_bytes).expect("model parses");
    assert_eq!(model.physics.len(), 1);
    let group = &model.physics[0];
    // Effective forces scale from the unit-vector convention.
    assert_eq!(group.gravity, [0.0, -9.8]);
    assert_eq!(group.wind, [0.0, 0.0]);
    // Input: Position window maximum 30 * weight 50%.
    assert_eq!(group.inputs.len(), 1);
    assert_eq!(group.inputs[0].parameter, "ParamAngleX");
    assert!((group.inputs[0].scale_degrees - 15.0).abs() < 1e-6);
    // One strand for the single output; segment lengths and the
    // far-vertex particle values come from the source chain.
    assert_eq!(group.strands.len(), 1);
    let strand = &group.strands[0];
    assert_eq!(strand.anchor.kind, AnchorKind::Deformer);
    assert_eq!(strand.anchor.id, "DeformerWarp");
    assert_eq!(strand.anchor.offset, [0.0, 0.0]);
    assert_eq!(strand.particles.len(), 2);
    assert!((strand.particles[0].length - 10.0).abs() < 1e-6);
    assert!((strand.particles[0].mobility - 0.9).abs() < 1e-6);
    assert!((strand.particles[0].delay - 0.8).abs() < 1e-6);
    assert!((strand.particles[1].mobility - 0.8).abs() < 1e-6);
    let output = strand.output.as_ref().expect("strand output");
    assert_eq!(output.parameter, "ParamAngleY");
    // Scale 10 rad * weight 100% * (180/pi) conversion to degrees.
    assert!((output.scale - 572.9578).abs() < 0.001);
}

#[test]
fn motion_and_expression_assets_carry_curves_and_fades() {
    let test_dir = TestDir::new("motion");
    let (_, out_dir) = convert_fixture(&test_dir);
    let motion: serde_json::Value = serde_json::from_slice(
        &std::fs::read(out_dir.join("motions/idle.motion.json")).expect("motion file"),
    )
    .expect("motion json");
    assert_eq!(motion["id"], "idle");
    assert_eq!(motion["model"], "fixture");
    assert_eq!(motion["frames_per_second"], 30);
    let curves = motion["curves"].as_array().expect("curves");
    assert_eq!(curves.len(), 2);
    let angle = &curves[0];
    assert_eq!(angle["parameter"], "ParamAngleX");
    // Curve-level fades absent -> the motion reference's apply.
    assert_eq!(angle["fade_in_seconds"], 0.25);
    assert_eq!(angle["fade_out_seconds"], 0.5);
    let keyframes = angle["keyframes"].as_array().expect("keyframes");
    assert_eq!(keyframes.len(), 2);
    assert_eq!(keyframes[0]["interpolation"], "cubic");
    assert_eq!(keyframes[0]["tangent_out"], serde_json::json!([0.25, 5.0]));
    assert_eq!(keyframes[1]["value"], 30.0);
    // End key tangent_in = end - c2 = (0.25, 5).
    assert_eq!(keyframes[1]["tangent_in"], serde_json::json!([0.25, 5.0]));
    let blink = &curves[1];
    assert_eq!(blink["keyframes"][0]["interpolation"], "stepped");

    let expression: serde_json::Value = serde_json::from_slice(
        &std::fs::read(out_dir.join("expressions/Smile.expression.json")).expect("expression file"),
    )
    .expect("expression json");
    // Fades absent in the source -> the Cubism default 0.5 s is
    // written explicitly (the native default would be 0.3 s).
    assert_eq!(expression["fade_in_seconds"], 0.5);
    assert_eq!(expression["fade_out_seconds"], 0.5);
    let entries = expression["entries"].as_array().expect("entries");
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0]["parameter"], "ParamAngleX");
    assert_eq!(entries[0]["blend"], "add");
    assert_eq!(entries[0]["value"], 15.0);
    assert_eq!(entries[1]["blend"], "overwrite");
}

#[test]
fn report_accounts_for_unconvertible_constructs() {
    let test_dir = TestDir::new("report");
    let (outcome, _) = convert_fixture(&test_dir);
    let report = &outcome.report;
    let dropped_items: Vec<&str> = report
        .dropped
        .iter()
        .map(|entry| entry.item.as_str())
        .collect();
    // The manifest names a hit area and an EyeBlink group; the
    // physics vertices carry per-vertex acceleration. All three
    // must be accounted, none silently.
    assert!(
        dropped_items.contains(&"hit areas"),
        "dropped: {dropped_items:?}"
    );
    assert!(
        dropped_items.contains(&"model3 groups"),
        "dropped: {dropped_items:?}"
    );
    assert!(
        dropped_items.contains(&"physics vertex acceleration"),
        "dropped: {dropped_items:?}"
    );
    assert!(
        report
            .approximated
            .iter()
            .any(|entry| entry.item == "physics effective forces"),
        "approximated: {:?}",
        report.approximated
    );
}

#[test]
fn conversion_is_byte_identical_across_runs() {
    let test_dir = TestDir::new("golden");
    let (_, out_dir) = convert_fixture(&test_dir);
    let first = read_tree(&out_dir);
    assert!(!first.is_empty());
    // Second run into a different directory over the same inputs.
    let model_dir = test_dir.root.join("model");
    let out_dir2 = test_dir.root.join("out2");
    convert(&ConversionInputs {
        model_dir,
        license_path: test_dir.root.join("license.json"),
        out_dir: out_dir2.clone(),
    })
    .expect("second run converts");
    let second = read_tree(&out_dir2);
    assert_eq!(first.len(), second.len(), "file sets differ");
    for ((path_a, bytes_a), (path_b, bytes_b)) in first.iter().zip(second.iter()) {
        assert_eq!(path_a, path_b);
        assert_eq!(
            bytes_a, bytes_b,
            "output {path_a} is not byte-identical across runs"
        );
    }
    // And the golden output still validates.
    let model_bytes =
        std::fs::read(Path::new(&out_dir).join("fixture.model.json")).expect("model file");
    parse_model(&model_bytes).expect("golden model validates");
}

#[test]
fn texture_records_come_from_the_manifest() {
    let test_dir = TestDir::new("textures");
    let (_, out_dir) = convert_fixture(&test_dir);
    let model_bytes = std::fs::read(out_dir.join("fixture.model.json")).expect("model file");
    let model = parse_model(&model_bytes).expect("model parses");
    assert_eq!(model.textures.len(), 1);
    assert_eq!(model.textures[0].id, "texture_00");
    assert_eq!(model.textures[0].source, "fixture.1024/texture_00.png");
    let quad = model
        .meshes
        .iter()
        .find(|mesh| mesh.id == "MeshQuad")
        .expect("quad");
    assert_eq!(quad.texture, "texture_00");
}

#[test]
fn warp_lattice_keeps_all_forms_in_canonical_order() {
    let test_dir = TestDir::new("lattice");
    let (_, out_dir) = convert_fixture(&test_dir);
    let model_bytes = std::fs::read(out_dir.join("fixture.model.json")).expect("model file");
    let model = parse_model(&model_bytes).expect("model parses");
    let warp = model
        .deformers
        .iter()
        .find_map(|deformer| match deformer {
            Deformer::Warp(warp) if warp.id == "DeformerWarp" => Some(warp),
            _ => None,
        })
        .expect("warp deformer present");
    let lattice = warp.keyforms.as_ref().expect("warp lattice");
    // Axes in binding order (ParamAngleY first, ParamAngleX
    // second), keys ascending on both.
    assert_eq!(
        lattice.axes,
        vec!["ParamAngleY".to_string(), "ParamAngleX".to_string()]
    );
    assert_eq!(
        lattice.keys,
        vec![vec![-30.0, 0.0, 30.0], vec![-30.0, 0.0, 30.0]]
    );
    // Forms sorted into the native canonical order: ascending
    // key-index tuples (the converter reorders from the moc3
    // storage order, where the first-listed axis varies fastest).
    assert_eq!(lattice.forms[0].at, vec![0, 0]);
    assert_eq!(lattice.forms[1].at, vec![0, 1]);
    assert_eq!(lattice.forms[3].at, vec![1, 0]);
    assert_eq!(lattice.forms[8].at, vec![2, 2]);
}

#[test]
fn report_source_summary_accounts_for_the_whole_file() {
    let test_dir = TestDir::new("summary");
    let (outcome, _) = convert_fixture(&test_dir);
    let source = &outcome.report.source;
    assert_eq!(source.moc3_version_byte, 1);
    assert_eq!(source.canvas_pixels_per_unit, 100.0);
    assert_eq!(source.counts.parameters, 3);
    assert_eq!(source.counts.art_meshes, 2);
    assert_eq!(source.counts.keyform_positions, 258);
    // The fixture references every key and every keyform
    // position: nothing in the byte stream is unaccounted for.
    assert_eq!(source.keys_total, 8);
    assert_eq!(source.keys_referenced, 8);
    assert_eq!(source.keyform_positions_total, 258);
    assert_eq!(source.keyform_positions_referenced_end, 258);
    // The license grant is echoed into the report.
    assert_eq!(outcome.report.license.license_name, "CC0-1.0");
}

#[test]
fn physics_group_carries_zero_turbulence_by_default() {
    let test_dir = TestDir::new("turbulence");
    let (_, out_dir) = convert_fixture(&test_dir);
    let model_bytes = std::fs::read(out_dir.join("fixture.model.json")).expect("model file");
    let model = parse_model(&model_bytes).expect("model parses");
    // Cubism has no global turbulence term; the native field is
    // written 0.0 rather than invented.
    assert_eq!(model.physics[0].turbulence_amplitude, 0.0);
}

#[test]
fn unreferenced_motion_files_are_noted_not_converted() {
    let test_dir = TestDir::new("unreferenced");
    let moc3 = fixture_moc3(&FixtureOptions::default());
    let (model_dir, license_path, out_dir) = test_dir.write_model(
        &moc3,
        &fixture_model3(false),
        &fixture_motion3(30.0),
        LICENSE_OK,
    );
    std::fs::write(
        model_dir.join("motions/extra.motion3.json"),
        fixture_motion3(30.0),
    )
    .expect("extra motion");
    let outcome = convert(&ConversionInputs {
        model_dir,
        license_path,
        out_dir,
    })
    .expect("converts");
    assert_eq!(outcome.report.converted.motions, 1);
    assert!(
        outcome
            .report
            .notes
            .iter()
            .any(|note| note.contains("not referenced by the model3 manifest")),
        "notes: {:?}",
        outcome.report.notes
    );
}

#[test]
fn official_sample_model_converts_when_present() {
    // The real-world check: the official free Haru sample, when
    // it has been fetched into the git-ignored scratch dir,
    // converts end to end. Skipped (not failed) when absent —
    // the sample files are never committed.
    let sample =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("scratch/web-samples/Samples/Resources/Haru");
    if !sample.join("Haru.moc3").exists() {
        return;
    }
    let test_dir = TestDir::new("sample");
    let model_dir = test_dir.root.join("Haru");
    copy_tree(&sample, &model_dir);
    let license_path = test_dir.root.join("license.json");
    std::fs::write(&license_path, LICENSE_OK).expect("license");
    let outcome = convert(&ConversionInputs {
        model_dir,
        license_path,
        out_dir: test_dir.root.join("out"),
    })
    .expect("the official sample converts");
    assert_eq!(outcome.report.converted.parameters, 42);
    assert_eq!(outcome.report.converted.parts, 19);
    assert_eq!(outcome.report.converted.deformers, 97);
    assert_eq!(outcome.report.converted.meshes, 84);
    assert_eq!(outcome.report.converted.physics_groups, 4);
    assert_eq!(outcome.report.converted.motions, 6);
    assert_eq!(outcome.report.converted.expressions, 8);
    let model_bytes = std::fs::read(test_dir.root.join("out/Haru.model.json")).expect("model file");
    parse_model(&model_bytes).expect("sample model validates under lumen-rig-core");
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("copy dir");
    for entry in std::fs::read_dir(from).expect("read sample dir").flatten() {
        let path = entry.path();
        let target = to.join(entry.file_name());
        if path.is_dir() {
            copy_tree(&path, &target);
        } else {
            std::fs::copy(&path, &target).expect("copy sample file");
        }
    }
}
