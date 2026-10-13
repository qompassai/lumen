# Overnight audit 2026-10-12 (Pax) — gates green, current.md pending list reconciled

Commission: Matt overnight blanket authorization (2026-10-12 20:03 PDT) —
audit lumen, close clearly-defined leftovers, commit/push as needed.

## Repo state verified on primo (~/.GH/Qompass/lumen)

- main == origin/main == f524c450b373b6a09cc348f781c7bc96bac2c45a
  (ls-remote verified); working tree clean; no stash.
- The old ~/sprite-mcp path no longer exists; this mirror is the repo.
  Skill file path corrected in the sandbox skill (not a repo change).
- Local branch pax/oracle-intake-20261011 remains, pointing at the landed
  commit e934962 (harmless; removable at Matt convenience).

## Gates run 2026-10-12 (this tree, primo)

- cargo fmt --check: OK (the 27-file drift noted in the 20261012T070203Z
  inbox entry was closed by f524c45, tree-wide cargo fmt).
- cargo build: OK.
- cargo test: 383 passed / 0 failed / 8 ignored (the 8 ignored are the
  live interop tests in tests/interop.rs).
- cargo clippy --all-targets --all-features -- -D warnings: rc 0.

## Live interop (scripts/interop_check.sh prerequisites)

- Aseprite half RUN LIVE: cargo test --test interop -- --ignored aseprite
  -> 3 passed / 0 failed, including
  adv_live_aseprite_truncated_file_is_recovered_but_lumen_refuses_it.
  The current.md live-interop item 1 (Aseprite truncated-file behavior)
  is CLOSED with live evidence.
- Bevy half NOT RUNNABLE: the fixture /home/phaedrus/bevy_mcp (example_app)
  no longer exists anywhere on primo (find-verified, maxdepth 4-5, home +
  .GH trees). Port 15702 is free; /usr/bin/aseprite is present. The 5 live
  Bevy tests stay ignored-by-design until the fixture is restored.
  current.md items 2 (BRP malformed-query) and 3 (registry schema
  crate-filter) are therefore UNVERIFIED, not passing and not failing.

## current.md (2026-10-04) Pending list — item-by-item reconciliation

1. CUDA execution restore/prove — PARTLY STALE. src/upscale.rs wires the
   ort CUDA execution provider with CPU fallback (line 86). No .onnx model
   fixture exists in the repo, so a CUDA proof run cannot be produced from
   repo artifacts; CPU-side upscale logic is covered by 4 in-module tests.
   A real CUDA prove needs a Real-ESRGAN .onnx artifact placed on primo.
2. export_tag_animations MCP registration — CLOSED (stale note).
   Shipped as the export_tag op in 8e1552b (2026-10-03): registered in
   src/a2a/registry.rs:447-450, allowed in src/policy.rs:78, covered by
   4 tests in tests/export_pipeline.rs (2 validation, 2 adversarial).
   current.md was written 2026-10-04, after the landing.
3. LeakyReLU + Resize lowering — OBSOLETE as written. The ONNX upscaler
   (8e1552b) executes ops inside ONNX Runtime (ort); there is no
   lumen-side op-lowering layer in the tree (no LeakyReLU/Resize handling
   in src/), and none is needed for the ort design.
4. Pinned uv environment — NOT ACTIONABLE in this repo: the workspace is
   pure Rust; no pyproject.toml, uv.lock, or .python-version exists here.
5. Protobuf conflict repair — NO EVIDENCE in tree: zero protobuf/prost
   references in Cargo.toml or src/. Nothing to repair in this repo.

## Templates discrepancy (needs Matt call)

The lumen skill states the ratified emotion templates live in this repo
at templates/emotions.json / templates/emotions.md. This repo has no
templates/ directory, and no emotions.json exists anywhere on primo.
The only lumen-side copy found is the sandbox research workspace
~/workspace/lumen-2d3d/emotion-templates/ (with the sculpt-delta Python
tooling). Which copy is canonical, and whether it should be imported
into this repo, is Matt decision — not closed by this audit.

## Sandbox lumen-2d3d TODO.md evaluation queue

Pixal3D item is DONE (trial passed 2026-10-07; promoted to primary oracle
arm by Matt decision; intake landed as src/oracle.rs). The remaining
unchecked items (TRELLIS.2 GGUF verification, TRELLIS.2 reference,
Hunyuan3D-2mv, Hunyuan3D-Omni, TripoSG/TripoSR baselines) are open-ended
research evaluations in a queue whose purpose the Pixal3D result already
fulfilled; they are Matt research backlog, not defined code leftovers.
No GPU hours were spent on them in this audit.

## Sandbox copies

- ~/workspace/lumen (sandbox) holds only Oct-1 design docs (A2A_DESIGN,
  COMPANIONS, POLICY_DESIGN); not a git repo; stale next to this repo.
- No lumen clone exists under ~/workspace/repos/.

## What this audit changed

- Nothing in code: no defined, agent-doable code leftover was found open.
- This inbox note (docs only).
- Sandbox skill path fix (~/workspace/skills/lumen-sprite-to-3d/SKILL.md).

Validation commands: cargo fmt --check; cargo build; cargo test;
cargo clippy --all-targets --all-features -- -D warnings;
cargo test --test interop -- --ignored aseprite; git ls-remote origin main.
