# Progress

Last updated: **2026-10-18**

## Current milestone

Phase 0 complete; Phase 1 boot milestone implemented and verified under QEMU
(BIOS, three RAM sizes) **and VirtualBox (BIOS)**. Phase 2 and Phase 3 have
working foundations (descriptor tables, exceptions, legacy timer, physical frame
allocator, narrow MMIO page-table mapping). Phase 3's central deliverable — the
kernel's **own page-table manager** — Phase 4's **kernel heap with global
allocator**, Phase 5a's **kernel threads with a timer-preempted round-robin
scheduler**, Phase 5b's **ring-3 user mode with an `int 0x80` syscall
boundary and user-fault isolation**, and **all five Phase 6 stages** — the
**serial console, in-memory VFS, kernel shell**, **durable checkpoint
persistence over ATA PIO**, the **native ELF64 program loader** (three Limine
modules built from our own `userland/` workspace, parsed by a host-tested
`os_core::elf` parser and executed at CPL3), the **framebuffer log console**
(the serial log mirrored to the Limine framebuffer through host-tested
grid/renderer modules, with a boot-time readback self-test and a validated
screendump) and the **e1000 network stack** (a polled 82540EM driver over a
minimal PCI scan, host-tested ARP/IPv4/ICMP/UDP codecs, static `10.0.2.15/24`,
and a scripted QEMU slirp round trip proving ARP, three ICMP replies and a
UDP echo) — are implemented and verified in QEMU/VirtualBox BIOS boots.
Phase 6 scope is complete for that environment. **Phase 7 stage 1** — a native
PE32+ loader — is implemented and verified: a host-tested `os_core::pe` parser,
a kernel loader (`kernel/src/pe_loader.rs`) that maps W^X sections and applies
DIR64 base relocations under the fixed `0x400000` window, and a genuine PE32+
fixture produced by `rustc` + `rust-lld` (no MSVC/MinGW/Windows SDK) executing
at CPL3 via `int 0x80`; a corrupt PE module is rejected without harming boot.
**Phase 7 stage 2** — a native Win32 console surface — is implemented and
verified: a `raw-dylib` import directory in a new fixture (`peconsole.exe`) is
parsed by `os_core::pe`, resolved against an exact-name allowlist of three
`kernel32.dll`-shaped services (`GetStdHandle`, `WriteConsoleA`, `ExitProcess`)
through a kernel stub page and `int 0x80` dispatch, with the IAT patched via
the HHDM before protection; two negative mutants (`pebadimp.exe`,
`pebadfn.exe`) are rejected before any allocation. The remaining Phase 7 scope
(broader API surface, imports from real system DLLs, ordinals, forwarders,
delay imports, SEH/unwind, TLS, CLR, PE32) is not started; there is still no
Windows application compatibility.

This is **not** a Windows-compatible OS, not a desktop, and not a GPU-capable OS.

## Repository state

- Workspace root: this thread's local checkout.
- Git: branch `main`; eight commits before this progress edit (`77bb56e
  phase1`, `0cc9372 heap, kernel-owned page tables, threads, ring 3`,
  `43fdd7b phase6: serial console, vfs, shell, ata checkpoint persistence,
  ring-3 elf64 module loader`, `f3b17a3 progress: record phase6 commit and
  push`, `fb2da14 phase6: framebuffer log console with readback selftest and
  screendump test`, `5467bde progress: record phase6 stage 4 commit and push`,
  `b257a1e phase6: e1000 networking with arp/icmp/udp and qemu slirp
  round-trip test`, `25606ad progress: record phase6 stage 5 commit and push`).
  `b257a1e..25606ad` is pushed to `orgin/main` (github.com/Alex07lol/WinUx).
  The Phase 7 stage-1 commit is `b37b371` (`phase7: pe32+ loader, dir64 base
  relocations, rust-lld fixture at cpl3`), pushed (`25606ad..b37b371`) together
  with its progress follow-up `b8a2910` (`progress: record phase7 stage 1
  commit and push`). The Phase 7 stage-2 commit is `b19670f` (`phase7: native
  win32 console surface (raw-dylib imports, stub page, three kernel32
  services)`), pushed (`b8a2910..b19670f`); this progress edit is its
  follow-up commit, same pattern as `f3b17a3`, `5467bde` and `25606ad`.
  Nothing was reset or discarded.
- Generated inputs (`target/`, `third_party/`, `boot/limine/`, `artifacts/*`) are
  Git-ignored.
- Case-insensitive host: `agents.md`, `STOP.md` and `PROGRESS.md` resolve to
  `AGENTS.md`, `stop.md` and `progress.md`; there are no divergent duplicates.
- `stop.md` currently says `STOP=NO`.
- VirtualBox 7.2.20 with `VBoxManage` is installed on this host for the
  isolated boot test; no host driver or partition changes were made.

## Implemented in this session

- Rust workspace (`kernel`, `os_core`, `tools/membench`) with a pinned
  `x86_64-unknown-none` target and static-relocation linker flags.
- Limine protocol base revision 6 request block (bootloader info, firmware type,
  stack size, paging mode, HHDM, cmdline, executable address, memory map,
  framebuffer, TSC frequency) in a writable `.limine_requests` section.
- Assembly entry stub, COM1 serial logging, panic handler.
- Boot dashboard on the Limine framebuffer with an original 5x7 font.
- Memory-map copy/validation and accounting, with reserved-region safety.
- Physical frame allocator (bitmap, usable RAM only, low-1 MiB guard, contiguous
  power-of-two blocks, strict free validation) plus in-kernel HHDM self-test.
- GDT/TSS/IST1, 32 exception gates, Rust frame dispatcher, deliberate `int3`
  self-test, fatal exception diagnostics.
- 8259 remap/mask, 8254 PIT at 100 Hz (mode 2), IRQ0 dispatch with 5-tick
  verification.
- Narrow MMIO page-table mapping (`arch/x86_64/paging.rs`) through Limine's live
  4-level tables, plus xAPIC LINT0 ExtINT bring-up (`arch/x86_64/apic.rs`). This
  fixed VirtualBox, where IRQ0 stayed pending at the PIC because LINT0 was
  masked and the APIC page is not part of Limine's HHDM.
- BIOS+UEFI hybrid ISO generation with pinned Limine assets, SHA-256 verification,
  a stdlib El Torito catalog verifier, and a hybrid-MBR install on the ISO file.
- Build/ISO/unit/lint/format/boot/memory/graphics/VirtualBox/benchmark scripts,
  plus a per-memory-size boot matrix (256 MiB, 512 MiB, 1 GiB).
- ISO license notices for the redistributed Limine binaries, linked Rust
  runtime and project code.
- Host-tested free-list heap (`os_core/src/heap.rs`): address-ordered free list,
  first-fit with tail splitting, 16-byte alignment, two-header block layout
  (block + link header) so double frees are detected without corruption,
  poison-on-free, full `validate()` walk with `HeapStats`/`HeapError`, and a
  500-step churn test that validates after every operation. 63 host tests pass
  (the Phase 6 additions are 9 path-validation tests in `os_core::path`).
- Kernel heap (`kernel/src/heap.rs`): fixed 8 MiB contiguous region leased from
  the frame allocator and reached through the HHDM, wrapped in an IRQ-safe
  lock, registered as `#[global_allocator]`; in-kernel self-test allocates a
  `Box`, grows a 10,000-element `Vec`, writes/reads a 1 MiB block and verifies
  accounting is restored.
- IRQ-safe spin lock (`IrqSpinLock`) that saves/restores RFLAGS around the
  critical section; existing `SpinLock` kept for interrupt-disabled contexts.
- Kernel VM manager (`kernel/src/vm.rs`): own 4-level PML4 built by copying the
  kernel half (entries 256..512) from the live tables, switch of CR3 onto it at
  boot, page map/unmap/translate with on-demand intermediate tables, user-half
  teardown, `invlpg`+fence TLB discipline, and W^X enforced at the API boundary
  (writable ⇒ non-executable, host-tested predicate). Self-test covers mapping,
  translation, guard refusals (W^X, double map, misalignment, user-in-kernel-
  half, non-canonical), CR3 swap with live user-page touches, unmap and frame
  release.
- Kernel scheduler (`kernel/src/sched.rs`, `arch/x86_64/context.rs`): boot
  context as thread 0, fixed table of 16 threads with 16 KiB kernel stacks from
  the frame allocator, fabricated switch-in frames with SysV-correct stack
  alignment, register/RSP-swap context switch, cooperative `yield_now`/`exit`
  and timer preemption (ISR sets `need_resched`, switch at dispatcher tail so
  the interrupted frame stays on its own stack and unwinds through `iretq`),
  deferred stack reclaim for exited threads, `hlt`-based join with PIT/TSC
  budgets. Boot self-test: two yielding workers (5 A/B steps interleaved) plus
  a CPU-bound spinner that completes under preemption. Observed in QEMU 512 MiB:
  24 switches, 11 preemptions; VirtualBox: 21 switches, 8 preemptions.
- IRQ-safe serial sink: the write lock now runs with interrupts disabled so a
  preempted thread mid-write cannot strand another thread spinning on the lock.
- Ring-3 user mode (Phase 5b, `kernel/src/user.rs`, `kernel/src/syscall.rs`):
  GDT user segments (data 0x2b, code 0x33, DPL3), DPL3 `int 0x80` gate, an
  iretq trampoline that programs the TSS `rsp0` with the current thread's kernel
  stack, two test programs copied from kernel text into fresh user frames of a
  private address space built from the kernel-half template. Program A performs
  one validated console write (`SYS_WRITE`) and exits with code 0 (`SYS_EXIT`);
  program B writes to an unmapped user address and is killed by the page-fault
  path with the kernel continuing. Every syscall argument is validated
  (fd/length bounds, user-half range, every touched page mapped user-accessible
  via a live-CR3 walk). After both joins the kernel reactivates its PML4 and
  tears the user spaces down (unmap, free frames, destroy page tables).
  Observed identically in QEMU (three RAM sizes) and VirtualBox.
- Phase 6 stage 1 — console input (`kernel/src/console.rs`): COM1 receive on
  IRQ4 (vector 36), the ISR drains the UART into a 256-byte software ring and
  counts real FIFO overruns (LSR bit 1) plus full-ring drops; echo, backspace
  handling and line assembly run in the reader thread, which blocks in `hlt`.
  The UART FIFO trigger level is 1 (`FCR` 0x07): the common 0xc7 value strands
  short bursts below its 14-byte threshold, which produced reordered/dropped
  characters when driving QEMU through a pipe.
- Phase 6 stage 1 — in-memory VFS (`kernel/src/vfs.rs`): arena of ≤64 nodes in
  the kernel heap with inline data capped at 8192 B/file, seeded `/etc`,
  `/etc/motd`, `/etc/version`, `/tmp`; `write` creates or replaces, `append`
  appends, plus mkdir/rm/stat/list/read; boot self-test covers
  create/read/append/overwrite/remove/list and the error paths (observed:
  `vfs selftest: passed (6 nodes, 3 files; ...)`). Path rules (absolute, no
  `.`/`..`, printable ASCII, length caps) are host-tested in `os_core::path`.
  Stage 2 adds checkpoint persistence on top of this in-memory tree (see the
  stage-2 bullets).
- Phase 6 stage 1 — shell (`kernel/src/shell.rs`): a kernel thread spawned
  after the success marker (the boot thread hands it the CPU with
  `yield_now()`); builtins cover help/version/echo/mem/heap/ticks/uptime/
  threads/ls/cat/write/append/mkdir/rm/stat/vfs/console/clear over the kernel
  services. The marker gained `,vfs`.
- Phase 6 stage 1 — interactive ISO and scripted session test:
  `make_iso.sh --interactive` emits `artifacts/project-os-interactive.iso`
  with `debug_exit` stripped from the cmdline (kernel falls through to the
  shell); `scripts/test_shell_qemu.sh` boots it with a QEMU **TCP serial
  chardev** and `scripts/test_shell_qemu.py` types a paced session (2 ms per
  byte), asserting 17 commands, the prompts, the marker and `0 overruns`.
  `-serial stdio` is deliberately avoided: QEMU's 16550 inserts unthrottled
  host bursts into the 16-byte FIFO faster than any guest can drain, a
  harness artifact rather than kernel behaviour.
- Phase 6 stage 2 — checkpoint format (`os_core/src/pofs.rs`): CRC32 (IEEE
  0xEDB88320, bitwise; vectors for `""`/`"a"`/`"123456789"`), 512-byte header
  (magic `POFS`, version 1, sequence number, payload length and CRC, header
  CRC), DFS-pre-order node-record payload encode/decode with the full
  validation set (magic/version/reserved/CRCs/lengths/kinds/parents/names/
  duplicates/trailing bytes/empty slot) and a corruption-corpus host test
  suite. `os_core` host tests: 63 → 77. Full pinned contract (register
  protocol, log lines, harness plan, honest limits): `docs/PERSISTENCE.md`;
  decision record: ADR-0016.
- Phase 6 stage 2 — ATA PIO block driver (`kernel/src/blk.rs`): primary PIIX
  channel (0x1F0/0x3F6), master, LBA28 (`0xE0` drive/head select), polled
  with a 5 s PIT deadline, no IRQ14/DMA; IDENTIFY parses model + sector
  count, `read_sectors`/`write_sectors`/`flush` (0xE7 after every write),
  range validation, sector/flush/error counters, `BlkError` labels. The boot
  probe logs exactly one pinned line. A real defect was found and fixed
  during harness bring-up: an early `0xA0` (master, **CHS**) select made
  QEMU translate the LBA registers through the disk geometry (slot A landed
  at LBA 15, slot B at 8079) while reads/writes stayed self-consistent —
  only byte-level inspection of the raw image exposed it; `0xE0` (LBA mode)
  is correct and the spec always said so.
- Phase 6 stage 2 — checkpoint persistence (`kernel/src/persist.rs`): two
  1 MiB slots (A at LBA 16, B at LBA 2064 of the 8 MiB test image), fresh
  disk → format slot A seq 0; boot examines both slots (all-zero = empty,
  invalid = logged and skipped) and loads the valid highest-sequence slot;
  save targets the inactive slot at seq+1, verifies the header by read-back,
  then switches active; failed saves keep the previous checkpoint and count
  an error. VFS grew dirty tracking (`mark_dirty` on every mutation,
  `clear_dirty` after a confirmed save), `export()` (DFS pre-order with
  parent remap) and `replace()` (validating rebuild of the arena), plus
  `VfsError::Corrupt`. The shell loop calls `flush_if_dirty()` after each
  command (write-through at command boundaries); `blk` and `persist`
  builtins report state; the `version` line now says the VFS is a
  checkpoint image on ATA disk when present. The marker gains `,blk,persist`
  only when a usable disk is active; diskless boots are byte-identical to
  stage 1.
- Phase 6 stage 2 — persistence harness: shared QEMU TCP-serial session
  driver extracted to `scripts/qemu_serial.py` (used by the refactored
  `test_shell_qemu.py` and the new `test_persistence_qemu.py`);
  `scripts/test_persistence_qemu.sh` boots the interactive ISO four times
  against one raw 8 MiB disk on `-machine pc` (PIIX legacy IDE; R-009):
  fresh format + first save, reboot load + two more saves (A seq2 / B seq3),
  driver-side payload corruption of the newest slot → `payload crc
  mismatch` + fallback to A seq2 (lost-generation file gone), both payloads
  corrupted → fresh format (old file gone). Wired into `test_all.sh` as the
  `persistence` step.
- Phase 6 stage 3 — ELF64 parser (`os_core/src/elf.rs`): header validation
  (magic/class/data/version/OSABI/e_type `ET_EXEC`/machine `EM_X86_64`),
  program-header table rules (1..16 entries, no `PT_INTERP`), per-segment
  checks (page-aligned, `filesz <= memsz`, zero-fill tail, W^X refusal,
  no page overlap, entry inside an executable segment, fixed
  0x400000..0x500000 window) with overflow-safe range arithmetic; named
  `ElfError` labels; 19 new host tests (96 total). Contract:
  `docs/ELF_LOADER.md`; decision: ADR-0017.
- Phase 6 stage 3 — Limine modules + loader (`limine.rs`, `loader.rs`):
  module request (`ModuleResponse`) and exact-path lookup
  (`module_by_path`); `limine_file` corrected to its real 112-byte x86-64
  layout (`tftp_ipv4` is `[u8;4]`; R-010). Two-phase loader: `inspect`
  parses and policy-checks without allocating (rejects `bad.elf` with
  `bad magic`), `map` allocates, zero-fills and maps each segment through
  the HHDM with W^X flags, rolling back every page on failure (named
  `Reject` reasons).
- Phase 6 stage 3 — ring-3 integration (`kernel/src/user.rs`, `main.rs`):
  `hello.elf` and `fault.elf` run sequentially on scheduler threads at CPL3
  through the unchanged `int 0x80` path; the corrupt-module probe and absent
  modules produce explicit skipped verdicts rather than a false pass; the
  success marker gains `,elf` between `user` and `vfs`.
- Phase 6 stage 3 — userland workspace (`userland/`): independent `no_std`
  Rust workspace with its own `linker.ld` and no external crates; `hello`
  prints the exact stage-5b greeting, `fault` performs an unmapped write;
  `scripts/build_user.sh` builds both with `RUSTFLAGS`, verifies them with
  `check_user_elf.py`, and derives `bad.elf` by zeroing the magic (all three
  modules staged into the ISO by `make_iso.sh`). Deviation (ADR-0017):
  flushing kernel link flags per-crate needs a cargo config that also applies
  to userland (cargo joins ancestor `target.*.rustflags`), and relocating
  them to a `build.rs` is impossible in this checkout — any build-script
  executable path exceeds the Windows 260-character `MAX_PATH` (MSVC
  `LNK1104`), so userland flags are passed via `RUSTFLAGS` and the kernel's
  stay in the root `.cargo/config.toml`.
- Phase 6 stage 3 — harness: `scripts/test_elf_qemu.sh` boots the ISO and
  asserts the seven ELF serial lines, the skipped-path absence and the
  `sched,user,elf,vfs` marker; `test_all.sh` gained `userland` and `elf`
  steps (16 → 18 PASS); `format.sh`/`lint.sh` extended to the userland
  workspace (clippy without `--all-targets`, which cannot build a test
  harness for `x86_64-unknown-none`).
- Phase 6 stage 4 — framebuffer console core (`os_core`): the original 5x7
  font moved to `os_core::font` (5 tests), the scrolling grid model to
  `os_core::textcon` (wrap, `\n`/`\r`/`\t`/backspace, scroll, dirty ranges;
  14 tests), and the pitch-aware 16/24/32-bpp cell/rect compositor to
  `os_core::textrender` (`Colors`/`Rect` API, 13 tests incl. exact glyph
  patterns and buffer-unchanged OOB errors). `FbError::ZeroScale` added.
  Host tests 96 → 128. Contract `docs/FRAMEBUFFER_CONSOLE.md`; research
  R-011.
- Phase 6 stage 4 — kernel binding (`kernel/src/fbcon.rs`): static
  `IrqSpinLock<Option<FrameConsole>>` with a 160×64 cell grid; sizing
  `scale = max(1, min(w/(80*6), h/(25*8)))` then clamp to the ceiling grid
  (1280×800 → 106×50 at scale 2; 1024×768 → 85×48); dirty-row repaint with a
  cursor underline and full-screen background fill on init; boot readback
  self-test (re-render into a heap scratch buffer, `sfence`, per-scanline
  compare) that is fatal on mismatch. New `arch::x86_64::sfence()` (Intel SDM
  Vol. 3A §11.3) and `IrqSpinLock::try_lock`.
- Phase 6 stage 4 — log tap (`kernel/src/serial.rs`) and UI removal: the
  `kprint!` path mirrors blocking to the console; panic/exception/shell/user
  paths use the non-blocking `try_write_bytes` so a held console lock cannot
  deadlock a panic. `kernel/src/display.rs` and `kernel/src/font.rs`
  (dashboard) are deleted; `main.rs` logs the pinned `fbcon: enabled ...` /
  disabled line and selftest verdict, and the shell `clear` builtin clears the
  grid. Boot marker checks are unchanged (the console is not a gate).
- Phase 6 stage 4 — harness: `scripts/qemu_serial.py` gained an optional QEMU
  monitor port, `serial_wait` (this test uses `wait=on` so early boot lines are
  not dropped) and `screendump()`; new `scripts/test_console_qemu.py`,
  `scripts/check_screendump.py` (PPM dimensions vs serial geometry, ≥500
  foreground pixels in the grid, pure background outside it) and
  `scripts/test_console_qemu.sh`; `test_all.sh` gained the `fbcon` step
  (18 → 19 PASS). Evidence: 106×50 grid, 119,512 foreground pixels,
  `fbcon selftest: passed (4070400 bytes match after 1059 redraws)`.
   VirtualBox BIOS logs `fbcon: enabled 85x48 cells at scale 2` and
   `3133440 bytes match after 1088 redraws`.
- Phase 6 stage 5 — host-tested core (`os_core/src/pci.rs`,
  `os_core/src/net/{mod,eth,arp,ipv4,icmp,udp}.rs`): PCI config-address
  construction, BAR decode/sizing (32-bit wrap, 64-bit pair, I/O BARs);
  Ethernet MAC/type/header; ARP request/reply plus an 8-entry cache with a
  30 s lifetime (refresh, oldest-eviction); IPv4 header parse/build with
  RFC 1071 checksum, IHL-5-only, fragment and bad-checksum rejection; ICMP
  echo request/reply; UDP header with pseudo-header checksum, zero-checksum
  rules (never transmit 0x0000, accept received 0). Golden byte vectors,
  known-answer checksums and rejection vectors for each. Host tests
  128 → 179 (51 new).
- Phase 6 stage 5 — PCI scan and driver (`kernel/src/pci.rs`,
  `kernel/src/e1000.rs`, new `inl` in `arch::x86_64::io`): mechanism #1 on
  `0xCF8`/`0xCFC`, bus-0 scan for `8086:100E`, BAR0 all-ones write-back with
  immediate restore, memory-space + bus-master enable. Driver: BAR0 aligned
  to 2 MiB and mapped uncached, `CTRL.RST` reset with a 5 s-pump deadline,
  MAC from RAL/RAH then bounded EEPROM reads (VirtualBox fallback), legacy
  rings (RX 32 × 2048 B with tail/head protocol; TX 8 descriptors over one
  shared buffer, synchronous `STA.DD` poll), `RCTL = EN|BAM|SECRC`,
  `TCTL = EN|PSP|CT|COLD|RTLC`, `TIPG`; counters; non-fatal MAC loopback
  check (0x88B5 pattern frame). Note: the loopback selftest frame is a real
  transmit and is counted in `tx` (documented in `docs/NETWORK.md` §11).
- Phase 6 stage 5 — stack and shell (`kernel/src/net.rs`, `main.rs`,
  `shell.rs`): static `10.0.2.15/24` gw `10.0.2.2`; boot probe + ARP/ICMP
  selftest with the pinned `net:` lines; panic-free inactive/skipped paths;
  marker gains `,net` (after `,persist`) only when the selftest passed;
  `ping <ip> [-c n]` and `udpecho <port>` builtins; dispatch for ARP, IPv4
  (ICMP/UDP), with undocumented cases dropped silently. Poll loops take the
  IRQ-safe lock per iteration so PIT/serial interrupts keep flowing;
  transmit (which can run with interrupts disabled) also has a spin bound.
- Phase 6 stage 5 — end-to-end defect found and fixed by the harness
  (recorded in `docs/NETWORK.md` §11): `send_echo_request`/`send_echo_reply`
  wrote the echo payload *after* `icmp::Message::write` had computed the
  checksum, so every echo request carried a stale checksum and was dropped
  (`net selftest: no gateway reply (echo timeout)` on the first
  `test_net_qemu.py` run; ARP was unaffected because it has no checksum).
  Writing the payload first fixed it; host tests cannot catch this class
  because it was glue call order, not codec logic. A second correction:
  the design assumed QEMU loops `RCTL.LBM_MAC` frames; QEMU's `e1000.c`
  implements only PHY `BMCR` loopback (source read 2026-10-18), so
  `net: loopback selftest: unsupported` is the expected QEMU/VirtualBox
  outcome (R-012 updated; `,net` is unaffected because the ARP/ICMP probe
  is the gate).
- Phase 6 stage 5 — harness: new `scripts/test_net_qemu.py` (reuses
  `scripts/qemu_serial.py`; picks a free host UDP port for
  `hostfwd=udp:127.0.0.1:<port>-:7777`) and `scripts/test_net_qemu.sh`
  (interactive ISO on q35 with a pinned `model=e1000` slirp NIC — q35's
  default e1000e is unsupported by design). Asserts the pinned bring-up
  lines, the `,net` marker, `net` counters, three `ping` replies with `0%
  loss`, and a 19-byte host→guest UDP echo (`udpecho: echoed 19 bytes`);
   `test_all.sh` gained the `net` step (19 → 20 PASS); VirtualBox now
   attaches `--nic1 nat --nictype1 82540EM`.
- Phase 7 stage 1 — host-tested parser (`os_core/src/pe.rs`): PE32+/AMD64/CUI
  validation, section layout and file-range checks, data-directory policy
  (imports/delay/TLS/CLR must be empty), a DIR64 relocation walk (ABSOLUTE
  padding skipped, type 10 only) with compact RVA storage, and stable
  rejection labels; 56 new host tests (235 total). Contract: spec
  `docs/PE_LOADER.md`, decision ADR-0019, research R-013.
- Phase 7 stage 1 — kernel loader (`kernel/src/pe_loader.rs`): two-phase
  `inspect` (window `0x400000..0x500000` page-rounded, ≤16 sections, W^X,
  nonzero delta requires present non-stripped relocs) and `map` (alloc,
  zero-fill, payload copy through the HHDM, per-section W^X flags, DIR64
  application with per-target writable/page-cross checks, full rollback on
  failure). `kernel/src/user.rs` gained the PE thread path, a
  `LoadedPlan { Elf, Pe }` teardown enum and the PE phase in `demo()`
  (skip without PE modules, `pebad.exe` probe, spawn `user-pe`, exit/write/
  fault checks); `main.rs` gates the `,pe` marker and prints the PE verdict.
- Phase 7 stage 1 — fixture pipeline: `userland/pe` (separate workspace,
  `pehello` fixture: `#![no_std]`, rust-lld links it with
  `/nodefaultlib /entry:_start /subsystem:console`, a `global_asm!` `.quad`
  slot forces a real DIR64 relocation, the fixture compares the relocated
  value against a rip-relative address), `scripts/build_user_pe.sh` (target +
  rust-lld checks, builds, generates deterministic `pebad.exe` by zeroing the
  signature, runs the checker on both), `scripts/check_user_pe.py`
  (dependency-free contract check: image base, section ranges, W^X, entry,
  relocation targets), `scripts/test_pe_qemu.sh` (pinned serial assertions),
  `test_all.sh` gained `userland-pe` and `pe` steps (20 → 22 PASS),
  `test_elf_qemu.sh` marker now `sched,user,elf,pe,vfs`, `bootstrap.sh` checks
  the msvc target and rust-lld, `lint.sh`/`format.sh` cover `userland/pe`,
  and `boot/limine.conf` + `make_iso.sh` stage both `.exe` modules.
- Phase 7 stage 1 — bring-up corrections recorded: the fixture ends with
  **4** sections (no `.pdata`; the probe's 5-section form was larger), and
  the parser's per-image relocation cap is 256 (not 1024) so the plan fits
  the 16 KiB kernel thread stacks; `docs/PE_LOADER.md` and R-013 state both.
- Phase 7 stage 2 — host-tested import parsing (`os_core/src/pe.rs`): the
  import directory (index 1) is now parsed — descriptor scan with caps
  (`MAX_IMPORT_DLLS=2`, `MAX_IMPORTS=16`, name caps 16/32), ILT with IAT
  fallback, hint/name entries, case-insensitive `kernel32.dll` match,
  ordinal/TLS-callable/JMP-thunk imports rejected with stable labels, and
  `iat_rva = first_thunk + index*8` recorded per slot; 17 new host tests
  (252 total). Contract: spec `docs/WIN32_CONSOLE.md`, decision ADR-0020,
  research R-014.
- Phase 7 stage 2 — kernel resolver (`kernel/src/pe_loader.rs`):
  `WIN32_DLL`/`WIN32_SERVICES` allowlist (`GetStdHandle`, `WriteConsoleA`,
  `ExitProcess`), new `Reject::{UnknownImportDll, UnknownImportFunction,
  ImportTarget}` checks in `inspect`, and `map` now plans a one-page stub
  page at `WIN32_STUB_VA = 0x510000` (deduplicated `B8 <service> CD 80 C3`
  entries, stride 16) and patches each IAT slot through the HHDM before the
  section protection call (the Windows fixup-before-protection order);
  `map` returns `Loaded { image, stub_page, applied_relocations }`.
- Phase 7 stage 2 — kernel services (`kernel/src/syscall.rs`): services 3–5
  implement `GetStdHandle` (STD_OUTPUT_HANDLE → handle 1, pinned log),
  `WriteConsoleA` (validated user pointer + length against `MAX_WRITE`,
  writes to the serial console fd, pinned log) and `ExitProcess`
  (`sched::exit()` after a pinned log), with counters
  `WIN32_GET_STD_HANDLE_CALLS`/`WIN32_WRITE_CONSOLE_CALLS`/
  `WIN32_EXIT_PROCESS_CALLS`. `kernel/src/user.rs` gained the win32 demo
  phase (skip when fixtures absent), the `user-win32` thread, stub-page
  teardown, and a `win32` flag in `DemoReport`; `main.rs` gates the
  `,win32` marker and prints the verdict.
- Phase 7 stage 2 — fixtures (`userland/pe/peconsole`, workspace member
  added): `#![no_std]`, `#[link(name = "kernel32", kind = "raw-dylib")]`
  extern block, `global_asm!` marker slot checked against a rip-relative
  reference (M3 on mismatch), M1/M2 console writes, `ExitProcess(0)`;
  stage-2 exit codes 1/2/3/5/0 and panic `0xBEEF` are pinned in
  `docs/WIN32_CONSOLE.md` §4.
- Phase 7 stage 2 — pipeline and harness: `scripts/check_user_pe.py` gained
  `--imports <dll:fn,...>` / `--imports -` (descriptor termination, exact
  DLL name, ordinal-free ILT, name-set equality, IAT slots patchable and
  in-image); `scripts/build_user_pe.sh` builds/copies five artifacts and
  generates `pebadimp.exe` (`kernel32.dll` → `user32.dll`, unique-pattern
  patch) and `pebadfn.exe` (`WriteConsoleA` → `WriteConsoleB`), failing
  loudly if a pattern is not unique; `boot/limine.conf` and
  `scripts/make_iso.sh` stage five `.exe` modules; `scripts/test_pe_qemu.sh`
  asserts the full stage-2 set (`pe: 5 modules present`, both load lines,
  seven `win32:` lines, both rejection lines, `win32 selftest: passed`);
  `scripts/test_elf_qemu.sh` marker is now `sched,user,elf,pe,win32,vfs`.
- Phase 7 stage 2 — two bring-up defects found by the end-to-end harness and
  fixed before the PASS: (1) `pe_loader::map` iterated the full
  `image.imports` array (16 slots, including the `EMPTY` tail) instead of
  `image.imports[..image.import_count]` in both the stub-planning and
  IAT-patching loops, so valid modules were rejected with
  `unknown import function`; (2) `AddressSpace::translate`
  (`kernel/src/vm.rs`) returned the frame base without the in-page offset,
  so unaligned IAT writes (slots at `0x2108`/`0x2110`) landed on the wrong
  offset inside the page and the fixture faulted at `rip=0x212e`
  (`cr2=0x212e`, the raw hint/name RVA). The fix (`entry_address(leaf) +
  (virtual & 0xfff)`) also corrects any other unaligned write through
  `translate` — stage-1 relocation targets were page-aligned, so the bug
  was latent there. Both are recorded in R-014; no test was loosened.

## Exact commands and results

| Command | Result |
|---|---|
| `cargo build -p project_os_kernel --target x86_64-unknown-none --release` | PASS |
| `cargo test -p os_core` | PASS: 179 passed, 0 failed (includes 14 heap, 9 path, 14 checkpoint-format, 19 ELF-parser, 5 font, 14 text-grid, 13 renderer, and 51 new stage-5 PCI/BAR + Ethernet/ARP/IPv4/ICMP/UDP codec tests) |
| `cargo test -p os_core` (Phase 7 stage 1) | PASS: 235 passed, 0 failed (adds 56 PE32+ parser tests: every `PeError` row, relocation walk, accessor coverage) |
| `cargo clippy` (os_core, membench, kernel, userland) with `-D warnings` | PASS |
| `cargo clippy` (+ `userland/pe`, release, `--target x86_64-pc-windows-msvc`) with `-D warnings` (Phase 7 stage 1) | PASS |
| `cargo fmt --all -- --check` (kernel workspace and userland) | PASS |
| `cargo fmt --all -- --check` (+ `userland/pe`) (Phase 7 stage 1) | PASS |
| `scripts/build_user.sh` | PASS: hello/fault built and header-checked; bad.elf rejected with `bad magic` |
| `scripts/build_user_pe.sh` (Phase 7 stage 1) | PASS: `pehello.exe` 3072 bytes, entry `0x1000`, image base `0x140000000`, 4 sections, 1 DIR64 relocation; `pebad.exe` rejected by `check_user_pe.py` with `bad signature` |
| `scripts/fetch_limine.sh` | PASS: checksum verified, assets installed |
| `scripts/make_iso.sh` | PASS: ISO built (kernel + three ELF modules + two PE modules), BIOS+UEFI catalog entries verified |
| `scripts/test_boot_qemu.sh` (BIOS, 256 MiB / 512 MiB / 1 GiB) | PASS: marker reached, QEMU exit 33 at each size |
| `scripts/test_memory.sh` | PASS: host allocator tests + in-VM self-test |
| `scripts/test_vfs.sh` | PASS: host path tests + in-VM VFS self-test |
| `scripts/test_shell_qemu.sh` | PASS: 17-command scripted shell session, 0 overruns |
| `scripts/test_persistence_qemu.sh` | PASS: 4 boots on one raw disk — format/save, reboot load, CRC fallback, fresh format (`persistence: 4 boots, all phases passed`) |
| `scripts/test_elf_qemu.sh` | PASS: 3 modules, ring-3 greeting from the ELF, exit 0, one isolated fault, `bad.elf` rejected, `,elf` marker |
| `scripts/test_console_qemu.sh` | PASS: `fbcon: enabled 106x50 cells at scale 2`, selftest 4,070,400 bytes after 1059 redraws, shell `version` mirrored, screendump 1280x800 with foreground pixels inside the grid and pure-background margins |
| `scripts/test_net_qemu.sh` | PASS (reproduced twice): `net: e1000 at 00:02.0`, MAC `52:54:00:12:34:56`, `net: ip 10.0.2.15/24 gw 10.0.2.2`, `net: loopback selftest: unsupported`, `net selftest: passed (arp resolved, icmp echo reply in 0 ms)`, `,net` marker; `net` → `tx 3 frames, rx 2 frames, arp 1/8 entries, errors 0`; `ping 10.0.2.2 -c 3` → 3/3 replies, `0% loss`; host→guest 19-byte UDP echo byte-identical (`udpecho: echoed 19 bytes to 10.0.2.2:57841`) |
| `scripts/test_graphics_host.sh` | PASS: framebuffer tests |
| `scripts/test_all.sh` (Phase 6 stage 5, final) | PASS: 20 PASS, 0 FAIL, 4 SKIP (UEFI/pci/driver-model/gpu-vm; skipped is not passed); unit suite 179 passed |
| `scripts/test_pe_qemu.sh` (Phase 7 stage 1) | PASS: `pe: 2 modules present`; `pe: pehello.exe loaded at 0x400000 (entry 0x401000, 4 sections, 1 relocation applied)`; `pe: hello from a PE32+ image via int 0x80`; `pe: base relocation verified`; `user: /boot/pebad.exe rejected (bad signature)`; `pe selftest: passed (...)`; marker `checks=...,sched,user,elf,pe,vfs`; exit 33 |
| `scripts/test_all.sh` (Phase 7 stage 1, final) | PASS: 22 PASS, 0 FAIL, 4 SKIP (skipped is not passed); unit suite 235 passed; ELF, persistence, net and VirtualBox steps stayed green in the same run |
| `cargo test -p os_core` (Phase 7 stage 2) | PASS: 252 passed, 0 failed (adds 17 import-directory tests: descriptor/caps, ILT↔IAT fallback, names, ordinal rejection, empty imports) |
| `scripts/build_user_pe.sh` (Phase 7 stage 2) | PASS: five artifacts — `pehello.exe` unchanged (SHA-256 `96e2e6d6…`, hash-stable from clean rebuilds); `peconsole.exe` 3584 bytes, entry `0x1000`, 5 sections, 1 relocation, imports `kernel32.dll:ExitProcess,GetStdHandle,WriteConsoleA`; `pebad.exe` unchanged (SHA-256 `a0f8a2b4…`); `pebadimp.exe` + `pebadfn.exe` generated from unique byte patterns and structurally valid |
| `scripts/make_iso.sh` (Phase 7 stage 2) | PASS: ISO built (kernel + three ELF modules + **five** PE modules), BIOS+UEFI catalog entries verified |
| `scripts/test_pe_qemu.sh` (Phase 7 stage 2) | PASS: `pe: 5 modules present`; `pe: peconsole.exe loaded at 0x400000 (entry 0x401000, 5 sections, 1 relocation applied)`; the seven `win32:` lines (`GetStdHandle(-11) -> 1`, M1, `WriteConsoleA(1, 44 bytes) -> 1`, M2, `WriteConsoleA(1, 32 bytes) -> 1`, `ExitProcess(0)`); `user: /boot/pebadimp.exe rejected (unknown import dll)`; `user: /boot/pebadfn.exe rejected (unknown import function)`; `win32 selftest: passed (...)`; marker `checks=...,sched,user,elf,pe,win32,vfs`; exit 33 |
| `scripts/test_all.sh` (Phase 7 stage 2, final) | PASS: 22 PASS, 0 FAIL, 4 SKIP (skipped is not passed); unit suite 252 passed; the `pe` step ran the stage-2 assertions with `,win32`, the `elf` step used the new marker, and the persistence/net/VirtualBox steps stayed green in the same run |
| QEMU BIOS boot matrix, 256 MiB / 512 MiB / 1 GiB | PASS at each size: marker `checks=...,sched,user,elf,vfs`, exit 33; q35's default e1000e is deliberately unclaimed, so these boots log `net: inactive (no supported e1000 NIC)` and `net selftest: skipped (no NIC)` |
| VirtualBox BIOS (`scripts/test_virtualbox.sh`) | PASS with ELF-module ring-3 checks, `fbcon: enabled 85x48 cells at scale 2` / `3133440 bytes match after 1088 redraws`, and the 82540EM NIC observed: `net: e1000 at 00:03.0`, MAC `08:00:27:c4:5d:0a`, loopback `unsupported`, `net selftest: no gateway reply (arp timeout)` — non-fatal, marker without `,net` (`vbox-serial.log`); isolated VM created and deleted |
| `scripts/test_boot_qemu.sh --firmware uefi` | SKIP: no `OVMF_FD` / OVMF firmware |
| `scripts/test_virtualbox.sh artifacts/project-os.iso` | PASS (VirtualBox 7.2.20r175154): marker reached, 5 timer ticks, VM deleted |
| `scripts/test_pci.sh` | SKIP: PCI not implemented |
| `scripts/test_driver_model.sh` | SKIP: driver framework not implemented |
| `scripts/test_gpu_vm.sh` | SKIP: framebuffer-only, no virtual GPU driver |
| `scripts/bench_graphics.sh` | SKIP: no accelerated graphics |
| Corrupt-ISO negative boot test | PASS (expected failure): exit 1, marker absent |

Recorded QEMU boot log (Phase 6 stage 2, 512 MiB, **no disk attached** — log as
recorded then; the no-disk marker now reads `...,user,elf,vfs`, see the
stage-3 log below):

```text
kernel heap selftest: passed (box, vec growth, 1 MiB block)
vm selftest: passed (map/unmap, translate, W^X, guard refusals, CR3 swap)
vfs selftest: passed (6 nodes, 3 files; create/read/append/overwrite/rm/list/error paths)
exception selftest: passed
pit: 100 Hz, IRQ0 unmasked
console: COM1 receive interrupt enabled (IRQ4)
timer: 5 ticks observed
blk: no ATA device on primary channel
persist: inactive (no ATA disk)
sched: 5 workers finished (a=5, b=5, spinner=true), 24 switches, 11 preemptions, 1 live
sched selftest: passed (round-robin yield + timer preemption)
user: hello from ring 3 via int 0x80
syscall: thread exits with code 0
user fault isolated: vector=14 error=0x6 rip=0x40000a cr2=0x500000000 (thread killed, kernel continues)
user: ring-3 program exited (code 0), 1 write(s), 37 bytes, 1 isolated fault(s), 0 rejected syscall(s)
user selftest: passed (int 0x80 write/exit from CPL3, user fault isolated)
PROJECT_OS_BOOT_SUCCESS:0.1.0-0cc93723 checks=handoff,memmap,gdt,idt,exceptions,timer,allocator,heap,vm,sched,user,vfs
```

Recorded QEMU boot log (Phase 6 stage 3, 512 MiB, no disk attached,
`artifacts/qemu-serial-elf.log`, full ring-3 section):

```text
elf: 3 modules present
user: /boot/bad.elf rejected (bad magic)
user: hello from ring 3 via int 0x80
syscall: thread exits with code 0
user fault isolated: vector=14 error=0x6 rip=0x40000a cr2=0x500000000 (thread killed, kernel continues)
user: ring-3 program exited (code 0), 1 write(s), 37 bytes, 1 isolated fault(s), 0 rejected syscall(s)
user selftest: passed (ELF64 modules loaded, int 0x80 write/exit from CPL3, user fault isolated, corrupt module rejected)
PROJECT_OS_BOOT_SUCCESS:0.1.0-0cc93723 checks=handoff,memmap,gdt,idt,exceptions,timer,allocator,heap,vm,sched,user,elf,vfs
```

The stage-2 log above keeps its historical `...,user,vfs` marker; the
stage-3 boot inserts `elf` between `user` and `vfs` and the corruption probe
rejects `bad.elf` before any frame is allocated.

Recorded QEMU boot log (Phase 7 stage 1, 512 MiB, `artifacts/qemu-serial-pe.log`,
ring-3 section):

```text
elf: 3 modules present
pe: 2 modules present
user: /boot/bad.elf rejected (bad magic)
user: /boot/pebad.exe rejected (bad signature)
pe: pehello.exe loaded at 0x400000 (entry 0x401000, 4 sections, 1 relocation applied)
pe: hello from a PE32+ image via int 0x80
pe: base relocation verified
user selftest: passed (ELF64 modules loaded, int 0x80 write/exit from CPL3, user fault isolated, corrupt module rejected)
pe selftest: passed (PE32+ module loaded, base relocation applied, int 0x80 write from CPL3, corrupt module rejected)
PROJECT_OS_BOOT_SUCCESS:0.1.0-25606ad5 checks=handoff,memmap,gdt,idt,exceptions,timer,allocator,heap,vm,sched,user,elf,pe,vfs
```

The fixture has 4 sections (no `.pdata`); the loader's DIR64 delta is
`0x400000 - 0x140000000` and the relocation target check lives in the
fixture itself (`pe: base relocation verified` only prints on a match).

Recorded QEMU boot log (Phase 7 stage 2, 512 MiB, `artifacts/qemu-serial-pe.log`,
ring-3 section):

```text
elf: 3 modules present
pe: 5 modules present
user: /boot/bad.elf rejected (bad magic)
user: hello from ring 3 via int 0x80
syscall: thread exits with code 0
user fault isolated: vector=14 error=0x6 rip=0x40000a cr2=0x500000000 (thread killed, kernel continues)
user: /boot/pebad.exe rejected (bad signature)
pe: pehello.exe loaded at 0x400000 (entry 0x401000, 4 sections, 1 relocation applied)
pe: hello from a PE32+ image via int 0x80
pe: base relocation verified
syscall: thread exits with code 0
user: /boot/pebadimp.exe rejected (unknown import dll)
user: /boot/pebadfn.exe rejected (unknown import function)
pe: peconsole.exe loaded at 0x400000 (entry 0x401000, 5 sections, 1 relocation applied)
win32: GetStdHandle(-11) -> 1
win32: hello from the Win32 console surface
win32: WriteConsoleA(1, 44 bytes) -> 1
win32: console surface verified
win32: WriteConsoleA(1, 32 bytes) -> 1
win32: ExitProcess(0)
user: ring-3 program exited (code 0), 5 write(s), 184 bytes, 1 isolated fault(s), 0 rejected syscall(s)
user selftest: passed (ELF64 modules loaded, int 0x80 write/exit from CPL3, user fault isolated, corrupt module rejected)
pe selftest: passed (PE32+ module loaded, base relocation applied, int 0x80 write from CPL3, corrupt module rejected)
win32 selftest: passed (kernel32.dll imports resolved, WriteConsoleA wrote to the console fd, ExitProcess ended the thread)
PROJECT_OS_BOOT_SUCCESS:0.1.0-b8a2910d checks=handoff,memmap,gdt,idt,exceptions,timer,allocator,heap,vm,sched,user,elf,pe,win32,vfs
```

`peconsole.exe` has 5 sections (the `raw-dylib` import directory adds
`.idata`) and one forced DIR64 marker relocation; both mutants are rejected
by `inspect` before any frame is allocated, and the Win32 fixture's M1/M2
texts are asserted verbatim by `scripts/test_pe_qemu.sh`.

Interactive session (`artifacts/qemu-shell-session.log`, 512 MiB):

```text
PROJECT_OS shell. Type 'help' for commands.
project-os> version
PROJECT_OS 0.1.0 build 0.1.0-0cc93723 (x86_64, Limine base revision 6)
user-mode syscall gate: int 0x80; VFS: checkpoint image on ATA disk when present
project-os> write /tmp/note.txt hello
write: ok
project-os> append /tmp/note.txt  world
append: ok
project-os> cat /tmp/note.txt
hello world
project-os> stat /tmp/note.txt
/tmp/note.txt: file (11 B)
project-os> console
console: rx ring 0/256 bytes pending, 0 overruns (uart fifo or ring)
```

17 commands were typed at 2 ms per byte over a TCP serial chardev; all
responses, prompts and the marker were asserted (`session: 17 commands, 4067
bytes received, 0 failures`).

Persistence session (`artifacts/qemu-persist-boot1.log`, `-machine pc`,
8 MiB raw disk; boots 2–4 in the sibling logs):

```text
blk: found QEMU HARDDISK, 16384 sectors
persist: no valid checkpoint, formatted slot A seq=0
net: e1000 at 00:03.0 bar0 0xfebc0000 size 128 KiB
net: mac 52:54:00:12:34:56
net: ip 10.0.2.15/24 gw 10.0.2.2
net: loopback selftest: unsupported
net selftest: passed (arp resolved, icmp echo reply in 0 ms)
PROJECT_OS_BOOT_SUCCESS:0.1.0-5467bde4 checks=handoff,memmap,gdt,idt,exceptions,timer,allocator,heap,vm,sched,user,elf,vfs,blk,persist,net
project-os> blk
blk: QEMU HARDDISK, 16384 sectors (8 MiB), lba28 pio; reads 3 sectors, writes 2 sectors, flushes 1, errors 0
project-os> persist
persist: active slot A seq 0; saves 0, loads 0, last load none, errors 0
project-os> write /tmp/hello.txt hello world
write: ok
persist: saved slot B seq=1 (7 nodes, 254 bytes)
project-os> cat /tmp/hello.txt
hello world
```

Boot 2: `persist: loaded slot B seq=1 (...)` then `mkdir /tmp/notes` →
`saved slot A seq=2`, `write /tmp/notes/a.txt alpha` → `saved slot B seq=3`.
Boot 3 (slot B payload sector zeroed by the driver): `persist: slot B invalid
(payload crc mismatch)`, `persist: loaded slot A seq=2`, and
`cat /tmp/notes/a.txt` → `error: no such file or directory`. Boot 4 (both
payloads zeroed): both `invalid (payload crc mismatch)` lines, `persist: no
valid checkpoint, formatted slot A seq=0`, `cat /tmp/hello.txt` →
`error: no such file or directory`. Driver summary: `persistence: 4 boots, all
phases passed`.

Framebuffer console (`artifacts/qemu-console-session.log`, interactive ISO,
512 MiB; QEMU monitor screendump kept as `artifacts/console-screendump.ppm`):

```text
framebuffer: 1280x800 pitch 5120 bpp 32 at 0xffff8000fd000000
fbcon: enabled 106x50 cells at scale 2
...
fbcon selftest: passed (4070400 bytes match after 1059 redraws)
PROJECT_OS_BOOT_SUCCESS:0.1.0-f3b17a34 checks=handoff,memmap,gdt,idt,exceptions,timer,allocator,heap,vm,sched,user,elf,vfs
project-os> version
PROJECT_OS 0.1.0 build 0.1.0-f3b17a34 (x86_64, Limine base revision 6)
```

The screendump checker measured 1280×800, the 106×50 grid, 119,512
foreground pixels inside it and no non-background pixel outside it; a PNG
conversion was inspected to confirm the text is legible and correctly wrapped.
VirtualBox BIOS (`artifacts/vbox-serial.log`) shows the same contract at
1024×768: `fbcon: enabled 85x48 cells at scale 2` and
`fbcon selftest: passed (3133440 bytes match after 1088 redraws)`.

Network session (`artifacts/qemu-net-session.log`, interactive ISO on q35 with
a pinned `model=e1000` slirp NIC; `artifacts/qemu-net.log` is empty — QEMU
reported no errors; the run was reproduced twice):

```text
net: e1000 at 00:02.0 bar0 0xfebc0000 size 128 KiB
net: mac 52:54:00:12:34:56
net: ip 10.0.2.15/24 gw 10.0.2.2
net: loopback selftest: unsupported
net selftest: passed (arp resolved, icmp echo reply in 0 ms)
PROJECT_OS_BOOT_SUCCESS:0.1.0-5467bde4 checks=handoff,memmap,gdt,idt,exceptions,timer,allocator,heap,vm,sched,user,elf,vfs,net
project-os> net
net: mac 52:54:00:12:34:56, ip 10.0.2.15/24 gw 10.0.2.2; tx 3 frames, rx 2 frames, arp 1/8 entries, errors 0
project-os> ping 10.0.2.2 -c 3
reply from 10.0.2.2: seq=1 ttl=255 time=0 ms
reply from 10.0.2.2: seq=2 ttl=255 time=0 ms
reply from 10.0.2.2: seq=3 ttl=255 time=0 ms
ping: 3 sent, 3 received, 0% loss
project-os> udpecho 7777
udpecho: listening on 10.0.2.15:7777 for one datagram
udpecho: echoed 19 bytes to 10.0.2.2:57841
```

The harness sent `PROJECT_OS-NET-TEST` (19 bytes) to the host-forwarded UDP
port and received the identical payload back. `net: loopback selftest:
unsupported` is the expected QEMU outcome (its e1000 model implements only PHY
`BMCR` loopback — corrected research finding R-012; the ARP/ICMP probe is the
gate). The `tx 3` counter includes the loopback selftest frame (loopback
frame + ARP request + echo request); `rx 2` is the ARP reply and the ICMP
reply. The q35 default `e1000e` stays deliberately unclaimed: the boot matrix
and console/shell boots log `net: inactive (no supported e1000 NIC)` and
`net selftest: skipped (no NIC)`, leaving their markers unchanged, while the
`-machine pc` persistence boots claim the default e1000 and append `,net`
after `,persist` (boot-1 log above).

`scripts/test_boot_qemu.sh` (BIOS, 512 MiB) reached the marker and exited via
isa-debug-exit with the expected status 33; the 256 MiB / 1 GiB matrix and the
VirtualBox run were executed in the same `scripts/test_all.sh` pass.

Build identifier is `0.1.0-5467bde4`, derived from `VERSION` and the current
Git commit at build time (`5467bde`); rebuilding from changed sources without
a commit keeps the same ID, which is why the exact kernel/ISO hashes below are
recorded. (The stage-5 work was committed afterwards as `b257a1e`, so a fresh
build from HEAD would carry a new ID.)

## Artifact

| Item | Value |
|---|---|
| Kernel ELF | `artifacts/kernel.elf`, 1,269,912 bytes (Phase 7 stage 2) |
| Kernel SHA-256 | `ffa56e2420a73e46aeabf76f1a5917655bf8e1999fec097e20721ab59c963bcc` |
| Kernel ELF entry | `0xffffffff80000000` (verified with `readelf`) |
| userland modules | `artifacts/hello.elf` SHA-256 `f4ae1ae1a5bb1479f0fbf99a65991d1e5e12abb3b72537f6ffc0dac4f9f9a960`; `artifacts/fault.elf` SHA-256 `46c45ce6eca4bd67e2a1e06cc85b2ef79a68be4a5bf9d1b1af84a11d0c0c23f0`; `artifacts/bad.elf` SHA-256 `a8bec8d1c4b866fc0d0fd2e81a71cda920fc95da0c87c64286cb4c7b45f9006d` (hash-stable from a clean rebuild) |
| PE fixtures | `artifacts/pehello.exe`, 3072 bytes (entry RVA `0x1000`, base `0x140000000`, 4 sections, 1 DIR64 relocation), SHA-256 `96e2e6d6ff69db3ce5204a215defaa9996fe964224eb5fd0bc2d2ef72d577e41`; `artifacts/pebad.exe` (signature zeroed), 3072 bytes, SHA-256 `a0f8a2b401ec904eead12ba520bb9d0063e7ea212547457ab2a1439a017e453d`; `artifacts/peconsole.exe` (Phase 7 stage 2: 5 sections incl. `.idata`, 1 DIR64 relocation, imports `kernel32.dll:ExitProcess,GetStdHandle,WriteConsoleA`), 3584 bytes, SHA-256 `1e72a7b3d32421a5883b1c8344b1b1e149aa35a81b18648fcdba1e54e74eb406`; `artifacts/pebadimp.exe` (`kernel32.dll` → `user32.dll` pattern patch), 3584 bytes, SHA-256 `95c93ae440a10dbf8fc8b6bddc418752b06c4f5dc4fe68b8d0f76b889e0214e3`; `artifacts/pebadfn.exe` (`WriteConsoleA` → `WriteConsoleB` pattern patch), 3584 bytes, SHA-256 `4811ccf6488777b90b32c91e9f27fa212cf33fba1fc1c06404e16f4abebc2ab1` (pehello/pebad hash-stable from clean rebuilds; the three stage-2 add-ons rebuild deterministically from their patches) |
| ISO | `artifacts/project-os.iso` |
| ISO size | 7,299,072 bytes |
| ISO SHA-256 | `ab1e46c3710d4d77629e91feb86201213978e20566dbdb3aa59eedee06d38f5e` |
| Interactive ISO | `artifacts/project-os-interactive.iso` (same bytes but `debug_exit` stripped from the cmdline; rebuilt by each session harness) |
| Interactive ISO SHA-256 | `538e63479a3d1c8b6a5335dabb114ca565caa99fec05e7d2b097bfe84f7b3a3a` (rebuilt during the final stage-2 `test_all.sh` run) |
| Screendump | `artifacts/console-screendump.ppm`, 3,072,016 bytes (1280x800 P6), SHA-256 `002f45ab44049304dbf7f8605b58a597791caba5c6837ec7e4311c4ea73c0c40`; checker PASS (rebuilt in the stage-5 suite run; the boot log now includes the `net:` lines) |
| Framebuffer session log | `artifacts/qemu-console-session.log`, SHA-256 `501334a8fde73879a109602782a68a7591c8c571c2ca3e09f119ff8ad6c296e3` |
| Net session log | `artifacts/qemu-net-session.log`, 4,494 bytes, SHA-256 `940b29fbdd4477e325164e2df7615ef690947de1ca4dff7e6a5cda624ed3eb44`; `artifacts/qemu-net.log` is empty (0 bytes, no QEMU errors) |
| Checkpoint disk image | `artifacts/persist-disk.img`, 8,388,608 bytes (raw; final state after the 4-boot test: fresh format, slot A seq 0), SHA-256 `cb97111ff33d7e456e5ea82d9352ed3281e93c882fb02a02bd1947fc8f4857de` |
| ISO boot entries | BIOS platform 0x00 (4 sectors), UEFI platform 0xEF (6144 sectors), both no-emulation, verified by `scripts/verify_iso.py` |
| Serial logs | `artifacts/qemu-serial-256m.log`, `-512m.log`, `-1g.log`, `artifacts/qemu-serial.log` (memory test), `artifacts/vbox-serial.log` (VirtualBox), `artifacts/qemu-shell-session.log` (interactive session), `artifacts/qemu-console-session.log` (framebuffer console), `artifacts/qemu-persist-boot{1,2,3,4}.log` + `qemu-persist.log` (4-boot durability), `artifacts/qemu-serial-elf.log` (ELF64 module boot), `artifacts/qemu-serial-pe.log` (PE32+/Win32 module boot; stage-2 log 6738 bytes, SHA-256 `4259ec213189c53310b57e23f16d65998839185efa19c8f268f3496c5a98515a`), `artifacts/qemu-net-session.log` (e1000 network session) |
| VirtualBox result | `artifacts/vbox-test.log`: VirtualBox 7.2.20 BIOS PASS, marker, 5 ticks, `fbcon: enabled 85x48` and console selftest observed; with the 82540EM NAT NIC the kernel detected the NIC and reported `net: loopback selftest: unsupported` + `net selftest: no gateway reply (arp timeout)` — non-fatal, no VirtualBox networking claim |

ISO byte-for-byte reproducibility is not claimed: `mkisofs` embeds file
timestamps, so a rebuild changes the ISO checksum even with unchanged sources.
The kernel and fixture builds are deterministic for identical sources, flags
and build ID. The checksums above are from the final Phase 7 stage 2
`scripts/test_all.sh` run (22 PASS / 0 FAIL / 4 SKIP, unit suite 252 passed).
The ISO includes
`/licenses/LICENSE`, `/licenses/LICENSES.md`, `/licenses/limine-LICENSE`,
`/licenses/limine-3RDPARTY.md` and `/licenses/rust/*`, because it redistributes
the kernel (including linked Rust runtime code), the PE fixtures (linked
`core`/`compiler_builtins` for `x86_64-pc-windows-msvc`) and Limine binaries.

## Benchmarks

`scripts/bench_memory.sh --mib 64 --iterations 200000 --seed 8388610`, release
host build (Rust 1.99.1, commit 58cf8f9c, `opt-level=2`, `debug=1`, thin LTO, `panic=abort`),
Windows x86-64: 200,200 ops, 2.970 ms, mean 14.8 ns/op, p50 14 ns,
p99 17 ns, accounting restored, fragmentation ratio 1.0000 in this workload.
This is a host model, not a kernel or Linux comparison.

## Decisions

- [docs/DECISIONS.md](docs/DECISIONS.md): Limine/base revision 6, Rust target,
  modular monolith, usable-RAM bitmap allocator, legacy PIC/PIT bring-up, APIC
  MMIO mapping + LINT0 ExtINT, serial-authoritative dashboard, cdrtools ISO
  path, gated debug-exit, original font, case-insensitive control-file policy,
  serial console/VFS/shell (ADR-0015), ATA PIO + checkpoint persistence
  (ADR-0016), the native ELF64 module loader incl. the RUSTFLAGS/MAX_PATH
  deviation (ADR-0017), the framebuffer log console
  (`docs/FRAMEBUFFER_CONSOLE.md`, research R-011; no ADR — the spec is the
  contract), and the e1000 network stack (`docs/NETWORK.md`, ADR-0018,
  research R-012 including the corrected QEMU loopback finding). The PE32+
  loader stage is governed by `docs/PE_LOADER.md` (the contract), ADR-0019
  (real-linker PE fixtures via rust-lld, fixed load base with always-on
  relocation, reject-by-directory policy, two-phase loader) and research
  R-013 (MS PE format spec + toolchain probe, now recorded as PASSED with the
  observed boot log). The stage-2 import-mediated Win32 console surface is
  governed by `docs/WIN32_CONSOLE.md` (the contract), ADR-0020 (`raw-dylib`
  imports without an import library, user-space stub page + `int 0x80`
  dispatch, HHDM IAT patch before protection, exact-name allowlist of three
  `kernel32.dll` functions, `,win32` marker) and research R-014 (MS PE import
  format + probe; PASSED with the observed stage-2 log and the two bring-up
  defects recorded). ADR-0006's framing still holds: serial is authoritative
  for tests, the framebuffer is a display mirror, not a gate.
- [docs/RESEARCH.md](docs/RESEARCH.md): primary sources, findings, confidence and
  open questions.
- [docs/MEMORY_MANAGEMENT.md](docs/MEMORY_MANAGEMENT.md): invariants, tests,
  benchmark methodology.
- [docs/GRAPHICS.md](docs/GRAPHICS.md) and
  [docs/WINDOWS_COMPATIBILITY.md](docs/WINDOWS_COMPATIBILITY.md): explicit
  non-claims and future architecture.

## Open defects and blockers

- VirtualBox BIOS boot is now verified on the default VM configuration. The
  root cause of the earlier stall was an IRQ0 pending at the PIC while LINT0
  was masked; the kernel now maps the APIC page and configures LINT0 ExtINT.
  VirtualBox EFI boot is still untested.
- UEFI boot has not been run (no OVMF firmware). The UEFI El Torito entry is
  verified; a catalog entry is not a boot test.
- No PCI driver model, no general Win32/API compatibility and no GPU
  acceleration. The PE32+ path is Phase 7 stages 1–2 only: it runs
  filesystem-less fixtures and resolves imports against a three-name
  `kernel32.dll` allowlist; see the stage-1/stage-2 limits bullets below.
  Networking exists as stage 5 (one polled e1000 with static addressing);
  there is no general on-disk filesystem: persistence is the
  whole-tree checkpoint image (stage 2) described above.
- Phase 7 stage-1 limits (spec: `docs/PE_LOADER.md` §1/§9, ADR-0019): PE32+
  (AMD64) only — no PE32, no DLLs/exports, no delay imports, TLS, CLR,
  resources, SEH/unwind (`.pdata` ignored) or dynamic code; images are Limine
  modules (no filesystem-backed exec); fixed 1 MiB code window at `0x400000`
  with the one-page stack at `0x500000`; max 16 sections / 256 DIR64
  relocations per load, W^X per section, relocation targets must not cross a
  page; a nonzero load delta requires a present, non-stripped relocation table
  and relocations are always applied against the fixed base (no ASLR).
  Imports existed only in stage 2 and only as below. The loader is not
  Windows API compatibility, and the claim boundaries in §9 stay in force.
- Phase 7 stage-2 limits (spec: `docs/WIN32_CONSOLE.md` §1/§9, ADR-0020):
  exactly three allowlisted `kernel32.dll` names (`GetStdHandle`,
  `WriteConsoleA`, `ExitProcess`), exact-name match with case-insensitive DLL
  compare; no real DLL loading, exports, forwarders, ordinals, delay imports,
  API sets or bound imports (rejected); one stub page at `0x510000` with one
  `int 0x80` stub per service; one console handle (`1`), 4-argument services,
  `MAX_WRITE` byte cap, one user thread at a time; no CRT startup, SEH, TLS or
  Unicode/W variants; the mechanism proves our own import path, not Windows
  compatibility. Stage-2 bring-up defects (import-count slice, unaligned
  `translate` writes) were fixed before the PASS and are recorded in R-014.
- Phase 6 stage-5 limits (spec/ADR: `docs/NETWORK.md` §10, ADR-0018): one
  polled e1000 on bus 0 with static `10.0.2.15/24`; no DHCP/DNS/TCP/sockets,
  no interrupt/MSI receive or background RX (packets are processed only while
  a command polls the ring), no routing beyond the local subnet, fragmented
  IPv4 rejected and `udpecho` payloads over 1024 B dropped. QEMU slirp is the
  network evidence; VirtualBox detected the NIC but its headless NAT did not
  answer the gateway ARP (non-fatal, no claim). QEMU's e1000 model implements
  only PHY `BMCR` loopback, so `net: loopback selftest: unsupported` is the
  expected QEMU/VirtualBox outcome (R-012 corrected during implementation).
- Stage-5 bring-up defect found and fixed by the end-to-end harness: the ICMP
  echo request/reply paths wrote the echo payload after the checksum had been
  computed, so every echo request was dropped (`net selftest: no gateway reply
  (echo timeout)`); ARP was unaffected because it carries no checksum. Writing
  the payload first fixed it and the selftest passed instantly (`icmp echo
  reply in 0 ms`). Details: `docs/NETWORK.md` §11.
- Phase 6 stage-2 limits (spec: `docs/PERSISTENCE.md`): ATA PIO touches only
  the PIIX primary master in LBA28 polled mode — no AHCI, virtio, DMA, IRQ14
  or real-hardware validation; evidence is QEMU `-machine pc` only, and the
  VirtualBox profile still has no disk attached. Saves are whole-tree
  snapshots at shell-command boundaries (double buffering gives single-write
  atomicity, not a journal); CRC32 is integrity checking, not security; a
  save transiently allocates roughly 1.1 MiB, and a disk larger than 8 MiB
  is used only for its first 4112 sectors.
- Defect found and fixed during stage-2 bring-up: the ATA drive/head select
  used `0xA0` (master, CHS mode), so QEMU translated the LBA registers
  through the disk geometry and both slots landed on wrong sectors while
  staying self-consistent (reads and writes agreed). Byte-level inspection
  of the raw image exposed it; the correct value is `0xE0` (master, LBA).
  The four-boot harness now asserts the exact slot offsets via payload CRCs.
- Phase 6 stage-1 limits: shell input is serial-only and the framebuffer
  console is output-only (no PS/2/mouse input, no ANSI, no per-line colours,
  no Unicode — uppercase-folded ASCII 5x7 font, integer scale only); no line
  history/job control; `write`/`append` are
  whole-file operations; the VFS is capped at 64 nodes / 8192 B per file and
  the shell's boot self-test leaves a seeded `/tmp` file. QEMU's stdio serial
  can overrun its emulated 16-byte FIFO on a large paste (counted in
  `RX_OVERRUNS`, visible via `console`); the automated session uses a paced
  TCP chardev instead.
- Phase 6 stage-4 limits (spec: `docs/FRAMEBUFFER_CONSOLE.md` §10): the grid is
  capped at 160×64 cells (larger modes stop after one automatic scale step);
  there is no scrollback history (serial logs remain the persistent record);
  boot lines printed before framebuffer validation and lines emitted while the
  console lock is held by a panicking path reach COM1 only (deliberate
  no-deadlock policy); the readback self-test is validated on QEMU std VGA and
  VirtualBox VBoxVGA, not on real hardware or a WC-mapped framebuffer (the
  `sfence` is the documented ordering guard, not an experiment on WC); one
  framebuffer consumer, no compositor.
- Two stage-4 bring-up defects were found and fixed by the readback self-test:
  the scratch buffer was sized as one pixel row instead of one cell row, so
  `fill_rect` rejected every cell (all pixels mismatched); and comparing the
  contiguous scratch render against the pitch-strided framebuffer misaligned
  after the first scanline. The final comparison is per scanline, and the
  self-test's diagnostic fields (row, offset, expected, actual) made the
  failures analyzable.
- User-mode limits (Phase 6 stage 3, extended by Phase 7 stages 1–2): programs
  are our own ELF64 `ET_EXEC` modules or PE32+ images loaded from Limine —
  the PE path resolves only the three stage-2 `kernel32.dll` names (see the
  stage-2 limits above), and neither path has PIE/dynamic linking, PT_INTERP
  or ASLR, and there is no filesystem-backed exec; the code
  window is fixed at 0x400000..0x500000 with a one-page stack at 0x500000.
  One user program runs at a time (the TSS `rsp0` is programmed for the sole
  user thread, so concurrent user threads are not supported); there is no
  fork/exec/wait, no guard page above the user stack, and no syscall returns
  user data yet; the syscall gate is `int 0x80` with a 4 KiB write cap and
  console-fd-only write. Skip paths print explicit skipped verdicts when
  modules are absent (not a pass).
- Stage-3 environment constraint (ADR-0017): any `build.rs` is unusable in
  this checkout — build-script executable paths exceed the Windows
  260-character `MAX_PATH` (MSVC `LNK1104`). Kernel link flags therefore stay
  in the root `.cargo/config.toml` and the userland workspace receives its
  flags via `RUSTFLAGS` in `scripts/build_user.sh` (cargo joins ancestor
  `target.*.rustflags` arrays, so the kernel's flags cannot be un-set
  per-crate).
- The kernel now runs on its own 4-level address space (CR3 switch verified in
  QEMU). The old bootloader tables are intentionally left allocated but unused;
  reclaiming bootloader-reclaimable pages is future memory work. There is no
  5-level paging support, and the older MMIO mapper still installs permanent
  leaves into whichever tables are live.
- The kernel heap is a fixed 8 MiB region; it does not grow on demand yet, and
  kernel-space page mapping (rather than HHDM access) is not wired up.
- `AddressSpace::destroy` is now exercised by the user-mode teardown;
  `map_region` remains staged (dead-code annotated) for the process work.
- Scheduler limits: no priority, sleep/wake, or SMP; thread slots are not
  reused after exit; preemption switches run on the interrupted thread's own
  stack (correct but means nested preemption depth is unbounded by design);
  the `hlt`-based join relies on the PIT still delivering ticks.
- The legacy PIC/PIT path uses LINT0 ExtINT and does not start the APIC timer,
  IOAPIC or SMP. Those remain future work.
- The full QEMU BIOS matrix (256 MiB / 512 MiB / 1 GiB) and VirtualBox were
  rerun for the framebuffer-console milestone; all reached the unchanged marker
  (the console adds serial lines, not a check) and VirtualBox logged its own
  `fbcon` sizing and selftest. UEFI boot remains unrun (no OVMF).
- No CI workflow is included. The remote `orgin` (github.com/Alex07lol/WinUx) is
  a manual push target; the scripts are CI-ready and a runner would need Rust,
  `mkisofs`, Python and QEMU.
- ISO bytes vary with timestamps; no `SOURCE_DATE_EPOCH`-pinned ISO yet.
- `IrqSpinLock` (and an interrupt-disabled serial sink) make interrupt-context
  sharing safe on a single CPU; the plain `SpinLock` remains and must not be
  used from code that can be preempted by an interrupt handler that takes the
  same lock.

## Single best next action

Phase 7 stage 2 is complete: the native Win32 console surface (import
directory, allowlist resolution, stub page, three services), fixture pipeline
and harnesses are implemented and verified (22 PASS / 0 FAIL / 4 SKIP; Win32
boot log in `artifacts/qemu-serial-pe.log`); commit and push `orgin/main`,
then the user selects the next stage. Candidate work, none of it started: a
wider Win32/NT surface (more console/file/process functions beyond the three
named ones, imports from more than one synthetic DLL), the remaining Phase 2–4
scope gaps (APIC timer/IOAPIC/SMP, recoverable exceptions, user
address-space hardening, heap growth), and Phase 9 (minimal PCI/driver
framework — which also unblocks AHCI — plus a virtual GPU target).
