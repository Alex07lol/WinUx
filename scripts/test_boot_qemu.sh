#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=lib.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"

ISO="$ISO_PATH"
FIRMWARE="bios"
TIMEOUT=60
MEMORY="${PROJECT_OS_MEMORY:-512M}"
SERIAL_LOG_OVERRIDE=""
REQUIRE_IOAPIC=0
while [ "$#" -gt 0 ]; do
    case "$1" in
        --firmware) FIRMWARE="${2:?missing firmware value}"; shift 2 ;;
        --timeout) TIMEOUT="${2:?missing timeout value}"; shift 2 ;;
        --memory) MEMORY="${2:?missing memory value}"; shift 2 ;;
        --serial-log) SERIAL_LOG_OVERRIDE="${2:?missing serial log path}"; shift 2 ;;
        --require-ioapic) REQUIRE_IOAPIC=1; shift ;;
        --help|-h)
            log "usage: $0 [iso] [--firmware bios|uefi] [--memory 512M] [--serial-log path] [--timeout seconds] [--require-ioapic]"
            exit 0
            ;;
        *)
            ISO="$1"
            shift
            ;;
    esac
done

[ -f "$ISO" ] || die "ISO not found: $ISO (run scripts/make_iso.sh)"
QEMU=$(find_qemu) || skip_exit "qemu-system-x86_64 not found (set QEMU_SYSTEM_X86_64 or install QEMU)"

EXTRA=()
SERIAL_OUT="$ROOT_DIR/artifacts/qemu-serial.log"
if [ "$FIRMWARE" = "uefi" ]; then
    [ -n "${OVMF_FD:-}" ] || skip_exit "UEFI test requires OVMF_FD (no OVMF firmware is shipped with QEMU)"
    [ -f "$OVMF_FD" ] || skip_exit "OVMF_FD does not exist: $OVMF_FD"
    EXTRA=(-bios "$(winpath "$OVMF_FD")")
    SERIAL_OUT="$ROOT_DIR/artifacts/qemu-serial-uefi.log"
elif [ "$FIRMWARE" != "bios" ]; then
    die "unknown firmware '$FIRMWARE' (expected bios or uefi)"
fi
if [ -n "$SERIAL_LOG_OVERRIDE" ]; then
    SERIAL_OUT="$SERIAL_LOG_OVERRIDE"
fi

SERIAL=$(mktemp)
QEMU_LOG=$(mktemp)
QPID=""
cleanup() {
    if [ -n "$QPID" ] && kill -0 "$QPID" 2>/dev/null; then
        kill "$QPID" 2>/dev/null || true
        sleep 0.5
        kill -9 "$QPID" 2>/dev/null || true
    fi
    rm -f "$SERIAL" "$QEMU_LOG"
}
trap cleanup EXIT

log "QEMU: $QEMU"
"$QEMU" --version | head -1 | sed 's/^/  /'
"$QEMU" \
    -machine q35 \
    -m "$MEMORY" \
    -smp 1 \
    -cdrom "$(winpath "$ISO")" \
    -boot d \
    -display none \
    -monitor none \
    -serial "file:$(winpath "$SERIAL")" \
    -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
    -no-reboot \
    "${EXTRA[@]}" \
    >"$QEMU_LOG" 2>&1 &
QPID=$!

log "booting $FIRMWARE image with ${MEMORY} RAM (timeout ${TIMEOUT}s)"
deadline=$((SECONDS + TIMEOUT))
FOUND=0
while [ "$SECONDS" -lt "$deadline" ]; do
    if grep -aq "PROJECT_OS_BOOT_SUCCESS:" "$SERIAL" 2>/dev/null; then
        FOUND=1
        break
    fi
    if grep -aq "PROJECT_OS_PANIC\|PROJECT_OS_FATAL" "$SERIAL" 2>/dev/null; then
        break
    fi
    kill -0 "$QPID" 2>/dev/null || break
    sleep 0.25
done

mkdir -p "$ROOT_DIR/artifacts"
cp "$SERIAL" "$SERIAL_OUT" 2>/dev/null || true

if grep -aq "PROJECT_OS_PANIC\|PROJECT_OS_FATAL" "$SERIAL" 2>/dev/null; then
    warn "kernel reported a panic or fatal error:"
    grep -a "PROJECT_OS_PANIC\|PROJECT_OS_FATAL" "$SERIAL" | sed 's/^/  /' >&2
    exit 1
fi
if [ "$FOUND" -ne 1 ]; then
    warn "boot success marker not found within ${TIMEOUT}s"
    log "--- serial tail ---"
    tail -30 "$SERIAL" 2>/dev/null || true
    log "--- qemu output ---"
    tail -20 "$QEMU_LOG" 2>/dev/null || true
    exit 1
fi

# The kernel deliberately writes 0x10 to QEMU's isa-debug-exit port after the
# success marker; QEMU maps that to exit status (0x10 << 1) | 1 == 33.
WAITED=0
while kill -0 "$QPID" 2>/dev/null && [ "$WAITED" -lt 100 ]; do
    sleep 0.1
    WAITED=$((WAITED + 1))
done
if kill -0 "$QPID" 2>/dev/null; then
    warn "QEMU is still running after the success marker (expected debug-exit)"
    exit 1
fi
set +e
wait "$QPID"
STATUS=$?
set -e
QPID=""
if [ "$STATUS" -ne 33 ]; then
    warn "unexpected QEMU exit status $STATUS (expected 33 from isa-debug-exit)"
    exit 1
fi

log "marker: $(grep -a 'PROJECT_OS_BOOT_SUCCESS:' "$SERIAL" | tail -1)"
if [ "$REQUIRE_IOAPIC" -eq 1 ]; then
    grep -aqE "^ioapic: version 0x[0-9a-f]+, [0-9]+ pins, id [0-9]+; timer GSI2, console GSI4$" "$SERIAL" \
        || die "IOAPIC bring-up line missing (log: $SERIAL_OUT)"
    grep -aqF "pit: 100 Hz, IOAPIC GSI2" "$SERIAL" \
        || die "PIT is not routed through IOAPIC GSI2 (log: $SERIAL_OUT)"
    grep -aqF "console: COM1 receive interrupt enabled (IOAPIC GSI4)" "$SERIAL" \
        || die "COM1 is not routed through IOAPIC GSI4 (log: $SERIAL_OUT)"
    grep -aqE "^timer: [1-9][0-9]* ticks observed$" "$SERIAL" \
        || die "timer did not produce ticks through the IOAPIC (log: $SERIAL_OUT)"
    if grep -aqF "fell back to 8259" "$SERIAL"; then
        die "IOAPIC timer route timed out and fell back to the PIC (log: $SERIAL_OUT)"
    fi
    log "IOAPIC asserts: PIT GSI2, COM1 GSI4, timer ticks, no PIC fallback"
fi
log "serial log: $SERIAL_OUT"
log "PASS: $FIRMWARE boot reached PROJECT_OS_BOOT_SUCCESS and exited via isa-debug-exit"
