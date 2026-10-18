#!/usr/bin/env python3
"""Four-boot durability test for the PROJECT_OS checkpoint VFS.

Boot 1 writes a file to a fresh disk image; boot 2 proves it survived a
reboot and writes more; boot 3 corrupts the newest slot's payload and proves
the kernel falls back to the older checkpoint; boot 4 corrupts both slots and
proves a fresh format. Runs `-machine pc` because the ATA PIO driver needs
the PIIX legacy ports (see docs/RESEARCH.md R-009).

Usage: test_persistence_qemu.py <qemu.exe> <iso> <disk-image> <qemu-log>

Exit status: 0 on pass, 1 on any failed assertion, 2 on usage error.
"""

import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from qemu_serial import (  # noqa: E402
    BOOT_TIMEOUT,
    COMMAND_TIMEOUT,
    FATAL,
    QemuSerialSession,
    fail,
)

SECTOR = 512
SLOT_BYTES = 1024 * 1024
SLOT_A_OFFSET = 16 * SECTOR
SLOT_B_OFFSET = SLOT_A_OFFSET + SLOT_BYTES
DISK_BYTES = 8 * 1024 * 1024

MARKER_SUFFIX = b"vfs,blk,persist"


def make_fresh_disk(path):
    with open(path, "wb") as image:
        image.truncate(DISK_BYTES)


def slot_seq(path, offset):
    """Sequence number of a slot header, or None when the magic is absent."""
    with open(path, "rb") as image:
        image.seek(offset)
        header = image.read(32)
    if header[:4] != b"POFS":
        return None
    return int.from_bytes(header[8:16], "little")


def corrupt_slot_payload(path, offset):
    """Zero the first payload sector, leaving the header intact."""
    with open(path, "r+b") as image:
        image.seek(offset + SECTOR)
        image.write(b"\x00" * SECTOR)


def start_boot(qemu, iso, disk, qemu_log, number):
    session_log = os.path.join(
        os.path.dirname(disk), "qemu-persist-boot%d.log" % number
    )
    return QemuSerialSession(
        qemu,
        iso,
        session_log,
        qemu_log,
        machine="pc",
        extra_args=(
            "-drive",
            "file=%s,format=raw,if=ide,index=0,media=disk" % disk,
        ),
    )


def check_tokens(session, boot, tokens, what):
    missing = [token for token in tokens if token not in session.data]
    if missing:
        return session.finish(
            fail(
                "boot %d: %s: missing %r"
                % (boot, what, missing[0].decode(errors="replace")),
                session.data,
            )
        )
    return 0


def run_command(session, boot, line, tokens):
    ok, _, missing = session.send_command(line, tokens)
    if not ok:
        if missing[0] == b"<prompt>":
            return session.finish(fail("boot %d: no prompt after: %r" % (boot, line), session.data))
        return session.finish(
            fail(
                "boot %d: command %r: missing %r"
                % (boot, line, missing[0].decode(errors="replace")),
                session.data,
            )
        )
    return 0


def begin_boot(qemu, iso, disk, qemu_log, number):
    """Start a boot, wait for the shell and check the success marker."""
    try:
        session = start_boot(qemu, iso, disk, qemu_log, number)
    except RuntimeError as error:
        sys.stderr.write("FAIL: boot %d: %s\n" % (number, error))
        return None, 1
    error = session.wait_for_shell(BOOT_TIMEOUT, COMMAND_TIMEOUT)
    if error is not None:
        return None, session.finish(fail("boot %d: %s" % (number, error), session.data))
    if FATAL in session.data:
        return None, session.finish(
            fail("boot %d: kernel reported PROJECT_OS_FATAL" % number, session.data)
        )
    if MARKER_SUFFIX not in session.data:
        return None, session.finish(
            fail("boot %d: marker with %s missing" % (number, MARKER_SUFFIX.decode()), session.data)
        )
    return session, 0


def main():
    if len(sys.argv) != 5:
        sys.stderr.write(
            "usage: test_persistence_qemu.py <qemu> <iso> <disk-image> <qemu-log>\n"
        )
        return 2
    qemu, iso, disk, qemu_log = sys.argv[1:]

    make_fresh_disk(disk)

    # ---- Boot 1: fresh disk, first format, first save --------------------
    session, status = begin_boot(qemu, iso, disk, qemu_log, 1)
    if session is None:
        return status
    status = check_tokens(
        session,
        1,
        [
            b"blk: found",
            b"16384 sectors",
            b"persist: no valid checkpoint, formatted slot A seq=0",
        ],
        "boot log",
    )
    if status:
        return status
    if b"invalid" in session.data:
        return session.finish(fail("boot 1: unexpected slot-invalid line", session.data))
    for line, tokens in [
        ("blk", [b"QEMU HARDDISK", b"16384 sectors (8 MiB)"]),
        ("persist", [b"active slot A seq 0"]),
        ("write /tmp/hello.txt hello world", [b"write: ok", b"persist: saved slot B seq=1"]),
        ("cat /tmp/hello.txt", [b"hello world"]),
    ]:
        status = run_command(session, 1, line, tokens)
        if status:
            return status
    print("boot 1: fresh format, wrote /tmp/hello.txt, saved slot B seq=1")
    session.finish(0)

    # ---- Boot 2: the file survived; write a second generation ------------
    session, status = begin_boot(qemu, iso, disk, qemu_log, 2)
    if session is None:
        return status
    status = check_tokens(session, 2, [b"persist: loaded slot B seq=1"], "boot log")
    if status:
        return status
    for line, tokens in [
        ("cat /tmp/hello.txt", [b"hello world"]),
        ("mkdir /tmp/notes", [b"persist: saved slot A seq=2"]),
        ("write /tmp/notes/a.txt alpha", [b"persist: saved slot B seq=3"]),
        ("cat /tmp/notes/a.txt", [b"alpha"]),
    ]:
        status = run_command(session, 2, line, tokens)
        if status:
            return status
    print("boot 2: checkpoint survived reboot; tree now saved as A seq=2 / B seq=3")
    session.finish(0)

    # ---- Boot 3: corrupt the newest slot; the older one must win ---------
    seq_a = slot_seq(disk, SLOT_A_OFFSET)
    seq_b = slot_seq(disk, SLOT_B_OFFSET)
    if (seq_a, seq_b) != (2, 3):
        return fail("expected slot seqs A=2 B=3, found A=%r B=%r" % (seq_a, seq_b), b"")
    corrupt_slot_payload(disk, SLOT_B_OFFSET)

    session, status = begin_boot(qemu, iso, disk, qemu_log, 3)
    if session is None:
        return status
    status = check_tokens(
        session,
        3,
        [
            b"persist: slot B invalid (payload crc mismatch)",
            b"persist: loaded slot A seq=2",
        ],
        "boot log",
    )
    if status:
        return status
    for line, tokens in [
        ("cat /tmp/hello.txt", [b"hello world"]),
        ("cat /tmp/notes/a.txt", [b"no such file or directory"]),
    ]:
        status = run_command(session, 3, line, tokens)
        if status:
            return status
    print("boot 3: corrupted slot B rejected by CRC, fell back to slot A seq=2")
    session.finish(0)

    # ---- Boot 4: corrupt both slots; fresh format -------------------------
    corrupt_slot_payload(disk, SLOT_A_OFFSET)
    corrupt_slot_payload(disk, SLOT_B_OFFSET)

    session, status = begin_boot(qemu, iso, disk, qemu_log, 4)
    if session is None:
        return status
    status = check_tokens(
        session,
        4,
        [
            b"persist: slot A invalid (payload crc mismatch)",
            b"persist: slot B invalid (payload crc mismatch)",
            b"persist: no valid checkpoint, formatted slot A seq=0",
        ],
        "boot log",
    )
    if status:
        return status
    status = run_command(session, 4, "cat /tmp/hello.txt", [b"no such file or directory"])
    if status:
        return status
    print("boot 4: both slots rejected, fresh format, old file gone")
    session.finish(0)

    print("persistence: 4 boots, all phases passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
