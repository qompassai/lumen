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

# A family table is a bounded artifact: 4,096 variants is far
# past any floor's needs, and an unbounded count turns a data
# file into a memory exhaustion attack on the pipe host
# (mcp-codify 2026-11-03; Tiger Style bounded work).
MAX_VARIANTS = 4096


def _is_int(v):
    return isinstance(v, int) and not isinstance(v, bool)


def validate_family(family):
    """Structural validation before any generation. Returns a
    (code, detail) refusal tuple or None. Every malformed shape
    is a typed refusal here, never a traceback downstream:
    zero/empty weights divided by zero, a missing seed raised
    KeyError, and a float seed was silently truncated -- all
    found by adversarial probe (mcp-codify 2026-11-03)."""
    def bad(detail):
        return ("genesis_family_invalid", detail)
    if not isinstance(family.get("id"), str) or not family["id"]:
        return bad("id must be a non-empty string")
    if not _is_int(family.get("seed")):
        return bad("seed must be an integer, got %r" % (family.get("seed"),))
    count = family.get("variant_count")
    if not _is_int(count) or not 1 <= count <= MAX_VARIANTS:
        return bad("variant_count must be an integer in 1..=%d, got %r"
                   % (MAX_VARIANTS, count))
    base = family.get("base")
    if not isinstance(base, dict) or not isinstance(base.get("stat_block"), dict):
        return bad("base.stat_block must be an object")
    for stat in ("vigour", "guard"):
        if not _is_int(base["stat_block"].get(stat)):
            return bad("base.stat_block.%s must be an integer" % stat)
    derived = family.get("derived", {})
    if not isinstance(derived, dict):
        return bad("derived must be an object")
    for key, val in derived.items():
        if not _is_int(val):
            return bad("derived.%s must be an integer" % key)
    axes = family.get("variant_axes")
    if not isinstance(axes, list) or not axes:
        return bad("variant_axes must be a non-empty list")
    for axis in axes:
        if not isinstance(axis, dict) or not isinstance(axis.get("name"), str):
            return bad("each variant axis needs a string name")
        values, weights = axis.get("values"), axis.get("weights")
        if not isinstance(values, list) or not values:
            return bad("axis %s: values must be a non-empty list" % axis["name"])
        if (not isinstance(weights, list) or len(weights) != len(values)
                or any(not _is_int(w) or w < 0 for w in weights)):
            return bad("axis %s: weights must be non-negative integers "
                       "matching values in length" % axis["name"])
        if sum(weights) <= 0:
            return bad("axis %s: weights must sum to more than zero"
                       % axis["name"])
        if axis["name"] in ("hoard_depth", "chart_promises") \
                and any(not _is_int(v) for v in values):
            return bad("axis %s feeds stat derivation: values must "
                       "be integers" % axis["name"])
    return None


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

    try:
        family = json.loads(Path(a.family).read_bytes())
    except Exception as ex:
        return refuse("genesis_unreadable",
                      "family record unreadable: %s" % ex)
    refusal = check_lane(family)
    if refusal is not None:
        return refuse(refusal[0], refusal[1])
    if not isinstance(family, dict) \
            or family.get("format") != "lumen-genesis-family/1":
        return refuse("lane_unclassified",
                      "not a lumen-genesis-family/1 record (format %r)"
                      % (family.get("format") if isinstance(family, dict)
                         else None,))
    refusal = validate_family(family)
    if refusal is not None:
        return refuse(refusal[0], refusal[1])

    variants = generate(family)
    variants2 = generate(family)
    h1, h2 = genesis_hash(variants), genesis_hash(variants2)
    if h1 != h2:
        return refuse("double_bake_mismatch",
                      "genesis unstable: %s vs %s" % (h1, h2))

    # The pin is checked BEFORE the table is written: a refusal
    # is pre-mutation, so a mismatched pin leaves no table
    # behind for a downstream step to consume by mistake
    # (mcp-codify 2026-11-03).
    refusals = []
    if a.check_pin:
        try:
            pin = json.loads(Path(a.check_pin).read_bytes())
        except Exception as ex:
            return refuse("genesis_unreadable",
                          "pin record unreadable: %s" % ex)
        if not isinstance(pin, dict) or pin.get("genesis_hash") != h1:
            refusals.append("genesis_mismatch")
    if refusals:
        write_run_record(a.record_out, pipe_id="content",
                         pipe_version="1.0",
                         gate_id="enemy-genesis", gate_version="1.0",
                         inputs={"family": a.family},
                         outputs={},
                         verdict="refused",
                         refusals=refusals)
        return refuse(refusals[0],
                      "genesis hash %s does not match the pin" % h1)

    table = {"family": family["id"], "seed": family["seed"],
             "genesis_hash": h1, "variants": variants}
    Path(a.out).write_bytes(canonical_bytes(table) + b"\n")

    if a.pin:
        pin = {"family": family["id"], "genesis_hash": h1}
        Path(a.pin).write_bytes(canonical_bytes(pin) + b"\n")
    write_run_record(a.record_out, pipe_id="content", pipe_version="1.0",
                     gate_id="enemy-genesis", gate_version="1.0",
                     inputs={"family": a.family},
                     outputs={"table": a.out},
                     verdict="genesis-stable",
                     refusals=[])
    print(json.dumps({"family": family["id"], "genesis_hash": h1,
                      "variants": len(variants),
                      "pin_match": True}, indent=1, sort_keys=True))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
