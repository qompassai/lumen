# Conformance fixtures — torogao cast repertoire exports

Seed fixtures for the lumen R2 motion parser and evaluator,
taken verbatim from the torogao Phase F exports
(`torogao-rust-port-20261027/phase-f-gates/exports/`, produced
by the torogao-side `export-rig` bridge over the cast
repertoire: Prima, Yui, Alma × Greeting, Daydream), as the R2
handoff (`LUMEN-MOTION-HANDOFF.md`, item 4) prescribes.

Motion files are byte-identical to the exports. Model files
differ from the exports by exactly one field: the format
token. The bridge emits `torogao-rig-model` (its RP1-D1
writer rule); the lumen R1 validator requires
`lumen-rig-model`, and token reconciliation is an open item
the handoff assigns to the R2/model work — it is NOT resolved
by silently accepting the foreign token here. The fixture
models carry the lumen token so the motion leg's conformance
can run end-to-end; every other byte of model content is the
export's own (re-serialized with 2-space indent by the copy
step, so model files are content-identical, not
byte-identical, to the exports).

Source sha256 (of the original exports):

- alma model:    d4d1d61649d6215afb089771615f77b541601595422b588cf9ec78a6055e8737
- prima model:   05b6255a41c1feb7cea0f9241c585f97f23a0c2758a93c399af96f5e2d0c023b
- yui model:     4c269642a08c43f4831a7f5d212226407cee74b2d36e5b14297f2f52509c9a2f
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

Second documented fixture repair (alma model only): eight
rotation keyforms — the "feature off" lattice point of
Alma's toggleable parts (cow ears, horns, maid headdress,
bell collar, milk streams) — carry scale components of
exactly 0.0 in the export. INP semantics read scale 0 as
"hidden"; lumen SPEC section 3.4 requires `sx, sy > 0`
(`LRF-305`), so the R1 validator refuses the export as-is.
In this fixture those components are rewritten to 1e-6.
The motion leg never evaluates deformers, so the repair is
inert for every conformance claim made here; it is recorded
as a bridge/model divergence for reconciliation alongside
the format token, not as a property of the export.
