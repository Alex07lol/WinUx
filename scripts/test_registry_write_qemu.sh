#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=lib.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"

# Win32 registry write-surface evidence (docs/WIN32_REGISTRY_WRITE.md §15):
# boot the dedicated registry-write ISO and assert the full advapi32.dll
# mutation transcript, the phase self-test line, the boot marker and the
# debug-exit status.

REGISTRY_WRITE_ISO="$ROOT_DIR/artifacts/project-os-registry-write.iso"
SESSION_LOG="$ROOT_DIR/artifacts/qemu-registry-write-session.log"
QEMU_LOG="$ROOT_DIR/artifacts/qemu-registry-write.log"

[ -f "$KERNEL_ARTIFACT" ] || die "kernel artifact missing; run scripts/build.sh first"
have python || skip_exit "python unavailable; Win32 registry write test not run"

log "building registry-write ISO (peregw fixture, debug_exit kept)"
"$ROOT_DIR/scripts/make_iso.sh" --no-build --registry-write-fixture

if ! QEMU=$(find_qemu); then
    skip_exit "QEMU unavailable; Win32 registry write test not run"
fi

python "$ROOT_DIR/scripts/test_registry_write_qemu.py" \
    "$(winpath "$QEMU")" \
    "$(winpath "$REGISTRY_WRITE_ISO")" \
    "$(winpath "$SESSION_LOG")" \
    "$(winpath "$QEMU_LOG")"

log "PASS: Win32 registry write surface verified (advapi32.dll imports, create/open dispositions, REG_SZ writes, deletes, persistence)"
log "session log: $SESSION_LOG"
