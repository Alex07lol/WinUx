#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=lib.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"

have python || skip_exit "python is not installed; El Torito catalog verification skipped"
[ -f "$ISO_PATH" ] || die "ISO not found: $ISO_PATH (run scripts/make_iso.sh)"
python "$ROOT_DIR/scripts/verify_iso.py" "$ISO_PATH"
log "PASS: ISO contains BIOS and UEFI El Torito boot entries"
