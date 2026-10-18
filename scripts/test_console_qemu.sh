#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=lib.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"

# Framebuffer log console evidence (docs/FRAMEBUFFER_CONSOLE.md §9): boot the
# interactive ISO, assert the pinned `fbcon:` serial lines, drive one shell
# command, take a QEMU monitor screendump and validate the PPM against the
# grid geometry from the transcript.

INTERACTIVE_ISO="$ROOT_DIR/artifacts/project-os-interactive.iso"
SESSION_LOG="$ROOT_DIR/artifacts/qemu-console-session.log"
QEMU_LOG="$ROOT_DIR/artifacts/qemu-console.log"
SCREENDUMP="$ROOT_DIR/artifacts/console-screendump.ppm"

[ -f "$KERNEL_ARTIFACT" ] || die "kernel artifact missing; run scripts/build.sh first"
have python || skip_exit "python unavailable; framebuffer console test not run"

log "building interactive ISO (debug_exit stripped from the cmdline)"
"$ROOT_DIR/scripts/make_iso.sh" --no-build --interactive

if ! QEMU=$(find_qemu); then
    skip_exit "QEMU unavailable; framebuffer console test not run"
fi

python "$ROOT_DIR/scripts/test_console_qemu.py" \
    "$(winpath "$QEMU")" \
    "$(winpath "$INTERACTIVE_ISO")" \
    "$(winpath "$SESSION_LOG")" \
    "$(winpath "$QEMU_LOG")" \
    "$(winpath "$SCREENDUMP")"

log "validating screendump against the serial transcript"
python "$ROOT_DIR/scripts/check_screendump.py" \
    "$(winpath "$SCREENDUMP")" \
    "$(winpath "$SESSION_LOG")"

log "PASS: framebuffer console mirror verified (serial lines + screendump)"
log "session log: $SESSION_LOG"
log "screendump: $SCREENDUMP"
