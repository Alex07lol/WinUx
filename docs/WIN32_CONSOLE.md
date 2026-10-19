# Native Win32 console surface (Phase 7, stage 2) — design spec

Status: **draft for implementation 2026-10-18.** This document is the contract
for the stage: code, tests and logs must match it, and any deviation must be
recorded here or in `docs/DECISIONS.md`.

## 1. Goal and non-goals

Goal: make **imports real**. A PE32+ fixture declares three Win32 console
functions from `kernel32.dll` (`GetStdHandle`, `WriteConsoleA`, `ExitProcess`)
the way a Windows-target program does — via a genuine **import directory and
IAT produced by `rust-lld`** (`raw-dylib` linkage, no import library file) —
and our kernel resolves the names to its own native implementations. From
serial evidence the boot self-test must prove:

- `peconsole.exe` runs at CPL3: `GetStdHandle(STD_OUTPUT_HANDLE)` returns the
  kernel's console handle, two `WriteConsoleA` calls each write their exact
  byte count and report it back through the out-parameter, `ExitProcess(0)`
  ends the thread — all through the import table, not direct syscalls;
- imports resolve only against a fixed allowlist: a `kernel32.dll` import of an
  **unknown function** (`pebadfn.exe`) and an import of an **unknown DLL**
  (`pebadimp.exe`) are each rejected with a named label before any page is
  allocated;
- the success marker appends `,win32` after `,pe` only when this self-test ran.

Deliberately **not** in this stage:

- no `LoadLibrary`/`GetProcAddress`, no DLL files loaded or mapped, no
  exports, no forwarders, no ordinal imports, no delay imports, no API sets;
- no general Win32 API surface: exactly three `kernel32.dll` functions, one
  fabricated console handle (value `1`), no process/thread/handle table, no
  `GetLastError`, no `WriteFile`/`ReadFile`, no console input;
- no CRT: the fixtures are still `#![no_std]`, entry `_start`, `/nodefaultlib`;
- no exception machinery: `ExitProcess` ends the single user thread through
  the existing scheduler-kill path; there is no DLL-detach, no process object;
- no ASLR, no new memory policy: the stub page joins the existing fixed
  window layout, one user thread at a time, Limine modules only;
- no compatibility claim of any kind. This stage proves the **mechanism**
  (import parsing, name resolution, stubs, dispatch) against our own
  implementations; it does not run any third-party Windows binary.

## 2. Research and primary sources (R-014)

Recorded fully in `docs/RESEARCH.md` as R-014. Verified facts:

- **Import directory** (Microsoft *PE Format* specification): directory index
  1; an array of 20-byte `IMAGE_IMPORT_DESCRIPTOR` records terminated by an
  all-zero record: `OriginalFirstThunk` (ILT RVA), `TimeDateStamp`,
  `ForwarderChain`, `Name` (DLL name RVA), `FirstThunk` (IAT RVA). Lookup
  order: ILT, falling back to the IAT when `OriginalFirstThunk` is zero. Each
  thunk is 64-bit: bit 63 set means **import by ordinal** (`0..15` ordinal);
  otherwise the low 31 bits are an RVA to `IMAGE_IMPORT_BY_NAME` (a 16-bit
  hint followed by a NUL-terminated name). The loader walks the table and
  overwrites each IAT slot with the resolved function address.
- **`GetStdHandle`** (Microsoft Learn, fetched 2026-10-18):
  `HANDLE GetStdHandle(DWORD nStdHandle)`; `STD_OUTPUT_HANDLE` is
  `((DWORD)-11)` = `0xFFFF_FFF5`; failure returns `INVALID_HANDLE_VALUE`
  (`(HANDLE)-1`). No validation of the handle value happens here.
- **`WriteConsoleA`** (Microsoft Learn, fetched 2026-10-18):
  `BOOL WriteConsoleA(HANDLE, const VOID *lpBuffer, DWORD n, LPDWORD written,
  LPVOID reserved)`; nonzero on success, zero on failure; `lpReserved` must
  be NULL; despite "characters", the A-variant writes bytes.
- **`ExitProcess`** (Microsoft Learn, fetched 2026-10-18):
  `VOID ExitProcess(UINT uExitCode)`; ends the calling process; the doc's
  thread-termination steps are all unimplemented here (no threads, no DLLs).
- **`raw-dylib` linkage** (Rust reference; the pinned 1.99.1 toolchain):
  `#[link(name = "kernel32", kind = "raw-dylib")]` makes the compiler and
  `rust-lld` synthesize the import descriptor, ILT and IAT directly — no
  `.lib` file, no `LIB` tool, no Windows SDK. Verified by the stage probe
  below; no unstable feature gate is needed.
- **Toolchain probe** (this checkout, 2026-10-18, temp workspace): the probe
  binary (same flags as stage 1) linked successfully at 2560 bytes with one
  import descriptor (`kernel32.dll`, `ILT 0x2088`, `IAT 0x20A8`,
  `TimeDateStamp 0`, `ForwarderChain 0`) and three name entries
  (`ExitProcess`, `GetStdHandle`, `WriteConsoleA`, all hint 0), alongside the
  `global_asm!` relocation slot (sections `.text`, `.rdata`, `.data`,
  `.pdata`, `.reloc`; one DIR64 entry). **The IAT and the names live in
  `.rdata`, which is read-only** — the import fixups therefore cannot be
  applied through the mapped section protection (Windows' loader likewise
  applies fixups before final section protections take effect); this loader
  patches the IAT through the HHDM like relocations.

## 3. Fixture pipeline changes

- `userland/pe/peconsole` (new binary in the existing separate workspace):
  `#![no_std] #![no_main]`, `unsafe extern "system"` declarations under
  `#[link(name = "kernel32", kind = "raw-dylib")]`, built with the existing
  `rust-lld` flags (`/nodefaultlib /entry:_start /subsystem:console`).
  `extern "system"` on `x86_64-pc-windows-msvc` is the Win64 calling
  convention, which matches the kernel gate's register layout
  (`rcx/rdx/r8/r9`) — that was the point of the stage-1 ABI choice.
- The fixture keeps the stage-1 `global_asm!` `marker_slot` relocation probe
  (a `.reloc` section is required for the nonzero-delta policy anyway), and
  re-verifies the relocated pointer at runtime.
- No import library, no DLL fixture, no MSVC/MinGW/SDK: `rustc` + `rust-lld`
  produce the import table from the `raw-dylib` declarations.

### 3.1 The fixture (`peconsole`)

Order of operations in `_start` (exit codes are part of the contract):

1. `handle = GetStdHandle(0xFFFF_FFF5)` (`STD_OUTPUT_HANDLE`); if
   `handle != 1` → `ExitProcess(1)` **without output** (the console is not
   trusted yet). The kernel handler logs the call either way.
2. `ok = WriteConsoleA(handle, M1, 44, &written, NULL)`; if `ok == 0` or
   `written != 44` → `ExitProcess(2)`.
   `M1` = `win32: hello from the Win32 console surface\n` (44 bytes).
3. Relocation check (the stage-1 property): if
   `read_volatile(marker_slot) != addr_of!(reloc_marker)` then
   `WriteConsoleA(handle, M3, 32, ...)` and `ExitProcess(3)` — this exit code
   is how a relocation regression surfaces.
   `M3` = `win32: base relocation mismatch\n` (32 bytes).
4. `ok = WriteConsoleA(handle, M2, 32, &written, NULL)`; if `ok == 0` or
   `written != 32` → `ExitProcess(5)`.
   `M2` = `win32: console surface verified\n` (32 bytes).
5. `ExitProcess(0)`.

`lpReserved` is `core::ptr::null_mut()` in both calls. The out-pointer
`written` is a stack `u32`. The panic handler calls `ExitProcess(0xBEEF)`.

Total on success: 1 × `GetStdHandle`, 2 × `WriteConsoleA` (76 bytes), 1 ×
`ExitProcess`, exit code 0.

### 3.2 Negative fixtures (deterministic byte patches)

`scripts/build_user_pe.sh` produces both from `peconsole.exe` by a single
find-and-replace each, and **fails** if the pattern is not unique:

- `pebadimp.exe`: `kernel32.dll` → `user32.dll\0\0` (same 12 bytes) — the
  loader must reject with `unknown import dll`.
- `pebadfn.exe`: `WriteConsoleA` → `WriteConsoleB` (same 13 bytes) — the
  loader must reject with `unknown import function`.

Both remain structurally valid PE images (they must pass
`scripts/check_user_pe.py` with their patched expected import signature), so
the rejection is loader policy, not parser failure.

## 4. `os_core::pe` — import parsing (extension of stage 1)

New public surface:

```rust
pub const MAX_IMPORT_DLLS: usize = 2;
pub const MAX_IMPORTS: usize = 16;

pub struct ImportSlot {
    pub dll: [u8; 16], pub dll_len: u8,      // exact bytes, no NUL
    pub name: [u8; 32], pub name_len: u8,    // exact bytes, no NUL
    pub iat_rva: u32,                        // where the fixup is written
}
// PeImage additionally stores slots + count in-line (no allocation).
pub fn imports(&self) -> &[ImportSlot];
```

Parse rules (all host-tested):

- `dir[1]` (`rva`, `size`): `rva == 0 && size == 0` → zero imports (stage-1
  fixtures). `rva == 0 && size != 0`, or `size < 20`, or `size % 20 != 0`
  → `bad import table`.
- Walk 20-byte descriptors inside `rva..rva+size`; the array **must** end
  with an all-zero descriptor within `size` → else `bad import table`.
  More than `MAX_IMPORT_DLLS` descriptors → `too many import dlls`.
- Descriptor `Name` and `FirstThunk` must be nonzero; every descriptor,
  ILT/IAT entry and name RVA must lie inside the image (a section's
  `max(VirtualSize, SizeOfRawData)` window) → else `bad import table`.
- DLL name: NUL-terminated, `0 < len <= 16` → else
  `import name too long` (covers too-long, empty, unterminated).
- Lookup list: `OriginalFirstThunk != 0` → ILT; else `FirstThunk` (the
  fallback documented by the spec). Thunks are read until a zero entry; a
  thunk with bit 63 set → `import by ordinal is not supported`; more than
  `MAX_IMPORTS` slots → `too many imports`.
- Name entry: 16-bit hint skipped, NUL-terminated name, `0 < len <= 32` →
  else `import name too long`. `iat_rva = FirstThunk + 8 * index`.
- `TimeDateStamp` and `ForwarderChain` are read and ignored (no bind
  semantics, no forwarding support).
- Stage-1 directories that must still be empty: delay imports (13), TLS (9),
  CLR (14). Exports (0) and the rest stay ignored; **the import directory is
  no longer rejected** (supersedes ADR-0019's reject-by-directory rule for
  index 1).

New `PeError` rows (labels): `bad import table`, `import by ordinal is not
supported`, `import name too long`, `too many import dlls`, `too many
imports`. Every row gets at least one corruption vector in the host tests,
plus positive tests: single DLL two names, ILT-fallback (ILT = 0), duplicate
names in different DLLs, empty directory, and the exact `kernel32.dll` layout
the probe produces.

## 5. Kernel loader changes (`kernel/src/pe_loader.rs`)

New public surface (stage-1 call sites updated):

```rust
pub struct Loaded { pub image: Inspection, pub stub_page: u64, pub applied_relocations: u32 }
pub fn map(space: &mut AddressSpace, module: &limine::File, image: &Inspection)
    -> Result<Loaded, Reject>;   // stage 1 returned u32
```

- `stub_page` is `0` when the image has no imports.
- `Reject` gains `UnknownImportDll`, `UnknownImportFunction`, `ImportTarget`
  (labels `unknown import dll`, `unknown import function`, `import target
  invalid`).
- `inspect` policy additions (still allocation-free):
  - every import slot's DLL name must equal `kernel32.dll` exactly (byte
    comparison, case-sensitive — the allowlist is ours) → else
    `UnknownImportDll`;
  - every function name must be in the fixed table
    `GetStdHandle → service 3`, `WriteConsoleA → service 4`,
    `ExitProcess → service 5` → else `UnknownImportFunction`;
  - every non-empty IAT slot must lie (8 bytes, page-rounded) inside a
    section's mapped range whose characteristics include `MEM_READ` and
    inside the code window → else `ImportTarget`.
- `map` additions, after section mapping and DIR64 relocations:
  1. if `imports()` is non-empty: allocate one frame, fill the **stub page**
     (`WIN32_STUB_VA = 0x51_0000`, one page, `USER_CODE`), zeroing unused
     bytes to `0xCC` (int3) so a stray call faults in isolation;
  2. stub `i` (stride 16, at most `MAX_IMPORTS`): `B8 <service u32>` (mov
     eax, imm32), `CD 80` (int 0x80), `C3` (ret), then 8 × `90` (nop). The
     service table is the same fixed table as above; stubs are deduplicated
     by (dll, function) first occurrence order;
  3. for each import slot: resolve the stub VA, then write the 8-byte value
     through the HHDM at the **physical** address behind
     `CODE_BASE + iat_rva` (`space.translate`), followed by `vm::flush`.
     This is required because `rust-lld` puts the IAT in read-only `.rdata`
     (R-014); the section is never made writable and the mapping flags are
     unchanged.
- Rollback on any failure frees the stub page and every section page as in
  stage 1; `Loaded.stub_page` is what `user.rs` unmaps at teardown.

## 6. Syscall services 3–5 (`kernel/src/syscall.rs`)

The stubs keep arguments in `rcx/rdx/r8/r9`, so the existing gate dispatches
them with no marshalling. New counters: `WIN32_GET_STD_HANDLE_CALLS`,
`WIN32_WRITE_CONSOLE_CALLS`, `WIN32_EXIT_PROCESS_CALLS`.

- `3 GetStdHandle(rcx = nStdHandle, low 32 bits)`: `0xFFFF_FFF5` →
  `rax = 1` and log `win32: GetStdHandle(-11) -> 1`; any other value →
  `rax = u64::MAX` (`INVALID_HANDLE_VALUE`) and no log (the doc says no
  validation happens there, so this never kills the thread).
- `4 WriteConsoleA(rcx = handle, rdx = buffer, r8 = length, r9 = written)`:
  - `handle != 1`, `length == 0`, or `length > MAX_WRITE` → kill
    (`invalid WriteConsoleA arguments`);
  - buffer span validation is the exact syscall `write` policy (user half,
    ≥ `0x1000`, every page mapped user-accessible) → kill with the write
    messages otherwise;
  - `written` must be a mapped user page with 4 bytes available → kill
    (`WriteConsoleA out-pointer is not mapped`) otherwise;
  - on success: console write of the exact bytes, `*written = length as u32`
    stored via the validated pointer, `WIN32_WRITE_CONSOLE_CALLS += 1`,
    `WRITE_CALLS += 1`, `BYTES_WRITTEN += length`, log
    `win32: WriteConsoleA(1, {length} bytes) -> 1`, `rax = 1`.
  - Validation order matters: nothing is written and no counter moves until
    every check passed.
- `5 ExitProcess(rcx = exit code, low 32 bits)`: store
  `LAST_EXIT_CODE`, `WIN32_EXIT_PROCESS_CALLS += 1`, log
  `win32: ExitProcess({code})`, then `sched::exit()` — the same kill path as
  `2 exit`, deliberately not a second scheduler mechanism.

An unknown service or a rejected call still kills the thread and increments
`REJECTED_SYSCALLS`, exactly as in stage 1.

## 7. `user.rs` demo phase and serial contract

After the stage-1 PE phase (which keeps its pinned lines):

1. Module presence: `/boot/peconsole.exe`, `/boot/pebadimp.exe`,
   `/boot/pebadfn.exe`. Any missing → print
   `user: Win32 self-test skipped (`{path}` module missing)`,
   `win32 selftest: skipped (no Win32 fixtures present)`, and return a
   report with `win32 = false` (marker without `,win32`).
2. `pebadimp.exe` probe: `inspect` must fail with `UnknownImportDll`; print
   `user: /boot/pebadimp.exe rejected (unknown import dll)`. An accepted
   image is a boot-fatal `Err`.
3. `pebadfn.exe` probe: same with `UnknownImportFunction` and
   `user: /boot/pebadfn.exe rejected (unknown import function)`.
4. `peconsole.exe`: `inspect` + `map` + stub page, `pe: peconsole.exe loaded
   at 0x400000 (entry 0x401000, 5 sections, 1 relocation applied)`, activate,
   spawn `user-win32`, run at CPL3. After `wait_all_finished`, return to the
   kernel space, unmap every section page, the stub page and the stack page,
   destroy the space.
5. Assertions (per-fixture counters, so cross-phase bleed is impossible):
   exit code 0; `WIN32_GET_STD_HANDLE_CALLS` +1; `WIN32_WRITE_CONSOLE_CALLS`
   +2; `WIN32_EXIT_PROCESS_CALLS` +1; `WRITE_CALLS` +2; `BYTES_WRITTEN` +76;
   no new `USER_FAULTS`; `REJECTED_SYSCALLS` still 0 before and after.
6. Verdict line when `report.win32`:
   `win32 selftest: passed (kernel32.dll imports resolved, WriteConsoleA wrote
   to the console fd, ExitProcess ended the thread)`; otherwise the skip
   line from step 1.
7. `main.rs`: marker inserts `,win32` after `,pe` only when `report.win32`:
   `checks=...,sched,user,elf,pe,win32,vfs[,blk][,persist][,net]`. The
   persistence suffix (`vfs,blk,persist`) and `,net` tests are unaffected;
   `DemoReport` gains `win32: bool`.

Existing lines that must remain byte-identical: everything from stage 1
(including `elf: 3 modules present`, the stage-1 `pe:` lines for
`pehello.exe`/`pebad.exe`, and both stage-1 verdicts); the module count line
changes only in its number: `pe: 5 modules present`.

## 8. Script changes

- `scripts/build_user_pe.sh`: builds `pehello` and `peconsole`; copies
  `pehello.exe`, `peconsole.exe`; generates `pebad.exe` (zeroed signature,
  stage 1), `pebadimp.exe` and `pebadfn.exe` (unique find-and-replace, §3.2);
  runs `check_user_pe.py` on all five with per-file expected import specs;
  logs sizes and SHA-256s.
- `scripts/check_user_pe.py`: gains `--imports <spec>` (`-` for none, else
  `dll:fn,fn,...`). Checks: descriptor count, DLL name, function-name set
  equality (order-insensitive), no ordinal entries, IAT slots inside a
  readable section inside the window, ILT present. Failures are `exit 1`
  with a named message.
- `boot/limine.conf`: adds `module_path: boot():/boot/peconsole.exe`,
  `…/pebadimp.exe`, `…/pebadfn.exe`.
- `scripts/make_iso.sh`: the PE artifact list becomes
  `pehello pebad peconsole pebadimp pebadfn`; missing fixtures fail loudly.
- `scripts/test_pe_qemu.sh`: `pe: 5 modules present`; adds the win32
  assertions: `win32: GetStdHandle(-11) -> 1`,
  `win32: WriteConsoleA(1, 44 bytes) -> 1`,
  `win32: WriteConsoleA(1, 32 bytes) -> 1`, `win32: ExitProcess(0)`,
  `win32: hello from the Win32 console surface`,
  `win32: console surface verified`,
  `user: /boot/pebadimp.exe rejected (unknown import dll)`,
  `user: /boot/pebadfn.exe rejected (unknown import function)`,
  `win32 selftest: passed (kernel32.dll imports resolved`, and the marker
  `sched,user,elf,pe,win32,vfs`.
- `scripts/test_elf_qemu.sh`: marker assertion becomes
  `sched,user,elf,pe,win32,vfs` (the same one-line edit pattern as stage 1).
- `scripts/test_all.sh`: unchanged step set (`userland-pe` and `pe` already
  cover this stage); expected summary stays PASS=22 FAIL=0 SKIP=4.

## 9. Evidence and limits (claims this stage may and may not make)

Status (2026-10-18): implemented and PASSED. The evidence below was observed
in `scripts/build_user_pe.sh`, `scripts/test_pe_qemu.sh`
(`artifacts/qemu-serial-pe.log`) and the full `scripts/test_all.sh` run
(PASS=22 FAIL=0 SKIP=4); the exact lines and artifact hashes are recorded in
`docs/RESEARCH.md` R-014 (Experiment) and `progress.md`.

May claim (after tests pass, with logs recorded): a genuine PE32+ image with a
real `rust-lld`-produced import directory is parsed host-side, its imports are
resolved by exact-name allowlist against our own native implementations,
the IAT is patched in place, and the fixture calls Win32-shaped functions
(`GetStdHandle`, `WriteConsoleA`, `ExitProcess`) at CPL3 through the import
table; unknown DLL and unknown function imports are rejected cleanly before
allocation. Evidence: `os_core` unit tests, `check_user_pe.py`,
`test_pe_qemu.sh` serial log, full `test_all.sh` run, VirtualBox re-run.

May **not** claim: Windows API compatibility, running any third-party Windows
binary, DLL loading/exports/forwarders/ordinals/delay imports/API sets, real
handle or process semantics, `GetLastError`, console input, Unicode/W-variant
APIs, CRT startup, SEH, TLS, or any part of a Win32 console beyond the three
named functions. Limits to restate: fixed window and stub page, one console
handle (`1`), one user thread, four-argument services, `MAX_WRITE` byte cap,
bootloader-provided modules only, QEMU/VirtualBox only.

Stage 3 (`docs/WIN32_PROGRAM.md`, ADR-0021) extends the same allowlist to
eleven `kernel32.dll` functions and adds the five-argument stub form plus the
loader-owned command line without changing any stage-2 contract: the three
functions above keep their service numbers, semantics and serial lines, and
every pinned stage-2 line stays byte-identical.

## 10. Documentation and evidence updates (on implementation)

- `docs/RESEARCH.md`: R-014 (sources, probe, claims; experiment section
  updated to PASSED/FAILED with the observed log after the run only).
- `docs/DECISIONS.md`: ADR-0020 (raw-dylib import fixtures; user-space stub
  page + `int 0x80` dispatch; HHDM IAT patch; exact-name allowlist;
  three-function surface) including alternatives rejected; ADR-0019 gains a
  superseded-in-part note for the import directory.
- `docs/PE_LOADER.md`: §5/§6 amended with pointers to this document (imports
  allowed; delay/TLS/CLR still rejected; `map` returns `Loaded`).
- `docs/ARCHITECTURE.md`: `syscall.rs`, `pe_loader.rs`, `user.rs`,
  `os_core::pe`, `userland/pe` rows updated; Windows-compatibility wording
  restated (mechanism, not compatibility).
- `docs/TESTING.md`: `pe` row gains the win32 assertions; unit-test count
  update; marker documentation becomes `...,user,elf,pe,win32,vfs`.
- `docs/ROADMAP.md`: Phase 7 stage 2 complete; remaining Phase 7 items
  (broader API surface, imports from real DLLs) explicitly deferred.
- `docs/WINDOWS_COMPATIBILITY.md`: status note — import mechanism exists for
  an allowlisted surface; everything else unchanged.
- `progress.md`: milestone entry, exact commands, artifact table with sizes
  and SHA-256s (three new fixtures), serial excerpts, defects, next action.
- `LICENSES.md`: no new third-party code; the fixture uses the values and
  signatures documented by Microsoft but no Microsoft code, and no new
  dependencies were added.
