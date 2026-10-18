# Durable checkpoint VFS (Phase 6, stage 2) — design spec

Status: **approved (2026-10-17) and implemented the same day.** This document
is the contract for the stage: code, tests and logs must match it, and any
deviation must be recorded here or in `docs/DECISIONS.md`. Implementation
record: ADR-0016, `scripts/test_persistence_qemu.sh` (four boots, all phases
passed), `progress.md`.

## 1. Goal and non-goals

Goal: the in-memory VFS gains a durable, crash-safe **checkpoint image** on an
ATA disk, loaded at boot and rewritten write-through on mutation, with a
scripted two-boot (plus corruption-fallback) test proving reboot persistence.

This is deliberately **not** a general-purpose filesystem:

- no per-file on-disk allocation, no renames, no directory-order guarantees
  beyond arena index order;
- raw whole-disk image only — no MBR, no partition table, no FAT/ext2
  interoperability, no on-disk booting from this image;
- primary channel, master drive, LBA28 PIO polling only — no DMA, no IRQ14, no
  ATAPI, no AHCI (needs PCI BAR discovery, Phase 9), no virtio (needs PCI);
- CRC32 is integrity, not security (no encryption, no cryptographic MAC);
- single-writer; no concurrent flush; last-writer-wins across boots.

## 2. Why ATA PIO and which machine (research R-009)

Without PCI enumeration the kernel cannot locate AHCI/virtio BARs. QEMU's
`q35` storage is ICH9 **AHCI** only (`hw/ide/ahci.c`, `hw/ide/ich.c` register
no legacy ports; drives attach through `ich9-ahci`). The legacy 0x1F0/0x3F6
port pair is registered by the PIIX southbridge (`hw/ide/piix.c:131`) on the
i440fx `pc` machine. Therefore:

- the kernel's block layer targets classic ATA PIO registers and probes them;
- the persistence harness runs `-machine pc` **for this test only**; all other
  QEMU tests keep `-machine q35`;
- on machines without those ports the driver reports "no disk" and every other
  boot path is unchanged.

First implementation step is an empirical probe of this claim (see §9, boot 1
asserts `blk: found ...`); if it fails, work stops and the design is revisited
(candidate fallbacks: minimal PCI + AHCI, or virtio — both scoped as new work).

## 3. Block layer — `kernel/src/blk.rs` (new)

Primary IDE channel, master drive:

| Register | Port | Use |
|---|---|---|
| Data | 0x1F0 | 16-bit PIO data |
| Error / Features | 0x1F1 | error bits after failure |
| Sector count | 0x1F2 | 1..=255 sectors per command |
| LBA low / mid / high | 0x1F3 / 0x1F4 / 0x1F5 | LBA28 bits 0..7 / 8..15 / 16..23 |
| Drive / head | 0x1F6 | `0xE0 \| ((lba >> 24) & 0x0F)` for LBA28 master |
| Status / Command | 0x1F7 | bit7 BSY, bit6 DF, bit5 DSC, bit3 DRQ, bit0 ERR |
| Alt status / control | 0x3F6 | polled status, no IRQ side effects |

Protocol (polling, IRQ14 unused):

- **IDENTIFY (0xEC)**: select `0xA0`, zero count/LBA ports, issue command; a
  zero status means no device. Poll BSY clear; ERR without DRQ means
  ATAPI/unsupported (logged, treated as no usable disk). Read 256 words;
  sector count = `words[60] | (words[61] << 16)`; model = byteswapped ASCII
  `words[27..=46]`, trimmed.
- **READ (0x20) / WRITE (0x30)**: split the request into chunks of ≤255
  sectors; per chunk write drive/LBA/count, issue the command, then per sector
  poll BSY=0 and DRQ=1 (ERR/DF → `DeviceError(status)`), then move 256 words
  (`inw`/`outw` on 0x1F0).
- **FLUSH (0xE7)**: issue, poll BSY clear. Called after every slot write.
- 400 ns settle delay after drive select and command: four reads of 0x3F6.
- Every wait has a deadline of 5 s (500 PIT ticks at 100 Hz); expiry returns
  `Timeout`, increments the error counter and aborts the command. No
  automatic retry in this milestone.

API (exact names may follow implementation style, behavior is pinned):

```rust
pub struct BlkInfo { pub model: [u8; 40], pub model_len: usize, pub sectors: u32 }
pub struct BlkStats { pub sectors_read: u32, pub sectors_written: u32, pub flushes: u32, pub errors: u32 }
pub enum BlkError { NoDevice, Timeout, DeviceError(u8), BadRange }

pub fn init() -> Option<BlkInfo>;            // probes once at boot; logs result
pub fn info() -> Option<BlkInfo>;            // for the shell `blk` builtin
pub fn stats() -> BlkStats;
pub fn read_sectors(lba: u32, buf: &mut [u8]) -> Result<(), BlkError>;
pub fn write_sectors(lba: u32, buf: &[u8]) -> Result<(), BlkError>;
pub fn flush() -> Result<(), BlkError>;
```

Constraints: `buf.len()` is a non-zero multiple of 512; `lba + sectors <=
0x0FFF_FFFF` (LBA28) or `BadRange`.

Boot log lines (pinned for tests):

```text
blk: found QEMU HARDDISK, 16384 sectors          # model text may vary; format fixed
blk: no ATA device on primary channel
blk: ATAPI device on primary master (unsupported)
```

## 4. Image format — `os_core::pofs` (new, host-tested) + slots

Disk image (`artifacts/persist-disk.img` in tests): exactly 8 MiB (16,384
sectors of 512 B), all-zero except the slots we write.

| Region | LBA | Bytes |
|---|---|---|
| Reserved (left zero; no MBR is written) | 0..16 | 8 KiB |
| Slot A | 16..2064 | 1,048,576 |
| Slot B | 2064..4112 | 1,048,576 |
| Unused headroom | 4112..16384 | ~5.9 MiB |

Slot header (little-endian, at slot offset 0; bytes 28..512 zero, ignored on
read):

| Offset | Size | Field |
|---|---|---|
| 0 | 4 | magic `"POFS"` (0x50 0x4F 0x46 0x53) |
| 4 | 2 | version = 1 |
| 6 | 2 | reserved = 0 |
| 8 | 8 | seq (u64) |
| 16 | 4 | payload_len (≤ 1,048,064 = slot − header) |
| 20 | 4 | payload_crc32 |
| 24 | 4 | header_crc32 = CRC32(bytes 0..24) |

Payload (starts at slot offset 512):

```text
u32 node_count                       (1..=64)
per node, in arena index order:
    u8  kind                         (0 = directory, 1 = file)
    u32 parent                       (0xFFFF_FFFF = none; only node 0)
    u16 name_len                     (0 for node 0, else 1..=32)
    u8  name[name_len]
    u32 data_len                     (0 for directories, 0..=8192 for files)
    u8  data[data_len]
```

CRC32: reflected IEEE 802.3 polynomial 0xEDB88320, init/final `0xFFFF_FFFF`,
bitwise implementation (no table), documented with vectors in host tests:
`crc32(b"") = 0`, `crc32(b"123456789") = 0xCBF43926`, `crc32(b"a") = 0xE8B7BE43`.

Encode walks the tree in depth-first pre-order from the root (children in list
order) and remaps arena indices to stream indices, so every parent precedes
its children in the stream (`remove()` uses `swap_remove`, which does **not**
preserve arena index order). Decode rebuilds the arena in stream order, so
listing order is preserved and no pointer fixups are needed.
Worst-case payload: 64 × (1+4+2+32+4+8192) + 4 ≈ 527 KB, well under the 1 MiB
slot.

Decode validation (each failure has a label used in logs):

1. `payload_len` in 4..=1,048,064 (`0..=3` → `truncated`, above the bound →
   `payload too large`);
2. `node_count` in 1..=64 → `bad node count`;
3. a stream that runs out of bytes → `truncated`;
4. the stream is consumed exactly → `trailing bytes`;
5. kind ∈ {0,1} → `bad kind`;
6. node 0 is a nameless, parentless, empty directory → `bad root`;
7. i>0: `parent < i` → `bad parent`;
8. `name_len` 1..=32 and printable ASCII without `/` per `os_core::path`
   component rules → `bad name`;
9. directories carry zero data, files ≤ 8192 B → `bad data length`;
10. no duplicate sibling names → `duplicate name` (checked via pairwise
    full-path comparison, ≤64 nodes);
11. every reconstructed full path (walk parents, join with `/`) ≤128 bytes and
    passes `path::validate` → `bad path`.

`os_core` gains `extern crate alloc` for this module only (`Vec`/`String`);
the kernel global allocator and the host test harness both provide it.

## 5. Slot protocol — `kernel/src/persist.rs` (new)

State: `active_slot`, `seq`, counters (`saves`, `loads`, `errors`,
`last_load`), never touched when there is no usable disk.

**Format (first boot, or both slots invalid):** encode the current tree
(post boot self-test), write it to slot A with `seq = 0`, cache-flush, read the
header back and compare — log `persist: no valid checkpoint, formatted slot A
seq=0`. Save capability is enabled immediately after.

**Load (every boot with a usable disk):** read both headers; for each, check
magic → version → reserved → header CRC → payload bound → read payload →
payload CRC → decode. A header that is entirely zero means "empty slot" and is
not an error (fresh disk); any other failure logs `persist: slot {A|B} invalid
(reason)` with the first failing label from §4. Valid candidates are ranked by
`seq`; the winner replaces the seeded/self-tested tree (`vfs::replace`), logs
`persist: loaded slot {A|B} seq={n} ({nodes} nodes, {bytes} bytes, crc ok)`,
and becomes active; the loser remains the fallback copy.

**Save (write-through):** invoked after each shell command when the VFS dirty
flag is set and a disk is active. Encode the tree, write header+payload to the
**inactive** slot with `seq = previous_best + 1`, cache-flush, read the header
back and compare; on success swap active slot and log
`persist: saved slot {A|B} seq={n} ({nodes} nodes, {bytes} bytes)`; on failure
increment `errors`, log
`persist: save failed (reason); previous checkpoint kept` and leave the
previous slot active (the failed write targeted the inactive slot, so the good
copy is untouched).

**Interrupted write:** a kill during a save can only damage the inactive slot;
the next boot's CRC check rejects it and the older slot wins. There is no
journal — double buffering gives single-write atomicity only.

**Unusable disk:** no device, ATAPI, or `sectors < 4112` →
`persist: inactive (no ATA disk)` / `persist: disk too small ({n} sectors,
need 4112)`; no writes ever happen.

## 6. VFS, shell and boot integration

`kernel/src/vfs.rs` additions:

- `export(&self) -> Vec<pofs::NodeRecord>` in arena index order;
- `replace(&mut self, records: &[pofs::NodeRecord]) -> Result<(), VfsError>`
  rebuilding the arena (re-checks count ≤64, file ≤8192, component syntax);
- a module-level dirty `AtomicBool`: set by every successful mutation
  (`create_file`, `write_file`, `append_file`, `mkdir`, `remove`), cleared by
  `replace` and by an explicit `clear_dirty()` after a successful save.

`kernel/src/shell.rs`:

- new builtins:
  - `blk` → `blk: {model}, {sectors} sectors ({MiB} MiB), lba28 pio; reads
    {n} sectors, writes {n} sectors, flushes {n}, errors {n}` (when inactive:
    `blk: no ATA device on primary channel`);
  - `persist` → `persist: active slot {A|B} seq {n}; saves {n}, loads {n},
    last load {A|B} seq {n}, errors {n}` (or `persist: inactive (no ATA
    disk)`);
- after each executed command, `persist::flush_if_dirty()`;
- `version` line changes from `VFS: in-memory only (no persistence yet)` to
  `VFS: checkpoint image on ATA disk when present` — the scripted shell test's
  assertion is updated in the same change;
- `help` lists `blk` and `persist`.

`kernel/src/main.rs` boot order: the VFS boot self-test runs early and its
mutations therefore never reach the disk; after the PIT is verified (so poll
deadlines are real time) `blk::init()` then `persist::init()` run, before the
success marker and the shell.

Marker: with a usable disk the checks string gains `,blk,persist`. Correction
note (stage 3, ADR-0017): the no-disk marker now reads `...,user,elf,vfs` — the
byte-identical wording written at stage 2 applies to the `vfs`-onward suffix
pinned by `test_persistence_qemu.sh`, not to the full no-disk line. Disk boot:

```text
PROJECT_OS_BOOT_SUCCESS:<id> checks=handoff,memmap,gdt,idt,exceptions,timer,allocator,heap,vm,sched,user,elf,vfs,blk,persist
```

## 7. Test plan

**Host (`cargo test -p os_core`)** — new `pofs` tests: CRC32 vectors; round-trip
of 1..=64-node trees including a worst-case 64×8192 payload; corruption corpus:
flipped header byte, flipped payload byte, truncated payload, trailing bytes,
zero/bad node count, bad kind, bad parent, bad root, bad name (empty, too long,
bad chars), directory with data, file over 8192, duplicate sibling names, bad
path (too deep/long); empty payload.

**In-VM boot without disk** (existing `test_boot_qemu.sh`, `test_memory.sh`,
`test_vfs.sh`, `test_shell_qemu.sh`): unchanged marker, plus
`blk: no ATA device on primary channel` / `persist: inactive (no ATA disk)`
lines appear before the marker.

**Integration — `scripts/test_persistence_qemu.sh` +
`scripts/test_persistence_qemu.py`** (new; QEMU 11.1.0, `-machine pc -m 512M
-smp 1`, interactive ISO, paced TCP serial as in stage 1):

1. Fresh 8 MiB zeroed image (both slots empty). Boot 1: assert `blk: found` +
   `16384 sectors`, `persist: no valid checkpoint, formatted slot A seq=0`,
   **no** `invalid` line in the preamble, marker ends `vfs,blk,persist`;
   builtins `blk` and `persist` report the device and `active slot A seq 0`;
   `write /tmp/hello.txt hello world` → `persist: saved slot B seq=1`;
   `cat /tmp/hello.txt` → `hello world`; kill QEMU.
2. Boot 2: `persist: loaded slot B seq=1`; `cat /tmp/hello.txt` → `hello
   world`; `mkdir /tmp/notes` → `persist: saved slot A seq=2`; `write
   /tmp/notes/a.txt alpha` → `persist: saved slot B seq=3`; kill.
3. Harness parses both slot headers, picks the valid slot with the higher seq
   (B, seq 3) and zeroes 512 B at that slot's payload start (header intact,
   payload damaged). Boot 3: `persist: slot B invalid (payload crc mismatch)`
   then `persist: loaded slot A seq=2`; `cat /tmp/hello.txt` → `hello world`;
   `cat /tmp/notes/a.txt` → `no such file or directory` (the older checkpoint
   wins); kill.
4. Harness repeats the payload corruption on both slots. Boot 4: both invalid
   lines (`payload crc mismatch`), `persist: no valid checkpoint, formatted
   slot A seq=0`; `cat /tmp/hello.txt` → `no such file or directory`; marker
   ok; kill.

Exit: 0 = pass, 1 = assertion failure (phase printed), 2 = SKIP (missing
QEMU/python/ISO). The shared session/pacing logic is extracted into
`scripts/qemu_serial.py`; `scripts/test_shell_qemu.py` keeps its CLI and
17-command output byte-for-byte, proving the refactor by rerun.

**Full pipeline:** `scripts/test_all.sh` gains a `persistence` step after
`shell`; a skipped step is never a pass. Budget: four boots ≈ 40–60 s in
addition to the existing run.

## 8. Honest limits (documented, not hidden)

- PIIX legacy IDE, primary master, LBA28 (images up to 128 GiB), PIO polling
  only; q35/AHCI, virtio, DMA, IRQ14 and real hardware are untested and
  unimplemented (AHCI is Phase 9 driver work).
- Whole-tree snapshot per mutation; no per-file allocation, no journal, no
  metadata beyond the tree; CRC32 integrity only; no encryption.
- Save happens at shell-command boundaries: a kill mid-command loses that
  command's unflushed mutations but never the previous checkpoint.
- Transient heap cost per save ≈ 1.1 MiB (records + encoded image) against the
  fixed 8 MiB heap.
- Evidence is QEMU `pc` only this stage; VirtualBox with a disk remains
  untested (the existing VirtualBox test attaches no disk).
- Performance is not measured beyond observing that a full-image rewrite is
  acceptable at this data size; no claims relative to other systems.

## 9. Implementation checklist (order)

1. `os_core/src/pofs.rs` + export in `os_core/src/lib.rs` + host tests.
2. `scripts/qemu_serial.py` refactor; rerun `test_shell_qemu.sh` green.
3. `kernel/src/blk.rs` + boot probe; no-disk boots still pass.
4. `kernel/src/vfs.rs` export/replace/dirty flag.
5. `kernel/src/persist.rs`; `main.rs` wiring + marker; `shell.rs` builtins +
   flush + version/help text.
6. `scripts/test_persistence_qemu.{sh,py}`; run end-to-end.
7. `test_all.sh` step; docs (`TESTING.md`, `ROADMAP.md`, `ARCHITECTURE.md`,
   `DECISIONS.md` ADR-0016, `RESEARCH.md` R-009 conclusion, `progress.md` with
   commands, logs and hashes); full `test_all.sh` run.
8. Update `progress.md` open-defects list (remove "no persistence", add the
   new limits verbatim from §8).
