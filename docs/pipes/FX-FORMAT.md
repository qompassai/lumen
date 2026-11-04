# lumen-fx/1 — the lane-neutral effects timeline format

**Status:** Stage 2 definition (D-863). Lane-neutral: this
document is the whole seam between the Qompass pipes (home of
record) and the NTRanch pipes. The lanes share the *format*;
neither reads the other's effect content. A format revision is
a version bump here, reconciled by comparing documents, never
by importing content.

## Shape

One JSON record per effect:

```json
{
  "format": "lumen-fx/1",
  "id": "qompass/<title>-<effect>",
  "lane": "qompass", "origin_lane": "qompass",
  "content_label": "sfw",
  "seed": 20261103,
  "step_hz": 60,
  "duration_steps": 120,
  "uses_inversion": false,
  "intensity": {
    "loop": false,
    "segments": [
      {"kind": "smoothstep", "from": 0, "to": 160,
       "duration_ms": 800},
      {"kind": "hold", "value": 160, "duration_ms": 400},
      {"kind": "smoothstep", "from": 160, "to": 0,
       "duration_ms": 800}
    ]
  },
  "emitters": []
}
```

- `intensity` is the full-field luminance delta track — the
  quantity the photosafety law governs — in **milli-units**
  (1000 = 1.0 of full-field luminance), as a list of sequential
  segments. Segment kinds: `hold` (constant `value`),
  `linear` (`from` → `to`), `smoothstep` (`from` → `to`, the
  s-curve x²(3−2x)), `sine` (`base` + `amp` × sin(2πt /
  `period_ms`)). `loop: true` wraps the track (ambient loops);
  otherwise the last value holds past the end.
- `emitters` are deterministic particle systems, evaluated at
  `step_hz` for `duration_steps`: each emitter is
  `{id, count, origin: [x, y], spread_x, vel_y, vel_x_jitter,
  gravity_y, life_steps}` in integer milli-units (positions in
  milli-pixels). Spawning is staggered and respawning is
  cyclic; every random draw comes from SplitMix64 keyed by
  (seed, emitter, particle, cycle) — no other entropy exists.
- `uses_inversion` declares, as data, that the effect uses no
  luminance-inversion transition (the second half of the
  flash law). The gate refuses a record that declares `true`
  and a record that omits the field.

## Evaluation (the gate of record)

`scripts/pipes/fx_eval.py` evaluates a record **twice,
independently**, and the two runs must agree bit-for-bit:

1. **Replay hash.** Per simulation step, the evaluator records
   `[step, intensity at step, sum of particle x, sum of
   particle y, alive count]` as canonical JSON and chains
   SHA-256 digests (each step's digest covers the previous).
   The final digest is the replay hash. All arithmetic is
   integer fixed-point; the sine table is a literal integer
   table. Same record → same hash, on any machine.
2. **Photosafety flash gate.** The intensity track is sampled
   at 120 Hz over the window `max(duration_ms, 1000)` ms
   (ambient loops: `max(10000, 1000)`), and flashes are counted
   by the extrema walk exactly as shipped in Versemend's
   `game/effects.lua` (design s.8.3): walk direction changes,
   collect extrema, each successive-extremum swing of
   >= 80 milli (0.08 of full-field luminance) counts one half
   flash; the rate is flashes / window-seconds. **Cap: 3
   flashes/second.** The shipped selftest's seal-glow reading
   (0.250 flashes/s) is the correspondence check: this
   evaluator reproduces it from the re-expressed record.

An effect is *fx-verified* when both evaluations agree on the
replay hash, the flash rate is at or under the cap, and the
record carries `uses_inversion: false`. The evaluation writes
a conformance run record like every other pipe gate.
