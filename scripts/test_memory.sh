#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=lib.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"

log "host allocator tests"
cargo test -p os_core frame::

if [ ! -f "$ISO_PATH" ]; then
    warn "SKIP: in-VM allocator test needs $ISO_PATH (run scripts/make_iso.sh)"
    exit 0
fi
if QEMU=$(find_qemu); then
    QEMU_SYSTEM_X86_64="$QEMU" "$ROOT_DIR/scripts/test_boot_qemu.sh" "$ISO_PATH" --timeout 60
    grep -q "frame allocator selftest: passed" "$ROOT_DIR/artifacts/qemu-serial.log" \
        || die "kernel did not report the allocator self-test"
    log "PASS: in-VM frame allocator self-test"
else
    warn "SKIP: QEMU unavailable; in-VM allocator self-test not run"
fi
log "PASS: memory tests (host and in-VM where available)"
