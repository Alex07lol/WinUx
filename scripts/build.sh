#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=lib.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"

have rustc || die "rustc not found (run scripts/bootstrap.sh)"
have cargo || die "cargo not found (run scripts/bootstrap.sh)"

if ! rustup target list --installed 2>/dev/null | grep -qx "$KERNEL_TARGET"; then
    die "Rust target $KERNEL_TARGET is not installed. Run: rustup target add $KERNEL_TARGET"
fi

if [ -z "${PROJECT_OS_BUILD_ID:-}" ]; then
    if git rev-parse --short=8 HEAD >/dev/null 2>&1; then
        PROJECT_OS_BUILD_ID="0.1.0-$(git rev-parse --short=8 HEAD)"
    else
        PROJECT_OS_BUILD_ID="dev"
    fi
fi
export PROJECT_OS_BUILD_ID

log "building $KERNEL_BIN for $KERNEL_TARGET (build id: $PROJECT_OS_BUILD_ID)"
cargo build -p "$KERNEL_BIN" --target "$KERNEL_TARGET" --release

TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT_DIR/target}"
KERNEL_PATH="$TARGET_DIR/$KERNEL_TARGET/release/$KERNEL_BIN"
[ -f "$KERNEL_PATH" ] || die "kernel artifact not found at $KERNEL_PATH"

mkdir -p "$ROOT_DIR/artifacts"
cp "$KERNEL_PATH" "$KERNEL_ARTIFACT"

if have readelf; then
    entry=$(readelf -hW "$KERNEL_ARTIFACT" | awk '/Entry point address/ {print $4}')
    [ "$entry" = "0xffffffff80000000" ] || die "unexpected ELF entry point: $entry"
    readelf -SW "$KERNEL_ARTIFACT" | grep -q '\.limine_requests' \
        || die "kernel is missing the .limine_requests section"
    log "ELF check: entry $entry, limine requests present"
else
    log "SKIP: readelf unavailable; ELF layout not re-verified"
fi

log "kernel: $KERNEL_ARTIFACT"
log "size: $(wc -c < "$KERNEL_ARTIFACT") bytes"
log "sha256: $(sha256sum "$KERNEL_ARTIFACT" | awk '{print $1}')"
