# POLICY_DESIGN.md — deterministic policy layer (APPA-style)

Adapted from archestra-ai/openappa (MIT): deterministic guardrails that
answer one question before every tool call — **is this data allowed to go
to this destination?** Same event log, same decision, every run. No
probabilistic classifiers in the decision path.

Classification analogy: sensitivity labels are classification levels,
destination trust is clearance, and each tool call is a need-to-know check.

## Model

- Every tool declares **input sensitivity** (what it reads) and **output
  trust requirements** (where it writes).
- Sources: project sprites (`low`), proprietary reference sheets (`high`),
  licensed style references (`high`), web-fetched content (`untrusted`).
- Sinks: local project writes (`trusted`), cart exports wasm4/tic80
  (`public`), Bevy BRP (`loopback` vs `remote`), A2A tasks to external
  agents (`external`).
- The engine decides from the event log alone: no network, no file I/O in
  the decision path. Decisions are replayable.

## Default policy (ships as `lumen-policy.toml`)

1. `high`-sensitivity reference art never flows to `public` sinks
   (wasm4/tic80 cart exports) or `external` A2A tasks.
2. `run_lua_script` (raw Lua hatch, disabled by default, explicit opt-in)
   refuses when `untrusted`-sourced data is in context — the
   prompt-injection exfiltration case.
3. `restricted` data never POSTs to a non-loopback BRP target
   (complements the `LUMEN_ALLOW_REMOTE_BRP` opt-in already enforced in
   `bevy_discover`).
4. Deny-by-default: any flow not explicitly allowed is denied, with the
   denial naming the rule, the label, and the sink.

## Enforcement point

A `policy` module wraps the tool router: each `tools/call` is checked
against `lumen-policy.toml` **before** the tool executes. Denials return a
structured error naming the violated rule; they are not silent.

## Replay gates

Policy ships with scripted scenarios and expected allow/deny decisions:

- `lumen policy check` — validates that the TOML loads and every tool has
  labels.
- `lumen policy replay` — runs the scenario suite, fails CI on any
  unexpected decision. 50/50 validation/adversarial split: adversarial
  scenarios cover prompt-injection exfiltration, label confusion
  (high-sensitivity data relabeled low), and sink spoofing.

## Embed vs native (decision)

Evaluate embedding openappa's `appa-engine`/`appa-runtime` crates pinned
to an exact commit (the project is preview/RFC — never track main). If the
pinned API is too heavy or unstable, fall back to a native Tiger-Style
implementation of the same sensitivity x trust lattice. Either way the TOML
policy surface and the replay gates are identical, so the choice is
invisible to users. Milestone 1 already enforces two policy rules natively
in code: project-root path bounding and the loopback-only BRP default.

## Non-goals

The policy layer does not do content moderation, license enforcement beyond
labeling, or DRM. It controls **flows**, not meaning.
