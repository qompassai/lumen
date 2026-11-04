# R2 consumption fixtures — torogao cast repertoire, as exported

Frozen inputs for the re-homed R2 consumption gate
(`crates/lumen-rig-core/tests/r2_consumption_gate.rs`, D-937 /
D-913 Option B). The gate's first home was torogao's tree,
where it ran the torogao bridge over the cast repertoire
and fed the emitted documents to lumen's evaluator at
runtime; D-913 retired the torogao→lumen code link, so the
documents now cross the wall as data and the proof runs
where the evaluator of record lives.

## What is frozen here

For each cast member (Prima, Yui, Alma):

- `<member>.model.json` — the bridge's model document,
  **byte-exact as exported** (D-JSON + trailing newline),
  lumen token stamped by the writer itself under the token
  ruling. No edits, no repairs.
- `<member>.motion-Daydream.json`,
  `<member>.motion-Greeting.json` — the bridge's motion
  documents, byte-exact as exported.
- `<member>.samples-Daydream.json`,
  `<member>.samples-Greeting.json` — the torogao engine
  sampler's values for every exported curve, on the
  retired gate's own 0.37-frame grid (frames 0.37, 0.74,
  … up to the lane's last keyframe; `[time_seconds,
  engine_value]`, time = frame × animation timestep in
  f32 arithmetic widened to f64). This is the reference
  side of the fidelity sub-check, frozen as data.
- `manifest.json` — machine-readable provenance: source
  commit, exporter, input hashes, and the sha256 of every
  fixture file. The gate verifies all of it before any
  check runs.

## Provenance

- Source: NTRanch/Torogao @
  `9db8a9615e43adc9aed702dc7daf22bd08922235`, exporter
  `crates/torogao-cli/src/rigexport.rs` (`RigExporter::run`
  over `Walk::build`), documents serialized exactly as the
  bridge writes them.
- Inputs: the three `*-repertoire.inx` of the
  `TOROGAO_PHASE_F_FIXTURES` census set
  (`fixtures-orig` in the ParamGaze work area on primo);
  sha256 in `manifest.json`.
- Freeze run 2026-11-04 by a scratch generator example
  (uncommitted, discarded after the freeze), run twice
  with byte-identical output — the exporter is
  deterministic over these inputs.
- Cross-check: all six motion documents are byte-identical
  to the Phase F export generation already documented in
  `../conformance/README.md` (the sha256 list there matches
  these files exactly). The model documents are a newer
  generation than the Phase F copies: 40 vertex values in
  one mesh's keyforms differ from the conformance copies,
  and Alma's model is deliberately **not** the repaired
  one (see below).

## How these differ from `../conformance/`

The conformance fixtures are an earlier export generation,
with two documented edits: their model format tokens were
rewritten to the lumen token, and Alma's model was repaired
(eight keyforms' zero scale components rewritten to 1e-6)
so the motion leg could run. These fixtures carry **no
edits and no repairs**, and the gate's expectations differ
accordingly:

- Prima / Yui: the as-exported model parses; motions sweep
  inside the retired gate's bounds (cubic < 2e-3;
  linear/stepped inside the f32 envelope 4e-6).
- Alma: the as-exported model is **refused LRF-305**
  (keyform scale must be > 0 at deformer
  `rot_MilkStreamL_3106` — the `0a91349` gap). The refusal
  is the expected, green-honest outcome, encoded in the
  gate. If Alma's export ever parses, or refuses with a
  different code or locus, the gate fails loudly: the gap
  moved and the record must move with it.

The samples files are byte-identical across the three
members for each performance — the cast's repertoire lanes
are template-authored (the D-913 census measured the same
identity in its table). Each member's files are kept
separately so a future re-freeze can diverge per member
without a layout change.

## Re-freezing

Re-run the exporter over the same `.inx` inputs at the
torogao commit being frozen, regenerate the samples grid
with the same arithmetic, replace the files and the
manifest together, and re-measure the gate's table. A
re-freeze is a new fixture generation: name the new source
commit in the manifest and expect the gate's measured
table to be re-transcribed into the commissioning report.
Never edit a fixture file or a manifest hash in place.
