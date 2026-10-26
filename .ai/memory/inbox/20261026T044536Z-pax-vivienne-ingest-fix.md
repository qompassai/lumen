# Ingest fix landed on vivienne-proof/ingest (Pax takeover, 2026-10-25 night)

The G1 v3 worker died ~7 min after dispatch (no artifacts); Pax took the
fix over personally under the takeover policy. The fix rewrites opaque-
portrait ingest in crates/lumen-factory/src/identity.rs:

- No background keying anywhere (measured and rejected on the real BEGIN
  art: corner-median estimate matches nothing on a gradient backdrop;
  white sits 40.0 from the skin anchor, inside COLOR_MATCH_DIST; flood
  fills leak through JPEG edges into >93% of the hair).
- derive_skin: whole-portrait skin-like median with a warmth floor
  (8 <= R-B < 80). The BEGIN backdrop is neutral/cool (R-B <= 0); skin
  is warm (32). Reproduces the measured skin anchor exactly.
- derive_hair: median-cut population pre-filtered to saturated,
  non-background pixels; ranking stays share-first.
- derive_eye / derive_lips: candidates exclude the background estimate
  by anchor distance; the eye value floor is 0.25 (lash cluster ~0.15,
  iris rim/shadow mass < 0.25 otherwise wins the cut).
- segment_face: seeded skin component — densest strict-skin window in
  the upper half seeds a loose-predicate flood (shadowed skin sits ~96
  from the anchor, outside COLOR_MATCH_DIST). Face box from the
  component's row-span profile: top = first row, width = max span,
  chin = onset of the terminal narrowing (first post-plateau row under
  0.75 x width from which the span never recovers). On BEGIN this
  reproduces the F3 ground-truth box exactly (top 396, chin 950).

Acceptance vs F3 bounds (real BEGIN portrait + a valid sprite input):
hair (76,55,96) d=3.7 to hair body, 111.7 to lineart; skin
(253,234,221) d=0.0; eye (60,33,182) d=92.2 to hair body, 13.9 to the
nearer iris median; lips (247,177,182) d=4.1. Gates: cargo test
139/139 (136 baseline + 3 new opaque-ingest fixtures), clippy -D
warnings clean, fmt clean, Diver gate PASS (bacon_ls + rustana_ls
attached, 0 diagnostics).

OPEN: the staged G1 art dir's sprite is a single-frame GIF conversion
of the bust portrait itself. measure_proportions rightly fails closed
on it (no jaw narrowing in a full-width opaque frame; a bust has no
hips/legs to measure). G1 Step 4 (record + ParameterRecord + GLB)
waits on a real full-body animated sprite for Vivienne.
