# Windows application compatibility (future)

Status: **not implemented** as application compatibility. Phase 7 stages 1–3
and Phase 8 stages 2–4 provide a PE32+ loader and an import mechanism for two
allowlisted DLLs: twenty-one `kernel32.dll`/`advapi32.dll`-shaped services (a
console surface, program basics such as `WriteFile`, per-thread
`GetLastError`/`SetLastError`,
`ExitThread`, `GetTickCount` and `GetCommandLineA`, a blocking line-mode
console `ReadFile`, a read-only registry surface
`RegOpenKeyExA`/`RegQueryValueExA`/`RegCloseKey`, and the registry write
surface `RegCreateKeyExA`/`RegSetValueExA`/`RegDeleteValueA`, whose writes
persist through the config store) resolved through a genuine
import directory and served by
the kernel (`docs/WIN32_CONSOLE.md`, `docs/WIN32_PROGRAM.md`,
`docs/WIN32_INPUT.md`, `docs/WIN32_REGISTRY.md`,
`docs/WIN32_REGISTRY_WRITE.md`). There is still no NT
system-service layer, no real
DLL loading, no general Win32 implementation and no Windows driver support.
This document describes the intended architecture and its strict boundaries
so future work does not silently reintroduce Wine/Proton or translation.

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
| PE/PE32+ loader | sections, alignments, imports/exports, base relocations, TLS, bounds checks | partial (stages 1–3 + Phase 8 stages 2–4): PE32+ parsing/loading, W^X mapping, DIR64 relocations, import directory against an twenty-one-name two-DLL `kernel32.dll`+`advapi32.dll` allowlist, two-, four-, five-, six-, eight- and nine-argument stubs, loader-owned command line; no exports, forwarders, ordinals, delay imports, TLS/SEH or PE32 |
| Native execution | user/kernel boundary, per-process address spaces, W^X | partial: ring-3 `int 0x80` boundary, per-program address space with teardown, W^X; one user thread at a time, no process model |
| Win32 runtime | files, console, process/thread, synchronization, registry-like config, memory | not started; only the eighteen allowlisted call shapes above exist (console input fd `2` plus console output fd `1`; a registry-shaped read/write surface over the kernel `config` store, writes persisted) |
| NT-style services | handles/objects, waitable objects, events/semaphores/mutexes, IPC, tokens/ACLs | not started |
| GUI layer | window manager, messages, text/font, input, GDI-like drawing | not started |
| Direct3D/DXGI | device/resources/command submission/fences | not started |
| Kernel drivers | WDM/WDF/WDDM environment and isolation | research only |

The kernel must eventually expose versioned internal interfaces for processes,
virtual address spaces, waitable objects, handles, IPC, file objects, security
contexts and device resources. None of these NT-shaped interfaces exist yet;
the current kernel has boot-time memory management, a thread table, a ring-3
`int 0x80` gate and a checkpoint VFS, but no objects, handles or IPC.

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

- No process model, handles/IPC, security tokens or multi-process memory
  isolation. (Ring-3 execution with our own `int 0x80` syscalls, a kernel
  thread table and a checkpoint VFS exist; none of them are NT-shaped.)
- PE32+ parsing/loading exists, but not exports, real DLL loading, SEH/TLS,
  PE32 or imports beyond the twenty-one-name two-DLL allowlist.
- No Win32/COM/DXGI/Direct3D runtime.
- No Windows registry API: a native registry-like key/value store exists in
  the kernel and a registry-shaped surface (`advapi32.dll`
  `RegOpenKeyExA`/`RegQueryValueExA`/`RegCloseKey` reads and
  `RegCreateKeyExA`/`RegSetValueExA`/`RegDeleteValueA` writes, four-slot
  `HKLM` handle table, REG_SZ only, writes persisted through the store;
  `docs/WIN32_REGISTRY.md`/`docs/WIN32_REGISTRY_WRITE.md`, ADR-0024/0025)
  serves it, but there are no hives, no enumeration, no key deletion, no
  ACLs/transactions/redirection, no default values, no other roots and no
  typed (DWORD/binary/multi-string) values.
- No audio, printing or installers (networking is a single polled e1000 with
  static addressing, Phase 6).
- No Windows kernel-driver environment.
- No compatibility test corpus has been built or run beyond our own controlled
  fixtures; no third-party Windows binary has been executed.

No claim of "Windows compatible" is made anywhere in this repository.

## References

- PE/COFF format: <https://learn.microsoft.com/en-us/windows/win32/debug/pe-format>
- Windows API index: <https://learn.microsoft.com/en-us/windows/win32/apiindex/windows-api-list>
- Processes and threads: <https://learn.microsoft.com/en-us/windows/win32/procthread/about-processes-and-threads>
- Windows driver documentation: <https://learn.microsoft.com/en-us/windows-hardware/drivers/>
- ReactOS architecture (scope understanding only; no code reuse):
  <https://reactos.org/architecture/>
