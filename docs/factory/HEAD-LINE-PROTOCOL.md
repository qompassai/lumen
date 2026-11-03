# Lumen Factory — Line-System Head Protocol v1

**Surface:** `lumen-factory` authoring (from-scratch head route).
**Stage:** T-5 (D-733), the from-scratch companion to the
donor-based headhack modelled-head route (BQ-5). Both routes share
the hygiene census of T-6 (`lumen.blend-census`, D-734).
**Status:** normative for protocol version 1. The proving build is
**OWED** (see §7): this spec is authored and checkable today; its
one supervised build has not yet been run and must not be reported
as run.

The key words MUST, MUST NOT, SHOULD, and MAY are to be read as in
RFC 2119. Terminology follows the estate register (D-655):
Blender's words for Blender work (shape keys, modifiers, loops),
glTF's words for exported records (morph targets).

Design law, inherited from the factory:

1. **Lines before surfaces.** A head is specified as named lines
   with a construction order and depth rules, so a build can be
   checked line by line instead of judged as a lump.
2. **Front reference first.** The protocol is front-reference-only
   at the line stage: depth is authored afterwards, against the
   placed front lines, never guessed before them.
3. **Cage discipline.** The subdivision cage stays at level 1
   (T-6 rule (c)); control is kept by construction, not by
   denser meshes.

---

## 1. Coordinate frame and references

- Factory convention: meters, Y-up, facing +Z. The front
  reference is viewed along −Z; its horizontal axis is X
  (character left is −X), its vertical axis is Y.
- The reference image MUST be centred on the head's vertical
  centre line. All line coordinates in a build record are stated
  relative to that centre line and to the chin point, in meters.
- A build record names: the reference, the line table (§2) with
  per-line vertex counts, the construction log (§3 order, one
  entry per step), and the check results (§6).

## 2. The named lines

| Line | Definition | Depth rule |
|---|---|---|
| `contour` | The front silhouette: forehead, temples, cheeks, jaw outline, chin — one closed loop | Placed flat on the reference plane (Z = 0) at construction; depth authored in §4 |
| `jaw` | The jaw's structural line, derived by **backward extrusion** of the contour's lower segment | Extruded toward −Z; MUST NOT be drawn as part of `contour` — the two lines are distinct records with distinct checks |
| `brow` | The brow ridge line, one per side | Sits forward of the eye perimeter (+Z of `eye`), placed in §3 step 6 |
| `eye` | The eye perimeter loop, one per side | The perimeter runs **above the lash line**; the lash line is texture/morph territory, never this loop |
| `lid_upper`, `lid_lower` | The eyelid loops the census measures (T-6 rule (b)) | `lid_lower` centroid MUST sit behind `lid_upper` centroid along Z (checked, not eyeballed) |
| `nose` | The nose line: bridge, tip, base — with a dedicated **anchor vertex between the eyes** | The anchor vertex is a named, single vertex; nose depth is authored from it in §4 |
| `mouth` | The mouth loop: upper lip, lower lip, corners | The **upper lip sits forward** of the lower lip (+Z) and its line is **sharp** (a crease, not a blend) |

Every line is a separate named object in the build record. A
build that merges two named lines into one loop fails §6.

## 3. Construction order (normative)

A conforming build performs these steps in this order:

1. **Centred two-vertex start.** Construction begins from exactly
   two vertices placed on the centre line. No plane, no sphere,
   no imported starter mesh.
2. **Contour placement.** The `contour` line is grown from the
   start and closed on the reference plane, flat (all Z = 0).
3. **Mirror.** A Mirror modifier is applied across the centre
   plane (X = 0). All subsequent line work is authored on one
   side and mirrored; the centre-line vertices are shared, never
   duplicated (T-6 rule (a) is the census of this).
4. **Subdivision, level 1.** A Subdivision Surface modifier at
   level 1 is added. The cage level MUST NOT be raised at any
   later step (T-6 rule (c)).
5. **Jaw derivation.** The `jaw` line is derived by backward
   extrusion of the contour's lower segment toward −Z.
6. **Feature lines.** In this order: `eye` (+ `lid_upper`,
   `lid_lower`), `nose` (anchor vertex first), `mouth`, `brow`.
7. **Loop closure.** Each feature line is closed into its loop
   and the mesh faces are filled between the lines. Only after
   every line passes its §6 front checks does depth authoring
   begin (§4).

## 4. Depth authoring

- Depth is authored in a single profile pass after §3: the
  contour gains its Z profile (forehead forward, eye plane
  recessed, chin forward), the nose is pushed forward from its
  anchor vertex, and the mouth's upper-lip-forward rule is set.
- Depth moves MUST preserve the front projection: after the
  profile pass, the front-view silhouette is re-diffed (§6) and
  must still be inside the stated band. A depth move that breaks
  the front silhouette is reverted, not compensated.

## 5. What this protocol is not

- Not a sculpting guide: surface detail beyond the named lines
  (pores, creases beyond the mouth rule, ears, hair) is out of
  scope for v1 and is later, separate work.
- Not the donor route: headhack (BQ-5) adapts an existing head;
  this protocol builds one from lines. The routes share only the
  T-6 census and the T-5 §6 silhouette check.

## 6. Checks

A build is checked, in order:

1. **Line separation:** the build record's named lines exist as
   distinct loops with the §2 membership (in particular `contour`
   ≠ `jaw`, and `eye` is the perimeter, not the lash line).
2. **Hygiene census (T-6):** the mesh passes
   `lumen.blend-census` — no duplicate vertex coordinates, lower
   lid behind upper lid, subdivision cage exactly [1].
3. **Front-view silhouette diff:** the built head's front
   projection is diffed against the reference silhouette.
   Stated band: symmetric-difference area ≤ 2% of the reference
   silhouette's area, and each named landmark (chin, both jaw
   corners, nose tip, both brow ends) within 1.5% of head width
   of its reference position.
4. **Depth rules:** the §2 depth column is verified from the
   mesh (centroid comparisons and named-vertex positions), not
   from renders.

## 7. Proving test — OWED

The protocol's proving test is **one supervised build** to this
spec, checked by §6 in full (T-6 census pass + silhouette diff
inside the stated band), with the T-7 brow placement (Face
Project snapping) performed on the same build as its desk
occasion (D-735 names this build as that occasion).

**Status: OWED.** The build is a supervised desk occasion, not a
headless gate; it has not been run as of this spec's landing and
no result is claimed. When it runs, its build record and check
results are filed beside this spec and this section is amended
with the outcome and date.

## 8. Deferred

- Profile-reference-first construction (a second reference view
  driving lines directly): needs a measured case; v1 is
  front-only by design.
- Ear and hair line systems: separate protocols if the proving
  build shows the pattern carries.
- Automated line extraction from a reference image: the lines
  are authored; extraction is a converter proposal of its own,
  not a silent extension of this spec.
