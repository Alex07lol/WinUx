# Roadmap

Statuses are evidence-based. A phase is complete only when its acceptance tests
were run and observed. No dates or completion percentages are implied.

| Phase | Scope | Status |
|---|---|---|
| 0 | repository audit, toolchain, engineering controls, initial docs | **complete** (2026-10-17) |
| 1 | own kernel, Limine boot, serial/framebuffer, memory map, ISO, boot tests | **complete for available tooling** — QEMU BIOS (256 MiB/512 MiB/1 GiB) and VirtualBox BIOS verified; UEFI boot verification outstanding |
| 2 | GDT/IDT/exceptions, timer, diagnostics | **partial** — GDT/TSS/IST, 32 exception gates, int3 self-test, PIT IRQ0 and xAPIC LINT0 ExtINT work; APIC timer/IOAPIC, SMP, recoverable exceptions not started |
| 3 | physical/virtual memory, page tables, permissions | **partial** — usable-RAM bitmap frame allocator with host/in-VM tests and a narrow MMIO leaf mapper; no general page-table manager or user address spaces |
| 4 | kernel heap, slab/size classes, performance harness | not started |
| 5 | tasks, user mode, syscalls, handles/IPC, isolation | not started |
| 6 | VFS, storage, console/shell, persistence, networking | not started |
| 7 | PE loader and minimal native Windows console compatibility | not started |
| 8 | larger Win32/desktop, registry-like config, GUI/text/input | not started |
| 9 | graphics/driver framework, PCI, MMIO/DMA/IRQ ownership, VM display | not started (framebuffer only) |
| 10 | one scoped open-source GPU acceleration target or documented block | not started (research only) |
| 11 | Windows WDDM/driver compatibility prototype or documented block | not started (research only) |

## Phase 0 acceptance (met)

An explainable plan, pinned toolchain/bootloader, reproducible prerequisite check
and canonical control files exist. No destructive host changes were made.

## Phase 1 acceptance (met with skips)

- Own x86-64 kernel source and ELF64 image built from source.
- Limine loads it; base revision 6 verified at runtime.
- Serial and framebuffer status output; memory map copied and summarised.
- BIOS+UEFI hybrid ISO created; both El Torito entries verified.
- QEMU BIOS boot reaches `PROJECT_OS_BOOT_SUCCESS:<id>` and exits 33 via
  `isa-debug-exit`; panic/fatal detection and negative test work.
- VirtualBox 7.2.20 BIOS boot reaches the same marker with 5 PIT ticks on the
  default VM profile; the isolated script creates and removes only its own VM.
- UEFI ISO entry verified but **UEFI boot SKIP** (no OVMF).
- Host tests (33), formatting, lint, ISO creation and memory tests pass.

Remaining Phase 1 evidence: run a UEFI QEMU boot with pinned OVMF and record it
in `progress.md`. VirtualBox EFI is a secondary follow-up.

## Phase 2 acceptance (partial)

Met: own GDT/TSS/IST, IDT with exception gates, fatal exception reporting,
breakpoint self-test, PIT periodic interrupt at 100 Hz with observed ticks in
QEMU and VirtualBox, APIC MMIO mapping and LINT0 ExtINT for legacy PIC delivery.

Missing: APIC timer/IOAPIC, SMP bring-up, interrupt-safe locking, recoverable
page-fault handling, interrupt-driven input, diagnostic dumps for every vector,
TSC calibration independent of the bootloader.

## Phase 3 acceptance (partial)

Met: physical frame allocator restricted to usable RAM, low-memory reservation,
contiguous aligned allocation, strict free validation, host tests and in-kernel
HHDM read-back self-test, accounting restored after churn, and a narrow
uncached MMIO leaf mapper with host-tested 4-level index/entry math.

Missing: kernel-owned page tables, map/unmap helpers with permission checks,
W^X for new mappings, user/kernel address-space separation, mapping lifetime and
guard-page tests, exhaustion/fragmentation tests under multiple VM memory sizes.

## Phase 4 acceptance (planned)

Kernel object allocation with a debug mode (poisoning, guard pages where
feasible, double-free/overflow checks), plus a stable benchmark with environment,
compiler flags, iterations and seeds. Add slabs/per-CPU caches only after a
measured contention or fragmentation problem, with before/after data.

## Phase 5 acceptance (planned)

A native user-mode test program executes through a validated syscall boundary;
process/thread isolation, handle ownership and invalid-user-pointer rejection are
tested. Context switching and per-process address spaces are functional.

## Phase 6 acceptance (planned)

Reproducible launch of small native programs; initial VFS/filesystem; persistent
test state on a VM disk; console/shell; logging. Networking only after the core
services are stable.

## Phase 7 acceptance (planned)

PE parser/loader handles malformed inputs safely; a named set of controlled
x86-64 Windows console fixtures runs repeatedly with recorded outcomes using our
own API services. No general compatibility claim.

## Phase 8 acceptance (planned)

Specific published UI/application scenarios pass a compatibility matrix. A
screenshot or demo alone is insufficient.

## Phase 9 acceptance (planned)

Host graphics/driver tests pass; VM display capability is precisely labelled;
driver lifecycle, resource ownership, MMIO/DMA/interrupt validation, timeout and
reset paths are tested with mock devices before physical GPUs.

## Phase 10 acceptance (planned)

One narrowly scoped accelerated target is demonstrated by repeatable tests, or a
source/license/dependency feasibility report documents why it is blocked. A
research document alone is not a driver.

## Phase 11 acceptance (planned)

A documented, explicitly limited WDDM/driver proof of concept with named GPU,
driver version, features and tests—or a `BLOCKED/RESEARCH` conclusion. No general
Windows driver compatibility claim.
