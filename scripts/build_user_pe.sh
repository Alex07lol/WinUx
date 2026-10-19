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

log "building PE fixtures (release, $PE_TARGET, rust-lld)"
log "  linker: $RUST_LLD"
# RUSTFLAGS replaces every config rustflags array; cargo would otherwise join
# the repository-root kernel flags (-Tkernel/linker.ld) into this workspace.
(cd "$ROOT_DIR/userland/pe" && RUSTFLAGS="-C linker=$RUST_LLD -C link-arg=/nodefaultlib -C link-arg=/entry:_start -C link-arg=/subsystem:console" cargo build --release --target "$PE_TARGET")

mkdir -p "$ROOT_DIR/artifacts"
for fixture in pehello peconsole pebasics peinput pereg peregw; do
    [ -f "$PE_TARGET_DIR/$fixture.exe" ] || die "cargo did not produce $PE_TARGET_DIR/$fixture.exe"
    cp "$PE_TARGET_DIR/$fixture.exe" "$ROOT_DIR/artifacts/$fixture.exe"
done

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

# pebadimp.exe / pebadfn.exe are deterministic single byte patches of
# peconsole.exe's import table: an unknown DLL and an unknown function. Both
# stay structurally valid images; the kernel's allowlist is what must reject
# them, and each pattern must occur exactly once in the real image.
python - "$ROOT_DIR/artifacts/peconsole.exe" "$ROOT_DIR/artifacts/pebadimp.exe" "$ROOT_DIR/artifacts/pebadfn.exe" <<'PY'
import sys

source, badimp, badfn = sys.argv[1], sys.argv[2], sys.argv[3]
data = open(source, "rb").read()


def patch(data, old, new, label):
    assert len(old) == len(new), f"internal error: {label} replacement changes size"
    count = data.count(old)
    if count != 1:
        raise SystemExit(f"error: {label} pattern {old!r} occurs {count} times (must be 1)")
    return data.replace(old, new)


open(badimp, "wb").write(patch(data, b"kernel32.dll", b"user32.dll\x00\x00", "unknown-dll mutant"))
open(badfn, "wb").write(patch(data, b"WriteConsoleA", b"WriteConsoleB", "unknown-function mutant"))
PY

# pebadfn2.exe is a deterministic single byte-sequence patch of pebasics.exe's
# import table: the function name `GetTickCount` (12 bytes) becomes `Sleep`
# plus NUL padding, so the entry stays 12 bytes and the image remains
# structurally valid. The kernel's allowlist is what must reject it.
python - "$ROOT_DIR/artifacts/pebasics.exe" "$ROOT_DIR/artifacts/pebadfn2.exe" <<'PY'
import sys

source, badfn2 = sys.argv[1], sys.argv[2]
data = open(source, "rb").read()

old, new = b"GetTickCount", b"Sleep\x00\x00\x00\x00\x00\x00\x00"
assert len(old) == len(new), "internal error: pebadfn2 replacement changes size"
count = data.count(old)
if count != 1:
    raise SystemExit(f"error: GetTickCount pattern occurs {count} times (must be 1)")
open(badfn2, "wb").write(data.replace(old, new))
PY

python "$ROOT_DIR/scripts/check_user_pe.py" "$ROOT_DIR/artifacts/pehello.exe" --imports -
python "$ROOT_DIR/scripts/check_user_pe.py" "$ROOT_DIR/artifacts/peconsole.exe" \
    --imports 'kernel32.dll:ExitProcess,GetStdHandle,WriteConsoleA'
python "$ROOT_DIR/scripts/check_user_pe.py" "$ROOT_DIR/artifacts/pebasics.exe" \
    --imports 'kernel32.dll:ExitThread,GetCommandLineA,GetCurrentProcessId,GetCurrentThreadId,GetLastError,GetStdHandle,GetTickCount,SetLastError,WriteFile'
python "$ROOT_DIR/scripts/check_user_pe.py" "$ROOT_DIR/artifacts/peinput.exe" \
    --imports 'kernel32.dll:ExitThread,GetLastError,GetStdHandle,ReadFile,WriteFile'
python "$ROOT_DIR/scripts/check_user_pe.py" "$ROOT_DIR/artifacts/pereg.exe" \
    --imports 'advapi32.dll:RegCloseKey,RegOpenKeyExA,RegQueryValueExA;kernel32.dll:ExitThread,WriteFile'
python "$ROOT_DIR/scripts/check_user_pe.py" "$ROOT_DIR/artifacts/peregw.exe" \
    --imports 'advapi32.dll:RegCloseKey,RegCreateKeyExA,RegDeleteValueA,RegQueryValueExA,RegSetValueExA;kernel32.dll:ExitThread,WriteFile'
if CHECK_OUT=$(python "$ROOT_DIR/scripts/check_user_pe.py" "$ROOT_DIR/artifacts/pebad.exe" 2>&1); then
    die "pebad.exe passed the header checker; it must be rejected"
fi
printf '%s\n' "$CHECK_OUT" | grep -q "bad signature" || die "pebad.exe was rejected for the wrong reason: $CHECK_OUT"
log "pebad.exe rejected as expected (bad signature)"
python "$ROOT_DIR/scripts/check_user_pe.py" "$ROOT_DIR/artifacts/pebadimp.exe" \
    --imports 'user32.dll:ExitProcess,GetStdHandle,WriteConsoleA'
python "$ROOT_DIR/scripts/check_user_pe.py" "$ROOT_DIR/artifacts/pebadfn.exe" \
    --imports 'kernel32.dll:ExitProcess,GetStdHandle,WriteConsoleB'
python "$ROOT_DIR/scripts/check_user_pe.py" "$ROOT_DIR/artifacts/pebadfn2.exe" \
    --imports 'kernel32.dll:ExitThread,GetCommandLineA,GetCurrentProcessId,GetCurrentThreadId,GetLastError,GetStdHandle,Sleep,SetLastError,WriteFile'

for artifact in pehello.exe peconsole.exe pebasics.exe peinput.exe pereg.exe peregw.exe pebad.exe pebadimp.exe pebadfn.exe pebadfn2.exe; do
    log "  $artifact: $(wc -c <"$ROOT_DIR/artifacts/$artifact") bytes, sha256 $(sha256sum "$ROOT_DIR/artifacts/$artifact" | awk '{print $1}')"
done
log "PASS: PE fixtures built and checked"
