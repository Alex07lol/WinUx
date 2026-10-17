# Progress

Last updated: **2026-10-17**

## Current milestone

Phase 0 complete; Phase 1 boot milestone implemented and verified under QEMU
(BIOS, three RAM sizes) **and VirtualBox (BIOS)**. Phase 2 and Phase 3 have
working foundations (descriptor tables, exceptions, legacy timer, physical frame
allocator, narrow MMIO page-table mapping) but are not complete.

This is **not** a Windows-compatible OS, not a desktop, and not a GPU-capable OS.

## Repository state

- Workspace root: this thread's local checkout.
- Git: initialized on branch `main`; **no commits** yet. All project files are
  untracked; nothing was reset or discarded.
- Generated inputs (`target/`, `third_party/`, `boot/limine/`, `artifacts/*`) are
  Git-ignored.
- Case-insensitive host: `agents.md`, `STOP.md` and `PROGRESS.md` resolve to
  `AGENTS.md`, `stop.md` and `progress.md`; there are no divergent duplicates.
- `stop.md` currently says `STOP=NO`.
- VirtualBox 7.2.20 with `VBoxManage` is installed on this host for the
  isolated boot test; no host driver or partition changes were made.

## Implemented in this session

- Rust workspace (`kernel`, `os_core`, `tools/membench`) with a pinned
  `x86_64-unknown-none` target and static-relocation linker flags.
- Limine protocol base revision 6 request block (bootloader info, firmware type,
  stack size, paging mode, HHDM, cmdline, executable address, memory map,
  framebuffer, TSC frequency) in a writable `.limine_requests` section.
- Assembly entry stub, COM1 serial logging, panic handler.
- Boot dashboard on the Limine framebuffer with an original 5x7 font.
- Memory-map copy/validation and accounting, with reserved-region safety.
- Physical frame allocator (bitmap, usable RAM only, low-1 MiB guard, contiguous
  power-of-two blocks, strict free validation) plus in-kernel HHDM self-test.
- GDT/TSS/IST1, 32 exception gates, Rust frame dispatcher, deliberate `int3`
  self-test, fatal exception diagnostics.
- 8259 remap/mask, 8254 PIT at 100 Hz (mode 2), IRQ0 dispatch with 5-tick
  verification.
- Narrow MMIO page-table mapping (`arch/x86_64/paging.rs`) through Limine's live
  4-level tables, plus xAPIC LINT0 ExtINT bring-up (`arch/x86_64/apic.rs`). This
  fixed VirtualBox, where IRQ0 stayed pending at the PIC because LINT0 was
  masked and the APIC page is not part of Limine's HHDM.
- BIOS+UEFI hybrid ISO generation with pinned Limine assets, SHA-256 verification,
  a stdlib El Torito catalog verifier, and a hybrid-MBR install on the ISO file.
- Build/ISO/unit/lint/format/boot/memory/graphics/VirtualBox/benchmark scripts,
  plus a per-memory-size boot matrix (256 MiB, 512 MiB, 1 GiB).
- ISO license notices for the redistributed Limine binaries, linked Rust
  runtime and project code.

## Exact commands and results

| Command | Result |
|---|---|
| `cargo build -p project_os_kernel --target x86_64-unknown-none --release` | PASS |
| `cargo test -p os_core` | PASS: 33 passed, 0 failed |
| `cargo clippy` (os_core, membench, kernel) with `-D warnings` | PASS |
| `cargo fmt --all -- --check` | PASS |
| `scripts/fetch_limine.sh` | PASS: checksum verified, assets installed |
| `scripts/make_iso.sh` | PASS: ISO built, BIOS+UEFI catalog entries verified |
| `scripts/test_boot_qemu.sh` (BIOS, 256 MiB / 512 MiB / 1 GiB) | PASS: marker reached, QEMU exit 33 at each size |
| `scripts/test_memory.sh` | PASS: host allocator tests + in-VM self-test |
| `scripts/test_graphics_host.sh` | PASS: framebuffer tests |
| `scripts/test_all.sh` | PASS: 13 PASS, 0 FAIL, 4 SKIP |
| `scripts/test_boot_qemu.sh --firmware uefi` | SKIP: no `OVMF_FD` / OVMF firmware |
| `scripts/test_virtualbox.sh artifacts/project-os.iso` | PASS (VirtualBox 7.2.20r175154): marker reached, 5 timer ticks, VM deleted |
| `scripts/test_pci.sh` | SKIP: PCI not implemented |
| `scripts/test_driver_model.sh` | SKIP: driver framework not implemented |
| `scripts/test_gpu_vm.sh` | SKIP: framebuffer-only, no virtual GPU driver |
| `scripts/bench_graphics.sh` | SKIP: no accelerated graphics |
| Corrupt-ISO negative boot test | PASS (expected failure): exit 1, marker absent |

Recorded QEMU boot log ends with:

```text
frame allocator: 130029 free / 16777216 frames (65536 MiB managed), largest run 130029 frames
frame allocator selftest: passed
gdt: loaded (kernel code/data, TSS, IST1 double-fault stack)
idt: loaded (32 exception gates + IRQ0)
selftest: int3 exception handled (rip=0xffffffff80007dd3)
exception selftest: passed
pit: 100 Hz, IRQ0 unmasked
timer: 5 ticks observed
PROJECT_OS_BOOT_SUCCESS:dev checks=handoff,memmap,gdt,idt,exceptions,timer,allocator
```

Build identifier is `dev` because the repository has no commit yet; the build
uses a source-derived ID when `PROJECT_OS_BUILD_ID` or a Git commit exists.

## Artifact

| Item | Value |
|---|---|
| Kernel ELF | `artifacts/kernel.elf`, 295,960 bytes |
| Kernel SHA-256 | `ea5f608df34009fea9e3c6fcfa440967e3b67d69ecb3807aa23c666242695f36` |
| Kernel ELF entry | `0xffffffff80000000` (verified with `readelf`) |
| ISO | `artifacts/project-os.iso` |
| ISO size | 6,285,312 bytes |
| ISO SHA-256 | `2d919bb994aa3d2628e8df3c32339bc7a051d8bf6ebd4f48a2df02dcbc190336` |
| ISO boot entries | BIOS platform 0x00 (4 sectors), UEFI platform 0xEF (6144 sectors), both no-emulation, verified by `scripts/verify_iso.py` |
| Serial logs | `artifacts/qemu-serial-256m.log`, `-512m.log`, `-1g.log`, `artifacts/qemu-serial.log` (memory test), `artifacts/vbox-serial.log` (VirtualBox) |
| VirtualBox result | `artifacts/vbox-test.log`: VirtualBox 7.2.20 BIOS PASS, marker and 5 ticks observed |

ISO byte-for-byte reproducibility is not claimed: `mkisofs` embeds file
timestamps, so a rebuild changes the ISO checksum even with unchanged sources.
The kernel build is deterministic for identical sources, flags and build ID.
The checksums above are from the final `scripts/test_all.sh` run after the
allocator hardening, ISO-license-notice changes and the VirtualBox APIC fix. The ISO now includes
`/licenses/LICENSE`, `/licenses/LICENSES.md`, `/licenses/limine-LICENSE`,
`/licenses/limine-3RDPARTY.md` and `/licenses/rust/*`, because it redistributes
the kernel (including linked Rust runtime code) and Limine binaries.

## Benchmarks

`scripts/bench_memory.sh --mib 64 --iterations 200000 --seed 8388610`, release
host build (Rust 1.99.1, commit 58cf8f9c, `opt-level=2`, `debug=1`, thin LTO, `panic=abort`),
Windows x86-64: 200,200 ops, 2.970 ms, mean 14.8 ns/op, p50 14 ns,
p99 17 ns, accounting restored, fragmentation ratio 1.0000 in this workload.
This is a host model, not a kernel or Linux comparison.

## Decisions

- [docs/DECISIONS.md](docs/DECISIONS.md): Limine/base revision 6, Rust target,
  modular monolith, usable-RAM bitmap allocator, legacy PIC/PIT bring-up, APIC
  MMIO mapping + LINT0 ExtINT, serial-authoritative dashboard, cdrtools ISO
  path, gated debug-exit, original font, case-insensitive control-file policy.
- [docs/RESEARCH.md](docs/RESEARCH.md): primary sources, findings, confidence and
  open questions.
- [docs/MEMORY_MANAGEMENT.md](docs/MEMORY_MANAGEMENT.md): invariants, tests,
  benchmark methodology.
- [docs/GRAPHICS.md](docs/GRAPHICS.md) and
  [docs/WINDOWS_COMPATIBILITY.md](docs/WINDOWS_COMPATIBILITY.md): explicit
  non-claims and future architecture.

## Open defects and blockers

- VirtualBox BIOS boot is now verified on the default VM configuration. The
  root cause of the earlier stall was an IRQ0 pending at the PIC while LINT0
  was masked; the kernel now maps the APIC page and configures LINT0 ExtINT.
  VirtualBox EFI boot is still untested.
- UEFI boot has not been run (no OVMF firmware). The UEFI El Torito entry is
  verified; a catalog entry is not a boot test.
- No user mode, syscalls, process model, VFS, networking, PCI, driver model,
  Windows PE/API compatibility or GPU acceleration.
- Kernel runs on bootloader page tables; the new mapper only installs permanent
  MMIO leaves and does not split huge pages, unmap, or support 5-level paging.
  There is no own VM manager or kernel heap yet.
- The legacy PIC/PIT path uses LINT0 ExtINT and does not start the APIC timer,
  IOAPIC or SMP. Those remain future work.
- QEMU boots were verified at 256 MiB, 512 MiB and 1 GiB; the log shows usable
  RAM scaling (252 MiB, 508 MiB, 1020 MiB), so the allocator was exercised with
  different memory-map sizes. Larger sizes and NUMA hotplug are not tested.
- No CI workflow is included: this workspace has no Git remote/hosting. The
  scripts are CI-ready, and a runner would need Rust, `mkisofs`, Python and QEMU.
- ISO bytes vary with timestamps; no `SOURCE_DATE_EPOCH`-pinned ISO yet.
- Spin locks are not interrupt-safe; no interrupt handler takes one.

## Single best next action

Add a pinned-OVMF **UEFI boot** run (`OVMF_FD=... scripts/test_boot_qemu.sh
--firmware uefi`) and record the serial marker. That is now the only Phase 1
boot path still untested; the VirtualBox BIOS path just closed. After that, the
next engineering milestone is the kernel heap and own page-table manager
(Phase 3/4) on top of the narrow MMIO mapper.
