# Win32 registry read surface — `advapi32.dll` over the config store (Phase 8, stage 3)

Status: contract written before implementation (2026-10-19). Evidence is
appended only after the harness runs; the experiment section in
`docs/RESEARCH.md` R-017 and the Evidence section here start as PENDING.

## 1. Goal and non-goals

Goal: a genuine PE32+ fixture (`pereg.exe`, built and linked like the
stage-1/2/3 fixtures) imports `RegOpenKeyExA`, `RegQueryValueExA` and
`RegCloseKey` from `advapi32.dll` — our first second-DLL allowlist entries —
and reads values out of the Phase 8 config store through our own `int 0x80`
services. The surface is read-only, registry-shaped, and honest about its
mapping: `HKLM` is the only backed root, a "key" is a store path prefix, a
"value" is one store entry, and every value is REG_SZ.

Evidence: a new scripted QEMU harness (`scripts/test_registry_qemu.{sh,py}`)
boots the dedicated registry ISO and asserts the full pinned serial transcript
(open/query/close success and error paths, size queries, `ERROR_MORE_DATA`,
handle reuse, double close, slot exhaustion); plus the full
`scripts/test_all.sh` regression run.

Non-goals: no registry hives, per-user/class namespaces, ACLs, access checks,
transactions, change notifications, enumeration (`RegEnumKeyEx`,
`RegEnumValue`), `RegGetValue`, write paths (`RegSetValueEx`, `RegCreateKeyEx`,
`RegDeleteKey`), symbolic links, typed values beyond REG_SZ (no REG_DWORD,
REG_BINARY, REG_MULTI_SZ, REG_EXPAND_SZ), `W`-variants or Unicode, remote
registry (`RegConnectRegistry`), or a new marker token. The store remains the
Phase 8 config store (`docs/CONFIG.md`); on-disk format is unchanged and no
new persistence is added. No claim of Windows registry compatibility follows
from this stage.

## 2. Primary sources (R-017)

R-017 records the exact fetched statements: `RegOpenKeyExA`'s case-insensitive
key names, empty/NULL-subkey handle identity for predefined keys, zero-only
`ulOptions` (or `REG_OPTION_OPEN_LINK`, which we reject), the access-mask
statement (we ignore `samDesired`), and "does not create the key" with the
`ERROR_FILE_NOT_FOUND` example; `RegQueryValueExA`'s argument optionality,
`lpReserved` must be NULL, `lpcbData` NULL-only-with-NULL-`lpData`, the
`ERROR_MORE_DATA` + required-size contract, the NULL-data size query, the
REG_SZ size includes the terminating NUL, the not-NUL-terminated warning, and
`ERROR_FILE_NOT_FOUND` for a missing value name; `RegCloseKey`'s
single argument and "handle must not be used after it has been closed"; plus
the WinError values (0, 2, 4, 6, 87, 234).

## 3. Import surface

`pe_loader.rs` widens from one DLL to a two-DLL allowlist
(`kernel32.dll`, `advapi32.dll`) and from twelve to fifteen function names.
The three new entries:

| Function | DLL | Service | Stub shape |
|---|---|---|---|
| `RegOpenKeyExA` | `advapi32.dll` | 15 | `Args5` |
| `RegQueryValueExA` | `advapi32.dll` | 16 | `Args6` (new) |
| `RegCloseKey` | `advapi32.dll` | 17 | `Args4` |

`RegQueryValueExA` is the first six-argument import. The stub page gains an
`Args6` shape: `mov r10, [rsp+0x28]` (fifth argument) and
`mov r11, [rsp+0x30]` (sixth argument) — 18 bytes of stub — so the fixed stub
stride grows from 16 to 24 bytes (still `int3`-padded, still far below the
command-line region; a compile-time assertion pins
`MAX_IMPORTS * STUB_STRIDE <= COMMAND_LINE_OFFSET`). `InterruptFrame.r11` is
already saved by `isr_common`, so the entry stub is unchanged. Import policy
stays: every import must be by name, from an allowlisted DLL, resolving to an
allowlisted function; `pebadimp.exe` (unknown DLL) and `pebadfn.exe`/
`pebadfn2.exe` (unknown function) keep being rejected before allocation.

## 4. Handle table

- `HKEY_LOCAL_MACHINE` (`0x8000_0002`) is the only backed root. The other
  predefined root values (`0x8000_0000..=0x8000_000F`) are recognized but not
  backed: they return `ERROR_FILE_NOT_FOUND` (2), because no per-user, class
  or user-hive namespace exists in this stage. Anything else that is not a
  live slot handle returns `ERROR_INVALID_HANDLE` (6).
- `RegOpenKeyExA` allocates one of **four** kernel slots
  (`kernel/src/registry.rs`, `IrqSpinLock<[Option<String>; 4]>`); the handle
  value is `0x1000 + slot` (4096..4099 in logs). When all four slots are
  taken the call returns `ERROR_TOO_MANY_OPEN_FILES` (4). Slot exhaustion and
  reuse are tested by the fixture; the count is a documented stage limit, not
  a Windows property.
- A slot stores the opened key path prefix (as given by the caller; lookups
  are case-insensitive, see below). `RegCloseKey` frees the slot; a closed or
  never-open handle returns `ERROR_INVALID_HANDLE` (6). `RegCloseKey` on
  `HKLM` is a documented no-op returning 0 (predefined handles are not ours to
  close).
- Handles die with the program: `user.rs` resets the table after every user
  program's teardown, so a later program cannot observe or close an earlier
  program's handles.

## 5. `RegOpenKeyExA` — service 15, five-argument stub

Arguments: `rcx` `hKey`, `rdx` `lpSubKey` (NULL allowed), `r8` `ulOptions`,
`r9` `samDesired`, `r10` `phkResult`.

Order of operations (normative):

1. `phkResult` is validated as a mapped user page with 8 writable bytes; an
   unmapped pointer kills the thread (memory-safety policy). On success the
   kernel stores `0` immediately — our zeroing contract for failed opens.
2. Subkey string read (section 6). A NULL pointer and `""` are equivalent.
   An unterminated or non-UTF-8 string within the 128-byte cap is an
   argument error (87) with the line
   `win32: RegOpenKeyExA({hkey:#x}, "<invalid>") -> 87 (invalid parameter)`.
3. `ulOptions != 0` → `ERROR_INVALID_PARAMETER` (87),
   `win32: RegOpenKeyExA({hkey:#x}, "{subkey}") -> 87 (invalid parameter)`.
   `REG_OPTION_OPEN_LINK` is deliberately not supported (deviations in R-017).
4. Lookup and slot allocation:
   - `hKey` invalid (not `HKLM`, not a live slot) → 6, line
     `-> 6 (invalid handle)`.
   - `hKey` another predefined root → 2 (`-> 2 (file not found)`).
   - subkey empty/NULL: `hKey == HKLM` returns `HKLM` itself without using a
     slot (documented refresh behavior, line
     `win32: RegOpenKeyExA(0x80000002, "") -> 0 (handle 0x80000002)`); a live
     handle opens a second handle to the same prefix (new slot).
   - non-empty subkey: the joined prefix must have children in the store
     (section 7); otherwise `ERROR_FILE_NOT_FOUND` (2).
   - no free slot → `ERROR_TOO_MANY_OPEN_FILES` (4), line
     `-> 4 (too many open keys)`.
5. Success stores the handle and logs
   `win32: RegOpenKeyExA({hkey:#x}, "{subkey}") -> 0 (handle {handle})`.
   On success `GetLastError` is untouched; every failure path also stores the
   returned code as the thread's last-error (our contract — the API returns
   the error directly, so this is extra state, not a Windows claim).

## 6. User strings

Subkey and value-name pointers are read byte by byte through the live page
tables: each touched page must be mapped user-accessible (an unmapped page
kills the thread, like every other service). At most 127 bytes plus NUL are
accepted; a mapped span without a terminator, or one that is not valid UTF-8,
is an argument error (87), never a kill. A NULL pointer means the empty
string. The 128-byte cap is our contract (R-017).

## 7. Key namespace and lookups

- A "key" exists when the store contains at least one entry whose key starts
  with `prefix` + `/` (the root prefix `""` always exists). Leaves are values,
  not keys: `RegOpenKeyExA(HKLM, "System")`
  succeeds because `System/Name` exists; `RegOpenKeyExA(HKLM, "System/Name")`
  succeeds only if nested entries such as `System/Name/X` exist.
- Lookups are ASCII case-insensitive per the fetched "key names are not case
  sensitive" statement. The store itself stays byte-exact (POCF); if a store
  ever contains two keys differing only in case, lookups return the
  lexicographically first match — a documented edge of the mapping, not a
  Windows claim.
- Value lookup joins `prefix` and the value name with `/` (`""` prefix yields
  the name alone, so `RegQueryValueExA(HKLM, "Display/Theme", ...)` reads the
  same entry as querying `"Theme"` under a handle for `"Display"`). The
  joined string is compared case-insensitively against whole store keys.

## 8. `RegQueryValueExA` — service 16, six-argument stub

Arguments: `rcx` `hKey`, `rdx` `lpValueName` (NULL allowed), `r8` `lpReserved`,
`r9` `lpType` (NULL allowed), `r10` `lpData` (NULL allowed),
`r11` `lpcbData` (NULL allowed only per the rules below).

Order of operations (normative):

1. Value name read (section 6); an invalid string is 87 with the `<invalid>`
   line. NULL or empty name means the unnamed/default value, which this stage
   does not model → `ERROR_FILE_NOT_FOUND` (2).
2. `lpReserved != 0` → 87 (documented "must be NULL").
3. `lpcbData == 0` → 87 (our contract; the fetched docs allow NULL only with
   NULL `lpData` and leave the result undefined).
4. `lpcbData` is validated (4 writable bytes) and `*lpcbData` is read as the
   caller's buffer size.
5. Handle and value lookup: invalid handle → 6; missing key or value → 2.
   Lines: `win32: RegQueryValueExA({handle}, "{name}") -> 6 (invalid handle)`
   / `-> 2 (file not found)`.
6. The value is REG_SZ by construction: type code 1, data `bytes + NUL`,
   required size `len + 1`.
7. If `lpType != 0` it is validated (4 writable bytes) and `REG_SZ` (1) is
   stored — on the size query, the `ERROR_MORE_DATA` path and success alike.
8. `lpData == 0`: size query. `*lpcbData = required`; return 0; line
   `-> 0 (REG_SZ, {required} bytes)`.
9. `*lpcbData < required`: `*lpcbData = required`; return
   `ERROR_MORE_DATA` (234); the data buffer is not touched; line
   `-> 234 (more data, {required} bytes needed)`.
10. Otherwise the `lpData..lpData + required` span is validated (unmapped
    kills), the NUL-terminated value is copied, `*lpcbData = required`; return
    0; line `-> 0 (REG_SZ, {required} bytes) "{value}"` (values are POCF-
    validated printable ASCII, so the quoted value is transcript-safe).

`GetLastError` is untouched on success; every failure path also stores the
returned code as the thread's last-error (our contract, see section 5).

## 9. `RegCloseKey` — service 17, `Args4` stub

`rcx` `hKey`. `HKLM` → no-op success (line `win32: RegCloseKey(0x80000002) -> 0`).
Live handle → slot freed, success (line `win32: RegCloseKey({handle}) -> 0`).
Anything else, including an already-closed handle → 6, line
`win32: RegCloseKey({handle}) -> 6 (invalid handle)`.

## 10. Kernel changes

- `kernel/src/registry.rs` (new): `HKEY_LOCAL_MACHINE`, slot table,
  `open`/`value`/`close`/`reset`, `KeyError { InvalidHandle, NotFound,
  TooManyKeys }`; the case-insensitive prefix/value helpers over
  `config::with`.
- `kernel/src/syscall.rs`: `SYS_REG_OPEN_KEY_EX = 15`,
  `SYS_REG_QUERY_VALUE_EX = 16`, `SYS_REG_CLOSE_KEY = 17`; three counters
  (`WIN32_REG_OPEN_KEY_CALLS`, `WIN32_REG_QUERY_VALUE_CALLS`,
  `WIN32_REG_CLOSE_KEY_CALLS`); `read_user_string` (128-byte cap),
  `validated_out_pointer_u64`, the three handlers and dispatch arms; module
  doc updated (services 15–17, `r11` sixth argument).
- `kernel/src/pe_loader.rs`: `WIN32_SERVICES` entries gain the DLL;
  `WIN32_DLLS` two-entry allowlist; `StubShape::Args6`; `STUB_STRIDE` 16 → 24
  with the new stub bytes and the compile-time assertion; comments updated.
- `kernel/src/user.rs`: `PEREG_PATH = "/boot/pereg.exe"`, `thread_pereg`,
  registry phase after the input phase (probe by module presence, 3 s
  window), and `registry::reset()` in the teardown path.
- Marker: unchanged (registry is part of the existing `win32` check; the
  default ISO does not contain `pereg.exe`, so every pinned marker, module
  count and serial assertion from earlier stages stays byte-identical).

## 11. Fixture (`userland/pe/pereg`) and call deltas

Crate mirrors `peinput`: `#![no_std] #![no_main]`, `raw-dylib` imports
(`advapi32.dll`: `RegOpenKeyExA`, `RegQueryValueExA`, `RegCloseKey`;
`kernel32.dll`: `WriteFile`, `ExitThread`), the `reloc_marker`/`marker_slot`
relocation probe and `/entry:_start` link. Sequence (`_start`, distinct exit
codes 1–20):

1. `RegOpenKeyExA(1, "System", 0, KEY_READ, &k)` → 6 and `k == 0` (exit 1).
2. `RegOpenKeyExA(HKLM, "System", 1, 0, &k)` → 87 and `k == 0` (exit 2).
3. `RegOpenKeyExA(HKLM, "Missing", 0, 0, &k)` → 2 and `k == 0` (exit 3).
4. `RegOpenKeyExA(HKLM, "System", 0, KEY_READ, &k)` → 0, `k == 0x1000`
   (slot 0; exit 4); this handle is `system`.
5. `RegQueryValueExA(system, "Missing", NULL, NULL, NULL, &cb)` → 2 (exit 5).
6. Size query `"Name"`: 0, `cb == 11`, type REG_SZ (exit 6).
7. `"Name"` with `cb = 4` → 234, `cb == 11` (exit 7).
8. `"Name"` with `cb = 64` → 0, `cb == 11`, type 1, bytes
   `PROJECT_OS\0` (exit 8).
9. Non-NULL reserved pointer → 87 (exit 9).
10. NULL `lpcbData` → 87 (exit 10).
11. `RegOpenKeyExA(HKLM, NULL, 0, 0, &k)` → 0, `k == HKLM` (exit 11).
12. `RegQueryValueExA(HKLM, "Display/Theme", ...)` → 0, `cb == 8`, bytes
    `classic\0` (exit 12).
13. `RegCloseKey(HKLM)` → 0 (exit 13).
14. Three more `"System"` opens → handles `0x1001..0x1003` (exit 14).
15. Fifth open → 4 and `k == 0` (exit 15).
16. `RegCloseKey(system)` → 0 (exit 16).
17. `RegCloseKey(system)` again → 6 (exit 17).
18. Reopen `"System"` → 0, `k == 0x1000` (slot reuse; exit 18).
19. Close the reopened handle and the three extras → all 0 (exit 19).
20. `WriteFile(1, "win32: registry surface verified\n", 33, ...)` → 1,
    `written == 33` (exit 20); `ExitThread(0)`.

Call deltas for the phase: `RegOpenKeyExA +10`, `RegQueryValueExA +7`,
`RegCloseKey +7`, `WriteFile +1`, console writes `+1` / `+33` bytes, exit code
0, no user faults. The fixture compares value bytes itself
(`PROJECT_OS\0`, `classic\0`), so the serial count lines and the exit code are
byte-level evidence, not just line presence.

## 12. Script changes

- `scripts/build_user_pe.sh`: build/copy `pereg.exe`; check it with
  `check_user_pe.py --imports 'advapi32.dll:RegCloseKey,RegOpenKeyExA,RegQueryValueExA;kernel32.dll:ExitThread,WriteFile'`;
  add it to the hash log (nine artifacts). No new mutant: `pebadimp.exe` /
  `pebadfn.exe` / `pebadfn2.exe` already cover unknown-DLL and
  unknown-function rejection.
- `scripts/check_user_pe.py`: `--imports` accepts semicolon-separated
  `dll:fn,fn` groups and verifies the descriptor set exactly (one descriptor
  per DLL, no duplicates, per-DLL function sets; single-group specs stay
  working).
- `scripts/make_iso.sh`: new `--registry-fixture` flag appends `pereg.exe` to
  the PE module list and writes `artifacts/project-os-registry.iso` with
  `debug_exit` kept; the three variant flags are mutually exclusive. Default
  and interactive outputs and their module lists are unchanged.
- `scripts/test_registry_qemu.sh`: requires the kernel artifact, python and
  QEMU, builds the registry ISO with `--no-build --registry-fixture`, runs the
  python harness and logs PASS plus the session-log path.
- `scripts/test_registry_qemu.py`: starts QEMU (q35, 512 MiB,
  `isa-debug-exit`, TCP serial `wait=on`, no typing), waits for
  `win32: registry surface verified`, then the boot marker (and its line
  terminator), and requires within the boot timeout: every pinned line from
  sections 5–9 (`RegOpenKeyExA` success/6/87/2, `RegQueryValueExA` sizes and
  error lines, `ERROR_MORE_DATA`, the quoted values `PROJECT_OS` and
  `classic`, handle `4096/4097/4098/4099`, double close, slot reuse,
  `-> 4 (too many open keys)`), `win32 registry selftest: passed (...)`,
  `PROJECT_OS_BOOT_SUCCESS:...checks=...win32,vfs...`, no
  `PROJECT_OS_FATAL`/`PROJECT_OS_PANIC`, and QEMU exit 33. Repeated lines are
  counted (`RegCloseKey(4096) -> 0` twice; the 87 line twice). The transcript
  is written to `artifacts/qemu-registry-session.log`.
- `scripts/test_all.sh`: one new step `registry` after `input`; expected
  summary becomes `PASS=24 FAIL=0 SKIP=4` while every existing step keeps its
  result.

## 13. Evidence and limits (claims this stage may and may not make)

Status: VERIFIED (2026-10-19). `scripts/test_registry_qemu.sh` → PASS;
session log `artifacts/qemu-registry-session.log` (10,420 bytes, sha256
`021807014f814d557227c3774d7231a3d47386ec33b023f843b72d736a8e5945`); registry
ISO `artifacts/project-os-registry.iso` (7,860,224 bytes, sha256
`1f93a4566e707602b54b7b65478fd87bf05ecc37ecb6b8b5ecaf873c7434d451`); fixture
`artifacts/pereg.exe` (5,120 bytes, sha256
`0ca37e2e762035bc98729b102d3a1d53340214e67365eba91daa72866fd3b3d4`); kernel
`artifacts/kernel.elf` (1,817,664 bytes, sha256
`6c26d6cce01f4457405554071b550542a7bd3e49e52c7755922ba01368f706bf`, build id
`0.1.0-b1b8542c`); `scripts/test_all.sh` → `PASS=24 FAIL=0 SKIP=4`. The
default ISO prints the two registry skip lines and the `pe`/`input`/`shell`
regressions stayed PASS.

May claim (with the logs above): a genuine PE32+ image reaches native,
allowlisted `advapi32.dll` services through its own import table (including the
first six-argument stub); the
registry-shaped view serves case-insensitive `HKLM` key opens over the config
store with a four-slot handle table, size queries, REG_SZ reads,
`ERROR_MORE_DATA`, the documented error values for invalid handle, invalid
parameter and missing key/value, `ERROR_TOO_MANY_OPEN_FILES` for exhaustion,
and handle-freeing close semantics with double-close rejection. Deltas from
Windows are stated: read-only, no hives/ACLs/transactions/redirection/enum,
no default values, one root, four slots, zeroed `*phkResult` on failed opens,
always-NUL-terminated values, `W`-variants and Unicode unsupported.

May **not** claim: general registry compatibility, any write API, enumeration,
per-user or class namespaces, security semantics, typed values beyond the
REG_SZ mapping, or behavior of a real Windows registry beyond the fetched
statements in R-017.

## 14. Verification plan (executed 2026-10-19)

1. `scripts/format.sh`, `scripts/lint.sh`: clean.
2. `scripts/test_unit.sh`: 269/269 unchanged (no new `os_core` code).
3. `scripts/build_user_pe.sh`: nine artifacts incl. `pereg.exe` (5,120 B),
   each checker-verified; sizes and SHA-256 recorded; the eight existing
   fixtures kept their recorded hashes (`peinput.exe` `a2ab21aa…`).
4. `scripts/make_iso.sh --registry-fixture`: registry ISO with eight PE
   modules (pereg added after pebadfn2); default ISO still seven.
5. `scripts/test_registry_qemu.sh`: PASS; session log recorded (section 13).
6. `scripts/test_input_qemu.sh` and `scripts/test_pe_qemu.sh`: PASS unchanged
   (regressions for stub stride, allowlist and default ISO).
7. `scripts/test_all.sh`: `PASS=24 FAIL=0 SKIP=4`; exact hashes recorded.

## 15. Documentation and evidence updates

`docs/RESEARCH.md` R-017 (experiment/conclusion after the run), ADR-0024,
`docs/ARCHITECTURE.md` (syscall services 15–17, two-DLL allowlist, user
phase), `docs/TESTING.md` (registry step, fixture, counts),
`docs/ROADMAP.md` (Phase 8 stage 3 complete), `docs/WINDOWS_COMPATIBILITY.md`
(registry read surface and its gaps), `docs/PE_LOADER.md` (allowlist, stub
shapes), `progress.md` (commands, logs, hashes, defects).
