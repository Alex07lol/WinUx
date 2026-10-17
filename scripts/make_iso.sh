#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=lib.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"

BUILD=1
if [ "${1:-}" = "--no-build" ]; then
    BUILD=0
fi

have mkisofs || die "mkisofs not found (scoop install cdrtools / apt install genisoimage or xorriso)"
if [ "$BUILD" -eq 1 ]; then
    "$ROOT_DIR/scripts/build.sh"
fi
[ -f "$KERNEL_ARTIFACT" ] || die "kernel artifact missing: $KERNEL_ARTIFACT"

if [ ! -f "$ROOT_DIR/boot/limine/limine-bios.sys" ]; then
    "$ROOT_DIR/scripts/fetch_limine.sh"
fi
for asset in limine-bios.sys limine-bios-cd.bin limine-uefi-cd.bin BOOTX64.EFI; do
    [ -f "$ROOT_DIR/boot/limine/$asset" ] || die "missing Limine asset: boot/limine/$asset"
done

STAGE="$ROOT_DIR/artifacts/iso_root"
rm -rf "$STAGE"
mkdir -p "$STAGE/boot" "$STAGE/EFI/BOOT" "$STAGE/licenses/rust"
cp "$KERNEL_ARTIFACT" "$STAGE/boot/kernel.elf"
cp "$ROOT_DIR/boot/limine.conf" "$STAGE/limine.conf"
cp "$ROOT_DIR/boot/limine/limine-bios.sys" "$STAGE/"
cp "$ROOT_DIR/boot/limine/limine-bios-cd.bin" "$STAGE/"
cp "$ROOT_DIR/boot/limine/limine-uefi-cd.bin" "$STAGE/"
cp "$ROOT_DIR/boot/limine/BOOTX64.EFI" "$STAGE/EFI/BOOT/"

# The ISO redistributes the kernel and Limine binaries, so include the project
# license and Limine's BSD-2-Clause notice plus its bundled third-party notices.
for notice in "$ROOT_DIR/LICENSE" "$ROOT_DIR/LICENSES.md"; do
    [ -f "$notice" ] || die "license notice missing: $notice"
    cp "$notice" "$STAGE/licenses/"
done
for notice in LICENSE 3RDPARTY.md; do
    [ -f "$ROOT_DIR/boot/limine/$notice" ] || die "Limine notice missing: boot/limine/$notice"
    cp "$ROOT_DIR/boot/limine/$notice" "$STAGE/licenses/limine-$notice"
done

# The kernel links Rust core/compiler_builtins. Include the Rust library
# copyright and license texts that accompany a binary distribution.
[ -d "$ROOT_DIR/licenses/rust" ] || die "Rust license notices missing: licenses/rust"
cp -R "$ROOT_DIR/licenses/rust/." "$STAGE/licenses/rust/"

rm -f "$ISO_PATH"
log "creating ISO with BIOS + UEFI El Torito entries"
# cdrtools mkisofs spells the EFI platform id numerically (0xef = 239); it does
# not accept the string "efi" that xorriso accepts.
mkisofs \
    -R -r -J \
    -V PROJECT_OS \
    -b limine-bios-cd.bin \
    -no-emul-boot -boot-load-size 4 -boot-info-table \
    -eltorito-alt-boot \
    -eltorito-platform 239 \
    -b limine-uefi-cd.bin \
    -no-emul-boot \
    -o "$ISO_PATH" \
    "$STAGE" >/dev/null

if LIMINE_TOOL=$(find_limine_tool); then
    if "$LIMINE_TOOL" bios-install --force "$ISO_PATH" >/tmp/project-os-limine-install.log 2>&1; then
        log "Limine hybrid MBR installed (ISO also bootable as a disk image)"
    else
        warn "limine bios-install failed; El Torito ISO boot is unaffected"
        sed 's/^/  /' /tmp/project-os-limine-install.log >&2 || true
    fi
else
    warn "Limine host tool unavailable; skipped hybrid MBR install (El Torito boot unaffected)"
fi

if have python; then
    python "$ROOT_DIR/scripts/verify_iso.py" "$ISO_PATH"
else
    warn "python unavailable; El Torito catalog verification skipped"
fi

[ -s "$ISO_PATH" ] || die "ISO was not created"
log "ISO: $ISO_PATH"
log "size: $(wc -c < "$ISO_PATH") bytes"
log "sha256: $(sha256sum "$ISO_PATH" | awk '{print $1}')"
