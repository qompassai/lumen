# Lumen factory GAP-11-1..4 fixed (branch pax/lumen-factory-gaps, from origin/main 8f69368)

Work order: Owner D-322 (2026-10-29), gaps exposed by Canon-11's
measurement of the shipped factory (now Canon-03 Part B). All work
is in `crates/lumen-factory`; no other crate was touched.

## The 65e3c9a bust contradiction, resolved

Estate memory said a bust parameter merged 2026-10-20 (65e3c9a),
yet Séraphine's torso measured identical to base. Both were true:
65e3c9a (Stage-3 line, `factory/bust-param-20261019` e8ab5e3 merged
into `factory/multiview-identity-20261018`, clone preserved at
`~/workspace/mcp-lumen-migration-20261027/lumen`) carried
`bust_delta_cm` as a **ParameterRecord-only** field —
`ParameterRecord::from_identity` hard-set it to `None` and the
identity record had no such field — so no character could ever
carry a value. That line never landed on main; the shipped factory
had the parameter in no record at all. The fix ported the proven
Stage-3 shaping (lathe-model correction pass, Ramanujan ring
circumference) onto landed main AND wired the parameter through
the identity record as authored data — the step Stage 3 never took.

## Dispositions

- GAP-11-3 (bust): `bust_delta_cm` (bust − underbust circumference,
  cm, fail-closed [5.0, 35.0], never clamped) on IdentityRecord →
  ParameterRecord → ScaledBuild → profile shaping after band
  scaling. Ingest leaves it `None` (a sprite silhouette carries no
  depth — it is authored, not measured). `bust_grade` A=10…G=25 cm.
- GAP-11-4 (widths): `BandScaleSet {head, leg, torso}` as optional
  `width_scales`/`depth_scales` on both records, validated in
  [0.5, 2.0] (the SCALE_MIN/MAX precedent); resolved default exactly
  (1,1,1). Applied to profile rings by height band, joint X spans,
  head radii, limb/outfit tube radii.
- GAP-11-1 (amplitudes): new `src/amplitudes.rs` + checked-in spec
  `assets/seraphine.amplitudes.json` (schema lumen.factory-amplitudes
  v1): per-target max displacement measured from the GLB itself,
  per-channel amplitudes computed from blend.rs fields over the
  canonical head; `AmplitudeTable::check` re-measures and fails
  closed (source-hash + set + per-entry drift at 1e-5 m). CLI:
  `lumen-factory amplitudes --in <glb> [--check <table>]`.
- GAP-11-2 (eyes): outcome (b) — different quantities. Deform/paint
  centre ±0.36 nx is a span share of the head-ellipsoid width; the
  A0/Canon-01 ≈0.70 divides the same centre distance by the
  colour-segmented skin-face width (a narrower, hair-bounded
  denominator). No renumber: `base::EYE_CENTER_NX/NY` is the single
  source of truth, `project::EYE_NX/EYE_NY` re-export it, and the
  mapping + the unmeasured face÷head ratio it depends on are
  documented at the constant. Gates verify the painted atlas eye
  centroids and the lid-field peak against the constant.

## Hard-won facts (do not rediscover)

- **seraphine.glb has 28 morph targets, not 29.** The GLB's own
  targetNames and Canon-11's metrics-3d.json both list 28; the
  "29 named targets" in Canon-03 Part B P-34 is a prose erratum
  (canon-record fix proposed in the task report, not applied —
  STYLES/ was under concurrent restructure).
- The `neutral` morph target is the rest pose: displacement
  exactly 0.0 by construction. Max channel amplitude is jawOpen
  at 0.00736 m.
- The shipped seraphine.glb anchor was built with
  `BuildOptions { ramp: false, toon: true }`; with any other
  option set the rebuild differs legitimately.
- The hair tail's drop line clears the profile's deepest ring:
  under a bust delta the drop rings translate rigidly in Z
  (dz = −0.0088 at delta 25) and the root ring tilts ≤ 2 mm.
  Shaping therefore does NOT leave the hair bit-identical — by
  design, not by leak.
- The head grid is coarse (270 vertices; eye-row front vertices
  at nx = ±0.169/±0.487/±0.747): field-peak gates must be stated
  at grid resolution (argmax == closest-to-centre vertex).
- All new record fields are `Option` with serde default +
  skip_serializing_if, so pre-existing records keep identical
  canonical bytes and hashes (Séraphine's anchor did not move:
  base.glb a326f3d4…, seraphine.glb ce45290a…, identity 00bf2e98…,
  params f36e476a… — verified before and after).

## Validation (sandbox, this tree)

- `cargo build` (crates/lumen-factory): clean.
- `cargo test` (factory): 192 passed / 0 failed (baseline 149;
  +43 new: bust 16, frame 13, amplitudes 11, eye 3).
- lumen-canon 39/0, lumen-rig-core 93/0 (unchanged). The root
  package's tests were not re-run: ort-sys's prebuilt download
  403s in the sandbox, and the root package has no dependency on
  lumen-factory (git status shows changes only under
  crates/lumen-factory/**).
- `cargo fmt --check`: clean. Clippy via `rustup run
  nightly-2026-09-25` (resolved clippy 0.1.100 f7575a9da8),
  `--all-targets -D warnings`: clean.
- Roadmap: R-609…R-612 added unchecked to .ai/roadmap.md; bloch
  flips are Pax's to run at landing (primo's lumen mirror was
  stale at dispatch time, so bloch was not run against it).
