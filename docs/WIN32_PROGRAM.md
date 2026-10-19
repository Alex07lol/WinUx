# Native Win32 program basics (Phase 7, stage 3) — design spec

Status (2026-10-19): approved for implementation (designed, not yet
implemented). This document is the contract for the stage-3 implementation;
nothing here is evidence until the tests named in §8 have actually run and
their output is recorded in `docs/RESEARCH.md` R-015, `progress.md` and
`artifacts/qemu-serial-pe.log`.

Stage 2 (`docs/WIN32_CONSOLE.md`) established the mechanism: real
`rust-lld`-produced import tables, an exact-name allowlist, a kernel-owned
stub page, `int 0x80` dispatch, and three `kernel32.dll` functions resolved
by our own implementations. Stage 3 widens that surface from three functions
to eleven and exercises it with a second console fixture that behaves like a
small command-line program: five-argument `WriteFile`, per-thread
`GetLastError`/`SetLastError`, `ExitThread`, synthetic IDs, a tick source and
a command line.

## 1. Goal and non-goals

Goal: a native PE32+ fixture (`pebasics.exe`) that imports ten
`kernel32.dll` functions (eight new plus `GetStdHandle`), calls all of them
through a genuine import table at CPL3, and verifies each result against
values documented by Microsoft before exiting through `ExitThread`. The
kernel supplies the behaviour with its own code; the loader learns one new
stub shape (five arguments) and one new image-adjacent datum (the command
line).

Non-goals (explicitly out of scope for this stage):

- Any claim of Windows compatibility. The surface stays an exact-name
  allowlist of eleven functions with documented, simplified semantics.
- Real NT process/thread objects: `GetCurrentProcessId` and
  `GetCurrentThreadId` return synthetic constants (`1` and `1`), documented
  as such. There is no NT PID/TID space, no handle table beyond the console
  output handle `1`, and no kernel object manager.
- Real command-line parsing: `GetCommandLineA` returns the NUL-terminated
  module path string the loader writes into the stub page tail; there is no
  `argv` construction, quoting, or program name extraction.
- Unified APIs beyond the eleven names: no `WriteFileEx`, no
  `GetTickCount64`, no `GetLastError`-style `SetLastErrorEx`, no W-variants.
- New import features: no ordinal imports, no forwarders, no delay imports,
  no API sets, no DLL exports of our own, no loading any DLL other than the
  built-in allowlist.
- Console input, console modes, cursor handling, Unicode conversion.
- More than one user program running at a time, or more than one thread per
  program.

The stage is complete when the fixture passes in QEMU BIOS boot via
`scripts/test_pe_qemu.sh` and `scripts/test_all.sh` reports no new failure.

## 2. Research and primary sources (R-015)

Consulted 2026-10-19 (all Microsoft Learn, the vendor's own reference):

| API | Source | Key facts used |
| --- | --- | --- |
| `WriteFile` | `learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-writefile` (ms.date 2025-02-28) | `BOOL`; five parameters, the fifth (`lpOverlapped`) is the first stack argument; `lpNumberOfBytesWritten` receives the count; failure is `0` plus `GetLastError`; writing to a console handle paints at the cursor position |
| `GetLastError` | `…/errhandlingapi/nf-errhandlingapi-getlasterror` (2024-02-02) | per-thread last-error value; functions may set it on failure; success does not have to clear it |
| `SetLastError` | `…/errhandlingapi/nf-errhandlingapi-setlasterror` (2024-02-02) | sets the per-thread value; thread-local storage; used for testing error paths |
| `ExitThread` | `…/processthreadsapi/nf-processthreadsapi-exitthread` (2018-12-05) | ends the calling thread with an exit code; the last thread terminates the process |
| `GetCurrentProcessId` | `…/processthreadsapi/nf-processthreadsapi-getcurrentprocessid` (2018-12-05) | returns a `DWORD` process identifier |
| `GetCurrentThreadId` | `…/processthreadsapi/nf-processthreadsapi-getcurrentthreadid` (2024-02-02) | returns a `DWORD` thread identifier |
| `GetTickCount` | `…/sysinfoapi/nf-sysinfoapi-gettickcount` (2018-12-05) | milliseconds since start; resolution is typically 10–16 ms; `DWORD`, wraps after 49.7 days; callers must handle overflow |
| `GetCommandLineA` | `…/processenv/nf-processenv-getcommandlinea` (2024-10-23) | pointer to the process command-line string; lifetime managed by the system, callers must not free or modify it |
| Error codes | `…/debug/system-error-codes--0-499-` (2025-07-14) | `ERROR_INVALID_HANDLE` = 6, `ERROR_INVALID_PARAMETER` = 87 |
| x64 calling convention | reused from R-014 (`…/cpp/build/x64-calling-convention`) | the fifth integer argument arrives on the stack at `[rsp+0x28]` at function entry (32-byte shadow space + 8-byte return address) |

Claim, confidence, experiment and conclusion live in `docs/RESEARCH.md`
R-015; the experiment section stays `PENDING` until the observed run.

Research summary: the five-argument shape is the only mechanical unknown.
The documented x64 ABI places the fifth integer argument at `[rsp+0x28]`,
so a stub that loads that slot into `r10` before `int 0x80` can forward it
under the existing syscall frame layout (`r10` is already captured by
`InterruptFrame`, kernel/src/arch/x86_64/idt.rs:22-23 — no entry-stub
change). Everything else is analogous to stage 2.

## 3. Fixture pipeline changes

### 3.1 The new fixture (`pebasics`)

`userland/pe/pebasics/` (member of the `userland/pe` workspace) is a
`no_std`/`no_main` Rust binary built with the pinned `rust-lld`
(`/nodefaultlib /entry:_start /subsystem:console`), same flags as stage 2.
It declares nine `kernel32.dll` `raw-dylib` imports:

`GetStdHandle`, `WriteFile`, `GetLastError`, `SetLastError`, `ExitThread`,
`GetCurrentProcessId`, `GetCurrentThreadId`, `GetTickCount`,
`GetCommandLineA`.

(`WriteConsoleA` and `ExitProcess` are covered by `peconsole.exe` and stay
out of this fixture; `ExitThread` replaces `ExitProcess` here.)

Program sequence; every failure exits with a distinct code so the serial
log names the broken step:

| Step | Action | Required result | Exit code on failure |
| --- | --- | --- | --- |
| 1 | `GetStdHandle(0xFFFF_FFF5)` | `1` | 1 |
| 2 | `SetLastError(42)`; `GetLastError()` | `42` | 2 |
| 3 | `WriteFile(1, L1, 28, &written, 0)` | `TRUE`, `written == 28` | 3 |
| 4 | `GetLastError()` after the success in step 3 | still `42` (success does not clear) | 4 |
| 5 | `WriteFile(0, L1, 28, &written, 0)` | `FALSE`, then `GetLastError() == 6` | 5 |
| 6 | `WriteFile(1, L1, 28, &written, 1)` (non-null overlapped) | `FALSE`, then `GetLastError() == 87` | 6 |
| 7 | `GetCurrentProcessId()` and `GetCurrentThreadId()` | both `1` | 7 |
| 8 | `t1 = GetTickCount()`, `t2 = GetTickCount()` | `(t2 - t1) as i32 >= 0` (wrap-safe, per the doc's overflow advice) | 8 |
| 9 | `GetCommandLineA()` | exact match `"/boot/pebasics.exe"`, NUL-terminated | 9 |
| 10 | `SetLastError(0)`; `GetLastError()` | `0` | 10 |
| 11 | `WriteFile(1, L2, 31, &written, 0)` | `TRUE` | 11 |
| 12 | `ExitThread(0)` | thread ends, exit code stored | — |

Data:

- `L1 = b"win32: hello from WriteFile\n"` — 28 bytes.
- `L2 = b"win32: program basics verified\n"` — 31 bytes.

The fixture never hardcodes a kernel VA; it only uses the import table. The
`written` out-variable is a plain local (mapped stack page).

### 3.2 Negative fixture (`pebadfn2`)

`pebadfn2.exe` is a deterministic single-patch mutant of `pebasics.exe`:
the pattern `GetTickCount` (12 bytes) is replaced exactly once by
`Sleep` + 7 NUL bytes (still 12 bytes). The image stays structurally valid;
the hint/name entry now terminates after `Sleep`, so the kernel allowlist
must reject the module with `unknown import function` before any page is
allocated. `build_user_pe.sh` asserts the pattern occurs exactly once.

`pehello`, `pebad`, `peconsole`, `pebadimp` and `pebadfn` are untouched;
their bytes and all their pinned serial lines remain as in stage 2.

## 4. `os_core::pe` — no changes

Import parsing already supports multiple DLLs/functions within the caps
(`MAX_IMPORT_DLLS = 2`, `MAX_IMPORTS = 16`, names as documented in
`docs/PE_LOADER.md`); `pebasics` has one DLL and nine functions. No
`os_core` change and no new host unit tests are required. The host checker
(`scripts/check_user_pe.py`) gains only a new expected-imports spec for the
two new files.

## 5. Kernel loader changes (`kernel/src/pe_loader.rs`)

1. `WIN32_SERVICES` grows from three to eleven entries — same
   `kernel32.dll` allowlist, exact-name match:

   | Function | Service | Stub shape |
   | --- | --- | --- |
   | `GetStdHandle` | 3 | 4-arg (unchanged from stage 2) |
   | `WriteConsoleA` | 4 | 4-arg (unchanged) |
   | `ExitProcess` | 5 | 4-arg (unchanged) |
   | `WriteFile` | 6 | **5-arg** |
   | `GetLastError` | 7 | 4-arg |
   | `SetLastError` | 8 | 4-arg |
   | `ExitThread` | 9 | 4-arg |
   | `GetCurrentProcessId` | 10 | 4-arg |
   | `GetCurrentThreadId` | 11 | 4-arg |
   | `GetTickCount` | 12 | 4-arg |
   | `GetCommandLineA` | 13 | 4-arg |

2. Stub forms, still 16 bytes per slot (`WIN32_STUB_VA + 16*i`,
   `B8 <service u32> / CD 80 / C3` followed by 8 × `90`). The new 5-arg
   shape is one instruction longer:

   ```
   4C 8B 54 24 28      mov r10, [rsp+0x28]   ; fifth integer argument
   B8 <service u32>    mov eax, <service>
   CD 80               int 0x80
   C3                  ret
   90 90 90            padding to the 16-byte stride
   ```

   4-arg stubs keep the stage-2 bytes exactly. A 0-arg or 1-arg function
   reuses the 4-arg shape: the kernel service ignores the unused registers,
   so no extra shapes are needed. The IAT is patched to the stub VA as
   before; the stub page's W^X permissions are unchanged.

3. Command line: `map` writes the NUL-terminated Limine module path (e.g.
   `/boot/pebasics.exe`) at

   `COMMAND_LINE_VA = WIN32_STUB_VA + 0xF00` (capacity 0x100 bytes:
   255 path bytes + terminator).

   The stub-page frame is `int3` (0xCC)-filled, not zeroed, so the terminating
   zero is written explicitly after the path bytes (a zero byte cannot be
   assumed — assuming it was the one bring-up defect this stage produced,
   found by the fixture's terminator scan).
   If the module path is longer than 255 bytes, `map` fails with the new
   `Reject::CommandLineTooLong` before entering ring 3 (the fixture path is
   far below the limit; the variant exists so the failure is a clean
   rejection, not truncation). The write happens through the frame's HHDM
   window before the page is mapped into the user space, exactly like the
   stubs. There is no separate page and no new teardown: the string lives
   in the stub page that `cleanup_slot` already unmaps.

4. `Loaded` is unchanged; teardown (`user.rs::cleanup_slot`) is unchanged.

## 6. Syscall services 6–13 (`kernel/src/syscall.rs`, `sched.rs`)

State: each scheduler thread slot gains `last_error: u32`, initialised to
`0` in `sched.rs` and reached through `current_last_error()` /
`set_current_last_error()` helpers. It is per-thread storage, matching the
documented "last-error code is maintained on a per-thread basis".

New counters (all `AtomicU64`, same pattern as stage 2):
`WIN32_WRITEFILE_CALLS`, `WIN32_GET_LAST_ERROR_CALLS`,
`WIN32_SET_LAST_ERROR_CALLS`, `WIN32_EXIT_THREAD_CALLS`,
`WIN32_GET_CURRENT_PROCESS_ID_CALLS`, `WIN32_GET_CURRENT_THREAD_ID_CALLS`,
`WIN32_GET_TICK_COUNT_CALLS`, `WIN32_GET_COMMAND_LINE_CALLS`.

Errors: `ERROR_INVALID_HANDLE = 6`, `ERROR_INVALID_PARAMETER = 87` as named
constants with the WinError.h values from §2.

- `6 WriteFile(rcx = handle, rdx = buffer, r8 = length, r9 = &written,
  r10 = overlapped)`:
  - every call increments `WIN32_WRITEFILE_CALLS` exactly once;
  - `handle != 1` → `rax = 0`, `last_error = 6`, log
    `win32: WriteFile({handle}) -> 0 (invalid handle)`;
  - `r10 != 0` → `rax = 0`, `last_error = 87`, log
    `win32: WriteFile(1, {length} bytes, overlapped) -> 0 (invalid parameter)`;
  - `length == 0` or `length > MAX_WRITE` → `rax = 0`,
    `last_error = 87`, log
    `win32: WriteFile(1, {length} bytes) -> 0 (invalid parameter)`;
  - buffer span (`rdx..rdx+length`) not mapped user-accessible → kill the
    thread through the shared `validate_user_span` path (the existing
    `write buffer ...` messages); the 4-byte `written` span → kill with
    `WriteFile out-pointer is not mapped`. Both are the memory-safety gate;
    see §9 for the documented divergence from Windows' access violation.
  - success: console write of the exact bytes, `*written = length`,
    `rax = 1`, `WRITE_CALLS += 1`, `BYTES_WRITTEN += length`, log
    `win32: WriteFile(1, {length} bytes) -> 1`; `last_error` is untouched
    (success does not clear).
  - No `last_error` change and no output until all checks are considered:
    the error paths above are terminal for the call and return immediately.
- `7 GetLastError()`: `rax = current_last_error()`,
  `WIN32_GET_LAST_ERROR_CALLS += 1`, log
  `win32: GetLastError() -> {value}`.
- `8 SetLastError(rcx = code, low 32 bits)`:
  `set_current_last_error(code)`, `WIN32_SET_LAST_ERROR_CALLS += 1`, log
  `win32: SetLastError({code})`.
- `9 ExitThread(rcx = code, low 32 bits)`: store `LAST_EXIT_CODE`,
  `WIN32_EXIT_THREAD_CALLS += 1`, log `win32: ExitThread({code})`, then
  `sched::exit()` — the same kill path as `2 exit` / `5 ExitProcess`;
  `LAST_EXIT_CODE` and the completion wait are shared, so the demo's exit
  assertions work unchanged.
- `10 GetCurrentProcessId()`: `rax = 1`,
  `WIN32_GET_CURRENT_PROCESS_ID_CALLS += 1`, log
  `win32: GetCurrentProcessId() -> 1`. Synthetic constant, documented.
- `11 GetCurrentThreadId()`: `rax = 1`,
  `WIN32_GET_CURRENT_THREAD_ID_CALLS += 1`, log
  `win32: GetCurrentThreadId() -> 1`. Synthetic constant, documented.
- `12 GetTickCount()`: `rax = (pit ticks × 10) as u32` (the PIT runs at
  100 Hz, so this is milliseconds with 10 ms resolution, inside the
  documented 10–16 ms range; the `u32` wraps after 49.7 days exactly like
  the documented DWORD). `WIN32_GET_TICK_COUNT_CALLS += 1`, log
  `win32: GetTickCount() -> {ms}`.
- `13 GetCommandLineA()`: validate `[COMMAND_LINE_VA, COMMAND_LINE_VA+0x100)`
  is mapped user-accessible, scan for the terminator (≤ 255 bytes, else the
  state invariant is broken → kill), copy the string into a kernel stack
  buffer, log `win32: GetCommandLineA() -> "{path}"`, `rax = COMMAND_LINE_VA`,
  `WIN32_GET_COMMAND_LINE_CALLS += 1`. The returned pointer is the
  loader-owned string; the fixture only reads it, matching "the lifetime of
  the returned value is managed by the system".

Unknown services and rejected calls keep killing the thread and incrementing
`REJECTED_SYSCALLS` (stage-1 behaviour).

## 7. `user.rs` demo phase and serial contract

After the stage-2 assertions (which stay byte-identical):

1. Module presence: `/boot/pebasics.exe` and `/boot/pebadfn2.exe`. Any
   missing → print
   `user: Win32 program self-test skipped (`{path}` module missing)`,
   `win32 program selftest: skipped (no program fixtures present)`, return
   the report with `win32 = true` (stage 2 passed) and
   `win32_program = false`.
2. `pebadfn2.exe` probe: `inspect` must fail with `UnknownImportFunction`;
   print `user: /boot/pebadfn2.exe rejected (unknown import function)`. An
   accepted image is a boot-fatal `Err`.
3. `pebasics.exe`: `inspect` + `map` (stub page + command-line string),
   print
   `pe: pebasics.exe loaded at 0x400000 (entry 0x401000, {n} sections, {r} relocation applied)`
   (section/relocation counts pinned once observed, like stage 2; `{r}`
   pluralised), activate, spawn `user-win32-prog`, run at CPL3. After
   `wait_all_finished`: return to the kernel space, unmap every section
   page, the stub page and the stack page, destroy the space.
4. Assertions (per-phase counter deltas):
   - exit code 0;
   - `WIN32_GET_STD_HANDLE_CALLS` +1, `WIN32_WRITE_CONSOLE_CALLS` +0,
     `WIN32_EXIT_PROCESS_CALLS` +0;
   - `WIN32_WRITEFILE_CALLS` +4, `WIN32_GET_LAST_ERROR_CALLS` +5,
     `WIN32_SET_LAST_ERROR_CALLS` +2, `WIN32_EXIT_THREAD_CALLS` +1,
     `WIN32_GET_CURRENT_PROCESS_ID_CALLS` +1,
     `WIN32_GET_CURRENT_THREAD_ID_CALLS` +1,
     `WIN32_GET_TICK_COUNT_CALLS` +2, `WIN32_GET_COMMAND_LINE_CALLS` +1;
   - `WRITE_CALLS` +2, `BYTES_WRITTEN` +59;
   - no new `USER_FAULTS`; `REJECTED_SYSCALLS` still 0;
   - `DemoReport` gains `win32_program: bool`.
5. New serial lines (fixture phase, in expected order after the stage-2
   block):

   ```
   user: /boot/pebadfn2.exe rejected (unknown import function)
   pe: pebasics.exe loaded at 0x400000 (entry 0x401000, {n} sections, {r} relocation applied)
   win32: GetStdHandle(-11) -> 1
   win32: SetLastError(42)
   win32: GetLastError() -> 42
   win32: WriteFile(1, 28 bytes) -> 1
   win32: hello from WriteFile
   win32: GetLastError() -> 42
   win32: WriteFile(0) -> 0 (invalid handle)
   win32: GetLastError() -> 6
   win32: WriteFile(1, 28 bytes, overlapped) -> 0 (invalid parameter)
   win32: GetLastError() -> 87
   win32: GetCurrentProcessId() -> 1
   win32: GetCurrentThreadId() -> 1
   win32: GetTickCount() -> {ms}
   win32: GetTickCount() -> {ms}
   win32: GetCommandLineA() -> "/boot/pebasics.exe"
   win32: SetLastError(0)
   win32: GetLastError() -> 0
   win32: WriteFile(1, 31 bytes) -> 1
   win32: program basics verified
   win32: ExitThread(0)
   ```

   The two `GetTickCount` values are run-dependent; the test asserts the
   line prefix.
6. Verdict: when the phase ran, `main.rs` prints after the stage-2 verdict
   `win32 program selftest: passed (WriteFile, GetLastError/SetLastError,
   ExitThread, GetCurrentProcessId/GetCurrentThreadId, GetTickCount,
   GetCommandLineA verified through kernel32.dll imports)`.
7. Module counts: the fixture count line changes only in its number:
   `pe: 7 modules present`. The marker keeps the exact stage-2 shape
   `checks=...,sched,user,elf,pe,win32,vfs` — no new suffix; stage 3 is
   covered by `,win32` per the approved design.

Existing lines that must remain byte-identical: everything from stages 1
and 2, including `elf: 3 modules present`, the stage-1 `pe:` lines,
`pe: peconsole.exe loaded at 0x400000 (entry 0x401000, 5 sections, 1
relocation applied)`, all seven stage-2 `win32:` lines, the stage-2
rejection lines and both existing verdict lines.

## 8. Script changes

- `scripts/build_user_pe.sh`: builds `pehello`, `peconsole`, `pebasics`
  (workspace member added in `userland/pe/Cargo.toml`); copies the three
  `.exe`s; generates `pebad.exe` (stage 1), `pebadimp.exe` / `pebadfn.exe`
  (stage 2) and `pebadfn2.exe` (`GetTickCount` → `Sleep` + 7 NULs, unique
  once, §3.2); runs `check_user_pe.py` on all eight with per-file import
  specs (`pebasics` /
  `pebadfn2` expect the nine-name set from §3.1 with `GetTickCount` /
  `Sleep` respectively); logs sizes and SHA-256s.
- `scripts/check_user_pe.py`: no code change; two new spec invocations.
- `boot/limine.conf`: adds `module_path: boot():/boot/pebasics.exe` and
  `…/pebadfn2.exe`.
- `scripts/make_iso.sh`: the PE artifact list becomes
  `pehello pebad peconsole pebadimp pebadfn pebasics pebadfn2`; missing
  fixtures still fail loudly.
- `scripts/test_pe_qemu.sh`: `pe: 7 modules present`; adds the §7(5)
  assertions, the `win32 program selftest: passed (` verdict and keeps the
  marker assertion unchanged.
- `scripts/test_elf_qemu.sh`: unchanged (same marker).
- `scripts/test_all.sh`: expected step set and summary unchanged
  (PASS=22 FAIL=0 SKIP=4, verified after the run).

## 9. Evidence and limits (claims this stage may and may not make)

Status (2026-10-19): implemented and verified in QEMU (build
`0.1.0-ec04e74f`). `scripts/build_user_pe.sh` PASS (`pebasics.exe` 4096
bytes, SHA-256
`4a4aa956a3f5519875b43d8a31e91177e3392fa4316ef2ed4d98d70bdea8341b`, 9
imports; `pebadfn2.exe` 4096 bytes, SHA-256
`9d6082b9792859eba6d4b4ca1eae70c09c46314e79588757cbb74c37e6cd473d`);
`scripts/test_pe_qemu.sh` PASS with every §7 line above observed in
`artifacts/qemu-serial-pe.log` (including
`pe: pebasics.exe loaded at 0x400000 (entry 0x401000, 5 sections, 1
relocation applied)` and `win32 program selftest: passed (WriteFile,
GetLastError/SetLastError, ExitThread, GetCurrentProcessId/
GetCurrentThreadId, GetTickCount, GetCommandLineA verified through
kernel32.dll imports)`); full `scripts/test_all.sh` `PASS=22 FAIL=0 SKIP=4`.
Two bring-up defects were found by the fixture and fixed (recorded in
`progress.md` and R-015): the stub-page command-line terminator now written
explicitly, and `sched::wait_all_finished` bounded by PIT ticks only after
the TSC-frequency budget proved unreliable under QEMU. Deviations of the
final artifact from the planned contract: none.

May claim (after tests pass, with logs recorded): a real PE32+ fixture with
genuine `rust-lld` import tables calls eleven `kernel32.dll`-named functions
at CPL3 through the IAT; five-argument calls are forwarded through the
kernel's own `int 0x80` stub page; per-thread `last_error` round-trips
through `SetLastError`/`GetLastError`; documented `WriteFile` failure
values (`0` plus `ERROR_INVALID_HANDLE`/`ERROR_INVALID_PARAMETER`) are
returned for bad handle/overlapped/length; `GetCommandLineA` returns the
loader-owned module path; `GetTickCount` returns a monotone-increasing,
wrap-safe millisecond count readable against the PIT. Evidence:
`os_core` unit tests (unchanged, 252), `check_user_pe.py`, the serial log,
the full `test_all.sh` run.

May **not** claim: Windows API compatibility, running any third-party
Windows binary, real handle/process/thread semantics, NT PID/TID values,
command-line parsing (`argv`), console modes/cursor/input, Unicode,
`GetTickCount64`, `WriteFileEx`, ordinal imports, delay imports, API sets,
forwarders, DLL loading other than the allowlist, CRT startup, SEH, TLS, or
any part of Win32 beyond the eleven named functions. Documented
divergences, repeated here so they cannot be mistaken for fidelity:

- unmapped buffers kill the thread instead of raising an access violation;
- overlapped I/O is never accepted (`WriteFile` with non-null overlapped
  always returns `ERROR_INVALID_PARAMETER`; real Windows would attempt
  asynchronous behaviour for handles opened for it);
- zero-length `WriteFile` returns `ERROR_INVALID_PARAMETER` rather than a
  filesystem-dependent null write;
- `GetCurrentProcessId`/`GetCurrentThreadId` return `1`/`1` constants, not
  unique NT IDs;
- `GetTickCount` resolution is the 100 Hz PIT (10 ms), not the Windows
  timer resolution.

Limits to restate: fixed window and stub page, one console handle (`1`),
one user thread, four-argument services plus the one five-argument shape,
`MAX_WRITE` byte cap, bootloader-provided modules only, QEMU/VirtualBox
only.

## 10. Documentation and evidence updates (on implementation)

- `docs/RESEARCH.md`: R-015 (sources §2, claims, probe, experiment →
  PASSED/FAILED with observed output only).
- `docs/DECISIONS.md`: ADR-0021 (five-argument stub via `r10`, per-thread
  `last_error`, synthetic IDs, loader-owned command line, `WriteFile`
  error mapping, marker unchanged) including alternatives rejected.
- `docs/WIN32_CONSOLE.md`: forward pointer that stage 3 extends the
  surface without changing stage-2 contracts.
- `docs/PE_LOADER.md`: allowlist table grows to eleven entries; 5-arg stub
  form; command-line tail region; `CommandLineTooLong`.
- `docs/ARCHITECTURE.md`: `syscall.rs`, `pe_loader.rs`, `sched.rs`,
  `user.rs`, `userland/pe` rows updated; Windows-compatibility wording
  unchanged in substance ("mechanism, not compatibility").
- `docs/TESTING.md`: `pe` row gains the stage-3 assertions and fixture
  counts; marker documentation unchanged.
- `docs/ROADMAP.md`: Phase 7 stage 3 complete; Phase 7 acceptance status
  updated only after the user reviews the evidence.
- `docs/WINDOWS_COMPATIBILITY.md`: surface count and limits restated.
- `progress.md`: milestone entry, exact commands, artifact table with
  sizes and SHA-256s (two new fixtures), serial excerpts, defects, next
  action.
- `LICENSES.md`: no new third-party code or dependencies; the fixture uses
  documented Microsoft signatures and values only.
