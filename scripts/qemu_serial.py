#!/usr/bin/env python3
"""Shared QEMU TCP-serial session driver for scripted PROJECT_OS tests.

Why TCP and not `-serial stdio`: QEMU's 16550 model inserts bytes from an
unthrottled host chardev straight into the 16-byte hardware FIFO. A piped
stdio burst can fill (and overrun) that FIFO before the guest vCPU even wakes
from `hlt`, which drops bytes regardless of guest correctness. Driving a
session over TCP lets callers deliver bytes at a paced, UART-realistic rate
(see BYTE_DELAY), which is what a real serial line (or a human at a terminal)
produces.

Used by `test_shell_qemu.py` and `test_persistence_qemu.py`.
"""

import socket
import subprocess
import sys
import time

BYTE_DELAY = 0.002
BOOT_TIMEOUT = 60.0
COMMAND_TIMEOUT = 15.0
CONNECT_TIMEOUT = 10.0

PROMPT = b"project-os> "
BANNER = b"PROJECT_OS shell. Type 'help' for commands."
MARKER = b"PROJECT_OS_BOOT_SUCCESS:"
FATAL = b"PROJECT_OS_FATAL"


def fail(message, transcript):
    """Print a failure and the transcript tail; returns the exit status 1."""
    sys.stderr.write("FAIL: %s\n" % message)
    tail = bytes(transcript)[-2000:].decode(errors="replace")
    sys.stderr.write("--- session transcript tail ---\n%s\n" % tail)
    return 1


class QemuSerialSession:
    """A running QEMU instance plus its paced TCP serial connection."""

    def __init__(self, qemu, iso, session_log, qemu_log, machine="q35", extra_args=()):
        self.session_log = session_log
        self.data = bytearray()
        self.sock = None

        probe = socket.socket()
        probe.bind(("127.0.0.1", 0))
        port = probe.getsockname()[1]
        probe.close()

        self.qemu_out = open(qemu_log, "wb")
        self.process = subprocess.Popen(
            [
                qemu,
                "-machine", machine,
                "-m", "512M",
                "-smp", "1",
                "-cdrom", iso,
                "-boot", "d",
                "-display", "none",
                "-monitor", "none",
                "-serial", "tcp:127.0.0.1:%d,server=on,wait=off" % port,
                "-no-reboot",
            ]
            + list(extra_args),
            stdout=self.qemu_out,
            stderr=subprocess.STDOUT,
        )

        deadline = time.monotonic() + CONNECT_TIMEOUT
        while self.sock is None:
            try:
                self.sock = socket.create_connection(("127.0.0.1", port), timeout=1.0)
            except OSError:
                if time.monotonic() > deadline:
                    self.finish(1)
                    raise RuntimeError("could not connect to the QEMU serial port")
                time.sleep(0.1)

    def read_until(self, marker, start, deadline):
        """Read until `marker` appears in the transcript; returns index or -1."""
        while True:
            index = self.data.find(marker, start)
            if index != -1:
                return index
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                return -1
            self.sock.settimeout(min(0.2, remaining))
            try:
                chunk = self.sock.recv(4096)
            except socket.timeout:
                continue
            except OSError:
                return -1
            if chunk:
                self.data.extend(chunk)
            else:
                time.sleep(0.05)

    def wait_for(self, marker, timeout, start=0):
        return self.read_until(marker, start, time.monotonic() + timeout) != -1

    def send_paced(self, line):
        for byte in line.encode() + b"\n":
            self.sock.sendall(bytes([byte]))
            time.sleep(BYTE_DELAY)

    def send_command(self, line, tokens, timeout=COMMAND_TIMEOUT):
        """Send one shell line and wait for the prompt.

        Returns `(ok, segment, missing)`: the response segment after the
        command, and the required tokens that were not present in it. A
        missing prompt is reported as the token `b"<prompt>"`.
        """
        start = len(self.data)
        self.send_paced(line)
        if not self.wait_for(PROMPT, timeout, start):
            return False, bytes(self.data[start:]), [b"<prompt>"]
        segment = bytes(self.data[start:])
        missing = [token for token in tokens if token not in segment]
        return not missing, segment, missing

    def wait_for_shell(self, boot_timeout=BOOT_TIMEOUT, command_timeout=COMMAND_TIMEOUT):
        """Wait for the shell banner and first prompt.

        Returns an error message string on failure, or None on success. The
        boot-success marker is checked by the caller (it is part of the
        transcript, not a synchronization point).
        """
        if not self.wait_for(BANNER, boot_timeout):
            return "shell banner not seen within %ds" % boot_timeout
        if not self.wait_for(PROMPT, command_timeout):
            return "first prompt not seen"
        return None

    def finish(self, status):
        """Write the transcript, stop QEMU and close everything."""
        with open(self.session_log, "wb") as transcript:
            transcript.write(bytes(self.data))
        self.qemu_out.close()
        try:
            self.process.kill()
            self.process.wait(timeout=10)
        except OSError:
            pass
        if self.sock is not None:
            self.sock.close()
            self.sock = None
        return status
