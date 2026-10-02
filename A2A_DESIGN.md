# A2A_DESIGN.md — lumen as an A2A agent

MCP is lumen's hands (one client using tools interactively). A2A is
lumen's dispatch surface (agents handing each other long-running art jobs).
Both wrap the **same operation core** — no duplicated logic.

## Binding

A2A v0.3.x JSON-RPC, the same binding diver's `lua/ai/a2a/` speaks:

- `message/send` — dispatch one art task.
- `message/stream` — server-sent progress events while the task runs.
- `tasks/get` — poll task state and artifacts.
- `tasks/cancel` — cooperative cancellation; the running pipeline stage
  finishes its current bounded step, then the task moves to `canceled`.

Wire rules (verified against diver's implementation): lowercase
`role:"user"`, `kind` discriminators on parts, kebab-case task states.

## Agent card

Served at `/.well-known/agent-card.json`. Advertises capabilities:
`sprite-core`, `lightshow-pipeline`, `fidelity`, `styles`, `dream-automation`,
`export-backends` (`wasm4`, `tic80`, `godot` when landed), `bevy-bridge`,
`policy`. A v1.0 card with `supportedInterfaces[]` is accepted for
discovery. The card is generated from the tool registry so it cannot drift
from the implementation.

## Task states

`submitted -> working -> completed`, with `input-required`,
`failed`, `canceled`, `rejected`. Terminal states carry artifacts
(file paths) or a structured error — never a bare string.

## The art-director loop as protocol

Long jobs pause at `input-required` with the review artifact attached:

1. Coordinator sends task: "trial mature variant + contact sheet for Ondine".
2. lumen works, streaming progress (`message/stream`).
3. Task moves to `input-required`; artifact = contact sheet path.
4. Human (or director agent) approves or requests changes.
5. On approval the same task resumes to fleet rollout; on rejection it
   moves to `rejected` with reasons, and the trial artifacts are kept.

This is the Seraphine trial workflow formalized: trial on one character,
human eye on the sheet, fleet only after approval.

## Parallel fan-out

Task payloads are designed for coordinator fan-out: one task per
character or per mood row, with disjoint file ownership declared in the
payload so parallel lumen workers never write the same path. Workers are
addressable as separate A2A endpoints (e.g. per-host on primo).

## What is NOT in v1

Multi-agent negotiation, agent-to-agent payments, and reputation/trust
scoring. Trust between agents is expressed through the policy layer
(POLICY_DESIGN.md), not a separate mechanism.
