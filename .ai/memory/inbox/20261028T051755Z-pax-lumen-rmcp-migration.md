# Lumen rmcp modern-era migration (2026-10-27)

## Finding

Lumen's MCP server migrated from dual-era rmcp 3.5.0 to the fleet
standard: rmcp =3.8.0, modern era ONLY (protocol 2026-07-28), per D-113 /
D-143. A minimal framing guard (`src/guard.rs`, lib crate) owns the
stdio boundary: `initialize` in any version is refused with -32022
UnsupportedProtocolVersionError {requested, supported:["2026-07-28"]};
malformed JSON answered -32700 parse error; non-object frames -32600;
inbound notifications dropped unexecuted; request lines bounded at
4 MiB; ordering tracker keeps guard frames behind forwarded responses.
Everything else forwards verbatim to the rmcp SDK; the handler pins
supported_protocol_versions() to [2026-07-28]. Crate version 0.1.0 -> 0.2.0.

## Evidence

- tools/list byte-identical before/after: 7 tools, same order
  (bevy_discover, sprite_info, style_apply, style_list, validate_atlas,
  vrm_grade, vrm_inspect), canonical 3046 chars both sides, full-JSON
  equality.
- Conformance: discover supportedVersions exactly ["2026-07-28"];
  initialize at 2025-11-25 and at 2026-07-28 both refused -32022 with
  correct data and the server survives; legacy-_meta request refused
  -32022 (pre-migration baseline: connection EOF, server died).
  Inspector CLI 2.11.0: tools-modern exit 0, tools-strict-modern exit 0
  (0 errors; 3 pre-existing type-union schema warnings on the
  Option-field tools persist unchanged under rmcp 3.8), legacy runs
  exit 1 "Unsupported protocol version".
- Tests: 424 listed; full runs on BOTH sandbox and primo mirror:
  416 passed / 0 failed / 8 ignored, 17 suites. (Two ONNX upscale
  model-download tests failed early in the session in both the
  baseline and migrated trees — an environment artifact of the then
  missing ONNX runtime cache; with the cache in place they pass on
  both machines. An early in-session aggregate of 518 passed was a
  double count across repeated suite output and is superseded by
  these single-run figures.)
  New tests: +9 (4 guard unit, 5 era battery in tests/era.rs); the three
  stdio harnesses (policy.rs, stdio_style.rs, vrm.rs) modernized to
  discover + per-request _meta, assertions unchanged.
- Clippy (resolved 0.1.100 f7575a9da8, nightly-2026-09-25) --all-targets
  -D warnings: clean. cargo fmt --check: clean.
- Utilization (D-116): real Light Show art driven through the migrated
  server — sprite_info, validate_atlas, style_list (16 presets),
  style_apply demo_toon graded a real .lumen.json document; graded
  output hash differs from a pristine rebuild (conversion effect
  proven). Evidence in the migration workspace.
- Primo mirror checksum-identical (sha256 manifest); primo-built binary
  re-probed conformant; Diver gate per-file diagnostics recorded in the
  gate report.
- COMPLIANCE.md added at repo root from the canonical checklist.

## Affected paths

Cargo.toml, Cargo.lock (rmcp 3.5.0 -> 3.8.0), src/guard.rs (new),
src/lib.rs, src/main.rs, tests/era.rs (new), tests/policy.rs,
tests/stdio_style.rs, tests/vrm.rs, COMPLIANCE.md (new).

## Validation commands

- cargo test (416 passed / 0 failed / 8 ignored, 424 listed)
- cargo clippy --all-targets -- -D warnings (clean)
- cargo fmt --check (clean)
- Inspector CLI: tools/list modern + strict-modern + legacy refusals
- probe battery (discover / initialize / malformed / EOF exit)

## Notes

- Atelier fold NOT executed: D-143 stages the fold as its own gated
  step after the bloch migration; lumen's migration ran alone per the
  ruling. server.json untouched (v0.1.1); publication remains
  Owner-gated. Tree left uncommitted for Pax's landing under D-153.
