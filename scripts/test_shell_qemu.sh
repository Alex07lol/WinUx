#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=lib.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"

# Boots an ISO whose cmdline has no `debug_exit` (so the kernel reaches the
# interactive shell instead of terminating at the success marker) and drives a
# scripted session over a paced TCP serial chardev. See test_shell_qemu.py for
# why the session is not piped through `-serial stdio`.

INTERACTIVE_ISO="$ROOT_DIR/artifacts/project-os-interactive.iso"
SESSION_LOG="$ROOT_DIR/artifacts/qemu-shell-session.log"
QEMU_LOG="$ROOT_DIR/artifacts/qemu-shell.log"

[ -f "$KERNEL_ARTIFACT" ] || die "kernel artifact missing; run scripts/build.sh first"
have python || skip_exit "python unavailable; interactive shell test not run"

log "building interactive ISO (debug_exit stripped from the cmdline)"
"$ROOT_DIR/scripts/make_iso.sh" --no-build --interactive

if ! QEMU=$(find_qemu); then
    skip_exit "QEMU unavailable; interactive shell test not run"
fi

python "$ROOT_DIR/scripts/test_shell_qemu.py" \
    "$(winpath "$QEMU")" \
    "$(winpath "$INTERACTIVE_ISO")" \
    "$(winpath "$SESSION_LOG")" \
    "$(winpath "$QEMU_LOG")"

log "PASS: interactive shell session over serial"
log "session log: $SESSION_LOG"
