#!/usr/bin/env bash
# Live interop gate for lumen: real Aseprite + real Bevy 0.18 BRP.
#
# Runs the #[ignore]d live tests in tests/interop.rs. Fails closed, with a
# message, when a prerequisite is missing.
#
# BRP prerequisite:
# - Nothing listening on 127.0.0.1:15702: builds bevy_mcp's example_app,
#   starts it on a private Xvfb display (no window on the desktop), waits
#   until BRP answers, runs the tests, then kills the whole process group it
#   started (xvfb-run, Xvfb, example_app) and checks that none survived.
# - Something already listening: it must answer rpc.discover like Bevy (an
#   OpenRPC document listing world.query). Then it is reused and left
#   running, because this script did not start it. A foreign listener fails.
#
# Audio: example_app uses DefaultPlugins, which opens an audio output device,
# but the app spawns no sound sources. This script changes no audio or volume
# settings.
#
# Rendering under Xvfb needs a Vulkan or GL driver that can present to X11.
# If the app dies at startup, the log tail is printed; retrying with
# WGPU_BACKEND=gl (Mesa llvmpipe) is the usual fix.
set -euo pipefail

LUMEN_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
readonly LUMEN_DIR
readonly ASEPRITE_BIN=/usr/bin/aseprite
readonly EXAMPLE_MANIFEST=/home/phaedrus/bevy_mcp/example_app/Cargo.toml
readonly BRP_PORT=15702
readonly BRP_URL="http://127.0.0.1:${BRP_PORT}"
readonly BRP_READY_TRIES=120 # one per second
readonly STOP_TRIES=20       # x 0.25 s after SIGTERM, then SIGKILL

app_pgid=""
app_log=""

die() {
    printf 'interop_check: FAIL: %s\n' "$*" >&2
    exit 1
}

note() {
    printf 'interop_check: %s\n' "$*"
}

stop_app() {
    [[ -n "$app_pgid" ]] || return 0
    kill -TERM -- "-${app_pgid}" 2>/dev/null || true
    for _ in $(seq 1 "$STOP_TRIES"); do
        pgrep -g "$app_pgid" >/dev/null || break
        sleep 0.25
    done
    kill -KILL -- "-${app_pgid}" 2>/dev/null || true
    sleep 0.25
    if pgrep -g "$app_pgid" >/dev/null; then
        note "WARNING: processes survive in group ${app_pgid}:"
        pgrep -a -g "$app_pgid" >&2 || true
    else
        note "example_app process group ${app_pgid} stopped; none survive"
    fi
    app_pgid=""
}
trap stop_app EXIT
trap 'exit 130' INT TERM

# Succeeds only if the endpoint returns Bevy's OpenRPC document.
brp_is_bevy() {
    local reply
    reply="$(curl -sS --max-time 3 -H 'content-type: application/json' \
        -d '{"jsonrpc":"2.0","id":1,"method":"rpc.discover"}' "$BRP_URL" 2>/dev/null)" ||
        return 1
    grep -q '"openrpc"' <<<"$reply" && grep -q '"world.query"' <<<"$reply"
}

port_in_use() {
    [[ -n "$(ss -Hltn "sport = :${BRP_PORT}")" ]]
}

require_tools() {
    local tool
    for tool in cargo curl ss setsid pgrep xvfb-run; do
        command -v "$tool" >/dev/null || die "required tool not found: ${tool}"
    done
    [[ -x "$ASEPRITE_BIN" ]] || die "Aseprite not found at ${ASEPRITE_BIN}"
    [[ -f "$EXAMPLE_MANIFEST" ]] || die "bevy_mcp example_app not found: ${EXAMPLE_MANIFEST}"
}

start_app() {
    note "building example_app (build only; sources untouched)"
    local bin
    bin="$(cargo build --manifest-path "$EXAMPLE_MANIFEST" --message-format=json -q |
        grep -o '"executable":"[^"]*"' | tail -n 1 | cut -d'"' -f4)"
    [[ -x "$bin" ]] || die "example_app build produced no executable"
    app_log="$(mktemp -t lumen-interop-bevy.XXXXXX)"
    # setsid: the app tree gets its own process group, so cleanup can kill
    # xvfb-run, Xvfb and the app together. Unsetting WAYLAND_DISPLAY keeps
    # winit on the private X display instead of the desktop compositor.
    setsid env -u WAYLAND_DISPLAY xvfb-run -a -s "-screen 0 1280x720x24" "$bin" \
        >"$app_log" 2>&1 &
    app_pgid=$!
    note "started example_app under Xvfb: pgid ${app_pgid}, log ${app_log}"
    local try
    for try in $(seq 1 "$BRP_READY_TRIES"); do
        if ! kill -0 "$app_pgid" 2>/dev/null; then
            tail -n 40 "$app_log" >&2 || true
            die "example_app exited during startup (see log above)"
        fi
        if brp_is_bevy; then
            note "BRP ready at ${BRP_URL} after ${try}s"
            return 0
        fi
        sleep 1
    done
    tail -n 40 "$app_log" >&2 || true
    die "BRP did not come up at ${BRP_URL} within ${BRP_READY_TRIES}s"
}

main() {
    require_tools
    note "aseprite: $("$ASEPRITE_BIN" --version 2>&1 | head -n 1)"
    if port_in_use; then
        brp_is_bevy || die "port ${BRP_PORT} is taken by something that is not Bevy BRP"
        note "reusing the Bevy BRP app already on ${BRP_URL}; it will be left running"
    else
        start_app
    fi
    if [[ -n "${LUMEN_ALLOW_REMOTE_BRP:-}" ]]; then
        note "unsetting LUMEN_ALLOW_REMOTE_BRP for the run (the gate asserts loopback-only)"
    fi
    env -u LUMEN_ALLOW_REMOTE_BRP cargo test --manifest-path "${LUMEN_DIR}/Cargo.toml" \
        --test interop -- --ignored --test-threads=1
    note "PASS: live interop tests"
}

main "$@"
