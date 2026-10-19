#!/usr/bin/env bash
set -euo pipefail
# shellcheck source=lib.sh
source "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/lib.sh"

BUILD=1
INTERACTIVE=0
INPUT_FIXTURE=0
for argument in "$@"; do
    case "$argument" in
        --no-build) BUILD=0 ;;
        --interactive)
            # Strip the isa-debug-exit cmdline flag so the kernel reaches the
            # interactive shell instead of terminating at the success marker.
            INTERACTIVE=1
            ;;
        --input-fixture)
            # Add the console-input fixture module. The ISO keeps debug_exit:
            # the harness drives one typed line during the user phase and the
            # boot still ends at the success marker.
            INPUT_FIXTURE=1
            ;;
        *)
            die "unknown argument: $argument (use --no-build, --interactive and/or --input-fixture)"
            ;;
    esac
done
if [ "$INTERACTIVE" -eq 1 ] && [ "$INPUT_FIXTURE" -eq 1 ]; then
    die "--interactive and --input-fixture are separate ISO variants; pick one"
fi

OUT_ISO="$ISO_PATH"
if [ "$INTERACTIVE" -eq 1 ]; then
    OUT_ISO="$ROOT_DIR/artifacts/project-os-interactive.iso"
elif [ "$INPUT_FIXTURE" -eq 1 ]; then
    OUT_ISO="$ROOT_DIR/artifacts/project-os-input.iso"
fi

have mkisofs || die "mkisofs not found (scoop install cdrtools / apt install genisoimage or xorriso)"
if [ "$BUILD" -eq 1 ]; then
    "$ROOT_DIR/scripts/build.sh"
fi
[ -f "$KERNEL_ARTIFACT" ] || die "kernel artifact missing: $KERNEL_ARTIFACT"

# Ring-3 programs are Limine modules on the ISO. Mirror the Limine fetch
# behavior: build them on demand, and never produce a module-less ISO.
USER_ELFS=""
for name in hello fault bad; do
    artifact="$ROOT_DIR/artifacts/$name.elf"
    if [ ! -f "$artifact" ]; then
        log "userland artifact missing ($artifact); building"
        "$ROOT_DIR/scripts/build_user.sh"
        break
    fi
done
for name in hello fault bad; do
    artifact="$ROOT_DIR/artifacts/$name.elf"
    [ -f "$artifact" ] || die "userland artifact missing: $artifact"
    USER_ELFS="$USER_ELFS $artifact"
done

# PE32+ fixtures (Phase 7, stages 1-3; Phase 8 stage 2) are modules too; build
# on demand and never produce an ISO that references a missing .exe module.
# The console-input fixture is opt-in so the default ISO's module counts and
# serial assertions stay exactly as stages 1-3 recorded them.
PE_NAMES="pehello pebad peconsole pebadimp pebadfn pebasics pebadfn2"
if [ "$INPUT_FIXTURE" -eq 1 ]; then
    PE_NAMES="$PE_NAMES peinput"
fi
USER_PES=""
for name in $PE_NAMES; do
    artifact="$ROOT_DIR/artifacts/$name.exe"
    if [ ! -f "$artifact" ]; then
        log "PE fixture missing ($artifact); building"
        "$ROOT_DIR/scripts/build_user_pe.sh"
        break
    fi
done
for name in $PE_NAMES; do
    artifact="$ROOT_DIR/artifacts/$name.exe"
    [ -f "$artifact" ] || die "PE fixture missing: $artifact"
    USER_PES="$USER_PES $artifact"
done

if [ ! -f "$ROOT_DIR/boot/limine/limine-bios.sys" ]; then
    "$ROOT_DIR/scripts/fetch_limine.sh"
fi
for asset in limine-bios.sys limine-bios-cd.bin limine-uefi-cd.bin BOOTX64.EFI; do
    [ -f "$ROOT_DIR/boot/limine/$asset" ] || die "missing Limine asset: boot/limine/$asset"
done

STAGE="$ROOT_DIR/artifacts/iso_root"
rm -rf "$STAGE"
mkdir -p "$STAGE/boot" "$STAGE/EFI/BOOT" "$STAGE/licenses/rust"
cp "$KERNEL_ARTIFACT" "$STAGE/boot/kernel.elf"
for artifact in $USER_ELFS; do
    cp "$artifact" "$STAGE/boot/$(basename "$artifact")"
done
for artifact in $USER_PES; do
    cp "$artifact" "$STAGE/boot/$(basename "$artifact")"
done
cp "$ROOT_DIR/boot/limine.conf" "$STAGE/limine.conf"
if [ "$INTERACTIVE" -eq 1 ]; then
    sed -i 's/ debug_exit//' "$STAGE/limine.conf"
fi
if [ "$INPUT_FIXTURE" -eq 1 ]; then
    sed -i 's|    module_path: boot():/boot/pebadfn2.exe|    module_path: boot():/boot/pebadfn2.exe\n    module_path: boot():/boot/peinput.exe|' "$STAGE/limine.conf"
    grep -q 'module_path: boot():/boot/peinput.exe' "$STAGE/limine.conf" ||
        die "failed to add the peinput.exe module to limine.conf"
fi
cp "$ROOT_DIR/boot/limine/limine-bios.sys" "$STAGE/"
cp "$ROOT_DIR/boot/limine/limine-bios-cd.bin" "$STAGE/"
cp "$ROOT_DIR/boot/limine/limine-uefi-cd.bin" "$STAGE/"
cp "$ROOT_DIR/boot/limine/BOOTX64.EFI" "$STAGE/EFI/BOOT/"

# The ISO redistributes the kernel and Limine binaries, so include the project
# license and Limine's BSD-2-Clause notice plus its bundled third-party notices.
for notice in "$ROOT_DIR/LICENSE" "$ROOT_DIR/LICENSES.md"; do
    [ -f "$notice" ] || die "license notice missing: $notice"
    cp "$notice" "$STAGE/licenses/"
done
for notice in LICENSE 3RDPARTY.md; do
    [ -f "$ROOT_DIR/boot/limine/$notice" ] || die "Limine notice missing: boot/limine/$notice"
    cp "$ROOT_DIR/boot/limine/$notice" "$STAGE/licenses/limine-$notice"
done

# The kernel links Rust core/compiler_builtins. Include the Rust library
# copyright and license texts that accompany a binary distribution.
[ -d "$ROOT_DIR/licenses/rust" ] || die "Rust license notices missing: licenses/rust"
cp -R "$ROOT_DIR/licenses/rust/." "$STAGE/licenses/rust/"

rm -f "$OUT_ISO"
VARIANT_NOTE=""
if [ "$INTERACTIVE" -eq 1 ]; then
    VARIANT_NOTE=" (interactive shell build)"
elif [ "$INPUT_FIXTURE" -eq 1 ]; then
    VARIANT_NOTE=" (console-input fixture build)"
fi
log "creating ISO with BIOS + UEFI El Torito entries${VARIANT_NOTE}"
# cdrtools mkisofs spells the EFI platform id numerically (0xef = 239); it does
# not accept the string "efi" that xorriso accepts.
mkisofs \
    -R -r -J \
    -V PROJECT_OS \
    -b limine-bios-cd.bin \
    -no-emul-boot -boot-load-size 4 -boot-info-table \
    -eltorito-alt-boot \
    -eltorito-platform 239 \
    -b limine-uefi-cd.bin \
    -no-emul-boot \
    -o "$OUT_ISO" \
    "$STAGE" >/dev/null

if LIMINE_TOOL=$(find_limine_tool); then
    if "$LIMINE_TOOL" bios-install --force "$OUT_ISO" >/tmp/project-os-limine-install.log 2>&1; then
        log "Limine hybrid MBR installed (ISO also bootable as a disk image)"
    else
        warn "limine bios-install failed; El Torito ISO boot is unaffected"
        sed 's/^/  /' /tmp/project-os-limine-install.log >&2 || true
    fi
else
    warn "Limine host tool unavailable; skipped hybrid MBR install (El Torito boot unaffected)"
fi

if have python; then
    python "$ROOT_DIR/scripts/verify_iso.py" "$OUT_ISO"
else
    warn "python unavailable; El Torito catalog verification skipped"
fi

[ -s "$OUT_ISO" ] || die "ISO was not created"
log "ISO: $OUT_ISO"
log "size: $(wc -c < "$OUT_ISO") bytes"
log "sha256: $(sha256sum "$OUT_ISO" | awk '{print $1}')"
