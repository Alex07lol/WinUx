# Architecture

Status: **Phase 1 boot milestone plus partial Phase 2/3 foundations.** This
document describes what exists in the repository, not the eventual full OS.

## Boot chain

```text
VirtualBox/QEMU firmware (BIOS or UEFI)
        |
        v
Limine 12.9.3 bootloader (pinned; BIOS El Torito / UEFI El Torito image)
        |  loads ELF64 kernel, maps segments, builds page tables and HHDM,
        |  provides a stack, fills Limine request responses
        v
_start (kernel/src/entry.S)
        |  align stack, clear direction flag, call kmain
        v
kmain (kernel/src/main.rs)
        |  serial -> handoff validation -> framebuffer log console mirror
        |  memory map -> frame allocator self-test
        |  GDT/TSS/IST -> IDT -> int3 self-test
        |  PIC remap -> PIT 100 Hz -> 5 ticks
        v
PROJECT_OS_BOOT_SUCCESS:<build-id> on COM1
        |
        +--> optional QEMU isa-debug-exit (test cmdline token)
        +--> otherwise hlt idle loop
```

The bootloader is not the kernel. The ISO includes Limine's BIOS and UEFI El
Torito images; the kernel itself is our own ELF64 executable.

## Boot handoff contract

The kernel requests **Limine protocol base revision 6** and refuses to run if the
bootloader reports it could not honour that revision (third component of the
base-revision tag left non-zero).

Verified assumptions at entry:

- x86-64 long mode, paging, PAE, NX (where available), WP and PE enabled;
- kernel loaded in the higher half at `0xffffffff80000000`;
- CS = 0x28, DS/ES/SS/FS/GS = 0x30 from the bootloader GDT;
- IF and DF clear, IDT undefined (the kernel installs its own);
- `rsp` points to a bootloader-provided stack of at least the requested 256 KiB;
- the bootloader has exited EFI boot services;
- base revision 5+ also guarantees a defined local-APIC state, which matters:
  Limine enables the LAPIC, so the legacy-PIC bring-up path explicitly disables
  it (see ADR-0005).

The kernel requests only the features it uses: bootloader info, firmware type,
stack size, paging mode, HHDM, command line, executable address, memory map,
framebuffer and TSC frequency. All request/response structures are transcribed
from the official protocol and live in a writable `.limine_requests` section.

## Kernel ELF layout

Verified with `readelf -lW` on the built artifact:

| Segment | Base (virtual) | Permissions | Contents |
|---|---|---|---|
| text | `0xffffffff80000000` | R-X | entry stub, code |
| rodata | `0xffffffff80009000` | R-- | strings, font, constants |
| data | `0xffffffff80010000` | RW- | Limine requests, mutable state, BSS |
| BSS tail | — | RW- | ~2.1 MiB (frame bitmap) |

Entry point is `0xffffffff80000000`. The image is linked with
`-C relocation-model=static` because Limine loads a fixed-address image and does
not process runtime relocations. The request/response pointer block must be
writable, which the segment permissions above guarantee.

## Components

| Module | Responsibility | State |
|---|---|---|
| `kernel/src/entry.S` | stack alignment, call into Rust | working |
| `kernel/src/limine.rs` | protocol structs, request block, module request + `module_by_path`, string access | working |
| `kernel/src/serial.rs` | COM1 115200 8N1, locked logging, lock-free panic path, RX helpers, framebuffer-console tap | working |
| `kernel/src/fbcon.rs` | framebuffer log console: sizing, dirty-row repaint, cursor overlay, readback self-test | working (boot framebuffer only) |
| `kernel/src/memory.rs` | usable-RAM-only bitmap frame allocator, stats, self-test | working |
| `kernel/src/heap.rs` | fixed 8 MiB kernel heap, `#[global_allocator]`, stats/validate | working |
| `kernel/src/vm.rs` | own 4-level PML4, map/unmap/translate, W^X, user-half teardown | working |
| `kernel/src/sched.rs` | 16 kernel threads, round-robin yield + timer preemption | working |
| `kernel/src/syscall.rs` | `int 0x80` dispatch, pointer validation, user-fault kill path; services 3–5 are the Win32-shaped `GetStdHandle`/`WriteConsoleA`/`ExitProcess` | working |
| `kernel/src/user.rs` | ring-3 module life cycle (ELF and PE), iretq trampoline, teardown | working (ELF + PE32+ modules, Win32 console phase) |
| `kernel/src/loader.rs` | two-phase ELF64 module loader (`inspect`/`map`), code window, rollback | working (ADR-0017) |
| `kernel/src/pe_loader.rs` | two-phase PE32+ loader: window/W^X policy, DIR64 base relocations, `kernel32.dll` allowlist, stub page + HHDM IAT patch | working (stages 1–2, ADR-0019/0020) |
| `kernel/src/console.rs` | IRQ4 receive ring, echo and line editing | working (serial only) |
| `kernel/src/vfs.rs` | in-memory arena VFS, boot self-test, dirty tracking, export/replace for checkpoints | working |
| `kernel/src/blk.rs` | ATA PIO block device (PIIX primary master, LBA28, polled) | working (QEMU `-machine pc`) |
| `kernel/src/persist.rs` | double-buffered checkpoint slots, CRC-verified load/save | working |
| `kernel/src/pci.rs` | PCI config mechanism #1: bus-0 scan for the e1000 NIC, BAR0 sizing, command-register enable | working (e1000 only) |
| `kernel/src/e1000.rs` | Intel 82540EM driver: MMIO, reset, MAC, legacy RX/TX rings, polled, counters | working (QEMU/VirtualBox) |
| `kernel/src/net.rs` | network binding: ARP cache, IPv4/ICMP/UDP dispatch, `ping`, `udpecho`, status | working (static config) |
| `kernel/src/shell.rs` | serial shell builtins over kernel services | working (serial only) |
| `kernel/src/arch/x86_64/gdt.rs` | GDT (kernel + user segments), TSS, IST1 double-fault stack | working |
| `kernel/src/arch/x86_64/idt.rs` | exception gates, IRQ0, IRQ4, `int 0x80` gate, frame dispatcher | working |
| `kernel/src/arch/x86_64/pic.rs` | 8259 remap/mask, per-line unmask, EOI | working |
| `kernel/src/arch/x86_64/pit.rs` | 8254 channel 0 at 100 Hz, tick counter | working |
| `kernel/src/arch/x86_64/context.rs` | register/RSP-swap context switch | working |
| `kernel/src/arch/x86_64/paging.rs` | narrow uncached MMIO mapping through the live 4-level tables | working, not a VM manager |
| `kernel/src/arch/x86_64/apic.rs` | xAPIC mode + LINT0 ExtINT for the 8259 path | working |
| `kernel/src/sync.rs` | `SpinLock` and RFLAGS-saving `IrqSpinLock` | working |
| `kernel/src/panic.rs` | panic report and halt | working |
| `userland/` | independent no_std cargo workspace: syscall runtime plus the `hello`/`fault` ELF programs | working (ADR-0017) |
| `userland/pe/` | separate workspace: `pehello` (stage 1) and `peconsole` (`raw-dylib` import fixture, stage 2), linked by rust-lld without an SDK | working (ADR-0019/0020) |
| `os_core::memmap` | classification/accounting/validation | host-tested |
| `os_core::fb` | geometry, pixel format, bounds, colour packing | host-tested |
| `os_core::frame` | bitmap frame allocator | host- and in-VM-tested |
| `os_core::heap` | free-list heap model used by the kernel heap | host-tested |
| `os_core::path` | VFS path validation and component splitting | host-tested |
| `os_core::pofs` | checkpoint header/payload encode, decode, CRC32 | host-tested |
| `os_core::elf` | ELF64 `ET_EXEC` parser: headers, segments, W^X and overlap policy | host-tested |
| `os_core::pe` | PE32+ parser: headers, sections, import directory (ILT/IAT/names), data-directory policy, DIR64 relocations | host-tested |
| `os_core::pci` | config-address construction, BAR decode and sizing | host-tested |
| `os_core::net` | Ethernet/ARP/IPv4/ICMP/UDP codecs, RFC 1071 checksums, ARP cache with expiry | host-tested |
| `os_core::font` | original 5x7 bitmap font (moved from the kernel for host tests) | host-tested |
| `os_core::textcon` | scrolling text-grid model: byte semantics, wrap, dirty rows | host-tested |
| `os_core::textrender` | pitch-aware glyph/cell/rect compositor for 16/24/32 bpp | host-tested |
| `os_core::paging` | 4-level index/entry math and MMIO flags | host-tested |
| `os_core::rng` | deterministic PRNG for tests/benchmarks | host-tested |

## Address-space policy (current)

The kernel builds and switches to its own 4-level PML4 at boot: entries
256..512 (the kernel half) are copied from the bootloader tables, and CR3 is
swapped onto the new tables (`kernel/src/vm.rs`, ADR-0012). `map_page` enforces
W^X (writable implies non-executable), alignment, kernel-half and double-map
refusals; `unmap` issues `invlpg` and releases frames. The bootloader tables are
intentionally left allocated and unused for now; reclaiming bootloader-
reclaimable memory is future work.

Each ring-3 program is an ELF64 executable or a PE32+ image (`userland/`
workspace for ELF, `userland/pe/` for the PE fixture) shipped as a Limine
module. `loader::inspect` / `pe_loader::inspect` parse and policy-check the
module without allocating (sections inside `0x400000..0x500000`, entry
executable, PE W^X and DIR64 relocation policy); `map` then allocates,
zero-fills and maps each section/segment page with W^X flags, applies PE base
relocations when the load base differs from the image's preferred base, and
rolls every page back on failure. The program runs in a fresh address
space cloned from the kernel-half template, with its segments and a zeroed
one-page stack mapped in the lower half with the USER bit. Every syscall
argument is validated against the live CR3 (`vm::translate_live`): canonical
user range and every touched page mapped user-accessible. Teardown unmaps and
frees the load-plan pages, the stack page and the page tables, then reactivates
the kernel PML4 (ADR-0017). There is no demand paging, copy-on-write or
huge-page splitting, and one user thread runs at a time (single TSS `rsp0`).

Physical memory policy:

- Only `LIMINE_MEMMAP_USABLE` ranges are released to the frame allocator.
- Bootloader-reclaimable memory is **not** reclaimed; responses remain valid.
- Kernel image, framebuffer, ACPI/NVS and reserved regions stay unavailable.
- The first 1 MiB is explicitly reserved as a null-pointer guard.
- A 2 MiB static bitmap covers at most 64 GiB of physical address space; higher
  memory is ignored and documented, not silently wrapped.

## Interrupt path

```text
CPU exception / IRQ
   -> isr_N stub (assembly): push error code (0 if none), push vector
   -> isr_common: save 15 GPRs, align stack, call exception_dispatch
   -> Rust dispatcher:
        vector 32  -> PIT tick++, EOI, scheduler tick, reschedule at the tail
        vector 36  -> COM1 receive: drain the UART into the console ring, EOI
        vector 128 -> int 0x80 syscall dispatch (DPL3 gate); iretq to CPL3
        vector 3   -> breakpoint self-test flag
        CPL3 fault -> report, count, kill the user thread (kernel continues)
        other      -> serial fatal dump (vector, error, rip, cs, rflags; CR2 for #PF)
   -> restore GPRs, discard vector/error, iretq
```

Vector 8 (double fault) uses IST1 from the TSS. Same-ring frames contain only
RIP/CS/RFLAGS; the syscall gate's DPL3 entry means ring-3 frames carry SS/RSP
as well (ADR-0014).

## Boundaries and dependency rules

- `os_core` must never contain hardware access, globals or bootloader ABI types;
  it is the host-testable core.
- `kernel` may depend on `os_core`; `os_core` must not depend on `kernel`.
- Architecture-specific code stays under `kernel/src/arch/x86_64`; generic kernel
  policy belongs above it.
- The framebuffer log console is a debugging/UI aid, not a graphics driver. No GPU
  acceleration, command submission or display-driver interface exists.
- Windows/NT compatibility code is limited to what Phase 7 stages 1–2
  implement: the PE32+ loader and three `kernel32.dll`-shaped console services
  resolved through a real import table (`docs/WIN32_CONSOLE.md`). Everything
  beyond that is unimplemented; `docs/WINDOWS_COMPATIBILITY.md` describes the
  intended path.

## Known limitations

- Single CPU only; no SMP startup, no per-CPU state.
- Own 4-level VM manager; no 5-level paging, demand paging, copy-on-write or
  huge-page splitting. The bootloader page tables remain allocated and unused.
- Legacy PIC + PIT; xAPIC is enabled only to route LINT0 ExtINT. No APIC
  timer, IOAPIC, MSI or HPET.
- The kernel heap is a fixed 8 MiB region that does not grow on demand.
- One user thread at a time (single TSS `rsp0`); program images are our own
  ELF64 `ET_EXEC` modules and PE32+ images loaded from Limine, with no
  PIE/dynamic linking, no ASLR, no filesystem-backed exec and no process model
  (ADR-0017, ADR-0019). The PE path resolves imports only against a fixed
  three-name `kernel32.dll` allowlist (stub page + `int 0x80` dispatch,
  ADR-0020): no real DLL loading, exports, forwarders, ordinals, delay
  imports, SEH/unwind, TLS, CLR, resources, PE32 or general Win32 API surface
  exists, so it is not Windows application compatibility (Phase 7 stages 1–2
  of the roadmap).
- The filesystem is a whole-tree checkpoint image: two 1 MiB slots on a
  PIIX-legacy ATA disk, flushed write-through at shell-command boundaries
  (ADR-0016). No journal, no AHCI/virtio, no DMA or IRQ14, and the disk path
  is verified on QEMU `-machine pc` only. The console and shell are
  serial-only; there is no framebuffer console input. PCI access is a
  single-purpose config-space scan for the e1000, not a driver model
  (ADR-0018).
- Networking (ADR-0018, `docs/NETWORK.md`): one polled e1000 with static
  `10.0.2.15/24` on QEMU slirp / VirtualBox NAT — no DHCP, DNS, TCP, sockets,
  interrupts/MSI or routing beyond the local subnet; one NIC on bus 0; IPv4
  only, fragments rejected, no reassembly; the VirtualBox NAT run detected the
  NIC but did not answer the gateway ARP (non-fatal, reported as observed).
- No reclaim of bootloader-reclaimable memory.
- `IrqSpinLock` is safe to share with interrupt handlers on a single CPU; the
  plain `SpinLock` is not and must not be taken from code an interrupt handler
  can preempt.
- Framebuffer only, 16/24/32 bpp formats, software drawing.
- The 64 GiB bitmap cap is a bring-up constraint, not an allocator requirement.

These are deliberate scope boundaries, not claims that the missing subsystems
work.
