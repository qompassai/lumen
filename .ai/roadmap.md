# Roadmap — lumen (mcp) — v0.3 → v1.0
<!-- bloch:roadmap schema=1 kind=mcp lane=general baseline=0.3.0 target=1.0.0 -->

How to work this roadmap (any agent): run `bloch task start R-xxx --agent <you>` before working a task, `bloch task done R-xxx` when you believe it is complete — done runs every `verify:` command and flips the task green only if all pass; there is no override. `bloch task fail R-xxx --error "..."` records a failure. Fixes learned the hard way land in `.ai/memory/lessons.md` via the inbox/promote flow and are surfaced in every HANDOFF.

## Phase 1 — Baseline truth
- [x] R-101 Build is green from a clean checkout
  - verify: cargo build
  - evidence: 1 verify commands, all exit 0 at 2026-10-29T07:00:48Z
- [x] R-102 Existing tests pass at the baseline
  - verify: cargo test
  - evidence: 1 verify commands, all exit 0 at 2026-10-29T07:01:10Z
- [x] R-103 Repo identity, baseline version, and this roadmap are recorded
  - verify: test -f .ai/roadmap.md
  - evidence: 1 verify commands, all exit 0 at 2026-10-29T07:01:10Z

## Phase 2 — Core complete
- [x] R-201 Tool surface is contracted: every tool has a complete schema and annotations from birth
  - verify: cargo test
  - evidence: 1 verify commands, all exit 0 at 2026-10-29T07:01:31Z
- [x] R-202 Tool count stays within the declared budget (<= 25 tools)
  - verify: cargo test
  - evidence: 1 verify commands, all exit 0 at 2026-10-29T07:01:52Z
- [x] R-203 Typed error and refusal behaviour is implemented in server code, never left to client hints
  - verify: cargo test
  - evidence: 1 verify commands, all exit 0 at 2026-10-29T07:02:13Z

- [x] R-204 B1 canon-record/1 crate absorbed as workspace member crates/lumen-canon (B2, D-292); B1 fixtures behave identically through the absorbed gate
  - verify: cargo test -p lumen-canon
  - evidence: 1 verify commands, all exit 0 at 2026-10-29T07:02:18Z
- [x] R-205 Anchored generation broker: canon_resolve/canon_gate/canon_generate; no unanchored mode; typed refusals (missing_keeper_anchor, anchor_not_adopted, lane_mismatch, record_stale, unanchored_lineage)
  - verify: cargo test -p lumen --test canon
  - evidence: 1 verify commands, all exit 0 at 2026-10-29T07:02:19Z
- [x] R-206 Register bands scored by the gate beside proportion bands (D-312): ink colour, stroke weight, shadow-edge hardness, saturation/contrast percentiles, key-light direction, iris spec, blush
  - verify: cargo test -p lumen --test canon
  - evidence: 1 verify commands, all exit 0 at 2026-10-29T07:02:19Z
- [x] R-207 Workflow-JSON provenance receipts for every generation job (ComfyUI intake item 5), digest-verified; reference conditioning (turnaround + plates, intake item 1) is a required input contract
  - verify: cargo test -p lumen --test canon
  - evidence: 1 verify commands, all exit 0 at 2026-10-29T07:02:19Z

## Phase 3 — Tests and gates
- [x] R-301 Validation tests cover the happy path end to end (half the test split)
  - verify: cargo test
  - evidence: 1 verify commands, all exit 0 at 2026-10-29T07:02:41Z
- [x] R-302 Adversarial tests cover malformed, oversized, and hostile inputs (half the test split)
  - verify: cargo test
  - evidence: 1 verify commands, all exit 0 at 2026-10-29T07:03:03Z
- [x] R-303 Refusal and typed-error behaviour is covered by tests, including oversized payloads
  - verify: cargo test
  - evidence: 1 verify commands, all exit 0 at 2026-10-29T07:03:24Z

## Phase 4 — Hardening
- [ ] R-401 MCP Inspector conformance pass is recorded as evidence
  - verify: cargo test
  - evidence:
- [ ] R-402 Client matrix is verified with at least two real clients
  - verify: cargo test
  - evidence:
- [ ] R-403 Declared-vs-detected access (EFLO) check passes: no tool exceeds its annotations
  - verify: cargo clippy -- -D warnings
  - evidence:
- [ ] R-404 Registry/release packaging builds from a clean checkout
  - verify: cargo build
  - evidence:

## Phase 5 — Docs
- [ ] R-501 README documents install, usage, and verification
  - verify: test -f README.md
  - evidence:
- [ ] R-502 CHANGELOG records every user-facing change
  - verify: test -f CHANGELOG.md
  - evidence:
- [ ] R-503 Tool reference documents every tool, schema, annotation, and error kind
  - verify: test -f README.md
  - evidence:

## Phase 6 — Release v1
- [ ] R-601 Version is bumped to 1.0.0 and the changelog records the release
  - verify: grep -q 'version = "1.0.0"' Cargo.toml
  - evidence:
- [ ] R-602 Clean-environment install and run is verified
  - verify: cargo build
  - evidence:
- [ ] R-603 Release artefacts are built from a clean checkout and checksummed
  - verify: cargo build
  - evidence:
- [x] R-604 G-L1 lumen-side motion evaluator (Torogao Phase F handoff) — 1.0 close-out worker
  - verify: cargo test
  - evidence: 1 verify commands, all exit 0 at 2026-10-29T07:54:00Z
- [x] R-605 G-L2 sprite->3D fidelity metric (silhouette IoU + facial landmark delta) with first measurement — 1.0 close-out worker
  - verify: cargo test
  - evidence: 1 verify commands, all exit 0 at 2026-10-29T07:54:21Z
- [x] R-606 G-L3 EFLO re-measure (real tokenizer) for the 14-tool surface; number on record — 1.0 close-out worker
  - verify: cargo test
  - evidence: 1 verify commands, all exit 0 at 2026-10-29T07:54:41Z
- [x] R-607 canon_convert: existing conversion tools gain the optional canon handle (canon SPEC S1 fourth tool) — 1.0 close-out worker
  - verify: cargo test
  - evidence: 1 verify commands, all exit 0 at 2026-10-29T07:55:02Z
- [x] R-608 Rename/package line audit for the 1.0.0 release (name, server.json, registry record agree) — 1.0 close-out worker
  - verify: cargo test
  - evidence: 1 verify commands, all exit 0 at 2026-10-29T07:55:23Z
