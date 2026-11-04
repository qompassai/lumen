#!/usr/bin/env python3
"""Variant sweep gate for the SFW pipes' rig fixtures (D-863).

The RENDER-1 sweep pattern, as the gate for part-variant
machinery (PIPES.md s.9): render the rest frame (all defaults),
then for EVERY parameter render at min and at max with all
other parameters at default. A parameter passes iff
  (1) at least one extreme changes > 0 pixels vs rest
      (per-channel max diff > 8), and
  (2) >= 98% of changed pixels fall inside the union of its
      bound parts' AABBs (rest + driven positions, dilated).
A parameter that moves nothing is an inert channel -- a build
defect, never a known limitation.

The rest frame is rendered twice and the two PNGs must be
byte-identical (the ship pipe's double-bake, applied at
fixture scale). The run writes its JSON evidence plus a
conformance run record.

Usage:
    sweep_variants.py <rig.inx> <import-dir> <out.json>
                      --render-bin <puppet_render>
                      --record-out <run-record.json>
                      [--work <dir>] [--scale 1.0]
Exit 0 iff every parameter passes and the double-bake holds.
"""

import argparse
import json
import struct
import subprocess
import sys
from pathlib import Path

import numpy as np
from PIL import Image, ImageChops

sys.path.insert(0, str(Path(__file__).resolve().parent))
from run_record import REFUSAL_EXIT, file_hash, write_run_record

W, H = 600, 800


def parse_inx(path):
    d = Path(path).read_bytes()
    assert d[:8] == b"TRNSRTS\x00", "bad magic"
    ln = struct.unpack(">I", d[8:12])[0]
    return json.loads(d[12:12 + ln])


def walk(nodes, ox=0.0, oy=0.0):
    for n in nodes:
        t = n.get("transform", {}).get("trans", [0, 0, 0])
        ax, ay = ox + t[0], oy + t[1]
        yield n, ax, ay
        yield from walk(n.get("children", []), ax, ay)


def part_aabbs(payload):
    out = {}
    for n, ax, ay in walk([payload["nodes"]]):
        if n.get("type") == "Part" and "mesh" in n:
            v = n["mesh"]["verts"]
            xs = [ax + v[i] for i in range(0, len(v), 2)]
            ys = [ay + v[i + 1] for i in range(0, len(v), 2)]
            out[n["uuid"]] = (min(xs), min(ys), max(xs), max(ys))
    return out


def descendants(payload):
    out = {}

    def rec(n):
        s = set()
        if n.get("type") == "Part":
            s.add(n["uuid"])
        for c in n.get("children", []):
            s |= rec(c)
        out[n["uuid"]] = s
        return s

    rec(payload["nodes"])
    return out


class Renderer:
    def __init__(self, render_bin, import_dir, work, scale):
        self.bin = render_bin
        self.dir = import_dir
        self.work = Path(work)
        self.scale = scale
        self.work.mkdir(parents=True, exist_ok=True)

    def render(self, tag, sets):
        out_png = self.work / ("%s.png" % tag)
        args = [self.bin, "--dir", self.dir, "--out", str(out_png),
                "--frames", "30", "--scale", str(self.scale),
                "--width", str(W), "--height", str(H)]
        for name, val in sets:
            args += ["--set", "%s=%s,0" % (name, val)]
        r = subprocess.run(args, capture_output=True, text=True)
        if r.returncode != 0:
            raise RuntimeError("render failed %s: %s" % (sets, r.stderr))
        return Image.open(out_png).convert("RGBA"), out_png


def diff_mask(a, b):
    d = ImageChops.difference(a, b)
    arr = np.asarray(d).astype(int)
    return arr.max(axis=2) > 8


def main(argv):
    ap = argparse.ArgumentParser()
    ap.add_argument("inx")
    ap.add_argument("import_dir")
    ap.add_argument("out")
    ap.add_argument("--render-bin", required=True)
    ap.add_argument("--record-out", required=True)
    ap.add_argument("--work", default="/tmp/sfw-sweep")
    ap.add_argument("--scale", type=float, default=1.0)
    a = ap.parse_args(argv[1:])

    payload = parse_inx(a.inx)
    aabbs = part_aabbs(payload)
    desc = descendants(payload)
    cx, cy = W / 2, H / 2
    renderer = Renderer(a.render_bin, a.import_dir, a.work, a.scale)

    rest, rest_png = renderer.render("rest", [])
    rest2, rest2_png = renderer.render("rest-bake2", [])
    bake = {"first_sha256": file_hash(rest_png),
            "second_sha256": file_hash(rest2_png)}
    bake["identical"] = bake["first_sha256"] == bake["second_sha256"]

    results = {}
    for q in payload["param"]:
        name = q["name"]
        mn, mx = q["min"][0], q["max"][0]
        region, dil = [], 14.0
        for b in q.get("bindings", []):
            for pu in desc.get(b["node"], set()):
                if pu in aabbs:
                    region.append(aabbs[pu])
            if b["param_name"] == "deform":
                for yrow in b["values"]:
                    for pt in yrow:
                        for dx, dy in pt:
                            dil = max(dil, abs(dx) + 14, abs(dy) + 14)
            elif b["param_name"].startswith("transform.t"):
                for v in b["values"]:
                    dil = max(dil, abs(v[0]) + 14)
        entry = {"min": mn, "max": mx, "default": q["defaults"][0],
                 "bindings": len(q.get("bindings", []))}
        for label, val in (("at_min", mn), ("at_max", mx)):
            if val == q["defaults"][0]:
                entry[label] = {"changed": 0, "note": "extreme==default"}
                continue
            img, _ = renderer.render("%s-%s" % (name, label),
                                     [(name, val)])
            mask = diff_mask(rest, img)
            count = int(mask.sum())
            frac = None
            if count and region:
                x0 = min(r[0] for r in region) - dil
                y0 = min(r[1] for r in region) - dil
                x1 = max(r[2] for r in region) + dil
                y1 = max(r[3] for r in region) + dil
                px0 = max(0, int(cx + x0 * a.scale))
                py0 = max(0, int(cy + y0 * a.scale))
                px1 = min(W, int(cx + x1 * a.scale) + 1)
                py1 = min(H, int(cy + y1 * a.scale) + 1)
                frac = float(mask[py0:py1, px0:px1].sum()) / count
            entry[label] = {"changed": count, "region_frac": frac}
        moved = max(entry["at_min"].get("changed", 0),
                    entry["at_max"].get("changed", 0))
        fracs = [entry[k]["region_frac"] for k in ("at_min", "at_max")
                 if entry[k].get("region_frac") is not None]
        entry["pass"] = bool(moved > 0
                             and (not fracs or min(fracs) >= 0.98))
        entry["max_changed"] = moved
        results[name] = entry
        print("%s: max_changed=%d pass=%s"
              % (name, moved, entry["pass"]), flush=True)

    n_pass = sum(1 for e in results.values() if e["pass"])
    summary = {"params": len(results), "pass": n_pass,
               "fail": [k for k, e in results.items() if not e["pass"]],
               "rest_double_bake": bake}
    Path(a.out).write_text(json.dumps(
        {"summary": summary, "results": results}, indent=1))
    print("SUMMARY", json.dumps(summary))
    ok = n_pass == len(results) and bake["identical"]
    write_run_record(a.record_out, pipe_id="art-production",
                     pipe_version="1.0", gate_id="rig-variant-sweep",
                     gate_version="1.0",
                     inputs={"rig": a.inx},
                     outputs={"sweep": a.out},
                     verdict="pass" if ok else "refused",
                     refusals=[] if ok else ["chain_incomplete"])
    return 0 if ok else REFUSAL_EXIT


if __name__ == "__main__":
    sys.exit(main(sys.argv))
