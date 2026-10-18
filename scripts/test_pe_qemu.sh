#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=lib.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"

# The full PE32+ self-test evidence (Phase 7, stages 1-2): the standard
# non-interactive BIOS boot test runs first (marker + isa-debug-exit status),
# then this script asserts the serial lines that only occur when the real
# rust-lld-built PE fixtures are loaded, base-relocated and executed at CPL3,
# and when the native Win32 console surface (real import table, IAT patched to
# the kernel's own int 0x80 stubs) runs end to end.
SERIAL_LOG="$ROOT_DIR/artifacts/qemu-serial-pe.log"

"$ROOT_DIR/scripts/test_boot_qemu.sh" --serial-log "$SERIAL_LOG"

[ -f "$SERIAL_LOG" ] || die "serial log missing: $SERIAL_LOG"

require_line() {
    grep -aFq "$1" "$SERIAL_LOG" || die "expected line not found in $(basename "$SERIAL_LOG"): $1"
}

require_line "pe: 5 modules present"
require_line "pe: pehello.exe loaded at 0x400000 ("
require_line "pe: hello from a PE32+ image via int 0x80"
require_line "pe: base relocation verified"
require_line "user: /boot/pebad.exe rejected (bad signature)"
require_line "pe: peconsole.exe loaded at 0x400000 (entry 0x401000, 5 sections, 1 relocation applied)"
require_line "user: /boot/pebadimp.exe rejected (unknown import dll)"
require_line "user: /boot/pebadfn.exe rejected (unknown import function)"
require_line "win32: GetStdHandle(-11) -> 1"
require_line "win32: hello from the Win32 console surface"
require_line "win32: WriteConsoleA(1, 44 bytes) -> 1"
require_line "win32: console surface verified"
require_line "win32: WriteConsoleA(1, 32 bytes) -> 1"
require_line "win32: ExitProcess(0)"
require_line "win32 selftest: passed (kernel32.dll imports resolved, WriteConsoleA wrote to the console fd, ExitProcess ended the thread)"
require_line "pe selftest: passed (PE32+ module loaded"
require_line "sched,user,elf,pe,win32,vfs"

log "PASS: PE32+ modules loaded, base relocation applied, int 0x80 write from CPL3, corrupt modules rejected, kernel32.dll imports resolved via the native Win32 console surface"
