#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=lib.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"

log "PROJECT_OS bootstrap check"
log "host: $(uname -srm 2>/dev/null || echo unknown)"
log "repo: $ROOT_DIR"

missing_required=0
check_required() {
    local name=$1
    if have "$name"; then
        printf '  %-22s %s\n' "$name" "$( "$name" --version 2>&1 | head -1 )"
    else
        printf '  %-22s MISSING\n' "$name"
        missing_required=1
    fi
}
check_optional() {
    local name=$1
    if have "$name"; then
        printf '  %-22s %s\n' "$name" "$( "$name" --version 2>&1 | head -1 )"
    else
        printf '  %-22s missing (optional)\n' "$name"
    fi
}

check_required rustc
check_required cargo
check_required mkisofs
check_required sha256sum
check_required tar
check_required curl

log "optional test tooling:"
if QEMU_BIN=$(find_qemu); then
    printf '  %-22s %s\n' qemu-system-x86_64 "$("$QEMU_BIN" --version | head -1)"
else
    printf '  %-22s missing (optional)\n' qemu-system-x86_64
fi
if VBOX_BIN=$(find_vboxmanage); then
    printf '  %-22s %s\n' VBoxManage "$("$VBOX_BIN" --version | head -1)"
else
    printf '  %-22s missing (optional)\n' VBoxManage
fi
check_optional python
check_optional readelf

if have rustup; then
    if rustup target list --installed 2>/dev/null | grep -qx "$KERNEL_TARGET"; then
        log "rust target installed: $KERNEL_TARGET"
    else
        log "rust target missing: $KERNEL_TARGET"
        log "  fix: rustup target add $KERNEL_TARGET"
        missing_required=1
    fi
fi

if [ ! -f "$ROOT_DIR/boot/limine/limine-bios.sys" ]; then
    log "pinned Limine $LIMINE_VERSION boot assets not fetched yet"
    log "  fix: scripts/fetch_limine.sh"
fi

if [ "$missing_required" -ne 0 ]; then
    log ""
    log "Required tools are missing. Installation guidance (no elevation is performed by this script):"
    log "  Windows (scoop, per-user): scoop install rustup cdrtools qemu"
    log "  Debian/Ubuntu: sudo apt install genisoimage qemu-system-x86"
    log "  Fedora: sudo dnf install genisoimage qemu-system-x86"
    log "Note: this project needs a mkisofs-compatible binary; cdrtools 3.02a09 is"
    log "tested and is what the ISO scripts use. xorriso is not required."
    exit 1
fi

log "bootstrap: required tools present"
