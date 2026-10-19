#!/usr/bin/env python3
"""Win32 console-input evidence over a QEMU TCP serial chardev.

Boots the dedicated input ISO (console-input fixture, `debug_exit` kept), waits
until the fixture is blocked in `ReadFile` on the console input handle, types
one known line, and asserts the kernel echo, the fixture playback, the byte
count line, the self-test line and the boot marker. QEMU must then leave via
isa-debug-exit with status 33.

Usage: test_input_qemu.py <qemu.exe> <iso> <session-log> <qemu-log>

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

WAITING = b"win32: ReadFile(2, 64 bytes) waiting for input"
TYPED = b"hello, win32"
# The buffer holds `TYPED + CRLF`; serial output expands every LF to CRLF
# (`kernel/src/serial.rs` force paths), so the transcript shows `CR CR LF`.
ECHO_AND_PLAYBACK = TYPED + b"\r\r\n"
EXPECTED = [
    b"win32: GetStdHandle(-10) -> 2",
    b"win32: ReadFile(1) -> 0 (invalid handle)",
    b"win32: ReadFile(2, 16 bytes, overlapped) -> 0 (invalid parameter)",
    WAITING,
    b"win32: ReadFile(2, 64 bytes) -> 1 (14 bytes)",
    b"win32: input surface verified",
    b"win32 input selftest: passed (ReadFile line input",
]

EXIT_DEADLINE = 10.0


def main():
    if len(sys.argv) != 5:
        sys.stderr.write("usage: test_input_qemu.py <qemu> <iso> <session-log> <qemu-log>\n")
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

    if not session.wait_for(WAITING, BOOT_TIMEOUT):
        return fail_and_finish("ReadFile wait line not seen within %ds" % BOOT_TIMEOUT)
    if FATAL in session.data:
        return fail_and_finish("kernel reported a fatal error before the read")

    session.send_paced(TYPED.decode())

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
    if data.count(ECHO_AND_PLAYBACK) < 2:
        return fail_and_finish(
            "expected the typed line both echoed and played back (found %d)"
            % data.count(ECHO_AND_PLAYBACK)
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
        "serial asserts: GetStdHandle(-10) -> 2, echoed line, playback, "
        "ReadFile count 14, input selftest passed, marker reached\n"
    )
    session.finish(0)
    return 0


if __name__ == "__main__":
    sys.exit(main())
