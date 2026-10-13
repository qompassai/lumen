# Script path repair — retired sprite-mcp paths removed

Date: 2026-10-13 (Matt order: "please repair and when possible use
rootless xdg conventions instead of hard coding"). Closes the flag left
in `20261013T085742Z-pax-emotion-templates-adoption.md`.

## What was wrong

- `scripts/patch_scripts.py` hardcoded its two patch targets:
  `BASE = "/home/phaedrus/sprite-mcp/scripts/lumen_base_mesh.py"` and
  `GATE = "/home/phaedrus/sprite-mcp/scripts/lumen_gate.py"` — a retired
  repo layout and a machine-specific absolute path.
- `scripts/lumen_detail_prep.py` docstring described outputs as going
  "into the sprite-mcp models/ directory" (the code always took the
  directory as an argument; only the prose was stale).
- `templates/emotions.md` claimed `emotions.json` was "also installed
  in the sprite-mcp repo"; since the adoption (b0304cf) this repo is
  the canonical home, so the clause was removed.

## Repair

- `patch_scripts.py`: targets are pipeline scripts that do not live in
  this repo, so each now resolves (1) via its environment override
  (`LUMEN_BASE_MESH` / `LUMEN_GATE`), else (2) as a same-named file
  beside the script itself (`Path(__file__).resolve().parent`). Both
  targets are resolved and their expected blocks validated before any
  write; a missing target or block stops the run with a named error
  and every file left untouched (the original wrote the base file
  before discovering a gate problem, and its guards were `assert`s
  that `python -O` would strip). The four payload blocks
  (NEW_BLOCK / NEW_GATE / OLD_BLOCK / OLD_GATE) are byte-identical to
  the adopted versions — they must match the target files' text
  exactly. No XDG state was involved: these scripts keep no user-level
  data, config, or cache, so env override + script-relative resolution
  is the whole mechanism. No new dependencies; stdlib only.
- `lumen_detail_prep.py`: one docstring line now reads "Outputs (into
  <models_dir>):". No code change.
- Inspected, no change needed: `lumen_detail.py`,
  `lumen_pack_sheet.py`, `lumen_render_sheet.py`,
  `lumen_sculpt_deltas.py` (all paths arrive as CLI arguments;
  `lumen_gate.py` / `lumen_base_mesh.py` appear only as bare names in
  prose describing design parity), and `templates/emotions.json`
  (no filesystem paths; its `game/src/...` mentions are prose
  references to the light-show repo layout).

## Same class, outside the adopted set — reported, not fixed

- `scripts/interop_check.sh:28` — `EXAMPLE_MANIFEST=/home/phaedrus/bevy_mcp/example_app/Cargo.toml`.
- `COMPANIONS.md:24,52` — `/home/phaedrus/bevy_mcp`, `/home/phaedrus/sprite-tools-build`.
- `HISTORY.md:141` — dated record of a past run location; historical.
- `src/policy.rs` / `tests/policy.rs` sprite-mcp strings are policy
  test fixtures (paths the engine must reject), not references.

## Validation

- `python3 -m py_compile` on all six scripts: OK.
- Grep proof over `scripts/*.py` + `templates/`: zero hits for
  `sprite-mcp` or `/home/`.
- Functional fixture test of `patch_scripts.py` (fixtures built from
  the script's own block constants): env-override patch PASS,
  sibling-default patch PASS, missing target fails closed naming the
  env var PASS, missing block fails closed with files untouched PASS,
  identical result under `python3 -O` PASS.
- `ruff check` on the two changed Python files: clean. (`ruff format`
  not applied: the repo carries no ruff/pyproject config and the
  adopted scripts uniformly predate such a gate.)
- Repo gates: `cargo fmt --check` clean; `cargo clippy --all-targets
  -- -D warnings` clean; `cargo test` 383 passed / 0 failed / 8
  ignored — the recorded baseline, unchanged.
