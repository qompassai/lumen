# canon-record/1 — reference implementation (CC-PROGRAM B1), absorbed into lumen at B2

Status: built and gated in the sandbox (D-290 program, step B1);
absorbed into lumen at B2 (2026-10-28, D-292/D-312) as workspace
member `crates/lumen-canon` (package `lumen-canon`, lib name kept
`canon_record` so the B1 suites run unmodified). This crate is the
format's reference — lane-neutral, stateless, no tools of its own.

B2 deltas (additive within /1, B1 behaviour unchanged):
- `StyleFingerprint.register_bands` (D-312): register fingerprint
  bands beside the proportion bands, same band shape; `gate_register`
  and `gate_all` score them with the same math and the same floor +
  1e-9 edge rule (D-294). `gate_bands` is untouched; the B1 fixtures
  still refuse exactly 6 (v1) and exactly 4 (v2) bands.
- Three broker refusals in the taxonomy: `missing_keeper_anchor`,
  `anchor_not_adopted`, `receipt_tampered` (SPEC §6.1 / intake
  item 5).
- One B1 test adapted for relocation: `pinned_anchor_hash_matches_
  the_real_artifact` looks for the R2 anchor in candidate locations
  and, on machines without the Hands-On tree (e.g. the primo
  mirror), falls back to the pin's internal consistency — the
  adult-lane anchor image is never vendored into this repo.

Authority: `~/workspace/canon-pipeline-20261028/SPEC.md` (D-281).
Program: `~/workspace/character-pipeline-20261028/CC-PROGRAM.md`.

## Layout — module → SPEC

| File | Implements |
|---|---|
| `src/model.rs` | §3 record format: `CanonRecord` (deny-unknown-fields throughout), hash-pinned `Anchor`s with adoption-act fields, `Invariant` with the machine / `eye-only` check split, `StyleFingerprint` (`ProportionBand`, `rendering_markers`, `palette`), `AuthorityRef`, append-only `keeper_chain` (`append_chain_entry` refuses version skew and bad hashes); `effective_label()` = stored label or nsfw (§2.3 fail-closed default); `sha256_hex` (§5 pin). |
| `src/validate.rs` | §3/§5 structural + invariant validation: format tag, lane-scoped id agreeing with `origin_lane`, ≥1 anchor with a `primary`, adoption-act fields non-empty (anchors enter only by adoption), SHA-256 shapes, unique invariant names, machine invariants must name a method, unique band metrics with finite targets and positive tolerances, palette hex/tolerance shape, chain versions monotone and ≤ record version. Collects all issues, sorted. |
| `src/handle.rs` | §5 handle protocol: strict `canon://<lane>/<character>@<version>` parsing; `resolve()` = parse → SHA-256 verify → parse + validate record → id/version agreement (`record_stale` on any mismatch) → §2.3 lane check (`check_lane_access`: sfw fetchable by any caller, nsfw refused to qompass callers, unclassified defaults nsfw). |
| `src/gate.rs` | §6/§6.1 proportion-band gate: `gate_bands()` returns `Pass` or `Refusal::DriftDetected` with a per-metric delta table (measured, delta, excess beyond the band edge), failures sorted by metric so verdicts are byte-stable (§10 determinism). Missing or non-finite measurements fail. Floor bands (`floor_only`) bind only from below. PASS releases to the eye sheet, never to adoption (§6.1). |
| `src/refusal.rs` | §6 typed refusal taxonomy (`drift_detected`, `invariant_violated`, `unanchored_lineage`, `lane_mismatch`, `record_stale`) with byte-stable serialization, plus `require_lineage()` for the unanchored check. |
| `fixtures/` | Real measured values from the D-280 record: `opal-record.json` (Opal's record; primary anchor pinned to the actual SHA-256 of the R2 ardent anchor artifact, re-verified against the file on disk by a test; palette clusters sampled from that artifact with PIL median-cut quantization), `measurements-ardent-r2.json` (governing values — gate PASS), `measurements-v1-fatter.json` (the rejected Stage 1R v1 set — gate refuses all six bands with deltas), `measurements-v2-corrected.json` (GATE-V2's set — gate refuses exactly the four residuals GATE-V2 named). |

## Gates run

- `cargo test`: 31 passed, 0 failed (13 validation + 18 adversarial).
- `cargo clippy --all-targets -- -D warnings`: clean.
  Resolved clippy: **0.1.98 (48a229ceae 2026-09-01)**; cargo/rustc 1.98.1.
- Build notes: sandbox `~/.cargo/config.toml` wires sccache (Redis on
  primo) as rustc-wrapper; it was unreachable, so builds ran with
  `RUSTC_WRAPPER=` (empty = no wrapper). The config's ephemeral
  `/usr/bin/clang` symlinks had been wiped and were restored with
  `~/workspace/bin/ensure-sandbox-clang` before linking.

## Scope notes (what this crate deliberately does not do)

- **Invariant measurement.** The gate evaluates proportion bands.
  `invariant_violated` exists in the taxonomy, but measuring machine
  invariants against an artifact is lumen's measurement stack
  (SPEC §10 S1, productized face-state machinery), not band math.
  Eye-only invariants are never measured here by design (§6.1).
- **Storage / tools.** Registries, `canon_resolve`/`canon_gate` tool
  surfaces and generation brokering are B2 (lumen). This crate is the
  shared format + math those surfaces call.
- **Palette distances.** Palette entries are shape-validated
  (hex form, tolerance in [0, 1]); colour-distance grading is lumen's
  existing grade machinery (SPEC §3), not specified numerically in
  the SPEC.

## SPEC ambiguities found (flagged, not invented around)

- **A1 — format field unnamed.** §3 names the format `canon-record/1`
  but never names the field that carries it. Encoded here as a
  required top-level `format: "canon-record/1"` string, enforced by
  the validator. If S1 prefers a different carrier (e.g. deriving the
  format from the schema registry), this is the one field to change.
- **A2 — band shape cannot express the SPEC's own bust band.**
  §3 gives bands as `{ metric, target, tolerance, method }` (a
  symmetric window), but §3/§6 also mandate in-costume banding "with
  the cap stated in the band", and the live D-280 bust band is a
  *floor* (≥ 0.68 toward a 0.83 target measured in another costume),
  not a window: a symmetric window around 0.83 would refuse the
  ardent anchor's own Stage 1 keepers. Resolved with two
  additive-optional fields (legal within /1 per §3's extension
  rule): `cap: string` and `floor_only: bool`. Also noted: the live
  SHOULD-V2 bands are asymmetric intervals around the target
  (e.g. face 0.86–0.98); tolerances here are chosen to reproduce
  the binding edge of each live band, and the non-binding side is
  wider than SHOULD-V2's interval. Ratify or amend at B2.
- **A3 — `gate_verdict_ref` cannot be required.** §3 lists it as a
  plain chain-entry field, but §9 says records are born with current
  adopted keepers as chain heads — keepers that predate the gate and
  have no verdict ref. Encoded as optional; entries written by
  `canon_gate` at S1 are expected to always carry one (a B2 brief
  clause, not a format rule).
- **A4 — two refusal kinds are unnamed by the SPEC.** §6 names five
  refusals, all for well-formed inputs. An unparseable handle or an
  unparseable/invalid record body has no named refusal; this crate
  adds `malformed_handle` / `malformed_record` rather than fail
  silently or mislabel them `record_stale`. Flagged for the S1 tool
  surface to ratify (they are client errors, arguably not gate
  refusals at all).
- **A5 — band-edge arithmetic needs a representation rule.** Bands
  and measurements are decimal quantities compared in binary
  floating point: the v2 aperture 0.16 is "in band" in the D-280
  record, but `|0.16 − 0.17| > 0.01` in f64 by ~1e-17. The gate
  therefore treats an excess ≤ 1e-9 as on-edge (`EDGE_EPSILON` in
  `gate.rs`). The SPEC's determinism clause (§10) does not address
  float representation; the epsilon is documented here and in code
  so B2 inherits it consciously.
- **A6 — anchor roles are "at minimum", so not an enum.** §3 names
  four roles "at minimum", implying more may exist; roles are
  validated as strings with a required `primary`, not as a closed
  enum, so /1 records can add roles without a format break.
