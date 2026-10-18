#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=lib.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"

have cargo || die "cargo not found"
have python || die "python not found (needed for bad.elf generation and header checks)"

USER_TARGET_DIR="$ROOT_DIR/userland/target/x86_64-unknown-none/release"

log "building userland programs (release, x86_64-unknown-none)"
# RUSTFLAGS replaces every config rustflags array; cargo would otherwise join
# the repository-root kernel flags (-Tkernel/linker.ld) into this workspace.
(cd "$ROOT_DIR/userland" && RUSTFLAGS="-C relocation-model=static -C code-model=small -C link-arg=-Tlinker.ld -C link-arg=--build-id=none -C link-arg=-zmax-page-size=4096" cargo build --release)

mkdir -p "$ROOT_DIR/artifacts"
for program in hello fault; do
    [ -f "$USER_TARGET_DIR/$program" ] || die "cargo did not produce $USER_TARGET_DIR/$program"
    cp "$USER_TARGET_DIR/$program" "$ROOT_DIR/artifacts/$program.elf"
done

# bad.elf is a deterministic corruption of hello.elf: the ELF magic bytes are
# zeroed, so the kernel's parser must reject it with "bad magic".
python - "$ROOT_DIR/artifacts/hello.elf" "$ROOT_DIR/artifacts/bad.elf" <<'PY'
import sys

source, destination = sys.argv[1], sys.argv[2]
data = bytearray(open(source, "rb").read())
data[0:4] = b"\x00\x00\x00\x00"
open(destination, "wb").write(data)
PY

python "$ROOT_DIR/scripts/check_user_elf.py" "$ROOT_DIR/artifacts/hello.elf"
python "$ROOT_DIR/scripts/check_user_elf.py" "$ROOT_DIR/artifacts/fault.elf"
if CHECK_OUT=$(python "$ROOT_DIR/scripts/check_user_elf.py" "$ROOT_DIR/artifacts/bad.elf" 2>&1); then
    die "bad.elf passed the header checker; it must be rejected"
fi
printf '%s\n' "$CHECK_OUT" | grep -q "bad magic" || die "bad.elf was rejected for the wrong reason: $CHECK_OUT"
log "bad.elf rejected as expected (bad magic)"

for artifact in hello.elf fault.elf bad.elf; do
    log "  $artifact: $(wc -c <"$ROOT_DIR/artifacts/$artifact") bytes, sha256 $(sha256sum "$ROOT_DIR/artifacts/$artifact" | awk '{print $1}')"
done
log "PASS: userland programs built and checked"
