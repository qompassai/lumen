# SFW Content Pipes for Qompass — homed in Lumen (D-863)

**Status:** Stage 0 codification (D-863, Owner-commissioned
2026-11-03). The five pipeline shapes were codified from the SFW
teaching games' own production processes by D-839
(`~/workspace/ardor-pipelines-20261103/ARDOR-PIPELINES.md`) and
mapped there onto Ardor for the NTRanch lane. This document homes
the same shapes in **lumen** for the Qompass lane, per the Owner's
lane-split ruling (D-863): SFW pipes in lumen, NSFW pipes in
Ardor, mechanics shared, content never crossing.

**What transfers and what does not.** The shared mechanics are:
the spine (§2), the `canon-record/1` format and handle protocol,
named versioned converters, gate-of-record discipline (a gate is
a versioned artifact plus a check run against it), typed
pre-mutation refusals, and the conformance run-record format (§8).
Content, canon records, and posture answers never cross lanes.
These pipes run only Qompass-lane material: Niflheim, Versemend,
and future SFW titles. Nothing NSFW is authored, stored, or
processed in any Qompass tree by these pipes.

## 0. Homes and placement

Skill-first, server-on-evidence (estate doctrine): the pipes are
**pipeline definitions + gate scripts + a driving skill**. No new
MCP server is created by this commission.

| Piece | Home |
|---|---|
| Definitions of record | this file; in-tree mirror at `docs/pipes/` in the lumen tree (branch `pax/lumen-sfw-pipes`) |
| Gate scripts | lumen tree, `scripts/pipes/` (Python, the `scripts/` idiom) |
| Pipe fixtures + worked examples | lumen tree, `scripts/pipes/fixtures/` |
| Driving skill | `local-lumen-sfw-pipes` (estate skill home, `~/workspace/custom-skills/`), driving the in-tree tooling |
| Run records + evidence | per-run, written beside the gated artifacts; commission evidence under `~/workspace/content-factory-20261103/sfw/evidence/` |
| Canon machinery (shared) | lumen tree, `crates/lumen-canon` (`canon-record/1`, handle protocol, typed refusals) — consumed, never forked |
| Rig machinery (shared) | lumen tree, `crates/lumen-rig-core` (INP1 import, puppet stage, evaluator, render driver) — consumed, never forked |

**Graduation candidates (documented, not built).** Three
capabilities in these pipes meet the shape of the estate's
server-graduation rule (state + enforcement), the same three
D-839 named as tool-earners in the Ardor mapping: (a) the
**genesis defaults ledger** — downstream pipes refuse against
unruled state, so the ledger wants a decide()-class reader;
(b) the **canon census** — it feeds ship gates, and lumen-canon
already carries the gate/refusal machinery a census module
would extend; (c) the **keeper-chain registry** — keeper state
is enforcement state at handoff. Evidence for each is the
refusal traffic these pipes generate in skill form: if a gate
must be enforced against a caller that does not run the skill,
that is the demonstrated need. Until then they stay scripts;
this document is the graduation file.

## 1. Lane law for the pipes

- **Stamping.** Every run record, census artifact, chain record,
  and packet produced by these pipes carries `lane: "qompass"`,
  `origin_lane: "qompass"`, `content_label: "sfw"`. A pipe output
  without the stamp is not a pipe output.
- **Intake.** A pipe accepts only material whose label is `sfw`.
  Material labelled `nsfw` is refused with `lane_mismatch`.
  Material with no label, or any other label, is refused with
  `lane_unclassified` — unclassified content defaults to the
  NSFW side (bloch SPEC §9, fail closed); it is never processed
  "to find out", never persisted, never relabelled by a pipe.
  Reclassification is a gated Owner/Pax promotion act with
  evidence, outside these pipes.
- **Refusals are typed and pre-mutation.** A refused run leaves
  every input byte-identical; the refusal names the offending
  ids. Refusal codes are spelled once, here, and reused by every
  pipe: the lane codes above plus, per pipe, `defaults_unconfirmed`,
  `name_conditional`, `clearance_collision`, `canon_unverified`,
  `census_stale`, `canon_orphan`, `differentiation_collision`,
  `sanitization_hit`, `graph_orphan`, `unlock_undeclared`,
  `contract_requires_seal`, `pack_touches_safety`,
  `record_missing`, `canon_unadopted`, `chain_incomplete`,
  `chain_hash_mismatch`, `finish_not_replayable`, `drift_failed`,
  `double_bake_mismatch`, `checklist_stale`,
  `packet_digest_mismatch`.

## 2. The spine — Describe → Propose → Freeze → Emit → Prove

Every pipe is the spine applied to one kind of state:

1. **Describe.** The thing exists first as a text/data record
   (canon record, design record, part record, effect record).
   Generation against an unwritten record is forbidden.
2. **Propose.** Generative or human authoring produces candidates
   *against* the record, at authoring time only. A proposal is
   not an asset.
3. **Freeze.** A keeper gate — the Owner's ruling on a numbered
   verdict pack where canon is adopted or changed, a recorded
   gate verdict elsewhere — freezes the chosen proposal into the
   record as data: parameters, part variants, palettes, curves,
   hashes. Custody-frozen nodes refuse regeneration.
4. **Emit.** Named, versioned, deterministic converters turn the
   frozen record into game artifacts (sprites, puppets, effect
   timelines, packages). Same record in, byte-identical artifact
   out, or the difference is a finding.
5. **Prove.** Every stage leaves a conformance run record (§8):
   gate id + version, input state hash, output artifact hashes,
   verdict or refusal codes, ruling references, lane stamp. A
   claim without its run record is REVIEW, not passed.

## 3. Pipe 1 — Genesis

*Gate of record: `lumen-genesis-sheet 1.0`.*

A Qompass title starts ruled, not blank. Inputs: a working title
candidate, the project slug, intended channel targets, and the
**Qompass defaults profile** — the SFW lane's own block, in the
shape Versemend §9 established (seven items presented as
defaults-to-confirm, confirmed as a block in D-832): platforms,
release model (free / fully offline), outward-reference posture
(none in shipped text), language (English, the production rule),
art + audio pipeline, accessibility gate (photosafety law
included), packaging (owned-Gradle is the ruled Android default,
D-829). The profile's *answers* are this lane's; only the block
pattern is shared mechanics.

Steps: instantiate the block (every item `inherited`) → Owner
confirms as a block or re-rules items, each outcome ledgered with
its ruling reference → name-clearance scan (package/namespace,
storefront, trademark with secondary-source legs flagged and the
direct-register step named as owed, domain) — the pick is
conditional until the scan returns, adoption requires a
note-free sheet or an Owner ruling on its notes →
repo seeded per lumen-estate conventions → genesis closes with
the sheet recorded as the project's first conformance artifact.

Placement: the clearance scan is a **procedure in the skill**
(research-shaped; needs no server state). The defaults ledger is
graduation candidate (a) in §0. Determinism is record stability:
the sheet is canonical JSON (key-sorted); the seed manifest is
hash-pinned per file; the ledger is append-only. Refusals:
`defaults_unconfirmed`, `name_conditional`, `clearance_collision`.

## 4. Pipe 2 — Canon as Data

*Gate of record: `lumen-canon-census 1.0`, beside the shipped
`crates/lumen-canon` machinery.*

A title's canon — characters, setting facts, what may never
appear in shipped text — is typed data a machine can verify.
Records are `canon-record/1` (shared format; SFW records home in
lumen, fetched by handle under the protocol's flow rules: an
`sfw`-labelled record may be fetched by any caller; these pipes
never fetch across the wall for content, only formats cross).

The census (a gate script in this commission, graduation
candidate (b)) checks, over a project's record set + manifest:
**counts** (declared vs found, per table); **orphans** both
directions (every manifest entry resolves to a record; every
record is manifested; every referenced id exists);
**differentiation** per declared cast pair (name equality is a
collision outright; descriptor overlap is reported numerically —
the machine reports, the Owner decides); **sanitization** over
every shippable text field against the project denylist (the
teaching games' law: no studio name, no real project, no real
person in any in-game text — hits are findings with field
locations, never silent edits); **staleness** (the census records
the state hash it ran against; a canon write after a census makes
that census stale).

"Canon verified" means exactly: a `lumen-canon-census 1.0`
artifact exists whose recorded state hash equals the current
canon state hash, with zero orphans, differentiation verdicts
recorded, and zero sanitization hits in shippable fields.
Determinism: the census is a pure function of canon state under
canonical serialization — same state, same census bytes, same
hash, on any machine. Refusals: `canon_unverified`,
`census_stale`, `canon_orphan`, `differentiation_collision`,
`sanitization_hit`.

## 5. Pipe 3 — Content Graphs with Gates

*Gate of record: `lumen-content-census 1.0` plus the seeded
playtest run.*

Content — scenes, dialogue, arcs, beats, quests, levels — is one
graph that carries its own proof: every unit names its gate,
every unlock condition is declared data, and an integrity census
keeps the graph honest as it grows. Unit states: `ungated`,
`gated` (with the run id that gated it), `sealed`. A graph edit
after a gating run returns the unit to `ungated` (the Pipe 2
staleness rule applied to graphs).

Difficulty is expressed as **contract packs** layered over sealed
content — the Contract Board is Versemend's own design (this pipe
is its homecoming): a pack is a named set of conditions, each a
transformation of the unit's test suite (widen the hostile-input
half, permute execution order under recorded seeds, impose a step
budget, raise determinism runs from two to five, halve a stated
budget). Two laws ride with it: **mastery never requires a
contract**, and a contracted completion is distinguishable in the
record forever. Pack design is a skill procedure; census,
sealing, and pack application are gate scripts (graduation
candidates under §0's rule if enforcement is ever needed
in-server). Refusals: `graph_orphan`, `unlock_undeclared`,
`contract_requires_seal`, `census_stale`, `pack_touches_safety`
(a pack that would impose a cost on a safety surface is refused
at definition time — no pipe gates, scans, or prices a safety
surface, in this lane exactly as in the other).

## 6. Pipe 4 — Art Production

*Gate of record: `lumen-keeper-chain 1.0`.*

Every visual asset a title ships arrives the way the teaching
games' keepers did: produced from a written record, under an
adopted canon, through deterministic finishing, carrying a
complete keeper chain. The chain field set is ART-BIBLE.md §1.1
(the D-836 bible is this lane's own): asset id, record id, canon
references with hashes, verbatim prompt pair, model checkpoint +
hash with sampler parameters, conditioning models + weights,
seed, ordered finish operations, final SHA-256, verdict with
ruling source. An asset without a complete chain is not a
keeper, whatever it looks like.

Steps: **record before pixels** (Pipe 2 verified state, or
`record_missing`) → **generation through session records** — in
this lane, generation is brokered by lumen itself (its
generation lane and the D-838 lift build: generated media
arrives flat, lift decomposes it back into addressable
structure, and the media is legally a render of the session
record) → **deterministic finishing only** (named, parameterized,
ordered operations, replayable byte-identically from the chain,
or `finish_not_replayable`) → **chain completion and
registration** (`chain_incomplete`, `chain_hash_mismatch`) →
**canon adoption and change by D-629 verdict pack only** — a
canon is an authority set + locked palette + style prompt block
+ drift check, adopted only by the Owner's ruling on a numbered
pack; production against a candidate canon refuses with
`canon_unadopted` → **drift check on every production asset**
(palette-role comparison + prompt-block conformance run
numerically and attached; the side-by-side leg at phone size is
the Owner's eye, recorded as a ruling — the pipe never declares
a visual PASS, D-221/D-629 discipline; failure is `drift_failed`
with the measurement attached, never a silent re-baseline) →
**custody** (lifted nodes a human has touched are frozen against
regeneration; a proposal touching a frozen node is refused and
the refusal recorded verbatim; only a new Owner ruling thaws,
and the thaw is itself a chain entry).

Placement: verdict-pack assembly is a skill procedure; adoption
is the Owner's ruling, recorded. The chain validator is a gate
script today and graduation candidate (c). Until session
records land for every route, the chain's session-record field
carries the typed state `absent-unverified` — absence is a
stated state, never a silent gap.

## 7. Pipe 5 — Determinism + Ship

*Gate of record: `lumen-handoff-packet 1.0`.*

The loop closes: every determinism claim is proven by a second
bake, every engine and packaging choice is an inherited ruled
default or the product of the spike method, and a release leaves
as a validated handoff packet whose digests, checklist
freshness, and gate states have been checked by name.

Steps: **double-bake** every artifact any pipe calls
deterministic — produced twice from recorded inputs, hashes
compared, a mismatch is a finding with both hashes recorded
(`double_bake_mismatch`), never a re-run-until-it-passes →
**engine/packaging by inheritance or spike** (owned-Gradle is
the Android default, D-829; a spike's verdict is evidence for an
Owner ruling, never a ruling by itself — the D-861 Versemend
host adoption is this step executed) → **assemble channel
artifacts** per target on living, per-publisher checklists →
**preflight by name**: channel known and unflagged; checklist
fresh (≤ 30 days, verified from the publisher's own sources) and
not UNVERIFIED (`checklist_stale`); canon census current (Pipe
2); every shipped asset's chain valid (Pipe 4); every
deterministic artifact double-baked → **hand off and record**:
the packet passes to its executor (butler for itch; the
qompass.ai Route A flow — release files on the title's own
GitHub repo, the site records and links, D-855/D-856 — for site
distribution; slipway where a channel is slipway's), and the
shipped byte set's digests enter the project ledger. Executor
gates stay the executors'; this pipe forks nothing.

Determinism: packet bytes are canonical; digests are SHA-256
over shipped bytes; the ledger's shipped-digest entry makes the
released byte set permanently answerable. Refusals: the lane
codes, `double_bake_mismatch`, `checklist_stale`,
`packet_digest_mismatch`, plus Pipes 2/4 codes surfaced by name
through the preflight — a refused handoff reads as a list of
named failures, never a single opaque no.

## 8. Conformance — proving a pipe was followed

Every pipe run writes a **run record** (canonical JSON) beside
its artifacts, containing: pipe id + pipe version; gate-of-record
id + version; the lane stamp (§1); input state hash (the exact
state the run read); output artifact hashes (every artifact the
run produced or validated); verdict in the pipe's own terms, with
refusal codes where refused; ruling references for every Owner
decision the run relied on. No credential material of any kind
appears in a run record.

Conformance is a reading exercise: any reader can re-derive
every verdict from the run records and the artifacts they hash.
A verdict whose run record is missing, or whose hashes do not
match the artifacts on file, is **REVIEW** under D-713 — not
passed, not failed, unverified — and every downstream gate
treats REVIEW as not-yet-run. The shared tooling ships a
verifier (`scripts/pipes/run_record.py --verify`) that makes
this check mechanical; Stage 0's gate exercises it, including a
tampered-hash case that must come back REVIEW.

## 9. The worked machinery (Stages 1–3)

The pipes are proven on lane-neutral machinery that already
exists in lumen; the stages below are this commission's worked
evidence, each with its own section in PIPELINES-REPORT.md.

- **Objects + clothing (Stage 1)** ride the rig stack's
  part-variant machinery: a part variant is data — sibling parts
  under set parameters (a set parameter binds a part's
  `transform.s.x`/`transform.s.y` to [0, 1]; at 0 the part is
  absent from the render, not hidden by a flag), palette slots
  are sibling colourways under one palette parameter, and
  articulation is a deform binding. The gate is the RENDER-1
  sweep, applied per parameter: driven alone at its extremes, a
  parameter must change > 0 pixels (per-channel diff > 8) with
  ≥ 98% of changed pixels inside the union of its bound parts'
  AABBs (dilated). Fixture puppets are TEST plates, labelled as
  such; mechanics never block on keeper art, and art verdicts
  remain the Owner's eye.
- **Effects (Stage 2)** ride `lumen-fx/1`, the lane-neutral
  timeline format (in-tree definition: `docs/pipes/FX-FORMAT.md`):
  seeded emitters + curve timelines, fixed-step integer
  evaluation, a per-step digest chain ending in a replay hash,
  and the photosafety gate as an automated check — the flash
  method of Versemend's design §8.3 as shipped in its
  `effects.lua`: a flash is a swing of ≥ 0.08 of full-field
  luminance, up and back, counted by the extrema walk over the
  intensity track sampled at 120 Hz; the cap is 3 flashes/second.
  The format is shared with the NTRanch commission **by format
  only**: the format document is the whole seam; neither lane
  reads the other's effect content.
- **Enemy genesis (Stage 3)** is seeded procedural variation on
  the character pipeline: a family record (base stat block +
  behavior block, all as data) plus variant axes; the generator
  (SplitMix64, the estate's seeded idiom) emits a canonical
  variant table whose SHA-256 is the genesis hash, pinned in a
  checked fixture — the Niflheim floor-genesis precedent (its
  genesis value is pinned in the game's own unit tests). A
  re-run must reproduce the pinned hash exactly (double-bake).

## 10. UNVERIFIED / open register

1. **Stage 2 format convergence.** The NTRanch commission's
   effects stage publishes its own format document; convergence
   with `lumen-fx/1` is by format comparison when it lands. Any
   divergence is reconciled as a format revision, never by
   importing content. *Resolution: read their format doc when it
   exists; this commission does not block on it.*
2. **Gencond branch landing** (`pax/gencond-stage1`) and the
   D-838 lift builds' final field names — Pipe 4's session-record
   intake must re-read the landed reports before fixing field
   names; until then the `absent-unverified` typed state holds.
3. **Owned-Gradle template production home** — the template
   lives in the D-829 adaptation commission's tree; Pipe 5
   references it by version + hash once that commission fixes
   them.
4. **D-859 acceptance scene** — the full-rigs build's Stage 4
   ( Prima lifting Alma in the evaluator) is that commission's
   gate, in flight at this writing; Stage 1 here depends only on
   the evaluator + authoring machinery, which is landed and
   evidenced (see PIPELINES-REPORT.md, dependency disposition).

*PIPES.md ends. Companion: PIPELINES-REPORT.md (stage gates,
evidence, deviations) in this directory.*
