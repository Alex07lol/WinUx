#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=lib.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"

# Boots the default ISO on q35 with a pinned e1000 and asserts the Phase 9
# stage-3 driver lifecycle: one registered descriptor, the e1000 binding,
# a successful start (state Running), its resource claim (BAR0 MMIO range,
# exact DMA page ranges and the q35 legacy INTx line), the unmatched count, the structural self-test and
# the marker's `driver` check. Stop/restart is not exercised (not implemented).
SERIAL_OUT="$ROOT_DIR/artifacts/qemu-driver-session.log"
QEMU_LOG="$ROOT_DIR/artifacts/qemu-driver.log"
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

log "booting driver-model image with e1000 (timeout ${TIMEOUT}s)"
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
assert_line "driver: registered 1 descriptor(s)"
assert_line "driver: e1000 matched 00:02.0 8086:100e"
assert_line "ioapic: version 0x20, 24 pins, id 0; timer GSI2, console GSI4"
assert_line "pit: 100 Hz, IOAPIC GSI2"
assert_line "console: COM1 receive interrupt enabled (IOAPIC GSI4)"
assert_line "driver: e1000 state Running"
assert_line "driver: e1000 resources mmio 0xfebc0000..0xfebe0000 (131072 B), 4 DMA range(s)/19 page(s), irq 11 (legacy INTx)"
assert_line "driver: 5 function(s) unmatched (no registered driver)"
assert_line "driver selftest: passed (1 binding(s), 5 unmatched; match rules, uniqueness, single-instance and resource ownership verified)"
grep -aq "PROJECT_OS_BOOT_SUCCESS:.*checks=.*driver" "$SERIAL_OUT" \
    || die "boot marker does not include the driver check (log: $SERIAL_OUT)"
grep -aq "panic\|PROJECT_OS_FATAL" "$SERIAL_OUT" \
    && die "kernel panicked or reported a fatal error (log: $SERIAL_OUT)"

log "serial asserts: descriptor registration, e1000 Running binding, resource claim, unmatched count, structural self-test, marker check"
log "PASS: driver start and resource ownership verified (BAR0 + four DMA ranges; stop/restart covered by the restart harness)"
log "session log: $SERIAL_OUT"
