# Testing

No test below is marked passing unless it was executed and its output observed.
Skipped tests are listed separately from passes.

## Environment used for the recorded results

| Item | Value |
|---|---|
| Host | Windows 10/11 (MSYS/Git Bash), x86-64 |
| Rust | stable 1.99.1 (commit 58cf8f9c), target `x86_64-unknown-none` |
| QEMU | 11.1.0 (`qemu-system-x86_64.exe`) |
| mkisofs | cdrtools 3.02a09 |
| Limine | 12.9.3, SHA-256 pinned |
| QEMU machine | `q35`, 512 MiB RAM, 1 CPU, `-display none`, `-monitor none` |
| VirtualBox | 7.2.20r175154 (`VBoxManage` 7.2.20) |
| OVMF | not available |

## Entry point

```sh
scripts/test_all.sh
```

It runs every feasible check and prints a summary with `PASS`, `FAIL` and `SKIP`.
Exit codes used by individual scripts: `0` = pass, `2` = skipped (tool/feature
missing), any other = failure. `test_all.sh` treats `2` as SKIP, never PASS.

## Commands and what they prove

| Command | Proves | Recorded result |
|---|---|---|
| `scripts/test_all.sh` | orchestrates everything below and summarises | PASS: 13 PASS, 0 FAIL, 4 SKIP |
| `scripts/bootstrap.sh` | required tools and Rust target exist | PASS |
| `scripts/format.sh` | `rustfmt --check` clean | PASS |
| `scripts/lint.sh` | Clippy `-D warnings` for os_core, membench, kernel | PASS |
| `scripts/test_unit.sh` | 33 host tests for memmap, framebuffer, allocator, PRNG, paging math | PASS (33/33) |
| `scripts/build.sh` | kernel builds; ELF entry and Limine requests verified | PASS |
| `scripts/make_iso.sh` | ISO built; both El Torito entries verified | PASS |
| `scripts/test_boot_qemu.sh` | BIOS boot at 256 MiB, 512 MiB and 1 GiB; marker; no panic; debug-exit 33 | PASS (all three) |
| `scripts/test_boot_qemu.sh --firmware uefi` | UEFI boot | SKIP (no `OVMF_FD`) |
| `scripts/test_memory.sh` | allocator host tests + in-VM allocator self-test | PASS (both) |
| `scripts/test_graphics_host.sh` | framebuffer geometry/colour tests | PASS |
| `scripts/test_virtualbox.sh` | VirtualBox BIOS boot | PASS (7.2.20; marker + 5 ticks) |
| `scripts/test_pci.sh` | PCI enumeration | SKIP (not implemented, Phase 9) |
| `scripts/test_driver_model.sh` | driver lifecycle model | SKIP (not implemented, Phase 9) |
| `scripts/test_gpu_vm.sh` | virtual GPU driver | SKIP (framebuffer only) |
| `scripts/bench_graphics.sh` | accelerated graphics benchmark | SKIP (no implementation) |
| `scripts/bench_memory.sh` | host allocator performance | PASS (measurement, not a correctness gate) |

## QEMU boot test details

The runner invokes approximately:

```sh
qemu-system-x86_64 \
  -machine q35 -m 512M -smp 1 \
  -cdrom artifacts/project-os.iso -boot d \
  -display none -monitor none \
  -serial file:artifacts/qemu-serial-512m.log \
  -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
  -no-reboot
```

The kernel prints a unique marker only after all checks:

```text
PROJECT_OS_BOOT_SUCCESS:<build-id> checks=handoff,memmap,gdt,idt,exceptions,timer,allocator
```

Failure conditions:

- marker absent within the timeout;
- `PROJECT_OS_PANIC` or `PROJECT_OS_FATAL` present;
- QEMU does not exit after the marker;
- QEMU exit status is not 33.

The script copies the serial log to `artifacts/qemu-serial.log` by default; use
`--serial-log <path>` to keep it elsewhere. `test_all.sh` records one log per
RAM size (`qemu-serial-256m.log`, `-512m.log`, `-1g.log`). The memory sizes are
selected with `--memory` (default 512M). A corrupt-ISO negative test was run:
the marker was absent and the script exited 1.

### Exit status 33

The kernel writes `0x10` to I/O port `0xf4` only when its cmdline contains
`debug_exit`. QEMU's `isa-debug-exit` device maps the value to
`(value << 1) | 1` = 33. Without the device (VirtualBox), the write is ignored
and the kernel idles, so the same ISO can be booted manually.

## What the kernel verifies before the marker

1. Limine base revision 6 was honoured.
2. Memory-map response exists, is sorted, usable regions do not overlap, no
   address overflow, at least some usable RAM, usable regions page-aligned.
3. Framebuffer geometry (if present) validates: non-zero dimensions, supported
   bpp, pitch >= visible line, no size overflow.
4. The memory map was copied into kernel-owned storage.
5. Frame allocator initialised from usable RAM only; low 1 MiB reserved.
6. Allocator self-test: single and 4-frame allocation, natural alignment,
   HHDM write/read-back, free, accounting restored, double-free rejected,
   non-power-of-two rejected.
7. GDT/TSS/IST1 loaded; IDT loaded; deliberate `int3` reached the Rust handler.
8. PIC remapped/masked, PIT programmed, IRQ0 unmasked, the APIC MMIO page mapped,
   and LINT0 left in ExtINT delivery so the 8259 can reach the CPU.
9. Five PIT ticks observed within a 2 s TSC-based bound (or bounded spin fallback).

## VirtualBox

`scripts/test_virtualbox.sh` **was executed here and passed** with VirtualBox
7.2.20r175154 on the default VM profile (BIOS, 512 MiB, one CPU, PIIX3). The
first run exposed a real kernel defect rather than a VirtualBox bug: IRQ0 was
pending in the PIC (`IRR=0x01`, `ISR=0x00`) while IF=1, because LINT0 was
masked and the APIC MMIO page is not part of Limine's HHDM. The kernel now maps
that page through the live 4-level tables and configures LINT0 ExtINT; the
serial log then shows `timer: 5 ticks observed` and the success marker.

The script is deliberately defensive:

- creates a VM named `PROJECT_OS-test-<pid>` in `artifacts/vbox`;
- attaches only the generated ISO, no host disk;
- captures COM1 to a file;
- on completion or failure, powers off and deletes **only that VM**;
- refuses to reuse an existing VM name;
- if `VBoxManage` is missing, prints exact manual steps and exits 2.

Manual settings for a BIOS boot: Other/Unknown 64-bit, 512 MiB, no hard disk,
optical drive attached to the ISO, COM1 raw-file logging. For EFI, enable EFI in
System → Motherboard. A VirtualBox result must be recorded separately from QEMU.

## UEFI

The ISO is verified to contain a UEFI El Torito entry (platform 0xEF, no
emulation, 6144 sectors). **That is not a UEFI boot test.** To run one:

```sh
OVMF_FD=/path/to/OVMF.fd scripts/test_boot_qemu.sh --firmware uefi
```

Install/obtain OVMF from a trusted, pinned source; record its version and checksum
in `progress.md` before claiming a UEFI boot.

## Troubleshooting

| Symptom | Likely cause | Action |
|---|---|---|
| `cannot find linker script` | Cargo not invoked from the workspace root | run scripts from the repo root |
| `rust-lld`/target missing | target not installed | `rustup target add x86_64-unknown-none` |
| ISO not found | build skipped | `scripts/make_iso.sh` |
| No marker, no serial output | wrong COM1 configuration or boot failure before serial init | check QEMU log; try `-d int` or a graphical run |
| QEMU does not exit | cmdline lacks `debug_exit` or device flag missing | use the provided script |
| Exit status 1 from the boot script | marker absent/panic | inspect `artifacts/qemu-serial.log` |
| Host-policy toolchain update | `rust-toolchain.toml` uses the `stable` channel | pin an exact version in CI if required |

## Evidence limitations

- QEMU is TCG software emulation on this host; timings are not hardware timings.
- QEMU BIOS was exercised at 256 MiB, 512 MiB and 1 GiB. Larger sizes,
  hotplug/NUMA and physical hardware were not run.
- VirtualBox BIOS is now verified; VirtualBox EFI, UEFI/OVMF, real hardware, SMP,
  user mode, Windows apps and GPUs are untested or unimplemented and are not
  claimed.
