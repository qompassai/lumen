# Rig runtime R1 landed (Pax, 2026-10-26)

Owner ruling 2026-10-25 (build + name): the lumen rig runtime format spec
and validating parser landed on branch `rig-runtime/r1` (worktree
~/.local/share/lumen/rig-r1-land; merge to main follows once the
vivienne-proof/ingest worker releases the primary tree).

Paths added:
- crates/lumen-rig-core/ — the crate (src: lib, limits, error, model, raw,
  validate; SPEC.md beside it is the normative format spec, 867 lines).
- crates/fixtures/ — model fixtures; the crate tests resolve them as
  ../fixtures relative to the crate manifest, preserved from the draft.

Landing adaptations (only these): Cargo.toml gained an empty [workspace]
table (the lumen-factory pattern: the repo root package would otherwise
claim the crate), and the description lost its WORKING TITLE suffix (the
name lumen-rig-core was ruled final 2026-10-25).

Gates on primo (rustup toolchain 1.99.0, the crate pin):
- cargo test: 40 passed / 0 failed (28 adversarial + 12 validation).
- cargo clippy --all-targets -- -D warnings: clean.
- cargo fmt --check: clean.
- Diver gate: headless Neovim with the live config in the crate tree —
  bacon_ls and rustana_ls both attached; diagnostics 0/0 across src and
  tests, stable over 11 polls (~60 s). NOTE: the gate must run under a
  login shell (bash -lc) or bacon-ls is not on PATH (~/.cargo/bin) and
  silently never starts.

Next: R2 (motion/physics evaluation parsing; LRF-501/502/602 reserved).
