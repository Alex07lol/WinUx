#!/usr/bin/env python3
"""Scripted PROJECT_OS shell session over a QEMU TCP serial chardev.

See `qemu_serial.py` for why the session is driven over a paced TCP chardev
instead of `-serial stdio`.

Usage: test_shell_qemu.py <qemu.exe> <iso> <session-log> <qemu-log>

Exit status: 0 on pass, 1 on any failed assertion (the transcript tail is
printed and the full transcript is written to the session-log path).
"""

import sys

from qemu_serial import (
    BOOT_TIMEOUT,
    COMMAND_TIMEOUT,
    FATAL,
    MARKER,
    PROMPT,
    QemuSerialSession,
    fail,
)

# (command line, required substrings in the response segment)
COMMANDS = [
    ("help", [b"commands:", b"console"]),
    ("version", [b"PROJECT_OS 0.1.0", b"checkpoint image"]),
    ("ls /", [b"2 entries in /:", b"etc/", b"tmp/"]),
    ("cat /etc/motd", [b"Welcome to PROJECT_OS."]),
    ("config", [b"3 entries", b"/etc/config"]),
    ("config get System/Name", [b"System/Name = PROJECT_OS"]),
    ("config set Display/Theme light", [b"config set: ok"]),
    ("config get Display/Theme", [b"Display/Theme = light"]),
    (
        "config list",
        [
            b"3 entries in config:",
            b"Display/Theme = light",
            b"System/Name = PROJECT_OS",
        ],
    ),
    ("config del Display/Theme", [b"config del: ok"]),
    ("config get Display/Theme", [b"error: no such key"]),
    ("config set bad//key x", [b"error: bad key"]),
    ("write /tmp/note.txt hello", [b"write: ok"]),
    ("append /tmp/note.txt  world", [b"append: ok"]),
    ("cat /tmp/note.txt", [b"hello world"]),
    ("stat /tmp/note.txt", [b"/tmp/note.txt: file (11 B)"]),
    ("ls /tmp", [b"entries in /tmp:", b"note.txt (11 B)"]),
    ("rm /tmp/note.txt", [b"rm: ok"]),
    ("cat /tmp/note.txt", [b"no such file or directory"]),
    ("nosuchcommand", [b"unknown command: nosuchcommand (try 'help')"]),
    ("mem", [b"frames:", b"largest run"]),
    ("heap", [b"heap validate: ok"]),
    ("threads", [b"threads:"]),
    ("console", [b"rx ring", b"0 overruns"]),
    ("clear", [b"\x1b[2J\x1b[H"]),
]


def main():
    if len(sys.argv) != 5:
        sys.stderr.write("usage: test_shell_qemu.py <qemu> <iso> <session-log> <qemu-log>\n")
        return 2
    qemu, iso, session_log, qemu_log = sys.argv[1:]

    try:
        session = QemuSerialSession(qemu, iso, session_log, qemu_log)
    except RuntimeError as error:
        sys.stderr.write("FAIL: %s\n" % error)
        return 1

    error = session.wait_for_shell(BOOT_TIMEOUT, COMMAND_TIMEOUT)
    if error is not None:
        return session.finish(fail(error, session.data))
    if session.data.find(MARKER) == -1:
        return session.finish(fail("boot success marker missing from session output", session.data))

    for line, expected in COMMANDS:
        ok, _, missing = session.send_command(line, expected)
        if not ok:
            if missing[0] == b"<prompt>":
                return session.finish(fail("no prompt after command: %r" % line, session.data))
            return session.finish(
                fail(
                    "command %r: missing %r in response"
                    % (line, missing[0].decode(errors="replace")),
                    session.data,
                )
            )

    if FATAL in session.data:
        return session.finish(fail("kernel reported PROJECT_OS_FATAL", session.data))

    print("session: %d commands, %d bytes received, 0 failures" % (len(COMMANDS), len(session.data)))
    return session.finish(0)


if __name__ == "__main__":
    sys.exit(main())
