#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=lib.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"

# The full ELF self-test evidence: the standard non-interactive BIOS boot test
# runs first (marker + isa-debug-exit status), then this script asserts the
# stage-3 serial lines that only occur when ELF modules are really loaded and
# executed at CPL3.
SERIAL_LOG="$ROOT_DIR/artifacts/qemu-serial-elf.log"

"$ROOT_DIR/scripts/test_boot_qemu.sh" --serial-log "$SERIAL_LOG"

[ -f "$SERIAL_LOG" ] || die "serial log missing: $SERIAL_LOG"

require_line() {
    grep -aFq "$1" "$SERIAL_LOG" || die "expected line not found in $(basename "$SERIAL_LOG"): $1"
}

require_line "elf: 3 modules present"
require_line "user: hello from ring 3 via int 0x80"
require_line "syscall: thread exits with code 0"
require_line "user fault isolated: vector=14"
require_line "user: /boot/bad.elf rejected (bad magic)"
require_line "user selftest: passed (ELF64 modules loaded"
require_line "sched,user,elf,vfs"

log "PASS: ELF64 modules loaded, executed at CPL3, fault isolated, corrupt module rejected"
