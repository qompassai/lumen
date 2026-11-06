# Changelog — lumen

## 2.0.0 — 2026-11-05

- **Generation lane renamed to lumen terminology (D-1041, breaking).**
  The four generation tools served since the atelier fold (D-143) are
  renamed: `atelier_status` → `lumen_status`, `atelier_models` →
  `lumen_models`, `atelier_txt2img` → `lumen_txt2img`,
  `atelier_img2img` → `lumen_img2img`. Input schemas, annotations,
  and behaviour are unchanged. Served values follow: the status
  payload's `server.name` and `lifecycle_owner` now read `lumen`,
  output filename prefixes are `lumen-<job_id>`, and the lane's log
  files are `lumen-<profile>.jsonl`. Client registrations
  (rose, Diver) were updated names-only in the same programme.
- **Config path migrated.** The lane reads `$LUMEN_CONFIG`, else
  `~/.config/lumen/lumen.toml`. The pre-rename `$ATELIER_CONFIG` and
  `~/.config/atelier/atelier.toml` are honoured as fallbacks (the
  legacy default only when the lumen default is absent), so an
  unmigrated install keeps working.
- `server.json` still records the published 1.0.0 release package;
  the registry record is updated at the next release cut (D-646).

## 1.0.0 — 2026-10-29

The fleet-to-1.0 close-out (roadmap R-604..R-608).

- **Server package renamed** `lumen` → `lumen-mcp-server`
  (D-298); `license = "Apache-2.0"` added to the manifest. The
  library crate keeps the name `lumen` (explicit `[lib] name`),
  so `lumen::` consumers are untouched. The server binary is
  now `lumen-mcp-server`. `server.json` (`ai.qompass/lumen`)
  moves to version 1.0.0 with the renamed package identifier.
- **`canon_convert` landed (R-607)** — the canon SPEC §6 fourth
  tool: handled conversion only. A project PNG is remapped
  under the record's fingerprint palette (nearest palette
  colour within its tolerance; pixels outside every tolerance
  keep their colour and are counted) and the converted
  artifact is gated against the record's proportion + register
  bands before anything is written. Lane law, hash pin, and
  adopted-anchor lineage are the same typed refusals as
  `canon_gate`. Tools/list: 14 → 15 tools.
- **G-L1 closed (R-604)** — the Torogao Phase F motion handoff
  is verified landed: §4 parser, §4.2 evaluator, §4.3 fade law,
  model–motion binding, and the six cast-repertoire conformance
  fixtures are green in lumen-rig-core (see
  `MOTION-EVALUATOR.md`).
- **G-L2 closed (R-605)** — sprite→3D fidelity metric
  (silhouette IoU + landmark Δ) implemented in lumen-factory
  (`fidelity` module + CLI subcommand) with the first
  measurement on record (Seraphine: IoU 0.2456, landmark mean
  Δ 0.00685; see `crates/lumen-factory/FIDELITY.md`). PARITY
  leg 2c moves UNMEASURED → measured.
- **G-L3 closed (R-606)** — EFLO re-measured with a real
  tokenizer (cl100k_base): see `EFLO.md` for the number and
  its disposition against the ≤2,500-token budget.
- **Utilization evidence (D-116)** — a scripted stdio session
  against the 1.0.0 binary exercising `canon_resolve` /
  `canon_gate` on the B1 fixtures (admit + refusal paths):
  `UTILIZATION.md`.
- Subcrate versions: `lumen-canon` 0.1.0 → **1.0.0** (new
  crate, first publish line, absorbed at B2); `lumen-factory`
  0.1.0 → **0.2.0** (the fidelity module is a new feature);
  `lumen-rig-core` stays **0.2.0** (no code change in this
  close-out; its R2 content is verified, not modified).

## 0.3.0 — 2026-10-28

- Canon machinery (B2, D-292/D-312): `canon-record/1` absorbed
  as workspace member `crates/lumen-canon`; `canon_resolve`,
  `canon_gate` (proportion + register bands), and the anchored
  `canon_generate` broker with workflow-JSON provenance
  receipts. 14 tools total.
- lumen-rig-core R2 landed in-tree: §4 motion parser, §4.2
  evaluator, §4.3 fade law, cast-repertoire conformance
  fixtures.

## 0.2.0 / 0.3.0 (fold) — 2026-10-27

- rmcp 3.8.0 migration, modern-era only (2026-07-28), era
  guard; atelier generation lane folded in (D-143): 11 tools,
  surfaces byte-identical to the standalone baselines.
