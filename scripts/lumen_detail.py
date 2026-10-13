"""Give a projected Lumen character her sprite's actual features.

Three transfers from the 2D sprite to the 3D model, inside the existing
gate contract (meshes stay {Hair, LumenBody}; new materials are additive):

1. Face: her face cropped from her own sprite (lumen_detail_prep.py)
   becomes a texture on the head's front faces, anchored by the identity
   lock's measurements (hairline and chin), so the morph deltas sculpted
   from the ARKit recipes move the geometry under her real features.
2. Chest print: her hoodie + vest front as a texture on the torso's
   front faces.
3. Hair rebuilt from her silhouette: scalp cap, swept bangs, side locks,
   and a lofted ponytail that rises and cascades down her back, with the
   pale-cyan ribbon and silver tie as accent materials. Skinned to
   Head/Neck/Spine2 so it animates with the rig.

Run headless:
  blender --background --factory-startup --python scripts/lumen_detail.py -- \
      models/seraphine_mature.glb models/seraphine_mature_detailed.glb \
      models/seraphine_face.png models/seraphine_chest.png
"""

import math
import sys

import bpy
from mathutils import Vector

HAIR_RGB = (0.63, 0.105, 0.24)      # Her lit magenta, linear.
ACCENT_RGB = (0.42, 0.79, 0.83)     # Ribbon pale cyan, linear.
TIE_RGB = (0.68, 0.70, 0.72)        # Hair tie silver, linear.


def srgb_material(name, rgb, roughness=0.55):
    existing = bpy.data.materials.get(name)
    if existing is not None:
        bpy.data.materials.remove(existing)
    mat = bpy.data.materials.new(name)
    mat.use_nodes = True
    bsdf = mat.node_tree.nodes["Principled BSDF"]
    bsdf.inputs["Base Color"].default_value = (*rgb, 1.0)
    bsdf.inputs["Roughness"].default_value = roughness
    return mat


def image_material(name, image_path, uv_layer):
    existing = bpy.data.materials.get(name)
    if existing is not None:
        bpy.data.materials.remove(existing)
    mat = bpy.data.materials.new(name)
    mat.use_nodes = True
    nodes = mat.node_tree.nodes
    bsdf = nodes["Principled BSDF"]
    uv = nodes.new("ShaderNodeUVMap")
    uv.uv_map = uv_layer
    tex = nodes.new("ShaderNodeTexImage")
    tex.image = bpy.data.images.load(image_path)
    mat.node_tree.links.new(uv.outputs["UV"], tex.inputs["Vector"])
    mat.node_tree.links.new(tex.outputs["Color"], bsdf.inputs["Base Color"])
    bsdf.inputs["Roughness"].default_value = 0.6
    return mat


def head_metrics(body, neck_z):
    # The head is the vertex set skinned to the Head bone; a z-band alone
    # catches the T-posed arms on the projected model.
    head_group = next(g for g in body.vertex_groups if g.name.endswith("Head"))
    verts = []
    for v in body.data.vertices:
        w = next((g.weight for g in v.groups if g.group == head_group.index), 0.0)
        if w > 0.5:
            verts.append(v.co)
    assert verts, "no head vertices found"
    chin = min(c.z for c in verts)
    crown = max(c.z for c in verts)
    half_w = max(abs(c.x) for c in verts)
    front_r = -min(c.y for c in verts)
    print(f"DETAIL head: chin={chin:.3f} crown={crown:.3f} "
          f"h={crown - chin:.3f} half_w={half_w:.3f} front_r={front_r:.3f}")
    return chin, crown, crown - chin, half_w, front_r


def apply_face(body, metrics, face_mat):
    chin, crown, head_h, half_w, front_r = metrics
    uv_layer = body.data.uv_layers.new(name="FaceUV")
    hairline_z = chin + 0.84 * head_h
    chin_anchor_z = chin + 0.02 * head_h
    # Texture anchors from the sprite crop (82..160 px): hairline at
    # 97 px -> v 0.808, chin at 154 px -> v 0.077 (UV v is bottom-up).
    span_v = 0.808 - 0.077
    span_z = hairline_z - chin_anchor_z
    face_half = half_w * 0.72
    for loop in body.data.loops:
        co = body.data.vertices[loop.vertex_index].co
        u = 0.5 + co.x / (2.0 * face_half)
        v = 0.077 + (co.z - chin_anchor_z) / span_z * span_v
        uv_layer.data[loop.index].uv = (u, v)
    body.data.materials.append(face_mat)
    face_index = len(body.data.materials) - 1
    assigned = 0
    for poly in body.data.polygons:
        c = poly.center
        if (chin - 0.005 <= c.z <= chin + 0.97 * head_h
                and c.y < -0.25 * front_r and abs(c.x) < half_w * 1.02):
            poly.material_index = face_index
            assigned += 1
    print(f"DETAIL face: {assigned} faces assigned")
    assert assigned > 40, "face assignment implausibly small"


def apply_chest_print(body, rig, chest_mat):
    bones = {b.name: b for b in rig.data.bones}
    neck = next(b for b in bones if b.endswith("Neck"))
    hips = next(b for b in bones if b.endswith("Hips"))
    z0 = bones[hips].head_local.z + 0.10
    z1 = bones[neck].head_local.z + 0.01
    uv_layer = body.data.uv_layers.new(name="PrintUV")
    print_half = 0.21
    for loop in body.data.loops:
        co = body.data.vertices[loop.vertex_index].co
        u = 0.5 + co.x / (2.0 * print_half)
        v = (co.z - z0) / (z1 - z0)
        uv_layer.data[loop.index].uv = (u, v)
    body.data.materials.append(chest_mat)
    index = len(body.data.materials) - 1
    assigned = 0
    for poly in body.data.polygons:
        c = poly.center
        if z0 <= c.z <= z1 and c.y < -0.055 and abs(c.x) <= 0.20:
            poly.material_index = index
            assigned += 1
    print(f"DETAIL chest print: {assigned} faces, band z {z0:.2f}..{z1:.2f}")
    assert assigned > 40, "chest print assignment implausibly small"


def ring_frame(tangent):
    t = tangent.normalized()
    ref = Vector((0, 0, 1)) if abs(t.z) < 0.9 else Vector((1, 0, 0))
    n = t.cross(ref).normalized()
    b = t.cross(n)
    return n, b


def loft(points, radii, sides=12):
    """Tube along points with per-point radii. Returns (verts, faces)."""
    verts, faces = [], []
    count = len(points)
    for i, (center, radius) in enumerate(zip(points, radii)):
        prev = points[max(i - 1, 0)]
        nxt = points[min(i + 1, count - 1)]
        n, b = ring_frame(nxt - prev)
        for j in range(sides):
            a = 2.0 * math.pi * j / sides
            verts.append(center + radius * (math.cos(a) * n + math.sin(a) * b))
    for i in range(count - 1):
        for j in range(sides):
            a = i * sides + j
            b2 = i * sides + (j + 1) % sides
            c = (i + 1) * sides + j
            d = (i + 1) * sides + (j + 1) % sides
            faces.append((a, c, d, b2))
    # Cap the tip with a fan to the last point extended.
    tip = points[-1] + (points[-1] - points[-2]).normalized() * radii[-1]
    tip_index = len(verts)
    verts.append(tip)
    base = (count - 1) * sides
    for j in range(sides):
        faces.append((base + j, tip_index, base + (j + 1) % sides))
    return verts, faces


def catmull_rom(points, subdiv=3):
    out = [points[0]]
    for i in range(len(points) - 1):
        p0 = points[max(i - 1, 0)]
        p1, p2 = points[i], points[i + 1]
        p3 = points[min(i + 2, len(points) - 1)]
        for s in range(1, subdiv + 1):
            t = s / subdiv
            t2, t3 = t * t, t * t * t
            out.append(0.5 * ((2 * p1) + (-p0 + p2) * t
                              + (2 * p0 - 5 * p1 + 4 * p2 - p3) * t2
                              + (-p0 + 3 * p1 - 3 * p2 + p3) * t3))
    return out


def build_hair(metrics):
    chin, crown, h, half_w, front_r = metrics
    parts = []  # (verts, faces, material_slot, skin_weights)

    # Scalp cap: tilted parametric dome over the cranium.
    center = Vector((0.0, 0.012, chin + 0.66 * h))
    radii = Vector((half_w * 1.12, front_r * 1.14, h * 0.62))
    verts, faces = [], []
    rows, cols = 7, 18
    for i in range(rows + 1):
        polar = 0.40 * math.pi * i / rows
        for j in range(cols + 1):
            az = 2.0 * math.pi * j / cols
            p = Vector((radii.x * math.sin(polar) * math.cos(az),
                        radii.y * math.sin(polar) * math.sin(az),
                        radii.z * math.cos(polar)))
            # Slight tilt only: the rim must stay at the hairline
            # (0.85h) all around, never across the face.
            p.z -= 0.03 * h * max(0.0, -p.y) / front_r
            verts.append(center + p)
    for i in range(rows):
        for j in range(cols):
            a = i * (cols + 1) + j
            faces.append((a, a + cols + 1, a + cols + 2, a + 1))
    parts.append((verts, faces, 0, [[("Head", 1.0)]] * len(verts)))

    # Bangs: a shell hugging the forehead ellipsoid, from above the
    # hairline down to a gently slanted fringe edge.
    hairline_z = chin + 0.84 * h
    cap_center_z = chin + 0.66 * h
    cap_r = Vector((half_w * 1.12, front_r * 1.14, h * 0.62))
    verts, faces = [], []
    bcols = 24
    for i, t in enumerate((0.0, 0.5, 1.0)):
        for j in range(bcols + 1):
            frac = j / bcols
            az = math.radians(-68 + 136 * frac)
            slant = 0.030 * h * (frac - 0.5) * 2.0  # Lower toward +X.
            z = hairline_z + 0.16 * h - t * (0.17 * h + slant)
            k = max(0.05, 1.0 - ((z - cap_center_z) / cap_r.z) ** 2)
            rx = cap_r.x * math.sqrt(k) * 1.02
            ry = cap_r.y * math.sqrt(k) * 1.02
            verts.append(Vector((rx * math.sin(az),
                                 0.012 - ry * math.cos(az), z)))
    for i in range(2):
        for j in range(bcols):
            a = i * (bcols + 1) + j
            faces.append((a, a + bcols + 1, a + bcols + 2, a + 1))
    parts.append((verts, faces, 0, [[("Head", 1.0)]] * len(verts)))

    # Side locks: thin strands at the face's side-front edge, hugging
    # the same ellipsoid, from the hairline down to the chin.
    for sign in (1.0, -1.0):
        az = math.radians(70) * sign
        path, radii = [], []
        for t in (0.0, 0.5, 1.0):
            z = hairline_z + 0.02 * h - t * (hairline_z - chin + 0.02 * h)
            k = max(0.05, 1.0 - ((z - cap_center_z) / cap_r.z) ** 2)
            rx = cap_r.x * math.sqrt(k) * 1.03
            ry = cap_r.y * math.sqrt(k) * 1.03
            path.append(Vector((rx * math.sin(az), 0.012 - ry * math.cos(az), z)))
            radii.append((0.045 - 0.022 * t) * h)
        v2, f2 = loft(catmull_rom(path, 2),
                      [radii[0], radii[0], radii[1], radii[1], radii[2]], sides=8)
        parts.append((v2, f2, 0, [[("Head", 1.0)]] * len(v2)))

    # Ponytail: rises off the crown, arcs to her right (-X), cascades to
    # mid-back. y is behind the head (+Y).
    path = [
        Vector((0.00, 0.38 * front_r, chin + 1.00 * h)),
        Vector((-0.15 * h, 0.44 * front_r, chin + 1.24 * h)),
        Vector((-0.55 * h, 0.48 * front_r, chin + 1.20 * h)),
        Vector((-0.88 * h, 0.50 * front_r, chin + 0.90 * h)),
        Vector((-1.00 * h, 0.52 * front_r, chin + 0.42 * h)),
        Vector((-0.98 * h, 0.54 * front_r, chin - 0.10 * h)),
        Vector((-0.88 * h, 0.56 * front_r, chin - 0.62 * h)),
        Vector((-0.80 * h, 0.57 * front_r, chin - 1.05 * h)),
    ]
    radii = [0.26, 0.34, 0.40, 0.37, 0.31, 0.22, 0.13, 0.05]
    smooth = catmull_rom(path, 3)
    smooth_r = []
    for i in range(len(smooth)):
        t = i / (len(smooth) - 1)
        seg = min(int(t * (len(radii) - 1)), len(radii) - 2)
        lt = t * (len(radii) - 1) - seg
        smooth_r.append((radii[seg] * (1 - lt) + radii[seg + 1] * lt) * h)
    v3, f3 = loft(smooth, smooth_r, sides=12)
    weights = []
    rings = len(smooth) + 1  # loft adds a tip vertex.
    per_ring = 12
    for i in range(len(smooth)):
        t = i / (len(smooth) - 1)
        if t <= 0.35:
            w = [("Head", 1.0)]
        elif t >= 0.75:
            w = [("Head", 0.25), ("Neck", 0.35), ("Spine2", 0.40)]
        else:
            k = (t - 0.35) / 0.40
            w = [("Head", 1.0 - 0.7 * k), ("Neck", 0.35 * k), ("Spine2", 0.30 * k)]
        weights.extend([w] * per_ring)
    weights.append([("Head", 0.25), ("Neck", 0.35), ("Spine2", 0.40)])
    parts.append((v3, f3, 0, weights))

    # Ribbon: a thin strand hugging the ponytail's front.
    ribbon_path = [p + Vector((0.02 * h, -r * 0.85, 0.0))
                   for p, r in zip(path, radii)]
    rv, rf = loft(catmull_rom(ribbon_path, 3),
                  [max(0.05 * h * (1 - i / 24.0), 0.012) for i in range(25)],
                  sides=6)
    parts.append((rv, rf, 1, [[("Head", 1.0)]] * len(rv)))

    # Hair tie: a short fat ring at the ponytail base.
    tie_path = [path[0] + Vector((0, 0, -0.01)), path[0] + Vector((-0.02 * h, 0.005, 0.05 * h))]
    tv, tf = loft(tie_path, [0.335 * h, 0.335 * h], sides=12)
    parts.append((tv, tf, 2, [[("Head", 1.0)]] * len(tv)))
    return parts


def assemble_hair(parts, rig, materials):
    verts, faces, face_mats, weights = [], [], [], []
    for pverts, pfaces, slot, pweights in parts:
        offset = len(verts)
        verts.extend(pverts)
        weights.extend(pweights)
        for face in pfaces:
            faces.append(tuple(i + offset for i in face))
            face_mats.append(slot)
    mesh = bpy.data.meshes.new("Hair")
    mesh.from_pydata([tuple(v) for v in verts], [], faces)
    mesh.update()
    obj = bpy.data.objects.new("Hair", mesh)
    bpy.context.scene.collection.objects.link(obj)
    for mat in materials:
        mesh.materials.append(mat)
    for face, slot in zip(mesh.polygons, face_mats):
        face.material_index = slot
        face.use_smooth = True
    bone_names = [b.name for b in rig.data.bones]
    for i, vweights in enumerate(weights):
        for bone_suffix, w in vweights:
            if w <= 0.0:
                continue
            name = next(b for b in bone_names if b.endswith(bone_suffix))
            group = obj.vertex_groups.get(name) or obj.vertex_groups.new(name=name)
            group.add([i], w, "REPLACE")
    mod = obj.modifiers.new("Armature", "ARMATURE")
    mod.object = rig
    print(f"DETAIL hair: {len(verts)} verts, {len(faces)} faces")
    return obj


def main():
    argv = sys.argv[sys.argv.index("--") + 1:]
    in_path, out_path, face_path, chest_path = argv
    bpy.ops.wm.read_factory_settings(use_empty=True)
    bpy.ops.import_scene.gltf(filepath=in_path)
    rig = next(o for o in bpy.data.objects if o.type == "ARMATURE")
    body = bpy.data.objects.get("LumenBody")
    assert body is not None and body.data.shape_keys, "LumenBody missing"
    bones = {b.name: b for b in rig.data.bones}
    neck = next(b for b in bones if b.endswith("Neck"))
    neck_z = bones[neck].head_local.z

    metrics = head_metrics(body, neck_z)
    apply_face(body, metrics, image_material("Face", face_path, "FaceUV"))
    apply_chest_print(body, rig, image_material("ChestPrint", chest_path, "PrintUV"))

    old_hair = bpy.data.objects.get("Hair")
    if old_hair is not None:
        old_mesh = old_hair.data
        bpy.data.objects.remove(old_hair, do_unlink=True)
        bpy.data.meshes.remove(old_mesh)
    hair_mats = [srgb_material("Hair", HAIR_RGB, 0.5),
                 srgb_material("HairAccent", ACCENT_RGB, 0.4),
                 srgb_material("HairTie", TIE_RGB, 0.35)]
    assemble_hair(build_hair(metrics), rig, hair_mats)

    bpy.ops.export_scene.gltf(
        filepath=out_path, export_format="GLB", export_yup=True,
        export_skins=True, export_def_bones=True, export_influence_nb=4,
        export_morph=True, export_animations=False)
    print(f"DETAIL wrote {out_path}")


if __name__ == "__main__":
    main()
