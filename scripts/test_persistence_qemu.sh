#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=lib.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"

# Four-boot durability test for the checkpoint VFS (docs/PERSISTENCE.md).
# Boots an interactive ISO four times against one raw disk image and asserts
# load/save/fallback/format behavior. Uses `-machine pc` for the PIIX legacy
# IDE ports (docs/RESEARCH.md R-009), unlike the other QEMU tests.

INTERACTIVE_ISO="$ROOT_DIR/artifacts/project-os-interactive.iso"
DISK_IMAGE="$ROOT_DIR/artifacts/persist-disk.img"
QEMU_LOG="$ROOT_DIR/artifacts/qemu-persist.log"

[ -f "$KERNEL_ARTIFACT" ] || die "kernel artifact missing; run scripts/build.sh first"
have python || skip_exit "python unavailable; persistence test not run"

log "building interactive ISO (debug_exit stripped from the cmdline)"
"$ROOT_DIR/scripts/make_iso.sh" --no-build --interactive

if ! QEMU=$(find_qemu); then
    skip_exit "QEMU unavailable; persistence test not run"
fi

python "$ROOT_DIR/scripts/test_persistence_qemu.py" \
    "$(winpath "$QEMU")" \
    "$(winpath "$INTERACTIVE_ISO")" \
    "$(winpath "$DISK_IMAGE")" \
    "$(winpath "$QEMU_LOG")"

log "PASS: checkpoint persistence over four boots"
log "session logs: $ROOT_DIR/artifacts/qemu-persist-boot{1,2,3,4}.log"
