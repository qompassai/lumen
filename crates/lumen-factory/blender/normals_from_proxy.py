#!/usr/bin/env python3
"""Desk script for normals-from-proxy/1 (T-12, D-741).

Mirrors the Rust converter (`lumen_factory::convert::normals`) at
the Blender desk: a Data Transfer modifier moves Face Corner Data
→ Custom Normals from a sculpted proxy onto a target mesh, mapped
Nearest Face Interpolated (`POLYINTERP_NEAREST`) and scoped to one
vertex group (the group's weights are the scope).

Modes:
  direct  — refuses a target carrying an Armature modifier:
            custom normals must not be written through a
            deformation the transfer did not see.
  rigsafe — the rig-safe static-duplicate variant: the transfer
            is baked onto a static duplicate of the target
            (armature removed), then a second Data Transfer on the
            real target sources that duplicate, is placed AFTER
            the Armature modifier, and is left LIVE (never
            applied), so the rig keeps deforming and only the
            normals come from the static transfer. The duplicate
            is kept, named `<target>__static_normals`, as the
            export/census copy.

Usage (script auto-execution off):
  blender -b -Y <file.blend> --python normals_from_proxy.py -- \
      --proxy PROXY --target TARGET --group GROUP --mode direct|rigsafe

Prints one `NORMALS` line with the outcome; refuses by raising
SystemExit with a named reason, like the converter's typed errors.
"""
import sys

import bpy


def fail(reason):
    raise SystemExit(f"NORMALS REFUSED: {reason}")


def get_object(name, role):
    obj = bpy.data.objects.get(name)
    if obj is None or obj.type != "MESH":
        fail(f"{role} object '{name}' is not a mesh in this file")
    return obj


def add_transfer(obj, source, group_name):
    modifier = obj.modifiers.new(name="normals_from_proxy", type="DATA_TRANSFER")
    modifier.object = source
    modifier.use_loop_data = True
    modifier.data_types_loops = {"CUSTOM_NORMAL"}
    modifier.loop_mapping = "POLYINTERP_NEAREST"
    if group_name:
        modifier.vertex_group = group_name
    return modifier


def apply_modifier(obj, modifier):
    bpy.ops.object.select_all(action="DESELECT")
    obj.select_set(True)
    bpy.context.view_layer.objects.active = obj
    bpy.ops.object.modifier_apply(modifier=modifier.name)


def has_armature(obj):
    return any(mod.type == "ARMATURE" for mod in obj.modifiers)


def run(proxy_name, target_name, group_name, mode):
    proxy = get_object(proxy_name, "proxy")
    target = get_object(target_name, "target")
    if group_name and group_name not in target.vertex_groups:
        fail(f"vertex group '{group_name}' not on target '{target_name}'")
    if mode == "direct":
        if has_armature(target):
            fail(
                "direct transfer refuses an armature-deformed target; "
                "use --mode rigsafe"
            )
        modifier = add_transfer(target, proxy, group_name)
        apply_modifier(target, modifier)
        print(
            f"NORMALS mode=direct target={target_name} proxy={proxy_name} "
            f"group={group_name} applied=True"
        )
        return
    if mode == "rigsafe":
        # 1. Static duplicate: same mesh, no armature, transfer baked.
        duplicate = target.copy()
        duplicate.data = target.data.copy()
        duplicate.name = f"{target_name}__static_normals"
        bpy.context.collection.objects.link(duplicate)
        for modifier in list(duplicate.modifiers):
            if modifier.type == "ARMATURE":
                duplicate.modifiers.remove(modifier)
        baked = add_transfer(duplicate, proxy, group_name)
        apply_modifier(duplicate, baked)
        # 2. Live transfer on the real target, after the armature.
        live = add_transfer(target, duplicate, group_name)
        print(
            f"NORMALS mode=rigsafe target={target_name} proxy={proxy_name} "
            f"group={group_name} static_duplicate={duplicate.name} "
            f"live_modifier={live.name} applied=False"
        )
        return
    fail(f"unknown mode '{mode}' (expected direct or rigsafe)")


def main():
    argv = sys.argv[sys.argv.index("--") + 1 :]
    args = dict(zip(argv[::2], argv[1::2]))
    try:
        proxy = args["--proxy"]
        target = args["--target"]
        mode = args["--mode"]
    except KeyError as missing:
        raise SystemExit(f"missing required argument {missing}")
    run(proxy, target, args.get("--group", ""), mode)
    print("NORMALS DONE")


main()
