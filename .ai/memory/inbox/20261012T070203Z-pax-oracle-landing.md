# Oracle intake landed + two curative fixes — overnight campaign 2026-10-11 (Pax)

## What landed on main (all pushed, ls-remote verified)

1. 23a1c87 fix(build): restored src/pack.rs (333 lines), byte-identical to the
   preserved working-tree copy at ~/archive/2026-10-11-home-cleanup/
   sprite-mcp-preserve/worktree/src/pack.rs
   (sha256 3364b951bd1062fa847d03978aea50e52a8423be825550901178cac92b356079).
   src/lib.rs has declared `pub mod pack` since 8e1552b but the file was never
   committed, so main did not compile (E0583).
2. a7e0a13 fix(tests): corrupt_magic_rejected and truncated_file_rejected in
   src/aseprite.rs now create temp_dir()/lumen-aseprite-tests before writing
   (they relied on another test creating it; both failed on a fresh /tmp).
3. e934962 oracle intake: eb0d0e7 ("Add oracle GLB intake with Pixal3D
   180-degree Y rotation") rebased cleanly (no conflicts) from its 8e1552b
   base onto current main. Files: src/oracle.rs (1289), tests/oracle.rs (369),
   examples/oracle_intake.rs (138), src/lib.rs (+1: pub mod oracle).

## Validation (final combined tree, worktree /tmp/lumen-oracle)

- cargo build -> rc 0
- cargo test -> rc 0 (383 passed / 0 failed)
- cargo clippy -- -D warnings -> rc 0
- cargo fmt --check -> rc 1, PRE-EXISTING drift in 27 files (incl. the
  `pub mod upscale` ordering in src/lib.rs, which predates this landing).
  Every file added by the landing (oracle.rs, tests/oracle.rs,
  examples/oracle_intake.rs, pack.rs) is fmt-clean. A tree-wide cargo fmt
  commit was deliberately NOT folded into this landing; it is a separate
  decision for Matt.

## SCIP

rust-analyzer scip on the landed tree: rc 0, 4,834,469 bytes. Not committed:
this repo has no index.scip pattern. Artifact: /tmp/lumen-index.scip on primo (ephemeral).

## Source of the oracle commit

Recovered from the preserved bundle ~/archive/2026-10-11-home-cleanup/
sprite-mcp-preserve/sprite-mcp.bundle (refs/heads/pax/oracle-intake-20261011).
