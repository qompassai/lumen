# lumen

**Lux in motu** — light in motion. An MCP server for Aseprite sprite
artistry with Bevy live-world interop, written in Rust (edition 2024,
`rmcp` 3.3). Built for the light-show art pipeline: one operation core
behind three surfaces —

- **MCP tools** for interactive sprite work: sprite operations,
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

## Verified state (2026-10-01, primo)

- Build and clippy clean; **303 tests passed / 0 failed / 8 ignored**
  (suite split between validation and adversarial cases).
- Live interop gate against real Aseprite 1.3.18.6-dev and Bevy 0.18
  BRP (`scripts/interop_check.sh`): **5 passed / 3 failed** — the
  failures are documented premise mismatches between lumen's
  assumptions and the real tools' behavior; see HISTORY.md.

## Layout

- `src/` — operation core: `sprite_ops`, `inspect`, `palette`,
  `text`, `style`, `dream`, `qa`, `export`, `pipeline`, `policy`,
  `bevy`, `doc`, and the `a2a/` surface (`mod`, `task`, `http`,
  `registry`).
- `tests/` — validation and adversarial suites, incl. `interop.rs`
  (live-tool gates, ignored by default).
- `lumen-policy.toml` — the shipped policy; `lumen policy check` and
  `lumen policy replay` are its gates.
