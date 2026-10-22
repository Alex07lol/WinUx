#!/usr/bin/env python3
"""End-to-end network test for the PROJECT_OS e1000 stack (Phase 6, stage 5).

Boots the interactive ISO on q35 with an explicitly pinned e1000 NIC on a
user-mode (slirp) network, checks the pinned `net:` bring-up lines and the
`,net` boot marker (set only when the boot selftest passed), then drives the
shell:

  * `net`                     interface status and counters
  * `ping 10.0.2.2 -c 3`      ARP resolution + three ICMP echo replies
  * `udpecho 7777`            one host->guest UDP datagram forwarded through
                              `hostfwd=udp:127.0.0.1:<port>-:7777` echoed back
                              byte-for-byte

Usage: test_net_qemu.py <qemu.exe> <iso> <qemu-log>

Exit status: 0 on pass, 1 on any failed assertion, 2 on usage error.
"""

import os
import re
import socket
import sys
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from qemu_serial import (  # noqa: E402
    BOOT_TIMEOUT,
    COMMAND_TIMEOUT,
    FATAL,
    MARKER,
    PROMPT,
    QemuSerialSession,
    fail,
)

PAYLOAD = b"PROJECT_OS-NET-TEST"


def free_udp_port():
    probe = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    probe.bind(("127.0.0.1", 0))
    port = probe.getsockname()[1]
    probe.close()
    return port


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


def main():
    if len(sys.argv) != 4:
        sys.stderr.write("usage: test_net_qemu.py <qemu> <iso> <qemu-log>\n")
        return 2
    qemu, iso, qemu_log = sys.argv[1:]
    session_log = os.path.join(os.path.dirname(qemu_log), "qemu-net-session.log")
    host_port = free_udp_port()

    try:
        session = QemuSerialSession(
            qemu,
            iso,
            session_log,
            qemu_log,
            machine="q35",
            extra_args=(
                "-nic",
                "user,model=e1000,hostfwd=udp:127.0.0.1:%d-:7777" % host_port,
            ),
        )
    except RuntimeError as error:
        sys.stderr.write("FAIL: %s\n" % error)
        return 1

    error = session.wait_for_shell(BOOT_TIMEOUT, COMMAND_TIMEOUT)
    if error is not None:
        return session.finish(fail(error, session.data))
    if FATAL in session.data:
        return session.finish(fail("kernel reported PROJECT_OS_FATAL", session.data))

    # ---- Boot-time bring-up and selftest --------------------------------
    boot_tokens = [
        b"net: e1000 at 00:",
        b"bar0 0x",
        b" KiB",
        b"net: mac 52:54:00:",
        b"net: ip 10.0.2.15/24 gw 10.0.2.2",
        b"net: e1000 irq 11 (legacy INTx, RXT0/RXDMT0; RX queue 16 frames, drop-newest)",
        b"net: loopback selftest: ",
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
    if b",net" not in marker_line:
        return session.finish(
            fail("boot marker without ,net: %r" % marker_line, session.data)
        )

    loopback_outcome = "not reported"
    for line in bytes(session.data).splitlines():
        if line.startswith(b"net: loopback selftest: "):
            loopback_outcome = line.split(b"net: loopback selftest: ", 1)[1].decode()

    # ---- Shell: interface status ----------------------------------------
    status = run_command(
        session,
        "net",
        [
            b"net: mac 52:54:00:",
            b"ip 10.0.2.15/24 gw 10.0.2.2",
            b"tx ",
            b" frames, rx ",
            b"arp 1/8 entries",
            b"errors ",
            b"rxq ",
        ],
    )
    if status:
        return status

    def irq_events():
        events = 0
        for line in bytes(session.data).splitlines():
            if line.startswith(b"net: mac "):
                match = re.search(rb"irq (\d+) event", line)
                if match:
                    events = int(match.group(1))
        return events

    def irq_delivered():
        delivered = 0
        for line in bytes(session.data).splitlines():
            if line.startswith(b"net: mac "):
                match = re.search(rb"rxq \d+/\d+ \((\d+) delivered", line)
                if match:
                    delivered = int(match.group(1))
        return delivered

    baseline_events = irq_events()
    if baseline_events <= 0:
        return session.finish(
            fail("e1000 delivered no interrupt events after boot", session.data)
        )
    baseline_delivered = irq_delivered()
    if baseline_delivered <= 0:
        return session.finish(
            fail("IRQ RX queue delivered no frames after boot", session.data)
        )

    # ---- Shell: ARP + ICMP through the slirp gateway ---------------------
    status = run_command(
        session,
        "ping 10.0.2.2 -c 3",
        [
            b"reply from 10.0.2.2: seq=1 ",
            b"reply from 10.0.2.2: seq=2 ",
            b"reply from 10.0.2.2: seq=3 ",
            b"ping: 3 sent, 3 received, 0% loss",
        ],
    )
    if status:
        return status

    status = run_command(session, "net", [b"net: mac 52:54:00:", b"irq "])
    if status:
        return status
    after_events = irq_events()
    if after_events <= baseline_events:
        return session.finish(
            fail(
                "e1000 IRQ count did not grow after ping (%d -> %d)"
                % (baseline_events, after_events),
                session.data,
            )
        )
    after_delivered = irq_delivered()
    if after_delivered <= baseline_delivered:
        return session.finish(
            fail(
                "IRQ RX queue delivery did not grow after ping (%d -> %d)"
                % (baseline_delivered, after_delivered),
                session.data,
            )
        )

    # ---- Shell: host->guest UDP datagram echoed back ---------------------
    client = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    client.bind(("127.0.0.1", 0))
    client.settimeout(1.0)
    start = len(session.data)
    session.send_paced("udpecho 7777")
    if not session.wait_for(
        b"udpecho: listening on 10.0.2.15:7777", COMMAND_TIMEOUT, start
    ):
        client.close()
        return session.finish(fail("udpecho did not start listening", session.data))

    client.sendto(PAYLOAD, ("127.0.0.1", host_port))
    echoed = None
    deadline = time.monotonic() + 10.0
    while time.monotonic() < deadline:
        try:
            echoed, _ = client.recvfrom(2048)
            break
        except socket.timeout:
            continue
        except OSError as error:
            client.close()
            return session.finish(fail("host UDP receive failed: %s" % error, session.data))
    client.close()
    if echoed != PAYLOAD:
        return session.finish(
            fail("UDP echo payload mismatch: %r" % echoed, session.data)
        )
    if not session.wait_for(b"udpecho: echoed 19 bytes to 10.0.2.2:", COMMAND_TIMEOUT, start):
        return session.finish(fail("udpecho echo line missing", session.data))
    if not session.wait_for(PROMPT, COMMAND_TIMEOUT, start):
        return session.finish(fail("no prompt after udpecho", session.data))

    print("network: loopback %s; ARP + 3/3 ICMP replies; 19-byte UDP round trip; "
          "IRQ queue delivery %d -> %d frames"
          % (loopback_outcome, baseline_delivered, after_delivered))
    return session.finish(0)


if __name__ == "__main__":
    sys.exit(main())
