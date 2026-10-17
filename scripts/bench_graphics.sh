#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=lib.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"
skip_exit "no accelerated graphics implementation exists to benchmark; framebuffer drawing is labelled as such"
