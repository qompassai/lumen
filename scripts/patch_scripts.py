import sys

BASE = "/home/phaedrus/sprite-mcp/scripts/lumen_base_mesh.py"
GATE = "/home/phaedrus/sprite-mcp/scripts/lumen_gate.py"

NEW_BLOCK = """# Canonical portrait emotions: templates/emotions.json (schema
# lumen.emotion-templates v1, ratified by Matt 2026-10-06). 'fun' is
# PLAYFUL (VRoid/VRM 0.x semantics), not VRM 1.0 'relaxed'. Do not rename
# without bumping the template version and the base version together.
PORTRAIT_EMOTIONS = (
    "happy", "angry", "sad", "surprised", "fun",
    "annoyed", "worried", "embarrassed", "determined", "smug",
)
"""

OLD_BLOCK = """# Provisional names: no authoritative list of the 10 portrait emotions was
# found in light-show or lumen. Rename here; nothing else depends on spelling.
PORTRAIT_EMOTIONS = (
    "happy", "sad", "angry", "surprised", "relaxed",
    "embarrassed", "determined", "worried", "smug", "sleepy",
)
"""

src = open(BASE).read()
assert OLD_BLOCK in src, "base block not found"
src = src.replace(OLD_BLOCK, NEW_BLOCK)
assert "BASE_VERSION = 1" in src
src = src.replace("BASE_VERSION = 1", "BASE_VERSION = 2")
open(BASE, "w").write(src)

OLD_GATE = """EXPRESSIONS = (
    "happy", "sad", "angry", "surprised", "relaxed",
    "embarrassed", "determined", "worried", "smug", "sleepy",
"""
NEW_GATE = """EXPRESSIONS = (
    "happy", "angry", "sad", "surprised", "fun",
    "annoyed", "worried", "embarrassed", "determined", "smug",
"""
src = open(GATE).read()
assert OLD_GATE in src, "gate block not found"
src = src.replace(OLD_GATE, NEW_GATE)
open(GATE, "w").write(src)
print("patched OK")
