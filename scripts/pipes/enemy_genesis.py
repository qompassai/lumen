#!/usr/bin/env python3
"""Enemy genesis for the SFW pipes (D-863, Stage 3, PIPES.md s.9).

Seeded procedural variation on the character pipeline: a
family record carries a base stat block + behavior block (all
data) and weighted variant axes; the generator (SplitMix64,
the estate's seeded idiom) emits a canonical variant table
whose SHA-256 over the canonical variant list is the genesis
hash -- the Niflheim floor-genesis precedent, whose genesis
value is pinned in the game's own unit tests.

Generation runs twice inside the gate (double-bake); a pin
file records the genesis hash so any later re-run is checked
against it exactly.

Usage:
    enemy_genesis.py <family.json> --out <table.json>
                     --record-out <run-record.json>
                     [--pin <pin.json>] [--check-pin <pin.json>]
Exit 0 iff generation is stable (and the pin matches, when
checked); exit 3 on a typed refusal.
"""

import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from fx_eval import splitmix64
from run_record import (REFUSAL_EXIT, canonical_bytes, check_lane,
                        refuse, sha256_hex, write_run_record)

MASK64 = (1 << 64) - 1


def pick_weighted(draw, values, weights):
    total = sum(weights)
    r = draw % total
    acc = 0
    for value, w in zip(values, weights):
        acc += w
        if r < acc:
            return value
    return values[-1]


def generate(family):
    """The variant table: a pure function of the family record."""
    seed = int(family["seed"])
    base = family["base"]["stat_block"]
    derived = family.get("derived", {})
    variants = []
    for i in range(int(family["variant_count"])):
        state = (seed ^ (i * 0x9E3779B97F4A7C15)) & MASK64
        chosen = {}
        for axis in family["variant_axes"]:
            draw, state = splitmix64(state)
            chosen[axis["name"]] = pick_weighted(
                draw, axis["values"], axis["weights"])
        stats = dict(base)
        stats["vigour"] = (base["vigour"] + chosen.get("hoard_depth", 0)
                           * int(derived.get("vigour_per_depth", 0)))
        stats["guard"] = (base["guard"] + chosen.get("chart_promises", 0)
                          * int(derived.get("guard_per_promise", 0)))
        variant = {"index": i}
        variant.update(chosen)
        variant["stat_block"] = stats
        variants.append(variant)
    return variants


def genesis_hash(variants):
    return sha256_hex(canonical_bytes(variants))


def main(argv):
    ap = argparse.ArgumentParser()
    ap.add_argument("family")
    ap.add_argument("--out", required=True)
    ap.add_argument("--record-out", required=True)
    ap.add_argument("--pin")
    ap.add_argument("--check-pin")
    a = ap.parse_args(argv[1:])

    family = json.loads(Path(a.family).read_bytes())
    refusal = check_lane(family)
    if refusal is not None:
        return refuse(refusal[0], refusal[1])
    if family.get("format") != "lumen-genesis-family/1":
        return refuse("lane_unclassified",
                      "not a lumen-genesis-family/1 record (format %r)"
                      % (family.get("format"),))

    variants = generate(family)
    variants2 = generate(family)
    h1, h2 = genesis_hash(variants), genesis_hash(variants2)
    if h1 != h2:
        return refuse("double_bake_mismatch",
                      "genesis unstable: %s vs %s" % (h1, h2))

    table = {"family": family["id"], "seed": family["seed"],
             "genesis_hash": h1, "variants": variants}
    Path(a.out).write_bytes(canonical_bytes(table) + b"\n")

    refusals = []
    if a.pin:
        pin = {"family": family["id"], "genesis_hash": h1}
        Path(a.pin).write_bytes(canonical_bytes(pin) + b"\n")
    if a.check_pin:
        pin = json.loads(Path(a.check_pin).read_bytes())
        if pin.get("genesis_hash") != h1:
            refusals.append("genesis_mismatch")
    write_run_record(a.record_out, pipe_id="content", pipe_version="1.0",
                     gate_id="enemy-genesis", gate_version="1.0",
                     inputs={"family": a.family},
                     outputs={"table": a.out},
                     verdict="genesis-stable" if not refusals else "refused",
                     refusals=refusals)
    print(json.dumps({"family": family["id"], "genesis_hash": h1,
                      "variants": len(variants),
                      "pin_match": not refusals}, indent=1, sort_keys=True))
    if refusals:
        return refuse(refusals[0],
                      "genesis hash %s does not match the pin" % h1)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
