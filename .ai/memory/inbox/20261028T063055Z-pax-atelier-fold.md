# Atelier folded into lumen (D-143) — 2026-10-27

Crate 0.3.0. The standalone atelier server (primo `~/atelier` @
`11f027a`, branch `pax/atelier-s1`) is ported into `src/generation/`
and retired as a separate server; landing (commits/pushes) remains
Pax's under D-153, and atelier's trees are untouched by the fold.

What landed:

- `src/generation/` carries atelier's modules verbatim (admission,
  backend, config, error, lifecycle, log, paths, service, template +
  its test suite); only `crate::` paths and the two workflow
  `include_str!` paths changed mechanically. Workflow templates now
  live at `assets/workflows/` (crate root), byte-identical.
- The four tools (`atelier_status`, `atelier_models`,
  `atelier_txt2img`, `atelier_img2img`) are registered in `src/main.rs`
  with atelier's descriptions/annotations; every one of the 11 served
  tool objects is byte-identical to its standalone baseline (lumen 7:
  canonical 3046 in total; atelier 4: 4231; combined 7266).
- Lane profiles survive intact: config still resolves from
  `$ATELIER_CONFIG` / `~/.config/atelier/atelier.toml`; profiles,
  roots, admission, lifecycle, and error codes are atelier's.
- Startup posture (deliberate fold delta): an absent *default*
  config leaves the lane registered but unconfigured — tools refuse
  per call with atelier's `config_missing` shape — while a named or
  present-but-invalid config refuses startup. `atelier_status`
  reports `server.version` from the host crate (now 0.3.0).
- Policy: the four tools are labeled in `lumen-policy.toml`
  (reads low, sink trusted); the policy gate admits them like every
  other local tool.
- Scope (D-144/D-145): images only. Atelier never shipped video or
  music tools; `vera`/`aria` remain unimplemented roster names in
  `atelier_models` output, carried as data. Audio is torori's lane.

Gates (fold gate report: `~/workspace/mcp-atelier-fold-20261027/
GATE-REPORT.md`): cargo test 529/0/8 both machines; clippy 0.1.100
(f7575a9da8) + fmt clean; Inspector strict-modern exit 0 (0 errors,
23 type-union warnings — baseline unions, incl. atelier's 20);
Diver gate 17/17 files 0 diagnostics; utilization on primo: real
txt2img + img2img (Light Show source) through the folded server
with owned-engine start/stop on a fold-owned port-8199 engine, and
`sprite_info` reading both outputs in the same server.

Validation commands: `cargo test`, `cargo clippy --all-targets --
-D warnings`, `cargo fmt --check`, probes in
`~/workspace/mcp-atelier-fold-20261027/` (`fold_probe.py`,
`utilization_probe.py`, `baseline_probe.py`).
