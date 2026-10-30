# RENDER-1 + BIND-1 landing: merge, curative rig-import fix, stale-cache finding

Date: 2026-10-30 (landing pass, Pax). Scope: local mains only, no pushes
(origin main verified unmoved at 16e4cfc before and after).

## What landed

- Merge a248af3: pax/lumen-render1 (60983eb, based on origin main 16e4cfc)
  into local main e3dba39 (moc3 landing + TEXTURES_MAX 256 via aee0a96).
  Merge tree proven against a `git merge-tree` simulation before the real
  merge; the only conflict was `.ai/roadmap.md`.
- Roadmap R-ID collision resolved in the merge: the render branch had
  authored BIND-1 as R-614, but R-614 was already landed on main as the
  conformance fixture re-seed. BIND-1 renumbered to R-616 with a note in
  the roadmap; R-615 (RENDER-1) kept. Branch evidence timestamps preserved.

## Curative fix (landing commit after the merge)

Primo gates caught a real composition break the sandbox gate missed:
`lumen-rig-import` constructs `lumen_rig_core::Part` literals without the
BIND-1 `translation_keyforms` field at four sites —
`src/convert.rs` (moc3 converter), `src/convert/inp.rs` (INP converter),
and two fixture literals in `tests/export.rs`. rustc E0063 ×4.
Fix: `translation_keyforms: None` at all four sites. None is the
pre-amendment meaning (field is `Option`, skip-serializing), so converter
output is byte-identical to before. Also committed: rig-import's
standalone `Cargo.lock`, which the build completed from the members-only
stub the moc3 landing carried.

## Stale-cache finding (why the sandbox gate missed it)

The sandbox rig-import run reported 62 passed on the un-fixed merged tree.
Forcing a rebuild (`touch crates/lumen-rig-core/src/model.rs`) reproduced
E0063 in the sandbox immediately. The standalone-workspace target cache
for rig-import considered the path-dep rig-core fresh across the merge.
Lesson: for standalone-workspace crates wired by path to a sibling crate,
a gate run after a merge must force the sibling's rebuild (or clean the
crate's target) before its numbers count. Primo's colder cache is what
caught this — the two-machine gate order earned its keep.

## Validation

- Sandbox (after fix): rig-core 111, rig-import 62, factory 192, canon 39;
  clippy 0.1.100 (nightly-2026-09-25) `-D warnings` clean; fmt clean.
- Primo: same numbers after the fix propagated (see landing report);
  Diver gate on all touched .rs files.
- Neutral traces unchanged: Greeting 9683dee682d77b01,
  Daydream 3e0af7a21434ba9b (motion_conformance green on both machines).
- Aka check on the composed tree: torogao export-rig on Aka.inx →
  dropped 98 (pre-BIND-1: 193), 57/100 parts carry translation_keyforms;
  the 76-texture model now passes lumen-rig-core `parse_model`
  (TEXTURES_MAX 256) — previously refused at the 64 cap.
