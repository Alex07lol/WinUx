#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=lib.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"

# The full PE32+ self-test evidence (Phase 7, stage 1): the standard
# non-interactive BIOS boot test runs first (marker + isa-debug-exit status),
# then this script asserts the serial lines that only occur when the real
# rust-lld-built PE fixtures are loaded, base-relocated and executed at CPL3.
SERIAL_LOG="$ROOT_DIR/artifacts/qemu-serial-pe.log"

"$ROOT_DIR/scripts/test_boot_qemu.sh" --serial-log "$SERIAL_LOG"

[ -f "$SERIAL_LOG" ] || die "serial log missing: $SERIAL_LOG"

require_line() {
    grep -aFq "$1" "$SERIAL_LOG" || die "expected line not found in $(basename "$SERIAL_LOG"): $1"
}

require_line "pe: 2 modules present"
require_line "pe: pehello.exe loaded at 0x400000 ("
require_line "pe: hello from a PE32+ image via int 0x80"
require_line "pe: base relocation verified"
require_line "user: /boot/pebad.exe rejected (bad signature)"
require_line "pe selftest: passed (PE32+ module loaded"
require_line "sched,user,elf,pe,vfs"

log "PASS: PE32+ module loaded, base relocation applied, int 0x80 write from CPL3, corrupt module rejected"
