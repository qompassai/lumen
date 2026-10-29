# lumen

**Lux in motu** — light in motion. A cross-platform, cross-media
conversion engine, written in Rust (edition 2024, `rmcp` 3.8.0): one
operation core that bridges media formats — 2D art and sprite formats,
3D models, and game-engine packages — converting between them
deterministically from code-first records, so the same input record
yields the same output on any platform. Aseprite sprites and Bevy
were its first endpoints, not its purpose; the center is the factory
(one identity record, emitted as 2D or 3D) and the named, verified
converter chain between formats — one operation core behind three
surfaces —
<a href="./LICENSE"><img src="https://img.shields.io/badge/License-Apache%202.0-blue.svg" alt="License: Apache 2.0"></a>
<a href="https://doi.org/10.5281/zenodo.23503518"><img src="https://zenodo.org/badge/DOI/10.5281/zenodo.23503518.svg" alt="DOI"></a>

- **MCP tools** for interactive media work: sprite operations,
  inspection, palettes, text, style presets, dream-animation
  automation, QA gates, and export backends (PNG sheets, WASM-4,
  TIC-80, VoidSprite file interop). All paths are project-root
  bounded.
- **An A2A agent surface** (v0.3.x JSON-RPC) for dispatched,
  long-running art jobs, with the trial-then-approval art-director
  loop as a protocol state. See [A2A_DESIGN.md](A2A_DESIGN.md).
- **A deterministic policy layer** (APPA-style, adapted from
  archestra-ai/OpenAPPA): every tool call is checked against
  `lumen-policy.toml` before it executes, decisions are replayable,
  and denials name the rule they violated. See
  [POLICY_DESIGN.md](POLICY_DESIGN.md).

Companion strategy — pair, don't duplicate — is in
[COMPANIONS.md](COMPANIONS.md). How lumen was built, in what order,
and under what governance (including the evidence rule its first
failed build produced) is in [HISTORY.md](HISTORY.md).

## Canon machinery (B2, 2026-10-28, D-292/D-312)

Character generation and derivation run against a single canon
record per character (`canon-record/1`), enforced by four tools
(15 registered total at 1.0.0; the 11 pre-B2 tools are unchanged):

- `canon_resolve` — handle (`canon://<lane>/<character>@<version>`
  + record SHA-256) → record summary. Lane law in server code:
  nsfw records are refused to qompass-lane callers; unclassified
  defaults to nsfw. Records live in the SFW store under the
  project root (`canon/<lane>/<character>.json`); lumen never
  persists an adult record.
- `canon_gate` — measurements → verdict over proportion **and**
  register bands (ink colour, stroke weight, shadow-edge
  hardness, saturation/contrast percentiles, key-light direction,
  iris spec, blush). Drift returns a typed `drift_detected`
  refusal with per-metric deltas. PASS releases an artifact to
  the eye sheet, never to adoption.
- `canon_generate` — anchored generation broker. **There is no
  unanchored mode**: a job requires a canon handle, an anchor
  role, the keeper anchor + style-authority hashes among its
  inputs, and turnaround + plate reference inputs (reference
  conditioning). Jobs anchored outside the Owner-adopted keeper
  set are refused pre-generation. Every job persists a
  workflow-JSON provenance receipt (`canon-receipts/<job_id>.json`,
  digest-verified; tampering is detectable), and any supplied
  outputs are gated before the job returns as admitted.

- `canon_convert` — handled conversion (1.0.0, R-607): a
  project PNG is remapped under the record's fingerprint
  palette (nearest palette colour within its tolerance;
  out-of-tolerance pixels keep their colour and are counted)
  and the result is gated against the record's bands before
  anything is written — drift writes nothing. There is no
  unhandled mode on this tool; the pre-canon conversion tools
  keep their existing behaviour without a handle (SPEC §6).

The format/handle/gate crate is `crates/lumen-canon` (package
`lumen-canon`, lib `canon_record`), absorbed from the B1
reference implementation without re-derivation.

Also at 1.0.0: the Phase F motion evaluator leg is verified
landed in `crates/lumen-rig-core` (`MOTION-EVALUATOR.md`), and
the sprite→3D fidelity metric (silhouette IoU + landmark Δ)
ships in `crates/lumen-factory` with its first measurement on
record (`crates/lumen-factory/FIDELITY.md`). The server
package is `lumen-mcp-server` 1.0.0 (D-298 rename; the library
crate remains `lumen`). EFLO for the 15-tool surface:
`EFLO.md`. Utilization evidence: `UTILIZATION.md`.

## Verified state (1.0.0, 2026-10-29, both machines)

- Workspace **557 tests passed / 0 failed / 8 ignored**
  (including lumen-canon's 39), lumen-rig-core **93 / 0**,
  lumen-factory **149 / 0**; fmt clean; clippy `-D warnings`
  clean (resolved clippy 0.1.100, nightly-2026-09-25 pin) on
  sandbox and primo; Diver gate at the primo mirror.
  (Suite split between validation and adversarial cases.)
- Live interop gate against real Aseprite 1.3.18.6-dev and Bevy 0.18
  BRP (`scripts/interop_check.sh`): **5 passed / 3 failed** — the
  failures are documented premise mismatches between lumen's
  assumptions and the real tools' behavior; see HISTORY.md.

## Layout

- `src/` — operation core: `sprite_ops`, `inspect`, `palette`,
  `text`, `style`, `dream`, `qa`, `export`, `pipeline`, `policy`,
  `bevy`, `doc`, `vrm`, and the `a2a/` surface (`mod`, `task`, `http`,
  `registry`).
- `tests/` — validation and adversarial suites, incl. `interop.rs`
  (live-tool gates, ignored by default).
- `lumen-policy.toml` — the shipped policy; `lumen policy check` and
  `lumen policy replay` are its gates.

## License

This project is licensed under the [Apache License, Version 2.0](./LICENSE).
