# Emotion templates adopted as canonical in-repo

Date: 2026-10-13 (Owner ruling: adopt the sandbox-only copy as canonical).

The ratified emotion templates (schema `lumen.emotion-templates` v1,
ratified by Owner 2026-10-06) previously existed ONLY in a sandbox staging
directory (`~/workspace/lumen-2d3d/emotion-templates/` on the Pax VM);
the skill claimed `templates/emotions.json` lived in this repo but no
templates file existed here or anywhere on primo. The full sandbox set
is now adopted in-repo, byte-identical (sha256 verified against the
sandbox originals):

- `templates/emotions.json` (4c0e6f37…) + `templates/emotions.md` (b48b0fba…)
- `scripts/lumen_detail.py`, `lumen_detail_prep.py`, `lumen_pack_sheet.py`,
  `lumen_render_sheet.py`, `lumen_sculpt_deltas.py`, `patch_scripts.py`

Note: the adopted scripts still reference the retired `~/sprite-mcp`
layout in docstrings/hardcoded paths (e.g. patch_scripts.py BASE/GATE);
flagged, not rewritten — path repair is a separate change.

Validation: `python3 -m json.tool templates/emotions.json` parses;
cargo fmt/clippy/test re-run at adoption (baseline 383 passed / 0 failed
/ 8 ignored).
