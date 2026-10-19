# Win32 registry write surface — `advapi32.dll` mutations over the config store (Phase 8, stage 4)

Status: contract written before implementation (2026-10-19). Evidence is
appended only after the harness runs; the experiment section in
`docs/RESEARCH.md` R-018 and the Evidence section here start as PENDING.

## 1. Goal and non-goals

Goal: a genuine PE32+ fixture (`peregw.exe`, built and linked like every
earlier fixture) imports `RegCreateKeyExA`, `RegSetValueExA` and
`RegDeleteValueA` from `advapi32.dll` and mutates the Phase 8 config store
through our own `int 0x80` services, persisted through the same
`/etc/config` VFS file the checkpoint carries. Successes and failure values
are asserted by the fixture itself (distinct exit codes), and the boot phase
verifies the store and the serialised file afterwards.

Evidence: a new scripted QEMU harness (`scripts/test_registry_write_qemu.{sh,py}`)
boots the dedicated registry-write ISO and asserts the full pinned serial
transcript; plus the full `scripts/test_all.sh` regression run.

Non-goals: no hives, ACLs or access-rights enforcement, no security
descriptors, no transactions, no change notifications, no enumeration, no
`RegDeleteKey`/`RegDeleteTree` (key removal stays implicit: a key exists only
while it has entries), no named/default-value writes, no typed values beyond
REG_SZ (no REG_DWORD/REG_BINARY/REG_MULTI_SZ/REG_EXPAND_SZ), no `W`-variants,
no remote registry, no symbolic links or redirection, no
`RegCreateKeyExA` empty-key objects (a "created" key with no values leaves
no trace), and no enforcement of the fetched "cannot create a direct child of
HKLM" namespace rule. No new marker token. No claim of Windows registry
compatibility follows from this stage.

## 2. Primary sources (R-018)

R-018 records the exact fetched statements: `RegCreateKeyExA`'s nine-argument
shape and order, "If the key already exists, the function opens it", the
empty-subkey handle statement, "Reserved ... must be zero", `lpClass` "may be
ignored", the four `dwOptions` values, the access statement that key creation
is checked against the key's security descriptor "not the access mask
specified when the handle was obtained", the optional `lpdwDisposition` and
the two disposition values, "The key ... creates has no values" and the
direct-HKLM-child prohibition remark; `RegSetValueExA`'s six-argument shape,
the NULL/empty-name default-value statement, "Reserved ... must be zero",
the null-termination requirement for string types, the NULL `lpData` only
with `cbData` 0 rule and "cbData must include the size of the terminating
null character"; `RegDeleteValueA`'s two-argument shape, the NULL/empty-name
statement and "value names are not case sensitive"; plus the WinError values
(0, 2, 4, 6, 8, 87, 234) and the disposition constants
(`REG_CREATED_NEW_KEY` 1, `REG_OPENED_EXISTING_KEY` 2).

## 3. Import surface

The two-DLL allowlist (`kernel32.dll`, `advapi32.dll`) grows from fifteen to
eighteen function names. The three new entries:

| Function | DLL | Service | Stub shape |
|---|---|---|---|
| `RegCreateKeyExA` | `advapi32.dll` | 18 | `Args9` (new) |
| `RegSetValueExA` | `advapi32.dll` | 19 | `Args6` |
| `RegDeleteValueA` | `advapi32.dll` | 20 | `Args4` |

`RegCreateKeyExA` is the first nine-argument import. The stub page gains an
`Args9` shape: after the `Args6` five-byte loads it forwards the seventh,
eighth and ninth arguments through `r12` (`mov r12, [rsp+0x38]`), `r13`
(`mov r13, [rsp+0x40]`) and `r14` (`mov r14, [rsp+0x48]`) — 33 bytes of stub —
so the fixed stub stride grows from 24 to 48 bytes (still `int3`-padded; a
compile-time assertion pins `MAX_IMPORTS * STUB_STRIDE <= COMMAND_LINE_OFFSET`,
16 × 48 = 768 ≤ 3840). `isr_common` already saves `r12`–`r14`, so the entry
stub is unchanged and the kernel reads them from `InterruptFrame`. Import
policy stays: every import must be by name, from an allowlisted DLL,
resolving to an allowlisted function; the existing negative mutants keep
being rejected before allocation. The per-image unique-import cap
(`os_core::pe::MAX_IMPORTS` = 16) is unchanged; eighteen allowlisted names
means a single image still cannot import all of them, which is the existing
documented cap.

## 4. Handle table

Unchanged from stage 3: `HKEY_LOCAL_MACHINE` (`0x8000_0002`) is the only
backed root, other predefined roots return 2, anything else that is not a
live slot handle returns 6, four slots `0x1000 + slot` with
`ERROR_TOO_MANY_OPEN_FILES` (4) on exhaustion, `RegCloseKey` semantics as
before, and the table resets after every program's teardown. Write services
use the same handles; handles carry no access rights and `samDesired` is
ignored everywhere (see section 5). `RegCreateKeyExA` allocates its handle
from the same four slots, so a program can hold at most four open keys
total across read and write services.

## 5. `RegCreateKeyExA` — service 18, nine-argument stub

Arguments: `rcx` `hKey`, `rdx` `lpSubKey` (NULL allowed), `r8` `Reserved`,
`r9` `lpClass`, `r10` `dwOptions`, `r11` `samDesired`, `r12`
`lpSecurityAttributes`, `r13` `phkResult`, `r14` `lpdwDisposition`.

Documented deviations (stated, not hidden): a NULL `lpSubKey` is treated as
the empty string (the fetched page says the parameter "cannot be NULL"; we
keep stage 3's NULL-is-empty rule); `lpClass` and `lpSecurityAttributes` are
ignored entirely and never dereferenced (the page says `lpClass` "may be
ignored"); `samDesired` is ignored because creation access is checked against
the key's security descriptor, which we do not have (the page explicitly
notes the handle's access mask is not what is checked); the direct-HKLM-child
prohibition remark is not enforced (no namespace policy exists); no empty key
objects are created (a key exists only while entries live under it, section
7), so `REG_CREATED_NEW_KEY` means "the prefix had no entries underneath
before this call" and a created-but-never-populated key leaves no trace.

Order of operations (normative):

1. `phkResult` is validated as a mapped user page with 8 writable bytes; an
   unmapped pointer kills the thread. On success the kernel stores `0`
   immediately — our zeroing contract for failed creates.
2. Subkey string read (stage-3 rules: NULL/empty equivalent, 128-byte cap,
   unterminated/non-UTF-8 → 87 with the `<invalid>` line).
3. `Reserved != 0` → 87 (fetched "must be zero").
4. `dwOptions != 0` → 87. Only `REG_OPTION_NON_VOLATILE` (0) is supported;
   `REG_OPTION_VOLATILE`, `REG_OPTION_CREATE_LINK` and
   `REG_OPTION_BACKUP_RESTORE` are deliberately rejected.
5. Lookup and creation:
   - `hKey` invalid → 6; another predefined root → 2.
   - empty subkey → a new slot handle to the same key (fetched: `phkResult`
     receives "a new handle to the key specified by `hKey`"; note this differs
     from stage 3's `RegOpenKeyExA` refresh statement) and always reports
     `REG_OPENED_EXISTING_KEY` (2): the parent handle denotes an existing key,
     including the `HKLM` root.
   - non-empty subkey → the joined prefix must be a valid store key path
     (`os_core::config` rules: components of `[A-Za-z0-9_.-]`, ≤32 bytes
     each, whole path ≤96 bytes); an invalid path is an argument error (87),
     not a lookup miss. Unlike stage 3's read-only open (which simply misses
     and returns 2), the write surface validates what it would create; this
     asymmetry is deliberate and documented.
   - disposition: `REG_CREATED_NEW_KEY` (1) when the prefix had no children,
     `REG_OPENED_EXISTING_KEY` (2) otherwise (and always for an empty
     subkey).
   - no free slot → 4.
6. Success: the handle is stored, `phkResult` receives it, and
   `lpdwDisposition` (when non-NULL; validated 4 writable at this point) is
   filled with the disposition. Line:
   `win32: RegCreateKeyExA({hkey:#x}, "{subkey}") -> 0 (handle {handle}, created)`
   or `(handle {handle}, opened existing)`. On success `GetLastError` is
   untouched; every failure path also stores the returned code as the
   thread's last-error (stage-3 contract).

Failure lines mirror stage 3:
`win32: RegCreateKeyExA({hkey:#x}, "{subkey}") -> {code} ({label})`.

## 6. User strings

Unchanged from stage 3 (section 6 there): byte-wise reads through the live
page tables, unmapped pages kill, 127 bytes plus NUL cap, invalid strings are
87, NULL means empty. The same helper covers subkey and value-name pointers
for all three new services.

## 7. Key namespace and writes

- The stage-3 namespace rules stay in force: a key exists when the store has
  an entry starting with `prefix` + `/` (children-only; leaves are values),
  lookups are ASCII case-insensitive against the byte-exact POCF store, and
  value lookup joins `prefix` and the value name with `/`.
- Writes materialise entries, never empty keys: `RegSetValueExA` on a handle
  writes the joined entry; `RegDeleteValueA` removes it. If the last entry
  under a prefix is deleted, the key ceases to exist and later opens return 2
  — there are no empty key objects (deviation from Windows; the fetched page
  notes a created key "has no values", a state our store cannot represent).
- A value name that yields an invalid joined path (bad characters, empty
  component, over 96 bytes) is rejected with 87 by both write services;
  reads keep stage 3's simply-miss behavior.
- Deleting a name that is not present returns 2. The fetched
  `RegDeleteValueA` page does not enumerate a missing-value error; 2 is our
  contract, chosen for consistency with the documented
  `RegQueryValueExA` missing-value error (confidence noted in R-018).

## 8. `RegSetValueExA` — service 19, six-argument stub

Arguments: `rcx` `hKey`, `rdx` `lpValueName`, `r8` `Reserved`, `r9` `dwType`,
`r10` `lpData`, `r11` `cbData`.

Order of operations (normative):

1. Value-name read (section 6); invalid → 87 with the `<invalid>` line.
   NULL or empty name means the unnamed/default value, which this stage does
   not model → 87 (the fetched page defines default values; we reject rather
   than silently redirect; line with `""`).
2. `Reserved != 0` → 87 (fetched "must be zero").
3. `dwType != REG_SZ` (1) → 87. Only REG_SZ is supported.
4. `lpData == 0` → 87. The fetched page allows NULL data with `cbData` 0
   ("a null value"); we do not model null REG_SZ values and reject both.
5. `cbData` must be `1..=129`: REG_SZ data includes the terminating NUL
   (fetched statement) and store values cap at 128 printable ASCII bytes, so
   a payload is at most 128 bytes plus NUL. Anything else → 87.
6. The `lpData..lpData + cbData` span is validated (unmapped kills); the
   final byte must be NUL → otherwise 87. The payload (bytes before the
   terminator) must be valid store data (printable ASCII, ≤128 bytes) →
   otherwise 87.
7. Handle lookup: invalid → 6; another predefined root → 2.
8. The joined entry is written to the store and persisted through
   `config::save()` (section 10). Store-full / too-large / save failures map
   to `ERROR_NOT_ENOUGH_MEMORY` (8), our contract for resource exhaustion
   (labels in section 11).
9. Success line:
   `win32: RegSetValueExA({handle}, "{name}") -> 0 (REG_SZ, {payload} bytes)`
   where `{payload} = cbData - 1`. `GetLastError` untouched on success;
   failures store the code (stage-3 contract).

## 9. `RegDeleteValueA` — service 20, `Args4` stub

`rcx` `hKey`, `rdx` `lpValueName`. Name read (invalid → 87; NULL/empty means
the default value, not modeled → 87). Handle lookup (6 / 2 as above). The
joined entry is removed and persisted; line
`win32: RegDeleteValueA({handle}, "{name}") -> 0`. A missing entry → 2
(`file not found`); invalid paths → 87; resource failures at save time → 8.

## 10. Persistence

Each successful mutation (`RegSetValueExA`, `RegDeleteValueA`) serialises the
store through the existing `config::save()` — the same `POCF` blob in the
VFS file `/etc/config` that the checkpoint already carries, verifies (CRC)
and falls back for. The disk flush itself stays at shell-command boundaries
(stage-1 contract); the registry-write boot doesn't run shell commands, so
it writes no disk blocks. After the fixture exits, the boot phase verifies:
the store contains the fixture's final entry
(`Session/Current/Persisted` = `yes`), a save/read-back/decode round trip of
the VFS file preserves it, then the phase removes the entry and saves again,
asserting the store returns to the three seeded defaults. The boot therefore
leaves `/etc/config` present with default contents (documented; no disk
write without shell activity, so checkpoint bytes on disk are untouched).

## 11. Kernel changes

- `kernel/src/registry.rs`: `KeyError` gains `BadArgument` (87) and `NoSpace`
  (8); new `create(parent, subkey) -> Result<(u64, bool), KeyError>` (the
  bool is "created new"), `set_value(parent, name, value)` and
  `delete_value(parent, name)`; mutations call `config::set`/`config::remove`
  followed by `config::save()` (never inside the store lock). `open`, `value`
  and `close` are unchanged.
- `kernel/src/syscall.rs`: `SYS_REG_CREATE_KEY_EX = 18`,
  `SYS_REG_SET_VALUE_EX = 19`, `SYS_REG_DELETE_VALUE = 20`; three counters
  (`WIN32_REG_CREATE_KEY_CALLS`, `WIN32_REG_SET_VALUE_CALLS`,
  `WIN32_REG_DELETE_VALUE_CALLS`); `reg_error_label` gains
  `ERROR_NOT_ENOUGH_MEMORY` (8, "not enough memory"); the three handlers and
  dispatch arms; module doc updated (services 15–20, `Args9`).
- `kernel/src/pe_loader.rs`: `StubShape::Args9`; `WIN32_SERVICES` grows to
  eighteen entries; `STUB_STRIDE` 24 → 48 with the three new stub loads and
  the existing compile-time assertion; comments updated.
- `kernel/src/user.rs`: `PEREGW_PATH = "/boot/peregw.exe"`, `thread_peregw`,
  a registry-write phase after the registry-read phase (probe by module
  presence, 3 s window) including the store/save/decode assertions of
  section 10, skip lines when the module is absent, and the verdict line.
- Marker: unchanged (the write surface is part of the existing `win32`
  check; default ISOs don't carry `peregw.exe`, so every pinned marker,
  module count and serial assertion from earlier stages stays byte-identical
  for them).

## 12. Fixture (`userland/pe/peregw`) and call deltas

Crate mirrors `pereg`: `#![no_std] #![no_main]`, `raw-dylib` imports
(`advapi32.dll`: `RegCreateKeyExA`, `RegSetValueExA`, `RegDeleteValueA`,
`RegQueryValueExA`, `RegCloseKey`; `kernel32.dll`: `WriteFile`,
`ExitThread`), the `reloc_marker`/`marker_slot` relocation probe and
`/entry:_start` link. Sequence (`_start`, distinct exit codes 1–18):

1. `RegCreateKeyExA(HKLM, "Session/Current", 0, NULL, 0, 0, NULL, &k, &disp)`
   → 0, `disp == 1` (created new), `k == 0x1000` (exit 1).
2. `RegSetValueExA(k, "Flag", NULL, REG_SZ, "enabled\0", 8)` → 0 (exit 2).
3. The same create call again — an entry now lives under the prefix, so the
   children-only model reports the key as existing — → 0, `disp == 2`
   (opened existing), second handle `0x1001`; `RegCloseKey(k2)` → 0
   (exit 3). (Order matters: a repeat create *before* the first write would
   honestly report `created`, because no entry — no key — exists yet under
   the lazy model.)
4. `RegQueryValueExA(k, "Flag", NULL, &type, buf, &cb)` → 0, `cb == 8`,
   `type == 1`, bytes `enabled\0` (exit 4).
5. Update to `"on\0"`, `cbData = 3` → 0 (exit 5).
6. Size query → 0, `cb == 3` (exit 6).
7. `RegSetValueExA(k, "N", NULL, REG_DWORD, &dw, 4)` → 87 (exit 7).
8. `RegSetValueExA(k, "Flag", NULL, REG_SZ, NULL, 0)` → 87 (exit 8).
9. `RegSetValueExA(1, "X", NULL, REG_SZ, "y\0", 2)` → 6 (exit 9).
10. `RegSetValueExA(k, "Bad Name", NULL, REG_SZ, "z\0", 2)` → 87 (exit 10).
11. `RegSetValueExA(k, NULL, NULL, REG_SZ, "q\0", 2)` → 87 (exit 11).
12. `RegDeleteValueA(k, "Missing")` → 2 (exit 12).
13. `RegDeleteValueA(1, "Flag")` → 6 (exit 13).
14. `RegDeleteValueA(k, "Flag")` → 0, then size query `"Flag"` → 2
    (key still opens; its last value is gone) (exit 14).
15. `RegCreateKeyExA(HKLM, "Session/Current", ...)` again → 0,
    `disp == 1` (no children anymore), `k3 == 0x1001` (slot reuse) (exit 15).
16. `RegSetValueExA(k3, "Persisted", NULL, REG_SZ, "yes\0", 4)` → 0 (exit 16).
17. `RegCloseKey(k)` → 0; `RegCloseKey(k3)` → 0 (exit 17).
18. `WriteFile(1, "win32: registry write surface verified\n", 39, ...)` → 1,
    `written == 39` (exit 18); `ExitThread(0)`.

Call deltas for the phase: `RegCreateKeyExA +3`, `RegSetValueExA +8`,
`RegDeleteValueA +3`, `RegQueryValueExA +3`, `RegCloseKey +3`, `WriteFile
+1`, console writes `+1` / `+39` bytes, exit code 0, no user faults; plus the
store/save/decode assertions in `user.rs`. The fixture compares value bytes
itself, so counts and exit codes are byte-level evidence.

## 13. Script changes

- `scripts/build_user_pe.sh`: build/copy `peregw.exe`; check it with
  `check_user_pe.py --imports 'advapi32.dll:RegCloseKey,RegCreateKeyExA,RegDeleteValueA,RegOpenKeyExA,RegQueryValueExA,RegSetValueExA;kernel32.dll:ExitThread,WriteFile'`
  (the checker spec lists the full allowlisted set for the DLL group; the
  built image's descriptors are verified exactly) — nine artifacts become
  ten.
- `scripts/make_iso.sh`: new `--registry-write-fixture` flag appends
  `peregw.exe` to the PE module list and writes
  `artifacts/project-os-registry-write.iso` with `debug_exit` kept; the four
  variant flags (`--interactive`, `--input-fixture`, `--registry-fixture`,
  `--registry-write-fixture`) are mutually exclusive. Default and other
  variant outputs and module lists are unchanged.
- `scripts/test_registry_write_qemu.sh`: requires the kernel artifact, python
  and QEMU, builds the registry-write ISO with
  `--no-build --registry-write-fixture`, runs the python harness and logs
  PASS plus the session-log path.
- `scripts/test_registry_write_qemu.py`: starts QEMU (q35, 512 MiB,
  `isa-debug-exit`, TCP serial `wait=on`, no typing), waits for
  `win32: registry write surface verified`, then the boot marker (and its
  line terminator), and requires within the boot timeout: every pinned line
  from sections 5–10 (create 4096/4097 with `created`/`opened existing`,
  set success lines with byte counts, the 87/6/2 error lines, delete lines,
  `RegQueryValueExA` fallback to 2, slot reuse, the final
  `Session/Current/Persisted` write, `REG_SZ, 3 bytes`), the
  `win32 registry write selftest: passed (...)` line,
  `PROJECT_OS_BOOT_SUCCESS:...checks=...win32,vfs...`, no
  `PROJECT_OS_FATAL`/`PROJECT_OS_PANIC`, and QEMU exit 33. Repeated lines are
  counted. The transcript is written to
  `artifacts/qemu-registry-write-session.log`.
- `scripts/test_all.sh`: one new step `registry-write` after `registry`;
  expected summary becomes `PASS=25 FAIL=0 SKIP=4` while every existing step
  keeps its result.

## 14. Evidence and limits (claims this stage may and may not make)

Status: VERIFIED (2026-10-19). `scripts/test_registry_write_qemu.sh` → PASS;
session log `artifacts/qemu-registry-write-session.log` (10,407 bytes, sha256
`4c03490bed58031e65eea434a5ba4b0cb2698d49919fb1726af68535c7a16077`); registry
ISO `artifacts/project-os-registry-write.iso` (7,909,376 bytes, sha256
`527757b0529e5a52159090382201bdfe4b37f3661cb6dc6c930690b2e414f1aa`); fixture
`artifacts/peregw.exe` (5,632 bytes, sha256
`3b00b91a8677ce733f62af242809abe146005c853bb23e99d0c0d00135d5df01`); kernel
`artifacts/kernel.elf` (1,866,152 bytes, sha256
`dd468feec137d5e6fea0a8031aa28106e64cc2cd9f6fd16a709e2e44be38ac01`, build id
`0.1.0-fe3c278e`); `scripts/test_all.sh` → `PASS=25 FAIL=0 SKIP=4`. The
default ISO prints the registry skip lines and the `pe`/`input`/`registry`
regressions stayed PASS. One defect was found and fixed during bringing-up:
the fixture originally repeated the create *before* the first write, so the
lazy children-only model correctly reported `created` (not `opened
existing`); the fixture (and section 12) swapped steps 2 and 3, after which
the transcript matched line for line.

May claim (with the logs): a genuine PE32+ image mutates the config store
through native, allowlisted `advapi32.dll` write services (including the
first nine-argument stub), with create/open dispositions, REG_SZ sets with
update semantics, deletes, documented error values (2/4/6/8/87), the
read-back through `RegQueryValueExA`, persistence through the same
`/etc/config` codec the checkpoint carries, and the children-only key model
(a key vanishes when its last value is deleted). Deltas from Windows are
stated as in section 1.

May **not** claim: general registry compatibility, hives, security/access
rights, transactions, enumeration, key deletion APIs, typed values beyond
REG_SZ, empty key objects, default-value writes, or namespace rules such as
the direct-HKLM-child prohibition.

## 15. Verification plan (executed 2026-10-19)

1. `scripts/format.sh`, `scripts/lint.sh`: clean (fmt applied to one long
   line in `kernel/src/syscall.rs`).
2. `scripts/test_unit.sh`: 269/269 unchanged (no new `os_core` code).
3. `scripts/build_user_pe.sh`: ten artifacts incl. `peregw.exe` (5,632 B),
   each checker-verified; sizes and SHA-256 recorded; the nine existing
   fixtures kept their recorded hashes (`pereg.exe` `0ca37e2e…`,
   `peinput.exe` `a2ab21aa…`).
4. `scripts/make_iso.sh --registry-write-fixture`: registry-write ISO with
   eight PE modules (`peregw` added after pebadfn2); default ISO still seven.
5. `scripts/test_registry_write_qemu.sh`: PASS; session log recorded
   (section 14).
6. `scripts/test_registry_qemu.sh`, `scripts/test_input_qemu.sh` and
   `scripts/test_pe_qemu.sh`: PASS unchanged (regressions for stub stride,
   allowlist, default and registry ISOs).
7. `scripts/test_all.sh`: `PASS=25 FAIL=0 SKIP=4`; exact hashes recorded.

## 16. Documentation and evidence updates

`docs/RESEARCH.md` R-018 (experiment/conclusion after the run), ADR-0025,
`docs/ARCHITECTURE.md` (syscall services 15–20, `Args9` stride, write-phase
wording), `docs/TESTING.md` (registry-write step, fixture, counts),
`docs/ROADMAP.md` (Phase 8 stage 4 complete), `docs/WINDOWS_COMPATIBILITY.md`
(registry write surface and remaining gaps), `docs/PE_LOADER.md` (allowlist,
stub shapes), `progress.md` (commands, logs, hashes, defects).
