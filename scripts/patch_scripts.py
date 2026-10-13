"""One-shot patch: move the pipeline's base-mesh and gate scripts onto
the canonical portrait-emotion list (templates/emotions.json, schema
lumen.emotion-templates v1, ratified by Matt 2026-10-06).

Replaces the provisional PORTRAIT_EMOTIONS block in the base-mesh script
with the canonical block (bumping BASE_VERSION 1 -> 2) and the matching
provisional EXPRESSIONS block in the gate script with the canonical one.

The two targets are pipeline scripts that do not live in this repo, so
their paths are never hardcoded. Each target resolves, in order:
  1. its environment override (LUMEN_BASE_MESH / LUMEN_GATE), else
  2. a file of the same name beside this script.
Both targets are resolved and validated before anything is written; a
missing target or a missing expected block stops the run with every
file left untouched.
"""

import os
from pathlib import Path

BASE_MESH_NAME = "lumen_base_mesh.py"
GATE_NAME = "lumen_gate.py"

NEW_BLOCK = """# Canonical portrait emotions: templates/emotions.json (schema
# lumen.emotion-templates v1, ratified by Matt 2026-10-06). 'fun' is
# PLAYFUL (VRoid/VRM 0.x semantics), not VRM 1.0 'relaxed'. Do not rename
# without bumping the template version and the base version together.
PORTRAIT_EMOTIONS = (
    "happy", "angry", "sad", "surprised", "fun",
    "annoyed", "worried", "embarrassed", "determined", "smug",
)
"""

NEW_GATE = """EXPRESSIONS = (
    "happy", "angry", "sad", "surprised", "fun",
    "annoyed", "worried", "embarrassed", "determined", "smug",
"""

OLD_BLOCK = """# Provisional names: no authoritative list of the 10 portrait emotions was
# found in light-show or lumen. Rename here; nothing else depends on spelling.
PORTRAIT_EMOTIONS = (
    "happy", "sad", "angry", "surprised", "relaxed",
    "embarrassed", "determined", "worried", "smug", "sleepy",
)
"""

OLD_GATE = """EXPRESSIONS = (
    "happy", "sad", "angry", "surprised", "relaxed",
    "embarrassed", "determined", "worried", "smug", "sleepy",
"""


def resolve_target(env_var: str, file_name: str) -> Path:
    """Resolve one patch target, or stop naming what is missing."""
    override = os.environ.get(env_var)
    candidate = Path(override) if override else Path(__file__).resolve().parent / file_name
    if not candidate.is_file():
        raise SystemExit(
            f"patch_scripts: {file_name} not found (looked at {candidate}); "
            f"set {env_var} to its path"
        )
    return candidate


def replace_block(path: Path, old: str, new: str, label: str) -> str:
    """Return path's text with the old block swapped for new; stop if absent."""
    src = path.read_text(encoding="utf-8")
    if old not in src:
        raise SystemExit(f"patch_scripts: {label} block not found in {path}")
    return src.replace(old, new)


def main() -> None:
    base = resolve_target("LUMEN_BASE_MESH", BASE_MESH_NAME)
    gate = resolve_target("LUMEN_GATE", GATE_NAME)

    base_src = replace_block(base, OLD_BLOCK, NEW_BLOCK, "base")
    if "BASE_VERSION = 1" not in base_src:
        raise SystemExit(f"patch_scripts: BASE_VERSION = 1 not found in {base}")
    base_src = base_src.replace("BASE_VERSION = 1", "BASE_VERSION = 2")
    gate_src = replace_block(gate, OLD_GATE, NEW_GATE, "gate")

    base.write_text(base_src, encoding="utf-8")
    gate.write_text(gate_src, encoding="utf-8")
    print("patched OK")


if __name__ == "__main__":
    main()
