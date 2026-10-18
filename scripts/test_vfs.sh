#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=lib.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"

log "host path parser tests"
cargo test -p os_core path::

if [ ! -f "$ISO_PATH" ]; then
    warn "SKIP: in-VM VFS self-test needs $ISO_PATH (run scripts/make_iso.sh)"
    exit 0
fi
if QEMU=$(find_qemu); then
    QEMU_SYSTEM_X86_64="$QEMU" "$ROOT_DIR/scripts/test_boot_qemu.sh" "$ISO_PATH" --timeout 60
    grep -q "vfs selftest: passed" "$ROOT_DIR/artifacts/qemu-serial.log" \
        || die "kernel did not report the VFS self-test"
    log "PASS: in-VM VFS self-test"
else
    warn "SKIP: QEMU unavailable; in-VM VFS self-test not run"
fi
log "PASS: VFS tests (host and in-VM where available)"
