#!/usr/bin/env python3
"""Win32 registry write-surface evidence over a QEMU TCP serial chardev.

Boots the dedicated registry-write ISO (`--registry-write-fixture`,
`debug_exit` kept), waits until `peregw.exe` has written its own summary
line, then asserts the full `advapi32.dll` mutation transcript:
`RegCreateKeyExA` create/open dispositions and slot reuse, `RegSetValueExA`
REG_SZ sets/updates and the 87/6 error values, `RegDeleteValueA` success and
2/6 errors, the read-back `RegQueryValueExA` lines, the phase self-test line
and the boot marker. QEMU must then leave via isa-debug-exit with status 33.

Usage: test_registry_write_qemu.py <qemu.exe> <iso> <session-log> <qemu-log>

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

VERIFIED = b"win32: registry write surface verified"

# Lines that must appear at least once. The size-query line carries its
# terminator (LF expands to CRLF on the serial force path, hence `\r\r\n`) so
# it cannot match a longer full-read line as a prefix.
EXPECTED = [
    b'win32: RegCreateKeyExA(0x80000002, "Session/Current") -> 0 (handle 4096, created)',
    b'win32: RegCreateKeyExA(0x80000002, "Session/Current") -> 0 (handle 4097, opened existing)',
    b'win32: RegCreateKeyExA(0x80000002, "Session/Current") -> 0 (handle 4097, created)',
    b'win32: RegSetValueExA(4096, "Flag") -> 0 (REG_SZ, 7 bytes)',
    b'win32: RegSetValueExA(4096, "Flag") -> 0 (REG_SZ, 2 bytes)',
    b'win32: RegSetValueExA(4096, "Number") -> 87 (invalid parameter)',
    b'win32: RegSetValueExA(4096, "Flag") -> 87 (invalid parameter)',
    b'win32: RegSetValueExA(1, "Flag") -> 6 (invalid handle)',
    b'win32: RegSetValueExA(4096, "Bad Name") -> 87 (invalid parameter)',
    b'win32: RegSetValueExA(4096, "") -> 87 (invalid parameter)',
    b'win32: RegSetValueExA(4097, "Persisted") -> 0 (REG_SZ, 3 bytes)',
    b'win32: RegDeleteValueA(4096, "Missing") -> 2 (file not found)',
    b'win32: RegDeleteValueA(1, "Flag") -> 6 (invalid handle)',
    b'win32: RegDeleteValueA(4096, "Flag") -> 0',
    b'win32: RegQueryValueExA(4096, "Flag") -> 0 (REG_SZ, 8 bytes) "enabled"',
    b'win32: RegQueryValueExA(4096, "Flag") -> 0 (REG_SZ, 3 bytes)\r\r\n',
    b'win32: RegQueryValueExA(4096, "Flag") -> 2 (file not found)',
    b"win32: RegCloseKey(4097) -> 0",
    b"win32: RegCloseKey(4096) -> 0",
    b"win32: WriteFile(1, 39 bytes) -> 1",
    VERIFIED,
    b"win32 registry write selftest: passed (",
]

# Line -> exact required count. Only the second handle (4097) is closed
# twice: at step 2 (the second create) and at step 17 (the final cleanup).
REPEATED = [
    (b"win32: RegCloseKey(4097) -> 0", 2),
]

EXIT_DEADLINE = 10.0


def main():
    if len(sys.argv) != 5:
        sys.stderr.write(
            "usage: test_registry_write_qemu.py <qemu> <iso> <session-log> <qemu-log>\n"
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
        return fail_and_finish("registry write fixture summary line not seen within %ds" % BOOT_TIMEOUT)
    if FATAL in session.data:
        return fail_and_finish("kernel reported a fatal error before the registry write summary")

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
        "serial asserts: RegCreateKeyExA dispositions/slot reuse, "
        "RegSetValueExA sets/updates/87/6, RegDeleteValueA 0/2/6, "
        "RegQueryValueExA read-back, registry write selftest passed, "
        "marker reached\n"
    )
    session.finish(0)
    return 0


if __name__ == "__main__":
    sys.exit(main())
