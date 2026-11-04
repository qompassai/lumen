#!/usr/bin/env python3
"""Author the twobody mannequin fixtures (models + motions).

Two profile-view mannequins in the lumen rig format. Figure A
faces +x. Figure B is A mirrored (faces -x) and scaled 0.95.
The grasp is tuned by forward kinematics in this script: a grid
search picks (shoulder, elbow) so A's near palm lands on B's
front waist vertex at the grasp instant with a small constant
contact offset, and A's far forearm length is solved so the far
palm lands on B's back waist vertex at the same angles (both
arms share one deformer chain, so palm separation then stays
rigid-consistent for the whole hold).

FK here mirrors the Rust evaluator's semantics exactly:
rotation delta about the deformer's effective origin, chains
applied root-first (spine = 0 at the grasp instant).
"""
import json
import math
import os

OUT = os.path.join(os.path.dirname(os.path.abspath(__file__)), "fixtures")
os.makedirs(OUT, exist_ok=True)

# --- skeleton (A, rest) -------------------------------------------------
SHOULDER = (4.0, 130.0)
ELBOW = (10.5, 105.0)
NEAR_PALM = (17.0, 82.0)
FAR_PALM_BASE = (24.0, 75.0)
WAIST_Y = 116.0
B_SCALE = 0.95
# B waist vertices in B local space (mirrored): front (-7*s, 116*s),
# back (+7*s, 116*s). World = placement + local (no rotation).
B_FRONT_WAIST_X = -7.0 * B_SCALE
B_WAIST_Y = WAIST_Y * B_SCALE


def rot(point, origin, degrees):
    r = math.radians(degrees)
    c, s = math.cos(r), math.sin(r)
    x, y = point[0] - origin[0], point[1] - origin[1]
    return (origin[0] + c * x - s * y, origin[1] + s * x + c * y)


def fk_palm(palm, shoulder_deg, elbow_deg):
    """World position of a palm vertex under (shoulder, elbow)."""
    elbow_world = rot(ELBOW, SHOULDER, shoulder_deg)
    after_shoulder = rot(palm, SHOULDER, shoulder_deg)
    return rot(after_shoulder, elbow_world, elbow_deg)


# --- grasp tuning ---------------------------------------------------------
best = None
for sh10 in range(400, 1201, 5):  # shoulder 20.0..60.0 step 0.25
    sh = sh10 / 10.0
    for el10 in range(0, 1101, 5):  # elbow 0.0..55.0 step 0.25
        el = el10 / 10.0
        near = fk_palm(NEAR_PALM, sh, el)
        bx = near[0] - B_FRONT_WAIST_X  # B placement x: near x exact
        dy = B_WAIST_Y - near[1]  # near contact offset (constant)
        if abs(dy) > 5.0:
            continue
        # Solve far forearm scale k about the elbow so the far
        # palm's x hits B's back waist x at the same angles.
        elbow_world = rot(ELBOW, SHOULDER, sh)
        far1 = fk_palm(FAR_PALM_BASE, sh, el)
        # Palm under scale k: E + k*(P - E) then elbow rotation is
        # linear, so world palm x is affine in k. Probe k=1, k=1.2.
        def fk_palm_scaled(k):
            p = (ELBOW[0] + k * (FAR_PALM_BASE[0] - ELBOW[0]),
                 ELBOW[1] + k * (FAR_PALM_BASE[1] - ELBOW[1]))
            return fk_palm(p, sh, el)
        far2 = fk_palm_scaled(1.2)
        target_x = bx + 7.0 * B_SCALE
        if far2[0] == far1[0]:
            continue
        k = 1.0 + (target_x - far1[0]) * 0.2 / (far2[0] - far1[0])
        if not (0.8 <= k <= 1.35):
            continue
        far = fk_palm_scaled(k)
        dy_far = B_WAIST_Y - far[1]
        if abs(dy_far) > 5.0:
            continue
        cost = (abs(dy) + abs(dy_far) + 2.0 * abs(k - 1.0)
                + 0.02 * abs(sh - 40.0) + 0.08 * max(0.0, 18.0 - el))
        if best is None or cost < best[0]:
            best = (cost, sh, el, bx, dy, dy_far, k)
assert best, "no grasp solution in grid"
_, SH_GRASP, EL_GRASP, B_X, OFF_NEAR, OFF_FAR, FAR_K = best
print(f"grasp: shoulder={SH_GRASP} elbow={EL_GRASP} B_x={B_X:.9f} "
      f"offset_near={OFF_NEAR:.9f} offset_far={OFF_FAR:.9f} far_k={FAR_K:.9f}")
print(f"B_X_CONSTANT = {B_X:.9f}")
# Offsets under the module's b-local semantics are the
# NEGATIVES of the scene-axes dy values above.
print(f"B_LOCAL_OFFSET_NEAR = {-OFF_NEAR:.9f}")
print(f"B_LOCAL_OFFSET_FAR = {-OFF_FAR:.9f}")

# --- model construction ---------------------------------------------------
PARAMS = [  # (id, min, max, default) in A's convention
    ("elbow_bend", 0.0, 125.0, 10.0),
    ("head_tilt", -18.0, 18.0, 0.0),
    ("hip_swing", -35.0, 45.0, 0.0),
    ("knee_bend", -115.0, 0.0, 0.0),
    ("shoulder_swing", -15.0, 125.0, 0.0),
    ("spine_lean", -18.0, 8.0, 0.0),
]


def quad(a, b, c, d):
    return [a, b, c, d], [0, 1, 2, 0, 2, 3]


def poly(points):
    idx = []
    for i in range(1, len(points) - 1):
        idx += [0, i, i + 1]
    return points, idx


def build_model(model_id, mirror, scale):
    def tx(p):
        return (-scale * p[0], scale * p[1]) if mirror else (scale * p[0], scale * p[1])

    def rng(lo, hi, default):
        if mirror:
            return (-hi, -lo, -default)
        return (lo, hi, default)

    parameters = []
    for pid, lo, hi, default in PARAMS:
        lo2, hi2, d2 = rng(lo, hi, default)
        parameters.append({"kind": "scalar", "id": pid, "min": lo2, "max": hi2, "default": d2})

    def rotation_def(def_id, parent, origin, param):
        lo, hi, _ = rng(*[v for v in [p[1], p[2], p[3]] for p in PARAMS if p[0] == param][0]) \
            if False else (None, None, None)
        # identity lattice: form angle == key value, keys = param range
        prow = [p for p in PARAMS if p[0] == param][0]
        lo2, hi2, _ = rng(prow[1], prow[2], prow[3])
        return {
            "kind": "rotation", "id": def_id, "parent": parent,
            "origin": list(tx(origin)), "base_angle_degrees": 0.0,
            "keyforms": {
                "axes": [param], "keys": [[lo2, hi2]],
                "forms": [
                    {"at": [0], "angle_degrees": lo2, "scale": [1.0, 1.0]},
                    {"at": [1], "angle_degrees": hi2, "scale": [1.0, 1.0]},
                ],
            },
        }

    deformers = [
        rotation_def("elbow", "shoulder", ELBOW, "elbow_bend"),
        rotation_def("hip", None, (1.0, 93.0), "hip_swing"),
        rotation_def("knee", "hip", (2.5, 54.0), "knee_bend"),
        rotation_def("neck", "spine", (0.0, 140.0), "head_tilt"),
        rotation_def("shoulder", "spine", SHOULDER, "shoulder_swing"),
        rotation_def("spine", None, (0.0, 104.0), "spine_lean"),
    ]

    def mesh(mesh_id, part, deformer, draw_order, points, indices, k_about_elbow=None):
        pts = []
        for p in points:
            q = p
            if k_about_elbow is not None:
                q = (ELBOW[0] + k_about_elbow * (p[0] - ELBOW[0]),
                     ELBOW[1] + k_about_elbow * (p[1] - ELBOW[1]))
            pts.append(list(tx(q)))
        m = {
            "id": mesh_id, "part": part, "texture": "tex",
            "vertices": pts, "uvs": [[0.0, 0.0]] * len(pts),
            "indices": indices, "draw_order": draw_order,
        }
        if deformer is not None:
            m["deformer"] = deformer
        return m

    meshes = []
    v, i = poly([(1, 132), (6, 129), (10, 106), (5, 103)])
    meshes.append(mesh("arm_far_upper", "arm_far", "shoulder", -4, v, i))
    v, i = poly([(5, 107), (10, 103), (27, 79), (24, 75), (20, 79)])
    meshes.append(mesh("arm_far_fore", "arm_far", "elbow", -3, v, i, k_about_elbow=FAR_K))
    v, i = poly([(-5, 96), (2, 96), (3, 54), (-4, 54)])
    meshes.append(mesh("leg_far_thigh", "leg_far", "hip", -2, v, i))
    v, i = poly([(-4, 54), (3, 54), (2, 10), (-3, 10)])
    meshes.append(mesh("leg_far_shin", "leg_far", "knee", -2, v, i))
    v, i = poly([(-3, 10), (2, 10), (10, 4), (10, 0), (-3, 0)])
    meshes.append(mesh("leg_far_foot", "leg_far", "knee", -2, v, i))
    v, i = poly([(-9, 90), (9, 90), (9, 104), (-9, 104)])
    meshes.append(mesh("pelvis", "pelvis", None, 0, v, i))
    v, i = poly([(-2, 96), (5, 96), (6, 54), (-1, 54)])
    meshes.append(mesh("leg_near_thigh", "leg_near", "hip", 1, v, i))
    v, i = poly([(-1, 54), (6, 54), (5, 10), (0, 10)])
    meshes.append(mesh("leg_near_shin", "leg_near", "knee", 1, v, i))
    v, i = poly([(0, 10), (5, 10), (13, 4), (13, 0), (0, 0)])
    meshes.append(mesh("leg_near_foot", "leg_near", "knee", 1, v, i))
    v, i = poly([(-7, 104), (7, 104), (7, 116), (6, 136), (-5, 136), (-7, 116)])
    meshes.append(mesh("torso", "torso", "spine", 2, v, i))
    v, i = poly([(-3, 136), (4, 136), (4, 143), (-3, 143)])
    meshes.append(mesh("neck", "torso", "spine", 2, v, i))
    v, i = poly([(2, 132), (7, 129), (13, 106), (8, 103)])
    meshes.append(mesh("arm_near_upper", "arm_near", "shoulder", 4, v, i))
    v, i = poly([(8, 107), (13, 103), (20, 85), (17, 82), (15, 80)])
    meshes.append(mesh("arm_near_fore", "arm_near", "elbow", 5, v, i))
    # head: octagon + centre vertex (index 8)
    pts = []
    for n in range(8):
        a = math.radians(45 * n)
        pts.append((1 + 8.5 * math.cos(a), 152 + 9.5 * math.sin(a)))
    pts.append((1, 152))
    idx = []
    for n in range(8):
        idx += [8, n, (n + 1) % 8]
    meshes.append(mesh("head", "head", "neck", 6, pts, idx))
    if mirror:
        for m in meshes:
            m["draw_order"] += 20

    parts = [{"id": p} for p in
             ["arm_far", "arm_near", "head", "leg_far", "leg_near", "pelvis", "torso"]]
    return {
        "format": "lumen-rig-model", "format_version": 1, "id": model_id,
        "parameters": parameters, "parts": parts,
        "textures": [{"id": "tex", "source": "mannequin.png"}],
        "deformers": deformers, "meshes": meshes, "physics": [],
    }


def build_motion(motion_id, model_id, curves_semantic, mirror):
    curves = []
    for param, keys in curves_semantic:
        kfs = []
        for t, v in keys:
            kfs.append({"time": t, "value": -v if mirror else v,
                        "interpolation": "cubic"})
        curves.append({"parameter": param, "keyframes": kfs})
    return {
        "format": "lumen-rig-motion", "format_version": 1, "id": motion_id,
        "model": model_id, "duration_seconds": 12.0, "frames_per_second": 60,
        "curves": curves,
    }


SH, EL = SH_GRASP, EL_GRASP
motion_a = build_motion("lift_a", "mannequin_a", [
    ("shoulder_swing", [(0, 0), (1.2, 0), (1.8, SH), (3.0, SH + 2), (3.6, SH + 10),
                        (5.0, SH + 26), (8.0, SH + 26), (9.6, SH + 8), (10.2, SH),
                        (11.0, 16), (12.0, 0)]),
    ("elbow_bend", [(0, 10), (1.8, EL), (5.0, EL - 6), (8.0, EL - 6),
                    (10.2, EL), (12.0, 10)]),
    ("spine_lean", [(0, 0), (3.0, 0), (3.8, -6), (5.0, -3), (8.0, -3),
                    (9.8, -5), (11.0, 0), (12.0, 0)]),
    # Legs: the crouch is IK-solved in this generator's
    # terms - hip +theta / knee -theta keeps A's feet flat
    # at ground level while the placement dips (theta 17.25
    # for the 2.2-unit dip, 12.25 for the 1.2-unit set-down
    # dip; solved by grid search on the leg chain FK, both
    # foot bottom corners landing on the dip line).
    ("hip_swing", [(0, 0), (3.0, 0), (3.9, 17.25), (5.0, 0),
                   (9.6, 12.25), (10.4, 0), (12.0, 0)]),
    ("knee_bend", [(0, 0), (3.0, 0), (3.9, -17.25), (5.0, 0),
                   (9.6, -12.25), (10.4, 0), (12.0, 0)]),
    ("head_tilt", [(0, 0), (2.0, -4), (3.6, 0), (5.0, 14), (8.0, 14),
                   (9.6, 4), (10.2, -4), (12.0, 0)]),
], mirror=False)
motion_b = build_motion("lift_b", "mannequin_b", [
    ("shoulder_swing", [(0, -6), (1.8, 2), (2.0, 26), (5.0, 30),
                        (10.2, 26), (11.2, 6), (12.0, -6)]),
    ("elbow_bend", [(0, 8), (1.8, 18), (2.0, 42), (10.2, 42),
                    (11.2, 16), (12.0, 8)]),
    ("spine_lean", [(0, 0), (3.6, 0), (5.0, -4), (10.0, -2), (12.0, 0)]),
    ("hip_swing", [(0, 0), (3.4, 0), (4.2, -10), (5.2, -16), (8.0, -16),
                   (9.4, -6), (10.2, 0), (12.0, 0)]),
    ("knee_bend", [(0, 0), (3.4, 0), (4.4, -38), (5.4, -52), (8.6, -52),
                   (9.6, -14), (10.2, 0), (12.0, 0)]),
    ("head_tilt", [(0, 0), (1.6, 5), (2.0, 6), (3.6, 0), (5.0, -4),
                   (9.0, -4), (9.9, 3), (10.4, 0), (12.0, 0)]),
], mirror=True)

files = {
    "mannequin_a.model.json": build_model("mannequin_a", mirror=False, scale=1.0),
    "mannequin_b.model.json": build_model("mannequin_b", mirror=True, scale=B_SCALE),
    "lift_a.motion.json": motion_a,
    "lift_b.motion.json": motion_b,
}
for name, doc in files.items():
    with open(os.path.join(OUT, name), "w") as fh:
        json.dump(doc, fh, indent=1, sort_keys=True)
        fh.write("\n")
    print("wrote", name)
