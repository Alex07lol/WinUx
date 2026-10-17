#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=lib.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"
skip_exit "PCI enumeration is not implemented yet (Phase 9); no test is claimed"
