//! Operation-core contract tests: the lane gate, the
//! Describe -> Propose -> Freeze spine, and the flash
//! gate over the workspace's own scene record. These
//! run anywhere; the primo-only rig tests live in
//! `ops_primo.rs` behind an env gate.

use std::path::{Path, PathBuf};

use lumen_engine_tui::ops;

fn temp_project(test: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "lumen-engine-tui-test-{}-{}",
        std::process::id(),
        test
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("scenes")).unwrap();
    let scene_src =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenes/twobody-lift.scene.ron");
    std::fs::copy(&scene_src, dir.join("scenes/twobody-lift.scene.ron")).unwrap();
    dir
}

fn scene_rel() -> &'static str {
    "scenes/twobody-lift.scene.ron"
}

#[test]
fn describe_refuses_nsfw_before_any_write() {
    let root = temp_project("nsfw");
    let err = ops::author_describe(&root, "run-a", Some("nsfw"), "x").unwrap_err();
    assert_eq!(err.code, "lane_mismatch");
    assert!(
        !root.join("runs").exists(),
        "a refused describe must leave no artifact, not even the runs dir"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn describe_refuses_unclassified() {
    let root = temp_project("unclassified");
    let err = ops::author_describe(&root, "run-a", None, "x").unwrap_err();
    assert_eq!(err.code, "lane_unclassified");
    let err = ops::author_describe(&root, "run-a", Some("draft"), "x").unwrap_err();
    assert_eq!(err.code, "lane_unclassified");
    assert!(!root.join("runs").exists());
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn spine_describe_propose_freeze() {
    let root = temp_project("spine");
    let out = ops::author_describe(
        &root,
        "lift-tweak",
        Some("sfw"),
        "retime the second dialogue line",
    )
    .unwrap();
    assert!(out.ok);
    let out = ops::author_propose(
        &root,
        "lift-tweak",
        scene_rel(),
        "Okay — lifting.",
        "Okay — lifting now.",
    )
    .unwrap();
    assert!(out.ok);
    // The frozen scene is intake: it must still load.
    let out = ops::author_freeze(&root, "lift-tweak").unwrap();
    assert!(out.ok);
    let frozen = root.join("frozen/lift-tweak.scene.ron");
    let scene = ops::load_scene(&frozen).unwrap();
    assert_eq!(scene.id, "twobody-lift");
    // Stage order is enforced: a second freeze refuses.
    let err = ops::author_freeze(&root, "lift-tweak").unwrap_err();
    assert_eq!(err.code, "stage_order");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn propose_refuses_a_nonunique_edit() {
    let root = temp_project("nonunique");
    ops::author_describe(&root, "run-b", Some("sfw"), "x").unwrap();
    // "ParamArmL" occurs many times in the scene record.
    let err =
        ops::author_propose(&root, "run-b", scene_rel(), "ParamArmL", "ParamArmX").unwrap_err();
    assert_eq!(err.code, "edit_not_unique");
    assert!(
        !root.join("runs/run-b.proposed.scene.ron").exists(),
        "a refused proposal writes nothing"
    );
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn propose_refuses_an_invalid_result() {
    let root = temp_project("invalid");
    ops::author_describe(&root, "run-c", Some("sfw"), "x").unwrap();
    // Breaking the record's syntax must refuse, typed.
    let err =
        ops::author_propose(&root, "run-c", scene_rel(), "steps: 720", "steps: (720").unwrap_err();
    assert_eq!(err.code, "record_invalid");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn flash_gate_reads_zero_strobes_on_the_record_scene() {
    let scene = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenes/twobody-lift.scene.ron");
    let out = ops::gate_flash(&scene).unwrap();
    assert!(out.ok, "lines: {:?}", out.lines);
    assert!(
        out.lines.iter().any(|l| l.contains("0 strobe keys")),
        "lines: {:?}",
        out.lines
    );
}

#[test]
fn gate_compare_accepts_identical_directories() {
    let base = temp_project("compare");
    for name in ["run-a", "run-b"] {
        let dir = base.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("summary.json"), "{\"a\": 1}\n").unwrap();
        std::fs::write(dir.join("reads.json"), "[]\n").unwrap();
    }
    let out = ops::gate_compare(&base.join("run-a"), &base.join("run-b")).unwrap();
    assert!(out.ok, "lines: {:?}", out.lines);
    // And a divergence is a FAIL with the field named.
    std::fs::write(base.join("run-b/summary.json"), "{\"a\": 2}\n").unwrap();
    let out = ops::gate_compare(&base.join("run-a"), &base.join("run-b")).unwrap();
    assert!(!out.ok);
    assert!(out.lines.iter().any(|l| l.contains("summary.a")));
    let _ = std::fs::remove_dir_all(&base);
}

#[test]
fn project_open_reads_the_workspace_scene() {
    // The crate's own scenes dir stands in as a project
    // root: it has scenes/*.scene.ron by construction.
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let imports = Path::new("/nonexistent-imports");
    let out = ops::project_open(&root, imports).unwrap();
    assert!(
        out.lines.iter().any(|l| l.contains("scene twobody-lift")),
        "lines: {:?}",
        out.lines
    );
    // Imports are absent here, so the open reports the
    // project honestly as not-ok rather than hiding it.
    assert!(!out.ok);
    let _ = Path::new("/nonexistent-imports");
}
