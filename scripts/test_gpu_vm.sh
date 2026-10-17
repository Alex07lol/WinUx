#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=lib.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"
skip_exit "only the bootloader framebuffer is implemented; there is no virtual GPU driver (Phase 9)"
