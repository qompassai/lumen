#!/usr/bin/env python3
"""Lane gate for the SFW content pipes (D-863, PIPES.md s.1).

Checks JSON content files against the lane intake rule before
any pipe processes them. Refusals are typed and pre-mutation:
this script never writes to the files it checks.

Usage:
    check_lane.py <file.json> [more.json ...]
        One verdict JSON per file on stdout. Exit 0 iff every
        file is accepted; exit 3 if any file is refused.
"""

import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from run_record import REFUSAL_EXIT, USAGE_EXIT, check_lane


def main(argv):
    if len(argv) < 2:
        sys.stderr.write(__doc__)
        return USAGE_EXIT
    worst = 0
    for name in argv[1:]:
        try:
            obj = json.loads(Path(name).read_bytes())
        except Exception as ex:
            print(json.dumps({"file": name, "accepted": False,
                              "refusal": "lane_unclassified",
                              "detail": "unreadable: %s" % ex},
                             sort_keys=True))
            worst = REFUSAL_EXIT
            continue
        refusal = check_lane(obj)
        if refusal is None:
            print(json.dumps({"file": name, "accepted": True},
                             sort_keys=True))
        else:
            code, detail = refusal
            print(json.dumps({"file": name, "accepted": False,
                              "refusal": code, "detail": detail},
                             sort_keys=True))
            worst = REFUSAL_EXIT
    return worst


if __name__ == "__main__":
    sys.exit(main(sys.argv))
