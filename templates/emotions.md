# Lumen Emotion Templates

Ratified by Matt, 2026-10-06. Machine-readable source of truth:
`emotions.json` in this directory. This document is the human companion: what the
templates are, and how each medium and engine consumes them.

## Purpose

One canonical key per emotion, used identically across every medium the
companions appear in:

- **Sprites** — the 2D companion sheets and their face rows.
- **Portraits** — the dialogue expression sheets. The game's
  `Expression` enum (`game/src/waifu/dialogue_ui.rs`) ships only `Neutral`
  today; the "11-expression sheets" it anticipates are Neutral plus the
  ten templates below.
- **3D models** — animatable full models from the Lumen factory, as morph
  targets / blendshapes in Blender, Unreal, Godot, Redot, and Unity.

Same key, same face, every medium. A portrait, a sprite, and a 3D model
of the same companion in `worried` must read as the same expression.

## The canonical set

| Key | Face | VRM 0.x | VRM 1.0 |
|---|---|---|---|
| neutral | rest face | neutral | neutral |
| happy | warm, genuine pleasure | joy | happy |
| angry | real anger, protective | angry | angry |
| sad | sorrow, disappointment | sorrow | sad |
| surprised | startle, astonishment | surprised | surprised |
| fun | **playful**, teasing, mischievous | fun | custom |
| annoyed | irritation, deadpan | custom | custom |
| worried | anxiety, concern | custom | custom |
| embarrassed | flustered, caught out | custom | custom |
| determined | resolve, work face | custom | custom |
| smug | self-satisfied, superior | custom | custom |

Two decisions are baked in and are not defaults to drift away from:

- **`fun` means playful.** VRoid Studio's `fun` preset is an amused,
  teasing face; VRM 1.0 renamed the slot `relaxed`, which reads as calm.
  The canonical sculpt keeps the playful meaning and ships as a custom
  expression in VRM 1.0. Do not alias it to `relaxed` on export.
- **The first five keys are the VRM emotion presets verbatim** (under
  their 1.0 names), so preset tooling, VRoid references, and VRM export
  all line up. The remaining five are customs in every spec version.

## How each medium uses a template

### Sprites (2D)

Each template's `sprite_2d` entry describes the face in drawable parts —
brows, eyes, mouth, extras — at sheet scale. Draw the parts, not an
impression: at 288×576 the brow angle and mouth shape carry the read.
Overlays (tears, sweat drops, blush marks) are separate conventions
layered on top of templates, never baked into them.

### Portraits (2D)

Same `sprite_2d` recipes, at portrait fidelity: lid position, gaze
direction, and brow tension become visible and matter. File naming is
`{companion}_{key}` (e.g. `seraphine_worried`), matching the `Expression`
enum variants one-for-one so `portrait_path` resolution stays a table,
not a translation layer.

### 3D models

Each template's `arkit` entry is a recipe in the 52-channel ARKit
blendshape vocabulary: channel weights from 0.0 to 1.0, unlisted channels
at 0. Sculpt the morph target for an emotion so that posing the face with
its recipe reproduces the template's face. The weights are v1 starting
values, expected to be tuned per character against renders; the keys, the
VRM mapping, and the playful-`fun` semantics are the stable contract.

Emotions are a **layer**, never a bake. Three procedural groups compose
on top of any emotion and are defined once per character, following the
VRM 1.0 grouping:

- **blink**: `blink`, `blinkLeft`, `blinkRight`
- **gaze**: `lookUp`, `lookDown`, `lookLeft`, `lookRight`
- **visemes**: `aa`, `ih`, `ee`, `oh`, `ou`

Two emotions carry non-morph components: `embarrassed` requires the blush
layer (a texture/shader effect in 3D, drawn marks in 2D) or it
under-reads; `fun` and `smug` are defined by their asymmetry — sculpted
symmetric, they collapse into `happy` and lose their meaning.

Animation notes per template live in `emotions.json` (surprised animates
in fast and decays; annoyed caps the blink layer; determined is angry
minus the sneer — tune brow depth first if they blur together).

## Engine notes

- **Blender** — morph targets are shape keys; keep canonical keys as
  shape-key names. The glTF exporter writes them as morph target names
  (`extras.targetNames`), which is what the Lumen gate verifies.
- **Unreal** — Live Link Face and MetaHuman tooling speak the ARKit
  channel names directly, so the recipes transfer as-is; finished
  emotion poses can be stored as Pose Assets / curve sets keyed by the
  canonical names.
- **Godot / Redot** — glTF morph targets import as blend shapes with
  names preserved; drive them from `AnimationPlayer` tracks or code by
  canonical key. Redot follows the same importer behavior.
- **Unity** — blendshapes keep their names through FBX/glTF import; for
  VRM delivery, UniVRM binds the five preset keys natively and the five
  customs (plus `fun`) as custom expressions.
- **VRoid Studio** (reference, not a target) — its exports carry the
  VRM 0.x preset names (`joy`, `fun`, `sorrow`); use it as the visual
  reference for the five core faces.

## Existing sheet moods, in canonical terms

The shipped 2D companion sheets use six mood rows (`sprite.rs`
`MOOD_ROWS`). They are not a separate system:

| Sheet mood | Canonical reading |
|---|---|
| idle | neutral |
| blush | embarrassed (cheek component; over neutral or low happy) |
| wink | blinkRight held, over happy or fun |
| pout | annoyed with pushed lower lip |
| celebrate | happy at full weight, plus pose |
| alarmed | surprised with worried brow tension |

## Versioning

`emotions.json` carries `schema` and `version`. Changing a key, a VRM
mapping, or the `fun` semantics is a contract change: bump the version
and regenerate models. Tuning a weight is not: note it in the tuning log
of the character's model README instead.
