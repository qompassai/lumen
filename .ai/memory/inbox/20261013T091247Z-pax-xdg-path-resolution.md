# XDG path resolution — bevy_mcp references made rootless

Date: 2026-10-13 (Owner order: "please go Ahead with updating for xdg
paths"). Closes the last open flag in
`20261013T090650Z-pax-script-path-repair.md`.

## What was wrong

- `scripts/interop_check.sh:28` hardcoded
  `EXAMPLE_MANIFEST=/home/phaedrus/bevy_mcp/example_app/Cargo.toml` —
  a machine-specific absolute path; a missing manifest died naming
  only the path, with no hint how to point the script elsewhere.
- `COMPANIONS.md:24,52` named `/home/phaedrus/bevy_mcp` and
  `/home/phaedrus/sprite-tools-build` in prose.

## Repair

- `interop_check.sh`: the manifest now resolves (1) via its
  environment override `LUMEN_EXAMPLE_MANIFEST` — the repo's
  established `LUMEN_*` convention (`LUMEN_BASE_MESH`/`LUMEN_GATE` in
  `patch_scripts.py`, `LUMEN_ALLOW_REMOTE_BRP` in this same script) —
  else (2) under the XDG data dir:
  `${XDG_DATA_HOME:-$HOME/.local/share}/bevy_mcp/example_app/Cargo.toml`,
  computed from the environment, never a `/home/<user>` literal and
  never a root-owned path. The header documents the resolution, and
  the `require_tools` failure now names both the path tried and the
  override variable. Checks and success-path exit semantics are
  unchanged. Note the bevy_mcp checkout is currently absent on this
  machine at every resolution target (neither `~/bevy_mcp` nor the
  XDG location exists); this repair is about resolution, and the
  live Bevy interop proof stays parked on the missing fixture.
- `COMPANIONS.md`: both references rewritten to `$XDG_DATA_HOME/...`
  notation with the `~/.local/share` default spelled out; the
  bevy_mcp paragraph also points at the script and its override.
  Meaning unchanged; the document was not restructured.

## Out of scope, untouched

- `HISTORY.md:141` (dated run record — historical),
  `src/policy.rs`/`tests/policy.rs` fixtures (paths the engine must
  reject), and dated inbox notes. A fresh sweep of `scripts/` and
  top-level `*.md` found no other live `/home/` literals.

## Validation

- `bash -n scripts/interop_check.sh`: OK. `shellcheck` (zero
  warnings) and `shfmt -i 4 -ci` (no diff): clean.
- Grep proof: zero `/home/` hits in either repaired file.
- Fail-closed runs, fixture absent (the current state): default
  resolution dies with "bevy_mcp example_app manifest not found:
  /home/phaedrus/.local/share/bevy_mcp/example_app/Cargo.toml (set
  LUMEN_EXAMPLE_MANIFEST to override)", exit 1; with
  `XDG_DATA_HOME=/tmp/xdg-probe` it names the XDG path; with
  `LUMEN_EXAMPLE_MANIFEST` also set, the override wins.
- Repo gates: `cargo fmt --check` clean; `cargo clippy --all-targets
  -- -D warnings` clean; `cargo test` 383 passed / 0 failed / 8
  ignored — the recorded baseline, unchanged.
