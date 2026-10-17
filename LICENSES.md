# Third-party components, licenses and provenance

This file records external inputs and tools. It is not legal advice; material
licensing decisions need appropriate review. No Linux kernel, Windows, Wine,
ReactOS or NVIDIA implementation source is copied into this repository.

## Project source

| Component | License | Notes |
|---|---|---|
| PROJECT_OS kernel, `os_core`, scripts, docs, 5x7 font, dashboard design | MIT (`LICENSE`) | Original work created for this project |

There are **no Rust crate dependencies**; the workspace uses only `core`/`std`
from the pinned toolchain. The 5x7 font was authored for this project, so no font
license applies.

## Bootloader

| Component | Version | License | Provenance / obligations |
|---|---|---|---|
| Limine binaries (`limine-bios.sys`, `limine-bios-cd.bin`, `limine-uefi-cd.bin`, `BOOTX64.EFI`, Windows host tool) | 12.9.3 | BSD-2-Clause | <https://github.com/Limine-Bootloader/Limine/releases/tag/v12.9.3>; SHA-256 `e02a473c6dc6df25159697f94930272397f21fd86e5d4eeee61bba72c95ec8d2`; `LICENSE` and `3RDPARTY.md` are copied next to the binaries. Binaries are downloaded, Git-ignored and not committed. Redistribution must retain the copyright/conditions and bundled third-party notices. See `boot/limine.lock`. |

Limine's bundled `3RDPARTY.md` contains notices for third-party code included in
the bootloader binaries; it is preserved by `scripts/fetch_limine.sh`. Because
the generated ISO redistributes those binaries, `scripts/make_iso.sh` includes
`licenses/LICENSE`, `licenses/LICENSES.md`, `licenses/limine-LICENSE`,
`licenses/limine-3RDPARTY.md` and `licenses/rust/*` inside the ISO. No Limine
binary is committed to Git.

## Host build and test tools (not distributed with the ISO)

| Tool | Version used | License | Role |
|---|---|---|---|
| Rust / Cargo / rust-lld | 1.99.1, commit `58cf8f9c35b355199727627d07ae34a4e2846d67` | MIT OR Apache-2.0 | compile/link kernel and host tests; `core`/`compiler_builtins` are linked into the kernel, and their notices ship in the ISO |
| cdrtools `mkisofs` | 3.02a09 | CDDL-1.0 / GPL-2.0-only / LGPL-2.1-only (package metadata) | create ISO; not linked into the OS |
| QEMU | 11.1.0 | GPL-2.0-only | development boot test emulator; not distributed |
| Python | 3.12 | PSF License | stdlib-only ISO catalog verification |
| Git, Bash, GNU coreutils | host versions | GPL-compatible host licenses | development tooling |
| VirtualBox (optional) | 7.2.20r175154 | GPL-2.0 (VirtualBox base) | compatibility boot test; installed locally, not distributed or linked |

The build tools themselves are not included in the ISO. The kernel does link
Rust's `core` and `compiler_builtins`; Rust is MIT OR Apache-2.0. The ISO
includes `licenses/rust/COPYRIGHT-library.html` and the accompanying license
texts under `/licenses/rust` to satisfy the notice requirements for that linked
code. Package-manager metadata was used for the cdrtools/QEMU license
identification; verify exact upstream files before redistributing those tools.

## Specifications and documentation consulted (no code copied)

- Limine boot protocol, usage and configuration (BSD-2-Clause project docs).
- Intel 64 and IA-32 Software Developer Manuals (architecture reference).
- UEFI Forum specifications; TianoCore edk2 `ElTorito.h` (BSD-2-Clause-Patent)
  used only to confirm the El Torito section-header identifier.
- Microsoft PE/COFF, Win32 API, process/thread and WDDM documentation.
- NVIDIA open GPU kernel module README and Nouveau/Mesa NVK documentation.
- Linux memory-management documentation and license rules (design study only;
  Linux kernel code is GPL-2.0-only and was not copied).
- ReactOS architecture overview (scope/compatibility research only; no code).

Links and conclusions are recorded in `docs/RESEARCH.md`.

## Repository hygiene

- Downloaded bootloader archives/binaries: `third_party/` and `boot/limine/` are
  Git-ignored.
- Generated kernels, ISOs, staging directories and logs: `artifacts/` is
  Git-ignored.
- No secrets, API keys or credentials are used or stored.
- Adding a dependency requires updating this file with source, exact version,
  license, checksum and purpose before merge.
