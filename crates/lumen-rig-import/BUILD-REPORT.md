# R5a build report — lumen-rig-import (moc3 → native converter core)

Stage R5a of the lumen rig runtime: the deterministic Cubism→native
converter. Sandbox build in `~/workspace/lumen-rig-import-20261026/`;
local commits only, nothing pushed or published.

**Result: complete.** The converter takes a Cubism model directory
(model3.json + moc3 + cdi3/physics3/motion3/exp3 sidecars) plus a
per-model license record and emits native lumen-rig-core assets
(model, motions, expressions) plus a `conversion-report.json` that
accounts for every source construct that was approximated or
dropped. All four official sample models used for verification
convert end to end, and conversion is byte-for-byte deterministic.

## Constraints honored

- **Import-time conversion only.** Output is the native asset set;
  nothing in the output requires a moc3 parser at runtime.
- **No Live2D software in the tree.** No SDK code, no Cubism Core
  binary, no Live2D software output. The moc3 parser is an original
  from-scratch implementation written against the community OpenL2D
  format specification, with the layout verified byte-for-byte
  against official free sample models and guarded at runtime by a
  mandatory layout self-verification battery (a wrong layout fails
  loudly as a typed `LayoutCheck` error, never silently). The
  Orphism family (MIT) and Mocari/Ayagami were consulted as format
  documentation only; **no third-party code was reused**, so no
  license notices are owed.
- **Deterministic.** Same input bytes → byte-identical outputs
  (canonical ordering everywhere; validated by repeated runs over
  both the synthetic fixture and the largest sample).
- **Fail closed, fail loud.** One typed error enum
  (`ConvertError`); the license gate runs before any model file is
  read and refuses unless the record grants both `game_use` and
  `conversion_allowed`. The built model is re-parsed through
  `lumen-rig-core`'s own validator before anything is written —
  outputs land only after every validation has passed.

## Crate shape

`lumen-rig-import` (edition 2024, Rust 1.99.0 pinned,
`#![forbid(unsafe_code)]`), binary `rig-import`:

```
rig-import convert --model <dir> --license <record.json> --out <dir>
```

- `src/moc3.rs` — the from-scratch moc3 reader (checked arithmetic
  throughout; negative offsets/counts rejected; non-finite floats
  rejected; bool32 sections reject values outside {0, 1}) plus the
  layout battery (binding partition, per-object keyform-count ==
  binding-key product, warp grid identity, UV / position-index
  totals, unified-vs-specific binding agreement, range bounds).
- `src/cubism.rs` — serde types for the JSON sidecars.
- `src/convert.rs`, `src/convert/physics.rs`,
  `src/convert/motion.rs` — the mapping onto the native model,
  physics groups, and motion/expression assets.
- `src/native.rs` — native motion/expression assets with
  converter-side validation (rig-core R1 ships no motion parser;
  the checks implement the SPEC's LRF-202/203/207/303/501/502/602
  classes locally — see R5b owes).
- `src/license.rs` — the license record and the fail-closed gate.
- Depends on `lumen-rig-core` by path
  (`~/workspace/lumen-rig-runtime-20261025/r1/lumen-rig-core`).

**Version support.** moc3 version bytes 1–6 are accepted through
one shared section-offset-table slot map, guarded by the battery.
Bytes 1 (V3) and 3 (V4) are fully sample-verified; bytes 5 and 6
convert the official samples with every section read accounted for
except the unreferenced keys tail (below). Bytes 2 and 4 have no
local sample and are battery-guarded only. Byte 0 or ≥ 7 →
`UnsupportedMoc3Version`.

**Key format facts established empirically** (byte probes over the
four samples): section offsets are absolute from file start; the
stored keyform order varies the first-listed binding axis fastest;
warp row/column counts are cell counts (vertexCount ==
(rows+1)·(columns+1) exactly); every keyformed object's stored
keyform count equals the product of its binding axes' key counts
(verified on 1,207 keyformed objects across the four samples
with zero mismatches).

## Coverage map — moc3 → native

### Mapped

| Source | Native |
|---|---|
| Parameter id / min / max / default | Scalar parameter, verbatim |
| Part id / parent index | Parts forest |
| Deformer parent deformer | Deformer forest link |
| Rotation deformer: base angle, keyform angle/scale, origin (from the default form) | Rotation deformer + lattice |
| Warp deformer: rows/columns (cells), rest grid + keyform grids (from the default form) | Warp deformer + lattice |
| Binding axes (≤ 2) + key values | Lattice axes/keys; forms reordered into native canonical order (`at` = key-index tuples, ascending) |
| Default form (axis keys == parameter defaults, exact match; a miss is a loud error) | Base vertices / opacity / draw order, warp rest positions, rotation origin |
| Mesh vertices, UVs | Mesh vertices/uvs, verbatim (no re-origin, no rescale) |
| Position indices (i16 bit pattern → u32) | Mesh indices |
| Mesh texture number → manifest texture list | Texture records `texture_{index:02}`, manifest paths verbatim |
| Mesh visibility | `rendered` |
| Drawable flag blend bits | Blend mode (normal / linear_dodge / multiply; both bits set is a loud error) |
| Draw order, single group | Default-form keyform draw order, rounded |
| Clipping masks (by mesh id) | `clipping_mask_ids` (cycles surface as rig-core LRF-204) |
| Physics settings: vertices chain, input source/weight/type/reflect, output destination/vertex/scale/weight/reflect | Physics groups, inputs, strands (see Approximated for the transforms) |
| Motion parameter curves: linear / stepped / cubic-bezier segments | Keyframes; bezier control points become end/start tangents exactly |
| Motion loop / fps (integral, 1..=240) / duration | Motion asset fields |
| Curve fades, else the model3 motion reference's fades | Per-curve fades |
| Expression entries: Add / Multiply / Overwrite(→overwrite) | Expression entries |

### Approximated (recorded per occurrence in the report)

| Construct | Approximation |
|---|---|
| Bindings with > 2 axes (native lattice cap) | First two axes kept; remaining axes baked at their default keys |
| Draw order with multiple groups | Keyform value remapped into the mesh's group [min, max] range |
| Opacity float dust (source exports overshoot [0, 1] by ~5e-5) | Repaired to the boundary when within 1e-4; beyond that, a loud error |
| Inverted clipping masks (flag 0x8, in real use) | Mask list dropped; the mesh renders unclipped |
| Physics EffectiveForces | Scaled ×9.8 from the unit-vector convention ((0,−1) lands exactly on the native default gravity) |
| Physics inputs | `scale_degrees` = normalization window maximum × weight %; source windows are default-centered, native normalization is min-based — magnitudes agree at parameter maximum, the resting point differs. Position-X inputs are carried as angle inputs (the native input shape has one kind) |
| Physics outputs | One strand per output; particles are the chain prefix up to VertexIndex; output scale = source Scale × Weight % × (180/π) (source outputs are radians) |
| Physics strand anchor | First deformer (else first mesh) whose binding drives the output parameter; chain-root position as offset. Source chains hang from a model-space point, native strands from an object |
| Physics anchor fallback | Output parameters driving nothing in the parsed sections (on SDK 5 exports, most likely blend-shape keyforms in the tail sections) anchor to the model's first deformer/mesh |
| Physics particle fields out of native range (observed: delay 1.3) | Clamped to the native range; delay is carried-but-inert in native v1 (SPEC §12.2), so no behavior changes today |
| Motion duration | The exporter rounds Meta.Duration to two decimals; overshoot ≤ 0.01 s is repaired to the last keyframe time |
| Inverse-stepped motion segments | Converted as stepped |
| Expression fades absent | The Cubism default 0.5 s is written explicitly (the native default is 0.3 s) |

### Dropped with reason (recorded in the report)

| Construct | Reason |
|---|---|
| Per-vertex physics Acceleration | Pendulum response tuning with no native field (SPEC §3.6 / annex T-12) |
| Particle `damping` | Written 0.0 — Cubism has no counterpart (annex T-12) |
| Deformer keyform opacities; mesh keyform opacities / draw orders | Native keyforms carry shape only (opacity/draw order are base-value fields) |
| Deformer → part membership | Native deformers form their own forest; part membership lives on meshes |
| Art mesh culling flag; enabled toggle | Consumer render state / editor interaction state, not asset data |
| Zero-geometry art meshes (no indices) | Render no triangles in the source; native LRF-402 requires ≥ 3 indices |
| Motion curves on physics-output parameters | Dead in the source runtime too: Cubism's physics pass evaluates after motions and overwrites those parameters (LRF-602 class) |
| Model / PartOpacity motion curves | Native motions drive parameters only |
| Motion user data; motion group names | Consumer-layer data / consumer labels |
| Expression entries outside the native range law | SPEC §4.5: an entry value must lie in the parameter's range for **every** blend (observed in an official sample: an Add entry of −2 on a [−1, 1] parameter) |
| Hit areas; model3 parameter/part groups; pose file; user-data file | Consumer-layer data (SPEC §12.4); pose conversion is R5b scope |
| Glue (present in 3 of 4 samples) | No native counterpart in format v1 (SPEC §12.1) |
| Part keyform draw orders | Native parts carry no draw order |
| Parameter repeat flags; cdi3 display names/groups | Editor/runtime-hint metadata; cdi3 is used as a cross-check only (cdi3-only parts dropped, cdi3-only parameters noted) |
| Canvas info | Carried in the report as a note; coordinates and UVs pass through verbatim |

Loud errors (no conversion) include: mirrored rotation keyforms
(native scales must be > 0), a parameter driven by two physics
outputs (the native one-writer rule, LRF-601), duplicate motion
ids, unknown motion curve targets, non-integral motion fps,
moc3 structural breakage, and any failure of the layout battery
or of rig-core's validator on the result.

## Real-world check (local only)

Official free sample models from Live2D's CubismWebSamples
(git sparse clone into the git-ignored `scratch/`; the sample files
are **not** in the repo and appear here as aggregate counts only).
A test license record granting both booleans was used for the
parse-coverage runs. All four convert; every emitted model
re-validates under `lumen-rig-core`.

| Model | moc3 byte | Parameters | Parts | Deformers | Meshes | Physics groups/strands | Motions (curves) | Expressions |
|---|---|---|---|---|---|---|---|---|
| Haru | 1 (V3) | 42 | 19 | 97 | 84 | 4 / 4 | 6 (228) | 8 |
| Hiyori | 3 (V4) | 70 | 24 | 104 | 130 of 134 | 11 / 35 | 10 (308) | 0 |
| Mao | 5 (V5) | 132 | 32 | 178 | 262 | 16 / 20 | 8 (896) | 8 |
| Ren | 6 (post-V5) | 73 | 51 | 150 | 198 | 16 / 20 | 3 (159) | 5 |

Notable real-world findings, each now a converter rule with a
report trail: Hiyori carries 4 zero-geometry placeholder meshes
(13 vertices, 27 keyforms, no indices); Mao inverts 10 clipping
masks and uses a physics delay of 1.3; Mao and Ren each have 5
physics outputs whose parameters bind nothing in the parsed
sections (the anchor fallback); Mao and Ren use multiple draw
order groups (every mesh remapped); all samples carry glue
except Haru (0).

Unreferenced regions, reported per model: binding keys are fully
referenced on v1/v3 (Haru 126/126, Hiyori 212/212) but v5/v6
carry an unreferenced keys tail (Mao 433/934, Ren 182/422 —
suspected SDK 5 blend-shape key storage; never consumed).
Keyform positions: Haru 69,320/69,328 floats referenced (an
8-float residue also present in Hiyori: 80,392/80,400); Mao
125,288/143,920; Ren 140,832/154,688 (the larger v5/v6 gap is
consistent with blend-shape keyform positions).

Determinism: Haru converted twice into separate directories —
all 16 output files byte-identical; outputs also byte-identical
across the clippy/fmt cleanup refactor.

## Tests

27 tests, 0 failures — 12 validation / 15 adversarial, built on
a byte-level synthetic moc3 fixture assembled from the spec
layout (2 parts, 3 parameters, rotation + warp deformers,
clipped quad + mask, physics, motion, expression).

- Validation: full conversion + revalidation under rig-core;
  structure/values of the emitted model (defaults, lattices,
  draw order, clipping); hand-computed physics expectations
  (input scale 15°, strand lengths, output scale 572.9578°);
  motion tangents/fades and expression defaults; report
  accounting; texture records; lattice canonical order; source
  summary totals; unreferenced-file noting; golden byte-identity
  across runs; the official sample (runs when the git-ignored
  scratch copy is present, skips otherwise).
- Adversarial: truncations at six cut points; bad magic; version
  bytes 0 and 7; CIT count overflow (i32::MAX); cyclic deformer
  parent, cyclic masks, self-mask (all → rig-core LRF-204);
  mirrored rotation keyform; inverted mask (recorded
  approximation); license refusals (game_use false,
  conversion_allowed false, missing file, malformed JSON);
  motion value outside the parameter range; duplicate motion ids.

## Gates

| Gate | Result |
|---|---|
| `cargo build` (debug + release) | clean |
| `cargo test` | **27 passed / 0 failed** (12 validation, 15 adversarial) |
| `cargo clippy --all-targets -- -D warnings` (with `RUSTC_WRAPPER=`, the standing sccache quirk) | clean, 0 warnings |
| `cargo fmt --check` | clean |
| Sample conversions (Haru/Hiyori/Mao/Ren) | all convert; all models re-validate |
| Determinism (Haru, two runs, 16 files) | byte-identical |

Toolchain: Rust 1.99.0 (pinned by `rust-toolchain.toml`, the
latest stable in this environment), edition 2024.

## R5b still owes

1. **pose3.json conversion** (currently dropped with reason).
2. **motionsync3** — not present in any local sample; the format
   is unread so far.
3. **Texture atlas handling** — manifest texture paths convert
   verbatim today; atlas re-packing/regeneration is untouched.
4. **SDK 5+ tail sections**: the unreferenced keys tail and the
   keyform-positions gap (blend-shape parameters and their
   keyforms, most likely), the v5/v6 tail SOT sections (offscreen
   drawables; Ren populates all 160 slots), and the 8-float
   keyform-positions residue in v1/v3 files.
5. **Per-version slot tables for version bytes 2 and 4** — no
   samples exist locally; both are battery-guarded only today.
6. **Part keyform binding layout** (part slots beyond id/parent)
   — part keyforms are dropped regardless in v1.
7. **The 2-axis lattice cap** — baking extra axes at their
   defaults is an approximation a future native format version
   could lift.
8. **Physics fidelity**: multi-output settings become independent
   prefix strands (Cubism stages share one chain); the delay
   consumption law and per-stage outputs are T-12/R2 questions.
9. **A native motion/expression parser in rig-core (R2)** to
   replace the converter-side self-validation in `native.rs`.
10. **The Diver gate** for this crate at the sandbox→primo mirror
    step (standing rule for agent Rust builds; the sandbox has no
    nvim binary, so the gate runs on primo when the work mirrors).
