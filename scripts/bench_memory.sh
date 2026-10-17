#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=lib.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"

log "frame allocator benchmark (host model, release build)"
cargo run -p membench --release -- "$@"
