#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=lib.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"

have cargo || die "cargo not found"
if ! cargo clippy --version >/dev/null 2>&1; then
    skip_exit "clippy is not installed (rustup component add clippy)"
fi

log "clippy: os_core (host)"
cargo clippy -p os_core --all-targets -- -D warnings
log "clippy: membench (host)"
cargo clippy -p membench -- -D warnings
log "clippy: kernel ($KERNEL_TARGET)"
cargo clippy -p "$KERNEL_BIN" --target "$KERNEL_TARGET" --release -- -D warnings
log "clippy: userland ($KERNEL_TARGET)"
(cd "$ROOT_DIR/userland" && cargo clippy -- -D warnings)
log "clippy: userland/pe (x86_64-pc-windows-msvc, release)"
(cd "$ROOT_DIR/userland/pe" && cargo clippy --release --target x86_64-pc-windows-msvc -- -D warnings)
log "PASS: lint"
