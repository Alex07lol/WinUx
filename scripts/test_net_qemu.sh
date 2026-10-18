#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=lib.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"

# Boots the interactive ISO (no `debug_exit`) on q35 with an explicitly pinned
# e1000 NIC and a user-mode network, then runs the scripted session in
# test_net_qemu.py: pinned bring-up lines, the `,net` boot marker, shell
# counters, `ping 10.0.2.2 -c 3` and a host-forwarded UDP echo round trip.
# q35 is pinned because the machine's default NIC is an unsupported e1000e;
# the test must prove the driver works on a NIC it actually supports.

INTERACTIVE_ISO="$ROOT_DIR/artifacts/project-os-interactive.iso"
QEMU_LOG="$ROOT_DIR/artifacts/qemu-net.log"

[ -f "$KERNEL_ARTIFACT" ] || die "kernel artifact missing; run scripts/build.sh first"
have python || skip_exit "python unavailable; network test not run"

log "building interactive ISO (debug_exit stripped from the cmdline)"
"$ROOT_DIR/scripts/make_iso.sh" --no-build --interactive

if ! QEMU=$(find_qemu); then
    skip_exit "QEMU unavailable; network test not run"
fi

python "$ROOT_DIR/scripts/test_net_qemu.py" \
    "$(winpath "$QEMU")" \
    "$(winpath "$INTERACTIVE_ISO")" \
    "$(winpath "$QEMU_LOG")"

log "PASS: e1000 network end-to-end (ARP, ICMP, UDP echo)"
log "session log: $ROOT_DIR/artifacts/qemu-net-session.log"
