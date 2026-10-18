#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=lib.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"

have cargo || die "cargo not found"
have python || die "python not found (needed for pebad.exe generation and header checks)"

PE_TARGET="x86_64-pc-windows-msvc"
PE_TARGET_DIR="$ROOT_DIR/userland/pe/target/$PE_TARGET/release"

# rustc's sysroot and host triple locate the rust-lld that ships with the
# pinned toolchain: /nodefaultlib /entry:_start /subsystem:console must be
# driven by rust-lld directly; no MSVC, MinGW or Windows SDK is involved.
SYSROOT=$(rustc --print sysroot | sed 's|\\|/|g')
[ -n "$SYSROOT" ] || die "rustc --print sysroot failed"
HOST=$(rustc -vV | sed -n 's/^host: //p')
[ -n "$HOST" ] || die "cannot determine the rustc host triple"

RUST_LLD="$SYSROOT/lib/rustlib/$HOST/bin/rust-lld"
[ -x "$RUST_LLD" ] || RUST_LLD="$RUST_LLD.exe"
[ -x "$RUST_LLD" ] || die "rust-lld not found in the toolchain (looked under $SYSROOT/lib/rustlib/$HOST/bin)"

if have rustup; then
    rustup target list --installed | grep -qx "$PE_TARGET" ||
        die "$PE_TARGET target not installed (run: rustup target add $PE_TARGET)"
else
    TARGET_LIBDIR=$(rustc --print target-libdir --target "$PE_TARGET" 2>/dev/null || true)
    [ -n "$TARGET_LIBDIR" ] && [ -d "$TARGET_LIBDIR" ] ||
        die "$PE_TARGET target is not installed (run: rustup target add $PE_TARGET)"
fi

log "building PE fixture (release, $PE_TARGET, rust-lld)"
log "  linker: $RUST_LLD"
# RUSTFLAGS replaces every config rustflags array; cargo would otherwise join
# the repository-root kernel flags (-Tkernel/linker.ld) into this workspace.
(cd "$ROOT_DIR/userland/pe" && RUSTFLAGS="-C linker=$RUST_LLD -C link-arg=/nodefaultlib -C link-arg=/entry:_start -C link-arg=/subsystem:console" cargo build --release --target "$PE_TARGET")

mkdir -p "$ROOT_DIR/artifacts"
[ -f "$PE_TARGET_DIR/pehello.exe" ] || die "cargo did not produce $PE_TARGET_DIR/pehello.exe"
cp "$PE_TARGET_DIR/pehello.exe" "$ROOT_DIR/artifacts/pehello.exe"

# pebad.exe is a deterministic corruption of pehello.exe: the 4 signature
# bytes at e_lfanew are zeroed, so the kernel's parser must reject it with
# "bad signature".
python - "$ROOT_DIR/artifacts/pehello.exe" "$ROOT_DIR/artifacts/pebad.exe" <<'PY'
import struct
import sys

source, destination = sys.argv[1], sys.argv[2]
data = bytearray(open(source, "rb").read())
assert data[0:2] == b"MZ", "pehello.exe lost its DOS magic"
(e_lfanew,) = struct.unpack_from("<I", data, 0x3C)
assert data[e_lfanew : e_lfanew + 4] == b"PE\x00\x00", "pehello.exe lost its PE signature"
data[e_lfanew : e_lfanew + 4] = b"\x00\x00\x00\x00"
open(destination, "wb").write(data)
PY

python "$ROOT_DIR/scripts/check_user_pe.py" "$ROOT_DIR/artifacts/pehello.exe"
if CHECK_OUT=$(python "$ROOT_DIR/scripts/check_user_pe.py" "$ROOT_DIR/artifacts/pebad.exe" 2>&1); then
    die "pebad.exe passed the header checker; it must be rejected"
fi
printf '%s\n' "$CHECK_OUT" | grep -q "bad signature" || die "pebad.exe was rejected for the wrong reason: $CHECK_OUT"
log "pebad.exe rejected as expected (bad signature)"

for artifact in pehello.exe pebad.exe; do
    log "  $artifact: $(wc -c <"$ROOT_DIR/artifacts/$artifact") bytes, sha256 $(sha256sum "$ROOT_DIR/artifacts/$artifact" | awk '{print $1}')"
done
log "PASS: PE fixture built and checked"
