# Lumen Rig Format — Specification v1

**Crate:** `lumen-rig-core` (name ruled final by the Owner,
2026-10-25).
**Stage:** R1 (format spec + validating parser for the model asset).
**Status:** normative for format version 1. Written under the R1 brief
in `../BRIEF.md`; where this spec and `../BRIEF.md` conflict, the
brief wins and the conflict is flagged in the R1 report.
**Vocabulary:** phase labels, arc kinds, and face-track terminology
are fathom's (`fathom-motion-record/3`). This format consumes that
vocabulary; it never redefines it.

The key words MUST, MUST NOT, SHOULD, and MAY are to be read as in
RFC 2119. "Validator" means any conforming parser: a validator MUST
reject every input this spec marks invalid, with the typed error
named in §9, and MUST NOT silently repair, drop, or clamp anything
except the single sanctioned clamp named in §9.3.

Design law, inherited from the brief:

1. **Determinism.** Simulation is a pure function of (model, motion,
   audio curves, fixed dt, seed). No wall-clock anywhere.
2. **Loud errors.** Anything unsupported or invalid is a typed error
   at load. Nothing silently degrades (the inox2d MeshGroup failure
   and bevy_inochi2d's dropped blend modes are the anti-spec).
3. **Content-free.** The format carries geometry, parameters, and
   timing — never artwork, text content, or character data. Texture
   pixels live outside the model asset (§3.4).

---

## 1. Assets and containers

The format defines three asset kinds, each a single UTF-8 JSON text
file:

| Asset | `format` string | Specified in | R1 parser |
|---|---|---|---|
| Model | `lumen-rig-model` | §3 | YES |
| Motion | `lumen-rig-motion` | §4 | NO (lands with R2) |
| Expression | `lumen-rig-expression` | §4.5 | NO (lands with R2) |

### 1.1 Encoding rules (all assets)

- The file MUST be valid UTF-8 JSON (RFC 8259). A byte-order mark is
  invalid (`LRF-102`).
- Duplicate object keys are invalid (`LRF-103`): JSON leaves them
  implementation-defined, so this format forbids them outright.
- Unknown fields are invalid everywhere (`LRF-104`). Every object's
  field set is exactly the set its section lists, plus fields marked
  optional. This is what makes versioning honest (§10): a new field
  is a new format version, never a silent extension.
- Numbers MUST be finite. The literals `NaN` and `Infinity` are not
  JSON; a numeric value that overflows f64 in parsing, or f32 where
  the field is f32-typed, is invalid (`LRF-301`). Integers in u32
  fields MUST be in `0..=4294967295`; a negative or fractional value
  in an integer field is a schema error (`LRF-103`).
- Field types in this spec: `f32` (IEEE 754 binary32), `i32`, `u32`,
  `string`, `bool`, arrays, and `[x, y]` pairs (a JSON array of
  exactly two f32 values).
- Input size: a validator MUST reject any asset larger than
  `INPUT_BYTES_MAX` (§11) before parsing (`LRF-101`).

### 1.2 Canonical emission

A conforming emitter (the parser's serializer) MUST produce:

- Keys in the order the fields are listed in this spec (struct
  order, not alphabetical), 2-space indentation, one trailing
  newline at end of file. In a tagged object (parameters,
  deformers) the `kind` discriminator key comes first; the
  remaining keys follow in the order this spec lists them.
  Fields with spec defaults are emitted at their resolved
  values; absent optional references (`parent`, `deformer`,
  `generator`, `keyforms`, strand `output`) are omitted.
- f32 values in shortest round-trip decimal form (the Ryu/Grisu
  shortest representation that parses back to the identical f32 bit
  pattern; this is what serde_json emits).
- Arrays in authored order, EXCEPT keyform `forms` arrays, which are
  emitted sorted ascending by lattice coordinate (§3.7). A validator
  MUST reject keyform `forms` that are not in that canonical order
  (`LRF-404`) — canonical order is part of validity, so emission is
  a pure function of the validated model and
  `parse → emit → parse` is the identity on models, and
  `emit ∘ parse ∘ emit` is byte-stable.

### 1.3 Identifiers

An **id** is a string of 1..=64 bytes matching `[A-Za-z0-9_-]+`
(`LRF-201`). Ids are unique **within their collection**: parameter
ids among parameters, part ids among parts, and so on (`LRF-202`).
References are always typed by the field that holds them, so two
collections may reuse a spelling without ambiguity. The model id,
generator string, part names, and strand ids follow the same id
rule where this spec calls them ids; free-text fields are called
out as such and carry their own length bounds (§11).

---

## 2. Shared concepts

### 2.1 Coordinate system and units

Model space is a right-handed 2D plane: +x right, +y up, units are
abstract "model units" (the asset's own art space; a model SHOULD
span roughly ±1 but the validator only enforces the magnitude bound
in §11). Angles are in **degrees**, counter-clockwise positive,
0° pointing along +x, unless a section states otherwise. Times in
motion assets are **seconds** (f32). Physics constants are in model
units and seconds (§5).

### 2.2 Parameters

A **parameter** is a named scalar channel. Every parameter has a
closed range `[min, max]` and every value a parameter ever holds is
inside that range (the sanctioned clamp, §9.3, enforces this at
write time in evaluation; the validator enforces it for every
constant the asset itself states). Parameters are the only thing
motion curves, expressions, physics, and deformers read.

### 2.3 The lattice (keyform grids)

Meshes, rotation deformers, warp deformers, and (since the
BIND-1 amendment, §3.2) parts carry **keyforms**: complete
states of the object, addressed by a lattice of one or two
scalar parameters (the **axes**). Evaluation (R2) interpolates
between lattice points bilinearly (one axis: linearly). The lattice
is specified in §3.7 and shared by all four object kinds.

---

## 3. The model asset

Top-level fields, in canonical order:

| Field | Type | Required | Meaning |
|---|---|---|---|
| `format` | string | yes | Exactly `lumen-rig-model` (`LRF-105`) |
| `format_version` | u32 | yes | Exactly `1` in this spec (`LRF-106`) |
| `id` | id | yes | The model's own id |
| `generator` | string ≤ 128 bytes | no | Provenance: tool that emitted the file |
| `parameters` | array | yes | §3.1; may be empty |
| `parts` | array | yes | §3.2; may be empty |
| `textures` | array | yes | §3.3; may be empty |
| `deformers` | array | yes | §3.4; may be empty |
| `meshes` | array | yes | §3.5; may be empty |
| `physics` | array | yes | §3.6; may be empty |

### 3.1 Parameters

Each entry is one of two kinds, discriminated by `kind`:

**Scalar** — `{ "id", "kind": "scalar", "min", "max", "default" }`.
`min < max` is required and `default ∈ [min, max]` (`LRF-302`,
`LRF-303`). All three are f32 and finite.

**Combined** — a derived parameter:
`{ "id", "kind": "combined", "min", "max", "terms", "offset" }`,
where `terms` is a non-empty array (≤ 16 entries) of
`{ "parameter": <id>, "weight": <f32> }` and `offset` is f32. Its
value is defined, exactly, as

```
value = offset + Σ_i  weight_i · value(term_i.parameter)
```

Semantics the validator MUST enforce:

- Every term references an existing parameter (`LRF-203`) that is
  scalar or combined; the reference graph over combined parameters
  MUST be acyclic (`LRF-205`). Evaluation order is the topological
  order of that graph.
- The **reachable range** of a combined parameter, computed in f64
  from its terms' ranges as
  `offset + Σ_i [min(w_i·lo_i, w_i·hi_i), max(w_i·lo_i, w_i·hi_i)]`,
  MUST be contained in the declared `[min, max]` (exact f64
  containment, no epsilon) (`LRF-304`). Authors widen the declared
  range; the validator never widens it for them.
- A combined parameter has no `default` field; its value at rest is
  the formula applied to its terms' rest values.

A parameter that is neither driven nor referenced is legal (it is
an author's channel). What a parameter may NOT be is listed in §9.

### 3.2 Parts

`{ "id", "parent", "opacity", "translation_keyforms"? }` —
`parent` is an optional part id (absent or `null` = root),
`opacity` is an optional f32 in `[0, 1]`, default `1.0`. Parts
form a forest: every parent MUST exist (`LRF-203`) and the
parent relation MUST be acyclic (`LRF-204`). A mesh's effective
opacity is its own opacity times the product of its part
chain's opacities (evaluation semantics, stated here so the
data has one meaning). Part chains deeper than `TREE_DEPTH_MAX`
(§11) are invalid (`LRF-204` — reported as a cycle class error
because the check is the same bounded walk).

**Part translations (BIND-1 amendment).** `translation_keyforms`
is an optional lattice (§3.7) whose payload is
`{ "translation": [x, y] }`: the **absolute model-space
translation of the part's whole subtree** at that lattice
point. Evaluation interpolates it like every other lattice
(§3.7), and a mesh's total part translation is the **sum over
its part chain** (its own part plus every ancestor) — the
translations commute, so the sum is their composition. The
accumulated translation applies to the mesh's vertices
**after** its deformer chain: the part forest and the
deformer forest are separate trees, the format records no
interleaving between a part node and a deformer node, and a
model-space translation is the only order the data supports.
Two consequences are named, not hidden:

- A part translation does **not** move deformer origins or
  physics anchors (both are absolute model-space positions in
  this format). The INP reference's node translation also
  moves descendant pivots; that behaviour has no
  representation here.
- The payload is a model-space vector. A converter carrying
  an INP node-translation binding converts the binding's
  local deltas into model space at rest orientation (the
  bridge's `delta_to_world`); the INP behaviour of the delta
  rotating with an ancestor's animated rotation is not
  reproduced.

The field exists because rigs author real deformation as
node translations (brow height, head and body anchor shifts
in the reference rigs): without it those parameter bindings
have no representation and their channels are
deformation-inert in this format — the failure BIND-1
closes. Absent = the part never translates, exactly the
pre-amendment meaning; see §10 for the amendment's
versioning record.

### 3.3 Textures

`{ "id", "source" }` — `source` is a relative path string,
1..=256 bytes, that MUST NOT begin with `/`, MUST NOT contain a
`\` character, a NUL, or a `..` path component (`LRF-206`). The
model asset references image data; it never embeds it. How `source`
resolves against a content root is the host application's
business, not the format's: the validator checks the string, never
the filesystem.

### 3.4 Deformers

Deformers form a forest via `parent` (optional deformer id; same
existence and acyclicity rules as parts). A mesh or deformer names
at most one parent deformer; evaluation applies the chain
root-first. Two kinds, discriminated by `kind`:

**Rotation** —
```
{ "id", "kind": "rotation", "parent"?, "origin": [x, y],
  "base_angle_degrees": f32, "keyforms"? }
```
Keyform payload (§3.7): `{ "angle_degrees": f32, "scale": [sx, sy] }`
with `sx, sy > 0` (`LRF-305`). The keyformed angle is an absolute
angle (not an offset from base); the base angle is the state when
no keyform lattice applies.

**Warp** —
```
{ "id", "kind": "warp", "parent"?, "columns": u32, "rows": u32,
  "rest_positions": [[x, y], ...], "keyforms"? }
```
`columns, rows ∈ [1, 32]` (grid cells). `rest_positions` holds
exactly `(columns+1) · (rows+1)` control points in row-major order
(row 0 first, x fastest). Keyform payload: `{ "positions": [[x, y], ...] }`
with the same count (`LRF-403`). A warp deformer plus its
descendants (through the parent chains) is a **warp mesh group**:
the warp's grid deforms every descendant mesh's vertices and every
descendant deformer's origin/control points. (The inox2d failure —
MeshGroup nodes silently non-deforming — is impossible here: a
warp group either validates whole or the model does not load.)

**Warp mapping (R2 evaluation pin).** A point is mapped by
locating it in a rest-grid cell, solving that cell's inverse
bilinear (Newton-Raphson from the cell centre, a fixed 16
iterations, barycentric fallback on the cell's first triangle),
and evaluating the same bilinear coordinates in the deformed
grid. Cells are scanned row-major, so a point on a shared edge
resolves to the first containing cell, deterministically. A
point outside every rest cell maps through the nearest cell
(by rest-centroid distance) with its coordinates clamped to
the cell — edge extension, never extrapolation. This is the
format's own warp: the INP reference's per-triangle affine
lattice (over the group mesh's own triangulation) is a
different discretisation of the same idea, and the bridge's
grid conversion is where the two are reconciled.

### 3.5 Meshes (drawables)

```
{ "id", "part": <part id>, "texture": <texture id>,
  "deformer": <deformer id>?,           // optional parent deformer
  "vertices": [[x, y], ...],            // 1..=VERTICES_PER_MESH_MAX
  "uvs": [[u, v], ...],                 // exactly vertices.len()
  "indices": [u32, ...],                // triangle list
  "opacity": f32 in [0,1],              // optional, default 1.0
  "blend_mode": <blend mode>,           // optional, default "normal"
  "draw_order": i32 in [-10000, 10000], // optional, default 0
  "rendered": bool,                     // optional, default true
  "clipping_mask_ids": [<mesh id>, ...],// optional, default []
  "keyforms"? }
```

Validator rules: `part` and `texture` MUST exist (`LRF-203`);
`deformer`, when present, MUST exist. `uvs.len()` MUST equal
`vertices.len()` (`LRF-403`). `indices.len()` MUST be a multiple
of 3 and ≥ 3, and every index MUST be `< vertices.len()`
(`LRF-401`, `LRF-402`). Keyform payload:
`{ "vertices": [[x, y], ...] }` with exactly the base vertex count
(`LRF-403`).

**Draw order.** Rendering order is a total order computed as:
meshes sorted by `(draw_order, id)` ascending (id compared as
bytes). Draw orders need not be unique; the id tiebreak makes the
order deterministic and machine-independent.

**Clipping masks.** `clipping_mask_ids` lists meshes whose alpha
clips this mesh. Each entry MUST reference an existing mesh, MUST
NOT be the mesh itself, and the mask relation over all meshes
MUST be acyclic (`LRF-203`, `LRF-204`). Any mesh may serve as a
mask; a mask-only mesh sets `"rendered": false`. How masks
composite is a renderer matter (R3); the data contract is the
relation itself.

**Blend modes.** `blend_mode` is one of exactly these 18 strings
(`LRF-405` for any other value) — the W3C Compositing and
Blending Level 1 set plus the two linear modes the rig class
uses in practice:

`normal`, `multiply`, `screen`, `overlay`, `darken`, `lighten`,
`color_dodge`, `color_burn`, `hard_light`, `soft_light`,
`difference`, `exclusion`, `hue`, `saturation`, `color`,
`luminosity`, `linear_dodge`, `linear_burn`.

Blend factor definitions are the W3C Compositing and Blending
Level 1 formulas, applied per channel in the renderer's working
space (R3 pins the space). No mode may be dropped, aliased to
`normal`, or approximated: a renderer that cannot implement one
MUST fail at load, not substitute (brief, §Quality bar).

### 3.6 Physics definitions

Physics is data in the model; its evaluation model is §5. A
physics group:

```
{ "id",
  "gravity": [gx, gy],              // optional, default [0, -9.8]
  "wind": [wx, wy],                 // optional, default [0, 0]
  "turbulence_amplitude": f32 ≥ 0,  // optional, default 0
  "inputs":  [ { "parameter": <id>, "scale_degrees": f32 } ],
  "strands": [ <strand> ] }
```

`gravity` and `wind` are accelerations in model units/s², each
component finite with magnitude ≤ 1e5 (`LRF-305`). Inputs rotate
the group's gravity vector (§5.2): each input's parameter MUST
exist and be scalar (`LRF-203`, `LRF-207`), and
`scale_degrees ∈ [-360, 360]`.

A **strand** is an anchored particle chain:

```
{ "id",
  "anchor": { "kind": "deformer" | "mesh", "id": <id>,
              "offset": [x, y] },
  "particles": [ { "length": f32 > 0, "radius": f32 ≥ 0,
                   "mobility": f32 in [0, 1],
                   "delay": f32 in [0, 1],
                   "damping": f32 in [0, 1) } ],   // 1..=64
  "output": { "parameter": <id>, "scale": f32,
              "offset": f32 }? }
```

The anchor object MUST exist in the named collection (`LRF-203`).
The output parameter MUST exist, MUST be scalar, and MUST NOT be
the output of any other strand in any group (`LRF-601`): one
writer per parameter is what keeps evaluation order-free. Strand
ids are unique within their group (`LRF-202`).

### 3.7 The lattice (normative for meshes, both deformer kinds, and part translations)

A `keyforms` object:

```
{ "axes": [<parameter id>] or [<parameter id>, <parameter id>],
  "keys": [[f32, ...]] or [[f32, ...], [f32, ...]],
  "forms": [ { "at": [i] or [i, j], <payload> }, ... ] }
```

- `axes.len() ∈ {1, 2}` and `keys.len() == axes.len()`. Every axis
  MUST reference an existing **scalar** parameter (`LRF-203`,
  `LRF-207` — a combined parameter or a physics output may not be
  an axis), and the two axes of a 2-axis lattice MUST differ.
- Each `keys[k]` is a strictly increasing list of
  2..=`KEYS_PER_AXIS_MAX` f32 values, each inside its axis
  parameter's `[min, max]` (`LRF-404`).
- `forms` MUST contain exactly `keys[0].len()` (× `keys[1].len()`
  for two axes) entries: one per lattice point, each `at` a valid
  index tuple, no duplicates, sorted ascending in lexicographic
  tuple order (canonical order, §1.2) (`LRF-404`).
- A payload is a complete state of the object (absolute vertex
  positions / absolute angle+scale / absolute control points /
  absolute part translation), never a delta. Interpolation
  between lattice points is evaluation's business (R2) and is
  exact there: linear along one axis, bilinear across two, in
  f64, rounded once to f32 at write. A parameter value outside
  the key range holds the edge form (the lattice never
  extrapolates).

An object with no `keyforms` is static under parameters (it can
still move through ancestor deformers and physics).

---

## 4. The motion asset

Top-level fields: `format` = `lumen-rig-motion`, `format_version`
= 1, `id`, `model` (the id of the model this motion targets;
a loader MUST reject a motion whose `model` does not match the
model it is applied to, `LRF-203`), `duration_seconds` (f32 > 0),
`loop` (bool, default false), `frames_per_second` (u32 in
1..=240 — the time base for the fathom binding, §4.4),
`curves`, and optionally `audio` (§6) and `fathom` (§4.4).

### 4.1 Curves

A curve binds one parameter: `{ "parameter": <id>, "fade_in_seconds":
f32 ≥ 0 (default 0), "fade_out_seconds": f32 ≥ 0 (default 0),
"keyframes": [ <key>, ... ] }`. The parameter MUST exist in the
target model and be scalar or combined-driven — a curve MUST NOT
target a physics-output parameter (`LRF-602`, one-writer rule).
At most one curve per parameter per motion (`LRF-202` applied to
the curve collection).

A keyframe:

```
{ "time": f32, "value": f32,
  "interpolation": "linear" | "stepped" | "cubic",   // default "linear"
  "tangent_in":  [dt, dv]?,     // cubic only, see §4.2
  "tangent_out": [dt, dv]? }    // cubic only
```

- Keyframe times MUST be strictly increasing, the first ≥ 0, the
  last ≤ `duration_seconds` (`LRF-501`).
- `value` MUST lie in the parameter's `[min, max]` (`LRF-303`).
- Before the first key and after the last key the curve **holds**
  the endpoint value. Nothing extrapolates.

### 4.2 Interpolation (exact)

Let keys be `(t₀,v₀) … (tₙ,vₙ)`, and `t` in segment `[tᵢ, tᵢ₊₁]`.

- **stepped:** value = `vᵢ` for `t < tᵢ₊₁`; at `t = tᵢ₊₁` the next
  segment owns the instant.
- **linear:** value = `vᵢ + (vᵢ₊₁ − vᵢ)·(t − tᵢ)/(tᵢ₊₁ − tᵢ)`,
  computed in f64.
- **cubic:** the segment is the cubic Bézier in (time, value)
  space with control points
  `P0 = (tᵢ, vᵢ)`,
  `P1 = (tᵢ + dt_outᵢ, vᵢ + dv_outᵢ)`,
  `P2 = (tᵢ₊₁ − dt_inᵢ₊₁, vᵢ₊₁ − dv_inᵢ₊₁)`,
  `P3 = (tᵢ₊₁, vᵢ₊₁)`,
  where `tangent_out` of key `i` is `[dt_outᵢ, dv_outᵢ]` and
  `tangent_in` of key `i+1` is `[dt_inᵢ₊₁, dv_inᵢ₊₁]`. A missing
  tangent defaults to `[0, 0]`. Tangent constraints the validator
  MUST enforce (`LRF-502`): `dt ≥ 0` for both tangents;
  `tᵢ + dt_outᵢ ≤ tᵢ₊₁`; `tᵢ₊₁ − dt_inᵢ₊₁ ≥ tᵢ`. These make the
  Bézier's time component monotonically non-decreasing, so the
  inverse is unique. Tangents on a non-cubic key are invalid
  (`LRF-502`).
  Evaluation: solve `x(u) = t` for `u ∈ [0, 1]`, where `x` is the
  Bézier time component, by Newton–Raphson from `u₀ = (t − tᵢ) /
  (tᵢ₊₁ − tᵢ)` for at most 8 iterations, stopping when
  `|x(u) − t| ≤ 1e-6` s; if not converged, or if the derivative
  vanishes, fall back to bisection on `[0, 1]` for exactly 24
  iterations. The value is `y(u)`. All arithmetic in f64; the
  iteration counts are part of the format — an evaluator that
  iterates differently produces different bits and is
  non-conforming. (This is the clause the inox2d "Unknown
  interpolate mode Cubic" failure makes necessary: cubic here is
  a formula, not a label.)

### 4.3 Fades (motion level)

When a motion starts, each curve's contribution is weighted by a
fade envelope: weight ramps linearly from 0 to 1 over
`fade_in_seconds` from the motion's start; when a motion ends or
is replaced, it ramps from its current weight to 0 over
`fade_out_seconds`. A weight multiplies the curve's *deviation
from the parameter's current base value* (base = the value the
parameter would hold without this motion); it never multiplies
the absolute value. Mid-fade interruption starts the new ramp
from the weight at the interruption instant. All ramps are
linear in f64 over the fixed-step clock.

### 4.4 Fathom binding

A motion MAY carry a `fathom` object aligning it with a fathom
motion record: `{ "schema": "fathom-motion-record/3",
"phases": [...], "arc": {...} }`. `phases` entries are
`{ "label": string, "start_frame": u32, "end_frame": u32,
"provenance": "measured" | "asserted-from-scenario" |
"asserted-by-owner-review", "source": string? }` with
`start_frame ≤ end_frame`; frame `f` covers the time interval
`[f / fps, (f+1) / fps)` at the motion's `frames_per_second`.
`arc.kind` is one of fathom's `progression`, `cycle`,
`segment_library`, `excerpt`, `single_frame`, `none`. Labels and
arc vocabulary are fathom's, used verbatim; this format adds no
phase semantics of its own.

### 4.5 Expressions (asset kind `lumen-rig-expression`)

An expression is a named, fading parameter set:

```
{ "format": "lumen-rig-expression", "format_version": 1,
  "id": <id>, "model": <model id>,
  "fade_in_seconds": f32 ≥ 0, "fade_out_seconds": f32 ≥ 0,
  "entries": [ { "parameter": <id>, "value": f32,
                 "blend": "overwrite" | "add" | "multiply" } ] }
```

Defaults: fades default to 0.3 s when absent; `blend` defaults to
`overwrite`. Each entry's parameter MUST exist and be scalar, its
value in range (`LRF-203`, `LRF-303`); a parameter appears at
most once per expression (`LRF-202`). Semantics: while active,
the expression has a weight `w` following the same linear-ramp
law as §4.3 (in on activation, out on deactivation, from the
current weight on interruption). Its entries apply, in entry
order, against the parameter's post-motion value `p` with entry
target `e`: overwrite → `p + (e − p)·w`; add → `p + e·w`
(where `e` is an offset for add); multiply → `p · (1 + (e − 1)·w)`
(where `e` is a factor for multiply). The result passes through
the sanctioned parameter range clamp (§9.3).

---

## 5. Physics evaluation model (normative for R2; defined in the model asset, §3.6)

### 5.1 State and step

Physics advances in fixed steps of `dt` seconds. `dt` is a
runtime input; conforming runtimes use `dt = 1/120` unless the
host declares another value from the set {1/30, 1/60, 1/120}
in its session record — the value used is part of the simulation
inputs (§7), never read from a clock. Each particle carries
position `p` and previous position `pp` (f64 vec2). Strand state
initializes at rest: all particles placed along the strand's
rest direction (from the anchor, each segment laid along the
gravity-free rest heading declared by the first step's
constraint solve from the anchor with zero velocity) — R2's
conformance tests pin the exact rest construction: particles
start at `anchor + Σ segment lengths` along +x from the anchor
in anchor-local space, `pp = p` (zero velocity).

### 5.2 Per-step update (exact order)

For each physics group, for each strand, per step `s`
(0-based, counting from simulation start):

1. **Effective gravity.** `angle = Σ_inputs scale_degrees_i ·
   norm_i`, where `norm_i = (value_i − min_i) / (max_i − min_i)`
   for input parameter `i` at its current value. Effective
   gravity `g` = the group's gravity vector rotated by `angle`
   degrees. Acceleration per particle: `a = g + wind +
   turbulence` (§5.3).
2. **Verlet integration.** For each particle:
   `p' = p + (p − pp) · (1 − damping) + a · dt²`; then `pp = p`,
   `p = p'`. Particle 0 is not integrated; its position is the
   anchor object's evaluated position plus the strand offset,
   every step.

   **R2 reading of the particle list.** Particle 0 *is* the
   anchor point: the strand's listed particles are the free
   particles, listed particle `k` joined to the previous point
   by the segment of its own `length`, with the stiffness of
   step 3 read from its own `mobility` ("particle `k + 1`"
   counting the anchor point as particle 0). This is the only
   reading under which a one-particle strand — the shape every
   converted INP pendulum takes — has a segment, a tip, and an
   output angle at all. The anchor object's evaluated position
   is: for a deformer anchor, its origin carried through its
   ancestor chain (a warp deformer's: the centroid of its
   evaluated control grid — a warp has no single origin in this
   format); for a mesh anchor, the centroid of the mesh's
   evaluated vertices (the reference point the bridge records
   when it converts a mesh anchor).
3. **Constraint solve.** Exactly `CONSTRAINT_ITERATIONS = 4`
   relaxation passes. Each pass: pin particle 0 at the anchor;
   for each segment `k` (particles `k`, `k+1`) in order, enforce
   its rest `length` by moving both endpoints symmetrically
   (particle 0's endpoint never moves), scaled by the segment's
   `stiffness`, where `stiffness = 1 − mobility` of particle
   `k+1`. (Mobility 1 = stiffness 0 = the constraint yields
   entirely that pass; mobility 0 = full correction.)
4. **Output write.** If the strand has an output: let `θ` be the
   angle in degrees of the vector from the second-to-last
   particle to the tip particle, measured against +x. The output
   parameter is written `offset + scale · θ`, through the
   sanctioned clamp (§9.3).

No other forces exist in v1: no collisions, no floor, no
particle-particle interaction. `radius` is carried data
(validated, used by renderers for thickness) — it has no
simulation effect in v1 (§12).

### 5.3 Seeded RNG (turbulence only)

Randomness exists in exactly one place: the optional turbulence
acceleration. Draws are a **pure function of indices** — there is
no RNG state to drift. For physics group `g`, step `s`, particle
ordinal `n` (0-based across the group's strands, in declaration
order):

```
stream_seed = run_seed XOR fnv1a64(utf8(group_id))
z = splitmix64_finalize(stream_seed + (s · 2^32 + n) · 0x9E3779B97F4A7C15)
u = (z >> 11) · 2^-53                      // in [0, 1)
turbulence_accel_axis = amplitude · (2u − 1)   // per axis, two draws: n·2, n·2+1 in place of n
```

`splitmix64_finalize(z)`: `z = (z ^ (z >> 30)) · 0xBF58476D1CE4E5B9`;
`z = (z ^ (z >> 27)) · 0x94D049BB133111EB`; `return z ^ (z >> 31)`,
all arithmetic wrapping u64. `fnv1a64`: hash = `0xCBF29CE484222325`;
per byte: `hash = (hash ^ byte) · 0x100000001B3`, wrapping.
`run_seed` is a simulation input (§7). When
`turbulence_amplitude = 0` the term is exactly zero and no draws
are consumed conceptually (draws are index-pure, so consumption
order cannot matter).

---

## 6. Audio binding

Audio reaches parameters through curves only. Two modes:

### 6.1 Baked curves (shipping default)

A motion's `audio` object: `{ "baked": [ <baked curve set> ] }`.
Each baked curve set is one ordinary parameter curve (§4.1) plus
a **provenance** object, both required:

```
{ "parameter": <id>, "keyframes": [...],
  "provenance": { "kind": "envelope" | "onset" | "manual",
    "tool": string, "tool_version": string,
    "source_audio_sha256": <64 lowercase hex chars>,
    "window_seconds": f32 > 0, "hop_seconds": f32 > 0 } }
```

`source_audio_sha256` is the hash of the audio the curve was
analyzed from (absent for `kind: "manual"`, where the three
analysis fields are also absent). A baked curve is data like any
other: playback evaluates §4.1/§4.2 and nothing else — identical
every run, every machine.

**Envelope→mouth mapping shape.** The analysis tool, not the
runtime, owns the mapping from audio to the mouth parameter. It
emits keyframe values already in the target parameter's range:
`value = min + (max − min) · e` with normalized envelope
`e ∈ [0, 1]` computed by the tool (window RMS normalized against
the tool's declared reference level in its own documentation).
The runtime performs no audio analysis in baked mode and no
mapping: a baked keyframe value outside the parameter range is
a data error (`LRF-303`), never something to clamp quietly.

### 6.2 Runtime follower (live / D6 sync)

For live audio, the host feeds the runtime one envelope sample
per fixed step. The follower state per bound parameter is one
f64 `y`, init 0. Per step, with input sample `x ∈ [0, 1]`
(host-normalized; a host sample outside [0, 1] is a host contract
violation — the follower clamps it, and that clamp is the
follower's own named clamp, §9.3):

```
τ = τ_attack if x > y else τ_release
α = 1 − exp(−dt / τ)
y += α · (x − y)
parameter_value = min + (max − min) · y
```

The binding is data: a motion or session declares
`{ "parameter": <id>, "attack_tau_seconds": f32 > 0,
"release_tau_seconds": f32 > 0 }`; defaults when undeclared are
attack 0.010 s, release 0.080 s. `y` advances only on fixed
steps; there are no wall-clock windows and no FFT anywhere in
the follower. The D6 layer drives parameters through this same
binding (or through baked curves); there is no second audio
path.

---

## 7. Determinism conformance requirements

A conforming implementation MUST satisfy all of the following.
Each is testable; R2's gates test them.

1. **Pure function.** Evaluation output (parameter values,
   vertex buffers, draw state) is a function of exactly: the
   model asset, the motion assets, the audio curves/follower
   inputs, `dt`, and `run_seed`. Any other input — wall-clock,
   frame delivery rate, thread scheduling, hash-map iteration
   order — MUST NOT influence output.
2. **Arithmetic discipline.** Curve and physics evaluation in
   f64; values written to f32 buffers round once, round-to-
   nearest-even (the Rust default). No fast-math, no fused
   multiply-add contraction beyond what the Rust compiler emits
   for the written expressions, no platform-specific intrinsics
   in evaluation paths.
3. **Order discipline.** Parameters evaluate in the topological
   order of §3.1; deformers root-first in declaration order
   within equal depth; meshes in draw order (§3.5) for any
   order-visible output; physics groups and strands in
   declaration order. Iteration over any map in evaluation is
   over sorted keys.
4. **Repeat-run identity.** Same inputs ⇒ byte-identical vertex
   buffers and identical frame hashes across repeat runs, on
   the same machine and across conforming machines.
5. **Step counting.** The number of physics steps taken for a
   given elapsed simulation time is `floor(elapsed / dt)`
   computed in f64 — a function of simulation time only.

---

## 8. Validation order

A validator MUST check in this order and report the FIRST
failure (so the first error is deterministic):

1. Input byte length (`LRF-101`).
2. Header probe: `format`, `format_version` (`LRF-105`,
   `LRF-106`) — read before full schema validation, so a
   version-2 file fails as a version error, not a schema error.
3. Full JSON parse: syntax (`LRF-102`), duplicate keys
   (`LRF-103` class: reported as schema), unknown fields and
   type errors (`LRF-104`, `LRF-103`).
4. Collection count limits (§11) (`LRF-107`).
5. Ids: charset/length, then duplicates, per collection in the
   order parameters, parts, textures, deformers, meshes,
   physics groups (and strands within their group).
6. Parameters: ranges and defaults; combined-parameter graph
   (references, acyclicity, reachable-range containment).
7. Parts: parent references, acyclicity/depth.
8. Textures: path rules.
9. Deformers: parent references, acyclicity; per-deformer field
   ranges; lattices.
10. Meshes: references (part, texture, deformer, masks);
    mask acyclicity; geometry (counts, indices); field ranges;
    lattices.
11. Physics: references (inputs, anchors, outputs); field
    ranges; one-writer rule.

---

## 9. The loud-error law

### 9.1 Error table (normative)

Every row is a typed error. Codes are stable across versions;
a conforming validator's error type carries the code. Rows
marked ◐ are enforced by the R1 parser (model asset); the rest
are specified now and enforced by the motion/expression
parsers landing with R2.

| Code | Condition |
|---|---|
| `LRF-101` ◐ | Input exceeds `INPUT_BYTES_MAX` bytes |
| `LRF-102` ◐ | JSON syntax error (line/column reported) |
| `LRF-103` ◐ | Schema error: missing field, wrong type, duplicate key, malformed integer |
| `LRF-104` ◐ | Unknown field present |
| `LRF-105` ◐ | `format` string wrong or missing |
| `LRF-106` ◐ | `format_version` unsupported |
| `LRF-107` ◐ | Collection or per-object count exceeds §11 bounds |
| `LRF-201` ◐ | Id violates charset/length rule |
| `LRF-202` ◐ | Duplicate id within a collection |
| `LRF-203` ◐ | Reference to a nonexistent object |
| `LRF-204` ◐ | Reference cycle (parents, masks) or tree depth exceeded |
| `LRF-205` ◐ | Combined-parameter reference cycle |
| `LRF-206` ◐ | Texture path violates §3.3 |
| `LRF-207` ◐ | Reference of the wrong kind (axis/input/output naming a combined parameter; lattice axes identical) |
| `LRF-301` ◐ | Non-finite number, or f32-overflow in a numeric field |
| `LRF-302` ◐ | Parameter `min ≥ max` |
| `LRF-303` ◐ | Stated value outside its declared range (defaults, opacity, keyframe values, expression values) |
| `LRF-304` ◐ | Combined parameter's reachable range not contained in its declared range |
| `LRF-305` ◐ | Field-specific range violation (scale ≤ 0, gravity/wind magnitude, tangent of a strand field out of its interval, particle length ≤ 0, etc.) |
| `LRF-401` ◐ | Mesh index out of bounds |
| `LRF-402` ◐ | Index count not a multiple of 3 (or < 3) |
| `LRF-403` ◐ | Parallel-array length mismatch (uvs vs vertices; keyform payload vs base; warp rest positions vs grid) |
| `LRF-404` ◐ | Lattice invalid: keys not strictly increasing / out of axis range / wrong count; forms incomplete, duplicated, or not in canonical order |
| `LRF-405` ◐ | Unknown blend mode string |
| `LRF-501` | Keyframe times not strictly increasing, or outside [0, duration] |
| `LRF-502` | Cubic tangent invalid (negative dt, tangent reaching past the neighboring key, tangent on a non-cubic key) |
| `LRF-601` ◐ | Physics one-writer violation (model asset: two strand outputs name the same parameter) |
| `LRF-602` | Motion curve targets a physics-output parameter |

### 9.2 No silent degradation, anywhere

A validator or evaluator MUST NOT: ignore an unknown field,
substitute a default for a missing required field, drop a blend
mode, skip a malformed keyform, treat a missing reference as
"no deformer", or continue past any §9.1 condition. The only
defaults in the format are the ones this spec names (each is a
field whose absence has exactly one specified meaning).

### 9.3 The sanctioned clamps (complete list)

Exactly two clamps exist in the format, both named, both
bounded, both specified above:

1. **Parameter range clamp** — every parameter write in
   evaluation (motion, expression, physics output) clamps the
   written value to the parameter's declared `[min, max]`
   (§4.5, §5.2). Constants inside assets are never clamped —
   they are validated, and an out-of-range constant is an
   error ( §9.1), because asset data has an author who can fix
   it, while a runtime write is arithmetic that must land
   somewhere defined.
2. **Follower input clamp** — the runtime follower clamps a
   host envelope sample to `[0, 1]` (§6.2), because the host
   is a live system whose samples are not authored data.

No other clamping is permitted anywhere in a conforming
implementation.

---

## 10. Versioning

- `format_version` is a u32, `1` in this spec, present in every
  asset kind. A validator accepts exactly the versions it
  implements; anything else is `LRF-106` — never a best-effort
  parse.
- Any change to a field's meaning, any new required field, any
  removed field, and any newly permitted field (unknown fields
  are errors, §1.1) is a **version bump**. There are no
  compatible silent extensions.
- Migration between versions is by **named, versioned converter
  tools** (the lumen doctrine: conversion is specified and
  deterministic, with golden tests), never by parser
  auto-migration. A v1 parser reading a v2 file says so and
  stops.
- **Recorded exception — the BIND-1 amendment (in §3.2).**
  `translation_keyforms` was admitted into v1 as an optional,
  defaulted field without a version bump, by explicit owner
  direction, and this note is its record so the exception is
  not silent. The case: the field's absence reproduces the
  pre-amendment meaning exactly (no part translates),
  pre-amendment documents parse and re-emit byte-identically,
  and a bump to v2 would have invalidated every landed
  export and fixture in one move — including the format's
  own adversarial fixture that uses version 2 as the
  canonical unsupported version (`LRF-106`). No other field
  may cite this exception as precedent; the next newly
  permitted field bumps the version.
- Error codes (§9.1) are never renumbered; new conditions get
  new codes.

---

## 11. Bounds (normative constants)

| Constant | Value | Applies to |
|---|---|---|
| `INPUT_BYTES_MAX` | 16 MiB (16,777,216 B) | every asset file |
| `ID_BYTES_MAX` | 64 B | every id |
| `NAME_BYTES_MAX` | 128 B | generator, free-text fields |
| `PATH_BYTES_MAX` | 256 B | texture `source` |
| `PARAMETERS_MAX` | 512 | parameters per model |
| `COMBINED_TERMS_MAX` | 16 | terms per combined parameter |
| `PARTS_MAX` | 256 | parts per model |
| `TEXTURES_MAX` | 256 | textures per model |
| `DEFORMERS_MAX` | 512 | deformers per model |
| `MESHES_MAX` | 1024 | meshes per model |
| `VERTICES_PER_MESH_MAX` | 65,536 | base vertices per mesh |
| `INDICES_PER_MESH_MAX` | 393,216 | indices per mesh |
| `KEYS_PER_AXIS_MAX` | 32 | lattice keys per axis |
| `KEYFORMS_PER_OBJECT_MAX` | 1024 | forms per lattice |
| `TREE_DEPTH_MAX` | 64 | part/deformer chain depth |
| `COORD_MAGNITUDE_MAX` | 1e6 | absolute value of any coordinate component |
| `PHYSICS_GROUPS_MAX` | 64 | physics groups per model |
| `STRANDS_PER_GROUP_MAX` | 32 | strands per group |
| `PARTICLES_PER_STRAND_MAX` | 64 | particles per strand |
| `CURVES_PER_MOTION_MAX` | 512 | motion asset (R2) |
| `KEYFRAMES_PER_CURVE_MAX` | 4096 | motion asset (R2) |

Memory bound: a validator's peak allocation is bounded by a
small constant multiple of the input size × the largest
per-object expansion above; nothing in validation allocates
proportional to anything except the input and these constants.

---

## 12. DEFERRED items

Each item names why it is not specified in v1. DEFERRED is a
decision, not an omission.

1. **Glue (vertex-pair coupling between meshes).** Ayagami's
   feature list includes it; v1 omits it because its evaluation
   semantics (constraint solve order interacting with draw
   order) need a measured case to specify against, and none of
   the fathom cases isolates glue. Candidate for v2.
2. **Physics delay / input smoothing.** The particle `delay`
   field is validated, carried data in v1 with **no simulation
   effect**: no consumption law for it survived exact
   specification in this pass (a delay line changes phase
   relationships that only a measured physics case can pin),
   and an unspecified-but-active field would violate the
   loud-error law's spirit. It is specified — per-strand or
   per-particle — when R2's conformance work has a case to
   measure against. Authors should treat `delay` as reserved.
3. **Physics collisions and `radius` as simulation input.**
   `radius` is validated, carried data in v1 with no simulation
   effect; collision response (particle vs particle, particle
   vs mesh) needs a determinism-safe broadphase design that v1
   does not attempt.
4. **Hit areas / tap targets.** Mocari exposes them; they are
   application interaction data, not animation data, and belong
   to the consumer (game) layer if anywhere.
5. **Motion blending between two motions** (cross-fade trees,
   layering weights beyond §4.3's per-motion fade envelope and
   §4.5's expressions). Sequencing and blending policy is a
   runtime/session design (R2/R3), not asset data; v1 assets
   deliberately carry no blend graph.
6. **Off-mesh deformer types** (path deformers, free-form
   lattice deformers beyond the warp grid). Rotation + warp
   cover the measured corpus behavior; new deformer kinds are
   a version bump with their own conformance cases.

---

## Appendix A — fathom vocabulary alignment

Used verbatim from `fathom-motion-record/3`, never redefined:
phase labels (free-form strings, semantics asserted by their
provenance), phase provenance values (`measured`,
`asserted-from-scenario`, `asserted-by-owner-review`), arc
kinds (`progression`, `cycle`, `segment_library`, `excerpt`,
`single_frame`, `none`), and the face-track concept (a motion
may be *measured against* a face track; face tracks themselves
live in fathom records, not in rig assets). Case F (post
2342056) is the reference alignment: its five phases (`trance`,
`act`, `surface`, `fluster`, `re-trance`) are exactly what a
§4.4 `fathom` block carries when a measured piece becomes a
rig motion.
