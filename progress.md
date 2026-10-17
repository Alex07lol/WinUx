# Progress

Last updated: **2026-10-17**

## Current milestone

Phase 0 complete; Phase 1 boot milestone implemented and verified under QEMU
(BIOS, three RAM sizes) **and VirtualBox (BIOS)**. Phase 2 and Phase 3 have
working foundations (descriptor tables, exceptions, legacy timer, physical frame
allocator, narrow MMIO page-table mapping). Phase 3's central deliverable — the
kernel's **own page-table manager** — Phase 4's **kernel heap with global
allocator**, Phase 5a's **kernel threads with a timer-preempted round-robin
scheduler** and Phase 5b's **ring-3 user mode with an `int 0x80` syscall
boundary and user-fault isolation** are implemented and verified in
QEMU/VirtualBox BIOS boots. VFS, PE loading and Windows compatibility remain
unstarted.

This is **not** a Windows-compatible OS, not a desktop, and not a GPU-capable OS.

## Repository state

- Workspace root: this thread's local checkout.
- Git: branch `main`, one commit (`77bb56e phase1`). The heap, VM, scheduler,
  syscall and user-mode work since then is uncommitted; nothing was reset or
  discarded.
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
- Host-tested free-list heap (`os_core/src/heap.rs`): address-ordered free list,
  first-fit with tail splitting, 16-byte alignment, two-header block layout
  (block + link header) so double frees are detected without corruption,
  poison-on-free, full `validate()` walk with `HeapStats`/`HeapError`, and a
  500-step churn test that validates after every operation. 54 host tests pass.
- Kernel heap (`kernel/src/heap.rs`): fixed 8 MiB contiguous region leased from
  the frame allocator and reached through the HHDM, wrapped in an IRQ-safe
  lock, registered as `#[global_allocator]`; in-kernel self-test allocates a
  `Box`, grows a 10,000-element `Vec`, writes/reads a 1 MiB block and verifies
  accounting is restored.
- IRQ-safe spin lock (`IrqSpinLock`) that saves/restores RFLAGS around the
  critical section; existing `SpinLock` kept for interrupt-disabled contexts.
- Kernel VM manager (`kernel/src/vm.rs`): own 4-level PML4 built by copying the
  kernel half (entries 256..512) from the live tables, switch of CR3 onto it at
  boot, page map/unmap/translate with on-demand intermediate tables, user-half
  teardown, `invlpg`+fence TLB discipline, and W^X enforced at the API boundary
  (writable ⇒ non-executable, host-tested predicate). Self-test covers mapping,
  translation, guard refusals (W^X, double map, misalignment, user-in-kernel-
  half, non-canonical), CR3 swap with live user-page touches, unmap and frame
  release.
- Kernel scheduler (`kernel/src/sched.rs`, `arch/x86_64/context.rs`): boot
  context as thread 0, fixed table of 16 threads with 16 KiB kernel stacks from
  the frame allocator, fabricated switch-in frames with SysV-correct stack
  alignment, register/RSP-swap context switch, cooperative `yield_now`/`exit`
  and timer preemption (ISR sets `need_resched`, switch at dispatcher tail so
  the interrupted frame stays on its own stack and unwinds through `iretq`),
  deferred stack reclaim for exited threads, `hlt`-based join with PIT/TSC
  budgets. Boot self-test: two yielding workers (5 A/B steps interleaved) plus
  a CPU-bound spinner that completes under preemption. Observed in QEMU 512 MiB:
  24 switches, 11 preemptions; VirtualBox: 21 switches, 8 preemptions.
- IRQ-safe serial sink: the write lock now runs with interrupts disabled so a
  preempted thread mid-write cannot strand another thread spinning on the lock.
- Ring-3 user mode (Phase 5b, `kernel/src/user.rs`, `kernel/src/syscall.rs`):
  GDT user segments (data 0x2b, code 0x33, DPL3), DPL3 `int 0x80` gate, an
  iretq trampoline that programs the TSS `rsp0` with the current thread's kernel
  stack, two test programs copied from kernel text into fresh user frames of a
  private address space built from the kernel-half template. Program A performs
  one validated console write (`SYS_WRITE`) and exits with code 0 (`SYS_EXIT`);
  program B writes to an unmapped user address and is killed by the page-fault
  path with the kernel continuing. Every syscall argument is validated
  (fd/length bounds, user-half range, every touched page mapped user-accessible
  via a live-CR3 walk). After both joins the kernel reactivates its PML4 and
  tears the user spaces down (unmap, free frames, destroy page tables).
  Observed identically in QEMU (three RAM sizes) and VirtualBox.

## Exact commands and results

| Command | Result |
|---|---|
| `cargo build -p project_os_kernel --target x86_64-unknown-none --release` | PASS |
| `cargo test -p os_core` | PASS: 54 passed, 0 failed (includes 14 heap tests) |
| `cargo clippy` (os_core, membench, kernel) with `-D warnings` | PASS |
| `cargo fmt --all -- --check` | PASS |
| `scripts/fetch_limine.sh` | PASS: checksum verified, assets installed |
| `scripts/make_iso.sh` | PASS: ISO built, BIOS+UEFI catalog entries verified |
| `scripts/test_boot_qemu.sh` (BIOS, 256 MiB / 512 MiB / 1 GiB) | PASS: marker reached, QEMU exit 33 at each size |
| `scripts/test_memory.sh` | PASS: host allocator tests + in-VM self-test |
| `scripts/test_graphics_host.sh` | PASS: framebuffer tests |
| `scripts/test_all.sh` (scheduler milestone) | PASS: 13 PASS, 0 FAIL, 4 SKIP (UEFI/pci/driver-model/gpu-vm; skipped is not passed) |
| `scripts/test_all.sh` (user-mode milestone, final) | PASS: 13 PASS, 0 FAIL, 4 SKIP; unit suite 54 passed, 0 failed |
| QEMU BIOS boot matrix, 256 MiB / 512 MiB / 1 GiB | PASS at each size: marker `checks=...,heap,vm,sched,user`, exit 33 |
| VirtualBox BIOS (`scripts/test_virtualbox.sh`) | PASS with user-mode checks (ring-3 write/exit + isolated fault in `vbox-serial.log`); isolated VM created and deleted |
| `scripts/test_boot_qemu.sh --firmware uefi` | SKIP: no `OVMF_FD` / OVMF firmware |
| `scripts/test_virtualbox.sh artifacts/project-os.iso` | PASS (VirtualBox 7.2.20r175154): marker reached, 5 timer ticks, VM deleted |
| `scripts/test_pci.sh` | SKIP: PCI not implemented |
| `scripts/test_driver_model.sh` | SKIP: driver framework not implemented |
| `scripts/test_gpu_vm.sh` | SKIP: framebuffer-only, no virtual GPU driver |
| `scripts/bench_graphics.sh` | SKIP: no accelerated graphics |
| Corrupt-ISO negative boot test | PASS (expected failure): exit 1, marker absent |

Recorded QEMU boot log (user-mode milestone, 512 MiB) reaches:

```text
kernel heap selftest: passed (box, vec growth, 1 MiB block)
vm selftest: passed (map/unmap, translate, W^X, guard refusals, CR3 swap)
exception selftest: passed
pit: 100 Hz, IRQ0 unmasked
timer: 5 ticks observed
sched: worker-a step 0 (yield)
sched: worker-b step 0 (yield)
... (steps 0..4 alternate A/B) ...
sched: 5 workers finished (a=5, b=5, spinner=true), 24 switches, 11 preemptions, 1 live
sched selftest: passed (round-robin yield + timer preemption)
user: hello from ring 3 via int 0x80
syscall: thread exits with code 0
user fault isolated: vector=14 error=0x6 rip=0x40000a cr2=0x500000000 (thread killed, kernel continues)
user: ring-3 program exited (code 0), 1 write(s), 37 bytes, 1 isolated fault(s), 0 rejected syscall(s)
user selftest: passed (int 0x80 write/exit from CPL3, user fault isolated)
PROJECT_OS_BOOT_SUCCESS:0.1.0-77bb56ec checks=handoff,memmap,gdt,idt,exceptions,timer,allocator,heap,vm,sched,user
```

`scripts/test_boot_qemu.sh` (BIOS, 512 MiB) reached the marker and exited via
isa-debug-exit with the expected status 33; the 256 MiB / 1 GiB matrix and the
VirtualBox run were executed in the same `scripts/test_all.sh` pass.

Build identifier is `0.1.0-77bb56ec`, derived from `VERSION` and the current
Git commit (`77bb56e`); rebuilding from changed sources without a commit keeps
the same ID, which is why the exact kernel/ISO hashes above are recorded.

## Artifact

| Item | Value |
|---|---|
| Kernel ELF | `artifacts/kernel.elf`, 517,736 bytes (user-mode milestone) |
| Kernel SHA-256 | `66d7323f33495dc7a8e24c8ab6c29591979eeb85cb186d00c0eefba0c6763805` |
| Kernel ELF entry | `0xffffffff80000000` (verified with `readelf`) |
| ISO | `artifacts/project-os.iso` |
| ISO size | 6,506,496 bytes |
| ISO SHA-256 | `1ae233ada21cebdd12b036cd88968092bc411bcd3b008fa696a20db6ff7b51c7` |
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
- No VFS, networking, PCI, driver model, Windows PE/API compatibility or GPU
  acceleration.
- User-mode limits (Phase 5b is deliberately minimal): test programs are
  embedded in kernel text and copied to fresh frames — there is no ELF/PE
  loader yet; one user program runs at a time (the TSS `rsp0` is programmed for
  the sole user thread, so concurrent user threads are not supported); there is
  no fork/exec, no guard page above the user stack, and the user address-space
  layout is fixed (code at 0x400000, stack at 0x500000); user threads share the
  kernel half by construction and no syscall returns user data yet; the
  syscall gate is `int 0x80` with a 4 KiB write cap and console-fd-only write.
- The kernel now runs on its own 4-level address space (CR3 switch verified in
  QEMU). The old bootloader tables are intentionally left allocated but unused;
  reclaiming bootloader-reclaimable pages is future memory work. There is no
  5-level paging support, and the older MMIO mapper still installs permanent
  leaves into whichever tables are live.
- The kernel heap is a fixed 8 MiB region; it does not grow on demand yet, and
  kernel-space page mapping (rather than HHDM access) is not wired up.
- `AddressSpace::destroy` is now exercised by the user-mode teardown;
  `map_region` remains staged (dead-code annotated) for the process work.
- Scheduler limits: no priority, sleep/wake, or SMP; thread slots are not
  reused after exit; preemption switches run on the interrupted thread's own
  stack (correct but means nested preemption depth is unbounded by design);
  the `hlt`-based join relies on the PIT still delivering ticks.
- The legacy PIC/PIT path uses LINT0 ExtINT and does not start the APIC timer,
  IOAPIC or SMP. Those remain future work.
- The full QEMU BIOS matrix (256 MiB / 512 MiB / 1 GiB) and VirtualBox were
  rerun for the user-mode milestone and all reached the marker with `user` in
  the checks. UEFI boot remains unrun (no OVMF).
- No CI workflow is included: this workspace has no Git remote/hosting. The
  scripts are CI-ready, and a runner would need Rust, `mkisofs`, Python and QEMU.
- ISO bytes vary with timestamps; no `SOURCE_DATE_EPOCH`-pinned ISO yet.
- `IrqSpinLock` (and an interrupt-disabled serial sink) make interrupt-context
  sharing safe on a single CPU; the plain `SpinLock` remains and must not be
  used from code that can be preempted by an interrupt handler that takes the
  same lock.

## Single best next action

Start Phase 6 (VFS/console/shell): a kernel console abstraction with a simple
in-memory VFS and a serial/framebuffer shell, so the interactive surface exists
before the PE/Win32 work; optionally load the first real user program from a
Limine module (ELF64) to replace the embedded ring-3 fixture from Phase 5b.
