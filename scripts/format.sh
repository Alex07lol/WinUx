#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=lib.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"

have cargo || die "cargo not found"
if ! cargo fmt --version >/dev/null 2>&1; then
    skip_exit "rustfmt is not installed (rustup component add rustfmt)"
fi
log "checking formatting"
cargo fmt --all -- --check
log "checking formatting (userland)"
(cd "$ROOT_DIR/userland" && cargo fmt --all -- --check)
log "PASS: formatting"
