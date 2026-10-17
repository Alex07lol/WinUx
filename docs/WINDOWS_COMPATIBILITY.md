# Windows application compatibility (future)

Status: **not implemented.** There is no PE loader, no Win32 implementation, no NT
system-service layer and no Windows driver support. This document describes the
intended architecture and its strict boundaries so future work does not silently
reintroduce Wine/Proton or translation.

## Target execution path

```text
x86-64 Windows PE executable
        |
        v
PROJECT_OS PE/PE32+ loader (first-party)
        |
        v
native x86-64 execution on a compatible CPU (no translation)
        |
        v
PROJECT_OS Win32/COM/DXGI-compatible user-space runtime (first-party)
        |
        v
PROJECT_OS NT-style system services / syscall interface
        |
        v
our kernel: processes, threads, VM, handles, IPC, files, security, drivers
        |
        v
our hardware and driver layer
```

This is a target diagram, not an assertion that any layer exists.

## Non-negotiable rules

- **No Wine, Proton, Winlator or Wine-derived runtime** may be installed,
  bundled, wrapped or invoked.
- **No QEMU user-mode emulation, binary translation, rewriting or CPU emulation**
  on the x86-64 path. Instructions execute natively.
- No Windows VM as the product compatibility mechanism. VirtualBox is only a
  development/test environment for our OS.
- Reimplement Windows-compatible behavior ourselves from public specifications
  and tests. Do not copy Wine, ReactOS or proprietary implementation code.
- Do not use proprietary Windows binary DLLs as a shortcut.
- API compatibility and kernel-driver compatibility are separate milestones.

## Layers and required kernel primitives

| Layer | Needs | Status |
|---|---|---|
| PE/PE32+ loader | sections, alignments, imports/exports, base relocations, TLS, bounds checks | not started |
| Native execution | user/kernel boundary, per-process address spaces, W^X | not started |
| Win32 runtime | files, console, process/thread, synchronization, registry-like config, memory | not started |
| NT-style services | handles/objects, waitable objects, events/semaphores/mutexes, IPC, tokens/ACLs | not started |
| GUI layer | window manager, messages, text/font, input, GDI-like drawing | not started |
| Direct3D/DXGI | device/resources/command submission/fences | not started |
| Kernel drivers | WDM/WDF/WDDM environment and isolation | research only |

The kernel must eventually expose versioned internal interfaces for processes,
virtual address spaces, waitable objects, handles, IPC, file objects, security
contexts and device resources. None are implemented yet; the current kernel has
only boot-time memory and interrupt foundations.

## Test corpus strategy

Compatibility claims must be evidence-based and narrowly scoped:

1. Build controlled x86-64 Windows console test executables from our own tiny
   fixtures (no third-party binaries with unclear redistribution rights).
2. Start with a documented subset: process start/exit, console I/O, file open/
   read/write, heap allocation, thread creation/synchronization, environment.
3. Record a compatibility matrix per executable and API group, with expected and
   observed outcomes.
4. Expand API coverage from real application requirements, not API count.
5. Keep malformed-PE tests (bad offsets, overlapping sections, relocation
   overflow) in CI.

Loading a PE successfully is **loader success**, not application compatibility.
Running a console program is not GUI, Direct3D or driver compatibility.

## Driver strategy

- Do not load arbitrary Windows `.sys` drivers into the kernel. A driver expects
  NT/WDM/WDF services, object semantics, DMA, interrupts, power and Plug and
  Play, and—for graphics—WDDM/DXGKRNL behavior.
- Define driver isolation, validation and recovery before accepting third-party
  privileged code.
- Windows display-driver compatibility is a separate, high-risk research path
  (see `docs/GRAPHICS.md`). It must not be implied by user-mode API work.

## Current gaps

- No user mode, syscalls, process/thread model, handles, IPC, VFS or security.
- No PE parser/loader.
- No Win32/COM/DXGI/Direct3D runtime.
- No registry-like configuration store.
- No networking, audio, printing or installers.
- No Windows kernel-driver environment.
- No compatibility test corpus has been built or run.

No claim of "Windows compatible" is made anywhere in this repository.

## References

- PE/COFF format: <https://learn.microsoft.com/en-us/windows/win32/debug/pe-format>
- Windows API index: <https://learn.microsoft.com/en-us/windows/win32/apiindex/windows-api-list>
- Processes and threads: <https://learn.microsoft.com/en-us/windows/win32/procthread/about-processes-and-threads>
- Windows driver documentation: <https://learn.microsoft.com/en-us/windows-hardware/drivers/>
- ReactOS architecture (scope understanding only; no code reuse):
  <https://reactos.org/architecture/>
