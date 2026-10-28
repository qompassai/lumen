# Lumen rig-core R2: motion parser + evaluator landed on branch (uncommitted)

Date: 2026-10-28. Branch `pax/lumen-r2-motion` (working tree only;
no commit/push — landing is a separate D-153 step). Gate report:
`~/workspace/lumen-r2-motion-20261028/GATE-REPORT.md`.

What R2 adds in `crates/lumen-rig-core` (0.1.0 → 0.2.0):

- `parse_motion(bytes, &model)` — field-strict §4 parser with
  loader-side LRF checks: binding by model id (LRF-203),
  one-curve-per-parameter across plain + baked curves
  (LRF-202), physics-output refusal (LRF-602), keyframe
  range/times/tangents (LRF-303/501/502), §6.1 baked
  provenance, §4.4 fathom binding.
- `MotionPlayer` — §4.2 evaluator (Newton ≤ 8, 1e-6 s,
  bisection × 24, f64) + §4.3 fade law (weights multiply
  deviation-from-base; interrupted ramps restart from the
  current weight over the full named duration — the SPEC
  pins the restart point, not the duration; recorded here so
  the choice is not rediscovered).
- Tangent law is segment-relative (a key's `tangent_in`
  belongs to the incoming segment): `tangent_in` is legal
  exactly when the previous key's mode is cubic;
  `tangent_out` exactly when its own mode is cubic and it is
  not the last key. Dead tangents are LRF-502.
- Conformance fixtures in `crates/fixtures/conformance/`
  (torogao Phase F cast exports) + `evaluate_motion` example
  as the utilization driver. Trace hashes (480 steps @1/120):
  Greeting 9683dee682d77b01, Daydream 3e0af7a21434ba9b —
  identical sandbox and primo.

Divergences found in the torogao bridge's model exports
(for the bridge/model reconciliation queue, NOT fixed here):
1. Model format token is `torogao-rig-model`; the R1
   validator requires `lumen-rig-model` (already flagged in
   the Phase F handoff). Fixture models carry the lumen
   token (one field changed, documented in the fixtures
   README).
2. Alma's export carries 8 rotation keyforms with scale
   components of exactly 0.0 (INP "feature off" states);
   SPEC §3.4 requires sx, sy > 0 (LRF-305). Fixture repairs
   them to 1e-6, documented; the motion leg never evaluates
   deformers.

Out of R2 scope, still absent (stubbed nowhere): physics
evaluation (§5), expression assets (§4.5), the §6.2 runtime
follower, canonical-emission reconciliation for the bridge's
compact writer (§1.2 binds this crate's own serializer only).

Validation: `cargo test` in crates/lumen-rig-core — 93/0
sandbox and primo; clippy 0.1.99 (b940084d7e, the crate's
1.99.0 pin) `-D warnings` clean both machines; Diver gate at
the primo mirror 0 diagnostics.
