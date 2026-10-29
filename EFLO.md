# EFLO — tools/list measurement (G-L3, R-606)

Measured 2026-10-29 at the 1.0.0 tree, real tokenizer
(`tiktoken` cl100k_base), over the tools/list result served by
the built `lumen-mcp-server` 1.0.0 binary in a live stdio
session (the same session as `UTILIZATION.md`; raw result
captured from the wire, canonicalized as compact JSON with
sorted keys before counting).

## The number

| Surface | Chars | Tokens (cl100k_base) |
|---|---|---|
| **15 tools (1.0.0, incl. `canon_convert`)** | 12,224 | **2,826** |
| 14 tools (the B2 surface, `canon_convert` removed) | 10,834 | 2,537 |
| Full tools/list result incl. wrapper | 12,290 | 2,842 |

Budget (fleet EFLO, torori reference): **≤ 2,500 tokens**.

**Disposition: OVER BUDGET — waiver sought on this record.**
The 1.0.0 surface measures **2,826 tokens, 326 over**. The
overage is not a 1.0.0 regression: the B2 14-tool surface
already measures 2,537 (37 over) under the same method; B2's
own count (11,479 chars) was a character count, not a token
count — this is the first real-tokenizer measurement of this
surface (PROGRAM §3 G-L3 named exactly that gap).

Per-tool (tokens): canon_generate 464, atelier_txt2img 398,
atelier_img2img 357, canon_convert 289, canon_gate 205,
canon_resolve 163, vrm_grade 162, style_apply 152,
atelier_models 145, bevy_discover 114, validate_atlas 91,
vrm_inspect 88, atelier_status 86, sprite_info 73,
style_list 51.

## Waiver recommendation (for the Owner's record)

Grant a D-147-class waiver for the 1.0.0 line at 2,826,
because the surface is three deliberately fused tool groups
whose descriptions carry enforcement semantics the estate
does not want silently compressed:

- lumen core (7 tools, 823 tokens) — sprite/VRM/style/BRP;
- atelier fold (4 tools, 986 tokens) — generation lane, folded
  at D-143 with byte-identical descriptions as the fold's own
  gate condition;
- canon (4 tools, 1,017 tokens) — descriptions state the lane
  law, the no-unanchored-mode rule, and the refusal kinds;
  compressing them re-opens the exact ambiguity the canon
  pipeline exists to close.

The structural alternative, if the Owner prefers the budget
hard: trigger-scoped group loading in the torori pattern —
canon (1,017) or atelier (986) as an on-demand group brings
the always-on surface to ~1,800 — as a 1.1 item, not a silent
description shrink at 1.0.0. No tool was removed and no
description was edited to chase the number in this close-out.
