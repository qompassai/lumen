# Current Work — lumen

2D sprite tools + parametric 2D→3D character factory.

## Active (2026-10-04)

- **2D→3D pipeline**: Parametric character factory (canonical rigged base
  + identity projection from sprite). NOT neural single-view reconstruction.
  Mature companions are the default target. Sprite/portrait is design
  authority. First export target: Blender. Canonical interchange: GLB/glTF 2.0.
- **Format research**: Complete (`~/workspace/lumen-2d3d/format-research.md`).
  GLB confirmed canonical (only format receivable by Blender/Godot native,
  Unreal Interchange, Unity plugin). VRM 1.0 nearly free on top of GLB.
- **Live interop failures** (fix curatively, never document as limitations):
  1. Aseprite truncated-file behavior
  2. Bevy BRP malformed-query HTTP 200 behavior
  3. Registry schema crate-filter behavior
- **Pending**: CUDA execution restore/prove; `export_tag_animations` MCP
  registration; LeakyReLU + Resize lowering; pinned `uv` environment;
  protobuf conflict repair.

## Standing rules

- Scoped push authorization: qompassai/lumen (gated). Byte-identical
  remote verification, never force-push.
- Curative fix policy: fix failures in code, never file as known limitations.
- Skill: `~/workspace/skills/lumen-sprite-to-3d/SKILL.md`.
