# Utilization evidence — lumen-mcp-server 1.0.0 (D-116)

Scripted modern-era stdio session against the built 1.0.0
binary (`target/debug/lumen-mcp-server`, package
`lumen-mcp-server` 1.0.0), run 2026-10-29 in the sandbox.
Driver: `~/workspace/night-20261028/lumen-util-driver.py`
(session pattern per the fleet template); project root staged
with the B1 fixture record at `canon/ntranch/opal.json` plus
the B2 server-surface test record (`qompass/testchar`, the
`tests/canon.rs` shape) at `canon/qompass/testchar.json`.

**Result: 10/10 expectations PASS.**

| # | Expectation | Outcome |
|---|---|---|
| 1 | `server/discover` advertises exactly `["2026-07-28"]` | PASS |
| 2 | discover serverInfo = `lumen` **1.0.0** | PASS |
| 3 | tools/list carries **15 tools** | PASS |
| 4 | `canon_convert` registered (R-607) | PASS |
| 5 | `canon_resolve` admit — Opal record, ntranch caller | PASS (`ntranch/opal`, v1) |
| 6 | `canon_resolve` refusal — qompass caller vs nsfw record | PASS (`lane_mismatch`) |
| 7 | `canon_gate` refusal — Opal B1 record carries no register bands (D-312: the gate refuses what it cannot score) | PASS (`malformed_record`) |
| 8 | `canon_gate` admit — in-band measurement set vs the register-band record | PASS (`{"verdict":"pass"}`) |
| 9 | `canon_gate` refusal — drifted measurement set (face ratio 0.30 vs band 0.86±0.12) | PASS (`drift_detected`, delta table returned) |
| 10 | `canon_gate` refusal — no anchor lineage | PASS (`unanchored_lineage`) |

Fixtures are the B1 set named in the close-out brief
(`crates/lumen-canon/fixtures/opal-record.json`,
record sha256 pin `810164581d…`): resolve admit + lane
refusal run against it directly. Its gate leg surfaces a real
finding instead of a PASS: the B1 record predates the D-312
register bands, so the B2 server gate refuses it as
unscorable — the admit/drift gate legs therefore run against
the B2 test record, which carries register bands, and both
behave as specified. No character generation happened in this
session (D-292); the broker surface was exercised with
fixtures and verdicts only.
