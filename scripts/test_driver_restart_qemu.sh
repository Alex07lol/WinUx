#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=lib.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"

# Boots the interactive ISO on q35 with a pinned e1000 and exercises the
# Phase 9 stage-4 stop/restart path: frames freed, ledger claims released,
# network inactive, restart reclaims the same 19 frames, ping works again.
# Stop/restart is deliberately not part of the boot selftest, so this test
# drives the shell over a paced TCP serial session.
INTERACTIVE_ISO="$ROOT_DIR/artifacts/project-os-interactive.iso"
QEMU_LOG="$ROOT_DIR/artifacts/qemu-driver-restart.log"

[ -f "$KERNEL_ARTIFACT" ] || die "kernel artifact missing; run scripts/build.sh first"
have python || skip_exit "python unavailable; driver restart test not run"

log "building interactive ISO (debug_exit stripped from the cmdline)"
"$ROOT_DIR/scripts/make_iso.sh" --no-build --interactive

if ! QEMU=$(find_qemu); then
    skip_exit "QEMU unavailable; driver restart test not run"
fi

python "$ROOT_DIR/scripts/test_driver_restart_qemu.py" \
    "$(winpath "$QEMU")" \
    "$(winpath "$INTERACTIVE_ISO")" \
    "$(winpath "$QEMU_LOG")"

log "PASS: driver stop/restart lifecycle (frames freed, claims released, ping after restart)"
log "session log: $ROOT_DIR/artifacts/qemu-driver-restart-session.log"
