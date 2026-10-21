# Progress

Last updated: **2026-10-20**

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
`pebadfn.exe`) are rejected before any allocation. **Phase 7 stage 3** —
Win32 program basics — is implemented and verified: the `kernel32.dll`
allowlist grows from three to eleven names (`WriteFile`, `GetLastError`,
`SetLastError`, `ExitThread`, `GetCurrentProcessId`, `GetCurrentThreadId`,
`GetTickCount`, `GetCommandLineA` join the console surface), the stub page
gains a 13-byte five-argument form (`mov r10, [rsp+0x28]`) and a
loader-owned NUL-terminated command-line tail, per-thread `last_error` lives
in the scheduler thread slot, and a new `pebasics.exe` fixture with nine
`raw-dylib` imports exercises the documented `WriteFile` failure values
(0 + `ERROR_INVALID_HANDLE` / 0 + `ERROR_INVALID_PARAMETER`), error-state
persistence across a successful call, the synthetic IDs, a wrap-safe
`GetTickCount` pair and the loader-owned command line at CPL3; the
`pebadfn2.exe` mutant (`Sleep` substituted for `GetTickCount`) is rejected
before any allocation. After the user reviewed this evidence, **Phase 7
acceptance was marked complete** in `docs/ROADMAP.md` (2026-10-19). The
remaining Phase 7-adjacent scope (a broader API surface, imports from real
system DLLs, ordinals, forwarders, delay imports, SEH/unwind, TLS, CLR,
PE32) is explicitly outside the phase and not started; there is still no
Windows application compatibility.
**Phase 8 stage 1** — a native registry-like configuration store — is
implemented and verified: a host-tested `os_core::config` codec (`POCF`
version 1: 8-byte header, ≤64 entries, path-shaped ASCII keys ≤96 bytes,
printable values ≤128 bytes, blob ≤7984 bytes) serialises the store into the
ordinary VFS file `/etc/config`, so the checkpoint format, slots, CRCs and
fallback rule are untouched (`docs/CONFIG.md`, ADR-0022). Boot decodes the
file when the checkpoint carries it and otherwise seeds `System/Name`,
`Display/Theme`, `Net/Mode` in memory; the first shell mutation writes the
file and the existing command-boundary flush persists it. The shell gains
`config [get|set|del|list]`, the boot self-test logs
`config selftest: passed (...)` and the marker gains `,config` after `,vfs`.
The persistence harness now also proves theme survival across reboots, theme
rollback when the newest slot is corrupt, and defaults after a fresh format.
This is not a Windows registry: no `advapi32`/`HKEY_*` API, no hives, no
typed values (limits: `docs/CONFIG.md` §8).
**Phase 8 stage 2** — Win32 console input — is implemented and verified:
`GetStdHandle(-10)` now serves the distinct input handle `2`; service 14
(`ReadFile`, the existing five-argument stub, allowlist grows from eleven to
twelve names) blocks inside the `int 0x80` handler with interrupts enabled and
`hlt` (no lock held), consumes serial-console bytes one at a time, echoes
printable ASCII with backspace/DEL editing, terminates on CR/LF with a
CRLF-inclusive line and count, zeroes `*lpNumberOfBytesRead` before any
check, and returns the documented `FALSE` + WinError values (`0`+6 bad
handle, `0`+87 overlapped or length outside `2..=4096`); unmapped pointers
kill the thread like the write services (`docs/WIN32_INPUT.md`, ADR-0023,
research R-016). A genuine `peinput.exe` fixture (five `kernel32.dll`
imports, distinct exit codes per assertion) ships only on
`artifacts/project-os-input.iso` (`make_iso.sh --input-fixture`); the default
and interactive ISOs, their module counts, serial assertions and all markers
are byte-identical to stages 1–3. The paced TCP-serial harness
(`scripts/test_input_qemu.sh`) types one known line while the fixture is
blocked and asserts echo, playback, `ReadFile(2, 64 bytes) -> 1 (14 bytes)`,
the error lines and the self-test line; the full regression stayed green
(23 PASS / 0 FAIL / 4 SKIP). Limits: line mode with echo only — no
`ReadConsole`, console modes, Ctrl+C/Ctrl+Z semantics, raw mode, Unicode/W
variants, redirected stdin, overlapped/async I/O or multiple readers; CRLF
inclusion is our documented contract, not vendor-verified (`docs/
WIN32_INPUT.md` §1/§7).
**Phase 8 stage 3** — a Win32 registry read surface — is implemented and
verified: the import allowlist becomes two DLLs (`kernel32.dll` +
`advapi32.dll`) and fifteen names, adding services 15–17 (`RegOpenKeyExA`,
`RegQueryValueExA` with the first six-argument stub form forwarding `lpcbData`
through `r11`, and `RegCloseKey`) served by a case-insensitive, read-only
registry-shaped view (`kernel/src/registry.rs`): `HKLM` only, four handle
slots (`0x1000 + slot`), values as NUL-terminated REG_SZ bytes, the
documented error values (2/4/6/87/234) reported without killing the thread,
per-program reset (`docs/WIN32_REGISTRY.md`, ADR-0024, research R-017). A
genuine `pereg.exe` fixture (two DLLs, five imports, distinct exit codes
1–20) ships only on `artifacts/project-os-registry.iso`
(`make_iso.sh --registry-fixture`); the default and input ISOs boot to the
unchanged markers and serial contract (the registry fixture's absence now
prints explicit skip verdicts). The registry harness
(`scripts/test_registry_qemu.sh`) boots that ISO and asserts the full session
(handle slot reuse, size query, `ERROR_MORE_DATA`, `"PROJECT_OS"`/`"classic"`
reads, slot exhaustion, double close, 33-byte summary, exit 33); the full
regression stayed green (24 PASS / 0 FAIL / 4 SKIP). Limits: read-only — no
hives, ACLs, transactions, redirection, enumeration, write APIs, default
values, other roots, typed values beyond REG_SZ or `W`/Unicode variants; four
concurrent handles and always-NUL-terminated values are our documented
contract, not vendor-verified (`docs/WIN32_REGISTRY.md` §13).
**Phase 8 stage 4** — a Win32 registry write surface — is implemented and
verified: the two-DLL allowlist grows from fifteen to eighteen names and
services 18–20 (`RegCreateKeyExA` with the first nine-argument stub, its
seventh–ninth arguments forwarded through `r12`–`r14` and the fixed stub
stride grown 24 → 48 bytes; `RegSetValueExA` through the existing six-argument
shape; `RegDeleteValueA` through the four-argument shape) mutate the same
`/etc/config` store the shell `config` builtin uses and persist through the
existing `config::save()`. Created keys report `REG_CREATED_NEW_KEY` (1) when
the prefix had no children before the call and `REG_OPENED_EXISTING_KEY` (2)
whenever children existed (a key whose last value is deleted ceases to exist);
writes validate joined store paths (87) while reads keep their stage-3
simply-miss behavior; REG_SZ only, NUL-terminated, capped by the store, with
the documented error values (2/6/8/87) and the first nine-argument stub
(`Args9`). A genuine `peregw.exe` fixture (two DLLs, seven imports,
distinct exit codes 1–18) ships only on
`artifacts/project-os-registry-write.iso`
(`make_iso.sh --registry-write-fixture`); the default and other variant ISOs
boot to the unchanged markers and serial contract (the fixture's absence
prints explicit skip verdicts). The harness
(`scripts/test_registry_write_qemu.sh`) boots that ISO and asserts the full
session (create dispositions `created`/`opened existing`/`created`, REG_SZ
sets and updates with read-back, 87/6/2 error lines, delete-then-query → 2,
slot reuse, `Persisted` write, 39-byte summary, exit 33); the boot phase also
proves persistence by saving `/etc/config`, reading it back through the VFS
and decoding it with `os_core::config::Config::decode`, then restoring the
defaults. The full regression stayed green (25 PASS / 0 FAIL / 4 SKIP).
Limits: no hives, ACLs, access checks, transactions, enumeration, key
deletion (`RegDeleteKey`), typed values beyond REG_SZ, default/null values or
namespace rules; four concurrent handles and the 64-entry store remain our
documented contracts, not vendor-verified (`docs/WIN32_REGISTRY_WRITE.md`
§14, ADR-0025, research R-018).
**Phase 8 stage 5** — Win32 registry enumeration and key deletion — has its
contract and design recorded (2026-10-20) and is **not implemented yet**:
`docs/WIN32_REGISTRY_ENUM.md` pins services 21–23 (`RegDeleteKeyA` via the
new two-argument stub, `RegEnumKeyExA`/`RegEnumValueA` via the new
eight-argument stub inside the unchanged 48-byte stride), deterministic
insertion-order enumeration over the flattened namespace, the fetched
non-recursive delete rule (child keys → 5, so callers enumerate and delete
children first), immediate removal with stale open handles (stated
deviation) and the buffer contracts (234 with the required size, 259 at the
end). ADR-0026 and R-019 record the decisions and fetched primary sources;
the planned `peregen.exe` fixture and `--registry-enum-fixture` ISO are
specified but not built.

This is **not** a Windows-compatible OS, not a desktop, and not a GPU-capable OS.

**Phase 8 stage 5** — Win32 registry enumeration and non-recursive key
deletion — is implemented and verified: services 21–23 (`RegDeleteKeyA` via a
two-argument stub, `RegEnumKeyExA`/`RegEnumValueA` via the new eight-argument
stub loading `r10`–`r13`) enumerate child keys and REG_SZ values of the same
config store and delete keys under the fetched non-recursive rule
(234/259/5/6/2/87), with the store's canonical ascending order as the stated
deterministic deviation from Windows' arbitrary order. A genuine
`peregen.exe` fixture ships only on
`artifacts/project-os-registry-enum.iso`; the harness PASS and
`test_all.sh` PASS=26 FAIL=0 SKIP=4 are recorded below
(`docs/WIN32_REGISTRY_ENUM.md`, ADR-0026, R-019).

**Phase 9 stage 1** — PCI bus-0 inventory — is implemented and verified: every
present bus-0 function is recorded with vendor/device, class, header type and
all implemented BARs (sized by all-ones write-back and restored immediately,
64-bit BARs consuming two slots, 64-function cap failing the self-test on
truncation), printed at boot, exposed through the shell `pci` command and
marked in the boot checks as `,pci`; the e1000 locator now reads the inventory
with a direct-scan fallback. A new q35 harness pins the six-function/seven-BAR
layout, host bridge/LPC and e1000 BAR0 (`docs/PCI.md`, ADR-0027, R-020).

**Phase 9 stage 2** — driver registration and binding model — is implemented
and verified: `os_core::driver` provides exact vendor/device and class match
rules, compile-time descriptors, deterministic first-match binding in
descriptor order, optional single-instance drivers, caps (8 descriptors,
64 bindings) and explicit `Bound -> Running -> Stopped` / `-> Failed(reason)`
transitions with illegal transitions rejected. The kernel registers the real
e1000 descriptor, reports its `Bound` binding over the PCI inventory plus the
unmatched count and a structural self-test, and exposes `drivers` in the
shell; no hardware is started or stopped through the model yet. Evidence:
10 new host tests (suite 280/280), `scripts/test_driver_model_qemu.sh` PASS
and `scripts/test_all.sh` PASS=28 FAIL=0 SKIP=2 (`docs/DRIVER_MODEL.md`,
ADR-0028, R-021).

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
  services)`), pushed (`b8a2910..b19670f`) together with its progress
  follow-up `ec04e74` (`progress: record phase7 stage 2 commit and push`).
  The Phase 7 stage-3 commit is `8045d2b` (`phase7: win32 program basics
  (five-arg writefile stub, per-thread last-error, eleven-name kernel32
  allowlist, loader-owned command line)`); it is pushed (`ec04e74..8045d2b`)
  together with its progress follow-up `68b3cc9` (`progress: record phase7
  stage 3 commit and push`). After the user reviewed the stage-3 evidence,
  `docs/ROADMAP.md` was updated to mark Phase 7 acceptance complete; that
  edit (with this progress update) is pushed after `68b3cc9`, same
  commit-then-record pattern as `f3b17a3`, `5467bde`, `25606ad`, `b8a2910`
  and `68b3cc9`.
  The Phase 8 stage-1 commit is `e7ecdc4` (`phase8: native configuration store
  (pocf codec, /etc/config on the checkpoint, config shell builtin)`), 14
  files changed; it is pushed (`82bc098..e7ecdc4`) together with its progress
  follow-up `58f98e4` (`progress: record phase8 stage 1 commit and push`).
  The Phase 8 stage-2 commit is `0ec7a59` (`phase8: win32 console input
  (blocking line-mode ReadFile service, distinct input handle, paced-serial
  fixture and harness)`), 20 files changed; it is pushed
  (`58f98e4..0ec7a59`) together with its progress follow-up (this commit),
  same commit-then-record pattern as `f3b17a3`, `5467bde`, `25606ad`,
  `b8a2910`, `68b3cc9`, `e7ecdc4` and `58f98e4`. (An earlier attempt created
  `a7b35f6` with a truncated message and was amended before any push; the
  remote never saw it.)
  The Phase 8 stage-3 commit is `a07cf73` (`phase8: win32 registry read
  surface (advapi32.dll RegOpenKeyExA/RegQueryValueExA/RegCloseKey over the
  config store, two-DLL allowlist, six-argument stub, pereg fixture and
  harness)`), 23 files changed; it is pushed (`b1b8542..a07cf73`) together
  with this progress follow-up (this commit), same commit-then-record pattern
  as `f3b17a3`, `5467bde`, `25606ad`, `b8a2910`, `68b3cc9`, `e7ecdc4`,
  `58f98e4` and `b1b8542`.
  The Phase 8 stage-4 commit is `30a7f09` (`phase8: win32 registry write
  surface (advapi32.dll RegCreateKeyExA/RegSetValueExA/RegDeleteValueA
  persisting through the config store, eighteen-name allowlist, nine-argument
  stub, peregw fixture and harness)`), 21 files changed; it is pushed
  (`fe3c278..30a7f09`) together with this progress follow-up (this commit),
  same commit-then-record pattern as the earlier stages.
  The Phase 8 stage-5 design commit is `3686a58` (`docs: phase8 stage 5
  registry enumeration/deletion contract`), four files changed; it is pushed
  (`30a7f09..3686a58`) together with this progress follow-up (this commit),
  same commit-then-record pattern.  The Phase 8 stage-5 implementation commit is fc09d78 (`phase8: win32 registry
  enumeration and non-recursive key deletion ...`), pushed together with this
  progress follow-up (`21bfe98..<PENDING>`), same commit-then-record pattern; the
  design commit `3686a58` and its progress record `21bfe98` were pushed
  earlier (`11d4fa6..21bfe98`).
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
- Phase 7 stage 3 — kernel resolver (`kernel/src/pe_loader.rs`): the
  `WIN32_SERVICES` allowlist grows to eleven exact names (service numbers
  3–13); the stub planner emits the five-argument form for `WriteFile`
  (`4C 8B 54 24 28` = `mov r10, [rsp+0x28]`, then `B8 <service>` `CD 80`
  `C3` = `mov eax, imm32; int 0x80; ret`, 13 bytes + 3 `nop` bytes to the
  16-byte stride) and keeps the stage-2 8-byte form for the rest; `map`
  also writes the NUL-terminated module path at
  `COMMAND_LINE_VA = WIN32_STUB_VA + 0xF00` (explicit terminator — the frame
  is `int3`-filled) and rejects paths over 255 bytes with the new
  `Reject::CommandLineTooLong`. Contract: spec `docs/WIN32_PROGRAM.md`,
  decision ADR-0021, research R-015.
- Phase 7 stage 3 — kernel services (`kernel/src/syscall.rs`, services 6–13):
  `WriteFile` (validated buffer/out-pointer spans, `MAX_WRITE` cap, documented
  failure values `ERROR_INVALID_HANDLE=6` / `ERROR_INVALID_PARAMETER=87`,
  overlapped always invalid, success does not clear `last_error`),
  `GetLastError`/`SetLastError` (per-thread slot), `ExitThread` (scheduler
  exit path), `GetCurrentProcessId`/`GetCurrentThreadId` (synthetic `1`/`1`),
  `GetTickCount` (PIT ticks × 10 as `u32`), `GetCommandLineA` (validates and
  copies out the loader-written path); counters
  `WIN32_WRITEFILE_CALLS`/`WIN32_GET_LAST_ERROR_CALLS`/
  `WIN32_SET_LAST_ERROR_CALLS`/`WIN32_EXIT_THREAD_CALLS`/
  `WIN32_GET_CURRENT_PROCESS_ID_CALLS`/`WIN32_GET_CURRENT_THREAD_ID_CALLS`/
  `WIN32_GET_TICK_COUNT_CALLS`/`WIN32_GET_COMMAND_LINE_CALLS`. `sched.rs`
  gains per-thread `last_error: u32` with `current_last_error()`/
  `set_current_last_error()`; `user.rs` gains the stage-3 phase (skip line,
  the `user-win32-prog` thread, stub-page teardown, counter-delta assertions
  incl. `WRITE_CALLS` +2 / `BYTES_WRITTEN` +59) and the `win32_program`
  flag; `main.rs` gates the verdict; the `fatal()` helper now renders the
  whole message into a stack buffer with interrupts disabled before halting
  (bring-up finding).
- Phase 7 stage 3 — fixtures (`userland/pe/pebasics`, workspace member
  added): `#![no_std]`, nine `raw-dylib` imports, the pinned 12-step error
  ladder (`SetLastError(42)` → success keeps 42 → bad handle 6 → overlapped
  87 → IDs → tick pair → exact command line → `SetLastError(0)` → second
  write → `ExitThread(0)`); `scripts/build_user_pe.sh` builds it, invokes
  `check_user_pe.py` with the exact import set, and generates
  `pebadfn2.exe` (`GetTickCount` → `Sleep` pattern patch, count-checked).
- Phase 7 stage 3 — pipeline and harness: `boot/limine.conf` and
  `scripts/make_iso.sh` stage seven `.exe` modules; `scripts/test_pe_qemu.sh`
  asserts `pe: 7 modules present`, the full stage-3 line block, the pinned
  `pe: pebasics.exe loaded at 0x400000 (entry 0x401000, 5 sections, 1
  relocation applied)`, both stage-3 rejection/verdict lines and the
  unchanged marker tokens. `scripts/test_persistence_qemu.py`'s boot-1
  "no slot-invalid line" check is now scoped to `persist: slot A/B invalid`
  (a bare `invalid` search misread the new, legitimate
  `(invalid handle)`/`(invalid parameter)` lines — test-side fix, kernel
  output unchanged).
- Phase 7 stage 3 — bring-up defects found by the fixture/harness and fixed
  (no test loosened): (1) the stub-page tail is `int3` (0xCC)-filled, not
  zeroed, so the command-line terminator is now written explicitly
  (`GetCommandLineA` had killed the thread with "string is not terminated");
  (2) `sched::wait_all_finished` dropped its TSC-frequency budget — under
  QEMU the Limine-reported frequency disagrees with the real `rdtsc` rate,
  so the old `frequency × 2` bound expired while the stage-3 program was
  still completing syscalls and `main::fatal` + `halt_forever` froze the
  machine mid-program; the wait is now bounded by PIT ticks only
  (`sched::demo()`/`user::demo()` signatures lost the parameter). Both are
  recorded in R-015 and ADR-0021.
- Phase 8 stage 1 — host-tested codec (`os_core/src/config.rs`, module added
  to `lib.rs`): `POCF` version 1 with `MAGIC`/`VERSION`/`HEADER_BYTES`
  constants, caps (`MAX_ENTRIES=64`, `MAX_KEY_BYTES=96`, `MAX_VALUE_BYTES=128`,
  `MAX_BLOB_BYTES=7984`, component ≤32 reuse of `path::MAX_COMPONENT`),
  key/value validation (`key_is_valid`, `value_is_valid`), sorted `Config`
  (`get`/`set`/`remove`/`contains`/`list(prefix)`), canonical key-sorted
  `encode`, validating `decode` (magic/version/truncation/trailing/count/key/
  value/duplicate/blob-cap rows with distinct `label()` strings) and 17 unit
  tests (validation tables, caps incl. blob boundary, sorted insert/overwrite/
  remove, prefix listing, canonical encode, round trips, empty store,
  unsorted-input acceptance, every decode rejection, stable labels).
- Phase 8 stage 1 — kernel store (`kernel/src/config.rs`, new):
  `IrqSpinLock<Option<Config>>`, `init()` loads `/etc/config` when present or
  seeds `System/Name=PROJECT_OS`, `Display/Theme=classic`, `Net/Mode=static`
  in memory (no boot-time write), `save()` encodes and writes the file through
  the VFS (dirty bit drives the existing flush), `self_test()` round-trips a
  temporary `SelfCheck/Value` entry and error paths without touching the file,
  plus `get`/`set`/`remove`/`list`/`status` helpers for the shell. `main.rs`
  wires `mod config`, `config::init()` + `config::self_test()` after
  `persist::init()` (verdict line `config selftest: passed (...)`) and pushes
  `,config` onto the checks string directly after `,vfs`.
- Phase 8 stage 1 — shell surface (`kernel/src/shell.rs`): the `config`
  builtin with `get`/`set`/`del`/`list` (list takes an optional path prefix
  and prints `{n} entries in config:` / `{n} entries matching {p}:` plus
  `  {key} = {value}` lines), `set`/`del` call `config::save()`, error paths
  print through the existing `error: {}` wrapper (`error: no such key`,
  `error: bad key`), usage lines pinned in the spec, one new `help` line.
- Phase 8 stage 1 — harnesses: `scripts/test_shell_qemu.py` grows from 17 to
  25 commands (the eight config commands incl. the `bad//key` rejection);
  `scripts/test_persistence_qemu.py` gains `MARKER_SUFFIX =
  vfs,config,blk,persist` and the configuration assertions in all four boots
  (boot 1 `config set` → A seq=2; boot 2 loads `light`, then `dark` → B
  seq=5; boot 3 corrupts B, falls back to A seq=4 with the theme rolled back
  to `light` and `/tmp/notes/a.txt` present; boot 4 defaults `classic` after
  the fresh format). No bring-up defect was found in this stage; two host-test
  iterations during development (an initial 6-byte header assumption corrected
  to the 8-byte header, and a cap-boundary assertion refined so it tests the
  actual invariant) were fixed before any artifact was recorded.
- Phase 8 stage 2 — syscall surface (`kernel/src/syscall.rs`):
  `STD_INPUT_HANDLE = -10`, `INPUT_HANDLE = 2`, `SYS_READFILE = 14`,
  `MAX_READ = 4096`, `WIN32_READFILE_CALLS` counter, the `-10` branch in
  `get_std_handle` (pinned line `win32: GetStdHandle(-10) -> 2`), the service
  dispatch arm and `read_file()`: out-count page validated writable then
  zeroed, handle/overlapped/length checks with the documented lines and
  last-errors (6 / 87), buffer span validated before waiting, then the
  blocking loop (`interrupts_enable()` + `halt()`, no lock held) consuming
  the serial RX ring byte by byte, echoing printables and `\x08 \x08` for
  backspace/DEL, appending CRLF at CR/LF with at most `length - 2` content
  bytes. `kernel/src/pe_loader.rs`: the allowlist grows from eleven to
  twelve names (`ReadFile`, service 14, `Args5`); stub page/loader untouched.
- Phase 8 stage 2 — fixture (`userland/pe/peinput`, workspace member added):
  five `raw-dylib` imports, distinct exit codes 1–8 for each assertion
  (std handle, invalid-handle result/count, last-error 6, overlapped 87,
  blocking read failure, CRLF structure, playback, surface line),
  `const BUFFER_LEN: usize = 64` static buffer (a `static_mut_refs` error was
  fixed during bring-up), playback via `WriteFile` and
  `win32: input surface verified` (30 bytes) before `ExitThread(0)`.
- Phase 8 stage 2 — pipeline and harnesses: `scripts/build_user_pe.sh` builds
  and checker-verifies `peinput.exe` (eight artifacts, hash log);
  `scripts/make_iso.sh` gains `--input-fixture` (rejects `--interactive`,
  output `artifacts/project-os-input.iso`, `debug_exit` kept) and injects the
  `peinput.exe` module line into the staged `limine.conf` with a grep guard —
  the static module list in `boot/limine.conf` is copied as-is, so a variant
  module must be injected (found when the first input run booted without the
  module; `sed` + verify fix). The new `scripts/test_input_qemu.py`/`.sh`
  drive the session (wait line → paced typed line → marker → exit-33);
  `scripts/test_all.sh` gains the `input` step after `shell`. Cosmetic fix:
  `make_iso.sh`'s build log printed the interactive note for every variant
  (`${INTERACTIVE:+...}` is non-empty for `0`); it now uses explicit
  `-eq 1` conditionals. `user.rs` gains `PEINPUT_PATH`/`thread_peinput`, the
  stage-4 probe block (skip when the module is absent; 30 s window;
  counter deltas `GetStdHandle +1`, `ReadFile +3`, `GetLastError +2`,
  `WriteFile +2`, exit 0, no faults) and the verdict line.
- Phase 8 stage 2 — bring-up defects (harness-side, fixed before the PASS):
  (1) the echo/playback expectation ignored the serial layer's LF→CRLF
  expansion, so the transcript shows `hello, win32\r\r\n` while the buffer
  holds 14 bytes ending CRLF — the pinned expectation and
  `docs/WIN32_INPUT.md` now record the display artifact; (2) the marker
  checks ran as soon as `wait_for` matched the marker *prefix*, before the
  rest of the line arrived — the harness now waits for the marker line's
  terminator. Also fixed during lint: clippy `manual_range_contains` in
  `read_file` (now `!(2..=MAX_READ).contains(&length)`), `static_mut_refs`
  in the fixture (`BUFFER_LEN` const), and a dead `CONSOLE_HANDLE` warning
  (now used by the fixture's `WriteFile` calls). No kernel defect was found;
  the first boot passed on unmodified kernel code.
- Phase 8 stage 3 — registry view (`kernel/src/registry.rs`, new): four-slot
  `HKLM` handle table (`0x1000 + slot`, `MAX_KEYS = 4`), case-insensitive
  ASCII prefix/value lookups over the config store (`value()` joins
  `key/value` with `/`), `KeyError::{InvalidHandle,NotFound,TooManyKeys}` and
  `reset()` called from `user.rs` teardown.
- Phase 8 stage 3 — syscall surface (`kernel/src/syscall.rs`): services 15–17
  (`SYS_REG_OPEN_KEY_EX`, `SYS_REG_QUERY_VALUE_EX`, `SYS_REG_CLOSE_KEY`),
  `WIN32_REG_{OPEN_KEY,QUERY_VALUE,CLOSE_KEY}_CALLS` counters,
  `read_user_string` (128-byte cap, NULL → empty),
  `validated_out_pointer_u64`, `RegHandle` display (`0x80000002`, slot
  numbers) and `reg_error_label`; the handlers follow the documented spec
  order (`docs/WIN32_REGISTRY.md` §5/§8). Clippy `manual_range_contains`
  fixed during lint (`!(0x1000..USER_LIMIT).contains(&pointer)`).
- Phase 8 stage 3 — loader (`kernel/src/pe_loader.rs`): `WIN32_DLLS` grows
  to two entries (`kernel32.dll`, `advapi32.dll`), `WIN32_SERVICES` to
  fifteen with a per-entry DLL, map dedupes by `(dll, name)`,
  `StubShape::Args6` added (`mov r11, [rsp+0x30]` after the Args5
  `mov r10, [rsp+0x28]`), `STUB_STRIDE` 16 → 24 with a const assert that
  `MAX_IMPORTS as u64 * STUB_STRIDE <= COMMAND_LINE_OFFSET as u64`.
- Phase 8 stage 3 — fixture (`userland/pe/pereg`, workspace member added):
  two `raw-dylib` DLLs and five imports, the §11 twenty-step sequence with
  distinct exit codes 1–20; a relocation marker probe confirms the applied
  base delta before the first API call; summary line
  `win32: registry surface verified` (33 bytes) then `ExitThread(0)`.
- Phase 8 stage 3 — pipeline and harnesses: `scripts/build_user_pe.sh`
  builds and checker-verifies `pereg.exe` (nine artifacts; the pre-existing
  eight hashes unchanged); `scripts/check_user_pe.py` accepts
  semicolon-separated multi-DLL `--imports` groups; `scripts/make_iso.sh`
  gains `--registry-fixture` (three-way mutual exclusion with the other
  variants, output `artifacts/project-os-registry.iso`, module injection
  with grep guard); new `scripts/test_registry_qemu.py`/`.sh`;
  `scripts/test_all.sh` gains the `registry` step after `input`; `user.rs`
  gains `PEREG_PATH`/`thread_pereg`, the registry phase block (counter
  deltas open +10, query +7, close +7, `WriteFile` +1, writes +1, bytes +33,
  exit 0, no faults; 300-tick window) with pass/skip verdict lines, and
  `registry::reset()` in teardown.
- Phase 8 stage 3 — bring-up defects (harness-side, fixed before the PASS):
  the registry harness's size-query expectation missed the serial layer's
  LF→CRLF expansion (`\r\r\n` display terminator; same class as the stage-2
  harness defect), and the first `pe` regression booted the stale default
  ISO built before the stage-3 kernel — rebuilt and re-ran PASS with build
  id `0.1.0-b1b8542c`. Two spec-text reconciliations:
  `docs/WIN32_REGISTRY.md` §5/§8 now state the failure-sets-last-error
  contract explicitly, and §7's key-existence rule is children-only. No
  kernel defect was found; the registry session passed on the first kernel
  build.
- Phase 8 stage 4 — registry write API (`kernel/src/registry.rs`): `KeyError`
  extended to `{InvalidHandle, NotFound, TooManyKeys, BadArgument, NoSpace}`,
  a `joined_entry` helper validating `prefix/name` against the store path
  rules, `write_error` mapping store failures (`BadArgument` → 87, `Full` →
  8), `lookup_value` refactored over the new helper, `create(parent,
  subkey) -> (u64, bool)` (disposition from the children count before the
  call; empty subkey returns a new slot handle and always `opened existing`),
  `set_value(parent, name, value)` and `delete_value(parent, name)`; every
  mutation runs under `config::with`, then `config::save()` is called after
  the lock is released.
- Phase 8 stage 4 — syscall surface (`kernel/src/syscall.rs`): services 18–20
  (`SYS_REG_CREATE_KEY_EX`, `SYS_REG_SET_VALUE_EX`, `SYS_REG_DELETE_VALUE`),
  `WIN32_REG_{CREATE_KEY,SET_VALUE,DELETE_VALUE}_CALLS` counters,
  `ERROR_NOT_ENOUGH_MEMORY = 8`, `REG_CREATED_NEW_KEY = 1`,
  `REG_OPENED_EXISTING_KEY = 2`, `MAX_REG_VALUE = MAX_VALUE_BYTES + 1 = 129`,
  `reg_key_error_code`, `reg_error_label` gains "not enough memory",
  handlers `reg_create_key_ex` (Args9: `r13` = `phkResult`, `r14` =
  `lpdwDisposition`, `samDesired`/`lpSecurityAttributes`/`lpClass` ignored),
  `reg_set_value_ex` and `reg_delete_value`, the shared `read_user_reg_value`
  (span validated per page, byte-capped, final NUL required, payload through
  `os_core::config::value_is_valid`) and `reg_*_fail` helpers that set the
  thread `last_error`.
- Phase 8 stage 4 — loader (`kernel/src/pe_loader.rs`): `StubShape::Args9`
  (after the Args6 loads, `r12`/`r13`/`r14` from `[rsp+0x38]`/`[rsp+0x40]`/
  `[rsp+0x48]`; `isr_common` already saves those registers), `STUB_STRIDE`
  24 → 48 with the existing compile-time assert, `WIN32_SERVICES` grows to
  eighteen with the three new `advapi32.dll` entries.
- Phase 8 stage 4 — fixture (`userland/pe/peregw`, workspace member added):
  two `raw-dylib` DLLs and seven imports, the §12 eighteen-step sequence
  (create → set/update → repeat create → error paths → delete → slot reuse →
  `Persisted` write → close → 39-byte summary) with distinct exit codes 1–18;
  summary line `win32: registry write surface verified` (39 bytes) then
  `ExitThread(0)`.
- Phase 8 stage 4 — pipeline and harnesses: `scripts/build_user_pe.sh`
  builds and checker-verifies `peregw.exe` (ten artifacts; the pre-existing
  nine hashes unchanged); `scripts/make_iso.sh` gains
  `--registry-write-fixture` (four-way mutual exclusion, output
  `artifacts/project-os-registry-write.iso`, module injection with grep
  guard); new `scripts/test_registry_write_qemu.py`/`.sh` (pinned
  create/set/delete/query lines, counted `RegCloseKey(4097) -> 0` twice, the
  `\r\r\n` size-query terminator, `win32 registry write selftest: passed`,
  marker, exit 33); `scripts/test_all.sh` gains the `registry-write` step
  after `registry`; `user.rs` gains `PEREGW_PATH`/`thread_peregw`, the write
  phase block (counter deltas create +3, set +8, delete +3, query +3, close
  +3, `WriteFile` +1, writes +1, bytes +39, exit 0, no faults) and the
  persistence proof: `Session/Current/Persisted` is saved through the store,
  `/etc/config` is read back through the VFS, decoded with
  `os_core::config::Config::decode` and asserted, then the default entries
  are restored before teardown.
- Phase 8 stage 4 — bring-up defect (fixture-side, fixed before the PASS):
  the first harness run failed because the fixture repeated the create
  *before* the first write; under the lazy children-only model a key with no
  values yet honestly reports `created`, not `opened existing`. The fix
  swapped fixture steps 2 and 3 (write `Flag` before the repeat create) and
  `docs/WIN32_REGISTRY_WRITE.md` §12 records the ordering rule; the rerun
  matched every pinned line. No kernel defect was found; the session passed
  on the fixed fixture with the same kernel build.

- Phase 8 stage 5 — registry backend (`kernel/src/registry.rs`):
  `KeyError::NoMoreItems`/`AccessDenied`, `entries_under`/`child_names`
  (canonical store order, case-insensitive dedup), `enum_key`, `enum_value`,
  `delete_key` (non-recursive; saves through `config::save()`), unit-simple
  prefix logic exercised by the PE harness.
- Phase 8 stage 5 — syscall surface (`kernel/src/syscall.rs`): services 21–23,
  counters, allowlist names, `ERROR_ACCESS_DENIED` (5)/`ERROR_NO_MORE_ITEMS`
  (259), user-buffer helpers (reserved/size 0 → 87, short → 234 with len+1,
  success size excl NUL, class/time ignored, REG_SZ enforced), dispatch and
  labels; `RegSetValueExA` log prints payload length excluding NUL.
- Phase 8 stage 5 — loader (`kernel/src/pe_loader.rs`): `StubShape::Args2`
  and `Args8` (r10–r13 from `[rsp+0x28]`–`[rsp+0x40]`), allowlist 21 names.
- Phase 8 stage 5 — runtime bring-up fixes: `arch::x86_64::enable_sse()`
  (CR0.EM/TS clear, CR4.OSFXSR|OSXMMEXCPT) before user mode because rustc
  emits `movaps`; `PE_STACK_TOP = STACK_TOP - 8` so the PE entry observes
  Win64 `rsp % 16 == 8`. No FXSAVE context switching exists yet.
- Phase 8 stage 5 — fixture (`userland/pe/peregen`, seventh workspace member,
  eleven artifacts): 34-step sequence with distinct exit codes, 45-byte
  summary `win32: registry enumeration surface verified`, handle reuse and
  stale-handle checks, nine-call delete sequence ending with
  `DeleteKey(HKLM, "Session/Current") -> 0`, then close/write; kernel phase
  asserts create +4, set +4, delete-key +9, enum-key +8, enum-value +6,
  query +1, close +4, WriteFile +1, write +1, 45 bytes, exit 0, no faults,
  `Session` empty and the persisted config decoded back to defaults.
- Phase 8 stage 5 — pipeline/harness: `build_user_pe.sh` (peregen import
  check), `make_iso.sh --registry-enum-fixture` (registry-enum ISO),
  `scripts/test_registry_enum_qemu.{py,sh}` (pinned transcript, exit 33),
  `test_all.sh` step `registry-enum`.

- Phase 9 stage 1 — host helper (`os_core/src/pci.rs`): `header_type` splits
  the 7-bit header type and multifunction bit; unit-tested (suite now 270).
- Phase 9 stage 1 — kernel inventory (`kernel/src/pci.rs`): `Function`/`BarInfo`
  records, multifunction-aware bus-0 scan, all-ones BAR probe with restore,
  type 0/1/2 BAR counts, 64-bit slot handling, 64-function cap + truncation
  flag, invariant self-test, boot printer, shell `pci`, inventory-backed
  `find_e1000` (fallback scan retained).
- Phase 9 stage 1 — boot/shell integration (`kernel/src/main.rs`,
  `kernel/src/shell.rs`): `pci::init()` before networking, `,pci` marker check,
  `pci` help/command.
- Phase 9 stage 1 — harness: `scripts/test_pci_qemu.sh` boots q35 with a pinned
  e1000 and asserts the inventory, BAR0 line, self-test and marker; replaces
  the old SKIP-only `scripts/test_pci.sh`; `test_all.sh` points at it.
- Stage-5 leftovers corrected: canonical-order wording in `registry.rs`,
  DECISIONS and the enum contract; ADR-0026 evidence sentence fixed.

- Phase 9 stage 2 — host model (`os_core/src/driver.rs` + lib export):
  `Device`/`Match`/`Descriptor`/`Binding`/`State`, strict descriptor
  validation, deterministic binding, single-instance, caps, explicit
  lifecycle transitions, 10 unit tests.
- Phase 9 stage 2 — kernel model (`kernel/src/driver.rs`): e1000 descriptor,
  inventory snapshot conversion, bind + structural self-test (rules,
  uniqueness, single-instance, determinism, `Bound` only), boot lines and
  `drivers` shell report.
- Phase 9 stage 2 — integration/harness: `pci::snapshot()`, `driver::init()`
  after the PCI inventory, `,driver` boot check, shell help/command,
  `scripts/test_driver_model_qemu.sh` (pinned lines, marker, exit 33)
  replacing the old SKIP-only script; `test_all.sh` path updated.
- Phase 9 stage 2 — docs: `docs/DRIVER_MODEL.md`, ADR-0028, R-021 (Microsoft
  driver/device-object pages fetched 2026-10-20 used only for terminology and
  scope boundaries), TESTING/ROADMAP/ARCHITECTURE updates.

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
| `scripts/build_user_pe.sh` (Phase 7 stage 3) | PASS: seven artifacts — `pehello.exe`/`pebad.exe` unchanged and hash-stable; `peconsole.exe`/`pebadimp.exe`/`pebadfn.exe` unchanged from stage 2; `pebasics.exe` 4096 bytes, 5 sections, 1 relocation, imports `kernel32.dll:ExitThread,GetCommandLineA,GetCurrentProcessId,GetCurrentThreadId,GetLastError,GetStdHandle,GetTickCount,SetLastError,WriteFile`; `pebadfn2.exe` generated from the unique `GetTickCount`→`Sleep` pattern and structurally valid |
| `scripts/make_iso.sh` (Phase 7 stage 3) | PASS: ISO built (kernel + three ELF modules + **seven** PE modules), BIOS+UEFI catalog entries verified |
| `scripts/test_pe_qemu.sh` (Phase 7 stage 3) | PASS: `pe: 7 modules present`; `pe: pebasics.exe loaded at 0x400000 (entry 0x401000, 5 sections, 1 relocation applied)`; the full stage-3 `win32:` block (`SetLastError(42)`, `GetLastError() -> 42`, `WriteFile(1, 28 bytes) -> 1`, still `-> 42`, `WriteFile(0) -> 0 (invalid handle)` + `-> 6`, `WriteFile(1, 28 bytes, overlapped) -> 0 (invalid parameter)` + `-> 87`, IDs `1`/`1`, `GetTickCount` pair, `GetCommandLineA() -> "/boot/pebasics.exe"`, `SetLastError(0)` → `-> 0`, second write `-> 1`, `ExitThread(0)`); `user: /boot/pebadfn2.exe rejected (unknown import function)`; `win32 program selftest: passed (WriteFile, GetLastError/SetLastError, ExitThread, GetCurrentProcessId/GetCurrentThreadId, GetTickCount, GetCommandLineA verified through kernel32.dll imports)`; marker unchanged; exit 33 |
| `scripts/test_all.sh` (Phase 7 stage 3, final) | PASS: 22 PASS, 0 FAIL, 4 SKIP; format and lint green after the rustfmt pass; the first stage-3 run exposed the persistence driver's bare-`invalid` search (FAIL, boot 1), which was scoped to the `persist: slot` diagnostics before the final run |
| `cargo test -p os_core` (Phase 8 stage 1) | PASS: 269 passed, 0 failed (adds 17 configuration-store codec tests: key/value validation tables and length caps, sorted insert/overwrite/remove, prefix listing, entry-count and blob caps, canonical encode and round trips, empty store, unsorted-input acceptance, every decode rejection, stable labels) |
| `scripts/test_shell_qemu.sh` (Phase 8 stage 1) | PASS: 25-command session (the eight config commands incl. `config: 3 entries (/etc/config, …)`, list/`error: no such key`/`error: bad key`), 0 overruns, 10022 bytes received |
| `scripts/test_persistence_qemu.sh` (Phase 8 stage 1) | PASS: 4 boots — boot 1 `write` → B seq=1 and `config set Display/Theme light` → A seq=2; boot 2 loads A seq=2 with `light`, `mkdir` → B seq=3, `write /tmp/notes/a.txt` → A seq=4, `config set Display/Theme dark` → B seq=5; boot 3 corrupts B → `loaded slot A seq=4` with theme `light` (rollback) and `alpha`; boot 4 both invalid → fresh format, `classic` defaults |
| `scripts/test_all.sh` (Phase 8 stage 1, final) | PASS: 22 PASS, 0 FAIL, 4 SKIP (skipped is not passed); unit suite 269 passed; format/lint green in the same run; marker `checks=...,sched,user,elf,pe,win32,vfs,config`, disk boots add `,blk,persist,net` |
| `cargo test -p os_core` (Phase 8 stage 2) | PASS: 269 passed, 0 failed (no new host-tested code; the stage is kernel/syscall/fixture-side) |
| `scripts/build_user_pe.sh` (Phase 8 stage 2) | PASS: eight artifacts — `pehello.exe`/`pebad.exe` unchanged and hash-stable; `peconsole.exe`/`pebasics.exe`/`pebadimp.exe`/`pebadfn.exe`/`pebadfn2.exe` unchanged from stages 2–3; `peinput.exe` 4096 bytes, 5 sections, 1 relocation, imports `kernel32.dll:ExitThread,GetLastError,GetStdHandle,ReadFile,WriteFile`, checker-verified |
| `scripts/make_iso.sh` (Phase 8 stage 2) | PASS: default ISO rebuilt (kernel + three ELF modules + seven PE modules), BIOS+UEFI catalog entries verified; `scripts/make_iso.sh --no-build --input-fixture` built `artifacts/project-os-input.iso` (kernel + three ELF modules + eight PE modules, `debug_exit` kept, `peinput.exe` module line injected into the staged `limine.conf` and grep-verified) |
| `scripts/test_input_qemu.sh` (Phase 8 stage 2) | PASS: `pe: peinput.exe loaded at 0x400000 (entry 0x401000, 5 sections, 1 relocation applied)`; `win32: GetStdHandle(-10) -> 2`; `win32: ReadFile(1) -> 0 (invalid handle)` + `GetLastError() -> 6`; overlapped read `-> 0 (invalid parameter)` + `GetLastError() -> 87`; `win32: ReadFile(2, 64 bytes) waiting for input`; typed line echoed and played back; `win32: ReadFile(2, 64 bytes) -> 1 (14 bytes)`; `win32: input surface verified`; `win32 input selftest: passed (...)`; marker `checks=...,win32,vfs,config`; QEMU exit 33; transcript 9,166 bytes |
| `scripts/test_pe_qemu.sh` (Phase 8 stage 2 regression) | PASS: unchanged stage-1–3 assertions on the default ISO (7 modules, allowlist resolution, stub page, all stage-2/3 `win32:` lines and mutants) |
| `scripts/test_all.sh` (Phase 8 stage 2, final) | PASS: 23 PASS, 0 FAIL, 4 SKIP (skipped is not passed); unit suite 269 passed; format/lint and every boot/shell/persistence/network/VirtualBox step green in the same run; the new `input` step runs after `shell`. Two earlier harness iterations failed on their own pinned expectations (serial LF display, marker prefix race) and were fixed before this run; the kernel passed unmodified on the first boot. |
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

Recorded QEMU boot log (Phase 7 stage 3 addition, same
`artifacts/qemu-serial-pe.log`, build `0.1.0-ec04e74f`):

```text
user: /boot/pebadfn2.exe rejected (unknown import function)
pe: pebasics.exe loaded at 0x400000 (entry 0x401000, 5 sections, 1 relocation applied)
win32: GetStdHandle(-11) -> 1
win32: SetLastError(42)
win32: GetLastError() -> 42
win32: hello from WriteFile
win32: WriteFile(1, 28 bytes) -> 1
win32: GetLastError() -> 42
win32: WriteFile(0) -> 0 (invalid handle)
win32: GetLastError() -> 6
win32: WriteFile(1, 28 bytes, overlapped) -> 0 (invalid parameter)
win32: GetLastError() -> 87
win32: GetCurrentProcessId() -> 1
win32: GetCurrentThreadId() -> 1
win32: GetTickCount() -> 5990
win32: GetTickCount() -> 6010
win32: GetCommandLineA() -> "/boot/pebasics.exe"
win32: SetLastError(0)
win32: GetLastError() -> 0
win32: program basics verified
win32: WriteFile(1, 31 bytes) -> 1
win32: ExitThread(0)
user: ring-3 program exited (code 0), 7 write(s), 243 bytes, 1 isolated fault(s), 0 rejected syscall(s)
win32 program selftest: passed (WriteFile, GetLastError/SetLastError, ExitThread, GetCurrentProcessId/GetCurrentThreadId, GetTickCount, GetCommandLineA verified through kernel32.dll imports)
```

The stage-3 block runs after the stage-2 console block; every line above is
asserted by `scripts/test_pe_qemu.sh` (`grep -Fq` per line). The counter
deltas (`WRITE_CALLS` +2, `BYTES_WRITTEN` +59 = 28 + 31, eight service
counters) are asserted inside `user.rs` before the verdict prints, so the
"passed" line implies them.

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

Configuration store (Phase 8 stage 1; `artifacts/qemu-shell-session.log` and
`artifacts/qemu-persist-boot{1..4}.log` for the persistence sequence):

```text
config selftest: passed (3 entries, 75 bytes; get/set/remove/list/encode/decode/error paths)
PROJECT_OS_BOOT_SUCCESS:0.1.0-82bc098b checks=handoff,memmap,gdt,idt,exceptions,timer,allocator,heap,vm,sched,user,elf,pe,win32,vfs,config
project-os> config
config: 3 entries (/etc/config, 75 bytes)
project-os> config set Display/Theme light
config set: ok
project-os> config list
3 entries in config:
  Display/Theme = light
  Net/Mode = static
  System/Name = PROJECT_OS
project-os> config del Display/Theme
config del: ok
project-os> config get Display/Theme
error: no such key
project-os> config set bad//key x
error: bad key
```

Persistence sequence with the store: boot 1 saves `hello.txt` (B seq=1) and
then `config set Display/Theme light` (A seq=2); boot 2 loads A seq=2, reads
`Display/Theme = light`, saves B seq=3 (`mkdir`), A seq=4 (`write
/tmp/notes/a.txt`) and B seq=5 (`config set Display/Theme dark`); boot 3
(in slot B's payload zeroed by the driver) falls back to `persist: loaded
slot A seq=4`, reads `Display/Theme = light` — the `dark` value existed only
in the corrupt slot — and `/tmp/notes/a.txt` → `alpha`; boot 4 formats fresh
and reads `Display/Theme = classic` (defaults reseeded) with
`/tmp/hello.txt` gone.

Win32 console input (Phase 8 stage 2; `artifacts/qemu-input-session.log`,
`project-os-input.iso`, build `0.1.0-58f98e48`; QEMU q35 512 MiB,
`isa-debug-exit`, paced TCP serial; the harness typed `hello, win32` as soon
as the wait line appeared):

```text
pe: peinput.exe loaded at 0x400000 (entry 0x401000, 5 sections, 1 relocation applied)
win32: GetStdHandle(-10) -> 2
win32: ReadFile(1) -> 0 (invalid handle)
win32: GetLastError() -> 6
win32: ReadFile(2, 16 bytes, overlapped) -> 0 (invalid parameter)
win32: GetLastError() -> 87
win32: ReadFile(2, 64 bytes) waiting for input
hello, win32
win32: ReadFile(2, 64 bytes) -> 1 (14 bytes)
hello, win32
win32: WriteFile(1, 14 bytes) -> 1
win32: input surface verified
win32: WriteFile(1, 30 bytes) -> 1
win32: ExitThread(0)
win32 input selftest: passed (ReadFile line input, echo, byte count, invalid handle/parameter errors verified through kernel32.dll imports)
user: ring-3 program exited (code 0), 9 write(s), 287 bytes, 1 isolated fault(s), 0 rejected syscall(s)
PROJECT_OS_BOOT_SUCCESS:0.1.0-58f98e48 checks=handoff,memmap,gdt,idt,exceptions,timer,allocator,heap,vm,sched,user,elf,pe,win32,vfs,config
```

The first `hello, win32` line is the kernel's echo while reading; the second
is the fixture's `WriteFile` playback of the 14-byte CRLF-terminated buffer
(`ReadFile(2, 64 bytes) -> 1 (14 bytes)` is the count line). The serial layer
expands every LF to CRLF (`kernel/src/serial.rs`), so both displayed lines
end `CR CR LF` while the buffer holds a single CRLF — the doubled CR is a
display-layer artifact, and the count line plus the fixture's own CRLF
structural check are the normative evidence. The harness asserts QEMU exit
33 (isa-debug-exit). Two harness bring-up defects were found and fixed by the
runner before this PASS: the echo/playback expectation did not account for
the serial LF expansion, and the marker checks were asserted before the
marker line's terminator had arrived (`wait_for` returns on the prefix). No
kernel defect was found; the kernel and loader behaved as specified on the
first run.

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

Phase 8 stage 3 (`docs/WIN32_REGISTRY.md`, ADR-0024, research R-017; build
id `0.1.0-b1b8542c`, derived from the then-current commit `b1b8542`):

```sh
scripts/format.sh                        # PASS
scripts/lint.sh                          # PASS (after one clippy fix)
scripts/test_unit.sh                     # PASS (269/269)
scripts/build.sh                         # PASS (kernel.elf 1,817,664 bytes)
scripts/build_user_pe.sh                 # PASS (9 artifacts)
scripts/make_iso.sh --registry-fixture   # PASS -> project-os-registry.iso
scripts/test_registry_qemu.sh            # FAIL (harness expectation), fixed -> PASS
scripts/make_iso.sh                      # PASS (default ISO rebuilt)
scripts/test_input_qemu.sh               # PASS (regression)
scripts/test_pe_qemu.sh                  # stale ISO first, rebuilt -> PASS
scripts/test_all.sh                      # PASS: 24 PASS, 0 FAIL, 4 SKIP
```

Recorded registry session (`artifacts/qemu-registry-session.log`, 10,420
bytes, SHA-256
`021807014f814d557227c3774d7231a3d47386ec33b023f843b72d736a8e5945`,
exit-33 run, build id `0.1.0-b1b8542c`):

```text
pe: pereg.exe loaded at 0x400000 (entry 0x401000, 5 sections, 1 relocation applied)
win32: RegOpenKeyExA(0x1, "System") -> 6 (invalid handle)
win32: RegOpenKeyExA(0x80000002, "System") -> 87 (invalid parameter)
win32: RegOpenKeyExA(0x80000002, "Missing") -> 2 (file not found)
win32: RegOpenKeyExA(0x80000002, "System") -> 0 (handle 4096)
win32: RegQueryValueExA(4096, "Missing") -> 2 (file not found)
win32: RegQueryValueExA(4096, "Name") -> 0 (REG_SZ, 11 bytes)
win32: RegQueryValueExA(4096, "Name") -> 234 (more data, 11 bytes needed)
win32: RegQueryValueExA(4096, "Name") -> 0 (REG_SZ, 11 bytes) "PROJECT_OS"
win32: RegQueryValueExA(4096, "Name") -> 87 (invalid parameter)
win32: RegQueryValueExA(4096, "Name") -> 87 (invalid parameter)
win32: RegOpenKeyExA(0x80000002, "") -> 0 (handle 0x80000002)
win32: RegQueryValueExA(0x80000002, "Display/Theme") -> 0 (REG_SZ, 8 bytes) "classic"
win32: RegCloseKey(0x80000002) -> 0
win32: RegOpenKeyExA(0x80000002, "System") -> 0 (handle 4097)
win32: RegOpenKeyExA(0x80000002, "System") -> 0 (handle 4098)
win32: RegOpenKeyExA(0x80000002, "System") -> 0 (handle 4099)
win32: RegOpenKeyExA(0x80000002, "System") -> 4 (too many open keys)
win32: RegCloseKey(4096) -> 0
win32: RegCloseKey(4096) -> 6 (invalid handle)
win32: RegOpenKeyExA(0x80000002, "System") -> 0 (handle 4096)
win32: RegCloseKey(4096) -> 0
win32: RegCloseKey(4097) -> 0
win32: RegCloseKey(4098) -> 0
win32: RegCloseKey(4099) -> 0
win32: registry surface verified
win32: WriteFile(1, 33 bytes) -> 1
win32: ExitThread(0)
win32 registry selftest: passed (advapi32.dll RegOpenKeyExA/RegQueryValueExA/RegCloseKey over the config store; handle validation, size queries, ERROR_MORE_DATA, REG_SZ reads, slot exhaustion/reuse and double close verified)
user: ring-3 program exited (code 0), 8 write(s), 276 bytes, 1 isolated fault(s), 0 rejected syscall(s)
```

The first harness run of this session failed only its size-query pin: the
serial layer expands LF to CRLF, so the pinned expectation needs the
`\r\r\n` display terminator (the same display artifact as stage 2); the
fixed harness passed. The first `pe` regression reran before the default ISO
was rebuilt and therefore booted the stage-2 kernel (`0.1.0-58f98e48`);
after `scripts/make_iso.sh` the rerun passed with `0.1.0-b1b8542c` and the
registry skip verdicts present. No kernel defect was found; the registry
session passed on the first kernel build.

Phase 8 stage 4 (`docs/WIN32_REGISTRY_WRITE.md`, ADR-0025, research R-018;
build id `0.1.0-fe3c278e`, derived from the then-current commit `fe3c278`):

```sh
scripts/format.sh                             # PASS (one rustfmt fix in syscall.rs)
scripts/lint.sh                               # PASS
scripts/test_unit.sh                          # PASS (269/269)
scripts/build.sh                              # PASS (kernel.elf 1,866,152 bytes)
scripts/build_user_pe.sh                      # PASS (10 artifacts)
scripts/make_iso.sh --registry-write-fixture  # PASS -> project-os-registry-write.iso
scripts/test_registry_write_qemu.sh           # FAIL (fixture order), fixed -> PASS
scripts/test_all.sh                           # PASS: 25 PASS, 0 FAIL, 4 SKIP
```

The first harness run failed because the fixture repeated the create before
the first write and the lazy children-only model therefore reported
`created`; the fixture swapped steps 2 and 3 (write `Flag`, then repeat the
create) and the spec recorded the rule. The fixed run's transcript
(`artifacts/qemu-registry-write-session.log`, 10,407 bytes, SHA-256
`4c03490bed58031e65eea434a5ba4b0cb2698d49919fb1726af68535c7a16077`,
exit-33 run, build id `0.1.0-fe3c278e`):

```text
pe: peregw.exe loaded at 0x400000 (entry 0x401000, 5 sections, 1 relocation applied)
win32: RegCreateKeyExA(0x80000002, "Session/Current") -> 0 (handle 4096, created)
win32: RegSetValueExA(4096, "Flag") -> 0 (REG_SZ, 7 bytes)
win32: RegCreateKeyExA(0x80000002, "Session/Current") -> 0 (handle 4097, opened existing)
win32: RegCloseKey(4097) -> 0
win32: RegQueryValueExA(4096, "Flag") -> 0 (REG_SZ, 8 bytes) "enabled"
win32: RegSetValueExA(4096, "Flag") -> 0 (REG_SZ, 2 bytes)
win32: RegQueryValueExA(4096, "Flag") -> 0 (REG_SZ, 3 bytes)
win32: RegSetValueExA(4096, "Number") -> 87 (invalid parameter)
win32: RegSetValueExA(4096, "Flag") -> 87 (invalid parameter)
win32: RegSetValueExA(1, "Flag") -> 6 (invalid handle)
win32: RegSetValueExA(4096, "Bad Name") -> 87 (invalid parameter)
win32: RegSetValueExA(4096, "") -> 87 (invalid parameter)
win32: RegDeleteValueA(4096, "Missing") -> 2 (file not found)
win32: RegDeleteValueA(1, "Flag") -> 6 (invalid handle)
win32: RegDeleteValueA(4096, "Flag") -> 0
win32: RegQueryValueExA(4096, "Flag") -> 2 (file not found)
win32: RegCreateKeyExA(0x80000002, "Session/Current") -> 0 (handle 4097, created)
win32: RegSetValueExA(4097, "Persisted") -> 0 (REG_SZ, 3 bytes)
win32: RegCloseKey(4096) -> 0
win32: RegCloseKey(4097) -> 0
win32: registry write surface verified
win32 registry write selftest: passed (advapi32.dll RegCreateKeyExA/RegSetValueExA/RegDeleteValueA over the config store; create/open dispositions, REG_SZ sets and updates, deletes, error values 2/6/87, persistence through /etc/config verified)
user: ring-3 program exited (code 0), 8 write(s), 282 bytes, 1 isolated fault(s), 0 rejected syscall(s)
```

The build id of this run is `0.1.0-fe3c278e`; the serial/timer regressions,
the `pe`/`input`/`registry` harnesses and the full `test_all.sh` matrix ran
inside the recorded `test_all.sh` pass (25 PASS / 0 FAIL / 4 SKIP; unit
suite 269). No kernel defect was found; the write session passed on the
fixed fixture with the same kernel build.

Phase 8 stage 5 (final runs, 2026-10-20):

```sh
scripts/format.sh                    # PASS (rustfmt fix in pe_loader.rs)
scripts/lint.sh                      # PASS
scripts/build.sh                     # PASS -> kernel 1,952,160 bytes
scripts/build_user_pe.sh             # PASS (11 artifacts)
scripts/make_iso.sh --registry-enum-fixture
scripts/test_registry_enum_qemu.sh   # PASS (exit 33)
scripts/test_all.sh                  # PASS=26 FAIL=0 SKIP=4
```

The enum harness now passes after three fixture-side/latent bring-up fixes,
each found by a failing run and fixed without weakening an assertion:
`#UD` on `movaps` (SSE off) → `enable_sse()`; `#GP` on the first fixture
instruction sequence (entry `rsp % 16 != 8`) → `PE_STACK_TOP`; missing `r11`
load in `Args8` → stub fixed. Enumeration order was measured and documented
as the store's canonical ascending order (values under `Session/Current`:
`Enum/Alpha/Item`, `Enum/Beta/Item`, `Keep`, `Removed`), so the contract's
"insertion order" wording was corrected in the spec/R-019/ADR-0026 and code
comments. Lazy-key deletion semantics then required a nine-call sequence
(values are not keys; `Session` is denied while `Current` exists).
Final artifacts: kernel `9d3d7354…` 1,952,160 B; `peregen.exe`
`25ea3d45…` 7,680 B; enum ISO `49af9b59…` 7,997,440 B; enum session log
`a44687f8…` 11,572 B; default ISO `2bd3b5d0…` 7,989,248 B.

Phase 9 stage 1 (final runs, 2026-10-20):

```sh
scripts/format.sh              # PASS
scripts/lint.sh                # PASS
scripts/build.sh               # PASS -> kernel 1,697,064 bytes
scripts/test_unit.sh           # PASS (270/270)
scripts/test_pci_qemu.sh       # PASS (exit 33)
scripts/test_all.sh            # PASS=27 FAIL=0 SKIP=3
```

The q35 run with a pinned e1000 enumerated 6 functions/7 BARs and printed
`pci: 00:02.0 8086:100e ... bar0 mem32 base=0xfebc0000 size=0x20000`; the
VirtualBox run enumerated 8 functions/14 BARs incl. the 82540EM at 00:03.0,
so the self-test passed in both environments. Kernel builds are deterministic
across repeated builds (`7761d0e6...`). Final artifacts: kernel `7761d0e6...`
1,697,064 B; default ISO `2e890c65...` 7,733,248 B; PCI session log
`aea6f88d...` 9,836 B; VirtualBox serial `f679b6ec...` 10,169 B.

Phase 9 stage 2 (final runs, 2026-10-20):

```sh
scripts/format.sh                 # PASS
scripts/lint.sh                   # PASS
scripts/build.sh                  # PASS -> kernel 1,826,000 bytes
scripts/test_unit.sh              # PASS (280/280)
scripts/test_driver_model_qemu.sh # PASS (exit 33)
scripts/test_all.sh               # PASS=28 FAIL=0 SKIP=2
```

The q35 boot with a pinned e1000 printed `driver: registered 1 descriptor(s)`,
`driver: e1000 matched 00:02.0 8086:100e (state Bound)`,
`driver: 5 function(s) unmatched (no registered driver)` and
`driver selftest: passed (1 binding(s), 5 unmatched; match rules, uniqueness
and single-instance verified)`, and the marker gained `,driver`. The model
stays `Bound`: net bring-up is unchanged and no start/stop runs through it.
Final artifacts: kernel `2ce7c393...` 1,826,000 B; default ISO `6ddd0274...`
7,862,272 B; driver session log `f5578086...` 10,097 B; VirtualBox serial
`89390a76...` 10,430 B. Full suite 28 PASS / 0 FAIL / 2 SKIP, 280 unit tests.

## Artifact

| Item | Value |
|---|---|
| Kernel ELF | `artifacts/kernel.elf`, 1,826,000 bytes (Phase 9 stage 2) |
| Kernel SHA-256 | `2ce7c393a8484cf7a57138c0c745d4997b16dd8393b6078d9ff0ec8344bb1e93` (build id `0.1.0-65ecbfdf`) |
| Kernel ELF entry | `0xffffffff80000000` (verified with `readelf`) |
| userland modules | `artifacts/hello.elf` SHA-256 `f4ae1ae1a5bb1479f0fbf99a65991d1e5e12abb3b72537f6ffc0dac4f9f9a960`; `artifacts/fault.elf` SHA-256 `46c45ce6eca4bd67e2a1e06cc85b2ef79a68be4a5bf9d1b1af84a11d0c0c23f0`; `artifacts/bad.elf` SHA-256 `a8bec8d1c4b866fc0d0fd2e81a71cda920fc95da0c87c64286cb4c7b45f9006d` (hash-stable from a clean rebuild) |
| PE fixtures | `artifacts/pehello.exe`, 3072 bytes (entry RVA `0x1000`, base `0x140000000`, 4 sections, 1 DIR64 relocation), SHA-256 `96e2e6d6ff69db3ce5204a215defaa9996fe964224eb5fd0bc2d2ef72d577e41`; `artifacts/pebad.exe` (signature zeroed), 3072 bytes, SHA-256 `a0f8a2b401ec904eead12ba520bb9d0063e7ea212547457ab2a1439a017e453d`; `artifacts/peconsole.exe` (Phase 7 stage 2: 5 sections incl. `.idata`, 1 DIR64 relocation, imports `kernel32.dll:ExitProcess,GetStdHandle,WriteConsoleA`), 3584 bytes, SHA-256 `1e72a7b3d32421a5883b1c8344b1b1e149aa35a81b18648fcdba1e54e74eb406`; `artifacts/pebadimp.exe` (`kernel32.dll` → `user32.dll` pattern patch), 3584 bytes, SHA-256 `95c93ae440a10dbf8fc8b6bddc418752b06c4f5dc4fe68b8d0f76b889e0214e3`; `artifacts/pebadfn.exe` (`WriteConsoleA` → `WriteConsoleB` pattern patch), 3584 bytes, SHA-256 `4811ccf6488777b90b32c91e9f27fa212cf33fba1fc1c06404e16f4abebc2ab1`; `artifacts/pebasics.exe` (Phase 7 stage 3: 5 sections, 1 relocation, nine imports `kernel32.dll:ExitThread,GetCommandLineA,GetCurrentProcessId,GetCurrentThreadId,GetLastError,GetStdHandle,GetTickCount,SetLastError,WriteFile`), 4096 bytes, SHA-256 `4a4aa956a3f5519875b43d8a31e91177e3392fa4316ef2ed4d98d70bdea8341b`; `artifacts/pebadfn2.exe` (`GetTickCount` → `Sleep` pattern patch), 4096 bytes, SHA-256 `9d6082b9792859eba6d4b4ca1eae70c09c46314e79588757cbb74c37e6cd473d`; `artifacts/peinput.exe` (Phase 8 stage 2: 5 sections, 1 relocation, five imports `kernel32.dll:ExitThread,GetLastError,GetStdHandle,ReadFile,WriteFile`), 4096 bytes, SHA-256 `a2ab21aa4ea7e282c79e2ed1b437c72cee0a21e7b706ba4d0b007a46d5167de0`; `artifacts/pereg.exe` (Phase 8 stage 3: 5 sections, 1 relocation, imports `advapi32.dll:RegCloseKey,RegOpenKeyExA,RegQueryValueExA;kernel32.dll:ExitThread,WriteFile`), 5120 bytes, SHA-256 `0ca37e2e762035bc98729b102d3a1d53340214e67365eba91daa72866fd3b3d4` (pehello/pebad hash-stable from clean rebuilds; the existing eight artifacts' hashes were unchanged by the stage-3 rebuild; `pereg.exe` was built once and recorded); `artifacts/peregw.exe` (Phase 8 stage 4: 5 sections, 1 relocation, imports `advapi32.dll:RegCloseKey,RegCreateKeyExA,RegDeleteValueA,RegQueryValueExA,RegSetValueExA;kernel32.dll:ExitThread,WriteFile`), 5632 bytes, SHA-256 `3b00b91a8677ce733f62af242809abe146005c853bb23e99d0c0d00135d5df01` (the nine pre-existing artifacts' hashes were unchanged by the stage-4 rebuild) |
| ISO | `artifacts/project-os.iso` (default: seven PE modules) |
| ISO size | 7,862,272 bytes |
| ISO SHA-256 | `6ddd0274cb3af372e3c0323a229d9fbeb9dcd4175d92973ef76ea26ca9b7e692` (rebuilt in the final stage-2 `test_all.sh` run; `mkisofs` timestamps make byte-for-byte reproducibility impossible) |
| Registry ISO | `artifacts/project-os-registry.iso` (`make_iso.sh --registry-fixture`: kernel + three ELF modules + eight PE modules incl. `pereg.exe`, `debug_exit` kept) |
| Registry ISO size / SHA-256 | 7,909,376 bytes / `d6d909a6c499f58d534cbc5a62b8b8365020dd83215c0abfaedb13246efcdf22` (rebuilt in the final stage-4 `test_all.sh` run; the stage-4 kernel grows the image) |
| Registry-write ISO | `artifacts/project-os-registry-write.iso` (`make_iso.sh --registry-write-fixture`: kernel + three ELF modules + eight PE modules incl. `peregw.exe`, `debug_exit` kept) |
| Registry-write ISO size / SHA-256 | 7,909,376 bytes / `527757b0529e5a52159090382201bdfe4b37f3661cb6dc6c930690b2e414f1aa` (built by the stage-4 registry-write harness inside the final `test_all.sh` run) |
| Input ISO | `artifacts/project-os-input.iso` (`make_iso.sh --input-fixture`: kernel + three ELF modules + eight PE modules incl. `peinput.exe`, `debug_exit` kept; built by `scripts/test_input_qemu.sh` with `--no-build`) |
| Input ISO size / SHA-256 | 7,907,328 bytes / `d988d81f5a03bcbb55705f551ece3bb15b604536dfed6df10fbb3308b69e47fa` (rebuilt in the final stage-4 `test_all.sh` run) |
| Interactive ISO | `artifacts/project-os-interactive.iso` (same bytes but `debug_exit` stripped from the cmdline; rebuilt by each session harness) |
| Interactive ISO size / SHA-256 | 7,903,232 bytes / `25d394736521e16563ae6ab24fabb5876a7f55e0c591d2e7f47cb9d0ae33d993` (rebuilt during the final stage-4 `test_all.sh` run; the shell and persistence runs each rebuild it, so each harness log names its own build) |
| Screendump | `artifacts/console-screendump.ppm`, 3,072,016 bytes (1280x800 P6), SHA-256 `07b98cd5ab99065048fa6e607465ba3ce3ad0c848aab1c500a52beab28041bed`; checker PASS (rebuilt in the final stage-4 `test_all.sh` run) |
| Framebuffer session log | `artifacts/qemu-console-session.log`, 8,932 bytes, SHA-256 `293b6853ef9c1e547376d74f8d48322f3af2cd7b3b6dbcbbd81232d7fc0ea665` (rebuilt in the final stage-4 `test_all.sh` run) |
| Net session log | `artifacts/qemu-net-session.log`, 8,196 bytes, SHA-256 `be1b9445a8712ac96129781084c076f15146d84111402faac4c1ceb9917d7f78` (rebuilt in the final stage-4 `test_all.sh` run); `artifacts/qemu-net.log` is empty (0 bytes, no QEMU errors) |
| Checkpoint disk image | `artifacts/persist-disk.img`, 8,388,608 bytes (raw; final state after the stage-2 Phase 8 4-boot regression: fresh format, slot A seq 0, defaults reseeded — byte-identical to the stage-1 final state), SHA-256 `05d2d3ad26aa59c8140d1195e6e8423e19206895c6ed4dbe7c0aff0c0c245d22` |
| Input session log | `artifacts/qemu-input-session.log`, 9,463 bytes, SHA-256 `c00321a4b992330917c045f8179a46d651a3549195c48ddd28c0e2ea5678250b` (paced TCP serial transcript, exit-33 run, rebuilt in the final stage-4 `test_all.sh` run); `artifacts/qemu-input.log`, 192 bytes, SHA-256 `a9eed819f1f42c84f2b77080b139b3a6ec7c3646a36488bf3b071e467c1c8127` (only QEMU's `wait=on` connection-info line) |
| Registry session log | `artifacts/qemu-registry-session.log`, 10,578 bytes, SHA-256 `bed88828899f67035a4057585a6ff6873b939600c7ba319e688e1eeddd8f2dea` (rebuild in the final stage-4 `test_all.sh` run); `artifacts/qemu-registry.log`, 192 bytes, SHA-256 `0bad30e9fa07e4386a3cd8701a650d59d897ad908e0e9be0a00b9e164c53ef30` (only QEMU's `wait=on` connection-info line) |
| Registry-write session log | `artifacts/qemu-registry-write-session.log`, 10,407 bytes, SHA-256 `4c03490bed58031e65eea434a5ba4b0cb2698d49919fb1726af68535c7a16077` (paced TCP serial transcript, exit-33 run); `artifacts/qemu-registry-write.log`, 192 bytes, SHA-256 `84de33eb93aa7f275002f0f0ee660531a9634341b1f4b34bf9251480a8907fed` (only QEMU's `wait=on` connection-info line) |
| ISO boot entries | BIOS platform 0x00 (4 sectors), UEFI platform 0xEF (6144 sectors), both no-emulation, verified by `scripts/verify_iso.py` |
| Serial logs | `artifacts/qemu-serial-256m.log`, `-512m.log`, `-1g.log`, `artifacts/qemu-serial.log` (memory test), `artifacts/vbox-serial.log` (VirtualBox), `artifacts/qemu-shell-session.log` (interactive session, rebuilt in the final stage-4 `test_all.sh` run), `artifacts/qemu-console-session.log` (framebuffer console), `artifacts/qemu-persist-boot{1,2,3,4}.log` + `qemu-persist.log` (4-boot durability), `artifacts/qemu-serial-elf.log` (ELF64 module boot), `artifacts/qemu-serial-pe.log` (PE32+/Win32 module boot; stage-4 regression log 8,707 bytes, SHA-256 `f88f06a9270e73229cb30de7492616010814a0143835cb80f72ffa2ee29eac1d`), `artifacts/qemu-input-session.log` (Win32 console-input session), `artifacts/qemu-registry-session.log` (Win32 registry session), `artifacts/qemu-registry-write-session.log` (Win32 registry-write session), `artifacts/qemu-net-session.log` (e1000 network session) |
| VirtualBox result | `artifacts/vbox-test.log`: VirtualBox 7.2.20 BIOS PASS, marker, 5 ticks, `fbcon: enabled 85x48` and console selftest observed; with the 82540EM NAT NIC the kernel detected the NIC and reported `net: loopback selftest: unsupported` + `net selftest: no gateway reply (arp timeout)` — non-fatal, no VirtualBox networking claim |

ISO byte-for-byte reproducibility is not claimed: `mkisofs` embeds file
timestamps, so a rebuild changes the ISO checksum even with unchanged sources.
The kernel and fixture builds are deterministic for identical sources, flags
and build ID. The checksums above are from the final Phase 9 stage-2
`scripts/test_all.sh` run (28 PASS / 0 FAIL / 2 SKIP, unit suite 280 passed).
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
  defects recorded). The stage-3 Win32 program-basics surface is governed by
  `docs/WIN32_PROGRAM.md` (the contract), ADR-0021 (five-argument stub via
  `r10`, per-thread `last_error`, synthetic IDs, loader-owned command line,
  documented `WriteFile` error mapping, unchanged marker) and research R-015
  (MS console/thread/time API contracts; PASSED with the observed stage-3
  log, the fixture hashes and the two bring-up defects recorded).
  The Phase 8 stage-1 configuration store is governed by `docs/CONFIG.md`
  (the contract) and ADR-0022 (one ordinary VFS file `/etc/config` plus a
  host-tested `POCF` codec, no checkpoint format change, defaults seeded in
  memory with a lazy first-mutation save, `,config` after `,vfs`); no new
  research entry was needed because the format is internal and makes no
  external protocol claim.
  The Phase 8 stage-2 Win32 console-input surface is governed by
  `docs/WIN32_INPUT.md` (the contract), ADR-0023 (one new service `ReadFile` =
  14 behind the existing five-argument stub, distinct input handle `2`,
  blocking line-mode read with echo inside the syscall via `hlt`, CRLF-
  inclusive line/count as the project contract, documented `FALSE` + WinError
  mapping with the count zeroed first, dedicated `--input-fixture` ISO, no
  marker change) and research R-016 (MS `ReadFile`/console documentation;
  PASSED with the observed session log, the corrected serial CRLF display
  expectation and the final `PASS=23 FAIL=0 SKIP=4` summary).
  The Phase 8 stage-3 Win32 registry read surface is governed by
  `docs/WIN32_REGISTRY.md` (the contract), ADR-0024 (two-DLL allowlist with
  a per-entry DLL name, first six-argument stub form via `r11`, read-only
  four-slot `HKLM` handle table over the config store, documented error
  mapping without killing the thread, dedicated `--registry-fixture` ISO, no
  marker change) and research R-017 (MS registry/`RegQueryValueEx` primary
  sources; PASSED with the observed session log and the harness expectation
  correction).
  The Phase 8 stage-4 Win32 registry write surface is governed by
  `docs/WIN32_REGISTRY_WRITE.md` (the contract), ADR-0025 (services 18–20
  behind the first nine-argument stub form via `r10`–`r14`, lazy
  children-only key creation with `created`/`opened existing` dispositions,
  write-path validation that joins and checks the whole key path, REG_SZ-only
  sets with a NUL-terminated 129-byte cap, persistence through the config
  store's existing save path, dedicated `--registry-write-fixture` ISO, no
  marker change) and research R-018 (MS registry write-API primary sources;
  PASSED with the observed session log, the fixture-order bring-up defect
  and the `/etc/config` persistence proof).
  ADR-0006's framing still holds: serial is authoritative
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
  acceleration. The PE32+ path is Phase 7 stages 1–3 plus Phase 8 stages
  2–4: it runs filesystem-less fixtures and resolves imports against an
  eighteen-name two-DLL (`kernel32.dll` + `advapi32.dll`) allowlist; see the
  stage-1/stage-2/stage-3 and Phase 8 stage-2/stage-3/stage-4 limits bullets
  below.
  Networking exists as stage 5 (one polled e1000 with static addressing);
  there is no general on-disk filesystem: persistence is the
  whole-tree checkpoint image (stage 2) described above.
- Phase 9 stage 2 limits: bindings stay `Bound` (no probe/start callback is
  invoked by the kernel); e1000 start/stop and MMIO/DMA/IRQ ownership remain
  outside the model; no IRPs/device stacks/WDM compatibility, hotplug,
  power management or virtual GPU. The model is compile-time descriptors
  only (no dynamic loading), 8 descriptors and 64 bindings.

- Phase 9 stage 1 limits: bus 0 only (no bridge/secondary-bus traversal),
  boot-time snapshot (no hotplug), no resource assignment/rebalancing, no
  interrupt routing/MSI, no capability parsing, no driver matching/lifecycle,
  no GPU claim; the 64-function cap fails the self-test instead of dropping
  devices.

- Phase 8 stage 5 limits: SSE is enabled but there is no FXSAVE/FXRSTOR
  switching (kernel built without SSE; fixtures run one at a time);
  enumeration order is the store's canonical order, not Windows' arbitrary
  order; no recursion (`RegDeleteTree`/`RegDeleteKeyEx`), typed values, hives,
  ACLs, default values, empty keys, `W` variants or remote registry.

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
- Phase 7 stage-3 limits (spec: `docs/WIN32_PROGRAM.md` §9, ADR-0021):
  exactly eleven allowlisted `kernel32.dll` names; the five-argument stub
  form exists only for `WriteFile`; per-thread `last_error` is a `u32` slot
  in the fixed thread table; process/thread IDs are constants `1`/`1`;
  `GetTickCount` resolution is the 100 Hz PIT (10 ms) and wraps as `u32`;
  the command line is the loader's own Limine module path (no `argv`
  parsing, 255-byte cap → `CommandLineTooLong`); overlapped I/O is never
  accepted, zero-length `WriteFile` returns `ERROR_INVALID_PARAMETER`,
  unmapped spans kill the thread (no access violation); one console handle
  (`1`), one user thread, `MAX_WRITE` cap; no CRT, SEH, TLS, Unicode/W
  variants, `ReadFile`, console input, `GetTickCount64` or `WriteFileEx`.
  Stage-3 bring-up defects (missing command-line terminator,
  `wait_all_finished` TSC budget) were fixed before the PASS and are
  recorded in R-015/ADR-0021; the persistence driver's bare-`invalid` boot-1
  check was scoped to `persist: slot` diagnostics (test-side fix).
- Phase 8 stage-1 limits (spec: `docs/CONFIG.md` §1/§8, ADR-0022): the store
  is a native kernel facility, not a Windows registry — no `advapi32`/
  `HKEY_*` API, no hives, no typed (DWORD/binary/multi-string) values, no
  ACLs or per-user hives, no transactions or change notifications, and keys
  are case-sensitive byte strings. Hard caps: 64 entries, 96-byte keys
  (components ≤32 bytes, `[A-Za-z0-9_.-]` plus `/`), 128-byte
  printable-ASCII values, 7984 serialized bytes; violations are rejected with
  a distinct label, never truncated. Durability is exactly the checkpoint's:
  reboots survive, newest-slot corruption rolls the store back with the tree,
  and a fresh format reseeds the defaults. `rm /etc/config` means factory
  settings on the next boot; the boot self-test never writes the file. No
  bring-up defect occurred in this stage (two host-test iterations during
  development were fixed before any artifact was recorded).
- Phase 8 stage-2 limits (spec: `docs/WIN32_INPUT.md` §1/§7, ADR-0023):
  twelve allowlisted `kernel32.dll` names now, with `ReadFile` accepting only
  handle `2` (`GetStdHandle(-10)`, the distinct input handle; output stays
  `1`); line mode with echo is the only mode — no `ReadConsoleA/W`,
  `GetConsoleMode`/`SetConsoleMode`, Ctrl+C (`ERROR_OPERATION_ABORTED`) or
  Ctrl+Z/EOF semantics, raw mode, `INPUT_RECORD` low-level input, Unicode/W
  variants, redirected stdin, overlapped/asynchronous I/O or a second reader;
  the read length must be `2..=4096` (a CRLF terminator plus the `MAX_READ`
  cap), the buffer always reserves two bytes for CRLF, and a full buffer
  returns the accumulated content with the rest kept in the 256-byte serial
  RX ring. CRLF inclusion in line and count is our documented contract, not
  a vendor-verified `ReadFile` statement. The wait re-enables interrupts and
  uses `hlt`: it depends on the serial RX IRQ and the still-running PIT. A
  boot of the input ISO without a harness fails the phase after the 30 s
  window instead of hanging. No kernel defect was found; two harness
  bring-up defects (serial LF display expectation, marker-prefix race) were
  fixed before the PASS.
- Phase 8 stage-3 limits (spec: `docs/WIN32_REGISTRY.md` §13, ADR-0024;
  historical — stage 4 supersedes the read-only scope and the fifteen-name
  count):
  fifteen allowlisted names across two DLLs (`kernel32.dll` +
  `advapi32.dll`); the surface is read-only (`RegOpenKeyExA`,
  `RegQueryValueExA`, `RegCloseKey` only — no create/set/delete/enumeration),
  `HKLM` is the only root, four handle slots (`0x1000 + slot`) with reuse
  after close, values are returned as NUL-terminated REG_SZ bytes only, key
  existence means children-only (`prefix + "/"`), and failures set the
  thread last-error without killing the thread (our documented contract).
  No hives, ACLs, transactions, redirection, default/unnamed values, other
  roots, write paths, subkey iteration, typed values beyond REG_SZ,
  `W`/Unicode variants, remote registry or change notifications; `samDesired`
  is ignored and `REG_OPTION_OPEN_LINK` is rejected 87. Always-NUL-terminated
  values and NULL-subkey-opens-`HKLM` are project contracts (spec §7/§9), not
  vendor-verified. No kernel defect was found; one harness expectation (the
  `\r\r\n` size-query terminator) and one stale-ISO rerun (the first `pe`
  regression) were fixed before the PASS.
- Phase 8 stage-4 limits (spec: `docs/WIN32_REGISTRY_WRITE.md` §14,
  ADR-0025): eighteen allowlisted names across the same two DLLs; writes are
  `RegCreateKeyExA`/`RegSetValueExA`/`RegDeleteValueA` over a four-slot
  `HKLM` table, REG_SZ values only, `samDesired` ignored. Keys are lazy and
  children-only: they are created on the first successful write, and a key
  whose last value was deleted reports `created` again rather than being
  removed — there are no empty key objects. Write-path validation joins and
  checks the whole path (bad names/data → 87, invalid handle → 6, missing
  value → 2, store-full → 8) unlike the stage-3 read miss rule (2/87).
  Values cap at the store's 128 bytes + NUL (`MAX_REG_VALUE` 129). No
  `RegDeleteKey`, enumeration, `RegQueryInfoKey`, `RegFlushKey` (every
  successful write saves), no hives/ACLs/transactions, no typed values
  beyond REG_SZ, no `W`/Unicode variants, no default/unnamed values, no other
  roots, no access rights checked (`KEY_SET_VALUE` is not honored), and no
  concurrency guarantees beyond the store's lock. NULL `lpSubKey` (create)
  and NULL `lpValueName`/`lpData` acceptance/rejection are project contracts
  (spec §7–§12), not vendor-verified. No kernel defect was found; one
  fixture-order bring-up defect (repeat create before the first write)
  was fixed in the fixture before the PASS.
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
- User-mode limits (Phase 6 stage 3, extended by Phase 7 stages 1–3 and
  Phase 8 stages 2–4): programs are our own ELF64 `ET_EXEC` modules or PE32+
  images loaded from Limine — the PE path resolves only the eighteen
  stage-2/stage-3/Phase-8-stage-2/stage-3/stage-4
  `kernel32.dll`+`advapi32.dll`
  names (see those limits above), and neither path has PIE/dynamic linking,
  PT_INTERP or ASLR, and there is no filesystem-backed exec; the code window
  is fixed at
  0x400000..0x500000 with a one-page stack at 0x500000.
  One user program runs at a time (the TSS `rsp0` is programmed for the sole
  user thread, so concurrent user threads are not supported); there is no
  fork/exec/wait, no guard page above the user stack, and the syscalls that
  write into user memory are `ReadFile` (line-mode console input) and the
  registry surface (`RegOpenKeyExA`/`RegQueryValueExA`/`RegCreateKeyExA`
  output pointers plus the `RegSetValueExA` value pointer, all
  validated); the syscall gate is `int 0x80` with a 4 KiB
  write cap, console-fd-only write and console-input-fd-only read. Skip paths
  print explicit skipped verdicts when modules are absent (not a pass).
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

Phase 8 stages 1–5 and Phase 9 stages 1–2 are complete: configuration store
(ADR-0022), Win32 console input (ADR-0023, R-016), read-only registry
(ADR-0024, R-017), registry write (ADR-0025, R-018), registry
enumeration/deletion (ADR-0026, R-019), the PCI bus-0 inventory
(docs/PCI.md, ADR-0027, R-020) and the driver binding model
(docs/DRIVER_MODEL.md, ADR-0028, R-021). Final verified state: 28 PASS /
0 FAIL / 2 SKIP; unit suite 280; kernel `2ce7c393...`, driver session
`f5578086...`.
Single best next action: **Phase 9 stage 3 — resource ownership and the first
real lifecycle-managed driver**, defining MMIO/DMA/polled-IRQ ownership and
stop/restart for the e1000 behind the model, with mock failure tests and the
network harness as the regression gate. Virtual GPU work and the Phase 8/9
acceptance matrices remain open and are not claimed.
