#!/usr/bin/env python3
"""Canon census for the SFW content pipes (D-863, PIPES.md s.4).

Gate of record: lumen-canon-census 1.0. Runs over a project's
canon-record/1 set as declared by a manifest:

    {"project": "<id>",
     "records": [{"id": "<record id>", "file": "<path>"}, ...],
     "declared_pairs": [["<id>", "<id>"], ...],
     "denylist": ["<term>", ...],
     "shippable_fields": ["<field>", ...]}

Checks: counts (declared vs found), orphans in both directions
(manifest entries with no file; record files in the manifest's
directory the manifest does not declare; references to ids that
do not resolve), differentiation per declared pair (name
equality is a collision; descriptor overlap is reported
numerically -- the machine reports, the Owner decides),
sanitization of the manifest's shippable fields against the
denylist (findings carry field locations; nothing is edited),
and a state hash for staleness.

Every record passes the lane gate first; a lane refusal aborts
the census pre-mutation (no census artifact is written). The
census writes its artifact plus a conformance run record.

Usage:
    canon_census.py <manifest.json> --out <census.json>
                    --record-out <run-record.json>
        Exit 0 iff the canon state verifies; exit 3 on a typed
        refusal (lane, canon_orphan, sanitization_hit, ...).
    canon_census.py --check-stale <census.json> <manifest.json>
        Recompute the state hash: CURRENT or STALE.
"""

import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from run_record import (OK_EXIT, REFUSAL_EXIT, USAGE_EXIT,
                        canonical_bytes, check_lane, file_hash,
                        refuse, sha256_hex, write_run_record)

GATE_ID, GATE_VERSION = "lumen-canon-census", "1.0"


def load_json(path):
    return json.loads(Path(path).read_bytes())


def record_state_hash(records_by_id):
    """SHA-256 over the canonical (id, file-hash) set."""
    items = sorted((rid, file_hash(p))
                   for rid, p in records_by_id.items())
    return sha256_hex(canonical_bytes(items))


def descriptor_tokens(record):
    tokens = set(record.get("display_name", "").lower().split())
    for inv in record.get("identity_invariants", []):
        tokens.add(str(inv.get("name", "")).lower())
    return tokens


def run_census(manifest_path):
    """Returns (exit_code, census_or_None, refusal_codes)."""
    manifest = load_json(manifest_path)
    base = Path(manifest_path).resolve().parent
    declared = manifest.get("records", [])
    records, files_by_id = {}, {}
    orphans = []

    for entry in declared:
        rid, rel = entry["id"], entry["file"]
        path = (base / rel).resolve()
        if not path.exists():
            orphans.append({"kind": "manifest_without_file",
                            "id": rid})
            continue
        record = load_json(path)
        refusal = check_lane(record)
        if refusal is not None:
            code, detail = refusal
            return None, None, [(code, "%s: %s" % (rid, detail))]
        if record.get("id") != rid:
            orphans.append({"kind": "id_mismatch", "id": rid,
                            "found": record.get("id")})
        records[rid] = record
        files_by_id[rid] = path

    declared_files = {(base / e["file"]).resolve() for e in declared}
    for path in sorted(base.glob("*.record.json")):
        if path.resolve() not in declared_files:
            orphans.append({"kind": "file_without_manifest",
                            "file": path.name})

    for rid, record in records.items():
        for ref in record.get("references", []):
            if ref not in {e["id"] for e in declared}:
                orphans.append({"kind": "unresolved_reference",
                                "id": rid, "references": ref})

    differentiation = []
    refusal_codes = []
    if orphans:
        refusal_codes.append("canon_orphan")
    for pair in manifest.get("declared_pairs", []):
        a, b = (records.get(pair[0]), records.get(pair[1]))
        if a is None or b is None:
            differentiation.append({"pair": pair,
                                    "verdict": "unresolved"})
            continue
        if a.get("display_name") == b.get("display_name"):
            differentiation.append({"pair": pair,
                                    "verdict": "collision"})
            refusal_codes.append("differentiation_collision")
            continue
        ta, tb = descriptor_tokens(a), descriptor_tokens(b)
        union = ta | tb
        overlap = (len(ta & tb) / len(union)) if union else 0.0
        differentiation.append({"pair": pair, "verdict": "distinct",
                                "descriptor_overlap": round(overlap, 4)})

    hits = []
    denylist = manifest.get("denylist", [])
    for rid, record in sorted(records.items()):
        for field in manifest.get("shippable_fields", []):
            value = record.get(field)
            if not isinstance(value, str):
                continue
            for term in denylist:
                if term in value:
                    hits.append({"record": rid, "field": field,
                                 "term": term})
    if hits:
        refusal_codes.append("sanitization_hit")

    verified = not refusal_codes
    census = {
        "gate": "%s %s" % (GATE_ID, GATE_VERSION),
        "project": manifest.get("project"),
        "state_hash": record_state_hash(files_by_id),
        "counts": {"declared": len(declared), "found": len(records)},
        "orphans": orphans,
        "differentiation": differentiation,
        "sanitization_hits": hits,
        "verified": verified,
    }
    return census, files_by_id, refusal_codes


def main(argv):
    if len(argv) == 4 and argv[1] == "--check-stale":
        census = load_json(argv[2])
        result, _, _ = run_census(argv[3])
        if result is None:
            return refuse("lane_unclassified",
                          "census re-run refused; staleness unknown")
        state = ("CURRENT" if result["state_hash"] == census["state_hash"]
                 else "STALE")
        print(json.dumps({"state": state,
                          "recorded": census["state_hash"],
                          "current": result["state_hash"]},
                         sort_keys=True))
        return OK_EXIT
    if len(argv) != 6 or argv[2] != "--out" or argv[4] != "--record-out":
        sys.stderr.write(__doc__)
        return USAGE_EXIT
    manifest_path, out_path, record_path = argv[1], argv[3], argv[5]
    census, files_by_id, refusal_codes = run_census(manifest_path)
    if census is None:
        code, detail = refusal_codes[0]
        write_run_record(record_path, pipe_id="canon", pipe_version="1.0",
                         gate_id=GATE_ID, gate_version=GATE_VERSION,
                         inputs={"manifest": manifest_path}, outputs={},
                         verdict="refused", refusals=[code])
        return refuse(code, detail)
    Path(out_path).write_bytes(canonical_bytes(census) + b"\n")
    inputs = {"manifest": manifest_path}
    inputs.update({"record:%s" % rid: str(p)
                   for rid, p in sorted(files_by_id.items())})
    verdict = "canon verified" if census["verified"] else "refused"
    write_run_record(record_path, pipe_id="canon", pipe_version="1.0",
                     gate_id=GATE_ID, gate_version=GATE_VERSION,
                     inputs=inputs, outputs={"census": out_path},
                     verdict=verdict, refusals=refusal_codes)
    print(json.dumps(census, indent=1, sort_keys=True))
    if refusal_codes:
        return refuse(refusal_codes[0],
                      "census completed with findings; verified state "
                      "refused", ids=refusal_codes)
    return OK_EXIT


if __name__ == "__main__":
    sys.exit(main(sys.argv))
