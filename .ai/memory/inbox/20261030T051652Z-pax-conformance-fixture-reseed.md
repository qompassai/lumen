# Conformance fixtures re-seeded from native exporter output

2026-10-30 (Pax subagent, branch `pax/lumen-fixture-reseed` off
origin/main 16e4cfc).

The `crates/fixtures/conformance/` fixtures no longer carry the
two R2-era lumen-side conversions (model token rewritten
`torogao-rig-model` → `lumen-rig-model`; Alma's zero keyform
scales rewritten to a literal 1e-6). The torogao `export-rig`
bridge was fixed at the source 2026-10-29 (native
`LUMEN_MODEL_FORMAT_TOKEN`; exporter floors non-positive
keyform scale components at 1e-6 f32 with conversion-report
notes), and the fixtures were re-seeded byte-for-byte from its
regenerated output.

Verified diff shape vs the old fixtures: motions — 0 bytes
(all six already byte-identical; sha256 unchanged from the
original R2 seeding). Models — Prima/Yui 0 semantic leaves
(their fixture copies already carried the converted token);
Alma exactly 14 leaves, all
`deformers[*].keyforms.forms[0].scale[*]`, literal 1e-6 →
9.999999974752427e-07 (the f32 value the exporter emits).

Validation (sandbox; toolchain nightly-2026-09-25, clippy
0.1.100):

- `cargo test` in crates/lumen-rig-core: 93/0/0 before and
  after (motion_conformance 3/3 included).
- `cargo test` in crates/lumen-factory: 192/0/0 before/after.
- `cargo test -p lumen-canon` from root: 39/0/0 before/after.
- `cargo run --example evaluate_motion` on all six pairs:
  Greeting trace 9683dee682d77b01, Daydream 3e0af7a21434ba9b,
  identical before and after, all three rigs.
- clippy `-D warnings` clean (rig-core, factory, canon);
  `cargo fmt --check` clean.
- Root workspace `cargo test --workspace` cannot build in the
  sandbox before or after: ort-sys fails to download prebuilt
  binaries (cdn.pyke.io answers 403 through the egress proxy).
  Environmental, pre-existing; the server suites' gate venue
  remains primo.
