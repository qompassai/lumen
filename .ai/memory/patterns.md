# Patterns — lumen

## Export gotchas (from format research)

13 concrete gotchas documented in `~/workspace/lumen-2d3d/format-research.md`.
Key ones: Unreal Interchange route (not native), Unity plugin-only GLB,
VRM license defaults must be set deliberately, Mixamo naming provisional.

## Testing

- Unit suite: 372 tests (after raw-bytes-bridge + Godot-loop fixes).
- Live interop gate: 5 passed, 3 failed (the 3 failures are the curative-fix
  list in current.md). Never call interop proven until live gate passes.
- Treat external tools' actual behavior as the contract, not assumptions.

## Push

- Scoped auth: qompassai/lumen. Remote main == local, byte-identical.
- Repo-seeding lesson: zero-commit repos 409 on blob endpoint — seed via
  one Contents-API PUT, then force-update to real root commit.

## Skill

`~/workspace/skills/lumen-sprite-to-3d/SKILL.md` — mirrored to primo
`~/.claude/skills/`, `~/.local/share/skills/`, `~/.local/share/pax-skills/`.
Status banner updated as milestones land.

## Repomap (codebase map for agents)

One-shot generation (no flake wiring in this repo):

```
nix run github:qompassai/nix?dir=repomap -- /path/to/repo --budget 15000 --out .repomap.txt
```

`.repomap.txt` is a derived artifact — gitignore it, never commit it.
For automatic regeneration on `nix develop`, wire the flake input per
github.com/qompassai/nix/tree/main/repomap/README.md.
