#!/usr/bin/env python3
"""Framebuffer console evidence over a QEMU TCP serial chardev.

Boots the interactive ISO (no `debug_exit`), asserts the pinned `fbcon:` lines
on serial, drives one shell command, then asks the QEMU monitor for a PPM
screendump. The PPM is validated in a second step by `check_screendump.py`.

Usage: test_console_qemu.py <qemu.exe> <iso> <session-log> <qemu-log> <screendump.ppm>

Exit status: 0 on pass, 1 on any failed assertion (the transcript tail is
printed and the full transcript is written to the session-log path).
"""

import re
import socket
import sys

from qemu_serial import (
    BOOT_TIMEOUT,
    COMMAND_TIMEOUT,
    FATAL,
    MARKER,
    QemuSerialSession,
    fail,
)

ENABLED = re.compile(rb"fbcon: enabled (\d+)x(\d+) cells at scale (\d+)")
SELFTEST_PASSED = b"fbcon selftest: passed"


def free_port():
    probe = socket.socket()
    probe.bind(("127.0.0.1", 0))
    port = probe.getsockname()[1]
    probe.close()
    return port


def main():
    if len(sys.argv) != 6:
        sys.stderr.write(
            "usage: test_console_qemu.py <qemu> <iso> <session-log> <qemu-log> <screendump>\n"
        )
        return 2
    qemu, iso, session_log, qemu_log, screendump = sys.argv[1:]

    try:
        session = QemuSerialSession(
            qemu,
            iso,
            session_log,
            qemu_log,
            monitor_port=free_port(),
            serial_wait=True,
        )
    except RuntimeError as error:
        sys.stderr.write("FAIL: %s\n" % error)
        return 1

    def fail_and_finish(message):
        return session.finish(fail(message, session.data))

    if not session.wait_for(MARKER, BOOT_TIMEOUT):
        return fail_and_finish("boot marker not seen within %ds" % BOOT_TIMEOUT)
    if FATAL in session.data:
        return fail_and_finish("kernel reported a fatal error")
    enabled = ENABLED.search(bytes(session.data))
    if enabled is None:
        return fail_and_finish(
            "'fbcon: enabled' line missing (framebuffer console did not activate)"
        )
    if SELFTEST_PASSED not in session.data:
        return fail_and_finish("'fbcon selftest: passed' line missing")

    error = session.wait_for_shell(BOOT_TIMEOUT, COMMAND_TIMEOUT)
    if error is not None:
        return fail_and_finish(error)
    ok, _segment, missing = session.send_command(
        "version", [b"PROJECT_OS", b"syscall gate"]
    )
    if not ok:
        return fail_and_finish("'version' response missing tokens: %r" % (missing,))

    try:
        session.screendump(screendump)
    except RuntimeError as error:
        return fail_and_finish("screendump failed: %s" % error)

    sys.stdout.write(
        "serial asserts: fbcon enabled %sx%s cells at scale %s, selftest passed\n"
        % (
            enabled.group(1).decode(),
            enabled.group(2).decode(),
            enabled.group(3).decode(),
        )
    )
    session.finish(0)
    return 0


if __name__ == "__main__":
    sys.exit(main())
