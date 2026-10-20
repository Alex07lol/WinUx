# Win32 registry enumeration and key deletion — `advapi32.dll` (Phase 8, stage 5)

Status: contract written before implementation (2026-10-20). Evidence is
appended only after the harness runs; the experiment section in
`docs/RESEARCH.md` R-019 and the Evidence section here start as PENDING.

## 1. Goal and non-goals

Goal: a genuine PE32+ fixture (`peregen.exe`, built and linked like every
earlier fixture) imports `RegEnumKeyExA`, `RegEnumValueA` and `RegDeleteKeyA`
from `advapi32.dll` and enumerates/mutates the Phase 8 config store through
our own `int 0x80` services, following the fetched non-recursive deletion
rule. Successes and failure values are asserted by the fixture itself
(distinct exit codes), and the boot phase verifies the store afterwards.

Evidence: a new scripted QEMU harness (`scripts/test_registry_enum_qemu.{sh,py}`)
boots the dedicated registry-enum ISO and asserts the full pinned serial
transcript; plus the full `scripts/test_all.sh` regression run.

Non-goals: no hives, ACLs or access-rights enforcement, no security
descriptors, no transactions or change notifications, no
`RegDeleteTree`/`RegDeleteKeyEx`, no class names or
`REG_QUERY_INFO_KEY` statistics (the fetched `lpClass`/`lpcchClass`/
`lpftLastWriteTime` outputs are ignored, never dereferenced), no value or
key ordering guarantee beyond our deterministic insertion order, no `W`/
Unicode variants, no remote registry, no symbolic links or redirection, no
typed values beyond REG_SZ, no default/unnamed values, and no recursion in
`RegDeleteKeyA` (per the fetched rule). No new marker token. No claim of
Windows registry compatibility follows from this stage.

## 2. Primary sources (R-019)

R-019 records the exact fetched statements: `RegEnumKeyExA`'s eight-argument
shape and order, "Enumerates the subkeys", "retrieves information about one
subkey each time it is called", the arbitrary-order statement, the
`lpName`-includes-NUL and "only the name of the subkey" statements, the
`lpcchName` in/out rule (buffer size includes the NUL; success reports the
stored characters without it), `lpReserved` "must be NULL", the
`lpClass`/`lpcchClass` NULL pairing and optional `lpftLastWriteTime`,
`ERROR_NO_MORE_ITEMS`/`ERROR_MORE_DATA`; `RegEnumValueA`'s eight-argument
shape, "copies one indexed value name and data block", the name and
`lpcchValueName` rules, the undefined-size statement on a too-small name
buffer, `lpReserved` "must be NULL", optional `lpType`, the
`lpData`/`lpcbData` NULL pairing and size-query statement, REG_SZ size
including the NUL, and the 234/259 conditions; `RegDeleteKeyA`'s two-argument
shape, "Deletes a subkey and its values", "cannot have subkeys", "cannot be
NULL", "The access rights ... do not affect the delete operation", "A
deleted key is not removed until the last handle to it is closed", and the
enumerate-then-delete prescription; plus the WinError values 5, 6, 2, 87,
234 and 259.

## 3. Import surface

The two-DLL allowlist (`kernel32.dll`, `advapi32.dll`) grows from eighteen to
twenty-one function names. The three new entries:

| Function | DLL | Service | Stub shape |
|---|---|---|---|
| `RegDeleteKeyA` | `advapi32.dll` | 21 | `Args2` (new) |
| `RegEnumKeyExA` | `advapi32.dll` | 22 | `Args8` (new) |
| `RegEnumValueA` | `advapi32.dll` | 23 | `Args8` (new) |

`Args8` forwards the fifth–eighth arguments through `r10` (`mov r10,
[rsp+0x28]`), `r11` (`[rsp+0x30]`), `r12` (`[rsp+0x38]`) and `r13`
(`[rsp+0x40]`) — 27 bytes of stub — inside the existing 48-byte
`STUB_STRIDE` (compile-time assertion unchanged: 16 × 48 = 768 ≤ 3840).
`Args2` uses only `rcx`/`rdx` (7-byte stub). `isr_common` already saves
`r12`/`r13`. Import policy stays: by name, allowlisted DLL, allowlisted
function; the existing negative mutants keep being rejected before
allocation. The per-image unique-import cap (`os_core::pe::MAX_IMPORTS` = 16)
is unchanged.

## 4. Handle table

Unchanged from stages 3–4: `HKEY_LOCAL_MACHINE` (`0x8000_0002`) is the only
backed root, other predefined roots return 2, anything else that is not a
live slot handle returns 6, four slots `0x1000 + slot` with
`ERROR_TOO_MANY_OPEN_FILES` (4) on exhaustion, `RegCloseKey` semantics as
before, and the table resets after every program's teardown. No access
rights are enforced; the fetched "access rights ... do not affect the delete
operation" matches our no-rights model.

## 5. Namespace model for enumeration

The stage-3/4 flattened model is unchanged for reads and writes. For a key
prefix `P`:

- **Child keys** are the distinct first components `c` of entry paths whose
  tail after `P/` contains a `/` (an entry `P/c/...`); comparison is ASCII
  case-insensitive and the first-seen spelling wins.
- **Values** are the entries exactly under `P` — the full remaining tail is
  the value name. A value name may contain `/` (stage-4 joined-path rule), so
  one entry can appear both as a value of `P` and as part of the child key
  `P/c`; Windows' separate key/value namespaces cannot represent this. The
  overlap is a stated consequence of the flattened store, not hidden.
- **Order** is store insertion order (the order returned by
  `os_core::config::Config::list`), stable while the store is unchanged.
  The fetched pages say enumeration order is arbitrary; deterministic order
  is our contract so fixtures and hashes can pin it.
- **Exhaustion** is index ≥ count → `ERROR_NO_MORE_ITEMS` (259). The index
  is a `DWORD`; no wrap semantics are defined.
- Mutating the store between calls is allowed but shifts indices; the fetched
  warning against concurrent changes is noted but not enforced.

## 6. `RegEnumKeyExA` — service 22, eight-argument stub

Arguments: `rcx` `hKey`, `rdx` `dwIndex`, `r8` `lpName`, `r9` `lpcchName`,
`r10` `lpReserved`, `r11` `lpClass`, `r12` `lpcchClass`, `r13`
`lpftLastWriteTime`.

Order of operations (normative):

1. `lpReserved != 0` → 87 (fetched "must be NULL"). `lpClass`,
   `lpcchClass` and `lpftLastWriteTime` are ignored and never dereferenced,
   whatever their values (fetched outputs we do not model; deviation stated).
2. `lpName == 0` or `lpcchName == 0` → 87; `*lpcchName == 0` → 87 (the size
   must include room for the NUL).
3. Handle lookup: invalid → 6; another predefined root → 2.
4. The child list at `dwIndex` is looked up; out of range → 259.
5. If `*lpcchName < name.len() + 1` → 234 with `*lpcchName = name.len() + 1`
   (required size including the NUL — our contract; the fetched page leaves
   the failure size undefined). No bytes are written.
6. Success: NUL-terminated name copied into `lpName` (span validated per
   page; unmapped kills), `*lpcchName = name.len()` (excluding the NUL —
   fetched rule). Line:
   `win32: RegEnumKeyExA({hkey:#x}, {index}) -> 0 ("{name}")`.
   Failures: `win32: RegEnumKeyExA({hkey:#x}, {index}) -> {code} ({label})`.
   Success leaves `GetLastError` untouched; failures store the code.

Class and last-write-time outputs are never filled; a caller that relies on
them must treat this as unsupported (stated).

## 7. `RegEnumValueA` — service 23, eight-argument stub

Arguments: `rcx` `hKey`, `rdx` `dwIndex`, `r8` `lpValueName`, `r9`
`lpcchValueName`, `r10` `lpReserved`, `r11` `lpType`, `r12` `lpData`, `r13`
`lpcbData`.

Order of operations (normative):

1. `lpReserved != 0` → 87 (fetched).
2. `lpValueName == 0` or `lpcchValueName == 0` → 87; `*lpcchValueName == 0`
   → 87.
3. Handle lookup: invalid → 6; other predefined root → 2.
4. The value list at `dwIndex` is looked up; out of range → 259.
5. Name buffer: if `*lpcchValueName < name.len() + 1` → 234 with
   `*lpcchValueName = name.len() + 1`, no writes to the name or data
   (fetched: contents undefined; we define "nothing written").
6. Name copied (NUL-terminated, span validated); `*lpcchValueName =
   name.len()`.
7. `lpData == 0 && lpcbData == 0` → success, name/type only.
   `lpData == 0 && lpcbData != 0` → success, `*lpcbData = value.len() + 1`
   (fetched size-query rule).
   `lpData != 0 && lpcbData == 0` → 87 (fetched NULL pairing).
8. Data buffer too small (`*lpcbData < value.len() + 1`) → 234 with
   `*lpcbData = value.len() + 1` (fetched); the name was already written in
   step 6. Otherwise the payload plus its NUL is copied (span validated) and
   `*lpcbData = value.len() + 1`.
9. `lpType != 0` → `*lpType = 1` (`REG_SZ`) on success (fetched optional
   output; only REG_SZ exists here).
10. Success line:
    `win32: RegEnumValueA({hkey:#x}, {index}) -> 0 ("{name}", REG_SZ, {n} bytes)`
    where `{n} = value.len() + 1`. Failures are code/label lines as above.
    `GetLastError` contract unchanged.

## 8. `RegDeleteKeyA` — service 21, two-argument stub

`rcx` `hKey`, `rdx` `lpSubKey`.

Order of operations (normative):

1. Subkey string read (stage-3 rules: NULL/empty equivalent, 128-byte cap,
   unterminated/non-UTF-8 → 87 with the `<invalid>` line). NULL or empty →
   87: the fetched parameter "cannot be NULL", and `HKLM` itself is not a
   deletable subkey.
2. Handle lookup: invalid → 6; another predefined root → 2.
3. The joined path (prefix + subkey, multi-component subkeys allowed) must be
   a valid store key path → otherwise 87.
4. The key must exist (children under its prefix) → otherwise 2.
5. **Non-recursive rule (fetched):** if any entry under the prefix has a `/`
   after it (a child key exists) → `ERROR_ACCESS_DENIED` (5). The fetched
   page states the "must not have subkeys" rule but names no code; 5 is our
   contract (confidence medium, R-019). The caller is expected to enumerate
   and delete children first, as the page prescribes.
6. Otherwise every direct entry under the prefix is removed and the store is
   persisted through `config::save()`. A key that is already absent after
   removals is not an error. Line:
   `win32: RegDeleteKeyA({hkey:#x}, "{subkey}") -> 0`; failures use the
   code/label form.
7. Open handles to the deleted prefix stay valid but become stale: reads on
   them return 2, enumerations 259, and `RegCloseKey` still succeeds. This
   deviates from the fetched "not removed until the last handle is closed"
   (our store has no key objects to defer); stated, not hidden.

## 9. Persistence

Every successful `RegDeleteKeyA` serialises the store through the existing
`config::save()` (the same `POCF` blob in `/etc/config` the checkpoint
carries). Deletion under the registry-enum boot leaves no entries behind:
the fixture removes its whole `Enum` tree, and the boot phase verifies the
store is byte-equivalent to the three seeded defaults before restore. The
disk flush itself stays at shell-command boundaries (stage-1 contract); this
boot runs no shell commands, so no disk blocks are written.

## 10. Kernel changes

- `kernel/src/registry.rs`: `KeyError` gains `NoMoreItems` (259) and
  `AccessDenied` (5). New helpers for prefix-scoped entry iteration and
  child-name collection (case-insensitive dedup, insertion order). New
  `delete_key(parent, subkey)`, `enum_key(parent, index) -> String` and
  `enum_value(parent, index) -> (String, String)`; delete removes the direct
  entries and calls `config::save()` after the store lock is released.
- `kernel/src/syscall.rs`: `SYS_REG_DELETE_KEY = 21`,
  `SYS_REG_ENUM_KEY_EX = 22`, `SYS_REG_ENUM_VALUE = 23`; three counters;
  `ERROR_ACCESS_DENIED = 5`, `ERROR_NO_MORE_ITEMS = 259`;
  `reg_error_label` gains "access denied" and "no more items"; the three
  handlers (with the new user-buffer write helpers for name/data/type and
  the in/out sizes), dispatch arms, and the module doc (services 15–23).
- `kernel/src/pe_loader.rs`: `StubShape::Args2` and `Args8`;
  `WIN32_SERVICES` grows to twenty-one entries; comments updated; the
  48-byte stride and its assertion are unchanged.
- `kernel/src/user.rs`: `PEREGEN_PATH = "/boot/peregen.exe"`, `thread_peregen`,
  a registry-enum phase after the registry-write phase (probe by module
  presence, 3 s window), skip lines when absent, and the defaults-restored
  assertion.
- Marker: unchanged (`win32`), so default ISOs keep every pinned serial
  assertion; only the new variant ISO carries `peregen.exe`.

## 11. Fixture (`userland/pe/peregen`) and call deltas

Crate mirrors `peregw`: `#![no_std] #![no_main]`, `raw-dylib` imports
(`advapi32.dll`: `RegCloseKey`, `RegCreateKeyExA`, `RegDeleteKeyA`,
`RegEnumKeyExA`, `RegEnumValueA`, `RegQueryValueExA`, `RegSetValueExA`;
`kernel32.dll`: `ExitThread`, `WriteFile`), the relocation probe and
`/entry:_start` link. Sequence (`_start`, exit codes 1–33):

1. Create `Enum` → 0, disposition 1, handle 0x1000 (`kE`) (exit 1).
2. Create `Enum/Alpha` → 0, disposition 1, handle 0x1001 (`kA`) (exit 2).
3. `RegSetValueExA(kA, "First", REG_SZ, "one\0", 4)` → 0 (exit 3).
4. `RegSetValueExA(kA, "Second", REG_SZ, "two\0", 4)` → 0 (exit 4).
5. Create `Enum/Beta` → 0, disposition 1, handle 0x1002 (`kB`) (exit 5).
6. `RegSetValueExA(kB, "Only", REG_SZ, "1\0", 2)` → 0 (exit 6).
7. Create `Enum/Beta/Child` → 0, disposition 1, handle 0x1003 (`kC`) (exit 7).
8. `RegSetValueExA(kC, "Deep", REG_SZ, "x\0", 2)` → 0 (exit 8).
9. `RegEnumKeyExA(kE, 0, name(32), &len)` → 0, `"Alpha"`, `len == 5` (exit 9).
10. Index 1 → 0, `"Beta"`, `len == 4` (exit 10).
11. Index 2 → 259 (exit 11).
12. Index 0 with `len = 1` → 234, `*len == 6` (exit 12).
13. `RegEnumValueA(kA, 0, vname(32), &vlen, NULL, &type, data(8), &cb)` →
    0, `"First"`, `type == 1`, `"one\0"`, `cb == 4` (exit 13).
14. Index 1 → 0, `"Second"`, `"two\0"`, `cb == 4` (exit 14).
15. Index 2 → 259 (exit 15).
16. Index 0 with `lpData = NULL`, `cb = 0` → 0, `"First"`, `*cb == 4`
    (exit 16).
17. Index 0 with `cb = 2` → 234, `*cb == 4` (exit 17).
18. Bad handle 1 → 6 (exit 18).
19. Nonzero `lpReserved` → 87 (exit 19).
20. `RegDeleteKeyA(HKLM, "Enum/Beta")` → 5 (child key exists) (exit 20).
21. `RegDeleteKeyA(HKLM, "Enum/Beta/Child")` → 0 (exit 21).
22. `RegEnumKeyExA(kB, 0, ...)` → 259 (child gone) (exit 22).
23. `RegDeleteKeyA(HKLM, "Enum/Beta")` → 0 (only a value remains) (exit 23).
24. `RegEnumKeyExA(kE, 1, ...)` → 259 (Beta gone) (exit 24).
25. `RegDeleteKeyA(HKLM, "Enum/Missing")` → 2 (exit 25).
26. `RegDeleteKeyA(1, "Enum")` → 6 (exit 26).
27. `RegDeleteKeyA(HKLM, "")` → 87 (exit 27).
28. `RegDeleteKeyA(HKLM, "Bad Name")` → 87 (exit 28).
29. `RegDeleteKeyA(HKLM, "Enum/Alpha")` → 0 (exit 29).
30. `RegQueryValueExA(kA, "First", ...)` → 2 (stale handle) (exit 30).
31. `RegEnumValueA(kA, 0, ...)` → 259 (exit 31).
32. `RegDeleteKeyA(HKLM, "Enum")` → 2 (nothing left; `Enum` was never
    materialised) (exit 32).
33. `RegCloseKey` on `kE`/`kA`/`kB`/`kC` → 0 ×4 (exit 33); write the
    45-byte summary `win32: registry enumeration surface verified\n`
    (exit 34); `ExitThread(0)`.

Call deltas: create +4, set +4, delete-key +9, enum-key +8, enum-value +6,
query +1, close +4, `WriteFile` +1 (45 bytes), exit 0, no user faults; plus
the boot-phase defaults assertion. The fixture compares bytes itself, so the
counts and exit codes are byte-level evidence.

## 12. Script changes

- `scripts/build_user_pe.sh`: build/copy `peregen.exe`; check with
  `--imports 'advapi32.dll:RegCloseKey,RegCreateKeyExA,RegDeleteKeyA,RegDeleteValueA,RegEnumKeyExA,RegEnumValueA,RegOpenKeyExA,RegQueryValueExA,RegSetValueExA;kernel32.dll:ExitThread,WriteFile'`;
  ten artifacts become eleven.
- `scripts/make_iso.sh`: new `--registry-enum-fixture` flag appends
  `peregen.exe` and writes `artifacts/project-os-registry-enum.iso` with
  `debug_exit` kept; the five variant flags are mutually exclusive.
- `scripts/test_registry_enum_qemu.sh`: requires kernel/python/QEMU, builds
  the enum ISO with `--no-build --registry-enum-fixture`, runs the python
  harness, logs PASS and the session-log path.
- `scripts/test_registry_enum_qemu.py`: QEMU (q35, 512 MiB,
  `isa-debug-exit`, TCP serial `wait=on`, no typing); waits for the summary
  line, then the boot marker; requires every pinned line from sections 6–8,
  the `win32 registry enumeration selftest: passed (...)` line, the marker
  with `win32,vfs`, no fatal/panic, and QEMU exit 33; repeated lines counted;
  transcript at `artifacts/qemu-registry-enum-session.log`.
- `scripts/test_all.sh`: one new step `registry-enum` after
  `registry-write`; expected summary `PASS=26 FAIL=0 SKIP=4` with every
  earlier step unchanged.

## 13. Documentation and evidence updates

`docs/RESEARCH.md` R-019 (experiment/conclusion after the run), ADR-0026,
`docs/ARCHITECTURE.md` (services 15–23, stub shapes), `docs/TESTING.md`
(registry-enum step, fixture, counts), `docs/ROADMAP.md` (stage 5),
`docs/WINDOWS_COMPATIBILITY.md` (enumeration/deletion surface and remaining
gaps), `docs/PE_LOADER.md` (allowlist, stub shapes), `progress.md`.

## 14. Evidence and limits (claims this stage may and may not make)

Status: PENDING. Filled after the harness and `test_all.sh` runs.

May claim (with the logs): a genuine PE32+ image enumerates subkeys and
REG_SZ values of the config store and deletes keys under the fetched
non-recursive rule through native, allowlisted `advapi32.dll` services
(including new eight- and two-argument stub forms), with documented
234/259/5/6/2/87 results, deterministic insertion-order enumeration, and
persistence through the same `/etc/config` codec. The flattened-namespace
overlap (a `/`-containing value name is also visible as a child key) and the
immediate-removal deviation are stated.

May **not** claim: general registry compatibility, hives, security/access
rights, transactions, class or last-write-time outputs, recursion
(`RegDeleteTree`/`RegDeleteKeyEx`), typed values beyond REG_SZ, default
values, empty key objects, ordering guarantees matching Windows (ours is
insertion order), or namespace rules such as the direct-HKLM-child
prohibition.

## 15. Verification plan (to be executed)

1. `scripts/format.sh`, `scripts/lint.sh`.
2. `scripts/test_unit.sh`: 269/269 expected (no new `os_core` code; registry
   logic stays in the kernel crate and is exercised by the PE harness).
3. `scripts/build_user_pe.sh`: eleven artifacts incl. `peregen.exe`, each
   checker-verified; hashes recorded.
4. `scripts/make_iso.sh --registry-enum-fixture`: enum ISO with eight PE
   modules; default ISO still seven.
5. `scripts/test_registry_enum_qemu.sh`: PASS; session log recorded.
6. `scripts/test_registry_write_qemu.sh`, `scripts/test_registry_qemu.sh`,
   `scripts/test_input_qemu.sh`, `scripts/test_pe_qemu.sh`: PASS unchanged.
7. `scripts/test_all.sh`: `PASS=26 FAIL=0 SKIP=4`; exact hashes recorded.
