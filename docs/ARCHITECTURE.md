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
| `kernel/src/limine.rs` | protocol structs, request block, string access | working |
| `kernel/src/serial.rs` | COM1 115200 8N1, locked logging, lock-free panic path | working |
| `kernel/src/display.rs` | validated framebuffer drawing and dashboard | working (boot framebuffer only) |
| `kernel/src/font.rs` | original 5x7 bitmap font | working |
| `kernel/src/memory.rs` | usable-RAM-only bitmap frame allocator + self-test | working |
| `kernel/src/arch/x86_64/gdt.rs` | GDT, TSS, IST1 double-fault stack | working |
| `kernel/src/arch/x86_64/idt.rs` | 32 exception gates + IRQ0, frame dispatcher | working |
| `kernel/src/arch/x86_64/pic.rs` | 8259 remap/mask, IRQ0 unmask, EOI | working |
| `kernel/src/arch/x86_64/pit.rs` | 8254 channel 0 at 100 Hz, tick counter | working |
| `kernel/src/arch/x86_64/paging.rs` | narrow uncached MMIO mapping through the live 4-level tables | working, not a VM manager |
| `kernel/src/arch/x86_64/apic.rs` | xAPIC mode + LINT0 ExtINT for the 8259 path | working |
| `kernel/src/sync.rs` | single-CPU spin lock | working, not IRQ-safe |
| `kernel/src/panic.rs` | panic report and halt | working |
| `os_core::memmap` | classification/accounting/validation | host-tested |
| `os_core::fb` | geometry, pixel format, bounds, colour packing | host-tested |
| `os_core::frame` | bitmap frame allocator | host- and in-VM-tested |
| `os_core::paging` | 4-level index/entry math and MMIO flags | host-tested |

## Address-space policy (current)

The kernel runs on the bootloader-provided 4-level page tables. It does not yet
create its own address space, split huge pages, unmap, or enter user mode. HHDM
mappings are used to write/read allocated physical frames and to walk the live
tables. One narrow exception exists: `paging.rs` maps the APIC MMIO page
uncached because Limine's HHDM does not cover it, so the kernel can program
LINT0 ExtINT (ADR-0011). That mapper installs permanent leaves only and is not a
general VM manager.

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
        vector 32 -> PIT tick++, EOI
        vector 3  -> breakpoint self-test flag
        other     -> serial fatal dump (vector, error, rip, cs, rflags; CR2 for #PF)
   -> restore GPRs, discard vector/error, iretq
```

Vector 8 (double fault) uses IST1 from the TSS. Same-ring frames contain only
RIP/CS/RFLAGS; a user-mode frame unwinder is future work.

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
- Bootloader page tables plus a narrow MMIO leaf mapper; no kernel virtual-memory
  manager, unmap, huge-page splitting or W^X enforcement for new mappings
  (Limine's segment permissions are in effect).
- Legacy PIC + PIT; xAPIC is enabled only to route LINT0 ExtINT. No APIC
timer, IOAPIC, MSI or HPET.
- No kernel heap, no user mode, no syscalls, no process model.
- No filesystem, block device, PCI enumeration or driver model.
- No reclaim of bootloader-reclaimable memory.
- Spin locks are not interrupt-safe; no lock is taken from an interrupt handler.
- Framebuffer only, 16/24/32 bpp formats, software drawing.
- The 64 GiB bitmap cap is a bring-up constraint, not an allocator requirement.

These are deliberate scope boundaries, not claims that the missing subsystems
work.
