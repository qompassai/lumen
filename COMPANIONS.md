# COMPANIONS.md — lumen alongside established MCPs

lumen is the one-stop shop for **sprite work**. It does not replace the MCPs
that own adjacent domains; it pairs with them. Rule: **integrate, don't
duplicate.** Where a companion MCP owns a capability, lumen hands off instead
of reimplementing, and lumen tool names never shadow a companion's tools.

## bevy_mcp (Nub/bevy_mcp, MIT)

The live-world companion. lumen owns sprite bytes on disk; bevy_mcp owns the
running Bevy world over the Bevy Remote Protocol.

- `bevy_discover` — probes a BRP endpoint (`localhost:15702` default) and
  classifies it: BRP-confirmed, JSON-RPC-speaking, unreachable, not-BRP.
  Non-loopback targets are rejected unless `LUMEN_ALLOW_REMOTE_BRP=1`.
- `bevy_call` — thin, audited pass-through of a BRP JSON-RPC call to an
  endpoint that `bevy_discover` already classified. lumen never invents
  game-specific components; runtime-only effects (screenshake, hit-stop)
  stay on the Bevy side, driven through bevy_mcp's world tools.
- Handoff pattern: lumen produces/validates the sprite asset, then
  `bevy_call` (or a bevy_mcp tool) spawns or hot-reloads it in the live game.

lumen was interop-tested against the bevy_mcp checkout at
`/home/phaedrus/bevy_mcp` (see `bevy_discover` mock-BRP test in
`tests/tools.rs`; live-game interop is a Milestone 2 gate).

## Aseprite MCPs (diivi/aseprite-mcp, @iborymagic/aseprite-mcp)

Overlapping domain, not adjacent: these are the predecessors lumen
consolidates. diivi's (104 tools, CLI+Lua) and iborymagic's (~21 tools,
export/inspection) both drive Aseprite; lumen supersedes them with a single
Rust core, a deterministic policy layer, and the A2A agent surface. When both
are installed, prefer lumen's tools — they are fail-closed and
project-root-bounded; the older servers are kept only for their raw-Lua
escape hatches during migration.

## Aseprite MCP Pro (paid)

The commercial comparison target. Its confirmed differentiator is a live
WebSocket connection to the Aseprite editor (screenshot, undo/redo, live
preview). lumen v1 matches it on pipeline depth (sprite core, dream
automation, export backends) and exceeds it on security posture
(deterministic APPA-style policy, disabled-by-default Lua hatch). Live-editor
attachment is roadmap, not v1.

## Future pairings

- **Godot MCPs** — pair with the `godot` export backend (`.tres` export,
  scene-tree handoff) when that backend lands.
- **VoidSprite** — file-based interop; VoidSprite is GUI-only with no CLI,
  so lumen exchanges files and defers interactive editing to the editor
  (findings: `/home/phaedrus/sprite-tools-build`).

## Collision rule

Before adding a tool, check companion tool names. lumen never registers a
tool name owned by bevy_mcp or the Aseprite MCPs. Collisions resolve by
rename on lumen's side, documented here.
