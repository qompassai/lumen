//! Primo-only operation tests: census, sweep, and
//! preview against the real full-rig imports. These
//! need the imports tree, which lives on primo, not in
//! the repo — set LUMEN_TEST_IMPORTS to the imports
//! base (the directory holding prima/ and alma/) to
//! run them; unset, they stand down and say so in the
//! test output rather than failing a machine that
//! cannot host them. The Stage 2 gate demonstration
//! runs the same verbs on primo as its artifact.

use std::path::{Path, PathBuf};

use lumen_engine_tui::ops;

fn imports() -> Option<PathBuf> {
    std::env::var_os("LUMEN_TEST_IMPORTS").map(PathBuf::from)
}

#[test]
fn census_and_sweep_on_prima() {
    let Some(base) = imports() else {
        eprintln!("LUMEN_TEST_IMPORTS unset; primo rig tests standing down");
        return;
    };
    let prima = base.join("prima");
    let census = ops::rig_census(&prima).unwrap();
    assert!(
        census.lines.iter().any(|l| l.contains("parameters: 46")),
        "census lines: {:?}",
        census.lines
    );
    let sweep_path = std::env::temp_dir().join("lumen-engine-tui-sweep-prima.json");
    let sweep = ops::rig_sweep(&prima, &sweep_path).unwrap();
    // The of-record sweep (D-859 line) is 46/46 pass.
    assert!(sweep.ok, "sweep lines: {:?}", sweep.lines);
    let gate = ops::gate_sweep(&sweep_path).unwrap();
    assert!(gate.ok);
}

#[test]
fn preview_renders_prima_at_the_lift() {
    let Some(base) = imports() else {
        eprintln!("LUMEN_TEST_IMPORTS unset; primo rig tests standing down");
        return;
    };
    let scene = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scenes/twobody-lift.scene.ron");
    let dir = std::env::temp_dir().join("lumen-engine-tui-preview");
    std::fs::create_dir_all(&dir).unwrap();
    let out = ops::scene_preview(
        &scene,
        &base,
        "prima",
        5.0,
        &dir.join("preview.png"),
        &dir.join("preview.cells.txt"),
        &dir.join("preview.kitty"),
    )
    .unwrap();
    assert!(out.ok);
    assert!(
        out.lines.iter().any(|l| l.contains("nontransparent")),
        "lines: {:?}",
        out.lines
    );
    assert!(dir.join("preview.png").is_file());
    assert!(dir.join("preview.kitty").is_file());
}
