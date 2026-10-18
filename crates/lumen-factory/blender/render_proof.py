#!/usr/bin/env python3
"""Blender render proof for lumen-factory GLBs (Phase 5).

Run as:
    blender --background --python blender/render_proof.py -- \
        <model.glb> <outdir> <textured|ramp>

Contract:
- Imports the GLB into a clean scene and frames every view from the
  imported character's bounding box (no hardcoded scene positions):
  full-body views sit at distance 2.2 x height with the camera eye at
  0.62 x height; face views sit at 1.1 x the head-span estimate
  (height x 0.17) from the centre of the head primitive's bounding
  box (the faces using the "head" material slot).
- Lights: a key Sun (energy 3) plus a point fill, over a mid-grey
  (0.35) world so both pale skin and dark hair read against it.
- EEVEE renders: full body at 480x640 (portrait), faces at 512x512.
  Face views pose the mesh's shape keys (neutral / happy / fun); a
  requested expression that does not exist is a hard failure listing
  the available keys.
- In `ramp` mode every material slot of the mesh is replaced with a
  LumenRamp node material built from the `lumen_toon` configuration
  read out of the GLB's own mesh extras (never hardcoded here):
  white Diffuse BSDF (the lit value alone) -> ShaderToRGB -> CONSTANT
  ColorRamp whose band output multiplies the texture color into a
  roughness-1 Principled BSDF. The middle ramp stop is neutral white,
  so the mid band passes the texture through; the shadow and
  highlight stops carry the configured tints.
- Saves the six PNGs to <outdir>, prints RENDER_PROOF_PASS, and exits
  non-zero on any failure.
"""

import json
import math
import os
import struct
import sys

import bpy
from mathutils import Vector


def fail(message):
    print(f"RENDER_PROOF_FAIL: {message}")
    sys.exit(1)


def look_at(obj, target):
    direction = Vector(target) - obj.location
    obj.rotation_euler = direction.to_track_quat("-Z", "Y").to_euler()


def srgb_channel_to_linear(c):
    if c <= 0.04045:
        return c / 12.92
    return ((c + 0.055) / 1.055) ** 2.4


def hex_to_linear_rgba(value):
    digits = value.lstrip("#")
    if len(digits) != 6:
        fail(f"bad tint in lumen_toon config: {value!r}")
    rgb = [int(digits[i : i + 2], 16) / 255.0 for i in (0, 2, 4)]
    return tuple(srgb_channel_to_linear(c) for c in rgb) + (1.0,)


def read_toon_config(model_path):
    """Parse the GLB JSON chunk and return the lumen_toon extras."""
    with open(model_path, "rb") as handle:
        data = handle.read()
    if len(data) < 20 or data[0:4] != b"glTF":
        fail(f"{model_path} is not a GLB")
    (json_len,) = struct.unpack_from("<I", data, 12)
    document = json.loads(data[20 : 20 + json_len])
    for mesh in document.get("meshes", []):
        extras = mesh.get("extras") or {}
        if "lumen_toon" in extras:
            return extras["lumen_toon"]
        for primitive in mesh.get("primitives", []):
            prim_extras = primitive.get("extras") or {}
            if "lumen_toon" in prim_extras:
                return prim_extras["lumen_toon"]
    return None


def original_texture_info(material):
    """(image, base_color) of a material's Principled BSDF, if any."""
    if material is None or not material.use_nodes:
        return None, None
    principled = None
    for node in material.node_tree.nodes:
        if node.type == "BSDF_PRINCIPLED":
            principled = node
            break
    if principled is None:
        return None, None
    base = principled.inputs["Base Color"]
    base_color = list(base.default_value) if base is not None else None
    image = None
    if base is not None and base.links:
        from_node = base.links[0].from_node
        if from_node.type == "TEX_IMAGE":
            image = from_node.image
        elif from_node.type == "RGB":
            base_color = list(from_node.outputs["Color"].default_value)
    return image, base_color


def build_ramp_material(original, config, thresholds, shadow_rgba, highlight_rgba):
    name = "LumenRamp" if original is None else f"LumenRamp_{original.name}"
    existing = bpy.data.materials.get(name)
    if existing is not None:
        return existing
    image, base_color = original_texture_info(original)
    material = bpy.data.materials.new(name)
    material.use_nodes = True
    nodes = material.node_tree.nodes
    links = material.node_tree.links
    nodes.clear()

    output = nodes.new("ShaderNodeOutputMaterial")
    output.location = (600, 0)
    principled = nodes.new("ShaderNodeBsdfPrincipled")
    principled.location = (380, 0)
    principled.inputs["Roughness"].default_value = 1.0
    principled.inputs["Metallic"].default_value = 0.0
    links.new(principled.outputs["BSDF"], output.inputs["Surface"])

    # Texture color source: the original image when there is one,
    # else the original base color, else a 0.8 vertex-zone grey.
    if image is not None:
        tex = nodes.new("ShaderNodeTexImage")
        tex.image = image
        tex.location = (-600, 200)
        tex_color = tex.outputs["Color"]
    else:
        rgb = nodes.new("ShaderNodeRGB")
        rgb.location = (-600, 200)
        fallback = base_color if base_color else (0.8, 0.8, 0.8, 1.0)
        rgb.outputs["Color"].default_value = fallback
        tex_color = rgb.outputs["Color"]

    # The band is chosen by the lit value alone: the diffuse shader
    # sees white, not the texture. (Feeding it the texture banded on
    # albedo x light, so any dark-ish albedo — the magenta hair, the
    # navy outfit — sat in the shadow band under full light.)
    diffuse = nodes.new("ShaderNodeBsdfDiffuse")
    diffuse.location = (-380, -100)
    diffuse.inputs["Roughness"].default_value = 1.0
    diffuse.inputs["Color"].default_value = (1.0, 1.0, 1.0, 1.0)

    to_rgb = nodes.new("ShaderNodeShaderToRGB")
    to_rgb.location = (-160, -100)
    links.new(diffuse.outputs["BSDF"], to_rgb.inputs["Shader"])

    # The Color Ramp shader node: idname ShaderNodeValToRGB in
    # Blender (ValToRamp is the compositor node's name).
    try:
        ramp = nodes.new("ShaderNodeValToRGB")
    except RuntimeError:
        ramp = nodes.new("ShaderNodeValToRamp")
    ramp.location = (40, -100)
    ramp.color_ramp.interpolation = "CONSTANT"
    elements = ramp.color_ramp.elements
    elements[0].position = 0.0
    elements[0].color = shadow_rgba
    middle = elements.new(thresholds[0])
    middle.color = (1.0, 1.0, 1.0, 1.0)
    highlight = elements.new(thresholds[1])
    highlight.color = highlight_rgba
    links.new(to_rgb.outputs["Color"], ramp.inputs["Fac"])

    # Band factor multiplied over the texture color.
    try:
        mix = nodes.new("ShaderNodeMixRGB")
        mix.blend_type = "MULTIPLY"
        mix.inputs[0].default_value = 1.0
        links.new(tex_color, mix.inputs[1])
        links.new(ramp.outputs["Color"], mix.inputs[2])
        mixed = mix.outputs["Color"]
    except RuntimeError:
        mix = nodes.new("ShaderNodeMix")
        mix.data_type = "RGBA"
        mix.blend_type = "MULTIPLY"
        mix.inputs["Factor"].default_value = 1.0
        links.new(tex_color, mix.inputs["A"])
        links.new(ramp.outputs["Color"], mix.inputs["B"])
        mixed = mix.outputs["Result"]
    mix.location = (180, 100)
    links.new(mixed, principled.inputs["Base Color"])
    return material


def main():
    argv = sys.argv
    if "--" not in argv:
        fail("usage: blender --background --python render_proof.py -- <model.glb> <outdir> <textured|ramp>")
    args = argv[argv.index("--") + 1 :]
    if len(args) != 3:
        fail(f"expected 3 arguments (model, outdir, mode), got {len(args)}")
    model_path, outdir, mode = args
    if mode not in ("textured", "ramp"):
        fail(f"mode must be textured|ramp, got {mode!r}")
    if not os.path.isfile(model_path):
        fail(f"model not found: {model_path}")
    os.makedirs(outdir, exist_ok=True)

    config = read_toon_config(model_path)
    if mode == "ramp":
        if config is None:
            fail("ramp mode requested but the GLB carries no lumen_toon extras")
        print(f"RAMP_CONFIG: {json.dumps(config, sort_keys=True)}")
    elif config is not None:
        print(f"TOON_CONFIG_PRESENT (textured mode, unused): {json.dumps(config, sort_keys=True)}")

    # Clean scene.
    bpy.ops.object.select_all(action="SELECT")
    bpy.ops.object.delete(use_global=False)
    bpy.ops.outliner.orphans_purge(do_recursive=True)

    bpy.ops.import_scene.gltf(filepath=model_path)

    meshes = [obj for obj in bpy.data.objects if obj.type == "MESH"]
    armatures = [obj for obj in bpy.data.objects if obj.type == "ARMATURE"]
    if not meshes:
        fail("no mesh objects imported")
    if len(armatures) != 1:
        fail(f"expected exactly 1 armature, found {len(armatures)}")
    armature = armatures[0]
    # The skinned character mesh: the object carrying shape keys.
    character = None
    for obj in meshes:
        if obj.data.shape_keys is not None:
            character = obj
            break
    if character is None:
        character = meshes[0]
    # Blender's glTF importer adds a placeholder "Icosphere" mesh
    # object for this file's scene structure; it is not part of the
    # character (the GLB declares exactly one mesh). It would corrupt
    # the bounding box (it spans -1..1 m at the origin), so every
    # non-character mesh object is removed and the bbox below is
    # computed from the character mesh only.
    for obj in list(meshes):
        if obj is not character:
            print(f"removing non-character importer artifact: {obj.name}")
            bpy.data.objects.remove(obj, do_unlink=True)
    meshes = [character]

    # Character bounding box over all mesh objects (Blender Z-up).
    gmin = Vector((math.inf, math.inf, math.inf))
    gmax = Vector((-math.inf, -math.inf, -math.inf))
    for obj in meshes:
        for corner in obj.bound_box:
            world = obj.matrix_world @ Vector(corner)
            for axis in range(3):
                gmin[axis] = min(gmin[axis], world[axis])
                gmax[axis] = max(gmax[axis], world[axis])
    center = (gmin + gmax) / 2.0
    height = gmax.z - gmin.z
    if height <= 0.0:
        fail(f"degenerate bounding box: min {gmin}, max {gmax}")
    print(f"bbox_min: {tuple(round(v, 4) for v in gmin)}")
    print(f"bbox_max: {tuple(round(v, 4) for v in gmax)}")
    print(f"height: {height:.4f}")

    # Head bone world position (structural check) and face framing.
    if "Head" not in armature.data.bones:
        fail(f"Head bone missing (bones: {[b.name for b in armature.data.bones]})")
    head_bone = armature.data.bones["Head"]
    head_pos = armature.matrix_world @ head_bone.head_local
    head_span = height * 0.17
    # Face views aim at the centre of the head primitive's own bounding
    # box (the faces using the "head" material slot), so framing follows
    # the projected head instead of a fixed offset above the Head bone
    # (v2's head is ~0.23 m tall, v1's was 0.40 m).
    head_slots = [i for i, slot in enumerate(character.material_slots)
                  if slot.material is not None and slot.material.name.split(".")[0] == "head"]
    if not head_slots:
        fail(f"no 'head' material slot (slots: {[s.name for s in character.material_slots]})")
    head_verts = {v for poly in character.data.polygons if poly.material_index in head_slots
                  for v in poly.vertices}
    head_world = [character.matrix_world @ character.data.vertices[i].co for i in head_verts]
    hmin = Vector(tuple(min(p[axis] for p in head_world) for axis in range(3)))
    hmax = Vector(tuple(max(p[axis] for p in head_world) for axis in range(3)))
    face_target = (hmin + hmax) / 2.0
    print(f"head_bone_world: {tuple(round(v, 4) for v in head_pos)}")
    print(f"head_bbox: {tuple(round(v, 4) for v in hmin)} .. {tuple(round(v, 4) for v in hmax)}")
    print(f"face_target: {tuple(round(v, 4) for v in face_target)}")
    print(f"head_span_estimate: {head_span:.4f}")

    # Shape keys / expressions.
    key_blocks = None
    available = []
    if character.data.shape_keys is not None:
        key_blocks = character.data.shape_keys.key_blocks
        available = [block.name for block in key_blocks if block.name != "Basis"]
    print(f"shape_keys: {available}")

    def set_expression(name):
        if key_blocks is None:
            fail("character mesh has no shape keys")
        for block in key_blocks:
            block.value = 0.0
        if name is None:
            return
        if name not in key_blocks:
            fail(f"expression {name!r} missing; available shape keys: {available}")
        key_blocks[name].value = 1.0

    # Ramp materials (Phase 5 node graph), before any rendering.
    if mode == "ramp":
        thresholds = [float(v) for v in config["ramp_thresholds"]]
        shadow_rgba = hex_to_linear_rgba(config["shadow_tint"])
        highlight_rgba = hex_to_linear_rgba(config["highlight_tint"])
        print(f"ramp thresholds: {thresholds}")
        print(f"ramp shadow_tint: {config['shadow_tint']} -> {shadow_rgba}")
        print(f"ramp highlight_tint: {config['highlight_tint']} -> {highlight_rgba}")
        print(f"ramp rim_strength (recorded, not wired in v1 graph): {config['rim_strength']}")
        for obj in meshes:
            for slot in obj.material_slots:
                slot.material = build_ramp_material(
                    slot.material, config, thresholds, shadow_rgba, highlight_rgba
                )

    # Lights + world.
    scene = bpy.context.scene
    if scene.world is None:
        scene.world = bpy.data.worlds.new("ProofWorld")
    scene.world.color = (0.35, 0.35, 0.35)
    if scene.world.use_nodes:
        background = scene.world.node_tree.nodes.get("Background")
        if background is not None:
            background.inputs["Color"].default_value = (0.35, 0.35, 0.35, 1.0)
            background.inputs["Strength"].default_value = 1.0

    sun_data = bpy.data.lights.new("ProofKey", type="SUN")
    sun_data.energy = 3.0
    sun = bpy.data.objects.new("ProofKey", sun_data)
    scene.collection.objects.link(sun)
    sun.rotation_euler = (math.radians(55.0), 0.0, math.radians(25.0))

    fill_data = bpy.data.lights.new("ProofFill", type="POINT")
    fill_data.energy = 120.0
    fill = bpy.data.objects.new("ProofFill", fill_data)
    scene.collection.objects.link(fill)
    fill.location = (center.x - 0.9, center.y - 1.6, gmin.z + 0.75 * height)

    camera_data = bpy.data.cameras.new("ProofCamera")
    camera = bpy.data.objects.new("ProofCamera", camera_data)
    scene.collection.objects.link(camera)
    scene.camera = camera

    scene.render.engine = "BLENDER_EEVEE"
    scene.render.image_settings.file_format = "PNG"

    front = Vector((0.0, -1.0, 0.0))
    three_quarter = Vector((-0.66, -0.75, 0.0)).normalized()
    profile = Vector((1.0, 0.0, 0.0))

    def render(name, direction, target, distance, resolution, lens, expression):
        set_expression(expression)
        scene.render.resolution_x, scene.render.resolution_y = resolution
        scene.render.resolution_percentage = 100
        camera.data.lens = lens
        camera.location = Vector(target) + direction * distance
        camera.location.z = target[2] if resolution[0] == resolution[1] else camera.location.z
        look_at(camera, target)
        path = os.path.join(outdir, name)
        scene.render.filepath = path
        bpy.ops.render.render(write_still=True)
        size = os.path.getsize(path)
        print(f"rendered {name} ({size} bytes)")
        return path

    # Full-body views: distance 2.2 x height, eye at 0.62 x height.
    # A 55 mm lens at that distance fills the portrait frame with
    # the character; face views use 35 mm so the whole head (not a
    # cropped patch) is visible at the spec face distance.
    full_distance = 2.2 * height
    eye_z = gmin.z + 0.62 * height
    full_target = Vector((center.x, center.y, gmin.z + 0.52 * height))
    for name, direction in (
        ("full_front.png", front),
        ("full_three_quarter.png", three_quarter),
        ("full_profile.png", profile),
    ):
        target = Vector(full_target)
        render_pos = target + direction * full_distance
        set_expression(None)
        scene.render.resolution_x, scene.render.resolution_y = (480, 640)
        scene.render.resolution_percentage = 100
        camera.data.lens = 55.0
        camera.location = Vector((render_pos.x, render_pos.y, eye_z))
        look_at(camera, target)
        path = os.path.join(outdir, name)
        scene.render.filepath = path
        bpy.ops.render.render(write_still=True)
        print(f"rendered {name} ({os.path.getsize(path)} bytes)")

    # Face views: distance 1.1 x head-span from the Head bone target.
    face_distance = 1.1 * head_span
    for name, expression in (
        ("face_neutral.png", None),
        ("face_happy.png", "happy"),
        ("face_fun.png", "fun"),
    ):
        render(
            name,
            front,
            face_target,
            face_distance,
            (512, 512),
            35.0,
            expression,
        )

    set_expression(None)
    print("RENDER_PROOF_PASS")


if __name__ == "__main__":
    try:
        main()
    except SystemExit:
        raise
    except Exception as exc:  # Blender exits 0 on unhandled script errors; force a real failure.
        print(f"RENDER_PROOF_FAIL: unhandled error: {exc!r}")
        sys.exit(1)
