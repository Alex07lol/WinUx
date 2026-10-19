#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=lib.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"

# Win32 registry read-surface evidence (docs/WIN32_REGISTRY.md §12): boot the
# dedicated registry ISO and assert the full advapi32.dll transcript, the
# phase self-test line, the boot marker and the debug-exit status.

REGISTRY_ISO="$ROOT_DIR/artifacts/project-os-registry.iso"
SESSION_LOG="$ROOT_DIR/artifacts/qemu-registry-session.log"
QEMU_LOG="$ROOT_DIR/artifacts/qemu-registry.log"

[ -f "$KERNEL_ARTIFACT" ] || die "kernel artifact missing; run scripts/build.sh first"
have python || skip_exit "python unavailable; Win32 registry test not run"

log "building registry ISO (pereg fixture, debug_exit kept)"
"$ROOT_DIR/scripts/make_iso.sh" --no-build --registry-fixture

if ! QEMU=$(find_qemu); then
    skip_exit "QEMU unavailable; Win32 registry test not run"
fi

python "$ROOT_DIR/scripts/test_registry_qemu.py" \
    "$(winpath "$QEMU")" \
    "$(winpath "$REGISTRY_ISO")" \
    "$(winpath "$SESSION_LOG")" \
    "$(winpath "$QEMU_LOG")"

log "PASS: Win32 registry read surface verified (advapi32.dll imports, handle table, REG_SZ reads)"
log "session log: $SESSION_LOG"
