#!/usr/bin/env python3
"""Asset record checks for the content pipes (D-865 + keeper
chain, codified by the mcp-codify pass 2026-11-03).

Two mechanical checks every asset record must pass before a
pipe output is treated as an asset:

    stamp  The D-865 metadata law on an asset sidecar: status
           (born "candidate"; a keeper carries the gate ref
           that promoted it, and an auditable status history),
           made_by in the girl-name roster in all three layers
           (name, category, underlying model id + weights
           hash) -- an unnamed model never generates into the
           databases, so a sidecar naming one is refused --
           plus seed, pipe, pipe run, frozen record hash, and
           the lane stamp. A lesson pack (D-871) loaded as
           generation context is recorded, versioned, in the
           optional lessons field.
    chain  Keeper-chain completeness (art bible s.1.1): every
           mandatory field present and well-formed. An asset
           without a complete chain is not a keeper, whatever
           it looks like; with --asset, the recorded sha256
           must match the file's actual bytes.

Usage:
    asset_record_check.py stamp <sidecar.json> [--lane LANE]
    asset_record_check.py chain <chain.json> [--asset FILE]

One verdict JSON on stdout. Exit 0 iff the check passes;
exit 3 on a typed refusal; exit 2 on usage error. The script
never writes to the files it checks.
"""

import hashlib
import json
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from run_record import REFUSAL_EXIT, USAGE_EXIT

# The roster of record (2026-10-21, D-865): first letter is the
# capability category. A model with no roster name does not
# generate into the asset databases.
ROSTER = {
    "Aria": "audio",
    "Clara": "code",
    "Freya": "fast",
    "Iris": "image",
    "Mira": "multimodal",
    "Opal": "operator",
    "Sylvia": "structured",
    "Tessa": "tools",
    "Vera": "video",
}

HEX64 = re.compile(r"^[0-9a-f]{64}$")
STATUSES = ("candidate", "keeper", "rejected")


def _is_int(v):
    return isinstance(v, int) and not isinstance(v, bool)


def _refuse(code, detail):
    print(json.dumps({"accepted": False, "refusal": code,
                      "detail": detail}, sort_keys=True))
    return REFUSAL_EXIT


def _accept(kind):
    print(json.dumps({"accepted": True, "check": kind},
                     sort_keys=True))
    return 0


def check_stamp(obj, lane):
    """Returns a (code, detail) refusal tuple or None."""
    def bad(detail):
        return ("stamp_invalid", detail)
    if not isinstance(obj, dict):
        return ("stamp_invalid", "sidecar is not a JSON object")
    for field in ("lane", "origin_lane", "content_label"):
        if field not in obj:
            return bad("missing lane stamp field: %s" % field)
    if obj["content_label"] not in ("sfw", "nsfw"):
        return ("lane_unclassified",
                "content_label is %r; unclassified defaults to "
                "the NSFW side" % (obj["content_label"],))
    if lane == "qompass" and (obj["lane"] != "qompass"
                              or obj["content_label"] != "sfw"):
        return ("lane_mismatch",
                "a qompass asset is sfw-labelled and qompass-stamped")
    if lane == "ntranch" and obj["lane"] != "ntranch":
        return ("lane_mismatch", "asset lane is not ntranch")
    status = obj.get("status")
    if status not in STATUSES:
        return bad("status must be one of %s, got %r"
                   % ("/".join(STATUSES), status))
    history = obj.get("status_history")
    if not isinstance(history, list) or not history:
        return bad("status_history must be a non-empty list "
                   "(status is auditable from birth)")
    for entry in history:
        if not isinstance(entry, dict) \
                or entry.get("status") not in STATUSES \
                or not isinstance(entry.get("at"), str):
            return bad("each status_history entry needs a status "
                       "and an at timestamp")
    if status != "candidate":
        if not isinstance(obj.get("status_gate_ref"), str) \
                or not obj["status_gate_ref"]:
            return bad("a %s asset carries the gate ref that "
                       "decided it (status_gate_ref)" % status)
        if not any(e.get("status") == status for e in history):
            return bad("status_history has no %s entry" % status)
    made_by = obj.get("made_by")
    if not isinstance(made_by, dict):
        return ("made_by_missing",
                "made_by must name the generating model in the roster")
    name = made_by.get("name")
    if name not in ROSTER:
        return ("made_by_unnamed",
                "made_by.name %r is not in the roster; unnamed "
                "models never generate into the databases" % (name,))
    if made_by.get("category") != ROSTER[name]:
        return bad("made_by.category must be %r for %s"
                   % (ROSTER[name], name))
    if not isinstance(made_by.get("model_id"), str) \
            or not made_by["model_id"]:
        return bad("made_by.model_id must name the underlying model")
    if not HEX64.match(str(made_by.get("weights_sha256", ""))):
        return bad("made_by.weights_sha256 must be a 64-hex SHA-256")
    if not _is_int(obj.get("seed")):
        return bad("seed must be an integer")
    for field in ("pipe", "pipe_run", "format"):
        if not isinstance(obj.get(field), str) or not obj[field]:
            return bad("%s must be a non-empty string" % field)
    if not HEX64.match(str(obj.get("record_hash", ""))):
        return bad("record_hash must be a 64-hex SHA-256 of the "
                   "frozen record the asset was emitted from")
    lessons = obj.get("lessons")
    if lessons is not None:
        if not isinstance(lessons, dict) \
                or not isinstance(lessons.get("pack"), str) \
                or not lessons.get("pack") \
                or not isinstance(lessons.get("version"), str) \
                or not lessons.get("version"):
            return bad("lessons, when present, names the pack and "
                       "its version (D-871)")
    return None


def check_chain(obj, asset_path):
    """Keeper-chain completeness (art bible s.1.1). Returns a
    (code, detail) refusal tuple or None."""
    def bad(detail):
        return ("chain_incomplete", detail)
    if not isinstance(obj, dict):
        return ("chain_incomplete", "chain is not a JSON object")
    for field in ("asset_id", "game", "kind", "record_id",
                  "canon_refs", "prompt_pos", "prompt_neg", "model",
                  "conditioning", "seed", "finish_ops", "sha256",
                  "verdict"):
        if field not in obj:
            return bad("missing keeper-chain field: %s" % field)
    if obj["record_id"] is None and obj["kind"] != "ui-ornament":
        return bad("record_id is null only for pure UI ornament")
    if not isinstance(obj["canon_refs"], list):
        return bad("canon_refs must be a list")
    for ref in obj["canon_refs"]:
        if not isinstance(ref, dict) or not ref.get("id") \
                or not HEX64.match(str(ref.get("sha256", ""))):
            return bad("each canon_ref needs an id and a 64-hex sha256")
    if not isinstance(obj["prompt_pos"], str) or not obj["prompt_pos"]:
        return bad("prompt_pos must be the verbatim positive prompt")
    if not isinstance(obj["prompt_neg"], str):
        return bad("prompt_neg must be the verbatim negative prompt")
    model = obj["model"]
    if not isinstance(model, dict):
        return bad("model must be an object")
    for field in ("filename", "sha256", "sampler", "steps", "cfg", "size"):
        if field not in model:
            return bad("model is missing field: %s" % field)
    if not HEX64.match(str(model.get("sha256", ""))):
        return bad("model.sha256 must be a 64-hex SHA-256")
    if not _is_int(obj["seed"]):
        return bad("seed must be an integer")
    if not isinstance(obj["finish_ops"], list):
        return bad("finish_ops must be a list")
    for op in obj["finish_ops"]:
        if not isinstance(op, dict) or not op.get("op"):
            return bad("each finish op names its operation")
    if not HEX64.match(str(obj.get("sha256", ""))):
        return bad("sha256 must be the 64-hex SHA-256 of the asset")
    verdict = obj["verdict"]
    if not isinstance(verdict, dict) \
            or verdict.get("status") not in STATUSES:
        return bad("verdict.status must be candidate, keeper, or rejected")
    if verdict["status"] == "keeper" \
            and not (isinstance(verdict.get("ruling"), str)
                     and verdict["ruling"]):
        return bad("a keeper verdict names its ruling source "
                   "(Owner item number / date)")
    if asset_path is not None:
        actual = hashlib.sha256(Path(asset_path).read_bytes()).hexdigest()
        if actual != obj["sha256"]:
            return ("chain_hash_mismatch",
                    "asset bytes hash to %s, chain records %s"
                    % (actual, obj["sha256"]))
    return None


def main(argv):
    args = argv[1:]
    lane = None
    asset = None
    while "--lane" in args or "--asset" in args:
        if "--lane" in args:
            i = args.index("--lane")
            lane = args[i + 1] if i + 1 < len(args) else None
            del args[i:i + 2]
        if "--asset" in args:
            i = args.index("--asset")
            asset = args[i + 1] if i + 1 < len(args) else None
            del args[i:i + 2]
    if len(args) != 2 or args[0] not in ("stamp", "chain"):
        sys.stderr.write(__doc__)
        return USAGE_EXIT
    kind, path = args
    try:
        obj = json.loads(Path(path).read_bytes())
    except Exception as ex:
        return _refuse("record_unreadable", "%s: %s" % (path, ex))
    if kind == "stamp":
        refusal = check_stamp(obj, lane)
    else:
        refusal = check_chain(obj, asset)
    if refusal is not None:
        return _refuse(refusal[0], refusal[1])
    return _accept(kind)


if __name__ == "__main__":
    sys.exit(main(sys.argv))
