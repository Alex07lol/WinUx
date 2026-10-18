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
        |  serial -> handoff validation -> framebuffer dashboard
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
| `kernel/src/serial.rs` | COM1 115200 8N1, locked logging, lock-free panic path, RX status/byte helpers | working |
| `kernel/src/display.rs` | validated framebuffer drawing and dashboard | working (boot framebuffer only) |
| `kernel/src/font.rs` | original 5x7 bitmap font | working |
| `kernel/src/memory.rs` | usable-RAM-only bitmap frame allocator, stats, self-test | working |
| `kernel/src/heap.rs` | fixed 8 MiB kernel heap, `#[global_allocator]`, stats/validate | working |
| `kernel/src/vm.rs` | own 4-level PML4, map/unmap/translate, W^X, user-half teardown | working |
| `kernel/src/sched.rs` | 16 kernel threads, round-robin yield + timer preemption | working |
| `kernel/src/syscall.rs` | `int 0x80` dispatch, pointer validation, user-fault kill path | working |
| `kernel/src/user.rs` | ring-3 ELF module life cycle, iretq trampoline, teardown | working (ELF modules) |
| `kernel/src/loader.rs` | two-phase ELF64 module loader (`inspect`/`map`), code window, rollback | working (ADR-0017) |
| `kernel/src/console.rs` | IRQ4 receive ring, echo and line editing | working (serial only) |
| `kernel/src/vfs.rs` | in-memory arena VFS, boot self-test, dirty tracking, export/replace for checkpoints | working |
| `kernel/src/blk.rs` | ATA PIO block device (PIIX primary master, LBA28, polled) | working (QEMU `-machine pc`) |
| `kernel/src/persist.rs` | double-buffered checkpoint slots, CRC-verified load/save | working |
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
| `os_core::memmap` | classification/accounting/validation | host-tested |
| `os_core::fb` | geometry, pixel format, bounds, colour packing | host-tested |
| `os_core::frame` | bitmap frame allocator | host- and in-VM-tested |
| `os_core::heap` | free-list heap model used by the kernel heap | host-tested |
| `os_core::path` | VFS path validation and component splitting | host-tested |
| `os_core::pofs` | checkpoint header/payload encode, decode, CRC32 | host-tested |
| `os_core::elf` | ELF64 `ET_EXEC` parser: headers, segments, W^X and overlap policy | host-tested |
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

Each ring-3 program is an ELF64 executable (`userland/` workspace, Rust
`no_std`) shipped as a Limine module. `loader::inspect` parses and policy-checks
it without allocating (segments inside `0x400000..0x500000`, entry executable);
`loader::map` then allocates, zero-fills and maps each segment page with W^X
flags and rolls every page back on failure. The program runs in a fresh address
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
- The framebuffer dashboard is a debugging/UI aid, not a graphics driver. No GPU
  acceleration, command submission or display-driver interface exists.
- No Windows/NT compatibility code is present yet. `docs/WINDOWS_COMPATIBILITY.md`
  describes the intended path.

## Known limitations

- Single CPU only; no SMP startup, no per-CPU state.
- Own 4-level VM manager; no 5-level paging, demand paging, copy-on-write or
  huge-page splitting. The bootloader page tables remain allocated and unused.
- Legacy PIC + PIT; xAPIC is enabled only to route LINT0 ExtINT. No APIC
  timer, IOAPIC, MSI or HPET.
- The kernel heap is a fixed 8 MiB region that does not grow on demand.
- One user thread at a time (single TSS `rsp0`); program images are our own
  ELF64 `ET_EXEC` modules loaded from Limine, with no PIE/dynamic linking,
  no ASLR, no filesystem-backed exec and no process model (ADR-0017).
- The filesystem is a whole-tree checkpoint image: two 1 MiB slots on a
  PIIX-legacy ATA disk, flushed write-through at shell-command boundaries
  (ADR-0016). No journal, no AHCI/virtio, no DMA or IRQ14, and the disk path
  is verified on QEMU `-machine pc` only. The console and shell are
  serial-only; there is no framebuffer console input. No PCI enumeration or
  driver model.
- No reclaim of bootloader-reclaimable memory.
- `IrqSpinLock` is safe to share with interrupt handlers on a single CPU; the
  plain `SpinLock` is not and must not be taken from code an interrupt handler
  can preempt.
- Framebuffer only, 16/24/32 bpp formats, software drawing.
- The 64 GiB bitmap cap is a bring-up constraint, not an allocator requirement.

These are deliberate scope boundaries, not claims that the missing subsystems
work.
