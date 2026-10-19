# Win32 console input — `ReadFile` line mode (Phase 8, stage 2)

Status: contract written before implementation (2026-10-19). Evidence is
appended only after the harness runs; the experiment section in
`docs/RESEARCH.md` R-016 and the Evidence section here start as PENDING.

## 1. Goal and non-goals

Goal: a genuine PE32+ fixture (`peinput.exe`, built and linked like the
stage-1/3 fixtures) imports `ReadFile` from `kernel32.dll` and receives one
typed line from the serial console through our own `int 0x80` service. The
service blocks until the line is complete, echoes printable input, reports the
byte count through the documented out-parameter, and returns the documented
failure values for a bad handle and an unsupported overlapped request. The
surface also serves `GetStdHandle(STD_INPUT_HANDLE)` with a distinct input
handle.

Evidence: a new paced TCP-serial harness (`scripts/test_input_qemu.{sh,py}`)
types one known line while the fixture is blocked, then asserts the echo, the
fixture playback, the kernel's count line and the self-test line; plus the
full `scripts/test_all.sh` regression run.

Non-goals: no `ReadConsoleA/W`, `GetConsoleMode`/`SetConsoleMode`,
raw/disable-line-input mode, Ctrl+C (`ERROR_OPERATION_ABORTED`) or Ctrl+Z/EOF
handling, `INPUT_RECORD`/low-level console input, Unicode/W-variant APIs,
files/pipes/redirected `stdin`, asynchronous or overlapped I/O, a second user
thread, shell integration, or a new marker token. No claim of Windows
compatibility follows from this stage.

## 2. Primary sources (R-016)

R-016 records the exact fetched statements: `ReadFile` success/failure shape,
`lpNumberOfBytesRead` zeroing before any work, synchronous blocking with NULL
overlapped, and the default line-input mode completing at a carriage return;
the high-level console I/O page's blocking/echo/count and "unread characters
are buffered" statements; `ReadConsole`'s cooked-mode description; and the
console input-buffer model. `GetStdHandle`'s `STD_INPUT_HANDLE = (DWORD)-10`
and the WinError values (6, 87) are reused from R-014/R-015.

## 3. Service contract

### 3.1 `GetStdHandle`

- `-11` (`STD_OUTPUT_HANDLE`, `0xFFFF_FFF5`) → `1`; serial line unchanged:
  `win32: GetStdHandle(-11) -> 1`.
- `-10` (`STD_INPUT_HANDLE`, `0xFFFF_FFF6`) → `2`; new serial line:
  `win32: GetStdHandle(-10) -> 2`.
- Anything else → `INVALID_HANDLE_VALUE` (`u64::MAX`), no line (unchanged).

### 3.2 `ReadFile` — service 14, five-argument stub

Arguments: `rcx` handle, `rdx` buffer, `r8` length, `r9` out-count pointer,
`r10` overlapped pointer. The allowlist maps `ReadFile` to
`syscall::SYS_READFILE` with the existing `Args5` stub shape; the stub page
layout and entry stub are unchanged.

Order of operations (normative):

1. `lpNumberOfBytesRead` is validated as a mapped user page with 4 writable
   bytes. An unmapped pointer is a memory-safety violation and kills the
   thread (same policy as the write services). On success the kernel stores
   `0` immediately, matching the documented "sets this value to zero before
   doing any work or error checking".
2. `handle != 2` → `FALSE` (`0`), last-error `ERROR_INVALID_HANDLE` (6),
   line `win32: ReadFile({handle}) -> 0 (invalid handle)`.
3. `overlapped != 0` → `FALSE`, last-error `ERROR_INVALID_PARAMETER` (87),
   line `win32: ReadFile(2, {length} bytes, overlapped) -> 0 (invalid
   parameter)`.
4. `length < 2 || length > 4096` → `FALSE`, last-error 87, line
   `win32: ReadFile(2, {length} bytes) -> 0 (invalid parameter)`.
   Why `< 2`: a line-mode read is defined to deliver CRLF, so a buffer that
   cannot hold both cannot receive a well-formed line.
5. `buffer[0..length]` is validated as a mapped, user-accessible, bounded
   span **before** waiting; an invalid span kills the thread.
6. Blocking line read (section 3.3), then store the count and return `TRUE`
   (`1`) with `win32: ReadFile(2, {length} bytes) -> 1 ({count} bytes)`.

The last-error value is untouched on success (consistent with stage 3's
`WriteFile`).

### 3.3 Line mode (the only mode implemented)

- The serial RX ring is the console input buffer; bytes are consumed one at a
  time. No lock is held while waiting.
- Printable ASCII `0x20..=0x7E` is appended and echoed. Backspace (`0x08`)
  and DEL (`0x7F`) remove the last byte and echo `\x08 \x08`. Other control
  bytes are ignored.
- CR (`0x0D`) or LF (`0x0A`) completes the line: `\r\n` is appended, `\r\n`
  is echoed, and the read returns. The buffer therefore ends in CRLF and the
  count includes both bytes.
- The read guarantees room for the terminator: at most `length - 2` content
  bytes are accumulated. If the buffer fills before a terminator, the
  accumulated bytes are returned (no CRLF guarantee) and unread bytes stay in
  the ring for the next read, matching the documented "unread characters are
  buffered until the next read operation".
- CRLF-inclusive line and count is **our normative contract**. The fetched
  documentation pins the return point (a carriage return) and "the total
  number of characters actually read" but not the terminator bytes for
  `ReadFile`; tests define this choice and no vendor-verified claim is made.
- The wait re-enables interrupts and executes `hlt`; the serial RX IRQ and the
  100 Hz PIT both wake the thread, so a blocked read uses no CPU and timer
  preemption keeps the other threads running. The `int 0x80` gate itself
  enters with interrupts disabled, so the service explicitly enables them
  before sleeping. A boot whose ISO contains `peinput.exe` but has no harness
  fails the demo phase after the phase timeout instead of hanging forever.

## 4. Kernel changes

- `kernel/src/syscall.rs`: `STD_INPUT_HANDLE`, `INPUT_HANDLE = 2`,
  `SYS_READFILE = 14`, `MAX_READ = 4096`, `WIN32_READFILE_CALLS` counter,
  `read_file()` implementation, dispatch arm, and the `-10` branch in
  `get_std_handle` (with the new serial line).
- `kernel/src/pe_loader.rs`: allowlist grows from eleven to twelve names
  (`ReadFile`, service 14, `Args5`); comments updated. No stub-page or loader
  change.
- `kernel/src/user.rs`: `PEINPUT_PATH = "/boot/peinput.exe"`, `thread_peinput`,
  and a stage-4 probe/assertions block after stage 3: skip lines when the
  module is absent (`user: Win32 input self-test skipped (...)`, `win32 input
  selftest: skipped (no input fixture present)`); otherwise a 30 s phase wait
  (`sched::wait_all_finished(3000)`), then exact counter deltas —
  `GetStdHandle +1`, `ReadFile +3`, `GetLastError +2`, `WriteFile +2`, exit
  code 0, no user faults — and `win32 input selftest: passed (ReadFile line
  input, echo, byte count, invalid handle/parameter errors verified through
  kernel32.dll imports)`.
- Marker: unchanged. Console input belongs to the existing `win32` check;
  the default ISO does not contain `peinput.exe`, so every pinned marker,
  module count and serial assertion from stages 1–3 stays byte-identical.

## 5. Fixture (`userland/pe/peinput`)

Crate mirrors `pebasics`: `#![no_std] #![no_main]`, raw-dylib `kernel32.dll`
imports (`GetStdHandle`, `ReadFile`, `WriteFile`, `GetLastError`,
`ExitThread`), the same `reloc_marker`/`marker_slot` relocation probe and
`/entry:_start` link. A 64-byte static buffer is the read destination.

Sequence (`_start`, distinct exit codes):

1. `GetStdHandle(0xFFFF_FFF6)` must be `2` (else `ExitThread(1)`).
2. `ReadFile(1, buf, 16, &count, NULL)` where `count` starts as `0xDEADBEEF`:
   result `0`, `count == 0`, `GetLastError() == 6` (else 2/3).
3. `ReadFile(2, buf, 16, &count, non-NULL)` → `0` and last-error `87`
   (else 4).
4. Blocking `ReadFile(2, buf, 64, &count, NULL)`; on failure exit 5; the
   returned line must be at least 2 bytes and end in CRLF (else 6).
5. `WriteFile(1, buf, count, &written, NULL)` plays the line back (else 7);
   `WriteFile(1, L2, 30, ...)` prints `win32: input surface verified\n`
   (else 8); `ExitThread(0)`.

Call deltas for the phase: `GetStdHandle +1`, `ReadFile +3`,
`GetLastError +2`, `WriteFile +2`. The known harness line `hello, win32` +
CR returns 14 bytes; the kernel's summary line and the fixture playback are
the byte-level evidence. The fixture itself checks only structure (length
and CRLF), not the harness's chosen content. Because the serial output path
expands every LF to CRLF (`kernel/src/serial.rs`), the transcript displays
the line as `hello, win32\r\r\n` in both places while the buffer holds
`hello, win32\r\n` (14 bytes); the doubled CR is a display-layer artifact,
and the count line is the normative evidence for the buffer contents.

## 6. Script changes

- `scripts/build_user_pe.sh`: build/copy `peinput.exe`; check it with
  `check_user_pe.py --imports 'kernel32.dll:ExitThread,GetLastError,GetStdHandle,ReadFile,WriteFile'`;
  add it to the hash log. No mutant is generated for this stage (the error
  paths are tested by the fixture at run time and the allowlist already has
  negative mutants).
- `scripts/make_iso.sh`: new `--input-fixture` flag appends `peinput.exe` to
  the PE module list and writes `artifacts/project-os-input.iso` (with
  `debug_exit` left in place). Default and `--interactive` outputs and their
  module lists are unchanged; the unknown-argument message lists the new flag.
- `scripts/test_input_qemu.sh`: requires the kernel artifact, python and
  QEMU, builds the input ISO with `--no-build --input-fixture`, runs the
  python harness and logs PASS plus the session log path.
- `scripts/test_input_qemu.py`: starts QEMU (q35, 512 MiB, `isa-debug-exit`,
  TCP serial `wait=on`), waits for
  `win32: ReadFile(2, 64 bytes) waiting for input`, sends `hello, win32`
  followed by LF at the paced byte rate, then requires within the boot
  timeout: the echo line, the fixture playback (`hello, win32` + the
  serial layer's expanded CRLF),
  `win32: ReadFile(2, 64 bytes) -> 1 (14 bytes)`,
  `win32: input surface verified`,
  `win32 input selftest: passed (ReadFile line input`,
  `PROJECT_OS_BOOT_SUCCESS:...checks=...win32,vfs...`, and no
  `PROJECT_OS_FATAL`/`PROJECT_OS_PANIC`. The QEMU process must then exit with
  status 33 (isa-debug-exit). The transcript is written to
  `artifacts/qemu-input-session.log`.
- `scripts/test_all.sh`: one new step `input` after `shell`; expected summary
  becomes `PASS=23 FAIL=0 SKIP=4` while every existing step keeps its result.
- `scripts/test_pe_qemu.sh` is unchanged: it boots the default ISO, which
  still contains seven PE modules.

## 7. Evidence and limits (claims this stage may and may not make)

Status: PASSED (2026-10-19). `scripts/test_input_qemu.sh` passed on the input
ISO (build id `0.1.0-58f98e48`; kernel
`56bd754ea1f7f302417940cbe3da280fae1669b800d335d34fcafd438a9e06af`,
1,745,512 bytes, rebuild-verified; input ISO
`e5b8165347b33fb45769477488783804812d96f649622b69953d7f21d0c0d834`,
7,786,496 bytes) and `scripts/test_all.sh` reported `PASS=23 FAIL=0 SKIP=4`. Observed serial
evidence in `artifacts/qemu-input-session.log`: `pe: peinput.exe loaded at
0x400000 (entry 0x401000, 5 sections, 1 relocation applied)`; `win32:
GetStdHandle(-10) -> 2`; `win32: ReadFile(1) -> 0 (invalid handle)` then
`win32: GetLastError() -> 6`; `win32: ReadFile(2, 16 bytes, overlapped) -> 0
(invalid parameter)` then `win32: GetLastError() -> 87`; `win32: ReadFile(2,
64 bytes) waiting for input`; the harness line echoed and played back;
`win32: ReadFile(2, 64 bytes) -> 1 (14 bytes)`; `win32: input surface
verified`; `win32 input selftest: passed (ReadFile line input, echo, byte
count, invalid handle/parameter errors verified through kernel32.dll
imports)`; `PROJECT_OS_BOOT_SUCCESS:...win32,vfs...`.

May claim (after the harness and full suite pass, with logs recorded): a
genuine PE32+ image reaches a native, allowlisted `ReadFile` through its own
import table; the console input handle is distinct from the output handle;
the synchronous read blocks and consumes a typed line from the serial
console, echoes it, delivers a CRLF-terminated buffer with the byte count, and
returns the documented `FALSE` + WinError values for a bad handle and an
overlapped request (with `*bytes` zeroed first). Deltas from Windows are
stated, not hidden: only line mode with echo, our CRLF contract, minimum
length 2, no Ctrl+C/EOF/raw/console-mode/Unicode/redirect/async support.

May **not** claim: general console input compatibility, `ReadConsole`,
console modes, keyboard events, multiple simultaneous readers, blocking
cancellation, or any behavior of a real Windows console beyond the fetched
statements above.

## 8. Verification plan

1. `scripts/format.sh`, `scripts/lint.sh`: clean.
2. `scripts/test_unit.sh`: 269/269 unchanged (no new `os_core` code).
3. `scripts/build_user_pe.sh`: eight artifacts incl. `peinput.exe`, each
   checker-verified; sizes and SHA-256 recorded.
4. `scripts/make_iso.sh --input-fixture`: ISO with eight PE modules;
   default ISO still seven.
5. `scripts/test_input_qemu.sh`: PASS lines above; record the session log.
6. `scripts/test_pe_qemu.sh`: unchanged PASS (regression for the allowlist
   and stub page).
7. `scripts/test_all.sh`: `PASS=23 FAIL=0 SKIP=4`; record exact hashes.

## 9. Documentation and evidence updates

`docs/RESEARCH.md` R-016 (experiment/conclusion after the run), ADR-0023,
`docs/ARCHITECTURE.md` (syscall service table, allowlist count, user phase),
`docs/TESTING.md` (input step, fixture, marker notes, counts),
`docs/ROADMAP.md` (Phase 8 stage 2 complete), `docs/WINDOWS_COMPATIBILITY.md`
(console input gap), `progress.md` (commands, logs, hashes, defects).
