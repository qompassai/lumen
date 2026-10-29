# Sprite->3D fidelity — metric + first measurement (G-L2)

PARITY leg 2c stood UNMEASURED until 2026-10-29: no silhouette or
landmark error between a source sprite and its factory GLB had
ever been computed. This file records the metric and the first
numbers. Status: **MEASURED**.

## The metric (`src/fidelity.rs`, CLI `lumen-factory fidelity`)

Two legs at matched framings (front view both sides, each side
normalized to its own bounding box):

- **Silhouette IoU.** Sprite alpha silhouette (alpha >= 128) and
  the GLB's front orthographic projection (filled triangles,
  positions in model space as the factory writer emits them),
  rasterized onto the same 256x256 lattice. IoU = |A∩B| / |A∪B|.
- **Landmark Δ.** Five landmarks, fractions of figure height
  (Y, origin at the sole) or width (X): `scalp_top`, `chin`,
  `neck`, `hip`, `face_centre_x`. Sprite side: the identity
  measurement's segmented rows/column (`measure_proportions`),
  or an identity record's stored `sprite_measurements`. GLB
  side: the canonical skeleton's named joints accumulated
  through the node hierarchy — `HeadTop_End` (scalp), `Head`
  (chin proxy: the Mixamo head joint sits at the skull base —
  a proxy, named as such; no facial-feature detection is
  claimed), `Neck`, `Hips`, and the head joint's X over the
  mesh width (face centre). Report carries per-landmark Δ,
  mean Δ, max Δ.

Everything fails closed: transparent/unsegmentable sprite,
garbage/truncated GLB, missing named joints, rotation/scale
nodes, non-finite geometry, degenerate bbox, grid outside
1..=1024 are typed errors, never zero scores. Tests:
`tests/fidelity_validation.rs` (4) + `tests/fidelity_adversarial.rs`
(6), in the factory suite.

## First measurement — Seraphine (2026-10-29)

Artifacts (both in-tree):

- GLB: `assets/seraphine.glb` — 1,370 vertices, 2,618 triangles.
- Identity record: `assets/seraphine.identity.json` — its
  `sprite_measurements` were taken at ingest from the source
  sprite (`seraphine_animated_mature.gif`, sha256
  `7e3f867325…`, listed in the record's `sources`); the landmark
  leg reads the record (`landmark_source: identity_record`).
- Silhouette sprite: `tests/fixtures/seraphine_sprite_sheet_frame.png`
  (sha256 `98fce0f241…`), a frame of Seraphine's mature fullbody
  expression sheet (workspace ref:
  `light-show-readme-idles/refs/seraphine_sheet_fullbody_mature.png`).
  It is a later sheet generation than the ingest GIF — the
  silhouette leg therefore also prices sheet-vs-model drift,
  disclosed here rather than hidden.

Command:

```
lumen-factory fidelity \
  --sprite tests/fixtures/seraphine_sprite_sheet_frame.png \
  --glb assets/seraphine.glb \
  --identity assets/seraphine.identity.json
```

Result (grid 256; identical on re-run, determinism-tested):

| Leg | Number |
|---|---|
| Silhouette IoU | **0.2456** (sprite 30,097 cells; GLB 11,905 cells) |
| Landmark mean Δ | **0.00685** |
| Landmark max Δ | **0.01389** (face_centre_x) |

Per-landmark (sprite vs GLB): scalp_top 1.0000/0.9930
(Δ 0.0070); chin 0.8664/0.8712 (Δ 0.0048); neck 0.8282/0.8225
(Δ 0.0057); hip 0.4504/0.4475 (Δ 0.0028); face_centre_x
0.5139/0.5000 (Δ 0.0139).

## Reading the numbers

The two legs measure different things and disagree honestly:

- **Proportions carry.** Landmark Δ ≈ 0.7% of figure height on
  average: the parametric build reproduces the source sprite's
  measured proportions (head/neck/hip rows) closely — the
  identity-projection leg of the pipeline does what it claims.
- **Silhouette does not.** IoU 0.246: the GLB's front projection
  fills barely a third of the sprite's silhouette cells. The
  sprite's mass is hair + outfit + skirt flare; the factory mesh
  is the body under them (no hair mass, no outfit geometry in
  the base build). Until hair/outfit volumes exist in the
  build, silhouette IoU is bounded by this gap — it is a
  coverage metric, and 0.246 is its honest first value, not a
  gate failure: no bar was set before the first measurement
  existed. Setting the bar is the follow-on act, now that the
  number is on record.
