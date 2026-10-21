#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=lib.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"

# Boots the default ISO on q35 with an explicitly pinned e1000 NIC and the
# isa-debug-exit device, then asserts the Phase 9 PCI inventory: the bus-0
# summary, the q35 host bridge, the e1000 function and its BAR0, the self-test
# line and the boot marker's `pci` check. q35 is pinned because its device
# layout is stable and its default NIC (e1000e) is not supported by our driver.
SERIAL_OUT="$ROOT_DIR/artifacts/qemu-pci-session.log"
QEMU_LOG="$ROOT_DIR/artifacts/qemu-pci.log"
TIMEOUT=60

[ -f "$ISO_PATH" ] || die "ISO not found: $ISO_PATH (run scripts/make_iso.sh)"
QEMU=$(find_qemu) || skip_exit "qemu-system-x86_64 not found (set QEMU_SYSTEM_X86_64 or install QEMU)"

SERIAL=$(mktemp)
QEMU_TMP_LOG=$(mktemp)
QPID=""
cleanup() {
    if [ -n "$QPID" ] && kill -0 "$QPID" 2>/dev/null; then
        kill "$QPID" 2>/dev/null || true
        sleep 0.5
        kill -9 "$QPID" 2>/dev/null || true
    fi
    rm -f "$SERIAL" "$QEMU_TMP_LOG"
}
trap cleanup EXIT

log "QEMU: $QEMU"
"$QEMU" --version | head -1 | sed 's/^/  /'
"$QEMU" \
    -machine q35 \
    -m 512M \
    -smp 1 \
    -cdrom "$(winpath "$ISO_PATH")" \
    -boot d \
    -display none \
    -monitor none \
    -serial "file:$(winpath "$SERIAL")" \
    -device e1000,netdev=n0 \
    -netdev user,id=n0 \
    -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
    -no-reboot \
    >"$QEMU_TMP_LOG" 2>&1 &
QPID=$!

log "booting PCI inventory image with e1000 (timeout ${TIMEOUT}s)"
deadline=$((SECONDS + TIMEOUT))
FOUND=0
while [ "$SECONDS" -lt "$deadline" ]; do
    if grep -aq "PROJECT_OS_BOOT_SUCCESS:" "$SERIAL" 2>/dev/null; then
        FOUND=1
        break
    fi
    if ! kill -0 "$QPID" 2>/dev/null; then
        break
    fi
    sleep 0.2
done
wait "$QPID" 2>/dev/null || true
QPID=""
cp "$SERIAL" "$SERIAL_OUT"
cp "$QEMU_TMP_LOG" "$QEMU_LOG"

[ "$FOUND" -eq 1 ] || {
    sed 's/^/  /' "$QEMU_LOG" >&2
    die "boot marker not found (serial log: $SERIAL_OUT)"
}

assert_line() {
    local pattern="$1"
    grep -aqF -- "$pattern" "$SERIAL_OUT" || die "missing serial line: $pattern (log: $SERIAL_OUT)"
}
assert_line "pci: enumerated 6 function(s), 7 BAR(s) on bus 0"
assert_line "pci: 00:00.0 8086:29c0 class 06:00:00 hdr 00"
assert_line "pci: 00:02.0 8086:100e class 02:00:00 hdr 00"
assert_line "pci:   bar0 mem32 base=0xfebc0000 size=0x20000"
assert_line "pci: 00:1f.0 8086:2918 class 06:01:00 hdr 00"
assert_line "pci selftest: passed"
grep -aq "PROJECT_OS_BOOT_SUCCESS:.*checks=.*pci" "$SERIAL_OUT" \
    || die "boot marker does not include the pci check (log: $SERIAL_OUT)"
grep -aq "panic\|PROJECT_OS_FATAL" "$SERIAL_OUT" \
    && die "kernel panicked or reported a fatal error (log: $SERIAL_OUT)"

log "serial asserts: PCI inventory (6 functions, 7 BARs), q35 host bridge/LPC, e1000 8086:100e BAR0, pci selftest, marker check"
log "PASS: PCI bus-0 inventory verified (multifunction scan, BAR sizing, e1000, shell unit has no interactive claim here)"
log "session log: $SERIAL_OUT"
