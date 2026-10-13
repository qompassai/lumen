"""Bake first-pass expression deltas into the Lumen base's shape keys.

The base mesh (lumen_base_mesh.py) creates the 16 expression shape keys
with the canonical names from templates/emotions.json but zero vertex
displacement. This script gives each key a real, recipe-driven delta:
every ARKit-52 channel in an emotion's recipe maps to a displacement
field over the head (brow band, eye band, mouth corners, jaw, ...), and
the key's delta is the weight-summed field. Fields are fractions of head
height, anchored to the base's profile rings (jaw z 1.51, cranium
z 1.60, crown z 1.67; the character faces -Y, its left is +X).

These are procedural first-pass sculpts so the templates are visible in
renders and the 3D -> 2D direction can be judged. They are not final art:
stage-4 hand sculpting replaces them per character. Amplitudes live in
CHANNEL_FIELDS and are the tuning surface.

Run headless:
  blender --background --factory-startup --python scripts/lumen_sculpt_deltas.py -- \
      models/base/lumen_base_mature_v2.glb models/base/lumen_base_mature_v3.glb \
      templates/emotions.json
"""

import json
import math
import sys

import bpy
from mathutils import Vector

# Head anchoring (meters, from lumen_base_mesh.py CENTER_NODES; the
# subdivided surface sits slightly inside the ring hull).
CHIN_Z = 1.45
MAX_DISPLACEMENT_FRACTION = 0.25  # Of head height; a recipe blowing past
# this is a bug, not a style choice.


def gauss(x, center, width):
    return math.exp(-(((x - center) / width) ** 2))


def clamp01(x):
    return max(0.0, min(1.0, x))


# Each field takes (u, v, f, s): u = x / head half width (-1..1, + is the
# character's left), v = height fraction (0 chin, 1 crown), f = frontness
# (0 back of head, 1 face front), s = the vertex's own side sign. Returns
# a displacement in fractions of head height, model axes (front is -Y).
def _brow(u, v, f, s, side):
    return gauss(v, 0.64, 0.075) * gauss(u, side * 0.30, 0.78) * f


def _brow_inner(u, v, f, s):
    return gauss(v, 0.64, 0.075) * gauss(u, 0.0, 0.45) * f


def _brow_outer(u, v, f, s, side):
    return gauss(v, 0.64, 0.075) * gauss(u, side * 0.62, 0.38) * f


def _eye(u, v, f, s, side):
    return gauss(v, 0.54, 0.060) * gauss(u, side * 0.50, 0.46) * f


def _cheek(u, v, f, s, side):
    return gauss(v, 0.40, 0.110) * gauss(u, side * 0.58, 0.30) * f


def _mouth(u, v, f, s):
    return gauss(v, 0.30, 0.050) * f


def _corner(u, v, f, s, side):
    return gauss(v, 0.30, 0.050) * gauss(u, side * 0.48, 0.24) * f


def _jaw(u, v, f, s):
    return clamp01((0.24 - v) / 0.24) * (0.35 + 0.65 * f)


CHANNEL_FIELDS = {
    "browDownLeft": lambda u, v, f, s: _brow(u, v, f, s, 1.0) * Vector((-0.006 * s, 0.0, -0.022)),
    "browDownRight": lambda u, v, f, s: _brow(u, v, f, s, -1.0) * Vector((-0.006 * s, 0.0, -0.022)),
    "browInnerUp": lambda u, v, f, s: _brow_inner(u, v, f, s) * Vector((0.0, 0.0, 0.028)),
    "browOuterUpLeft": lambda u, v, f, s: _brow_outer(u, v, f, s, 1.0) * Vector((0.0, 0.0, 0.020)),
    "browOuterUpRight": lambda u, v, f, s: _brow_outer(u, v, f, s, -1.0) * Vector((0.0, 0.0, 0.020)),
    "cheekSquintLeft": lambda u, v, f, s: _cheek(u, v, f, s, 1.0) * Vector((0.0, -0.004, 0.014)),
    "cheekSquintRight": lambda u, v, f, s: _cheek(u, v, f, s, -1.0) * Vector((0.0, -0.004, 0.014)),
    "eyeBlinkLeft": lambda u, v, f, s: _eye(u, v, f, s, 1.0) * Vector((0.0, 0.0, -0.016)),
    "eyeBlinkRight": lambda u, v, f, s: _eye(u, v, f, s, -1.0) * Vector((0.0, 0.0, -0.016)),
    "eyeLookDownLeft": lambda u, v, f, s: _eye(u, v, f, s, 1.0) * Vector((0.0, 0.0, -0.012)),
    "eyeLookDownRight": lambda u, v, f, s: _eye(u, v, f, s, -1.0) * Vector((0.0, 0.0, -0.012)),
    "eyeSquintLeft": lambda u, v, f, s: _eye(u, v, f, s, 1.0) * Vector((0.0, 0.004, -0.008)),
    "eyeSquintRight": lambda u, v, f, s: _eye(u, v, f, s, -1.0) * Vector((0.0, 0.004, -0.008)),
    "eyeWideLeft": lambda u, v, f, s: _eye(u, v, f, s, 1.0) * Vector((0.0, 0.0, 0.010)),
    "eyeWideRight": lambda u, v, f, s: _eye(u, v, f, s, -1.0) * Vector((0.0, 0.0, 0.010)),
    "jawForward": lambda u, v, f, s: _jaw(u, v, f, s) * Vector((0.0, -0.010, 0.0)),
    "jawOpen": lambda u, v, f, s: _jaw(u, v, f, s) * Vector((0.0, 0.0, -0.045)),
    "mouthDimpleLeft": lambda u, v, f, s: _corner(u, v, f, s, 1.0) * Vector((0.008, 0.004, 0.0)),
    "mouthDimpleRight": lambda u, v, f, s: _corner(u, v, f, s, -1.0) * Vector((-0.008, 0.004, 0.0)),
    "mouthFrownLeft": lambda u, v, f, s: _corner(u, v, f, s, 1.0) * Vector((0.004, 0.0, -0.022)),
    "mouthFrownRight": lambda u, v, f, s: _corner(u, v, f, s, -1.0) * Vector((-0.004, 0.0, -0.022)),
    "mouthFunnel": lambda u, v, f, s: _mouth(u, v, f, s) * Vector((-0.006 * s, -0.010, 0.0)),
    "mouthPressLeft": lambda u, v, f, s: _mouth(u, v, f, s) * gauss(u, 0.45, 0.55) * Vector((0.0, 0.006, 0.0)),
    "mouthPressRight": lambda u, v, f, s: _mouth(u, v, f, s) * gauss(u, -0.45, 0.55) * Vector((0.0, 0.006, 0.0)),
    "mouthShrugLower": lambda u, v, f, s: gauss(v, 0.24, 0.045) * gauss(u, 0.0, 0.55) * f * Vector((0.0, -0.008, -0.008)),
    "mouthSmileLeft": lambda u, v, f, s: _corner(u, v, f, s, 1.0) * Vector((0.010, -0.003, 0.026)),
    "mouthSmileRight": lambda u, v, f, s: _corner(u, v, f, s, -1.0) * Vector((-0.010, -0.003, 0.026)),
    "mouthStretchLeft": lambda u, v, f, s: _corner(u, v, f, s, 1.0) * Vector((0.016, 0.0, -0.004)),
    "mouthStretchRight": lambda u, v, f, s: _corner(u, v, f, s, -1.0) * Vector((-0.016, 0.0, -0.004)),
    "noseSneerLeft": lambda u, v, f, s: gauss(v, 0.41, 0.055) * gauss(u, 0.15, 0.30) * f * Vector((0.0, 0.0, 0.014)),
    "noseSneerRight": lambda u, v, f, s: gauss(v, 0.41, 0.055) * gauss(u, -0.15, 0.30) * f * Vector((0.0, 0.0, 0.014)),
}

# Sheet-mood recipes, per the sheet_mood_mapping in emotions.json. The
# geometry half only: blush's cheek color is a texture layer, and wink's
# "over happy or fun" is a runtime composite, not part of the morph.
SHEET_RECIPES = {
    "idle": {},
    "blush": {"cheekSquintLeft": 0.4, "cheekSquintRight": 0.4,
              "eyeSquintLeft": 0.15, "eyeSquintRight": 0.15},
    "wink": {"eyeBlinkRight": 1.0},
    "pout": {"mouthShrugLower": 0.8, "mouthPressLeft": 0.4,
             "mouthPressRight": 0.4, "browDownLeft": 0.2, "browDownRight": 0.2},
    "celebrate": None,  # Filled from the happy recipe + jawOpen below.
    "alarmed": None,     # Filled from the surprised recipe + extras below.
}


def load_recipes(path):
    with open(path, encoding="utf-8") as handle:
        doc = json.load(handle)
    recipes = {e["key"]: e.get("arkit", {}) for e in doc["emotions"]}
    happy = dict(recipes["happy"])
    happy["jawOpen"] = 0.35
    SHEET_RECIPES["celebrate"] = happy
    alarmed = dict(recipes["surprised"])
    alarmed["browInnerUp"] = 0.8
    alarmed["mouthStretchLeft"] = 0.3
    alarmed["mouthStretchRight"] = 0.3
    SHEET_RECIPES["alarmed"] = alarmed
    recipes.update(SHEET_RECIPES)
    return recipes


def sculpt(body, recipes):
    keys = body.data.shape_keys.key_blocks
    basis = [v.co.copy() for v in body.data.vertices]
    head = [co for co in basis if co.z >= CHIN_Z]
    assert head, "no vertices above the chin anchor"
    crown_z = max(co.z for co in head)
    head_h = crown_z - CHIN_Z
    half_w = max(abs(co.x) for co in head)
    front_r = -min(co.y for co in head)
    assert head_h > 0.1 and half_w > 0.03 and front_r > 0.03, (
        f"implausible head metrics: h={head_h:.3f} w={half_w:.3f} r={front_r:.3f}")
    print(f"SCULPT head: chin={CHIN_Z} crown={crown_z:.3f} h={head_h:.3f} "
          f"half_w={half_w:.3f} front_r={front_r:.3f}")
    limit = MAX_DISPLACEMENT_FRACTION * head_h
    for name, recipe in recipes.items():
        if name == "neutral" or not recipe:
            continue
        key = keys.get(name)
        assert key is not None, f"shape key {name!r} missing on the base"
        unmapped = sorted(set(recipe) - set(CHANNEL_FIELDS))
        assert not unmapped, f"{name}: channels with no field: {unmapped}"
        worst = 0.0
        for i, co in enumerate(basis):
            if co.z < CHIN_Z:
                continue
            u = co.x / half_w
            v = (co.z - CHIN_Z) / head_h
            raw = clamp01((-co.y / front_r + 0.25) / 0.9)
            f = raw * raw
            s = 1.0 if co.x >= 0 else -1.0
            delta = Vector((0.0, 0.0, 0.0))
            for channel, weight in recipe.items():
                delta += CHANNEL_FIELDS[channel](u, v, f, s) * (weight * head_h)
            assert delta.length <= limit, (
                f"{name}: displacement {delta.length:.4f} over limit at vertex {i}")
            worst = max(worst, delta.length)
            key.data[i].co = co + delta
        print(f"SCULPT {name}: max displacement {worst * 1000:.1f} mm")


def main():
    argv = sys.argv[sys.argv.index("--") + 1:]
    in_path, out_path, templates_path = argv
    recipes = load_recipes(templates_path)
    bpy.ops.wm.read_factory_settings(use_empty=True)
    bpy.ops.import_scene.gltf(filepath=in_path)
    body = bpy.data.objects.get("LumenBody")
    assert body is not None and body.data.shape_keys, "LumenBody with shape keys not found"
    sculpt(body, recipes)
    bpy.ops.export_scene.gltf(
        filepath=out_path, export_format="GLB", export_yup=True,
        export_skins=True, export_def_bones=True, export_influence_nb=4,
        export_morph=True, export_animations=False)
    print(f"SCULPT wrote {out_path}")


if __name__ == "__main__":
    main()
