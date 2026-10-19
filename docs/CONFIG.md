# Native registry-like configuration store (Phase 8, stage 1)

Status: approved and **implemented and verified** 2026-10-19 (this document
is the contract for the stage; evidence in §8). Decision record: ADR-0022.

## 1. Goal and non-goals

Goal: a small, persistent, hierarchical key/value configuration store served by
the kernel, usable from the shell and (a later stage) from user programs. It is
"registry-like" in shape — path-shaped keys, string values, per-key overwrite,
prefix listing — and it survives reboots through the existing checkpoint
persistence (docs/PERSISTENCE.md) **without any checkpoint-format change**: the
store is serialized by a host-tested codec into one ordinary VFS file,
`/etc/config` (≤ `MAX_BLOB_BYTES`), which the existing whole-tree snapshot
already carries, verifies (CRC) and falls back for.

Non-goals (explicitly not claimed by this stage):

- No Windows registry compatibility: no `HKEY_*` roots, no hives, no
  `advapi32` (`RegOpenKeyEx` etc.) surface, no typed values (DWORD/binary/
  multi-string) — values are printable-ASCII strings with defined limits.
- No security model (no ACLs, no per-user hives), no transactions, no change
  notifications, no case folding (keys are case-sensitive byte strings).
- No environment-variable mapping, no per-application virtualization, no
  import/export tooling beyond the shell commands in §5.
- Not a general database: fixed caps in §3 apply, and a configuration that
  outgrows them is rejected cleanly (never truncated).

## 2. On-disk format — `POCF` version 1

The store is serialized into the contents of the VFS file `/etc/config`.
Little-endian throughout.

```text
offset 0   magic   "POCF" (50 4F 43 46)
offset 4   version u16 = 1
offset 6   count   u16, 0..=64 entries
then `count` entries:
    key_len   u16, 1..=96
    value_len u16, 0..=128
    key       key_len bytes (ASCII, §3 rules)
    value     value_len bytes (printable ASCII 0x20..=0x7E)
```

- `encode` writes entries sorted by key (byte order), so an unchanged store
  serializes to identical bytes.
- `decode` accepts any entry order, rejects duplicate keys (`duplicate key`),
  and enforces every cap plus `len ≤ MAX_BLOB_BYTES` (`config too large`).
- The header is eight bytes; an empty store (count 0) is valid and serialises
  to an 8-byte file.
- Unknown/mismatched version → `bad version` (no migration in this stage; the
  next format change must bump `VERSION` and update this section).

## 3. `os_core::config` (host-tested)

Constants:

```rust
pub const MAGIC: [u8; 4] = *b"POCF";
pub const VERSION: u16 = 1;
pub const MAX_ENTRIES: usize = 64;
pub const MAX_KEY_BYTES: usize = 96;    // whole key incl. '/'
pub const MAX_VALUE_BYTES: usize = 128;
pub const MAX_BLOB_BYTES: usize = 7984; // 8192-byte VFS file cap minus headroom
pub const MAX_COMPONENT: usize = 32;    // one key segment
```

Key rules (`key_is_valid`): 1..=96 bytes; ASCII `[A-Za-z0-9_.-]` plus `/` as
separator; at least one component; no leading or trailing `/`; no empty
components (`//`); each component 1..=32 bytes. Examples: `System/Name` ✓,
`Display/Theme` ✓, `/bad` ✗, `bad/` ✗, `bad//key` ✗, `bad key` ✗.

Value rules: 0..=128 bytes, each byte 0x20..=0x7E (empty value allowed).

API:

```rust
pub struct Entry { pub key: String, pub value: String }

impl Config {
    pub fn new() -> Self;                          // empty store
    pub fn len(&self) -> usize;
    pub fn is_empty(&self) -> bool;
    pub fn get(&self, key: &str) -> Option<&str>;
    pub fn set(&mut self, key: &str, value: &str) -> Result<(), ConfigError>;
    pub fn remove(&mut self, key: &str) -> Result<(), ConfigError>;
    pub fn list(&self, prefix: Option<&str>) -> Vec<(&str, &str)>; // sorted
    pub fn encode(&self) -> Vec<u8>;
    pub fn decode(bytes: &[u8]) -> Result<Config, ConfigError>;
    pub fn key_is_valid(key: &str) -> bool;
}
```

`set` overwrites an existing key or inserts, rejecting `Full` at 64 entries and
`TooLarge` when the resulting blob would exceed `MAX_BLOB_BYTES` (the store is
never partially written). `remove` rejects `NoSuchKey`. `list(None)` returns all
entries; `list(Some(p))` returns keys equal to `p` or starting with `p + "/"`,
sorted.

Errors (`ConfigError`, with `label()` strings used verbatim in shell output):
`BadMagic` "bad magic", `BadVersion` "bad version", `Truncated` "truncated",
`TrailingBytes` "trailing bytes", `BadEntryCount` "bad entry count", `TooLarge`
"config too large", `BadKey` "bad key", `BadValue` "bad value", `DuplicateKey`
"duplicate key", `Full` "entry table full", `NoSuchKey` "no such key".

Host tests (new module `#[cfg(test)]`): validation table, set/get/overwrite,
insert-sorted, remove, list prefix/case, all caps (entry count, key length,
component length, value length, blob size), encode determinism, round trip,
and decode rejections for every error row above.

## 4. Kernel module (`kernel/src/config.rs`)

State: `static STORE: IrqSpinLock<Option<Config>>` (mirrors `vfs::with`).

Defaults (seeded in memory when no file exists; never written at boot):

| Key | Value |
|---|---|
| `System/Name` | `PROJECT_OS` |
| `Display/Theme` | `classic` |
| `Net/Mode` | `static` |

Functions:

- `pub fn init() -> Result<(), &'static str>` — called from `main.rs` after
  `persist::init()`; if `/etc/config` exists it is read and `decode`d (a
  malformed file is a boot-fatal `Err` with the codec label), otherwise the
  defaults are seeded. **Boot never writes**: the file is created lazily by the
  first mutation (§5).
- `pub fn with<R>(op: impl FnOnce(&mut Config) -> R) -> Result<R, &'static str>`
  — `Err("config store is not initialized")` before `init()`.
- `pub fn save() -> Result<(), &'static str>` — `encode` the current store and
  write `/etc/config` through `vfs::with` (`write_file`, falling back to
  `create_file` when absent). Writing marks the VFS dirty, so the existing
  `persist::flush_if_dirty()` boundary save carries the file to disk.
- `pub fn self_test() -> Result<(usize, usize), &'static str>` — runs after
  `init()`: reads the entry count, inserts `SelfCheck/Value = ok`, verifies
  `get`, verifies `list(Some("SelfCheck"))` contains it, removes it, verifies
  the count is restored and an invalid key (`bad//key`) is rejected. Returns
  `(entries, encoded_bytes)` of the current store. It mutates only memory —
  no file write, no VFS dirty bit — so a boot with no shell activity still
  leaves the checkpoint byte-identical.

Boot output (pinned):

```text
config selftest: passed ({n} entries, {b} bytes; get/set/remove/list/encode/decode/error paths)
```

and the marker gains `,config` inserted directly after `,vfs`, so the
no-disk marker is `checks=...,elf,pe,win32,vfs,config` and the disk marker is
`checks=...,vfs,config,blk,persist` (net appends `,net` last, unchanged).

## 5. Shell commands

`config` is one command with a sub-operation argument, dispatched in
`kernel/src/shell.rs` exactly like the other arms (no command table):

```text
project-os> config
config: {n} entries (/etc/config, {b} bytes)

project-os> config get {key}
{key} = {value}                    (or: error: no such key)

project-os> config set {key} {value}
config set: ok                     (persists; prints the standard saved-slot line on a disk boot)

project-os> config del {key}
config del: ok                     (persists like set)

project-os> config list
{n} entries in config:
  {key} = {value}                  (sorted)

project-os> config list {prefix}
{n} entries matching {prefix}:
  {key} = {value}
```

Malformed keys/values surface as `error: bad key` / `error: bad value` through
the existing `run()` error wrapper; missing arguments print
`usage: config get <key>` / `usage: config set <key> <value>` /
`usage: config del <key>`. Unknown sub-operations:
`usage: config [get|set|del|list] [key] [value]`. `help` gains one line:
`  config [get|set|del|list]  registry-like configuration store`.

## 6. Persistence integration

- `/etc/config` is an ordinary VFS file (≤ 7984 bytes, inside the 8192-byte
  file cap), so the checkpoint payload format, CRC scheme, slot rotation and
  fallback rules are untouched (PERSISTENCE.md §4 remains accurate; a note is
  added that `/etc/config` is a normal file).
- Durability is therefore the existing whole-tree durability: `config set`/
  `config del` dirty the VFS, the shell boundary flushes one snapshot to the
  inactive slot with `seq + 1`, and a boot loads the highest-valid-seq slot.
- Fallback semantics apply to the store like any file: if the newest slot is
  corrupt, the older snapshot's configuration is restored (observable in the
  four-boot harness, §7). `rm /etc/config` via the shell removes the store
  from the persisted tree; the next boot reseeds defaults in memory.
- No new counters, no new syscalls, no Win32 services in this stage.

## 7. Script changes

- `scripts/test_shell_qemu.py` `COMMANDS` grows by eight entries after
  `cat /etc/motd` (state in memory only — this session has no disk):
  `config` (`3 entries`, `/etc/config`), `config get System/Name`
  (`System/Name = PROJECT_OS`), `config set Display/Theme light`
  (`config set: ok`), `config get Display/Theme` (`Display/Theme = light`),
  `config list` (`3 entries in config:`, `Display/Theme = light`),
  `config del Display/Theme` (`config del: ok`), `config get Display/Theme`
  (`no such key`), `config set bad//key x` (`error: bad key`). Session total
  becomes 25 commands.
- `scripts/test_persistence_qemu.py`: `MARKER_SUFFIX` becomes
  `vfs,config,blk,persist`; boot 1 ends with `config set Display/Theme light`
  (saved slot A seq=2); boot 2 loads slot A seq=2, verifies the light theme
  survived, then `mkdir` → B seq=3, `write /tmp/notes/a.txt alpha` → A seq=4,
  and finally `config set Display/Theme dark` → B seq=5; boot 3 corrupts the
  newest slot B, expects the fallback load `slot A seq=4` (theme is `light`
  again — the dark change existed only in the corrupt slot — while
  `/tmp/notes/a.txt` is present); boot 4 corrupts both slots, formats fresh,
  and verifies defaults (`Display/Theme = classic`) and an empty
  `/tmp/hello.txt`.
- `scripts/test_elf_qemu.sh` / `test_pe_qemu.sh` marker substrings
  (`sched,user,elf,pe,win32,vfs`) stay valid because `,config` is appended
  after `,vfs`.

## 8. Evidence and limits (claims this stage may and may not make)

Status (2026-10-19): **implemented and verified** (build `0.1.0-82bc098b`,
QEMU 11.1.0). `scripts/test_unit.sh`: 269/269 host tests including the 17 new
`os_core::config` tests listed in §3. `scripts/test_shell_qemu.sh`: PASS, 25
commands (the §7 list). `scripts/test_persistence_qemu.sh`: PASS, four boots
(the §7 sequence; theme rollback on fallback and defaults-after-format
observed). Boot line: `config selftest: passed (3 entries, 75 bytes;
get/set/remove/list/encode/decode/error paths)`; no-disk marker
`PROJECT_OS_BOOT_SUCCESS:0.1.0-82bc098b
checks=...,sched,user,elf,pe,win32,vfs,config`; disk marker adds
`,blk,persist,net`. Full `scripts/test_all.sh`: `PASS=22 FAIL=0 SKIP=4`.
Exact hashes and log excerpts: ADR-0022 and `progress.md`.

May claim (after tests pass, with logs recorded): a persistent hierarchical
key/value store exists in the kernel with a host-tested, versioned on-disk
format; it survives reboots, participates in checkpoint fallback (newest-slot
corruption rolls the store back with the rest of the tree), and is reachable
from the shell with defined limits and clean error labels.

May **not** claim: Windows registry compatibility or any `advapi32`/`Reg*`
API, typed values, security/ACL semantics, per-user separation, transactional
updates, notifications, or any Win32 access path (a later stage). The caps in
§3 are hard limits; the store is one file in one VFS, so its size cannot
exceed 7984 serialized bytes and 64 entries.

## 9. Documentation and evidence updates (on implementation)

- `docs/CONFIG.md`: this section's status line updated with observed output.
- `docs/DECISIONS.md`: ADR-0022 (plain VFS file + `POCF` codec; no checkpoint
  format change; lazy file creation; defaults seeded in memory) with
  alternatives rejected.
- `docs/PERSISTENCE.md`: note that `/etc/config` is an ordinary VFS file and
  the store's rollback semantics follow from the existing fallback rule.
- `docs/ARCHITECTURE.md`: `os_core::config` and `kernel/src/config.rs` rows;
  shell row gains the `config` builtin.
- `docs/TESTING.md`: unit-test count; shell row (25 commands); persistence row
  (config assertions); marker documentation (`...,vfs,config` before
  `,blk`,`,persist`,`,net`); a Configuration store details section.
- `docs/ROADMAP.md`: Phase 8 stage 1 entry; overview row updated to "partial
  (stage 1: native config store)".
- `docs/WINDOWS_COMPATIBILITY.md`: "No registry-like configuration store" gap
  updated — native store exists, no Windows registry API.
- `progress.md`: milestone entry, exact commands, artifact table additions,
  serial excerpts, defects, next action.
