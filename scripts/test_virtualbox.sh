#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=lib.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"

ISO="$ISO_PATH"
FIRMWARE="bios"
TIMEOUT=90
while [ "$#" -gt 0 ]; do
    case "$1" in
        --firmware) FIRMWARE="${2:?missing firmware value}"; shift 2 ;;
        --timeout) TIMEOUT="${2:?missing timeout value}"; shift 2 ;;
        --help|-h)
            log "usage: $0 [iso] [--firmware bios|efi] [--timeout seconds]"
            exit 0
            ;;
        *) ISO="$1"; shift ;;
    esac
done

[ -f "$ISO" ] || die "ISO not found: $ISO (run scripts/make_iso.sh)"
VBOX=$(find_vboxmanage) || {
    warn "VirtualBox (VBoxManage) is not installed; automated VirtualBox validation skipped."
    log ""
    log "Manual verification:"
    log "  1. Open VirtualBox -> New -> Name: PROJECT_OS-test, Type: Other, Version: Other/Unknown (64-bit)."
    log "  2. Memory: 512 MiB. Do not attach a virtual hard disk."
    log "  3. Settings -> Storage -> add an Optical Drive and attach:"
    log "       $ISO"
    log "  4. Settings -> System -> Motherboard: enable EFI only if testing the UEFI path."
    log "  5. Settings -> Serial Ports -> Port 1: enable, Port Mode 'Raw File', path artifacts/vbox-serial.log."
    log "  6. Start the VM. Expect the PROJECT_OS dashboard and this serial line:"
    log "       PROJECT_OS_BOOT_SUCCESS:<build-id>"
    log "  7. Power off the VM; do not modify other VMs."
    exit 2
}

command -v VBoxManage >/dev/null 2>&1 || true
NAME="PROJECT_OS-test-$$"
BASE="$ROOT_DIR/artifacts/vbox"
SERIAL="$ROOT_DIR/artifacts/vbox-serial.log"
mkdir -p "$BASE"
rm -f "$SERIAL"

if "$VBOX" list vms | grep -Fq "\"$NAME\""; then
    die "refusing to reuse existing VM named $NAME"
fi

VM_CREATED=0
cleanup() {
    set +e
    if [ "$VM_CREATED" -eq 1 ]; then
        "$VBOX" controlvm "$NAME" poweroff >/dev/null 2>&1
        sleep 1
        "$VBOX" unregistervm "$NAME" --delete >/dev/null 2>&1
    fi
}
trap cleanup EXIT

log "VirtualBox: $("$VBOX" --version | head -1)"
log "creating isolated VM $NAME"
"$VBOX" createvm --name "$NAME" --ostype Other_64 --register --basefolder "$(winpath "$BASE")" >/dev/null
VM_CREATED=1
"$VBOX" modifyvm "$NAME" --memory 512 --cpus 1 --firmware "$FIRMWARE" \
    --uart1 0x3F8 4 --uartmode1 file "$(winpath "$SERIAL")" \
    --nic1 nat --nictype1 82540EM >/dev/null
"$VBOX" storagectl "$NAME" --name SATA --add sata --controller IntelAhci >/dev/null
"$VBOX" storageattach "$NAME" --storagectl SATA --port 0 --device 0 \
    --type dvddrive --medium "$(winpath "$ISO")" >/dev/null

log "starting headless (timeout ${TIMEOUT}s)"
"$VBOX" startvm "$NAME" --type headless >/dev/null

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
    sleep 0.5
done

if grep -aq "PROJECT_OS_PANIC\|PROJECT_OS_FATAL" "$SERIAL" 2>/dev/null; then
    warn "kernel panic/fatal reported by VirtualBox guest"
    grep -a "PROJECT_OS_PANIC\|PROJECT_OS_FATAL" "$SERIAL" | sed 's/^/  /' >&2
    exit 1
fi
if [ "$FOUND" -ne 1 ]; then
    warn "VirtualBox did not reach the success marker within ${TIMEOUT}s"
    tail -30 "$SERIAL" 2>/dev/null || true
    exit 1
fi

log "marker: $(grep -a 'PROJECT_OS_BOOT_SUCCESS:' "$SERIAL" | tail -1)"
log "serial log: $SERIAL"
log "PASS: VirtualBox $FIRMWARE boot reached PROJECT_OS_BOOT_SUCCESS"
