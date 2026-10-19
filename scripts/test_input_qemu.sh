#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=lib.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"

# Win32 console-input evidence (docs/WIN32_INPUT.md §6): boot the dedicated
# input ISO, type one line while the fixture is blocked in ReadFile, and
# assert the echo, playback, byte count, selftest line, marker and debug-exit.

INPUT_ISO="$ROOT_DIR/artifacts/project-os-input.iso"
SESSION_LOG="$ROOT_DIR/artifacts/qemu-input-session.log"
QEMU_LOG="$ROOT_DIR/artifacts/qemu-input.log"

[ -f "$KERNEL_ARTIFACT" ] || die "kernel artifact missing; run scripts/build.sh first"
have python || skip_exit "python unavailable; Win32 input test not run"

log "building input ISO (peinput fixture, debug_exit kept)"
"$ROOT_DIR/scripts/make_iso.sh" --no-build --input-fixture

if ! QEMU=$(find_qemu); then
    skip_exit "QEMU unavailable; Win32 input test not run"
fi

python "$ROOT_DIR/scripts/test_input_qemu.py" \
    "$(winpath "$QEMU")" \
    "$(winpath "$INPUT_ISO")" \
    "$(winpath "$SESSION_LOG")" \
    "$(winpath "$QEMU_LOG")"

log "PASS: Win32 console input verified (blocking ReadFile, echo, byte count)"
log "session log: $SESSION_LOG"
