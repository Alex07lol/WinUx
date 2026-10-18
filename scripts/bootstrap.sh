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
    for target in "$KERNEL_TARGET" x86_64-pc-windows-msvc; do
        if rustup target list --installed 2>/dev/null | grep -qx "$target"; then
            log "rust target installed: $target"
        else
            log "rust target missing: $target"
            log "  fix: rustup target add $target"
            missing_required=1
        fi
    done
fi

# The PE fixture build drives rust-lld directly (no MSVC/MinGW/Windows SDK).
if have rustc; then
    SYSROOT=$(rustc --print sysroot | sed 's|\\|/|g')
    HOST=$(rustc -vV | sed -n 's/^host: //p')
    RUST_LLD="$SYSROOT/lib/rustlib/$HOST/bin/rust-lld"
    [ -x "$RUST_LLD" ] || RUST_LLD="$RUST_LLD.exe"
    if [ -x "$RUST_LLD" ]; then
        log "rust-lld present: $RUST_LLD"
    else
        log "rust-lld missing under $SYSROOT/lib/rustlib/$HOST/bin"
        log "  fix: rustup component add rust-lld (or reinstall the toolchain)"
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
