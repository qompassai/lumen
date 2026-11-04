# SFW content pipes — in-tree home (D-863)

The Qompass lane's content pipelines live here as tooling, not
as a server (skill-first, server-on-evidence; PIPES.md section
0 names the graduation candidates and their evidence bar).

- **Definitions of record:** `PIPES.md` — the commission
  deliverable, mirrored into this directory; the five pipes are
  Genesis (`lumen-genesis-sheet 1.0`), Canon as Data
  (`lumen-canon-census 1.0`), Content Graphs
  (`lumen-content-census 1.0`), Art Production
  (`lumen-keeper-chain 1.0`), and Determinism + Ship
  (`lumen-handoff-packet 1.0`), all on the shared spine
  Describe → Propose → Freeze → Emit → Prove.
- **Gate scripts:** `scripts/pipes/` (shared conformance and
  lane tooling in `run_record.py`; one script per gate).
- **Effects format:** `FX-FORMAT.md` in this directory
  (`lumen-fx/1`, lane-neutral; shared with the NTRanch lane by
  format only).
- **Driving skill:** `local-lumen-sfw-pipes` in the estate
  skill home.

Lane law, binding on everything here: runs stamp
`lane: qompass` / `content_label: sfw`; unclassified content is
refused `lane_unclassified` and nsfw-labelled content
`lane_mismatch`, typed and pre-mutation. Nothing NSFW is
authored, stored, or processed in this tree by these pipes.
