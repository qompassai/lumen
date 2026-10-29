# Motion evaluator — Phase F handoff close-out (G-L1)

The Torogao Phase F handoff
(`torogao-rust-port-20261027/phase-f-gates/LUMEN-MOTION-HANDOFF.md`)
named four things R2 had to build lumen-side. All four landed
with the R2 commit (`1f5682e`, "land lumen-rig-core R2 — motion
parser, §4.2 evaluator, §4.3 fade law") and are re-verified at
the 1.0.0 close-out (2026-10-29):

| Handoff item | Where | State |
|---|---|---|
| 1. §4 parser, field-strict per §1.1, LRF checks loader-side | `crates/lumen-rig-core/src/motion.rs`, `validate_motion.rs` | landed; adversarial suite covers each refusal (unknown field, LRF-202/203/303/501/502/602, fps) |
| 2. §4.2 evaluator exactly as specified (Newton ≤ 8 from segment fraction, 1e-6 s tolerance, bisection × 24, f64) + §4.3 fade law (weights multiply deviation-from-base) | `crates/lumen-rig-core/src/evaluate.rs`; example `examples/evaluate_motion.rs` | landed |
| 3. Model–motion binding: motion whose `model` ≠ loaded model id refused (LRF-203) | parser binding in `motion.rs` | landed, tested |
| 4. Conformance fixtures from the bridge's cast-repertoire exports (Prima/Yui/Alma × Greeting/Daydream) | `crates/fixtures/conformance/` (provenance + source sha256 in its README) | landed; `motion_conformance.rs` loads, binds, and evaluates all six end-to-end through `parse_model` → `parse_motion` → `MotionPlayer` |

Acceptance, per the handoff's own criteria: the evaluator runs
the bridge's motion format against lumen-rig-core with the
parity fixtures green. At the 1.0.0 gate the rig-core suite is
**93 passed / 0 failed** on both machines (sandbox + primo),
including the six-fixture conformance test (each motion: 10
curves, 60 fps, `loop: false`, every evaluated value inside its
parameter's declared range over a full fixed-step performance
with fade-out) and the cubic/fade coverage in
`crates/fixtures/full.motion.json`.

Open items carried from the handoff, unchanged (not motion-leg
blockers, recorded there and here): model format-token
divergence (`torogao-rig-model` vs `lumen-rig-model` — motions
bind by model id, not token); bridge emission layout vs §1.2
canonical emission (parsers accept any valid JSON layout);
loop-flag provenance (INP carries none; `loop` stays a player
argument); §4.4 fathom binding and §6 audio reference not
emitted torogao-side.
