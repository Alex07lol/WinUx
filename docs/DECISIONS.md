# Architecture decisions

All decisions dated 2026-10-17 unless noted. Each entry records context,
alternatives, the decision, evidence and what would trigger reconsideration.

## ADR-0001 — Limine bootloader, protocol base revision 6

**Context.** We need a bootloader that loads a fixed-address ELF64 kernel in the
higher half, provides a memory map/framebuffer/HHDM and is well specified.

**Alternatives.** GRUB/Multiboot2 (heavier config and protocol surface), a custom
bootloader (explicitly out of scope for the MVP), UEFI-only loader (loses BIOS
VirtualBox compatibility).

**Decision.** Use the official Limine 12.9.3 release, pinned by URL and SHA-256,
with its binary BIOS and UEFI El Torito images. Request protocol base revision 6.
The kernel validates the base-revision tag and refuses to boot on a bootloader
that did not honour it.

**Evidence.** `limine-protocol` PROTOCOL.md (base revisions 0–6, x86-64 machine
state, requests/response ABI, memory map); Limine USAGE.md (hybrid ISO layout,
`limine.conf`); our ISO contains BIOS platform 0x00 and UEFI platform 0xEF
no-emulation entries; QEMU BIOS boot reaches the marker with base revision 6.

**Consequences.** We depend on a third-party bootloader (BSD-2-Clause). QEMU BIOS
is verified; UEFI boot is not yet executed (no OVMF). Limine's default LAPIC
state must be handled (ADR-0005).

**Reconsider if.** Limine drops a needed base revision, a documented requirement
cannot be met, or UEFI/BIOS compatibility tests fail in ways the bootloader
causes.

## ADR-0002 — Rust `no_std` kernel with a narrow C/assembly boundary

**Context.** Need memory safety where practical, freestanding x86-64 output, and
a Windows development host.

**Alternatives.** C kernel (more manual safety burden), C++ (complex freestanding
runtime requirements), mixed multi-language kernel (unjustified complexity).

**Decision.** Rust 2024, `x86_64-unknown-none`, `panic=abort`, static relocation,
kernel code model, rust-lld (target default). Assembly is limited to
`entry.S` and interrupt stubs; no C is currently used. `os_core` is `no_std` in
the kernel and ordinary Rust in host tests.

**Evidence.** Rust platform-support docs and target spec: the built-in target uses
rust-lld, no red zone, panic=abort, soft-float; its default relocation model is
PIC, so `-C relocation-model=static` is required. The build produces an ELF with
entry `0xffffffff80000000` and the expected three segments.

**Consequences.** No host std/libc in the kernel. `unsafe` is required for port
I/O, descriptors and raw bootloader pointers, and is documented at each site. C
remains available for future architecture glue but is not assumed beneficial.

**Reconsider if.** The target lacks a required CPU feature or stable-Rust support,
or a measured, isolated need justifies another language at a boundary.

## ADR-0003 — Modular monolithic kernel

**Context.** A small bootable kernel must grow toward processes, VFS, drivers and
compatibility services without a microkernel IPC tax or an unstructured blob.

**Decision.** Modular monolithic kernel with strict internal boundaries:
architecture, boot handoff, memory, synchronization, scheduling, syscalls, VFS,
drivers, security, later NT/Win32 services. Modules are added only with their
first real feature (no empty stub trees).

**Consequences.** Simpler bring-up and debugging; boundaries are enforced by
module layout and review rather than IPC. SMP and driver isolation are later
problems that may force interface changes.

## ADR-0004 — Physical frames: usable-RAM bitmap, no implicit reclaim

**Context.** First memory milestone must never hand out firmware, bootloader,
kernel or framebuffer memory, and needs testable allocation/free semantics.

**Alternatives.** Bump allocator only (no free), free-list spans (more complex
free path), buddy allocator now (premature without benchmarks and paging), Linux
allocator source (license and design mismatch).

**Decision.** Host-tested bitmap allocator: one bit per 4 KiB frame, all frames
reserved initially, only explicitly usable regions released, low 1 MiB reserved,
power-of-two contiguous allocation with natural alignment, strict free validation.
Static 2 MiB bitmap capped at 64 GiB. Reservations are explicit; bootloader
reclaimable memory is never freed implicitly.

**Evidence.** 29 host tests (alignment, exhaustion, double free, range overflow,
clamping, deterministic churn), in-VM self-test with HHDM read-back, accounting
restored after churn. Host benchmark (64 MiB model, 200 000 iterations):
14.8 ns/op mean, p50 14 ns, p99 17 ns, no fragmentation in that workload
(one representative run; run-to-run means vary by roughly ±0.5 ns).

**Consequences.** O(n) first-fit scanning and a 2 MiB BSS cost; acceptable for
bring-up but not a final allocator. No page-table integration yet. No claim of
Linux-level performance.

**Reconsider if.** Fragmentation or scan cost becomes measurable under realistic
workloads; introduce orders/buddy or per-CPU caches only with before/after data.

## ADR-0005 — Legacy PIC/PIT for the first interrupt milestone

**Context.** Limine base revision 5+ initialises and enables the local APIC. The
simplest periodic interrupt source is the 8254 PIT through the 8259 PIC.

**Decision.** Remap both PICs to vectors 0x20/0x28, mask all IRQs, unmask IRQ0,
program PIT channel 0 to 100 Hz (mode 2), keep the local APIC enabled, and
configure LINT0 ExtINT (ADR-0011) so the 8259 can reach the CPU. Keep all other
interrupts masked.

**Evidence.** Limine protocol x86-64 machine state (base revision 5); Intel SDM
8259/8254 programming model; QEMU BIOS and VirtualBox 7.2.20 runs both observe 5
ticks. This is a bring-up choice, not a claim that the PIC/PIT is the final
timer.

**Consequences.** The APIC MMIO page is mapped uncached and LINT0 is dedicated to
ExtINT. No APIC timer, IOAPIC, MSI, SMP or high-resolution timing yet.
Interrupt affinity and per-CPU delivery are later work.

## ADR-0006 — Serial is authoritative; framebuffer is a dashboard

**Context.** Automated tests need deterministic output; a display is useful for
humans and demonstrates the framebuffer path.

**Decision.** Every boot fact and failure is written to COM1. The framebuffer
dashboard renders project name, build id, status rows and the success marker, but
never replaces serial evidence. If no framebuffer exists, boot continues with a
warning and the serial path unchanged.

**Consequences.** Tests assert on serial. The dashboard is explicitly labelled as
a boot framebuffer, not a GPU driver. Text uses an original 5x7 font (ADR-0009).

## ADR-0007 — ISO tooling: cdrtools mkisofs, numeric EFI platform, forced hybrid MBR

**Context.** Limine's documented ISO recipe assumes xorriso, which is not available
in this environment. cdrtools 3.02a09 `mkisofs` is available through a per-user
package manager. The generated ISO is not partitioned, so `limine bios-install`
refuses to run without `--force`.

**Decision.** Create the ISO with `mkisofs`:
`-b limine-bios-cd.bin -no-emul-boot -boot-load-size 4 -boot-info-table`, then
`-eltorito-alt-boot -eltorito-platform 239 -b limine-uefi-cd.bin -no-emul-boot`.
239 is the numeric EFI platform id; this build rejects the string `efi` and
rejects `-e`. Then run `limine bios-install --force` on the generated ISO file to
add a hybrid MBR, and verify the catalog with `scripts/verify_iso.py`.

**Evidence.** QEMU BIOS boots via El Torito before and after the MBR install.
`verify_iso.py` reports platform 0x00 (4 sectors) and 0xEF (6144 sectors), both
no-emulation. The El Torito spec/edk2 header confirms 0x91 is the final section
header.

**Consequences.** xorriso is not a build dependency. `--force` is safe here
because it targets a freshly generated ISO file, never a host device; scripts
never pass a block device. UEFI boot itself is still unverified without firmware.

## ADR-0008 — Boot test uses a gated QEMU debug-exit

**Context.** Automated boot tests need a bounded, meaningful termination rather
than waiting for a timeout.

**Decision.** After all checks and the success marker, the kernel writes `0x10`
to port `0xf4` only when the Limine cmdline contains `debug_exit`. The test
runner supplies `-device isa-debug-exit,iobase=0xf4,iosize=0x04` and expects
status 33 (`(0x10 << 1) | 1`). Without the device (VirtualBox), the port write is
ignored and the kernel idles. The test also polls serial with a bounded timeout
and fails on panic/fatal markers.

**Evidence.** QEMU `debugexit.c` status formula and device properties; QEMU
successful run exits 33; corrupted-ISO negative test exits 1 after timeout.

## ADR-0009 — Original 5x7 bitmap font

**Context.** The boot dashboard needs legible text without a graphics stack or
third-party font licensing complexity.

**Decision.** Use a hand-authored 5x7 uppercase/digits/punctuation font in
`kernel/src/font.rs`; lowercase maps to uppercase; render at integer scales.

**Consequences.** No font license/provenance obligation, small code size, and an
intentional retro-terminal look. It is not a full Unicode or typographic font;
a real text stack is future work.

## ADR-0010 — Case-insensitive control-file aliases

**Context.** The development checkout is on a Windows filesystem where
`AGENTS.md`/`agents.md`, `STOP.md`/`stop.md` and `PROGRESS.md`/`progress.md` are
the same files. The mandate requires both names.

**Decision.** Keep one file per pair: canonical `AGENTS.md`, lowercase
`stop.md` and `progress.md`. Document that the uppercase/lowercase paths resolve
to the same file on this host. On a case-sensitive filesystem, add pointer files
with no duplicated rules.

**Consequences.** Six distinct files cannot exist here. The documentation is
explicit rather than pretending otherwise; tools that open either case read the
same content.

## ADR-0011 — Map APIC MMIO and use LINT0 ExtINT for the 8259

**Context.** On the default VirtualBox 7.2.20 BIOS profile, the kernel reached
`pit: 100 Hz, IRQ0 unmasked` and then stalled. Diagnostics showed IF=1, PIC
`IMR=0xfe`, PIC `IRR=0x01`, PIC `ISR=0x00`, PIT counter moving, and zero ticks:
the timer was pending at the 8259 but the CPU never took it. A 64-bit VirtualBox
guest cannot have its APIC disabled, and Limine's HHDM did not map the APIC page
at `0xfee00000`, so the kernel could not program LINT0.

**Decision.** Keep the local APIC enabled, switch it to xAPIC mode, map its MMIO
page through the live 4-level tables with a narrow uncached mapper in
`kernel/src/arch/x86_64/paging.rs`, and set LINT0 to ExtINT delivery and
unmasked in `kernel/src/arch/x86_64/apic.rs`. Fall back to the old
disabled-LAPIC path only if mapping or LINT0 programming fails.

**Alternatives considered.** Disabling the APIC (rejected: VirtualBox refuses
for 64-bit guests, and QEMU-only success was misleading); x2APIC MSR access
(rejected: the default VirtualBox CPU profile does not expose x2APIC); an IOAPIC
route (needs the same MMIO mapping and a larger interrupt-controller design); a
polled-timer fallback (rejected as it would weaken the interrupt test).

**Evidence.** VirtualBox 7.2.20r175154, default BIOS VM: before the fix,
`PROJECT_OS_FATAL: timer did not deliver 5 interrupts (observed 0)`; after the
fix, `timer: 5 ticks observed` and `PROJECT_OS_BOOT_SUCCESS`. QEMU and the
host-test suite still pass.

**Consequences.** The mapper is intentionally narrow: it installs permanent
MMIO leaves, never splits huge pages, never unmaps and requires 4-level paging.
It is not a general VM manager; that remains Phase 3/4 work. LINT0 is now owned
by the 8259 path, so the later IOAPIC/APIC-timer milestone must take it over
deliberately.

## ADR-0012 – Kernel-owned page tables, fixed heap region, IRQ-safe locks

**Context.** Phase 3/4 require the kernel to stop depending on bootloader page
tables and to provide a global allocator before user mode can exist. The
bootloader (Limine) maps kernel image, HHDM and (only some) MMIO through its own
PML4. Device MMIO is not fully covered, and the timer ISR now coexists with
potentially allocating threads.

**Decision.** (`kernel/src/vm.rs`, `kernel/src/heap.rs`, `kernel/src/sync.rs`)

1. Build a kernel PML4 by copying PML4 entries 256..512 from the live tables
   (kernel image + HHDM share lower kernel tables), zero the user half, and
   switch CR3 onto it. User spaces later copy the kernel half from this
   template. Bootloader tables/frames are intentionally leaked (reclaiming them
   needs reclaimer policy).
2. Lease a fixed 8 MiB contiguous frame run through the HHDM as the kernel heap
   (`FreeListHeap`, first-fit, address-ordered coalescing free list). No growth
   yet; growth needs kernel-space per-page mapping.
3. W^X is enforced in `vm::map_page`: any writable mapping must set NX.
   Intermediate tables are never split (huge-entry hits are errors).
4. `IrqSpinLock` (save/disable IF, spin, restore) protects heap and scheduler;
   the serial sink lock also disables interrupts for the write. Plain
   `SpinLock` remains for pre-interrupt-only paths.

**Alternatives considered.** Identity-mapping the kernel by hand with its own
new PML4 entries (rejected: duplicating Limine's layout risks missing regions);
moving the heap into kernel-image space (rejected for now: HHDM lease is
simplest and testable); wiring `PageTables::map_mmio_2m` to mutate the *kernel*
space after activation (kept: it walks live CR3, so it now edits the kernel
half — this is acceptable for one live CPU).

**Evidence.** QEMU BIOS 256 MiB/512 MiB/1 GiB and VirtualBox BIOS all reach
`PROJECT_OS_BOOT_SUCCESS:...checks=...,heap,vm,sched`; `vm selftest: passed
(map/unmap, translate, W^X, guard refusals, CR3 swap)`; `kernel heap selftest:
passed`; host `cargo test -p os_core`: 53 passed.

**Consequences.** The heap’s fixed 8 MiB is a hard ceiling until kernel-space
mapping and reclaim exist. Huge-page splitting and 5-level paging are not
supported. Freeing bootloader tables will be a later, separate decision with its
own safety analysis.

## ADR-0013 – Cooperative + timer-preempted kernel threads (Phase 5a)

**Context.** With IRQ0 working and an IRQ-safe lock available, the next
milestone is kernel threads and a scheduler strong enough to prove isolation of
stacks and preemption.

**Decision.** Thread 0 is the boot context; up to 15 more threads get 16 KiB
kernel stacks. Context switch (`arch/x86_64/context.rs`) saves only
callee-saved registers and swaps RSP; the trampoline fabricates the initial
frame so a new thread starts with the SysV stack alignment. Preemption: the
timer ISR sets `need_resched` when another thread is runnable; the switch is
performed at the tail of `exception_dispatch` so the interrupted thread keeps
its IRQ frame on its own stack and unwinds later through the original `iretq`.
Interrupts stay disabled across the switch; trampolines re-enable. Stacks of
exited threads are freed at the next scheduling point on another thread's
context.

**Alternatives considered.** Switching from inside `context_switch` while
holding the scheduler lock (rejected: lock alive across switch / possible
double-switch); full `iretq` frame surgery in the switch (rejected: complexity,
easier to leave frames on per-thread stacks); separate per-thread TSS (not
needed for ring-0 threads; needed later for ring 3).

**Evidence.** QEMU BIOS 256 MiB/512 MiB/1 GiB (512 MiB: 24 switches, 11
preemptions) and VirtualBox (21 switches, 8 preemptions): five A/B yields,
spinner completed under preemption, `sched selftest: passed`.

**Consequences.** Cooperative + preemptive kernel threads exist, but there is
no priority, sleep/wake or SMP; idle is `hlt` in thread 0; a thread cannot yet
own user-mode state (Phase 5b).

## ADR-0014 – Ring 3 via `int 0x80`, embedded test programs, one user thread (Phase 5b)

**Context.** The VM manager (ADR-0012) can build user address spaces and the
scheduler (ADR-0013) can run threads, but no code has executed below CPL0. The
first ring-3 milestone must prove: a user program runs at CPL3 in its own
address space, syscalls reach the kernel through a validated boundary, and a
user fault kills only that thread.

**Decision.**

- **Gate:** a DPL3 interrupt gate at vector `0x80` (not `syscall`/`sysret`):
  the existing 15-register exception frame already carries every argument
  register and is reachable from ring 3 by installing the gate with DPL3, so
  no additional entry path is needed. Argument registers are Windows-style
  (`rax` service, `rcx`/`rdx`/`r8`/`r9` arguments) so the PE compatibility
  layer can reuse the gate.
- **Services (minimal):** `1 write(fd=1, buf, len)` with `len <= 4096`, and
  `2 exit(code)`. Every pointer is validated against the live CR3: canonical
  user half, whole span below the user/kernel boundary, and every touched page
  mapped with the USER bit before the kernel reads a byte.
- **Test programs:** assembled with `global_asm!`, copied from kernel text
  into fresh user frames of a `new_like_kernel` address space (kernel half
  shared with the boot template). Program A writes and exits 0; program B
  dereferences an unmapped user address. No ELF loader yet — this keeps the
  ring-3 boundary the only new variable.
- **Fault isolation:** any exception taken with `cs & 3 == 3` is reported,
  counted and turned into `sched::exit()` (thread killed, kernel continues);
  kernel-mode exceptions stay fatal.
- **One user thread at a time:** the TSS `rsp0` is a single global, so the
  demo runs programs sequentially (`spawn A → join → spawn B → join`), each
  with `set_tss_rsp0` pointed at that thread's kernel stack before `iretq`.
  Concurrent user threads need per-thread TSS support and are explicitly out
  of scope for this milestone.
- **Teardown:** after both joins the kernel reactivates its PML4, unmaps and
  frees the user frames, and destroys the page tables. The stale TSS `rsp0`
  is harmless because no ring-3 entry happens after teardown.

**Alternatives considered.** `syscall`/`sysret` (rejected for now: needs
MSR programming, a swapgs/stack-switch stub and its own frame layout);
per-thread TSS (deferred until concurrent user threads); loading a Limine
module as the user program (deferred to Phase 6/7, when a real ELF/PE loader
exists).

**Evidence.** QEMU BIOS 256 MiB/512 MiB/1 GiB and VirtualBox all print
`user: hello from ring 3 via int 0x80`, `syscall: thread exits with code 0`,
`user fault isolated: vector=14 ... cr2=0x500000000 (thread killed, kernel
continues)`, `user: ring-3 program exited (code 0), 1 write(s), 37 bytes,
1 isolated fault(s), 0 rejected syscall(s)` and reach
`checks=...,heap,vm,sched,user`. Host unit suite: 54 passed, 0 failed.

**Consequences.** The first ring-3 boundary is real and tested end to end,
but user programs are embedded test fixtures, one runs at a time, and there is
no process model (no fork/exec, handles or per-process resources). Those arrive
with the VFS/process work (Phase 6).

## ADR-0015 – Serial console, in-memory VFS and shell (Phase 6, stage 1)

**Context.** Phase 5b proved ring-3 execution, but the only input path was the
bootloader handoff and there was no way to name or store anything at runtime.
Phase 6 starts the interactive surface: serial console input, a filesystem to
name things, and a shell. On-disk persistence and a real program loader are
later stages.

**Decision.**

- **Console input:** COM1 receive via IRQ4 (vector 36) only. The interrupt
  handler drains the UART into a 256-byte software ring under an IRQ-safe
  lock; echo, line editing and assembly happen in the reader thread, never in
  the handler. The reader blocks with `hlt` and never busy-polls. `read_line`
  accepts CR/LF, backspace (0x08/0x7F) and printable ASCII up to 128 bytes.
- **UART FIFO trigger level 1 (`FCR` 0x07):** the common `0xc7` value sets a
  14-byte trigger level, which strands short input bursts below the threshold
  until later bytes push the FIFO over it — observed as reordered and dropped
  characters when QEMU was driven through a pipe. Trigger level 1 raises IRQ4
  on every byte. The ISR additionally counts true hardware FIFO overruns
  (LSR bit 1) and full-ring drops into `RX_OVERRUNS`, viewable via the
  `console` builtin (`0 overruns` in the recorded session).
- **In-memory VFS:** an arena of at most 64 nodes in kernel heap memory with
  inline data capped at 8192 bytes per file. Path rules (absolute, no `.` or
  `..`, printable ASCII, component and total length caps) live in
  `os_core::path` and are host-tested. The kernel tree seeds `/etc`,
  `/etc/motd`, `/etc/version` and `/tmp`, and runs a boot self-test covering
  create/read/append/overwrite/remove/list and the error paths. `write`
  creates or replaces (`write_file` resolves the parent and pushes a new file
  when missing); `append` requires an existing file. Nothing persists across
  reboots — the `version` builtin states this explicitly.
- **Shell:** one kernel thread spawned after the success marker; the boot
  thread hands it the CPU with `yield_now()` and only runs again if the shell
  exits (there is no scheduler idle thread yet). Builtins cover the kernel
  services above (see `help`). Input and output are serial-only; there is no
  framebuffer console input.
- **Interactive ISO:** `make_iso.sh --interactive` produces
  `artifacts/project-os-interactive.iso` with `debug_exit` stripped from the
  staged Limine cmdline so the kernel falls through to the shell; the default
  ISO keeps the gated `isa-debug-exit` for boot tests.
- **Shell test:** `scripts/test_shell_qemu.sh` (with
  `scripts/test_shell_qemu.py`) boots the interactive ISO in QEMU with a
  **TCP serial chardev** and types a paced session (2 ms per byte),
  asserting each command's output and the prompt. `-serial stdio` is
  deliberately not used: QEMU's 16550 model inserts an unthrottled host
  burst straight into the 16-byte FIFO, which overruns before any guest can
  wake from `hlt` — a harness artifact, not kernel behaviour. The TCP path
  lets the host choose the delivered byte rate, like a real UART.

**Alternatives considered.** Polling the UART from the reader loop (wastes
CPU and cannot recover bytes the emulated FIFO already dropped); a larger
hardware FIFO (not emulated by QEMU's 16550); software flow control (the guest
cannot throttle a host pipe); running the shell as a function of the boot
thread (discards the scheduler integration Phase 5 built); `-serial pipe` on
this host (no pacer available on Windows).

**Evidence.** Every boot prints `vfs selftest: passed (6 nodes, 3 files; ...)`
and the marker gains `,vfs`. `scripts/test_all.sh`: 15 PASS, 0 FAIL, 4 SKIP
(UEFI/pci/driver-model/gpu-vm) with the shell session step included.
Interactive session: `session: 17 commands, 3900 bytes received, 0 failures`,
`console: ... 0 overruns`; host unit suite 63 passed.

**Consequences.** The OS now has a real interactive surface and a filesystem
namespace, but the filesystem is volatile, the console is serial-only, the
shell has no history/job control, and pasting a large burst into a QEMU stdio
serial can still overrun the emulated FIFO (counted, not hidden). On-disk
persistence, an ELF64 module loader and framebuffer console input are the next
Phase 6 work.

## ADR-0016 – ATA PIO block device and checkpoint persistence (Phase 6, stage 2)

**Context.** Stage 1 (ADR-0015) had a volatile VFS. Phase 6 acceptance needs
on-disk persistence verified across reboots in a VM. The register-level and
byte-level contract lives in `docs/PERSISTENCE.md`; this ADR records the
load-bearing choices.

**Decision.**

- **Driver:** classic ATA PIO on the primary PIIX channel (`0x1F0`/`0x3F6`),
  master drive, LBA28, polled with a PIT-based 5 s deadline; no IRQ14, no DMA.
  QEMU `q35` exposes only ICH9-AHCI and no legacy ports (R-009 in
  `docs/RESEARCH.md`), so the persistence harness boots `-machine pc` with
  `-drive file=...,format=raw,if=ide,index=0,media=disk`; every other QEMU
  test stays on q35. Drive/head select is `0xE0 | lba[24:28]` (master, LBA
  mode). An early implementation wrote `0xA0` (master, CHS mode); QEMU then
  translated the LBA registers through the disk geometry and both slots landed
  on wrong sectors while staying self-consistent — only byte-level inspection
  of the image exposed it.
- **Format:** `os_core::pofs` — one CRC32-checked header sector (magic `POFS`,
  version, sequence number, payload length, payload CRC, header CRC) followed
  by a length-prefixed node-record payload; host-tested with corruption,
  truncation and trailing-byte vectors.
- **Slots:** two fixed 1 MiB slots (A at LBA 16, B at LBA 2064 of the 8 MiB
  test image). Boot examines both, logs and skips invalid slots, loads the
  valid slot with the highest sequence number, or formats slot A on a fresh
  disk. A save targets the inactive slot at seq+1, verifies the header by
  read-back, and only then switches the active slot; a failed save keeps the
  previous checkpoint and is retried at the next command boundary.
- **Write-through:** the VFS sets a dirty flag on every mutation and the shell
  loop calls `persist::flush_if_dirty()` after each command, so a mutation is
  already on disk by the time the next prompt prints.
- **Evidence model:** the four-boot harness corrupts a slot's payload sector
  (header intact) so CRC rejection and fallback are proven, not assumed.

**Alternatives considered.** AHCI (needs PCI + MMIO, Phase 9); virtio-blk
(same, plus a non-ATA protocol); IRQ14/DMA (no correctness gain at polled PIO
rates, extra machinery); a log-structured/journal store (more write
amplification and recovery surface than a two-slot double buffer needs);
whole-slot zeroing as the corruption test (indistinguishable from a fresh
empty slot, so it cannot prove CRC rejection).

**Evidence.** `scripts/test_persistence_qemu.sh`: four boots against one raw
image — fresh format and first save; reboot load, then two more saves (A seq2
/ B seq3); corruption of the newest slot's payload → `payload crc mismatch`,
fallback to A seq2, and the file from the lost generation is gone; both slots
corrupt → fresh format, old file gone. Host `pofs` vectors are part of the
77-test `os_core` suite. No-disk boots remain byte-identical to stage 1
(marker `checks=...,vfs`, `blk: no ATA device on primary channel`,
`persist: inactive (no ATA disk)`); with a disk the marker gains `,blk,persist`.

**Consequences.** The VFS survives reboots on PIIX-legacy disks. Limits
(documented in `docs/PERSISTENCE.md`): PIIX primary-master LBA28 PIO only —
no AHCI/virtio/DMA/IRQ; whole-tree snapshots flushed at command boundaries;
CRC32 is integrity checking, not security; ~1.1 MiB transient heap per save;
evidence is QEMU `-machine pc` only. AHCI, a real on-disk filesystem and
framebuffer console input remain future work.

## ADR-0017 – Native ELF64 loading of userland programs (Phase 6, stage 3)

**Context.** Stage 5b (ADR-0014) demonstrated ring 3 with two inline-assembly
fixtures embedded in the kernel image. Phase 6 stage 3 replaces them with real
ELF64 executables built from Rust source and shipped as Limine modules. The
full contract (parser rules, loader API, serial lines, test plan) lives in
`docs/ELF_LOADER.md`; this ADR records the load-bearing choices and deviations.

**Decision.**

- **Native ET_EXEC loading, no emulation layer.** The kernel parses ELF64
  itself (`os_core::elf`, host-tested, 19 new unit tests) and maps `PT_LOAD`
  segments into a fresh user address space. `ET_DYN`/PIE, relocations and
  `PT_INTERP` are rejected with named errors — no consumer exists yet and a
  relocation engine would be unverifiable busywork.
- **Two-phase loader API** (`loader::inspect` then `loader::map`): rejection
  (including the deliberately corrupt `bad.elf`) happens before any frame is
  allocated; mapping rolls back every page on failure. The code window is
  fixed at `0x400000..0x500000` (1 MiB); the parser additionally requires
  page-aligned segments, no W+X, non-overlapping page ranges and an entry
  inside an executable segment.
- **Independent `userland/` cargo workspace** (Rust, `no_std`, no external
  crates): a shared runtime provides the `int 0x80` syscall shims and `entry`;
  `hello` prints the exact stage-5b greeting, `fault` writes to an unmapped
  address, and `build_user.sh` generates `bad.elf` by zeroing the magic.
  Userland code therefore stays in Rust, consistent with the language policy,
  and the programs are our own binaries only.
- **Boot self-test only** (no shell `exec` builtin): the demo runs `hello`,
  then `fault`, one at a time on scheduler threads; `bad.elf` is a runtime
  rejection probe. The success marker gains `,elf` between `user` and `vfs`,
  keeping the stage-2 persistence suffix `vfs,blk,persist` byte-compatible.
- **Rustc flags for userland via `RUSTFLAGS`** (deviation): cargo joins
  `target.*.rustflags` arrays from ancestor config files, so the root
  `-Tkernel/linker.ld` cannot be un-set per-key; a `build.rs` relocation of the
  kernel flags was abandoned because build-script executable paths exceed the
  Windows 260-character `MAX_PATH` in this checkout (MSVC `LNK1104`). The
  remaining deviations (`limine_file` size correction, `Inspection` storing
  `Segment`s, skip-line honesty, clippy target selection) are recorded in
  `docs/ELF_LOADER.md` §12.

**Alternatives considered.** Keeping inline-assembly fixtures (rejected:
the stage exists to prove real binaries execute); loading from the VFS
(rejected: no on-disk exec format and the 8192-byte file cap); `ET_DYN` with a
relocation engine (no consumer); a bootstrap `build.rs` for kernel link flags
(blocked by `MAX_PATH`, see above); byte-granular unaligned segments (only
consumers would be non-conforming toolchains); masking a missing module as a
pass (rejected: the skip path prints an explicit skipped verdict and the QEMU
ELF test fails on it).

**Evidence.** `os_core` unit tests 77 → 96 (all `elf` vectors); userland
artifacts hash-stable from a clean rebuild (`hello.elf` `f4ae1ae1...`,
`fault.elf` `46c45ce6...`, `bad.elf` `a8bec8d1...`); `scripts/test_elf_qemu.sh`
asserts the seven ELF serial lines and the `,elf` marker; `test_all.sh`
PASS=18 FAIL=0 SKIP=4 (was 16; +`userland`, +`elf`), with VirtualBox and the
four-boot persistence test unchanged.

**Consequences.** The OS now runs its own ELF64 binaries loaded from the
bootloader; a corrupt image cannot take down boot. Limits restated in
`docs/ELF_LOADER.md` §11: no dynamic linking/PIE/ASLR, no filesystem-backed
exec, fixed VA windows, one user thread at a time, bootloader-provided modules
only.

## ADR-0018 – e1000 networking: polled driver, static IPv4, host-tested codecs (Phase 6, stage 5)

**Context.** Phase 6's last scope item is networking. The full contract
(pinned serial lines, test plan, limits) lives in `docs/NETWORK.md`; the
research record is R-012. The kernel has no interrupt-driven device framework,
no DHCP client and no socket layer, so the stage had to prove real NIC
operation without inventing those subsystems.

**Decision.**

- **One NIC: Intel 82540EM ("e1000"), found by a minimal PCI mechanism-#1
  scan** (`0xCF8`/`0xCFC`, bus 0, first `8086:100E`), BAR0 sized by all-ones
  write-back with immediate restore; memory-space and bus-master bits enabled.
  No general PCI enumeration or driver model (`scripts/test_pci.sh` remains a
  Phase 9 SKIP).
- **Polled, interrupt-free driver** (`kernel/src/e1000.rs`): MMIO through the
  existing 2 MiB uncached mapper, `CTRL.RST` reset, MAC from RAL/RAH with a
  bounded EEPROM read fallback (VirtualBox), legacy 16-byte descriptors,
  RX 32 × 2048 B with tail/head protocol, one shared 2048 B TX buffer with
  synchronous `STA.DD` polling and PIT-deadline watchdogs (plus a spin bound
  for waits that run with interrupts disabled inside the network lock).
- **Static IPv4** `10.0.2.15/24`, gateway `10.0.2.2` — the QEMU slirp and
  VirtualBox NAT defaults. No DHCP, DNS, TCP or sockets; UDP exists only as the
  bounded `udpecho` shell fixture used by the harness.
- **Protocol codecs and state live in host-tested `os_core`**
  (`pci`, `net::{eth,arp,ipv4,icmp,udp}`, RFC 1071 checksums, 51 new tests);
  the kernel side is glue, a non-fatal loopback bring-up check and shell
  builtins (`net`, `ping`, `udpecho`).
- **Non-fatal by design:** no NIC (`e1000e` on q35 default), bring-up failure
  or an unanswered gateway never kills boot; the success marker appends `,net`
  only when the ARP+ICMP selftest passed. The MAC loopback check is a separate
  line and all three outcomes (`passed`/`unsupported`/`unexpected frame`) are
  accepted; QEMU's model implements only PHY `BMCR` loopback, so `unsupported`
  is the expected QEMU result (R-012, corrected during implementation).

**Alternatives considered.** virtio-net (more emulation-specific surface and no
VirtualBox equivalent; the roadmap names e1000), e1000e support (unnecessary —
q35 and VirtualBox both accept an 82540EM), interrupt/MSI receive (no device-IRQ
framework exists; would broaden the stage), DHCP (needs timers and a socket
layer; static config matches both target hypervisor defaults), TCP (out of
scope for the stage), and a blocking shared-drain poll loop outside the lock
(chosen: the implemented per-iteration lock keeps PIT/serial interrupts
flowing).

**Evidence.** Unit tests 128 → 179 (51 new: PCI config/BAR math, codecs, ARP
cache, rejection vectors, known checksums). `scripts/test_net_qemu.sh` PASS
(reproduced twice): `net: e1000 at 00:02.0 bar0 0xfebc0000 size 128 KiB`,
`net: mac 52:54:00:12:34:56`, `net selftest: passed (arp resolved, icmp echo
reply in 0 ms)`, marker `...,vfs,net`; `net` → `tx 3 frames, rx 2 frames,
arp 1/8 entries, errors 0`; `ping 10.0.2.2 -c 3` → 3/3 replies, `0% loss`;
host→guest UDP echo byte-identical. `test_all.sh` PASS=20 FAIL=0 SKIP=4.
VirtualBox 7.2.20 (82540EM NAT): NIC detected, loopback `unsupported`, gateway
ARP unanswered in headless mode — recorded honestly, non-fatal, no claim made.
One bring-up defect found by the harness and fixed: ICMP checksum computed
before the payload pattern was written (NETWORK.md §11).

**Consequences.** The OS now talks on a real (emulated) wire with its own
driver and protocol code. Limits stay as documented in `docs/NETWORK.md` §10:
polled receive (packets processed only while a command polls), static routing
to the local subnet only, one NIC on bus 0, no TCP/DHCP/DNS, fragmented IPv4
rejected, no RX offload. Reconsider when interrupt-driven devices (Phase 9) or
a socket/NIC-abstraction layer exist.

## ADR-0019 – PE32+ loading of Windows-target fixtures, real-linker only (Phase 7, stage 1)

**Context.** Phase 7's goal is a PE loader followed by minimal native Windows
console compatibility. Before any Win32 API work, the loader itself must be
proven with genuine PE images: the format as produced by a real toolchain,
parsed by host-tested code, mapped with W^X policy and **base-relocated**,
without ever involving Wine, MinGW, the Windows SDK or a Windows VM. The full
contract (pinned serial lines, test plan, limits) lives in
`docs/PE_LOADER.md`; the research record is R-013.

**Decision.**

- **Fixtures are genuine rustc + rust-lld output, built from our own Rust
  source.** `userland/pe/pehello` (separate workspace, `#![no_std]`,
  `#![no_main]`) is compiled for `x86_64-pc-windows-msvc` and linked by the
  `rust-lld` that ships inside the pinned Rust toolchain, with
  `/nodefaultlib /entry:_start /subsystem:console` — no CRT, no MSVC, no
  MinGW, no Windows SDK. A hand-written PE emitter was rejected: it would
  test our own simplification instead of real linker output.
- **Load at the fixed 1 MiB code window (`0x400000`), always apply
  relocations.** The fixture links at the lld default base (`0x140000000`),
  so the delta is nonzero and the DIR64 path runs in every boot. The fixture
  verifies the fixup itself: a `global_asm!` `.quad marker` slot in `.data`
  (Rust const-eval forbids a pointer-initialized static) must match the
  runtime `lea` address of `marker`; a mismatch prints
  `pe: base relocation mismatch` and exits 1, failing the selftest loudly.
- **Reject-by-directory policy.** Images with non-empty import, delay-import,
  TLS or CLR directories are rejected with named labels; export, resource,
  exception (`.pdata`), debug, load-config and certificate directories are
  ignored (they have no runtime effect until SEH/Win32 services exist). This
  keeps "runs" from silently meaning "runs with missing semantics".
- **Two-phase loader** (`inspect` then `map`) mirroring `kernel/src/loader.rs`:
  the corrupt-module probe (`pebad.exe`, signature zeroed) fails before any
  page is allocated.
- **W^X is kernel policy, not parser policy**: `MEM_EXECUTE|MEM_WRITE`
  sections are rejected (`WriteExecuteSection`); mapping uses
  `USER_CODE`/`USER_DATA`/`NO_EXECUTE` exactly as the ELF loader does.
- **DIR64 relocation targets must lie in a writable section and not cross a
  page** (`RelocationTarget` otherwise) — both fixtures and real console
  images satisfy this; it keeps the fixup a single translated 8-byte write.
- **Marker token `,pe` after `,elf`**, gated on the PE selftest verdict, so
  every existing marker test keeps its suffix semantics.

**Alternatives considered.** Hand-emitted PE fixtures via a `tools/pegen`
(less fidelity, more code to maintain); loading at the preferred base
(destroys the relocation evidence); PE32 (32-bit) support (no consumer, the
target is x86-64); accepting import/TLS directories and ignoring them
(silently wrong semantics later); mapping relocation targets through a second
mapping of the target page (needs page-table walking that `AddressSpace`
does not expose; rejected as stage-1 scope creep).

**Evidence.** Observed (recorded with logs in `progress.md`):
`scripts/test_pe_qemu.sh` PASS with `pe: pehello.exe loaded at 0x400000
(entry 0x401000, 4 sections, 1 relocation applied)`, fixture lines
`pe: hello from a PE32+ image via int 0x80` / `pe: base relocation verified`,
`user: /boot/pebad.exe rejected (bad signature)`, `pe selftest: passed (...)`,
marker `...,user,elf,pe,vfs`; unit tests for the parser and builder; full
`test_all.sh` run including VirtualBox.

**Consequences.** The OS now executes its second real executable format with
its own parser and loader, and the relocation engine required by ASLR-style
loading exists (though no ASLR is implemented). Limits stay as stated in
`docs/PE_LOADER.md` §1/§9: no imports/DLLs/exports, no Win32 API yet (stage 2
of Phase 7), no SEH/TLS/CLR, no PE32, fixed window, one thread at a time,
bootloader-provided modules only. No compatibility claim of any kind is made
from this stage; the word "Windows" in the roadmap refers to this format and
these fixtures, not to application support.

*Superseded in part by ADR-0020:* the reject-by-directory rule for the
**import** directory (index 1) no longer applies as of stage 2; imports are
parsed and resolved against a fixed allowlist. Delay imports, TLS and CLR
are still rejected.

## ADR-0020 – Import-mediated Win32 console surface via user-space stubs (Phase 7, stage 2)

**Context.** Stage 1 proved PE32+ parsing, mapping and base relocations with
import-free fixtures. Stage 2 must make the *import mechanism* real: a
Windows-target image declares `kernel32.dll` functions and expects the
loader to fill its IAT, while the project rules forbid Wine, binary
translation and Windows code. The full contract (pinned serial lines, test
plan, limits) lives in `docs/WIN32_CONSOLE.md`; the research record is R-014.

**Decision.**

- **Imports are produced by `raw-dylib`, not by an import library.**
  `userland/pe/peconsole` declares `#[link(name = "kernel32", kind =
  "raw-dylib")] extern "system"` functions, so `rustc` + `rust-lld` emit a
  genuine import descriptor, ILT and IAT directly (probe: 3 names, hint 0).
  A hand-built import library or a hand-patched `.idata` section was
  rejected: both test our own emitter instead of the real toolchain path,
  and `raw-dylib` needs nothing beyond the pinned toolchain.
- **The IAT is patched through the HHDM, before it is ever executed
  through the mapping.** `rust-lld` places IAT slots in read-only `.rdata`
  (probe); real Windows applies import fixups before final section
  protections. The loader therefore writes each resolved address via the
  physical frame like base relocations do, and never makes `.rdata`
  writable. Rejected alternative: requiring a writable IAT section (would
  reject real toolchain output).
- **Resolution is an exact-name allowlist, not a search.** DLL `kernel32.dll`
  only; functions `GetStdHandle` (service 3), `WriteConsoleA` (service 4),
  `ExitProcess` (service 5) only; no ordinals, no `TimeDateStamp` binding,
  no forwarders, no API-set names. Unknown DLL or function is a named
  rejection (`unknown import dll` / `unknown import function`) *before* any
  page is allocated. Rejected alternatives: ordinal support (no consumer,
  no ordinal table work wanted yet), case-insensitive names (Windows is
  case-insensitive, but a byte-exact allowlist is easier to audit; revisit
  when real third-party modules are in scope).
- **User-space stub page + `int 0x80`, never kernel code called from CPL3.**
  One RX page at `0x51_0000` holds deduplicated stubs of the form
  `mov eax, service; int 0x80; ret`; IAT slots receive the stub addresses.
  This keeps the page-table isolation property (the whole kernel stays
  supervisor-only) and reuses the existing gate, whose `rcx/rdx/r8/r9`
  find the Win64 argument registers already in place. Rejected
  alternatives: pointing the IAT directly at kernel functions (requires
  mapping kernel text as user-executable — disqualifying), syscall-less
  function-pointer callbacks (no privilege boundary left).
- **The API surface is three functions with honest partial semantics.**
  `GetStdHandle` returns `1` for `STD_OUTPUT_HANDLE` and
  `INVALID_HANDLE_VALUE` otherwise; `WriteConsoleA` validates like the
  syscall `write` (handle `1`, `MAX_WRITE` cap, user-page-validated buffer
  and out-pointer), stores the count and mirrors the bytes to the console;
  `ExitProcess` records the code and ends the thread through `sched::exit`.
  No process object, no DLL detach, no `GetLastError` — the documented
  gap, not a silent one.
- **Marker token `,win32` after `,pe`**, gated on the new self-test verdict,
  keeping every existing marker-suffix test valid.

**Alternatives considered.** Forwarding the IAT to a mapped *copy* of the
DLL code (nothing to copy — no DLL is loaded); emulating `kernel32` entries
as one generic dispatcher with a name lookup in the fixture (moves
resolution out of the loader, which is the actual stage goal); supporting a
wider API set now (`WriteFile`, `GetLastError`, W-variants) — each needs
semantics we do not have; making the stub page writable so the fixture
could patch itself (violates W^X for no benefit).

**Evidence.** Observed (recorded with logs in `progress.md`):
toolchain probe import dump (R-014), `scripts/test_pe_qemu.sh` PASS with the
win32 lines and marker `...,user,elf,pe,win32,vfs`; the two named import
rejections; unit tests for every new parser row; full `test_all.sh` run
including VirtualBox.

**Consequences.** PROJECT_OS now has the import-resolution *mechanism* and
three native reimplementations behind it. This is still not Windows
application compatibility: no third-party binary runs, no real DLL is
loaded, no handle/process semantics exist beyond the single console handle,
and the allowlist means an image importing anything else is rejected by
design. Limits stay as stated in `docs/WIN32_CONSOLE.md` §1/§9.

## ADR-0021 – Win32 program basics: five-argument stubs, per-thread last-error, loader-owned command line (Phase 7, stage 3)

**Context.** Stage 2 proved the import mechanism with three four-argument
functions but left the obvious console-program facilities unproven: a
five-argument call shape (`WriteFile`), per-thread error state
(`GetLastError`/`SetLastError`), a thread exit path (`ExitThread`), identity
and time queries, and a command line. The full contract (fixture sequence,
pinned serial lines, counter deltas, limits) lives in
`docs/WIN32_PROGRAM.md`; the research record is R-015.

**Decision.**

- **The fifth integer argument flows through `r10`.** The new stub shape is
  `mov r10, [rsp+0x28]; mov eax, service; int 0x80; ret` — the documented
  x64 position of the first stack argument (32-byte shadow space + 8-byte
  return address). `InterruptFrame` already captures `r10`
  (`kernel/src/arch/x86_64/idt.rs:22-23`), so the syscall gate reads it with
  no entry-stub or frame change, and the 13-byte instruction sequence still
  fits the 16-byte stub stride (3 padding bytes instead of 8). Rejected
  alternatives: copying the user shadow space into the interrupt frame
  (larger frame for one call shape), variable-stub-length layout (breaks the
  stride math and the teardown assumptions for no benefit), per-service
  entry stubs that remap registers (an assembly surface we do not need).
- **`last_error` is per-thread scheduler state, not a global.** Each thread
  slot carries `last_error: u32` initialized to `0`; `GetLastError` reads
  and `SetLastError` writes it. The docs are explicit that the last-error
  code is maintained per-thread, so a global atomic would encode the wrong
  semantics into the ABI. Rejected alternative: global atomic (simpler but
  wrong and untestable per-thread later).
- **`WriteFile` returns documented failure values instead of killing for
  argument errors.** Bad handle → `0` + `ERROR_INVALID_HANDLE` (6);
  non-null `lpOverlapped` and zero/oversized length → `0` +
  `ERROR_INVALID_PARAMETER` (87); success does not clear `last_error`. Only
  unmapped buffer/out-pointer spans kill the thread, matching the existing
  memory-safety gate (documented divergence: Windows would raise an access
  violation). Overlapped I/O is never accept-ed — with one synchronous
  console handle the only honest answer is "invalid parameter". Rejected
  alternatives: killing on every error path (no way to exercise
  `GetLastError`), pretending to support overlapped writes (nothing
  asynchronous exists to report through).
- **`GetCurrentProcessId`/`GetCurrentThreadId` return synthetic constants
  `1`/`1`.** There is no NT PID/TID space and no user handle table; the
  value is documented as a constant, not a unique identifier. Rejected
  alternatives: reusing the scheduler thread index (looks unique but is not
  a stable identifier across applications and still not NT), fake
  NT-looking ranges (dishonest precision).
- **`GetTickCount` is the PIT tick count × 10 as `u32`.** The PIT runs at
  100 Hz, inside the documented 10–16 ms resolution range, and the `u32`
  wraps after 49.7 days like the documented `DWORD`; the fixture compares
  wrap-safely. Rejected alternatives: TSC-derived milliseconds (needs
  calibration data as kernel state for one fixture), a fake constant
  (defeats the point of a time source).
- **`GetCommandLineA` returns a loader-written string in the stub-page
  tail.** `map` writes the NUL-terminated real Limine module path (e.g.
  `/boot/pebasics.exe`) at `WIN32_STUB_VA + 0xF00` when it prepares the stub
  page; the service validates, copies out for the log line, and returns the
  fixed VA. The string is honest (it is the path this OS actually loaded
  from), read-only by construction, and freed with the stub page. A path
  longer than 255 bytes is a named rejection (`CommandLineTooLong`), never a
  truncation. Rejected alternatives: fabricating a Windows-shaped
  `C:\...` command line (dishonest — no such path exists here), returning
  an empty string (a command line that lies by omission), building an
  `argv`-style blob (that is parsing, explicitly out of scope).
- **The marker keeps the `,win32` token.** Stage 3 extends the same
  kernel32 allowlist and the same self-test family; a new `,win32prog`
  suffix would force edits to every marker assertion for no added
  information. Rejected alternative: a new marker suffix (churn without
  signal).
- **Stub shapes are two (4-arg, 5-arg); 0- and 1-argument functions reuse
  the 4-arg shape.** Unused argument registers are simply ignored by the
  services, so extra shapes would be dead weight. Rejected alternative: one
  shape per arity (four encodings to test for no behavioural difference).
- **New fixtures only; stage 1/2 bytes and contracts untouched.**
  `pebasics.exe` plus the `pebadfn2.exe` mutant (unknown function `Sleep`)
  join the ISO; `pehello`, `pebad`, `peconsole`, `pebadimp` and `pebadfn`
  are not rebuilt differently and all their pinned lines stay
  byte-identical.

**Alternatives considered.** A wider jump into real compatibility
(implementing `ReadFile`, console modes, dynamic allocation for buffers) —
each needs machinery this stage does not have and would balloon the
contract; returning real NT identifiers — no NT objects exist to identify;
making the allowlist case-insensitive now — no consumer needs it and the
byte-exact list is easier to audit; a third fixture for the error paths —
the single `pebasics` sequence already exercises all four `WriteFile`
outcomes and keeps counter deltas readable.

**Evidence.** PASSED (2026-10-19, QEMU 11.1.0, build `0.1.0-ec04e74f`).
`scripts/build_user_pe.sh`: `pebasics.exe` 4096 bytes, SHA-256
`4a4aa956a3f5519875b43d8a31e91177e3392fa4316ef2ed4d98d70bdea8341b`,
5 sections, 1 relocation, 9 imports; `pebadfn2.exe` 4096 bytes, SHA-256
`9d6082b9792859eba6d4b4ca1eae70c09c46314e79588757cbb74c37e6cd473d`.
`scripts/test_pe_qemu.sh` PASS with the pinned lines of
`docs/WIN32_PROGRAM.md` §7 (`pe: pebasics.exe loaded at 0x400000 (entry
0x401000, 5 sections, 1 relocation applied)`, the full `win32:` program
sequence, `win32 program selftest: passed (...)`,
`user: /boot/pebadfn2.exe rejected (unknown import function)`) and marker
`PROJECT_OS_BOOT_SUCCESS:0.1.0-ec04e74f checks=...,sched,user,elf,pe,win32,vfs`.
Full `scripts/test_all.sh`: `PASS=22 FAIL=0 SKIP=4`. Deviations observed and
fixed during bring-up (none in the final artifact): the stub-page command-line
terminator is now written explicitly (the page is `int3`-filled, so a trailing
zero cannot be assumed), and `sched::wait_all_finished` dropped its
TSC-frequency budget after Limine's reported frequency disagreed with real
`rdtsc` under QEMU; it is bounded by PIT ticks only. Both fixes are recorded in
`progress.md`.

**Consequences.** The native surface grows from three to eleven
`kernel32.dll` names covering the first five-argument call, thread exit and
per-thread error state. This remains a mechanism, not compatibility: IDs are
constants, overlapped I/O does not exist, unmapped buffers kill the thread,
and the allowlist still rejects every unknown import — all divergences are
documented in `docs/WIN32_PROGRAM.md` §9 rather than hidden.

## ADR-0022 – Registry-like configuration store as one ordinary VFS file with a host-tested codec (Phase 8, stage 1)

**Context.** Phase 8 needs a persistent, registry-like configuration store.
The checkpoint persistence (Phase 6, stage 2) already carries a whole VFS tree
in two CRC-verified double-buffered slots with fallback; any second on-disk
store would duplicate that machinery and create two consistency domains. The
full contract (format, caps, shell surface, script changes) lives in
`docs/CONFIG.md`.

**Decision.**

- **The store is one ordinary VFS file, `/etc/config`.** Its contents are
  produced by a new host-tested codec (`os_core::config`, `POCF` version 1)
  and ride the existing checkpoint save/load/fallback paths with **no format,
  header, slot or CRC change**. Rejected alternatives: a v2 checkpoint payload
  with a dedicated config section (format migration, CRC and decoder churn for
  one file's worth of benefit); a separate reserved disk region (a second
  store to version, verify and corrupt-test); VFS directories as keys with
  values in file contents (64-node cap makes a fat registry impossible,
  listing/overwrite semantics get clumsy, and the on-disk byte layout would
  still need a spec).
- **Format caps are enforced in the codec, never by truncation.** `POCF`
  version 1: 8-byte header (`POCF`, version 1, entry count), then per-entry
  `key_len u16 | value_len u16 | key | value`; ≤64 entries, keys ≤96 bytes of
  `/`-separated `[A-Za-z0-9_.-]` components (each 1–32 bytes), values ≤128
  printable ASCII bytes, whole blob ≤7984 bytes so it always fits the
  8192-byte VFS file cap. Encode is canonical (key-sorted, so byte-identical
  for identical content); decode accepts any order but rejects duplicates,
  truncation, trailing bytes, bad entries, bad counts and oversized blobs with
  distinct labels. Rejected alternatives: a `FileTooBig`-only failure path
  (loses the failure reason at the shell), unlimited values (would let a
  single `config set` fail at the VFS layer after mutating kernel state).
- **Defaults are seeded in memory; the file is written lazily by the first
  mutation.** Boot with no `/etc/config` installs `System/Name=PROJECT_OS`,
  `Display/Theme=classic`, `Net/Mode=static` and does **not** dirty the VFS,
  so an idle boot leaves the checkpoint byte-identical and the persistence
  test's save sequence is exactly the mutations the script performs. Rejected
  alternatives: writing defaults at first boot (silently changes the disk in
  a boot the tests use to prove no writes happen; also shifts the four-boot
  seq expectations), no defaults (an empty registry is a worse first-run
  experience and makes the shell test assert nothing).
- **`rm /etc/config` is allowed and means "factory settings".** The next boot
  reseeds defaults in memory; the file reappears on the next mutation.
  Special-casing the path as read-only would add a VFS rule for one name.
- **The boot self-test mutates only memory.** It sets/gets/lists/removes a
  temporary `SelfCheck/Value` entry, checks invalid-key rejection and an
  encode/decode round trip, then asserts the entry count is restored. No
  file write, no dirty bit: a failed self-test is boot-fatal
  (`PROJECT_OS_FATAL`), and a passed one can never alter what is persisted.
- **The marker gains `,config` directly after `,vfs`; disk boots read
  `...,vfs,config,blk,persist`.** Appending after `,vfs` keeps the existing
  `elf`/`pe`/`win32` substring assertions byte-valid and puts the
  always-present components together, with the conditional disk components in
  their existing order.
- **No Win32 access path in this stage.** There is no `advapi32`, no `Reg*`
  service, no hive semantics and no typed values; the store is a native
  kernel facility with a shell surface. A later stage may expose a scoped
  subset, but nothing in this decision claims registry compatibility.

**Alternatives considered.** Implementing the Windows registry API directly
(no NT object/handle model exists to hang `HKEY_*` on, and typed values would
double the format for no current consumer); an append-only log format (CRC
and fallback already give all-or-nothing atomicity at the tree level;
per-entry journals add compaction work); case-insensitive keys (no consumer
needs it and byte-exact comparisons keep the codec auditable).

**Evidence.** PASSED (2026-10-19, QEMU 11.1.0, build `0.1.0-82bc098b`).
`scripts/test_unit.sh`: 269/269 (17 new `os_core::config` tests: key/value
validation tables, length caps, sorted insert/overwrite/remove, prefix
listing, entry-count and blob caps, canonical encode, round trips, empty
store, unsorted-input acceptance, header/truncation/trailing/bad-entry/
duplicate rejections, stable labels). `scripts/test_shell_qemu.sh` PASS with
25 commands (the eight new ones: `config` → `3 entries (/etc/config, ...)`,
`config get`, `config set`, `config get` after set, `config list` → 3
entries, `config del`, missing key → `error: no such key`, `config set
bad//key x` → `error: bad key`). `scripts/test_persistence_qemu.sh` PASS
(4 boots): boot 1 `write /tmp/hello.txt` → `saved slot B seq=1`,
`config set Display/Theme light` → `saved slot A seq=2`; boot 2 loads A
seq=2 with theme `light`, `mkdir` → B seq=3, `write /tmp/notes/a.txt` →
A seq=4, `config set Display/Theme dark` → B seq=5; boot 3 with slot B's
payload zeroed → `persist: slot B invalid (payload crc mismatch)`,
`persist: loaded slot A seq=4`, theme reads `light` (dark rolled back with
the corrupt slot) and `/tmp/notes/a.txt` reads `alpha`; boot 4 both slots
corrupt → fresh format, `/tmp/hello.txt` gone, theme `classic` (defaults
reseeded). Boot logs: `config selftest: passed (3 entries, 75 bytes;
get/set/remove/list/encode/decode/error paths)` (73 bytes in the boot-3 log
because `light` is two bytes shorter than `classic`, an incidental
confirmation of the loaded value). Marker
`PROJECT_OS_BOOT_SUCCESS:0.1.0-82bc098b checks=...,sched,user,elf,pe,win32,vfs,config`
(disk boot adds `,blk,persist,net`). Full `scripts/test_all.sh`:
`PASS=22 FAIL=0 SKIP=4`. Artifacts: `kernel.elf` 1739896 bytes SHA-256
`8de3faa87c3113dbbc474d5efcd438187143ac5fc281f3e6b2eec669ad8ab608`,
`project-os.iso` 7776256 bytes SHA-256
`cded5276dae6e9c8495618f5206cd02c0daf39a0451d6dd99b035cf27e105699`,
`persist-disk.img` after the four boots SHA-256
`05d2d3ad26aa59c8140d1195e6e8423e19206895c6ed4dbe7c0aff0c0c245d22`.
Two host-test iterations during development (an initial 6-byte header
assumption corrected to 8 bytes, and a cap-boundary assertion refined); none
of it reached the final artifact. No device-level bring-up defect occurred
in this stage.

**Consequences.** PROJECT_OS now has a persisted hierarchical configuration
store with defined limits, clean error labels and a shell surface, at the
cost of one file inside the existing checkpoint. The store is not a Windows
registry: no `advapi32`/`HKEY_*` API, no hives, no typed values, no ACLs or
per-user separation, no transactions or change notifications. Because it is
an ordinary file, its durability is exactly the checkpoint's: it survives
reboots, rolls back when the newest slot is corrupt, and resets to defaults
after a fresh format. A future format change must bump `VERSION` and update
`docs/CONFIG.md` §2 before any parser change.

