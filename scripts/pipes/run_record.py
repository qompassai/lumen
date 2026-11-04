#!/usr/bin/env python3
"""Shared conformance tooling for the SFW content pipes (D-863).

Implements the mechanics every pipe shares (PIPES.md sections 1
and 8): canonical JSON, SHA-256 artifact hashing, the Qompass
lane stamp, typed pre-mutation lane refusals, and conformance
run records with a mechanical verifier.

A run record is evidence, not a claim: the verifier recomputes
every hash it names. A record whose artifacts are missing or
whose hashes do not match is REVIEW under D-713 -- not passed,
not failed, unverified.

Usage:
    run_record.py verify <run-record.json>
        Prints a verdict JSON. Exit 0 iff the verdict is
        CONFORMS; exit 4 for REVIEW.

Library use (the other pipe scripts import this module):
    from run_record import (canonical_bytes, sha256_hex,
        check_lane, write_run_record, LANE_STAMP, REFUSAL_EXIT)
"""

import hashlib
import json
import sys
from datetime import datetime, timezone
from pathlib import Path

LANE_STAMP = {"lane": "qompass", "origin_lane": "qompass",
              "content_label": "sfw"}

# Exit codes, stable across all pipe scripts.
OK_EXIT = 0
USAGE_EXIT = 2
REFUSAL_EXIT = 3   # a typed refusal was issued (pre-mutation)
REVIEW_EXIT = 4    # conformance could not be verified


def canonical_bytes(obj):
    """Key-sorted canonical JSON, the pipes' one byte form."""
    return json.dumps(obj, sort_keys=True, separators=(",", ":"),
                      ensure_ascii=False).encode("utf-8")


def sha256_hex(data):
    return hashlib.sha256(data).hexdigest()


def file_hash(path):
    return sha256_hex(Path(path).read_bytes())


def check_lane(obj):
    """Lane intake rule (PIPES.md section 1).

    Returns None when the object may be processed by a Qompass
    pipe, else (refusal_code, detail). Only the content_label
    governs: 'sfw' accepts; 'nsfw' is a lane mismatch; anything
    else -- including a missing label -- is unclassified and
    defaults to the NSFW side, fail closed.
    """
    if not isinstance(obj, dict):
        return ("lane_unclassified", "not a JSON object")
    label = obj.get("content_label")
    if label == "sfw":
        return None
    if label == "nsfw":
        return ("lane_mismatch",
                "content_label is 'nsfw'; Qompass pipes refuse it")
    return ("lane_unclassified",
            "content_label is %r; unclassified content defaults "
            "to the NSFW side and is refused" % (label,))


def refuse(code, detail, ids=None):
    """Emit a typed refusal as JSON on stdout; caller exits
    with REFUSAL_EXIT. Nothing has been mutated when this runs."""
    out = {"refusal": code, "detail": detail}
    if ids:
        out["ids"] = ids
    print(json.dumps(out, indent=1, sort_keys=True))
    return REFUSAL_EXIT


def write_run_record(path, *, pipe_id, pipe_version, gate_id,
                     gate_version, inputs, outputs, verdict,
                     refusals=(), ruling_refs=()):
    """Write a conformance run record (PIPES.md section 8).

    inputs/outputs map artifact names to file paths; each is
    recorded with its SHA-256 at write time. The lane stamp is
    not optional: a pipe output without it is not a pipe output.
    """
    def entries(mapping):
        return {name: {"path": str(p), "sha256": file_hash(p)}
                for name, p in sorted(mapping.items())}

    record = {
        "record": "lumen-pipe-run/1",
        **LANE_STAMP,
        "pipe": {"id": pipe_id, "version": pipe_version},
        "gate": {"id": gate_id, "version": gate_version},
        "recorded_at": datetime.now(timezone.utc).isoformat(),
        "inputs": entries(inputs),
        "outputs": entries(outputs),
        "verdict": verdict,
        "refusals": list(refusals),
        "ruling_refs": list(ruling_refs),
    }
    Path(path).write_bytes(canonical_bytes(record) + b"\n")
    return record


def verify_run_record(path):
    """Recompute a run record's evidence. Returns a verdict
    dict: CONFORMS, or REVIEW with the reasons.

    Structural anomalies are REVIEW reasons, never exceptions:
    a verifier that crashes on a malformed record turns an
    unverifiable record into an unhandled error, and D-713
    routes unverifiable to REVIEW (mcp-codify 2026-11-03)."""
    reasons = []
    try:
        record = json.loads(Path(path).read_bytes())
    except Exception as ex:
        return {"verdict": "REVIEW",
                "reasons": ["run record unreadable: %s" % ex]}
    if not isinstance(record, dict):
        return {"verdict": "REVIEW",
                "reasons": ["run record is not a JSON object"]}
    for field in ("record", "lane", "origin_lane", "content_label",
                  "pipe", "gate", "inputs", "outputs", "verdict"):
        if field not in record:
            reasons.append("missing field: %s" % field)
    if record.get("record") != "lumen-pipe-run/1":
        reasons.append("not a lumen-pipe-run/1 record")
    for key, want in LANE_STAMP.items():
        if record.get(key) != want:
            reasons.append("lane stamp %s is %r, want %r"
                           % (key, record.get(key), want))
    for section in ("inputs", "outputs"):
        entries = record.get(section, {})
        if not isinstance(entries, dict):
            reasons.append("%s is not an object" % section)
            continue
        for name, entry in sorted(entries.items()):
            if not isinstance(entry, dict):
                reasons.append("%s %s: entry is not an object"
                               % (section, name))
                continue
            p = entry.get("path", "")
            if not isinstance(p, str) or not p:
                reasons.append("%s %s: entry has no path"
                               % (section, name))
                continue
            if not Path(p).exists():
                reasons.append("%s %s: artifact missing: %s"
                               % (section, name, p))
            elif file_hash(p) != entry.get("sha256"):
                reasons.append("%s %s: hash mismatch" % (section, name))
    if reasons:
        return {"verdict": "REVIEW", "reasons": reasons}
    return {"verdict": "CONFORMS",
            "pipe": record["pipe"], "gate": record["gate"],
            "run_verdict": record["verdict"]}


def main(argv):
    if len(argv) == 3 and argv[1] == "verify":
        verdict = verify_run_record(argv[2])
        print(json.dumps(verdict, indent=1, sort_keys=True))
        return OK_EXIT if verdict["verdict"] == "CONFORMS" else REVIEW_EXIT
    sys.stderr.write(__doc__)
    return USAGE_EXIT


if __name__ == "__main__":
    sys.exit(main(sys.argv))
