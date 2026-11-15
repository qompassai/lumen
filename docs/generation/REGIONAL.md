# `lumen_img2img_regional` — regional conditioning for composites (L7)

**Problem.** A *unify* pass (img2img over a composite: a figure plus pasted elements) re-draws every pixel under one
global prompt. Pasted elements merge into the figure or smear: the sampler has no instruction that a region holds a
particular object, and full-strength denoise erases its silhouette.

**Capability.** img2img in which each region keeps (a) its own prompt and (b) a held share of the denoise. Any later
sheet can use it; it is not tied to one character or set.

## Call

Same fields as `lumen_img2img` (`prompt`, `negative_prompt`, `source_path`, `model`, `denoise`, `width`, `height`,
`seed`, `steps`, `cfg`, `sampler_name`, `scheduler`) plus:

```json
"regions": [
  {"box": [x0, y0, x1, y1], "prompt": "what belongs here", "hold": 0.6, "strength": 1.0, "feather_px": 8}
]
```

| field | meaning | range / default |
|---|---|---|
| `box` | half-open rectangle in **source** pixels | `x0 < x1 <= W`, `y0 < y1 <= H` |
| `prompt` | applied only inside the region (combined with the global prompt) | nonempty, ≤ prompt byte limit |
| `hold` | share of the global denoise withheld inside the region (`0` = re-drawn like the rest, `1` = left as composited) | `[0, 1]`, default `0.6` |
| `strength` | region-prompt conditioning strength | `[0, 2]`, default `1.0` |
| `feather_px` | inward linear feather of both masks, output pixels | `≤ 128`, default `8` |

1 to 4 regions. The output size must keep the source aspect ratio (the template scales with a centre crop, so a changed
aspect would misalign the masks — refused, `argument_invalid`).

## Mechanism (template `iris_regional_img2img` v1, core ComfyUI nodes only)

- **Region prompts:** each region prompt → `CLIPTextEncode` → `ConditioningSetMask` (its mask, its strength,
  `set_cond_area: default`) → chained `ConditioningCombine` with the global positive. Unused slots: empty prompt,
  strength 0, all-zero mask.
- **Hold:** the encoded source passes through `SetLatentNoiseMask` with a fractional mask `1 - max(hold × coverage)`;
  the KSampler denoises at `denoise` outside regions and at `denoise × (1 - hold)` inside them.
- **Masks** are rasterised by lumen (deterministic integer geometry; coverage 1 inside the box with an inward linear
  ramp of `feather_px`, 0 outside) into two RGB PNGs: `regions` (R/G/B = regions 1–3) and `aux` (R = noise mask,
  G = region 4). Both are staged beside the source and uploaded with it.

## Result and manifest

The `lumen_img2img` result plus a `regions` record: each region's source and output box, prompt, hold, strength,
feather; the sha256 and engine upload name of both mask images; and the mask rule. The job manifest carries the same
record. Plain `lumen_txt2img` / `lumen_img2img` results and manifests are unchanged (the field is absent there).

## What does the work (measured, L7 proof 2026-11-15)

On a two-device composite at global denoise 0.5 (pink-silhouette IoU against the composite, inside each region):

| run | Domi 2 | Lush 4 |
|---|---|---|
| `lumen_img2img` (old path) | 0.735 | 0.860 |
| regional, hold 0 (region prompts only) | 0.742 | 0.764 |
| regional, hold 0.35 | 0.929 | 0.963 |
| regional, hold 0.6 | 0.937 | 0.976 |

The **hold** keeps the silhouette; region prompts alone (hold 0) do not save a pasted element at this denoise. Use
hold ≥ 0.35 for an element that must survive. The cost of a high hold is integration: a held region keeps its pasted
edges (for example a hand pasted over a device), so fix those in the composite first.

## Determinism

Masks are a pure function of the request. A repeat with the same source, regions and seed gives the same pixels, the
same lane determinism `lumen_img2img` has, **on an uncontended GPU**. Measured 2026-11-15 (L9R2): two jobs first run
while another process held most of the card (43.8 GB used after engine ready) did not reproduce on an empty card
(same source, mask hashes, prompts, seed); every empty-card run agreed byte-for-byte with its repeat. Treat a render
made under VRAM contention as unrepeatable: re-render it on a free card before it becomes a final.

## Tests

`src/generation/regional.rs` unit tests: box scaling, four regions, coverage ramp, deterministic rasterisation and the
hold encoding (validation); empty / too many regions, degenerate or out-of-source boxes, bad hold / strength /
feather / prompt, aspect change, unknown fields (adversarial). The stdio and policy roster tests carry the new tool.
