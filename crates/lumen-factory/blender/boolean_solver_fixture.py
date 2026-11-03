#!/usr/bin/env python3
"""Golden fixture generator for the T-2 boolean-solver contract (D-730).

Builds one deterministic multi-part union whose operand is a single
self-intersecting mesh (two joined, overlapping cubes — the shape of
part stock that generative kitbash tools emit), fuses it with a slab
under each Blender boolean solver, censuses the result by edge
manifoldness, and exports one GLB per solver.

Finding on Blender 5.2.3 (primo, 2026-11-03): the EXACT solver fails
this fusion outright — the evaluated result is an EMPTY mesh — while
the MANIFOLD solver returns the correct watertight union. On the
video's older Blender the same failure class surfaced as holes; the
census catches both, because an empty union of non-empty solids is
non-watertight by definition (see `lumen_factory::convert::contract`).

Usage (on primo, stock Blender, script auto-execution off):
  blender -b -Y --python boolean_solver_fixture.py -- <output-dir>

Prints one `FIXTURE` line per solver with the Python-side census;
the Rust gate (`tests/contract_validation.rs`) re-censuses the
exported GLBs independently and asserts the same verdicts.
"""
import sys
from collections import Counter

import bmesh
import bpy


def clear_scene():
    bpy.ops.object.select_all(action="SELECT")
    bpy.ops.object.delete(use_global=False)
    for mesh in list(bpy.data.meshes):
        bpy.data.meshes.remove(mesh)


def add_cube(name, location, scale):
    bpy.ops.mesh.primitive_cube_add(location=location)
    obj = bpy.context.object
    obj.name = name
    obj.scale = scale
    bpy.ops.object.transform_apply(location=False, rotation=False, scale=True)
    return obj


def build_parts():
    """The fixture: a slab plus a self-intersecting joined operand."""
    slab = add_cube("slab", (0.35, 0.15, 1.15), (1.3, 1.0, 0.45))
    left = add_cube("operand_left", (0.0, 0.0, 0.5), (0.6, 0.6, 0.6))
    right = add_cube("operand_right", (0.7, 0.35, 0.5), (0.6, 0.6, 0.6))
    bpy.ops.object.select_all(action="DESELECT")
    left.select_set(True)
    right.select_set(True)
    bpy.context.view_layer.objects.active = left
    bpy.ops.object.join()
    operand = bpy.context.object
    operand.name = "operand_self_intersecting"
    return slab, operand


def evaluated_mesh(obj):
    depsgraph = bpy.context.evaluated_depsgraph_get()
    return bpy.data.meshes.new_from_object(obj.evaluated_get(depsgraph))


def census(mesh):
    counts = Counter()
    degenerate = 0
    for polygon in mesh.polygons:
        verts = list(polygon.vertices)
        if len(verts) < 3:
            degenerate += 1
            continue
        for i in range(len(verts)):
            a, b = verts[i], verts[(i + 1) % len(verts)]
            counts[(min(a, b), max(a, b))] += 1
    boundary = sum(1 for c in counts.values() if c == 1)
    over = sum(1 for c in counts.values() if c > 2)
    watertight = (
        len(mesh.vertices) > 0
        and len(mesh.polygons) > 0
        and boundary == 0
        and over == 0
        and degenerate == 0
    )
    return {
        "boundary_edges": boundary,
        "degenerate_faces": degenerate,
        "edges": len(counts),
        "nonmanifold_edges": over,
        "polygons": len(mesh.polygons),
        "vertices": len(mesh.vertices),
        "watertight": watertight,
    }


def run_solver(solver, out_dir):
    clear_scene()
    slab, operand = build_parts()
    modifier = slab.modifiers.new(name="union", type="BOOLEAN")
    modifier.operation = "UNION"
    modifier.solver = solver
    modifier.object = operand
    result_mesh = evaluated_mesh(slab)
    # Triangulate in Blender, deterministically: the exporter must
    # not be the thing that decides the topology (its ngon
    # triangulation can double-cover faces, which censuses as
    # phantom nonmanifold edges). The census below and the Rust
    # census then measure the same triangles.
    bm = bmesh.new()
    bm.from_mesh(result_mesh)
    bmesh.ops.triangulate(bm, faces=list(bm.faces))
    bm.to_mesh(result_mesh)
    bm.free()
    report = census(result_mesh)
    slab.data = result_mesh
    slab.modifiers.clear()
    bpy.data.objects.remove(operand, do_unlink=True)
    bpy.ops.object.select_all(action="DESELECT")
    slab.select_set(True)
    bpy.context.view_layer.objects.active = slab
    path = f"{out_dir}/boolean-{solver.lower()}.glb"
    bpy.ops.export_scene.gltf(
        filepath=path, export_format="GLB", use_selection=True
    )
    print(f"FIXTURE solver={solver} census={report} glb={path}")


def main():
    argv = sys.argv[sys.argv.index("--") + 1 :]
    if len(argv) != 1:
        raise SystemExit("usage: boolean_solver_fixture.py -- <output-dir>")
    for solver in ("EXACT", "MANIFOLD"):
        run_solver(solver, argv[0])
    print("FIXTURE DONE")


main()
