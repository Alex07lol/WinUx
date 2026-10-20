#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=lib.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"

# Win32 registry enumeration/deletion evidence
# (docs/WIN32_REGISTRY_ENUM.md §15): boot the dedicated registry-enum ISO and
# assert the full advapi32.dll enumeration/deletion transcript, the phase
# self-test line, the boot marker and the debug-exit status.

REGISTRY_ENUM_ISO="$ROOT_DIR/artifacts/project-os-registry-enum.iso"
SESSION_LOG="$ROOT_DIR/artifacts/qemu-registry-enum-session.log"
QEMU_LOG="$ROOT_DIR/artifacts/qemu-registry-enum.log"

[ -f "$KERNEL_ARTIFACT" ] || die "kernel artifact missing; run scripts/build.sh first"
have python || skip_exit "python unavailable; Win32 registry enum test not run"

log "building registry-enum ISO (peregen fixture, debug_exit kept)"
"$ROOT_DIR/scripts/make_iso.sh" --no-build --registry-enum-fixture

if ! QEMU=$(find_qemu); then
    skip_exit "QEMU unavailable; Win32 registry enum test not run"
fi

python "$ROOT_DIR/scripts/test_registry_enum_qemu.py" \
    "$(winpath "$QEMU")" \
    "$(winpath "$REGISTRY_ENUM_ISO")" \
    "$(winpath "$SESSION_LOG")" \
    "$(winpath "$QEMU_LOG")"

log "PASS: Win32 registry enumeration/deletion verified (advapi32.dll imports, canonical-order children/values, size queries, 234/259/5/2/6 errors, stale handles, persisted cleanup)"
log "session log: $SESSION_LOG"
