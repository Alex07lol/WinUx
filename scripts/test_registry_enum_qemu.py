#!/usr/bin/env python3
"""Win32 registry enumeration/deletion evidence over a QEMU TCP serial chardev.

Boots the dedicated registry-enum ISO (`--registry-enum-fixture`,
`debug_exit` kept), waits until `peregen.exe` has written its own summary
line, then asserts the full `advapi32.dll` transcript: `RegEnumKeyExA` and
`RegEnumValueA` insertion-order names/values, size queries, 234/259 errors,
`RegDeleteKeyA` 0/2/5/6 outcomes (including the child-keys access-denied
rule), the stale-handle result, the phase self-test line and the boot
marker. QEMU must then leave via isa-debug-exit with status 33.

Usage: test_registry_enum_qemu.py <qemu.exe> <iso> <session-log> <qemu-log>

Exit status: 0 on pass, 1 on any failed assertion (the transcript tail is
printed and the full transcript is written to the session-log path).
"""

import sys
import time

from qemu_serial import (
    BOOT_TIMEOUT,
    FATAL,
    MARKER,
    QemuSerialSession,
    fail,
)

VERIFIED = b"win32: registry enumeration surface verified"

# Lines that must appear at least once. The kernel prints one line per
# service call, in fixture order.
EXPECTED = [
    b'win32: RegCreateKeyExA(0x80000002, "Session/Current") -> 0 (handle 4096, created)',
    b'win32: RegCreateKeyExA(0x80000002, "Session/Current/Enum/Alpha") -> 0 (handle 4097, created)',
    b'win32: RegCreateKeyExA(0x80000002, "Session/Current/Enum/Beta") -> 0 (handle 4098, created)',
    b'win32: RegCreateKeyExA(0x80000002, "Session/Current/Enum") -> 0 (handle 4099, opened existing)',
    b'win32: RegSetValueExA(4096, "Keep") -> 0 (REG_SZ, 3 bytes)',
    b'win32: RegSetValueExA(4096, "Removed") -> 0 (REG_SZ, 3 bytes)',
    b'win32: RegSetValueExA(4097, "Item") -> 0 (REG_SZ, 5 bytes)',
    b'win32: RegSetValueExA(4098, "Item") -> 0 (REG_SZ, 4 bytes)',
    b'win32: RegQueryValueExA(4096, "Keep") -> 0 (REG_SZ, 4 bytes) "one"',
    b'win32: RegEnumKeyExA(4099, 0) -> 0 ("Alpha")',
    b'win32: RegEnumKeyExA(4099, 1) -> 0 ("Beta")',
    b"win32: RegEnumKeyExA(4099, 2) -> 259 (no more items)",
    b"win32: RegEnumKeyExA(4099, 0) -> 234 (more data, 6 bytes needed)",
    b'win32: RegEnumKeyExA(4099, 0) -> 0 ("Alpha")',
    b"win32: RegEnumKeyExA(4099, 0) -> 87 (invalid parameter)",
    b'win32: RegEnumValueA(4096, 0) -> 0 ("Enum/Alpha/Item", REG_SZ, 6 bytes)',
    b'win32: RegEnumValueA(4096, 1) -> 0 ("Enum/Beta/Item", REG_SZ, 5 bytes)',
    b'win32: RegEnumValueA(4096, 2) -> 0 (size query, "Keep", 4 bytes)',
    b"win32: RegEnumValueA(4096, 4) -> 259 (no more items)",
    b"win32: RegEnumValueA(4097, 0) -> 234 (more data, 5 name bytes needed)",
    b'win32: RegEnumValueA(4097, 0) -> 0 ("Item", REG_SZ, 6 bytes)',
    b'win32: RegDeleteKeyA(4096, "Enum") -> 5 (access denied)',
    b'win32: RegDeleteKeyA(4099, "Alpha") -> 0',
    b'win32: RegDeleteKeyA(4099, "Alpha") -> 2 (file not found)',
    b'win32: RegDeleteKeyA(4099, "Beta") -> 0',
    b'win32: RegDeleteKeyA(4099, "Gamma") -> 2 (file not found)',
    b'win32: RegDeleteKeyA(1, "Item") -> 6 (invalid handle)',
    b'win32: RegDeleteKeyA(4096, "Enum") -> 2 (file not found)',
    b'win32: RegDeleteKeyA(0x80000002, "Session") -> 5 (access denied)',
    b'win32: RegDeleteKeyA(0x80000002, "Session/Current") -> 0',
    b"win32: RegEnumKeyExA(4099, 0) -> 259 (no more items)",
    b"win32: RegCloseKey(4096) -> 0",
    b"win32: RegCloseKey(4097) -> 0",
    b"win32: RegCloseKey(4098) -> 0",
    b"win32: RegCloseKey(4099) -> 0",
    b"win32: WriteFile(1, 45 bytes) -> 1",
    VERIFIED,
    b"win32 registry enum selftest: passed (",
]

# Line -> exact required count. The 87 line appears twice (reserved pointer
# and zero name capacity); the stale enumeration repeats 259 for handle 4099.
REPEATED = [
    (b"win32: RegEnumKeyExA(4099, 0) -> 87 (invalid parameter)", 2),
]

EXIT_DEADLINE = 10.0


def main():
    if len(sys.argv) != 5:
        sys.stderr.write(
            "usage: test_registry_enum_qemu.py <qemu> <iso> <session-log> <qemu-log>\n"
        )
        return 2
    qemu, iso, session_log, qemu_log = sys.argv[1:]

    try:
        session = QemuSerialSession(
            qemu,
            iso,
            session_log,
            qemu_log,
            extra_args=["-device", "isa-debug-exit,iobase=0xf4,iosize=0x04"],
            serial_wait=True,
        )
    except RuntimeError as error:
        sys.stderr.write("FAIL: %s\n" % error)
        return 1

    def fail_and_finish(message):
        return session.finish(fail(message, session.data))

    if not session.wait_for(VERIFIED, BOOT_TIMEOUT):
        return fail_and_finish("registry enum fixture summary line not seen within %ds" % BOOT_TIMEOUT)
    if FATAL in session.data:
        return fail_and_finish("kernel reported a fatal error before the registry enum summary")

    if not session.wait_for(MARKER, BOOT_TIMEOUT):
        return fail_and_finish("boot marker not seen within %ds" % BOOT_TIMEOUT)
    # `wait_for(MARKER)` returns on the prefix; the rest of the marker line is
    # still in flight, so wait for its terminator before asserting its checks.
    marker_start = session.data.find(MARKER)
    if not session.wait_for(b"\r\n", 10, marker_start):
        return fail_and_finish("boot marker line did not terminate")
    if FATAL in session.data:
        return fail_and_finish("kernel reported a fatal error")

    data = bytes(session.data)
    missing = [line for line in EXPECTED if line not in data]
    if missing:
        return fail_and_finish("missing serial lines: %r" % (missing,))
    for line, required in REPEATED:
        count = data.count(line)
        if count != required:
            return fail_and_finish(
                "line %r appears %d times (expected exactly %d)" % (line, count, required)
            )
    if b"win32,vfs" not in data:
        return fail_and_finish("marker does not contain the win32,vfs checks")

    deadline = time.monotonic() + EXIT_DEADLINE
    while session.process.poll() is None and time.monotonic() < deadline:
        time.sleep(0.1)
    status = session.process.poll()
    if status is None:
        return fail_and_finish("QEMU did not exit after the success marker")
    if status != 33:
        return fail_and_finish(
            "unexpected QEMU exit status %s (expected 33 from isa-debug-exit)" % status
        )

    sys.stdout.write(
        "serial asserts: RegEnumKeyExA order/234/259/87, "
        "RegEnumValueA name/type/size/full reads/259, "
        "RegDeleteKeyA 0/2/5/6, stale-handle 259, registry enum selftest "
        "passed, marker reached\n"
    )
    session.finish(0)
    return 0


if __name__ == "__main__":
    sys.exit(main())
