# lumen-engine (experiment workspace)

The lumen TUI-engine experiment (SPEC D-893, adopted position D-895):
can one deterministic engine core serve the estate's games across
hosts — a Bevy host today, the TUI host of the proving slice later —
while consuming the puppet evaluator of record (lumen-rig-core)
exactly as the games do?

This workspace is the experiment's home inside the lumen repo. It
does not displace Torogao (D-457 stands unamended until the Stage 4
evidence pack, DP-6), and the engine never becomes an evaluator
(DP-8): rig evaluation stays lumen-rig-core's, always.

## Crates

- `lumen-engine-core` — the deterministic runtime core:
  fixed-step clock, the scene/state/dialogue/effect/audio record
  family (strict RON), pose/placement track sampling, the two-body
  contact seam, and the dialogue/effect/audio sub-runtimes.
  No rendering, no I/O beyond record parsing, `forbid(unsafe_code)`.
- `lumen-engine-bevy` — the minimal Bevy host binding: a headless
  Bevy App whose Update schedule steps the core; puppet staging,
  C2 instruments, and rasterization bind the evaluator of record
  to the engine's records.

## Scenes

- `scenes/twobody-lift.scene.ron` — the standing 12-second
  two-body lift protocol (Prima lifts Alma, D-847/D-859/D-875) as
  an engine scene record. Its tune block pins the D-875 reads of
  record as gate values.

Run it (rig imports are the D-859 full-rig import directories):

```
cargo run -p lumen-engine-bevy --bin lumen-engine-host -- \
  scenes/twobody-lift.scene.ron <imports-base> <out-dir> \
  [--reference <verdict-pack-v3-dir>]
```

Outputs: `reads.json`, `summary.json` (the verdict-pack shape),
`evidence.json` (tune check, runtime snapshot/restore probe,
frame deltas), key + worst frames, `state-at-<step>.ron`,
`runtime-events.log`.
