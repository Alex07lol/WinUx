#!/usr/bin/env bash
# Shared helpers for PROJECT_OS scripts. Source this file; do not execute it.
# All callers use `set -euo pipefail`.

SCRIPT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
ROOT_DIR=$(cd "$SCRIPT_DIR/.." && pwd)
cd "$ROOT_DIR"

PROJECT_NAME="PROJECT_OS"
KERNEL_TARGET="x86_64-unknown-none"
KERNEL_BIN="project_os_kernel"
ISO_PATH="$ROOT_DIR/artifacts/project-os.iso"
KERNEL_ARTIFACT="$ROOT_DIR/artifacts/kernel.elf"
LIMINE_VERSION="12.9.3"
LIMINE_URL="https://github.com/Limine-Bootloader/Limine/releases/download/v${LIMINE_VERSION}/limine-binary.tar.xz"
LIMINE_SHA256="e02a473c6dc6df25159697f94930272397f21fd86e5d4eeee61bba72c95ec8d2"

log() { printf '%s\n' "$*"; }
warn() { printf 'WARN: %s\n' "$*" >&2; }
die() { printf 'ERROR: %s\n' "$*" >&2; exit 1; }
skip_exit() { printf 'SKIP: %s\n' "$*" >&2; exit 2; }
have() { command -v "$1" >/dev/null 2>&1; }

# Native tools on Windows need Windows paths for file arguments; MSYS/Git Bash
# otherwise rewrites or rejects POSIX paths inconsistently.
winpath() {
    if have cygpath; then
        cygpath -w "$1"
    else
        printf '%s' "$1"
    fi
}

find_qemu() {
    if [ -n "${QEMU_SYSTEM_X86_64:-}" ]; then
        [ -x "$QEMU_SYSTEM_X86_64" ] && { printf '%s' "$QEMU_SYSTEM_X86_64"; return 0; }
        return 1
    fi
    if have qemu-system-x86_64; then
        command -v qemu-system-x86_64
        return 0
    fi
    for candidate in \
        "$HOME/scoop/apps/qemu/current/qemu-system-x86_64.exe" \
        "/c/Program Files/qemu/qemu-system-x86_64.exe" \
        "/usr/bin/qemu-system-x86_64"; do
        if [ -x "$candidate" ]; then
            printf '%s' "$candidate"
            return 0
        fi
    done
    return 1
}

find_vboxmanage() {
    if [ -n "${VBOXMANAGE:-}" ]; then
        [ -x "$VBOXMANAGE" ] && { printf '%s' "$VBOXMANAGE"; return 0; }
        return 1
    fi
    if have VBoxManage; then
        command -v VBoxManage
        return 0
    fi
    for candidate in \
        "/c/Program Files/Oracle/VirtualBox/VBoxManage.exe" \
        "/usr/bin/VBoxManage"; do
        if [ -x "$candidate" ]; then
            printf '%s' "$candidate"
            return 0
        fi
    done
    return 1
}

find_limine_tool() {
    if have limine; then
        command -v limine
        return 0
    fi
    if [ -x "$ROOT_DIR/boot/limine/limine.exe" ]; then
        printf '%s' "$ROOT_DIR/boot/limine/limine.exe"
        return 0
    fi
    return 1
}
