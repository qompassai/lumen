# Conformance fixtures — torogao cast repertoire exports

Conformance fixtures for the lumen R2 motion parser and
evaluator: native output of the torogao-side `export-rig`
bridge (`crates/torogao-cli/src/rigexport.rs` in the torogao
tree) over the cast repertoire (Prima, Yui, Alma × Greeting,
Daydream), regenerated 2026-10-29 from the Phase F repertoire
fixtures. Every file here is byte-identical to the export it
was copied from — no conversion, repair, or re-serialization
of any kind is applied on the lumen side.

The bridge stamps the `lumen-rig-model` format token natively
(its `LUMEN_MODEL_FORMAT_TOKEN`), and keyform scale components
are floored by the exporter itself: a non-positive component
is emitted as 1e-6 (f32) with a note in the export's
conversion report. For this repertoire the floor touches only
Alma's model — 14 components across the eight "feature off"
rotation keyforms of her toggleable parts (milk streams,
cow ears, horns, maid headdress, bell collar), whose INP
source carries scale 0.0 ("hidden"); lumen SPEC section 3.4
requires `sx, sy > 0` (`LRF-305`). Prima's and Yui's exports
needed no flooring. The motion leg never evaluates
deformers, so the floored values are inert for every
conformance claim made here.

Source sha256 (of the native exports):

- alma model:    fe0b395b45caa9fa30ded67ba1872e8f784ea3001354a09eeb14ba2cd6f01594
- prima model:   49d28e5d762bf970c658fe0864c99e493d2bf2505b832fc4b872afde1cde6b44
- yui model:     f59b5b4a6e86a32e4342d63d8bfcd768d113f9798035b57eb8587aca6cc54a8f
- alma Daydream: 1747bee366d94357cc677b333e2bf50a6ec50428674b6b2613422ae8744c033c
- alma Greeting: 7723411b54c9d7528ddbf8a7b61ceb3ed9b440a50c47970b6ea78820d5a1564d
- prima Daydream: d77e9dfb15001ba1e494d15daca104b177eafa7ec1411a63c2686af7d10d80ae
- prima Greeting: 487c69b66b0125a90684c57502f3e80ef540cefb39bb41491eea541598e8c1fe
- yui Daydream:  98e0347753027315012bc480bcc37174fbd8fe29c8ab38a34fc22697b9bbc947
- yui Greeting:  97eaa1a92840a5a9ccff7f20afa9b8cf99c185544a032ec731c1720dd9508f92

All six motions are linear-only, fade-free, `loop: false`,
60 fps — that is what the bridge emits for this repertoire.
Cubic and fade coverage lives in `../full.motion.json` and
the evaluator's unit fixtures, not here.

History: these fixtures were seeded at R2 from the Phase F
exports with two documented lumen-side conversions — the
model format token rewritten from `torogao-rig-model` to
`lumen-rig-model`, and Alma's zero keyform scales rewritten
to a literal 1e-6 — because the bridge emitted a foreign
token and zero scales. Both conversions were retired
2026-10-29: the bridge was fixed at the source (token law
plus the exporter scale floor) and the fixtures re-seeded
from its native output. The motion files are unchanged since
the original seeding (identical sha256); the evaluated
conformance traces are likewise unchanged (Greeting
`9683dee682d77b01`, Daydream `3e0af7a21434ba9b` via the
`evaluate_motion` example, all three rigs).
