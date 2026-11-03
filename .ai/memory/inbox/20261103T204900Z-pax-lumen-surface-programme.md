# Lumen surface programme (four-video T set, lumen surface) — landed on pax/lumen-t-surface

2026-11-03, Pax subagent build. Branch-level only: no merge to
main, no push (landing goes through the merge walk). Base: main
9b7d775.

## Commits

- 35a4813 Stage 1 (T-2/T-4, D-730/D-732) — converter contract
  clauses: union solver naming (Manifold default for
  watertightness-critical unions), read-back verification as a
  harness clause (ReadBackMismatch carries the census diff),
  edge-manifoldness census (in-memory by index identity; GLB by
  exact-position welding). Golden fixture pair generated on
  Blender 5.2.3: EXACT evaluates the self-intersecting-operand
  union to an EMPTY mesh; MANIFOLD returns the watertight union
  (26/48/72, generator and Rust censuses agree; the generator
  triangulates in-Blender so the exporter does not re-decide
  topology).
- 0842880 Stage 2 (T-6/T-13, D-734/D-742) — head_census
  (lumen.blend-census): duplicate coordinates / lower lid behind
  upper lid / subdivision cage [1], each flagged independently;
  FacePlausibilityInputs distilled; permanent-shadow checklist
  step refuses by name (permanent_shadow_requires_uv_layer)
  without a UV layer.
- 84c526c Stage 3 (T-5, D-733) — docs/factory/HEAD-LINE-PROTOCOL.md:
  named lines, normative construction order, stated check bands.
  Its proving build is recorded in the spec as OWED (supervised
  desk occasion, doubles as the T-7 brow-placement occasion);
  not run, no result claimed.
- f9401b8 Stage 4 (T-12, D-741) — normals-from-proxy/1 converter
  (nearest-face-interpolated vertex-normal transfer, vertex-group
  scoped, rig-safe static-duplicate variant with skin-preservation
  proof) + desk mirror script; primo desk run verified (direct
  applies, direct refuses armature by name, rigsafe builds the
  static duplicate). Golden pins the renormalize-before-blend
  semantics at scope 1/2.
- 67af180 Stage 5 lumen half (T-11, D-740) — face_uv
  (lumen.face-uv-contract v1): five named checks; no layer =
  refused outright. The Torogao half (WGSL SDF material +
  three-number range-debug calibration) landed on the Torogao
  tree's pax/lumen-t-surface branch the same day.

## Gates

Factory tests (sandbox): baseline 231 → 248 → 259 → 268 → 277
passed, 0 failed at every stage; fmt + strict clippy
(-D warnings) clean at every stage. Logs:
~/workspace/lumen-t-surface-20261103/gates/factory-*.log.
Primo confirmation (login shell, same tree): fmt clean, clippy
clean, 277 passed / 0 failed / 0 ignored
(gates/primo-factory-gate.log). Diver diagnostics (primo, live
config, headless): 0 errors / 0 warnings on all 18 changed Rust
files across both trees (gates/diver-diag.log). Torogao half:
torogao-app 54/0/1 baseline → 62/0/1, fmt + clippy clean
(gates/torogao-stage5e.log); WGSL validated by naga; the
sdf_face_sweep evidence render is gates/sdf-face-sweep.png —
Owner-eye evidence, not a verdict.

## Owed

- T-5 supervised proving build (desk occasion; spec §7).
