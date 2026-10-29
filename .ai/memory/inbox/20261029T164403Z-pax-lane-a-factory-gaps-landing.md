# Factory-gaps (D-322) landed to main — Lane A item 4, 2026-10-29

The `pax/lumen-factory-gaps` branch (GAP-11-3 bust, GAP-11-4 frame
scales, GAP-11-1 amplitudes, GAP-11-2 eye reconciliation) was rebased
onto main 934058d in the sandbox (head 9adf78b; diff unchanged: 17
files, +2903/−40, `crates/lumen-factory/**` + `.ai/` only) and landed
after a full primo gate battery on the exact tree.

Gate evidence (primo, login shell, tree pin nightly-2026-09-25;
cargo 1.100.0-nightly 98a09e7e7, clippy resolved 0.1.100 f7575a9da8):

- `cargo build --workspace --all-targets` — exit 0
- `cargo test --workspace --no-fail-fast` — 596 passed / 0 failed (25 suites)
- `cargo clippy --workspace --all-targets -- -D warnings` — exit 0
- Diver gate (headless nvim, live diver config, night-program
  diver-gate.lua): the 14 changed .rs files each settled at
  diagnostics=0 with bacon_ls + rust-analyzer attached — 14/14 PASS.
- Bloch R-609, R-610, R-611, R-612: started, then `task done` — each
  green with its verify command exit 0 on primo.

Affected paths: `crates/lumen-factory/src/{amplitudes,base,identity,
lib,main,params,project}.rs`, `crates/lumen-factory/tests/{amplitudes,
bust,frame}_*.rs`, `eye_reconciliation.rs`, `.ai/roadmap.md`.

Note for the next session: GAP-11-2's eye reconciliation is narrowed —
the ±0.36 nx figure is the head-ellipsoid share (≈0.70 is the
skin-face share); the unmeasured face÷head ratio is the PARITY-2c
remainder, not a closed quantity.
