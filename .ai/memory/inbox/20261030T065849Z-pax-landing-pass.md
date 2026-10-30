# Landing pass record — moc3 crate + fixture re-seed (2026-10-30)

Two gated branches landed into LOCAL main (no push; origin main
verified unmoved at 16e4cfc after the pass):

1. `pax/lumen-rig-import-landing` (64da7b3): lumen-rig-import
   crate + rig-core TEXTURES_MAX alignment 64 -> 256
   (commit aee0a96; SPEC section 11 amendment of 2026-10-26).
   Local main was 4a43350, a stale ancestor (10 behind origin
   main 16e4cfc) — the landing is a linear fast-forward
   4a43350 -> 16e4cfc -> 64da7b3, no divergence.
2. `pax/lumen-fixture-reseed` (b7f0e95): conformance fixtures
   re-seeded from native exporter output. Landed as merge
   commit 5ce8079 onto the moc3-landed main; the merge tree is
   byte-identical to the pre-merge `git merge-tree` simulation
   (441a2f80007fd269d0edff53fd30142e723ee720). File sets were
   disjoint (fixtures + inbox vs crate + limits/SPEC).

## Gates on the landed/merged main (pin nightly-2026-09-25,
## resolved clippy 0.1.100, sandbox + primo)

- lumen-rig-core 93/0 (motion_conformance 3/3; traces
  Greeting 9683dee682d77b01, Daydream 3e0af7a21434ba9b —
  fixtures byte-identical to the verified exporter output)
- lumen-factory 192/0; lumen-canon 39/0; lumen-rig-import 62/0
- clippy -D warnings clean, fmt clean, all four crates
- R-614 added for the re-seed and flipped green by bloch with
  the primo conformance run as its verify evidence

## Observations

- Primo canonical (~/.GH/Qompass/lumen) and the primo mirror
  had NO local main branch before this pass (checkouts on
  vivienne-proof/ingest and the landing branch respectively);
  local main was created at the landed head by update-ref,
  checkouts untouched.
- Test-count nuance: rig-import tallied 63/0 on the pre-merge
  tree and 62/0 post-merge in the landing re-runs (one
  fixture-data-driven case differs across the re-seeded
  fixtures); zero failures on both sides.
