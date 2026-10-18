# demo_toon landing, stdio style ops, VRoid/VRM wiring (2026-10-18)

Three of Matt's rulings from the light-show video study landed as three
local commits (no push):

- `dc4f52a` — `demo_toon` + `demo_toon_portrait` style presets (table now
  15). Spec: `~/workspace/lightshow-video-study-20261018/TECHNIQUES.md`.
  `demo_toon_fire` was NOT landed: the spec leaves its posterize value
  open; the name fails closed as an unknown style (tested). Verified
  against the study's PIL simulations on the recovered subject (the
  study's /tmp subject file was lost; recovered pixel-exact from
  batch/lumen_grade_compare.png): portrait variant total RGB diff 35;
  sprite variant matches the pixel-check panel once that panel's +1px
  placement offset is accounted for.
- `a3fa3a7` — style ops exposed on the stdio MCP surface (D2). Same
  library functions as the A2A registration; no policy changes were
  needed (labels existed ahead of registration).
- D3 (this commit) — `src/vrm.rs`: `vrm_inspect` / `vrm_grade` on both
  surfaces under a new `vrm` capability. This is the VRoid Studio
  wiring at the file/format layer only: Studio authors, lumen consumes
  and returns `.vrm` files; the Studio GUI is never driven. Follows
  oracle.rs's hand-rolled GLB pattern (no gltf crate — a re-serializing
  parser cannot honor the byte-preservation contract). JPEG textures
  fail closed (image features are png + gif).

Test counts: 383 → 387 → 392 → 406, 0 failed throughout; fmt and strict
clippy green at each step. Subject-recovery detail for the study record:
the sandbox refs JPEG is a re-encode and does NOT contain the study's
crop; the grade-compare board in the study batch does, panel-exact.
