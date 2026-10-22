#!/usr/bin/env python3
"""Stop/restart lifecycle test for the PROJECT_OS e1000 driver (Phase 9, stage 4).

Boots the interactive ISO on q35 with a pinned e1000, checks the boot-time
driver and network selftest, then drives the shell:

  * `mem` before stop            frame allocator free count
  * `drivers stop`               NIC stopped, claims released, 19 frames freed
  * `mem` after stop             free count increased by exactly 19
  * `net`                        network inactive
  * `drivers`                    binding Stopped, zero resource claims
  * `drivers start`              same claim/start path, binding Running again
  * `mem` after restart          free count back to the pre-stop value
  * `ping 10.0.2.2 -c 1`         ARP + ICMP after the restart

Usage: test_driver_restart_qemu.py <qemu.exe> <iso> <qemu-log>

Exit status: 0 on pass, 1 on any failed assertion, 2 on usage error.
"""

import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from qemu_serial import (  # noqa: E402
    BOOT_TIMEOUT,
    COMMAND_TIMEOUT,
    FATAL,
    MARKER,
    QemuSerialSession,
    fail,
)

FREED_FRAMES = 19


def run_command(session, line, tokens):
    ok, _, missing = session.send_command(line, tokens)
    if not ok:
        if missing[0] == b"<prompt>":
            return session.finish(fail("no prompt after: %r" % line, session.data))
        return session.finish(
            fail(
                "command %r: missing %r"
                % (line, missing[0].decode(errors="replace")),
                session.data,
            )
        )
    return 0


def mem_free(session):
    ok, segment, _ = session.send_command("mem", [b"frames: "])
    if not ok:
        return None
    match = re.search(rb"frames: (\d+) free", segment)
    return int(match.group(1)) if match else None


def main():
    if len(sys.argv) != 4:
        sys.stderr.write("usage: test_driver_restart_qemu.py <qemu> <iso> <qemu-log>\n")
        return 2
    qemu, iso, qemu_log = sys.argv[1:]
    session_log = os.path.join(os.path.dirname(qemu_log), "qemu-driver-restart-session.log")

    try:
        session = QemuSerialSession(
            qemu,
            iso,
            session_log,
            qemu_log,
            machine="q35",
            extra_args=("-nic", "user,model=e1000"),
        )
    except RuntimeError as error:
        sys.stderr.write("FAIL: %s\n" % error)
        return 1

    error = session.wait_for_shell(BOOT_TIMEOUT, COMMAND_TIMEOUT)
    if error is not None:
        return session.finish(fail(error, session.data))
    if FATAL in session.data:
        return session.finish(fail("kernel reported PROJECT_OS_FATAL", session.data))

    boot_tokens = [
        b"driver: e1000 matched 00:02.0 8086:100e",
        b"driver: e1000 state Running",
        b"driver: e1000 resources mmio 0xfebc0000..0xfebe0000 (131072 B), 4 DMA range(s)/19 page(s), irq 11 (legacy INTx)",
        b"driver selftest: passed (1 binding(s), 5 unmatched; match rules, uniqueness, single-instance and resource ownership verified)",
        b"net selftest: passed (arp resolved, icmp echo reply in ",
    ]
    missing = [token for token in boot_tokens if token not in session.data]
    if missing:
        return session.finish(
            fail("boot log: missing %r" % missing[0].decode(), session.data)
        )

    marker_line = b""
    for line in bytes(session.data).splitlines():
        if line.startswith(MARKER):
            marker_line = line
    for token in (b",driver", b",net"):
        if token not in marker_line:
            return session.finish(
                fail("boot marker without %r: %r" % (token, marker_line), session.data)
            )

    def irq_events():
        events = 0
        for line in bytes(session.data).splitlines():
            if line.startswith(b"net: mac "):
                match = re.search(rb"irq (\d+) event", line)
                if match:
                    events = int(match.group(1))
        return events

    status = run_command(session, "net", [b"net: mac 52:54:00:", b"irq "])
    if status:
        return status
    baseline_events = irq_events()
    if baseline_events <= 0:
        return session.finish(
            fail("e1000 delivered no interrupt events before stop", session.data)
        )

    before = mem_free(session)
    if before is None:
        return session.finish(fail("mem before stop did not report free frames", session.data))

    status = run_command(
        session,
        "drivers stop",
        [
            b"net: stopped (19 DMA frame(s) released)",
            b"driver: e1000 state Stopped",
            b"driver: e1000 resources released (6 claim(s); mmio mapping retained)",
        ],
    )
    if status:
        return status

    after = mem_free(session)
    if after is None:
        return session.finish(fail("mem after stop did not report free frames", session.data))
    if after - before != FREED_FRAMES:
        return session.finish(
            fail(
                "stop freed %d frames, expected %d (before=%d after=%d)"
                % (after - before, FREED_FRAMES, before, after),
                session.data,
            )
        )

    status = run_command(session, "net", [b"net: inactive (no supported e1000 NIC)"])
    if status:
        return status
    status = run_command(
        session,
        "drivers",
        [b"driver: e1000 state Stopped", b"drivers: 0 resource claim(s)"],
    )
    if status:
        return status

    status = run_command(
        session,
        "drivers start",
        [
            b"net: e1000 at 00:02.0 bar0 0xfebc0000 size 128 KiB",
            b"driver: e1000 state Running",
            b"driver: e1000 resources mmio 0xfebc0000..0xfebe0000 (131072 B), 4 DMA range(s)/19 page(s), irq 11 (legacy INTx)",
            b"driver: e1000 restarted (stop/start cycle ok)",
        ],
    )
    if status:
        return status

    restarted = mem_free(session)
    if restarted is None:
        return session.finish(fail("mem after restart did not report free frames", session.data))
    if restarted != before:
        return session.finish(
            fail(
                "restart free frames %d != pre-stop %d (19-frame leak?)"
                % (restarted, before),
                session.data,
            )
        )

    status = run_command(
        session,
        "ping 10.0.2.2 -c 1",
        [
            b"reply from 10.0.2.2: seq=1 ",
            b"ping: 1 sent, 1 received, 0% loss",
        ],
    )
    if status:
        return status

    status = run_command(session, "net", [b"net: mac 52:54:00:", b"irq "])
    if status:
        return status
    restarted_events = irq_events()
    if restarted_events <= baseline_events:
        return session.finish(
            fail(
                "e1000 IRQ count did not grow after restart (%d -> %d)"
                % (baseline_events, restarted_events),
                session.data,
            )
        )

    print(
        "driver restart: stop freed %d frames, restart reclaimed them, "
        "ping and IRQ delivery after restart passed (%d -> %d events)"
        % (FREED_FRAMES, baseline_events, restarted_events)
    )
    return session.finish(0)


if __name__ == "__main__":
    sys.exit(main())
