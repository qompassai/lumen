"""Render a Lumen GLB back to 2D: the reverse half of the factory.

Produces, from one character GLB:
  - idle_frame_00.png .. idle_frame_11.png: a 12-frame breathing idle at
    sprite size (288x576 RGBA, transparent film), framed to the source
    sprite's measured occupancy (subject = 94.1% of frame height, 3.3%
    top margin, from seraphine_animated_mature.png frame 0).
  - expr_<name>.png: front stills at 576x1152 for neutral + the 10
    canonical emotions + wink + celebrate, each posed at weight 1.0.

Lighting, view transform, and the arms-down pose mirror lumen_gate.py so
renders are comparable across the pipeline. Assembly into APNG / strip /
GIF / contact sheets is a separate PIL step (lumen_pack_sheet.py).

Run headless:
  blender --background --factory-startup --python scripts/lumen_render_sheet.py -- \
      models/seraphine_mature.glb models/renders2d
"""

import math
import os
import sys

import bpy
from mathutils import Matrix, Vector

FRAME_W, FRAME_H = 288, 576
IDLE_FRAMES = 12
SUBJECT_FRACTION = 0.941   # Subject height / frame height in the source.
TOP_MARGIN_FRACTION = 0.033
ARM_DOWN_DEG = 72.0
PREFIX = "mixamorig:"
EXPRESSIONS = (
    "neutral", "happy", "angry", "sad", "surprised", "fun", "annoyed",
    "worried", "embarrassed", "determined", "smug", "wink", "celebrate",
)


def pose_arms_down(rig):
    bpy.context.view_layer.objects.active = rig
    bpy.ops.object.mode_set(mode="POSE")
    for side, sign in (("Left", 1.0), ("Right", -1.0)):
        bone = rig.pose.bones[PREFIX + side + "Arm"]
        pivot = bone.head.copy()
        turn = Matrix.Rotation(math.radians(ARM_DOWN_DEG) * sign, 4, "Y")
        bone.matrix = (Matrix.Translation(pivot) @ turn
                       @ Matrix.Translation(-pivot) @ bone.matrix)
        bpy.context.view_layer.update()
    bpy.ops.object.mode_set(mode="OBJECT")


def scene_bounds():
    depsgraph = bpy.context.evaluated_depsgraph_get()
    points = []
    for obj in bpy.data.objects:
        if obj.type != "MESH" or not obj.visible_get() or obj.hide_render:
            continue
        evaluated = obj.evaluated_get(depsgraph)
        mesh = evaluated.to_mesh()
        points += [evaluated.matrix_world @ v.co for v in mesh.vertices]
        evaluated.to_mesh_clear()
    low = Vector((min(p.x for p in points), min(p.y for p in points),
                  min(p.z for p in points)))
    high = Vector((max(p.x for p in points), max(p.y for p in points),
                   max(p.z for p in points)))
    return low, high


def setup_scene():
    scene = bpy.context.scene
    scene.render.engine = "BLENDER_EEVEE"
    scene.render.resolution_percentage = 100
    scene.render.image_settings.file_format = "PNG"
    scene.view_settings.view_transform = "Standard"
    scene.render.film_transparent = True
    world = bpy.data.worlds.new("LumenWorld")
    world.node_tree.nodes["Background"].inputs["Color"].default_value = (0.3, 0.3, 0.3, 1.0)
    scene.world = world
    for name, rotation, energy in (("Key", (50, 0, -30), 3.0), ("Fill", (60, 0, 150), 1.2)):
        light = bpy.data.lights.new(name, "SUN")
        light.energy = energy
        obj = bpy.data.objects.new(name, light)
        obj.rotation_euler = [math.radians(a) for a in rotation]
        scene.collection.objects.link(obj)
    camera = bpy.data.objects.new("Camera", bpy.data.cameras.new("Camera"))
    camera.data.type = "ORTHO"
    scene.collection.objects.link(camera)
    scene.camera = camera
    return scene, camera


def frame_camera(camera, low, high):
    """Front view (character faces -Y), framed to the source occupancy."""
    height = high.z - low.z
    span = height / SUBJECT_FRACTION
    camera.data.ortho_scale = span
    center_x = (low.x + high.x) / 2.0
    center_z = high.z - (0.5 - TOP_MARGIN_FRACTION) * span
    camera.location = (center_x, low.y - 6.0, center_z)
    direction = Vector((center_x, 0.0, center_z)) - camera.location
    camera.rotation_euler = direction.to_track_quat("-Z", "Y").to_euler()
    return Vector((center_x, 0.0, center_z))


def find_bone(rig, suffix):
    for bone in rig.pose.bones:
        if bone.name.endswith(suffix):
            return bone
    return None


def set_breath(rig, phase):
    """Subtle breathing: chest swell, head counter-tilt, root bob."""
    chest = find_bone(rig, "Spine2")
    head = find_bone(rig, "Head")
    sway = math.sin(phase)
    if chest is not None:
        chest.rotation_mode = "XYZ"
        chest.rotation_euler = (0.018 * sway, 0.0, 0.0)
    if head is not None:
        head.rotation_mode = "XYZ"
        head.rotation_euler = (-0.008 * sway, 0.0, 0.006 * math.sin(phase * 0.5))
    rig.location.z = 0.002 * sway
    bpy.context.view_layer.update()


def set_expression(body, name):
    keys = body.data.shape_keys.key_blocks
    for key in keys:
        key.value = 0.0
    if name != "neutral":
        keys[name].value = 1.0
    bpy.context.view_layer.update()


def render_to(scene, path):
    scene.render.filepath = path
    bpy.ops.render.render(write_still=True)
    print(f"RENDER {path}")


def main():
    argv = sys.argv[sys.argv.index("--") + 1:]
    glb_path, out_dir = argv
    os.makedirs(out_dir, exist_ok=True)
    bpy.ops.wm.read_factory_settings(use_empty=True)
    bpy.ops.import_scene.gltf(filepath=glb_path)
    rigs = [o for o in bpy.data.objects if o.type == "ARMATURE"]
    assert len(rigs) == 1, f"expected 1 armature, got {len(rigs)}"
    rig = rigs[0]
    body = bpy.data.objects.get("LumenBody")
    assert body is not None and body.data.shape_keys, "LumenBody with shape keys not found"
    pose_arms_down(rig)
    scene, camera = setup_scene()
    low, high = scene_bounds()
    frame_camera(camera, low, high)

    scene.render.resolution_x, scene.render.resolution_y = FRAME_W, FRAME_H
    for i in range(IDLE_FRAMES):
        set_breath(rig, 2.0 * math.pi * i / IDLE_FRAMES)
        render_to(scene, os.path.join(out_dir, f"idle_frame_{i:02d}.png"))
    set_breath(rig, 0.0)

    scene.render.resolution_x, scene.render.resolution_y = FRAME_W * 2, FRAME_H * 2
    for name in EXPRESSIONS:
        set_expression(body, name)
        render_to(scene, os.path.join(out_dir, f"expr_{name}.png"))
    set_expression(body, "neutral")
    print("RENDER done")


if __name__ == "__main__":
    main()
