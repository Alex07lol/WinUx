# Memory management

Status: **physical frame allocation only.** There is no virtual-memory manager,
kernel heap, page cache, reclaim or user-space memory yet. This document
describes what is implemented, what was measured, and what is explicitly not
claimed.

## Allocation layers (kept separate)

| Layer | Purpose | Status |
|---|---|---|
| Early boot allocation | temporary data before the frame allocator exists | not needed yet; bootloader stack and static kernel BSS suffice |
| Physical frame allocator | owns 4 KiB frames of usable RAM | **implemented** (`os_core::frame`, `kernel/src/memory.rs`) |
| Virtual memory manager | page tables, permissions, address spaces | not implemented; kernel runs on Limine page tables with a narrow permanent MMIO leaf mapper for the APIC (ADR-0011) |
| Kernel heap / slab | variable-size kernel objects | not implemented |
| User virtual memory / heap | process address spaces and `malloc` | not implemented (no user mode) |
| Page cache / reclaim | file-backed caching and pressure handling | not implemented |

Conflating these layers is a design error; each will be introduced only when its
prerequisites are testable.

## Physical frame allocator

### Data structure

- One bit per 4 KiB frame; **1 means in use**.
- Backed by a 2 MiB static bitmap, supporting at most 64 GiB of physical address
  space. Frames above the cap are ignored, not wrapped.
- The allocator starts with every bit set. Only ranges explicitly passed as
  `LIMINE_MEMMAP_USABLE` are cleared. This makes implicit allocation of
  bootloader-reclaimable, ACPI, framebuffer, kernel or reserved memory
  impossible by construction.
- The first 1 MiB is explicitly reserved after usable regions are released. The
  guard is defensive; Limine may legally report low memory as usable.

### Allocation

- `alloc_pages(count)` requires a power-of-two count and returns a naturally
  aligned block (`address % (count * 4 KiB) == 0`).
- A rotating cursor provides first-fit allocation without restarting from zero
  every time.
- `alloc()` is `alloc_pages(1)`.
- Exhaustion returns `None`; it does not panic or corrupt state.

### Free

`free_pages(frame, count)` validates all of the following before mutating:

- count is non-zero and a power of two;
- the frame address is aligned to `count * 4 KiB`;
- the whole block lies inside the managed window;
- every frame in the block is currently marked used.

A double free therefore returns `NotAllocated` and changes nothing. Partial
freeing is likewise rejected.

### Invariants

1. `free_frames` equals the number of zero bits in `[0, frame_limit)`.
2. No frame outside an explicitly usable, non-reserved range is ever returned.
3. No allocated block overlaps another allocated block.
4. Free with an invalid shape or ownership never partially mutates the bitmap.
5. Accounting returns to the exact baseline after every successful alloc/free
   pair.

These are checked by host tests and by the in-kernel self-test.

## Tests

Host tests (`cargo test -p os_core frame::`) cover:

- initial state: everything reserved, allocation returns `None`;
- usable-region release and idempotence;
- unaligned region rejection and address overflow;
- aligned, distinct single-frame allocations;
- low-memory reservation never handed out;
- contiguous allocation alignment;
- exhaustion and recovery after free;
- double-free and never-allocated free rejection;
- invalid free counts and out-of-range blocks;
- physical-limit and bitmap-capacity clamping;
- 20 000-operation seeded churn with no leak, and a 256-frame contiguous block
  available afterwards;
- largest-free-run reporting.

Host tests (`cargo test -p os_core paging::`) cover the 4-level page-table math
used by the narrow MMIO mapper: index extraction, canonical-address checks,
entry address masking/alignment and MMIO flag composition. The mapper itself is
exercised at boot in both QEMU and VirtualBox.

In-kernel self-test (runs before the success marker):

- single frame and 4-frame block allocation;
- both above the 1 MiB guard;
- natural alignment and non-overlap;
- write/read-back of a 16 KiB pattern through the HHDM;
- free and exact accounting restoration;
- double-free and non-power-of-two allocation rejection.

The in-VM test is real evidence that returned frames are mapped and writable;
the host tests alone would not prove that.

## Host benchmark

Command:

```sh
scripts/bench_memory.sh --mib 64 --iterations 200000 --seed 8388610
```

This benchmarks the **host model** of the allocator (release build, heap-backed
bitmap), not the kernel build and not Linux. Results are comparable only across
identical arguments, compiler, machine and load.

Recorded environment and result (2026-10-17):

| Item | Value |
|---|---|
| Host OS / arch | Windows, x86-64 |
| Build | `cargo run --release`, Rust 1.99.1 (commit 58cf8f9c); workspace release profile: `opt-level=2`, `debug=1`, `lto="thin"`, `panic="abort"` |
| Model size | 64 MiB, 48 MiB usable (12 288 frames) |
| Workload | 200 000 iterations, mixed orders 1/2/4, max 4096 live blocks, seed `0x800002` |
| Operations | 200 200 (100 100 allocations, 100 100 frees) |
| Elapsed | 2.970 ms |
| Mean | 14.8 ns/op |
| Batch p50 / p99 | 14 ns / 17 ns per op (64-op batches) |
| Post-churn | 12 288 free frames, largest run 12 288, fragmentation ratio 1.0000 |
| Accounting | OK, all frames returned |

Interpretation: at this small working set the bitmap scan is cache-friendly and
the seeded workload leaves no fragmentation. This is **not** evidence that the
allocator is faster than Linux, a buddy allocator or a slab allocator. It is a
baseline for future before/after comparisons.

## Debugging and failure behavior

- Double free and invalid free are errors, not silent no-ops.
- Overflow is checked in region release and allocation size calculations.
- The kernel reports allocator statistics and self-test failure over serial; a
  failed self-test is fatal before the success marker.
- The bitmap is zero-initialised BSS, so it does not inflate the kernel file; it
  does add ~2.1 MiB of loaded memory.

## Known limitations

- No general page-table manipulation: unmapping, permission changes, huge-page
  splitting and 5-level tables do not exist. The only kernel-installed mappings
  are permanent uncached MMIO leaves for the APIC; HHDM is used for the frame
  self-test.
- No buddy orders/coalescing structure; contiguity is found by scanning.
- O(n) first-fit; acceptable for bring-up, unmeasured for large fragmentation.
- 64 GiB cap and a single global locked allocator; no NUMA, no per-CPU caches,
  no SMP concurrency.
- No reclaim of bootloader-reclaimable memory; responses must stay valid.
- QEMU boots were recorded at 256 MiB, 512 MiB and 1 GiB, and VirtualBox at the
  default 512 MiB. Larger sizes and NUMA/hotplug are not tested.
- No kernel heap or user allocator, so no claim about `malloc` behavior.

## Next steps, only with evidence

1. Add virtual-memory mapping helpers and move the kernel to its own page tables
   (Phase 3), with permission and W^X tests.
2. Add a kernel heap with a simple correct allocator, then size classes/slabs
   only if measurements justify it (Phase 4).
3. Add buddy orders or per-CPU caches only after a fragmentation/contention
   benchmark shows a real problem, with before/after numbers.
4. Add reclaim and page cache much later, after user mode and storage exist.

Linux's documented buddy/slab/per-CPU designs are references for evaluating
trade-offs, not source to copy. Linux kernel code is GPL-2.0-only; this project
implements its own allocator.
