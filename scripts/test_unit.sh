#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=lib.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"

have cargo || die "cargo not found"
log "running host unit tests (os_core)"
cargo test -p os_core
log "PASS: host unit tests"
