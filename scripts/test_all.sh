#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=lib.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"

RESULTS=$(mktemp)
trap 'rm -f "$RESULTS"' EXIT

run() {
    local name=$1
    shift
    log ""
    log "===== $name ====="
    set +e
    "$@"
    local code=$?
    set -e
    local status
    case "$code" in
        0) status=PASS ;;
        2) status=SKIP ;;
        *) status=FAIL ;;
    esac
    printf '%s\t%s\n' "$status" "$name" >>"$RESULTS"
}

run bootstrap "$ROOT_DIR/scripts/bootstrap.sh"
run format "$ROOT_DIR/scripts/format.sh"
run lint "$ROOT_DIR/scripts/lint.sh"
run unit "$ROOT_DIR/scripts/test_unit.sh"
run build "$ROOT_DIR/scripts/build.sh"
run userland "$ROOT_DIR/scripts/build_user.sh"
run iso "$ROOT_DIR/scripts/make_iso.sh" --no-build
run verify-iso "$ROOT_DIR/scripts/test_iso.sh"
run boot-qemu-bios "$ROOT_DIR/scripts/test_boot_qemu.sh" "$ISO_PATH" --firmware bios --memory 512M \
    --serial-log "$ROOT_DIR/artifacts/qemu-serial-512m.log" --timeout 90
run boot-qemu-bios-256m "$ROOT_DIR/scripts/test_boot_qemu.sh" "$ISO_PATH" --firmware bios --memory 256M \
    --serial-log "$ROOT_DIR/artifacts/qemu-serial-256m.log" --timeout 90
run boot-qemu-bios-1g "$ROOT_DIR/scripts/test_boot_qemu.sh" "$ISO_PATH" --firmware bios --memory 1G \
    --serial-log "$ROOT_DIR/artifacts/qemu-serial-1g.log" --timeout 90
run boot-qemu-uefi "$ROOT_DIR/scripts/test_boot_qemu.sh" "$ISO_PATH" --firmware uefi --timeout 90
run elf "$ROOT_DIR/scripts/test_elf_qemu.sh"
run memory "$ROOT_DIR/scripts/test_memory.sh"
run vfs "$ROOT_DIR/scripts/test_vfs.sh"
run graphics-host "$ROOT_DIR/scripts/test_graphics_host.sh"
run virtualbox "$ROOT_DIR/scripts/test_virtualbox.sh" "$ISO_PATH" --timeout 90
run shell "$ROOT_DIR/scripts/test_shell_qemu.sh"
run persistence "$ROOT_DIR/scripts/test_persistence_qemu.sh"
run pci "$ROOT_DIR/scripts/test_pci.sh"
run driver-model "$ROOT_DIR/scripts/test_driver_model.sh"
run gpu-vm "$ROOT_DIR/scripts/test_gpu_vm.sh"

log ""
log "===== SUMMARY ====="
PASS=0
FAIL=0
SKIP=0
while IFS=$'\t' read -r status name; do
    printf '%-5s %s\n' "$status" "$name"
    case "$status" in
        PASS) PASS=$((PASS + 1)) ;;
        FAIL) FAIL=$((FAIL + 1)) ;;
        SKIP) SKIP=$((SKIP + 1)) ;;
    esac
done <"$RESULTS"
log ""
log "PASS=$PASS FAIL=$FAIL SKIP=$SKIP (skipped is not passed)"
log "serial logs: $ROOT_DIR/artifacts/qemu-serial-{256m,512m,1g}.log"
log "ISO: $ISO_PATH"

if [ "$FAIL" -ne 0 ]; then
    log "result: FAIL"
    exit 1
fi
log "result: no failures (see SKIP entries for what was not run)"
