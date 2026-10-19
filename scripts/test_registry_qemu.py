#!/usr/bin/env python3
"""Win32 registry read-surface evidence over a QEMU TCP serial chardev.

Boots the dedicated registry ISO (`--registry-fixture`, `debug_exit` kept),
waits until `pereg.exe` has written its own summary line, then asserts the
full `advapi32.dll` transcript: `RegOpenKeyExA` success/6/87/2/4 lines, the
four handles and slot reuse, size/`ERROR_MORE_DATA`/full reads of `Name` and
`Display/Theme` with their exact values, `RegCloseKey` success and
double-close, the phase self-test line and the boot marker. QEMU must then
leave via isa-debug-exit with status 33.

Usage: test_registry_qemu.py <qemu.exe> <iso> <session-log> <qemu-log>

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

VERIFIED = b"win32: registry surface verified"

# Lines that must appear at least once. The size-query line carries its
# terminator (LF expands to CRLF on the serial force path, hence `\r\r\n`) so
# it cannot match the longer full-read line as a prefix.
EXPECTED = [
    b'win32: RegOpenKeyExA(0x1, "System") -> 6 (invalid handle)',
    b'win32: RegOpenKeyExA(0x80000002, "System") -> 87 (invalid parameter)',
    b'win32: RegOpenKeyExA(0x80000002, "Missing") -> 2 (file not found)',
    b'win32: RegOpenKeyExA(0x80000002, "System") -> 0 (handle 4097)',
    b'win32: RegOpenKeyExA(0x80000002, "System") -> 0 (handle 4098)',
    b'win32: RegOpenKeyExA(0x80000002, "System") -> 0 (handle 4099)',
    b'win32: RegOpenKeyExA(0x80000002, "System") -> 4 (too many open keys)',
    b'win32: RegOpenKeyExA(0x80000002, "") -> 0 (handle 0x80000002)',
    b'win32: RegQueryValueExA(4096, "Missing") -> 2 (file not found)',
    b'win32: RegQueryValueExA(4096, "Name") -> 0 (REG_SZ, 11 bytes)\r\r\n',
    b'win32: RegQueryValueExA(4096, "Name") -> 234 (more data, 11 bytes needed)',
    b'win32: RegQueryValueExA(4096, "Name") -> 0 (REG_SZ, 11 bytes) "PROJECT_OS"',
    b'win32: RegQueryValueExA(0x80000002, "Display/Theme") -> 0 (REG_SZ, 8 bytes) "classic"',
    b"win32: RegCloseKey(0x80000002) -> 0",
    b"win32: RegCloseKey(4096) -> 6 (invalid handle)",
    b"win32: RegCloseKey(4097) -> 0",
    b"win32: RegCloseKey(4098) -> 0",
    b"win32: RegCloseKey(4099) -> 0",
    b"win32: WriteFile(1, 33 bytes) -> 1",
    VERIFIED,
    b"win32 registry selftest: passed (",
]

# Line -> exact required count, for the calls the fixture makes twice.
REPEATED = [
    (b'win32: RegOpenKeyExA(0x80000002, "System") -> 0 (handle 4096)', 2),
    (b'win32: RegQueryValueExA(4096, "Name") -> 87 (invalid parameter)', 2),
    (b"win32: RegCloseKey(4096) -> 0", 2),
]

EXIT_DEADLINE = 10.0


def main():
    if len(sys.argv) != 5:
        sys.stderr.write("usage: test_registry_qemu.py <qemu> <iso> <session-log> <qemu-log>\n")
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
        return fail_and_finish("registry fixture summary line not seen within %ds" % BOOT_TIMEOUT)
    if FATAL in session.data:
        return fail_and_finish("kernel reported a fatal error before the registry summary")

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
        "serial asserts: RegOpenKeyExA handle/error lines, RegQueryValueExA "
        "size/MORE_DATA/REG_SZ reads, RegCloseKey double-close and slot "
        "reuse, registry selftest passed, marker reached\n"
    )
    session.finish(0)
    return 0


if __name__ == "__main__":
    sys.exit(main())
