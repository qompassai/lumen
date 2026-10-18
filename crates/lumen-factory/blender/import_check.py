#!/usr/bin/env python3
"""Blender import gate for lumen-factory GLBs.

Run as:
    blender --background --python blender/import_check.py -- <model.glb> <outdir>

Contract:
- Imports the GLB into a clean scene.
- Asserts an armature with exactly 65 bones (spot-checking Mixamo names).
- Asserts the head mesh carries the expected shape keys: when
  <outdir>/expected_targets.json exists, the shape-key names must equal
  that list exactly; otherwise at least 20 keys including
  'happy', 'wink' and 'blink' must be present.
- Renders a 320x320 EEVEE frame at neutral and with 'happy' at 1.0,
  computes the mean absolute pixel difference, and asserts it exceeds
  MEAN_DIFF_THRESHOLD.
- Saves both PNGs to <outdir>, prints IMPORT_CHECK_PASS + stats, and
  exits non-zero on any failure.
"""

import json
import math
import os
import sys

import bpy
from mathutils import Vector

# Threshold rationale: EEVEE renders of this scene are deterministic
# (identical inputs measured an exact 0.0 mean diff during gate
# development), so the noise floor is zero and 1e-4 is a conservative
# small threshold. The 'happy' pose at the framing below measures
# ~4.6e-4, roughly 4.6x the threshold.
MEAN_DIFF_THRESHOLD = 0.0001
EXPECTED_BONES = 65
SPOT_BONES = ("Hips", "Head", "LeftHand", "RightToeBase")


def fail(message):
    print(f"IMPORT_CHECK_FAIL: {message}")
    sys.exit(1)


def look_at(obj, target):
    direction = Vector(target) - obj.location
    obj.rotation_euler = direction.to_track_quat("-Z", "Y").to_euler()


def main():
    argv = sys.argv
    if "--" not in argv:
        fail("usage: blender --background --python import_check.py -- <model.glb> <outdir>")
    args = argv[argv.index("--") + 1 :]
    if len(args) != 2:
        fail(f"expected 2 arguments (model, outdir), got {len(args)}")
    model_path, outdir = args
    if not os.path.isfile(model_path):
        fail(f"model not found: {model_path}")
    os.makedirs(outdir, exist_ok=True)

    # Clean scene: delete all default objects and data.
    bpy.ops.object.select_all(action="SELECT")
    bpy.ops.object.delete(use_global=False)
    bpy.ops.outliner.orphans_purge(do_recursive=True)

    # Import the GLB.
    bpy.ops.import_scene.gltf(filepath=model_path)

    # Armature: exactly 65 bones, spot-check names.
    armatures = [obj for obj in bpy.data.objects if obj.type == "ARMATURE"]
    if len(armatures) != 1:
        fail(f"expected exactly 1 armature, found {len(armatures)}")
    armature = armatures[0]
    bone_names = [bone.name for bone in armature.data.bones]
    if len(bone_names) != EXPECTED_BONES:
        fail(f"bone count {len(bone_names)} != {EXPECTED_BONES}: {bone_names}")
    for name in SPOT_BONES:
        if name not in bone_names:
            fail(f"spot bone missing: {name} (bones: {bone_names})")

    # Head mesh: the mesh object carrying shape keys.
    head = None
    shape_names = []
    for obj in bpy.data.objects:
        if obj.type != "MESH" or obj.data.shape_keys is None:
            continue
        names = [block.name for block in obj.data.shape_keys.key_blocks if block.name != "Basis"]
        if names:
            head = obj
            shape_names = names
            break
    if head is None:
        fail("no mesh object with shape keys found")
    expected_path = os.path.join(outdir, "expected_targets.json")
    if os.path.isfile(expected_path):
        with open(expected_path, "r", encoding="utf-8") as handle:
            expected = json.load(handle)
        if sorted(shape_names) != sorted(expected):
            fail(f"shape keys {sorted(shape_names)} != expected {sorted(expected)}")
    else:
        if len(shape_names) < 20:
            fail(f"shape key count {len(shape_names)} < 20: {shape_names}")
        for name in ("happy", "wink", "blink"):
            if name not in shape_names:
                fail(f"required shape key missing: {name} (have {shape_names})")

    # Camera + light, framed tight on the head. Blender's glTF importer
    # converts glTF Y-up to Blender Z-up (gltf (x, y, z) -> Blender
    # (x, -z, y)), so the head centre (gltf y ~1.665, facing +Z) sits
    # at Blender (0, ~0, 1.665) facing -Y. The camera goes in front of
    # the face, at negative Y.
    scene = bpy.context.scene
    camera_data = bpy.data.cameras.new("CheckCamera")
    camera = bpy.data.objects.new("CheckCamera", camera_data)
    scene.collection.objects.link(camera)
    camera.location = (0.0, -0.32, 1.665)
    camera_data.lens = 50.0
    look_at(camera, (0.0, 0.0, 1.65))
    scene.camera = camera

    sun_data = bpy.data.lights.new("CheckSun", type="SUN")
    sun_data.energy = 3.0
    sun = bpy.data.objects.new("CheckSun", sun_data)
    scene.collection.objects.link(sun)
    sun.rotation_euler = (math.radians(55.0), 0.0, math.radians(25.0))

    point_data = bpy.data.lights.new("CheckFill", type="POINT")
    point_data.energy = 30.0
    point = bpy.data.objects.new("CheckFill", point_data)
    scene.collection.objects.link(point)
    point.location = (0.3, -0.5, 1.7)

    # Render setup: EEVEE, 320x320 PNG.
    scene.render.engine = "BLENDER_EEVEE"
    scene.render.resolution_x = 320
    scene.render.resolution_y = 320
    scene.render.resolution_percentage = 100
    scene.render.image_settings.file_format = "PNG"
    if scene.world is None:
        scene.world = bpy.data.worlds.new("CheckWorld")
    scene.world.color = (0.08, 0.08, 0.10)

    key_blocks = head.data.shape_keys.key_blocks

    def set_all_shape_values(value):
        for block in key_blocks:
            if block.name != "Basis":
                block.value = value

    neutral_path = os.path.join(outdir, "neutral.png")
    happy_path = os.path.join(outdir, "happy.png")

    set_all_shape_values(0.0)
    scene.render.filepath = neutral_path
    bpy.ops.render.render(write_still=True)

    if "happy" not in key_blocks:
        fail("shape key 'happy' missing at render stage")
    key_blocks["happy"].value = 1.0
    scene.render.filepath = happy_path
    bpy.ops.render.render(write_still=True)
    set_all_shape_values(0.0)

    # Mean absolute pixel difference over RGBA float pixels.
    neutral_img = bpy.data.images.load(neutral_path)
    happy_img = bpy.data.images.load(happy_path)
    if neutral_img.size[:] != happy_img.size[:]:
        fail(f"render sizes differ: {neutral_img.size[:]} vs {happy_img.size[:]}")
    neutral_px = list(neutral_img.pixels)
    happy_px = list(happy_img.pixels)
    if len(neutral_px) != len(happy_px) or not neutral_px:
        fail("render pixel data missing or mismatched")
    total = 0.0
    for a, b in zip(neutral_px, happy_px):
        total += abs(a - b)
    mean_diff = total / len(neutral_px)
    print(f"bones: {len(bone_names)}")
    print(f"shape_keys: {len(shape_names)}")
    print(f"shape_key_names: {','.join(sorted(shape_names))}")
    print(f"head_object: {head.name}")
    print(f"render_mean_abs_diff: {mean_diff:.8f}")
    print(f"render_threshold: {MEAN_DIFF_THRESHOLD:.8f}")
    if not (mean_diff > MEAN_DIFF_THRESHOLD):
        fail(f"mean abs pixel diff {mean_diff:.8f} <= threshold {MEAN_DIFF_THRESHOLD}")
    print("IMPORT_CHECK_PASS")


if __name__ == "__main__":
    try:
        main()
    except SystemExit:
        raise
    except Exception as exc:  # Blender exits 0 on unhandled script errors; force a real failure.
        print(f"IMPORT_CHECK_FAIL: unhandled error: {exc!r}")
        sys.exit(1)
