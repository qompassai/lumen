#!/usr/bin/env python3
"""Author the SFW pipes' Stage 1 fixture puppets (D-863).

Builds two synthetic INP1 (.inx) puppets whose parts are TEST
plates (flat colour, darker border, white label bar -- the
estate's engineering-puppet precedent). Mechanics are the
deliverable; the plates are labelled TEST in each file's meta.

  props-stratum1.inx -- a Niflheim Stratum I object set:
      plank stone at two depths (variant pair), a torch whose
      base has two palette colourways and whose flame is a set
      part, the Borrowed Hoard chest (lid on a deform binding,
      chart as a set part), a seal token as the always-visible
      control.
  outfit-pupil.inx -- a teaching-game outfit on a TEST
      mannequin: a tunic in two palette colourways (sibling
      parts under one palette parameter), a cloak+sash set,
      a hood set. Clothing is part variants + palette slots as
      data -- never painted onto the base plates.

Part-variant mechanics (PIPES.md s.9): a set parameter binds a
part's transform.s.x / transform.s.y to [0, 1]; at 0 the part
is absent from the render. Variant pairs bind two siblings in
opposition under one parameter.

Draw order, measured on the evaluator of record (D-863
fixtures, centre-pixel probes): parts paint in descending
zsort order -- the SMALLEST zsort is frontmost. The zsorts
below are assigned under that convention (an overlay that must
read in front of its base part carries the smaller zsort).

Usage: author_fixtures.py <out-dir>
Deterministic: the same script produces byte-identical .inx
files (the ship pipe's double-bake checks this).
"""

import json
import struct
import sys
import zlib
from pathlib import Path

M32 = 0xFFFFFFFF


# ---------- INX container ------------------------------------
def write_inx(payload, textures):
    pj = json.dumps(payload, separators=(",", ":"),
                    ensure_ascii=False).encode()
    out = b"TRNSRTS\x00" + struct.pack(">I", len(pj)) + pj
    out += b"TEX_SECT" + struct.pack(">I", len(textures))
    for flag, blob in textures:
        out += struct.pack(">I", len(blob)) + bytes([flag]) + blob
    return out


def png_rgba(w, h, pixel_fn):
    raw = b""
    for y in range(h):
        raw += b"\x00" + b"".join(bytes(pixel_fn(x, y))
                                  for x in range(w))

    def chunk(tag, data):
        c = struct.pack(">I", len(data)) + tag + data
        return c + struct.pack(">I", zlib.crc32(tag + data) & M32)

    ihdr = struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr)
            + chunk(b"IDAT", zlib.compress(raw, 9))
            + chunk(b"IEND", b""))


def test_texture(rgb):
    """48x48 TEST plate: flat colour, darker border, label bar."""
    r, g, b = rgb
    dk = (int(r * 0.55), int(g * 0.55), int(b * 0.55))

    def px(x, y):
        if x < 3 or y < 3 or x >= 45 or y >= 45:
            return (dk[0], dk[1], dk[2], 255)
        if y < 9:
            return (245, 245, 245, 255)
        return (r, g, b, 255)

    return png_rgba(48, 48, px)


# ---------- payload helpers ----------------------------------
def quad_part(name, uuid, tex_slot, w, h, tx, ty, zsort):
    return {
        "blend_mode": "Normal", "emissionStrength": 1.0,
        "enabled": True, "lockToRoot": False, "mask_threshold": 0.5,
        "mesh": {"indices": [0, 1, 2, 2, 1, 3], "origin": [0, 0],
                 "uvs": [0, 0, 1, 0, 0, 1, 1, 1],
                 "verts": [-w / 2, -h / 2, w / 2, -h / 2,
                           -w / 2, h / 2, w / 2, h / 2]},
        "name": name, "opacity": 1.0, "screenTint": [0, 0, 0],
        "textures": [tex_slot, M32, M32], "tint": [1, 1, 1],
        "transform": {"rot": [0, 0, 0], "scale": [1, 1],
                      "trans": [tx, ty, 0]},
        "type": "Part", "uuid": uuid, "zsort": zsort,
    }


def set_bindings(node_uuid, on_at):
    """Bindings that scale a part from 0 at axis 0 to full at
    axis 1 (on_at=1), or the inverse for the outgoing sibling
    of a variant pair (on_at=0)."""
    vals = [0, 1] if on_at == 1 else [1, 0]
    out = []
    for prop in ("transform.s.x", "transform.s.y"):
        out.append({"isSet": [[True], [True]], "node": node_uuid,
                    "param_name": prop, "interpolate_mode": "Linear",
                    "values": [[float(vals[0])], [float(vals[1])]]})
    return out


def deform_binding(node_uuid, sets):
    return {"isSet": [[True] for _ in sets], "node": node_uuid,
            "param_name": "deform", "interpolate_mode": "Linear",
            "values": [[[[dx, dy] for dx, dy in s]] for s in sets]}


def param(uuid, name, axis, default, bindings):
    return {
        "axis_points": [axis, [0]], "defaults": [default, 0],
        "is_vec2": False,
        # Channel y carries the dummy [0, 1] range even for a
        # 1D parameter -- the encoding the landed rigs use; the
        # importer refuses min == max on any channel.
        "max": [axis[-1], 1.0], "min": [axis[0], 0.0],
        "name": name, "uuid": uuid, "bindings": bindings,
    }


def build_puppet(name, parts, params):
    """parts: (uuid, name, w, h, cx, cy, zsort, rgb). Texture
    slots follow part order."""
    textures = [(0, test_texture(rgb))
                for (_, _, _, _, _, _, _, rgb) in parts]
    children = [quad_part(nm, uid, i, w, h, cx, cy, z)
                for i, (uid, nm, w, h, cx, cy, z, _) in enumerate(parts)]
    payload = {
        "meta": {"name": name,
                 "rigger": "lumen SFW pipes fixture (D-863)",
                 "version": "INP 0.8"},
        "nodes": {"children": children, "enabled": True,
                  "lockToRoot": False, "name": "Root",
                  "transform": {"rot": [0, 0, 0], "scale": [1, 1],
                                "trans": [0, 0, 0]},
                  "type": "Node", "uuid": 1, "zsort": 0},
        "param": params,
        "physics": [],
    }
    return write_inx(payload, textures)


# ---------- the two fixtures ---------------------------------
def props_stratum1():
    parts = [
        (101, "PlankStoneNear", 220, 54, 0, 120, 0.12, (96, 110, 128)),
        (102, "PlankStoneFar", 150, 38, 0, 36, 0.12, (96, 110, 128)),
        (103, "TorchBaseCold", 26, 90, -190, -40, 0.11, (110, 116, 124)),
        (104, "TorchBaseMoss", 26, 90, -190, -40, 0.11, (92, 120, 84)),
        (105, "TorchFlame", 30, 44, -190, -128, 0.08, (240, 170, 60)),
        (106, "HoardChestBase", 170, 90, 120, 116, 0.10, (104, 78, 52)),
        (107, "HoardLid", 170, 30, 120, 56, 0.07, (124, 94, 62)),
        (108, "HoardChart", 90, 64, 120, 110, 0.06, (216, 200, 150)),
        (109, "SealToken", 44, 44, -150, 150, 0.08, (190, 150, 70)),
    ]
    params = [
        param(201, "ParamPlankDepth", [0, 1], 0,
              set_bindings(101, 0) + set_bindings(102, 1)),
        param(202, "ParamStonePalette", [0, 1], 0,
              set_bindings(103, 0) + set_bindings(104, 1)),
        param(203, "ParamTorchLit", [0, 1], 0, set_bindings(105, 1)),
        param(204, "ParamHoardOpen", [0, 1], 0,
              [deform_binding(107, [[[0, 0]] * 4,
                                    [[0, -40], [0, -40],
                                     [0, -26], [0, -26]]])]),
        param(205, "ParamChartShown", [0, 1], 0, set_bindings(108, 1)),
    ]
    return build_puppet("Stratum I Props (TEST)", parts, params)


def outfit_pupil():
    parts = [
        (101, "HeadPlate", 64, 64, 0, -150, 0.10, (216, 188, 168)),
        (102, "TorsoPlate", 110, 130, 0, -40, 0.10, (150, 150, 160)),
        (103, "ArmLPlate", 26, 95, -76, -35, 0.10, (216, 188, 168)),
        (104, "ArmRPlate", 26, 95, 76, -35, 0.10, (216, 188, 168)),
        (105, "LegLPlate", 34, 120, -30, 95, 0.10, (90, 90, 100)),
        (106, "LegRPlate", 34, 120, 30, 95, 0.10, (90, 90, 100)),
        (107, "TunicCold", 120, 124, 0, -44, 0.06, (84, 102, 150)),
        (108, "TunicMoss", 120, 124, 0, -44, 0.06, (96, 128, 88)),
        (109, "CloakPlate", 156, 180, 0, -16, 0.20, (70, 64, 96)),
        (110, "SashPlate", 122, 18, 0, 10, 0.04, (196, 150, 60)),
        (111, "HoodPlate", 78, 54, 0, -172, 0.05, (70, 64, 96)),
    ]
    params = [
        param(201, "ParamTunicPalette", [0, 1], 0,
              set_bindings(107, 0) + set_bindings(108, 1)),
        param(202, "ParamCloakSet", [0, 1], 0,
              set_bindings(109, 1) + set_bindings(110, 1)),
        param(203, "ParamHoodSet", [0, 1], 0, set_bindings(111, 1)),
    ]
    return build_puppet("Pupil + Outfit (TEST)", parts, params)


def main(argv):
    if len(argv) != 2:
        sys.stderr.write(__doc__)
        return 2
    out = Path(argv[1])
    out.mkdir(parents=True, exist_ok=True)
    for name, blob in (("props-stratum1.inx", props_stratum1()),
                       ("outfit-pupil.inx", outfit_pupil())):
        (out / name).write_bytes(blob)
        print("%s %d bytes" % (name, len(blob)))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
