#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=lib.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"

log "host framebuffer/geometry tests (boot framebuffer only; no GPU driver)"
cargo test -p os_core fb::
log "PASS: graphics host tests"
