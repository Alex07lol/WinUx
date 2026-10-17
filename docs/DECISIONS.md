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
