# Architectural Decisions — lumen

## Parametric factory, not neural (2026-10-03)

**Decision**: Build a parametric character factory (canonical rigged base
+ identity projection) instead of neural single-view reconstruction.

**Context**: Matt directed lumen to convert 2D sprites to high-quality
editable 3D characters. Neural methods don't give editable body parts
or character-specific expression sets.

**Consequence**: Canonical Mixamo-compatible rigged base; separate
hair/outfit meshes; blendshape expression set per character.

## GLB/glTF 2.0 canonical interchange (2026-10-03)

**Decision**: GLB is the canonical interchange format.

**Context**: Only format receivable by all four targets (Blender/Godot
native, Unreal Interchange, Unity plugin-based). VRM 1.0 is nearly free
(VRMC_vrm block on GLB; plain PBR legal, MToon not required).

**Consequence**: Export GLB-first; adapters for VRM/FBX/USD/FBX per target.

## Mature set default (2026-10-03)

**Decision**: Mature companions are the default factory target.

**Consequence**: Canonical base and identity pipeline built for mature
set first; base set follows.

## Blender first validation (2026-10-03)

**Decision**: Blender is the first real validation/export gate.

**Consequence**: Prove the pipeline in Blender before Unreal/Unity/Godot.
