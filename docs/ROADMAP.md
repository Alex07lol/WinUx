# Roadmap

Statuses are evidence-based. A phase is complete only when its acceptance tests
were run and observed. No dates or completion percentages are implied.

| Phase | Scope | Status |
|---|---|---|
| 0 | repository audit, toolchain, engineering controls, initial docs | **complete** (2026-10-17) |
| 1 | own kernel, Limine boot, serial/framebuffer, memory map, ISO, boot tests | **complete for available tooling** — QEMU BIOS (256 MiB/512 MiB/1 GiB) and VirtualBox BIOS verified; UEFI boot verification outstanding |
| 2 | GDT/IDT/exceptions, timer, diagnostics | **partial** — GDT/TSS/IST, 32 exception gates, int3 self-test, PIT IRQ0 and xAPIC LINT0 ExtINT work, IRQ-safe locking for interrupt-shared state; APIC timer/IOAPIC, SMP, recoverable exceptions not started |
| 3 | physical/virtual memory, page tables, permissions | **partial (kernel-side VM manager done)** — bitmap frame allocator; kernel-owned 4-level address space with CR3 switch, map/unmap/translate, on-demand tables, W^X enforcement and guard refusals, all self-tested in QEMU/VirtualBox BIOS boots; user address spaces, guard-page and fragmentation stress tests outstanding |
| 4 | kernel heap, slab/size classes, performance harness | **partial** — fixed 8 MiB first-fit free-list heap with global allocator, IRQ-safe locking, poisoning, double-free detection, `validate()` walk, host tests (14) and in-kernel self-test; growth, slabs per measurement and the heap benchmark harness outstanding |
| 5 | tasks, user mode, syscalls, handles/IPC, isolation | **partial (threads + first ring-3 slice done)** — thread table, kernel stacks, register/RSP context switch, cooperative yield/exit, timer preemption, deferred stack reclaim; ring 3 with a DPL3 `int 0x80` gate, validated `write`/`exit` services, user-fault isolation and address-space teardown verified in QEMU/VirtualBox BIOS; handles/IPC, process model, ELF loading not started |
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
HHDM read-back self-test, accounting restored after churn; a narrow uncached
MMIO leaf mapper with host-tested 4-level index/entry math; and the kernel's own
4-level address space (`kernel/src/vm.rs`): PML4 built from the kernel-half copy,
CR3 switch verified in QEMU BIOS (512 MiB) and VirtualBox BIOS, page
map/unmap/translate with on-demand intermediate tables, user-half teardown,
`invlpg` discipline, W^X policy host-tested, and self-test refusals for W^X
violations, double maps, misalignment, user pages in the kernel half and
non-canonical addresses.

Met since Phase 5b: user address spaces built from the kernel-half template are
exercised from ring 3 — map, run at CPL3, unmap, free frames and destroy page
tables all execute in every boot (ADR-0014).

Missing: guard pages around stacks, mapping-lifetime tests for shared pages,
exhaustion and fragmentation stress under multiple VM memory sizes, 5-level
paging, and reclaiming the bootloader's now-unused tables/frames.

## Phase 4 acceptance (partial)

Met: debug-mode-capable first-fit free-list heap with poisoning, double-free
detection, block/link magic, an O(n) `validate()` accounting walk, 14 host tests
including a 500-step churn that validates after every operation, and an in-kernel
self-test (Box, 10,000-element Vec growth, 1 MiB block, accounting restored)
that runs in every QEMU/VirtualBox boot.

Missing: heap growth beyond the fixed 8 MiB region, slab/per-size-class
allocators (deferred until a measured fragmentation/contention problem), guard
pages at heap edges, and the reproducible heap benchmark with recorded
environment/flags/iterations.

## Phase 5 acceptance (partial)

Met (kernel side): context switching and kernel threads are functional
(`kernel/src/sched.rs`); round-robin scheduling with cooperative yield, thread
exit and timer-driven preemption is verified in QEMU BIOS (three RAM sizes) and
VirtualBox BIOS. Exited-thread stacks are reclaimed at the next scheduling
point. No priority/sleep/SMP/final-thread-slot reuse yet.

Met (user side, first slice): a native ring-3 program executes in its own
address space built from the kernel-half template and reaches the kernel
through a DPL3 `int 0x80` gate (`kernel/src/user.rs`, `kernel/src/syscall.rs`).
`SYS_WRITE` output is validated per page against the live CR3 (user-half range,
USER bit, 4 KiB cap) and `SYS_EXIT` ends the thread; a user-mode page fault is
reported, counted and isolated (thread killed, kernel continues). Both paths
verified in QEMU BIOS (three RAM sizes) and VirtualBox BIOS with
`checks=...,heap,vm,sched,user`.

Missing (user side): programs are embedded fixtures, not loaded from a
filesystem (no ELF loader yet); one user thread at a time (single TSS `rsp0`);
no process model, handle table, or invalid-pointer fuzz corpus beyond the
single unmapped-write case; no guard pages around the user stack; no
`fork`/`exec`/`wait`; syscalls are console-write only.

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
