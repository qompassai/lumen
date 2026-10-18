# Lumen Factory — Build Plan (Phase 0: Reconciliation)

Commissioned: Matt, 2026-10-18 (D4 ruling). Coordinator: Pax.
Worktree: primo `~/scratch/lumen-factory-wt`, branch `factory/build-20261018`,
base commit `51ef294` (lumen main at program start). **No merge, no push.**

## 1. Design authority (what governs this build)

- `local-lumen-sprite-to-3d` SKILL.md — locked decisions: parametric factory
  (NOT neural reconstruction); mature set is the default target; glTF/GLB
  canonical interchange; Mixamo-compatible skeleton; Blender = first
  validation gate; expression set = 10 canonical emotions + neutral basis +
  sheet set (idle, blush, wink, pout, celebrate, alarmed).
- `~/workspace/lumen-2d3d/format-research.md` — primary-source format facts:
  glTF 2.0 right-handed +Y up, +Z forward, meters; morph targets are
  per-primitive delta arrays, names via `extras.targetNames`; VRM 1.0 = GLB +
  `VRMC_vrm` (15 required humanoid bones, expressions, MToon optional —
  plain PBR is legal VRM).
- lumen repo `templates/emotions.json` (schema `lumen.emotion-templates` v1,
  ratified 2026-10-06) — per-emotion ARKit-52 recipes + `sprite_2d` recipes +
  VRM mapping; procedural groups blink/gaze/visemes compose on top.
  **`fun` = playful, never VRM 1.0 `relaxed`.** `emotions` is a JSON list.
- Video study `TECHNIQUES.md` — T14 (real-time 3D skeletal performance, ramp
  shading, hair, DOF → factory requirements), T15 (token-IR-shaped canonical
  data records, REDox pattern), `demo_toon` preset spec (grade math stated
  verbatim: HSL shift/mult, contrast about 0.5, optional lightness posterize).

## 2. What already exists in lumen (reuse, do not rebuild)

| Asset | Where | Factory use |
|---|---|---|
| Sprite document model, PNG load/save, compositing | `src/doc.rs`, `src/export.rs` | Reference patterns only (factory crate is standalone — see §4) |
| Palette work (median-cut quantization) | `src/palette.rs`, `src/style.rs` | Algorithm reference; factory implements a small deterministic quantizer in-crate |
| Emotion templates (ratified) | `templates/emotions.json` | **Consumed as data** — factory reads the file, never forks the recipes |
| Project conventions | `src/lib.rs`, `AGENTS.md` | Fail-closed typed errors, `&Path` project roots, contract-summary doc headers, `forbid(unsafe_code)`, alphabetical module lists |
| Blender 5.2.2 LTS | primo `/usr/bin/blender` | Validation gate: GLB import (bones + shape keys), morph posing, ramp-material render proof |

What does **not** exist (the gap this program fills): any mesh, skeleton,
skinning, morph-target, or glTF code anywhere in the repo. The shipped
engine is 2D-only.

## 3. Interface assumptions — sibling work (D1 / D2 / VRoid), recorded

Another worker is concurrently landing, in the lumen **main checkout**:
(D1) `demo_toon` preset into `src/style.rs`'s table; (D2) `style_list` /
`style_apply` exposed on the stdio MCP surface; (VRoid) VRM import /
texture-grade / VRM export ops on MCP + A2A surfaces. This factory branch
designs against these assumptions, each to be reconciled in the final
integration-update phase (Matt's amendment, 2026-10-18):

- **A1.** `demo_toon` exists as a named style preset with the TECHNIQUES.md
  parameters (sprite variant posterize Some(6); portrait variant None;
  `demo_toon_fire` climax variant). Factory use: texture painting applies
  the documented grade math to painted textures; if the landed preset's
  parameters differ from the study spec, the landed code wins at
  integration time. **P6 verdict: HELD exactly** — the landed
  `demo_toon` / `demo_toon_portrait` parameters (hue +4.0, sat x1.15,
  light x1.0, contrast 1.06, posterize Some(6) / None) are numerically
  identical to the factory's in-crate constants, and the factory's
  posterize split (non-face zones posterized, face rect not) matches
  the landed variant split. `demo_toon_fire` does not exist; the
  landed surface fails closed on it (verified live over MCP) and the
  factory never references it.
- **A2.** VRM import op accepts a `.vrm` file and exposes, at minimum, its
  glTF scene (nodes, meshes, skins) and the `VRMC_vrm` humanoid bone
  mapping. Factory use: the VRoid input path (§5, Phase 2b) reads a VRM's
  humanoid proportions + skeleton as an alternative base source.
  **P6 verdict: BROKE, benign** — the landed `vrm_inspect` reports
  flavor / meta / texture inventory only; it does not expose the scene
  graph or the humanoid bone mapping, so structure extraction cannot
  be delegated to it. The factory keeps its own reader as the only
  implementation of bone / expression / proportion extraction, with
  the division of labor stated in `crates/lumen-factory/src/vrm.rs`.
  Interop is proven, not assumed: the factory reader extracts an
  identical summary from lumen's `vrm_grade` output
  (`assets/seraphine_graded.vrm`).
- **A3.** VRM export op can wrap a GLB with `VRMC_vrm` metadata. Factory
  use: factory GLB output stays plain glTF; VRM emission is delegated to
  that op at integration time (factory does not write its own VRM layer
  in this program beyond reading). **P6 verdict: BROKE, benign** — no
  landed op wraps a bare GLB; `vrm_grade` only re-encodes textures of
  an existing VRM. The factory's `wrap_as_vrm` (Phase 2b) is the
  estate's only GLB -> VRM path and stays; lumen's ops accept its
  output (verified over the stdio MCP surface).
- **A4.** Style ops are callable from the stdio MCP surface (D2) — factory
  CLI does not depend on this; noted for the eventual merged tool surface.
  **P6 verdict: HELD** — style ops landed on the stdio surface in
  a3fa3a7; the factory CLI still does not depend on them.

If any assumption breaks, the integration phase updates the factory's
VRoid stage and re-runs all gates + the end-to-end proof, including one
VRM-in → factory → GLB-out run.

## 4. Isolation strategy

- New standalone crate **`crates/lumen-factory/`** inside the worktree,
  package `lumen-factory`, its own `Cargo.toml` / `Cargo.lock`. It is NOT
  added to the parent package and does not path-depend on `lumen` (avoids
  the `ort`/`mlua` build weight and any chance of disturbing the sibling's
  files). Deps (all already in primo's cargo cache): `serde`,
  `serde_json`, `image` (png), `thiserror`, `clap`. Deterministic
  hashing: SHA-256 via `sha2` if cached, else an in-crate implementation
  (decided at Phase 1 start; recorded here as an open point).
- Files the factory branch may create/modify: everything under
  `crates/lumen-factory/`, factory docs under `docs/factory/` (new), and
  one `.ai/memory/inbox/` entry at the end. It must NOT modify
  `src/style.rs`, `src/main.rs`, `src/a2a/*`, or any other pre-existing
  file. Isolation proof per phase: `git diff --stat <base> -- . ':!crates/lumen-factory' ':!docs/factory'`
  shows no pre-existing file touched.
- Gate interpretation: "workspace suite stays green (383/0 + growth)" =
  the main package is untouched (isolation proof) and the factory crate
  carries its own growing suite, green at every phase gate.

## 5. Module map (crate `lumen-factory`)

Alphabetical, Tiger Style Rust, `#![forbid(unsafe_code)]`, fail-closed
typed errors (`FactoryError`), explicit bounds on every loop/buffer,
deterministic output (sorted maps, fixed construction order — same
record + same base ⇒ byte-identical GLB).

| Module | Contents | Phase |
|---|---|---|
| `base.rs` | Mixamo-named skeleton (joint table, hierarchy, bind pose); procedural base mesh (profile-lathe body, sculpted head, separate hair + outfit shells); skin weights by bone-distance falloff; UV layout (face region reserved) | 1 |
| `blend.rs` | ARKit-52 channel displacement fields over head vertex regions (brow/eye/mouth masks); compose emotion morph targets from `templates/emotions.json` recipes; sheet-set expressions per `sheet_mood_mapping`; blink/gaze/viseme groups as separate targets | 1 |
| `gltf.rs` | GLB writer (mesh, skin, morph targets + `extras.targetNames`, PBR materials, embedded PNG textures, `lumen_toon` extras ramp config) + GLB reader for parse-back validation (structure, counts, weights sanity) | 1, 4 |
| `identity.rs` | Ingest: portrait + sprite PNGs → palette (deterministic median-cut), proportion measurements (alpha-row segmentation: head/torso/leg ratios), signature colors; `IdentityRecord` (schema `lumen.factory-identity` v1) with SHA-256 content hash; re-ingest determinism check | 2 |
| `params.rs` | `ParameterRecord` — token-IR-shaped canonical JSON (schema `lumen.factory-params` v1): body proportions, appearance palette assignments, hair/face scalars; validation (ranges, finiteness); canonical serialization (sorted keys) | 3 |
| `project.rs` | Identity projection: proportions → bone scaling + mesh regeneration; palette → texture painting (skin/hair/outfit zones; face texture painted from `sprite_2d` recipes — eyes/brows/mouth at face UV region); hair shell selection/scaling | 2, 3 |
| `toon.rs` | Ramp/toon material configuration as data (thresholds, shadow tint, rim) written into GLB extras + consumed by the Blender proof script to build the ramp node material | 5 |
| `vrm.rs` | VRoid input path: minimal VRM 1.0 reader (GLB parse + `VRMC_vrm` humanoid + meta) extracting proportions/skeleton mapping into factory inputs (assumptions A2/A3) | 2b (stub), integration phase (real) |
| `main.rs` | CLI (clap): `base`, `blend-list`, `ingest`, `build`, `export`, `prove` subcommands | 1–5 |
| `blender/` | `import_check.py` (bones + shape keys + morph weight application), `render_proof.py` (turntable views + expression render + ramp material) — generated scripts are data, executed by Blender headless | 1, 5 |

### Base mesh approach (Phase 1, honest scope)

Procedural, code-generated, versioned in code (no downloaded assets —
provenance is "generated by this crate", license-clean by construction):

- Body: stacked profile rings (lathe) for torso/pelvis/legs/arms with
  elliptical cross-sections; joints placed at Mixamo-compatible positions
  for a 1.70 m mature female base (mature set is the default target).
- Head: UV-sphere-derived sculpt with flattened face plane, defined
  brow/eye/nose/mouth vertex regions (spatial masks) for blend fields;
  separate eye spheres optional (v1: painted eyes on face texture +
  morph-driven lids via fields).
- Hair: separate shell mesh (offset surface over scalp region, length
  parameter); outfit: separate torso/leg shells (offset surfaces) so
  palette zones stay independent.
- Skinning: ≤4 influences/vertex, weights from distance-to-bone-segment
  falloff, normalized; adversarial gate: extreme parameter values keep
  weights valid (sum ≈ 1, no NaN).
- Topology is stylized-low-poly by design (this is the v1 canonical base;
  the render proof shows exactly what it looks like — no beauty claims
  beyond the evidence).

### Skeleton (Mixamo-compatible names)

Hips, Spine, Spine1, Spine2, Neck, Head, HeadTop_End; Left/Right Shoulder,
Arm, ForeArm, Hand + finger chains (Thumb/ Index/ Middle/ Ring/ Pinky ×3);
Left/Right UpLeg, Leg, Foot, ToeBase (+ _End tips). Joint positions from
the base proportion table; hierarchy per Mixamo convention. The same
table also emits the VRM 1.0 humanoid mapping (15 required bones) for
the VRoid path.

## 6. Phase gates

Every phase: `cargo build`, `cargo test` (factory crate), `cargo fmt
--check`, `cargo clippy --all-targets -- -D warnings` in the worktree
crate; isolation proof; commit on `factory/build-20261018` with staged
set inspected (`git diff --cached --stat`). Tests ~50/50
validation/adversarial throughout (hostile PNGs, out-of-range params,
truncated GLB, NaN weights, oversized inputs rejected pre-decode).

- **P0** — this plan + reconciliation. ✅ (this document)
- **P1** — skeleton + base mesh + blendshape targets + GLB write of the
  bare base; Blender import check passes (bones + all shape keys present,
  one emotion posed ≠ neutral render pixels).
- **P2** — ingest Séraphine (portrait + sprite + expressions on record)
  → IdentityRecord; determinism gate (double ingest ⇒ identical hash);
  projection v1 (proportions + palette textures) → GLB.
- **P2b** — VRoid path stub: VRM reader against a synthetic VRM fixture
  (built in-crate from the base export + VRMC_vrm block) ⇒ proportions
  extracted; full reconciliation deferred to integration phase.
- **P3** — ParameterRecord drives build end-to-end; determinism gate
  (same record ⇒ byte-identical GLB); adversarial slider extremes pass
  skin-weight validity.
- **P4** — export validation: parse-back round trip (reader ⇔ writer
  agree), Blender import of the *built character* GLB (not just base),
  structural report (counts, names, weights).
- **P5** — end-to-end proof: Séraphine sprite → identity → model → GLB →
  Blender renders (front / three-quarter / profile, one emotion posed,
  one ramp-material render) + side-by-side sheet vs canonical art,
  phone-readable (≤1080 px wide panels), saved to
  `~/workspace/lumen-factory-20261018/proof/`.
- **P6 (integration update, per amendment)** — ✅ DONE 2026-10-18
  (takeover completion). Main merged into the factory branch
  (ca71a80; branch-level only). A1/A4 held, A2/A3 broke benignly (see
  §3 verdicts): the VRoid stage keeps its own structure reader/wrap
  beside lumen's texture ops, with the division of labor documented
  in `vrm.rs`. New: the VRM proportions bridge
  (`ParameterRecord::from_identity_with_vrm_proportions` +
  `params --vrm`), driven by the shared `scales_for_ratios` law, and
  `tests/vrm_interop.rs` against a real lumen-graded fixture
  (`assets/seraphine_graded.vrm`, produced over the stdio MCP
  surface). Gates after the pass: root 406/0, factory 121/0, fmt +
  strict clippy clean at both levels. End-to-end proof run: lumen
  `vrm_grade` (demo_toon_portrait) -> factory `vrm-inspect`
  (identical extraction) -> `params --vrm` -> `build` -> `validate`
  -> `vrm-wrap`, all green. The P5 Blender render proof was also
  re-run on the final tree: RENDER_PROOF_PASS in both modes.

## 7. Asset provenance

| Asset | Source | License |
|---|---|---|
| Base mesh, skeleton, blend fields | Generated in-crate by code (this program) | Apache-2.0 (repo license) |
| Emotion recipes | lumen `templates/emotions.json`, ratified by Matt 2026-10-06 (in-repo) | Repo |
| Séraphine canonical art | LightShow repo `assets/art/seraphine/` (Matt's own art) | Repo |
| Textures | Painted procedurally from identity palette (in-crate) | Apache-2.0 |
| External downloads | **None planned.** Any addition requires a provenance row here before use | — |

## 8. Known risks / honest walls (pre-registered)

- Procedural base aesthetics: v1 base is stylized; if renders read as
  mannequin-like, that is reported with the renders, not hidden. The
  factory's contract is rig/identity/parameter correctness; sculpt
  refinement is iterative follow-up.
- Face: painted-face-on-texture + morph fields is the v1 face; separate
  eye geometry is a documented v2 candidate.
- Byte-identical GLB: requires f32 bit-determinism in generation —
  all generation math is in-crate and ordered; if any step proves
  non-deterministic, the gate is downgraded to structure-identical with
  the cause documented (never silently).

## 9. Projection v2 (2026-10-18, Matt's commission) — DONE, branch-only

Branch `factory/projection-v2-20261018` off merged main 3944a06.
No merge to main, no push — integration stays Matt's word.
The dispatched worker exhausted its 150 turns after fixes 1–3;
Pax takeover completed fix 4 and all gates (record:
`~/workspace/lumen-factory-20261018/proof-v2/IDENTITY-READ-V2.md`).

- Fix 1 (166a5fe) — color-segmented head ingest: `head_ratio`
  0.2989 → 0.1711; color-segmented head fraction 0.1331; identity
  schema v2 (v1 records fail closed).
- Fix 2 (1e7343a) — uniform head scale: head aspect 1.34 → 1.230
  (base 1.233); head 12.9% of figure vs 13.3% measured.
- Fix 3 (4e06124) — real shell offsets + hair tail: hair offset
  6 → 14 mm, outfit torso 5 → 9 mm; `hair.length` measured 0.7980;
  tail bottom y 0.941 m.
- Fix 4 — glTF UV registration + face-scale features + hairline:
  v1 painted with a bottom-left UV origin (glTF is top-left), so
  every atlas zone was mirrored and the face sat on an unsampled
  island; v2 paints in glTF convention, features rasterized over
  the sampled head island (eyes 1/5 of face width), hair cap face
  opening fixed, ramp bands on lit value alone, face framing from
  the head primitive bbox.

DELIBERATE RE-ANCHOR chain (Séraphine build hash, params_validation
+ toon_validation): cd7abaca… (v1) → d41e41ac… (fix 1) →
5f93fd6e… (fix 2) → 5bcaa972… (fix 3) → ce45290a… (fix 4). Each
step justified by the measurements above; no silent hash bumps.

Final gates (Pax personally re-ran): factory 136/0, root 406/0,
fmt clean both levels, strict clippy factory clean, Blender
RENDER_PROOF_PASS textured + ramp. Expressions: happy 9.69% /
fun 8.90% of face pixels vs neutral (bar ≥1%; v1 0.04%). Head
aspect 1.233 vs base 1.23. Verdict: every v2 bar clears; the model
reads as Séraphine-identified (magenta ponytail, visible face,
correct outfit registration) at stylized-low-poly fidelity — not
yet a likeness; sculpt refinement is follow-up scope.
