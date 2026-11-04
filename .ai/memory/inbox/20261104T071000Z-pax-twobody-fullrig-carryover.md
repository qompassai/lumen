# Bloch candidate — full-rig carry-over: census scope, dialect at emission, solver-scene verticality (D-875)

origin_lane: qompass
content_label: sfw

Lessons from the D-875 carry-over (branch
`pax/lumen-twobody-fullrigs`), which re-ran the D-847
two-body scene with the estate's D-859 full rigs after the
D-858 census missed them:

1. **A capability census is only as strong as its
   candidate set's provenance.** The D-858 ladder censused
   the nearest fixtures (corpus pair, INP 0.8 avatar,
   paramgaze repertoires, a parser fixture) and concluded
   in estate-wide words that no other puppet has limbs.
   The same evening's full-rig build — the CURRENT build
   output, sitting in its deliverables tree — was never in
   the set, and the conclusion was wrong where it mattered.
   Rule: enumerate candidates from build/deliverable trees
   first and fixtures second, and bound every census
   conclusion's wording to the set actually tried.
2. **Close dialect gaps at the record builder when the
   producer is estate-owned.** The full rigs import
   through the strict INP1 importer with zero tolerance
   work because their builder normalizes the strictness
   trio at emission (`interpolate_mode` on every binding,
   [0,1] axis t-space, nominal [0,1] 1D y range — the
   LRI-007/LRP-304/LRP-302 family). Reserve importer
   tolerance for foreign files; per-file conversion of
   estate records is the smell that the builder is the
   right fix site.
3. **In a solver scene, a driven rig's internal
   translation channels cancel.** The placement solve is
   anchored on the contacts, so a RootY-class rise inside
   the driven rig's geometry is subtracted back out by
   the solve; scene clearance can only come from the
   non-driven rig's travel and the driven rig's deform
   tucks (e.g. knee bend raising the feet within the
   geometry). Clearance numbers from solver-free stagings
   (separately posed composites) do not transfer to
   solver scenes — quote the staging with the number.
4. **Compensate staging artifacts; disclose record
   properties.** A rigid foot plate rotated about its
   centre dips its corners by ~w·sinθ below the floor:
   measure the dip on the rig and compensate the
   placement pro-rata with |sin θ|. But when the record
   itself deforms past the stance plane (the template's
   crouch extends its foot plates ≈14 units at full
   range), that is the rig's own geometry — report it,
   do not stage it away. The line between the two is who
   owns the cause: the staging, or the record.

Evidence: `~/workspace/lumen-twobody-chars-20261103/CHARS-REPORT.md`
(carry-over section), `verdict-pack-v3/MEASUREMENTS.md`,
`evidence/fullrig-probe-hand-response.log`,
`evidence/fullrig-sweep-*.json`.
