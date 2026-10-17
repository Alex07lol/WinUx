#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=lib.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"

DEST_DIR="$ROOT_DIR/boot/limine"
CACHE_DIR="$ROOT_DIR/third_party"
ARCHIVE="$CACHE_DIR/limine-binary-${LIMINE_VERSION}.tar.xz"
mkdir -p "$CACHE_DIR" "$DEST_DIR"

log "Limine $LIMINE_VERSION (pinned, SHA-256 verified)"

if [ ! -f "$ARCHIVE" ]; then
    log "downloading $LIMINE_URL"
    curl -fL --retry 3 --connect-timeout 20 -o "$ARCHIVE.part" "$LIMINE_URL"
    mv "$ARCHIVE.part" "$ARCHIVE"
else
    log "using cached archive: $ARCHIVE"
fi

printf '%s  %s\n' "$LIMINE_SHA256" "$ARCHIVE" | sha256sum -c - >/dev/null
log "checksum OK"

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
tar -xJf "$ARCHIVE" -C "$TMP" limine-binary/limine-bios.sys limine-binary/limine-bios-cd.bin \
    limine-binary/limine-uefi-cd.bin limine-binary/BOOTX64.EFI \
    limine-binary/limine-tool-windows-x86/limine.exe limine-binary/LICENSE limine-binary/3RDPARTY.md

cp "$TMP/limine-binary/limine-bios.sys" "$DEST_DIR/"
cp "$TMP/limine-binary/limine-bios-cd.bin" "$DEST_DIR/"
cp "$TMP/limine-binary/limine-uefi-cd.bin" "$DEST_DIR/"
cp "$TMP/limine-binary/BOOTX64.EFI" "$DEST_DIR/"
cp "$TMP/limine-binary/limine-tool-windows-x86/limine.exe" "$DEST_DIR/"
cp "$TMP/limine-binary/LICENSE" "$DEST_DIR/LICENSE"
cp "$TMP/limine-binary/3RDPARTY.md" "$DEST_DIR/3RDPARTY.md"

log "installed boot assets into boot/limine/"
ls -l "$DEST_DIR" | sed 's/^/  /'
