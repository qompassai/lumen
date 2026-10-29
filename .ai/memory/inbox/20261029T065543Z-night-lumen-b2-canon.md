# B2 canon build landed (night worker, 2026-10-28)

Evidence-backed discoveries from the LUMEN B2 commission (D-292/D-312):

- Canonical tree: origin main == the R2 landing tree at 1f5682e; the
  census figure 4a43350 was its parent (the 0.3.0 landing merge).
  primo ~/.GH/Qompass/lumen remains stale 0.1.0-era; do not use it.
- B1 canon crate absorbed as workspace member `crates/lumen-canon`
  (package `lumen-canon`, lib name kept `canon_record`); the root
  package is now a cargo workspace (members = crates/lumen-canon
  only; lumen-rig-core / lumen-factory keep their own workspaces).
- Register bands are additive data in `StyleFingerprint`
  (`register_bands`, same band shape); `gate_bands` unchanged, so
  B1 fixtures behave identically (v1 refuses 6, v2 refuses 4).
- Broker tools: canon_resolve / canon_gate / canon_generate (14
  registered tools total). No unanchored mode; receipts persist
  under the project root at `canon-receipts/<job_id>.json`,
  digest-verified. Records live under `canon/<lane>/<character>.json`.
- Sandbox build quirk: cdn.pyke.io 403s through the sandbox proxy
  for the ort-sys prebuilt (ms@1.28.0); primo gets 200. Sandbox
  builds used ORT_LIB_LOCATION pointing at the primo-fetched
  static lib extracted under ~/workspace/night-20261028/ort-dist/.
  Primo builds download natively (cache at ~/.cache/ort.pyke.io).
- Deferred to the 1.0 close-out worker: G-L1, G-L2, G-L3 (real-
  tokenizer EFLO re-measure), canon_convert, version 1.0.0 line,
  rename audit, publish. Roadmap items R-604..R-608 record these.

Validation: cargo test --workspace 586 passed / 0 failed / 8 ignored
on both sandbox and primo mirror; subcrates 93 + 139 passed;
clippy 0.1.100 (f7575a9da8) -D warnings clean both machines;
fmt clean. Commands run with RUSTC_WRAPPER= CARGO_BUILD_JOBS=2
(sandbox) / login-shell rustup nightly-2026-09-25 (primo).
