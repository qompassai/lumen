#!/usr/bin/env python3
"""Evaluator for lumen-fx/1 effect records (D-863, Stage 2).

Evaluates a record twice, independently; the two runs must
agree bit-for-bit. Per evaluation:
  * a fixed-step, integer-only particle simulation whose
    per-step state feeds a SHA-256 digest chain -- the final
    digest is the replay hash;
  * the photosafety flash gate: the intensity track sampled at
    120 Hz, flashes counted by the extrema walk exactly as
    shipped in Versemend's game/effects.lua (a swing of
    >= 80 milli of full-field luminance between successive
    extrema counts one half flash), capped at 3 flashes/second,
    plus the declared no-inversion check.

Usage:
    fx_eval.py <effect.fx.json> --out <result.json>
               --record-out <run-record.json>
Exit 0 iff the record is fx-verified (hash agreement, flash
rate at or under cap, uses_inversion false, lane accepted).
"""

import argparse
import hashlib
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from run_record import (REFUSAL_EXIT, canonical_bytes, check_lane,
                        refuse, write_run_record)

FLASH_SWING_MILLI = 80
FLASH_PER_SECOND_MAX = 3.0
SAMPLE_HZ = 120
MASK64 = (1 << 64) - 1

# Integer sine table, one quadrant (0..90 deg in 17 steps of
# 5.625 deg), milli-units. Literal so no platform libm is ever
# consulted: the format's determinism is integer-only.
SIN_QUARTER = [0, 98, 195, 290, 383, 471, 556, 634, 707, 773,
               831, 882, 924, 956, 981, 995, 1000]


def sin_milli(deg_steps):
    """sin_milli(k) = 1000 * sin(2*pi*k/360), integer-only."""
    k = deg_steps % 360
    quad, rem = divmod(k, 90)
    idx = round(rem * 16 / 90)
    if quad == 0:
        return SIN_QUARTER[idx]
    if quad == 1:
        return SIN_QUARTER[16 - idx]
    if quad == 2:
        return -SIN_QUARTER[idx]
    return -SIN_QUARTER[16 - idx]


def splitmix64(state):
    state = (state + 0x9E3779B97F4A7C15) & MASK64
    z = state
    z = ((z ^ (z >> 30)) * 0xBF58476D1CE4E5B9) & MASK64
    z = ((z ^ (z >> 27)) * 0x94D049BB133111EB) & MASK64
    return (z ^ (z >> 31)) & MASK64, state


def track_duration_ms(track):
    return sum(int(s["duration_ms"]) for s in track["segments"])


def intensity_at(track, t_ms):
    """Full-field luminance delta (milli) at t_ms, integer
    fixed-point throughout."""
    total = track_duration_ms(track)
    if total <= 0:
        return 0
    if track.get("loop"):
        t_ms %= total
    elif t_ms >= total:
        t_ms = total
    acc = 0
    for seg in track["segments"]:
        dur = int(seg["duration_ms"])
        if t_ms > acc + dur:
            acc += dur
            continue
        local = t_ms - acc
        kind = seg["kind"]
        if kind == "hold":
            return int(seg["value"])
        if kind == "sine":
            # angle step: 360 * local / period, in whole steps
            step = (360 * local) // int(seg["period_ms"])
            return int(seg["base"]) + int(seg["amp"]) * sin_milli(step) // 1000
        frm, to = int(seg["from"]), int(seg["to"])
        if dur == 0:
            return to
        if kind == "linear":
            return frm + (to - frm) * local // dur
        if kind == "smoothstep":
            x = (1000 * local) // dur
            s = (x * x * (3000 - 2 * x)) // 1000000
            return frm + (to - frm) * s // 1000
        raise ValueError("unknown segment kind %r" % kind)
    last = track["segments"][-1]
    return int(last.get("to", last.get("value", 0)))


def flashes_per_second(track):
    """The shipped algorithm (Versemend effects.lua), on the
    milli-unit track sampled at 120 Hz."""
    total = track_duration_ms(track)
    window_ms = max(total if not track.get("loop") else 10000, 1000)
    step = 1000 // SAMPLE_HZ
    extrema = []
    prev = intensity_at(track, 0)
    direction = 0
    t = step
    while t <= window_ms:
        v = intensity_at(track, t)
        d = v - prev
        if d != 0:
            dirn = 1 if d > 0 else -1
            if direction != 0 and dirn != direction:
                extrema.append(prev)
            direction = dirn
        prev = v
        t += step
    extrema.append(prev)
    flashes = 0.0
    for i in range(1, len(extrema)):
        if abs(extrema[i] - extrema[i - 1]) >= FLASH_SWING_MILLI:
            flashes += 0.5
    return flashes / (window_ms / 1000)


def simulate(record):
    """One full deterministic evaluation. Returns the replay
    hash and the per-run measurements."""
    track = record["intensity"]
    step_hz = int(record["step_hz"])
    duration = int(record["duration_steps"])
    emitters = record.get("emitters", [])
    digest = hashlib.sha256(b"lumen-fx/1").digest()
    for step in range(duration):
        t_ms = (step * 1000) // step_hz
        inten = intensity_at(track, t_ms)
        sum_x = sum_y = alive = 0
        for ei, em in enumerate(emitters):
            count = int(em["count"])
            life = int(em["life_steps"])
            for i in range(count):
                offset = (i * life) // count
                age = (step + offset) % life
                cycle = (step + offset) // life
                key = (int(record["seed"]) ^ (ei << 40) ^ (i << 8)
                       ^ cycle) & MASK64
                r1, _ = splitmix64(key)
                r2, _ = splitmix64(r1)
                spread = int(em["spread_x"])
                x0 = int(em["origin"][0]) + (r1 % (spread + 1)) - spread // 2
                vx = (r2 % (2 * int(em["vel_x_jitter"]) + 1)) \
                    - int(em["vel_x_jitter"])
                vy = int(em["vel_y"])
                g = int(em["gravity_y"])
                # integer kinematics: p = p0 + v*age + g*age^2/2
                x = x0 + vx * age
                y = int(em["origin"][1]) + vy * age + g * age * age // 2
                sum_x += x
                sum_y += y
                alive += 1
        row = canonical_bytes([step, inten, sum_x, sum_y, alive])
        digest = hashlib.sha256(digest + row).digest()
    return {"replay_hash": digest.hex(),
            "flash_rate": flashes_per_second(track)}


def main(argv):
    ap = argparse.ArgumentParser()
    ap.add_argument("fx")
    ap.add_argument("--out", required=True)
    ap.add_argument("--record-out", required=True)
    a = ap.parse_args(argv[1:])

    record = json.loads(Path(a.fx).read_bytes())
    refusal = check_lane(record)
    if refusal is not None:
        write_run_record(a.record_out, pipe_id="art-production",
                         pipe_version="1.0", gate_id="lumen-fx",
                         gate_version="1.0", inputs={"effect": a.fx},
                         outputs={}, verdict="refused",
                         refusals=[refusal[0]])
        return refuse(refusal[0], refusal[1])
    if record.get("format") != "lumen-fx/1":
        return refuse("lane_unclassified",
                      "not a lumen-fx/1 record (format %r)"
                      % (record.get("format"),))
    if record.get("uses_inversion") is not False:
        write_run_record(a.record_out, pipe_id="art-production",
                         pipe_version="1.0", gate_id="lumen-fx",
                         gate_version="1.0", inputs={"effect": a.fx},
                         outputs={}, verdict="refused",
                         refusals=["flash_gate_inversion"])
        return refuse("flash_gate_inversion",
                      "uses_inversion must be declared false")

    run1 = simulate(record)
    run2 = simulate(record)
    agree = run1["replay_hash"] == run2["replay_hash"]
    flash_ok = run1["flash_rate"] <= FLASH_PER_SECOND_MAX
    result = {"effect": record["id"], "format": "lumen-fx/1",
              "replay_hash": run1["replay_hash"],
              "replay_hash_second": run2["replay_hash"],
              "hash_agreement": agree,
              "flash_rate_per_second": round(run1["flash_rate"], 4),
              "flash_cap_per_second": FLASH_PER_SECOND_MAX,
              "flash_ok": flash_ok,
              "fx_verified": bool(agree and flash_ok)}
    Path(a.out).write_bytes(canonical_bytes(result) + b"\n")
    write_run_record(a.record_out, pipe_id="art-production",
                     pipe_version="1.0", gate_id="lumen-fx",
                     gate_version="1.0", inputs={"effect": a.fx},
                     outputs={"result": a.out},
                     verdict="fx-verified" if result["fx_verified"]
                             else "refused",
                     refusals=[] if result["fx_verified"]
                              else ["double_bake_mismatch" if not agree
                                    else "flash_gate_rate"])
    print(json.dumps(result, indent=1, sort_keys=True))
    return 0 if result["fx_verified"] else REFUSAL_EXIT


if __name__ == "__main__":
    sys.exit(main(sys.argv))
