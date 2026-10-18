# Factory projection v2 — landed on factory/projection-v2-20261018

2026-10-18, Pax takeover completion (the v2 worker exhausted its
150 turns after committing fixes 1–3; fix 4 was verified on disk,
formatted, gated, rendered, and committed by Pax). Branch-level
only: no merge to main, no push. Base: merged main 3944a06.

## Commits

- 166a5fe fix 1 — color-segmented head ingest (head_ratio
  0.2989 → 0.1711; identity schema v2, v1 fails closed)
- 1e7343a fix 2 — uniform head scale (aspect 1.34 → 1.230)
- 4e06124 fix 3 — real shell offsets + hair tail (hair offset
  6 → 14 mm; hair.length measured 0.7980)
- fix 4 (78a44b3) — glTF UV registration (v1's bottom-left origin mirrored
  every atlas zone; the painted face sat on an unsampled island)
  + face features at face scale (eyes 1/5 face width) + hairline
  cap opening + ramp banding on lit value + head-bbox face framing

## Gates (Pax personally re-ran on the final tree)

Factory 136 passed / 0 failed; root lumen 406 / 0; cargo fmt
--check clean at both levels; strict clippy factory
(--all-targets -- -D warnings) clean; Blender render proof
RENDER_PROOF_PASS in textured and ramp modes.

## Measurements

Head 0.187 × 0.2306 m, aspect 1.233 vs base 1.23 (v1: 2.34);
head 12.9% of the 1.7868 m figure vs 13.3% color-segmented skull
fraction. Expressions vs neutral: happy 9.69%, fun 8.90% of face
pixels (bar ≥1%; v1 0.04%). Deliberate re-anchor chain:
cd7abaca… → d41e41ac… → 5f93fd6e… → 5bcaa972… → ce45290a…,
recorded in docs/factory/FACTORY-PLAN.md §9.

## Honest read

Every v2 bar clears. The renders read as Séraphine-identified —
magenta hair mass with a true ponytail tail, a visible face,
correct navy/white registration — at stylized-low-poly fidelity.
Not yet a likeness of the canonical portrait; sculpt/detail
refinement is follow-up scope. Full record:
~/workspace/lumen-factory-20261018/proof-v2/IDENTITY-READ-V2.md
