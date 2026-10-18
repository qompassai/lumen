# Factory Phase 5 + VRoid integration update — landed on factory/build-20261018

2026-10-18, Pax takeover completion (the Phase 4/5 worker was
interrupted mid-Phase-5; state verified on disk before any commit).
Branch-level only: no merge to main, no push. Main is at 39cf296.

## Phase 5 (commits 1229448, 0919a90)

The interrupted work was audited and kept as-is — it was sound:
`toon` module (3-band `ToonConfig` as data), `write_glb_with_toon`
writing the config into GLB mesh extras under `lumen_toon` behind
`BuildOptions::ramp` / CLI `--ramp`, and `blender/render_proof.py`
reading the config back out of the file to build its ramp material.
Byte-stability of the Séraphine anchor without the flag is
test-enforced (toon_validation). The validate.rs diff was pure
rustfmt reflow and was split into its own style commit.

Gates at Phase 5 commit: root 383/0, factory 118/0, fmt + strict
clippy clean; Blender proof RENDER_PROOF_PASS in textured and ramp
modes, run fresh on the audited tree.

**Identity read (the honest headline):** the end-to-end Séraphine
proof is structurally complete but does NOT yet read as Séraphine
visually. Measured from the committed GLB: projected head 0.403 m
tall x 0.172 m wide (aspect 2.34; base head 0.212 m, aspect 1.23) —
identity `head_ratio` 0.2989 comes from alpha-row segmentation of
the sprite and includes her long hair mass, and projection applies
that band ratio to the head mesh alone. The hair shell sits ~6 mm
off the head, so the (faithfully extracted) magenta #a24a86 reads
only as a rim; face features are painted but at ~30 px span inside
the 256 px face region; happy/fun morphs change 0.04% of face
pixels. Palette extraction is faithful throughout. Full record with
the comparison sheet:
`~/workspace/lumen-factory-20261018/proof/IDENTITY-READ.md`.
Follow-up fix shape (re-anchors the Séraphine asset deliberately):
color-aware head-band segmentation, aspect-clamped head scaling,
real shell offsets/silhouette, face painting at face-region scale.

## VRoid integration update / P6 (merge ca71a80 + this pass)

Main merged into the factory branch: dc4f52a (demo_toon presets),
a3fa3a7 (stdio style ops), 39cf296 (src/vrm.rs). Assumption verdicts
(also recorded in docs/factory/FACTORY-PLAN.md §3/§6):

- A1 demo_toon: HELD exactly — landed parameters numerically
  identical to the factory's in-crate constants; variant split
  matches. `demo_toon_fire` does not exist and fails closed on the
  landed surface (verified live over stdio MCP); factory never
  references it.
- A2 VRM import exposing scene/bones: BROKE, benign — landed
  `vrm_inspect` reports flavor/meta/textures only. The factory
  keeps its own reader as the only bone/expression/proportion
  extraction; division of labor documented in
  `crates/lumen-factory/src/vrm.rs` (lumen = texture layer,
  factory = structure layer + the estate's only GLB -> VRM wrap).
- A3 VRM export wrapping a GLB: BROKE, benign — `vrm_grade` only
  re-encodes textures of an existing VRM; `wrap_as_vrm` stays and
  lumen's ops accept its output.
- A4 stdio style ops: HELD.

New in this pass: `scales_for_ratios` (identity.rs) is now the
single scales law shared by the identity path and the new
`ParameterRecord::from_identity_with_vrm_proportions` bridge
(params.rs) + `params --vrm` CLI flag, so VRM-measured proportions
drive a build in the same scale space as sprite-measured ones.
`assets/seraphine_graded.vrm` is a real lumen `vrm_grade` output
(demo_toon_portrait @ 1.0 over the stdio MCP surface);
`tests/vrm_interop.rs` asserts the factory reader extracts an
identical summary from it and that its proportions build a clean
character. End-to-end CLI proof: vrm_grade -> vrm-inspect ->
params --vrm -> build -> validate -> vrm-wrap, all green.

Gates after the pass: root 406/0 (fmt + clippy clean), factory
121/0 (fmt + clippy clean), Blender proof re-run green both modes.

Remainder for the eventual merge to main (Matt's word): the factory
crate is still standalone (not a workspace member, no path-dep on
lumen) — merging the branch brings the crate in as-is; unifying the
two VRM readers into one implementation, if ever wanted, is a
separate decision now that both contracts are pinned by tests.
