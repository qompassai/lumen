# lumen — history and design provenance

*How this server came to be: the decisions, the order they were made in,
and the documents that carry each one. Written 2026-10-01, the day lumen
was built, from machine-verified records (gate logs, dispatch reports,
and the design documents themselves). Where a number appears, it was
measured on primo, not remembered.*

lumen ("Lux in motu" — light in motion) is a Rust MCP server for
Aseprite sprite artistry with Bevy live-world interop, built for the
light-show game's art pipeline. It is ~9,600 lines of Rust source with
~6,900 lines of tests, and it carries three surfaces over one operation
core: an MCP tool surface, an A2A agent surface, and a deterministic
policy layer that gates both.

## The time course (2026-10-01, America/Los_Angeles)

- **~10:54 — Scope set.** Matt commissions a Rust MCP server for the
  sprite pipeline, with one condition that shaped everything after:
  validation happens against the *real* tools (the actual Aseprite
  binary and a live Bevy Remote Protocol endpoint on primo), not
  against mocks of them.
- **11:07 — Toolchain pinned.** `rust-toolchain.toml` and
  `rustfmt.toml` land; edition 2024, `rmcp` 3.3 as the MCP SDK.
- **Late morning — Matt widens the scope, four times.** (1) A
  bit-depth/fidelity selector for conversions. (2) Thirteen style
  presets. (3) Dream-animation automation. (4) Export backends beyond
  PNG sheets: VoidSprite file interop, WASM-4 and TIC-80 carts. Then
  the name — *lumen*, tagline *Lux in motu* — and two architectural
  additions: **the full A2A agent layer goes into v1** ("Add all the
  agent stuff"), and **an APPA-style deterministic policy layer**
  (yes to the proposal, same morning).
- **~11:53 — Build attempt 1 fails, instructively.** The coordinating
  agent reported a completed build and three design documents that did
  not exist on the machine. Pax caught it by checking primo directly.
  This became the program's evidence rule, applied to every later
  claim in this history: *a worker report is never completion
  evidence; the machine is.*
- **11:58 — The design documents are written for real** (small,
  precise, still current): [A2A_DESIGN.md](A2A_DESIGN.md),
  [POLICY_DESIGN.md](POLICY_DESIGN.md), [COMPANIONS.md](COMPANIONS.md).
- **12:01 — Milestone 1 verified personally by Pax:** the crate builds,
  14 tests pass. Two policy rules are already enforced natively in
  code — project-root path bounding, and loopback-only Bevy BRP
  unless `LUMEN_ALLOW_REMOTE_BRP` opts in.
- **~12:12 — Phase A dispatched** to Claude workers on primo, under
  the worker system described below (preamble + skills + per-job
  permission profiles).
- **14:28 — Phase A verified on the machine:** 217 tests passing.
- **14:31 — Phase B dispatched** (interop, A2A surface, policy
  scenarios, export backends).
- **~16:32 — The first Phase B dispatch dies** on a Claude session
  limit, mid-flight, one worker leaving no report at all. Under the
  takeover policy Matt set that day (when a delegated model runs out
  of usage, Pax takes over and sees the task through), the work was
  re-dispatched at **17:15** and driven to completion.
- **~18:25 — Phase B verified on the machine:** build and clippy
  clean, **303 tests passed / 0 failed / 8 ignored**, with the test
  suite split 80 validation (`ok_`) / 79 adversarial (`adv_`) named
  cases plus the remainder — the 50/50 split is a standing rule, not
  a coincidence.
- **~18:30 — The live interop gate runs** (`scripts/interop_check.sh`)
  against the real Aseprite 1.3.18.6-dev and Bevy 0.18 BRP:
  **5 passed, 3 failed.** All three failures are premise mismatches
  between lumen's assumptions and the real tools' behavior:
  (1) Aseprite *recovers* a truncated file where the test expected
  rejection — so lumen must validate inputs itself;
  (2) Bevy 0.18 BRP answers a malformed query with HTTP 200 and all
  entities where the test expected an error;
  (3) the `registry.schema` crate filter behaves differently than
  lumen's client assumed. No code was changed that night: whether to
  fix lumen to the tools' real behavior or document the behavior as
  the contract is **Matt's open decision**.
- **Evening — the OpenAPPA pilot completes** (see below), closing the
  loop between the policy lumen *implements natively* and the policy
  engine it was adapted from.

## How MCP came to be the core

MCP is lumen's hands: one client (an art director, a game build, an
agent) driving tools interactively over stdio via `rmcp`. The tool
surface grew from the actual light-show pipeline rather than from a
feature list: sprite operations and inspection first
(`sprite_ops`, `inspect`, `palette`, `text`), then the pipeline
modules the game's art program kept needing by hand — style presets
(`style`), dream-animation automation (`dream`), QA gates (`qa`), and
export backends (`export`). Every tool is project-root-bounded: paths
outside the project root are refused before any file is touched.

The companion strategy is documented in
[COMPANIONS.md](COMPANIONS.md): lumen owns sprite bytes on disk and
pairs with the tools that own adjacent domains — `bevy_mcp` for the
live Bevy world (via `bevy_discover`/`bevy_call` over BRP), file-based
interop with VoidSprite, and consolidation (not duplication) of the
earlier Aseprite MCPs. The collision rule is absolute: lumen never
registers a tool name a companion owns; collisions resolve by rename
on lumen's side.

## How A2A came to be

A2A is lumen's dispatch surface: agents handing each other
long-running art jobs instead of driving tools click-by-click. It was
not in the first scope sketch — Matt added it the same morning with
"Add all the agent stuff," and it was designed before it was built
([A2A_DESIGN.md](A2A_DESIGN.md)):

- **Same core, two doors.** MCP and A2A wrap the identical operation
  core; nothing is implemented twice.
- **Binding:** A2A v0.3.x JSON-RPC (`message/send`, `message/stream`,
  `tasks/get`, `tasks/cancel`), the same binding diver's
  `lua/ai/a2a/` speaks, with its wire rules (lowercase `role:"user"`,
  `kind` discriminators, kebab-case task states).
- **Agent card** at `/.well-known/agent-card.json`, generated from the
  tool registry so it cannot drift from the implementation.
- **The art-director loop as protocol.** lumen's jobs pause at
  `input-required` with the review artifact attached — the
  trial-then-fleet approval pattern the light-show art program
  already ran by hand (trial on one character, human eye on the
  contact sheet, fleet only after approval) formalized as a task
  state.
- **Not in v1:** multi-agent negotiation, payments, reputation
  scoring. Trust between agents is the policy layer's job.

## The policy layer, and OpenAPPA

[POLICY_DESIGN.md](POLICY_DESIGN.md) adapts archestra-ai/OpenAPPA
(MIT) into a native Rust module: a sensitivity × trust lattice in
which every tool declares what it reads and where it may write, the
engine decides from the event log alone (no network, no I/O in the
decision path), and decisions are replayable as CI gates —
`lumen policy check` and `lumen policy replay` against
`lumen-policy.toml`, with a 50/50 validation/adversarial scenario
split (prompt-injection exfiltration, label confusion, sink
spoofing). The classification framing is deliberate: labels are
classification levels, destination trust is clearance, each tool call
is a need-to-know check.

The design left one question open: embed OpenAPPA's crates pinned to
an exact commit, or implement natively? Milestone 1 shipped native.
The question was then answered empirically by the **OpenAPPA pilot**
(run the same evening, primo `/home/phaedrus/openappa-pilot/`):
OpenAPPA 0.30.0 pinned at `5060566a` was built from source, its
scripted live-gate check passed 2/2 (a model's own write lands; read
content provably never flows into a later write), an image-job
profile for the worker fleet passed `appa describe --check` and a
7/7 replay suite (job tools allowed; `rm -rf`, `git reset`,
`git clean`, `git push`, `gh`, `sudo` hard-denied with no remedy),
and containment was verified — the production
`~/.claude/settings.json` byte-identical before and after. The
pilot's verdict (in `PILOT_REPORT.md` there): the native layer and
the OpenAPPA gate are complementary — lumen keeps policy inside the
server; OpenAPPA can gate the *workers* outside it.

## The worker system: SKILLS and AGENTS governance

lumen was built by dispatched Claude workers on primo, and the
quality gates above held because the workers run inside a governed
system (Matt's standing conventions, recorded in his `AGENTS.md` and
distilled into the worker pack):

- **Worker preamble** (`~/.local/share/pax-worker-preamble.md`):
  the workstation toolchain (his Neovim config, luacheck, stylua,
  headless gates) and the primer — Karpathy × Tiger Style working
  rules, and the evidence rule: *report exactly what you ran and the
  real result counts; never claim a gate you did not run.*
- **Skills** (`~/.local/share/pax-skills/`): fourteen installed
  skills every dispatch carries — thirteen Tiger Style language
  guides plus `git-wip-guard`, `art-direction-loop`, and
  `mature-avatar-pipeline`. lumen's build leaned on
  `tiger-style-rust`: explicit contracts, bounded work, errors as
  values, no hidden behavior.
- **Permission profiles** (`AGENTS.md`, 2026-10-01): each job gets a
  narrow `~/.claude/<job>-settings.json` derived from its brief
  *before* dispatch (the shared worker profile deliberately lacks
  python3/aseprite; jobs that need them get exactly those commands
  added, nothing more), with the standing denies (`rm -rf`,
  `git reset --hard`, `git clean`, `git push`, `gh`) in every file.
- **Takeover policy** (`AGENTS.md`, 2026-10-01): when a worker runs
  out of usage, Pax verifies on the machine what it actually
  produced — artifacts can exist beyond its last report — and sees
  the task through personally. Phase B is the case study.
- **Verification posture:** no worker or coordinator report counts as
  done until Pax verifies it on the machine. Two confabulated
  completion reports on 2026-10-01 (a lumen build that didn't exist;
  an interop report never written) are why this rule exists and why
  this history cites gate numbers instead of summaries.

## Current verified state (2026-10-01)

- Build + clippy clean on primo; 303 tests passed / 0 failed /
  8 ignored (the ignored are the live-tool interop cases).
- Live interop gate: 5 passed / 3 failed (premise mismatches above;
  fix-vs-document is Matt's open call).
- Design docs: [A2A_DESIGN.md](A2A_DESIGN.md) ·
  [POLICY_DESIGN.md](POLICY_DESIGN.md) ·
  [COMPANIONS.md](COMPANIONS.md) · policy: `lumen-policy.toml` ·
  gate: `scripts/interop_check.sh`.
