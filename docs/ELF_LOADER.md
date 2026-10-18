# Native ELF64 program loading (Phase 6, stage 3) — design spec

Status: **approved 2026-10-17, implemented 2026-10-18 with recorded deviations
(§12).** This document is the contract for the stage: code, tests and logs must
match it, and any deviation must be recorded here or in `docs/DECISIONS.md`.

## 1. Goal and non-goals

Goal: replace the two hard-coded ring-3 demo programs (inline assembly in
`kernel/src/user.rs`) with **real ELF64 executables built from Rust source**,
shipped as Limine boot modules, parsed by a host-tested `os_core::elf` parser,
loaded by a kernel loader into a fresh user address space and executed at
CPL3. The boot self-test must prove, from serial evidence: a well-formed ELF
writes to the console and exits 0 through `int 0x80`; a second ELF faults and
is isolated; a deliberately corrupt ELF module is **rejected cleanly** by the
loader (no panic, boot continues).

Deliberately **not** in this stage:

- no `ET_DYN`/PIE, no relocations, no interpreter (`PT_INTERP`), no dynamic
  linking — `ET_EXEC` with static addresses only;
- no ASLR/slide — programs are linked at fixed low addresses;
- no loading from the VFS (the 8192-byte file cap and no on-disk exec format
  make modules the right vehicle for this stage; disk-backed exec is future
  work);
- no multi-process lifecycle: programs still run as one-shot scheduler
  threads, one at a time; no fork/exec/wait API;
- no shell `exec` builtin (decided: boot self-test only in this stage);
- no new syscalls; the existing `int 0x80` ABI is unchanged;
- programs are our own tiny Rust binaries, not third-party applications.

## 2. Research and primary sources (R-010)

Recorded fully in `docs/RESEARCH.md` as R-010. Summary of verified facts:

- **Limine module request** (protocol base revision 6, pinned bootloader
  12.9.3): ID `{COMMON_MAGIC, 0x3e7e279702be32af, 0xca1c4f3bd1280cee}`,
  response `{revision, module_count, modules}` where each entry is a
  `limine_file`. "If no modules are available, no response will be provided" —
  the loader must treat a NULL response as zero modules. Source:
  `limine-protocol` PROTOCOL.md §Module Feature, fetched 2026-10-17.
- **`limine_file` pointer semantics**: "All non-NULL pointers point to the
  object with the HHDM offset already added to them"; `address` is "always at
  least 4KiB aligned" and all 4KiB chunks a module covers belong exclusively
  to it; `path` is 0-terminated ASCII with a leading slash (e.g.
  `/boot/hello.elf`). Modules are mapped by Limine at boot; the kernel reads
  them through the HHDM and never frees that memory in this stage.
- **ELF64**: System V gABI / x86-64 psABI program-header semantics
  (`PT_LOAD`, `p_offset/p_vaddr/p_filesz/p_memsz/p_flags`, `e_entry`), little
  endian, `EM_X86_64`, `ET_EXEC`. Our own parser is implemented from the
  field definitions; no code is copied.

Decision (with alternatives rejected):

- **ET_EXEC only.** Supporting `ET_DYN` needs a relocation engine (R_X86_64_*
  processing) with no consumer yet; reject with a named error instead.
- **Two-phase loader API** (`inspect` then `map`) so the corrupt-module test
  never needs a scratch address space and rejection happens before any page is
  allocated.
- **Page-aligned `PT_LOAD` segments required.** Our linker script emits them;
  allowing unaligned segments would force byte-granular copy/mapping logic
  whose only consumer would be non-conforming toolchains.
- **Runtime rejection probe with `bad.elf`.** Host tests cover malformed
  vectors; an actual corrupt module on the ISO proves the kernel's error path
  end-to-end (no panic, boot continues, no `,elf` marker masking the failure).

## 3. Artifact layout

```
userland/                      # independent cargo workspace (not a member of the root
│                              # workspace). Cargo JOINS target.*.rustflags arrays from
│                              # every ancestor config file, so the inherited
│                              # -Tkernel/linker.ld cannot be un-set from a config file
│                              # (see §12, deviation 1). scripts/build_user.sh passes the
│                              # userland flags via RUSTFLAGS, which replaces all config
│                              # rustflags; cargo runs rustc with the workspace root as
│                              # cwd, so -Tlinker.ld resolves inside userland/ regardless
│                              # of where the build is invoked from.
├── .cargo/config.toml         # [build] target = x86_64-unknown-none + comment only
├── Cargo.toml                 # [workspace] members = ["runtime", "hello", "fault"]
├── linker.ld                  # ENTRY(_start); . = 0x400000; page-aligned PHDRS
├── runtime/                   # no_std shared runtime library
│   └── src/lib.rs             # syscall shims, entry(), panic handler
├── hello/                     # writes the ring-3 greeting, exit 0
│   └── src/main.rs
└── fault/                     # writes to an unmapped address, expected #PF
    └── src/main.rs
```

Userland rustc flags (set by `scripts/build_user.sh` via `RUSTFLAGS`):
`relocation-model=static`, `code-model=small`, `-Tlinker.ld`,
`-zmax-page-size=4096`, `--build-id=none`.

Built artifacts staged into `artifacts/`:

| Artifact | Source | Purpose |
|---|---|---|
| `artifacts/hello.elf` | `userland/hello` | valid program: 1 console write, exit 0 |
| `artifacts/fault.elf` | `userland/fault` | valid program: user page fault, isolated |
| `artifacts/bad.elf` | `hello.elf` with `e_ident[0..4]` zeroed | loader rejection probe |

`bad.elf` is generated deterministically by `scripts/build_user.sh` (Python),
never authored by hand; its transform is documented here and in the script.

## 4. Userland runtime and programs

- `runtime` (crate `project_os_user_runtime`, `#![no_std]`) provides:
  - `sys_write(fd: u64, buffer: &[u8]) -> u64` — `rax=1`, `rcx=fd`,
    `rdx=buffer`, `r8=len` via `int 0x80`. The kernel's gate frame preserves
    every general-purpose register except `rax` (the return value), so the
    `asm!` block marks only `rax` as in/out and documents that assumption.
  - `sys_exit(code: u64) -> !` — `rax=2`, `rcx=code`, `options(noreturn)`
    (the kernel kills the thread inside the gate; execution never resumes).
  - `entry(program: fn()) -> !` — calls the program, then `sys_exit(0)`. Each
    binary (`#![no_std] #![no_main]`) defines
    `#[unsafe(no_mangle)] extern "C" fn _start() -> ! { entry(bin_main) }`.
  - `#[panic_handler]` — `sys_exit(0xBEEF)` so a panicking program terminates
    as a wrong exit code (the boot self-test then fails loudly) instead of
    spinning forever.
- `hello`: `bin_main` performs `sys_write(1, b"user: hello from ring 3 via
  int 0x80\n")` and returns; `entry` then exits 0 — the exact string the
  stage-5b fixture printed, so existing serial assertions remain valid.
- `fault`: `bin_main` performs one `write_volatile` to `0x5_0000_0000`
  (unmapped in the user address space). It cannot return: the #PF kills the
  thread. Should it ever return, `entry`'s exit-0 default would trip the
  self-test's zero-fault assertion — a failure, never a false pass.
- Programs are single-threaded, use no allocator, and carry no external
  crate dependencies (no `LICENSES.md` change).

## 5. `os_core::elf` — host-tested parser (new)

`os_core/src/elf.rs`, `no_std`, pure functions over `&[u8]` (little-endian
reads via `from_le_bytes`, so tests run on any host).

API (contract):

```rust
pub const EHDR_SIZE: usize = 64;
pub const PHDR_SIZE: usize = 56;
pub const MAX_PROGRAM_HEADERS: u64 = 64;
pub const PT_LOAD: u32 = 1;
pub const PF_X: u32 = 1; pub const PF_W: u32 = 2; pub const PF_R: u32 = 4;

pub struct ElfImage<'a> { /* bytes + validated header facts */ }
impl<'a> ElfImage<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<ElfImage<'a>, ElfError>;
    pub fn entry(&self) -> u64;
    pub fn program_headers(&self) -> ProgramHeaders<'a>; // yields Segment
}

pub struct Segment {
    pub offset: u64, pub vaddr: u64, pub filesz: u64, pub memsz: u64,
    pub flags: u32,
}
impl Segment {
    pub fn is_executable(&self) -> bool; pub fn is_writable(&self) -> bool;
    pub fn page_range(&self) -> core::ops::Range<u64>; // page-rounded vaddr range
}

pub enum ElfError { /* one variant per rejection, stable labels */ }
impl ElfError { pub fn label(&self) -> &'static str; }
```

Validation performed by `parse` (all errors have stable `label()` strings in
the style of `os_core::pofs`):

| Check | Error label |
|---|---|
| length < 64, or phdr table / any segment slice outside the image | `truncated` |
| `e_ident[0..4] != \x7fELF` | `bad magic` |
| `EI_CLASS != ELFCLASS64`, `EI_DATA != ELFDATA2LSB`, `EI_VERSION != 1` | `not 64-bit` / `not little endian` / `bad ident version` |
| `e_type != ET_EXEC` | `unsupported type` |
| `e_machine != EM_X86_64` | `wrong machine` |
| `e_version != 1`, `e_ehsize != 64`, `e_phentsize != 56` | `bad header sizes` |
| `e_phnum == 0` / `> MAX_PROGRAM_HEADERS` | `no program headers` / `too many program headers` |
| `p_align` not in `{0, 4096}`, or `p_vaddr % 4096 != 0` | `bad segment alignment` |
| `p_filesz > p_memsz`, `p_offset + p_filesz` overflows or `> len` | `segment size mismatch` / `segment out of file` |
| `p_vaddr + p_memsz` overflows, or the page-rounded end overflows | `segment range overflow` |
| `p_flags & !7 != 0`, or `PF_W | PF_X` | `bad segment flags` / `write+execute segment` |
| two `PT_LOAD` page ranges overlap | `overlapping segments` |
| `e_entry` outside every executable `PT_LOAD` (`vaddr..vaddr+memsz`) | `entry not executable` |

Contract points:

- Only `PT_LOAD` segments are validated/mapped; other program-header types are
  ignored (documented; future stages may reject them explicitly).
- `parse` validates everything up front; `program_headers()` re-reads the
  (already validated) table and yields plain `Segment` values without
  `Result`.
- The parser is address-space agnostic; the user-address window is kernel
  policy enforced by the loader (§6).

Host tests (in-module, added to the 77-test `os_core` suite): a synthetic
image builder produces valid minimal images; corruption vectors cover every
row above, plus: truncated at header/phdr/segment boundaries, `p_offset`
alignment freedom (files may be unaligned), entry exactly at segment start and
one byte past `memsz` (reject), executable-but-empty (`memsz == 0`) segment,
and a multi-segment image with distinct R-X/RW flags (accept).

## 6. Kernel loader (new `kernel/src/loader.rs`)

Constants (kernel policy, not parser):

```rust
pub const CODE_BASE: u64 = 0x40_0000; // 1 MiB code window, page-aligned
pub const CODE_END: u64 = 0x50_0000;
// Stack stays where stage 5b put it: one page at STACK_VA = 0x50_0000,
// STACK_TOP = 0x50_1000 (owned by user.rs).
```

API (contract):

```rust
pub struct Inspection { entry: u64, segments: [SegmentRange; MAX_SEGMENTS], count: usize }
pub enum Reject { Elf(os_core::elf::ElfError), OutsideWindow, TooManySegments, OutOfMemory, MapFailed }

pub fn inspect(module: &limine::File) -> Result<Inspection, Reject>;
pub fn map(space: &mut AddressSpace, module: &limine::File, image: &Inspection)
    -> Result<(), Reject>;
```

Module lookup itself stays in `limine.rs` (`modules()` / `module_by_path()`),
so `Reject` carries no lookup variants; a missing module is a user.rs skip
decision (§7), not a loader rejection.

- `inspect`: slices the module bytes through the HHDM (module addresses are
  HHDM-virtual, §2), calls `os_core::elf::parse`, checks every `PT_LOAD` page
  range lies inside `CODE_BASE..CODE_END` (`OutsideWindow`), and copies
  segment ranges into the fixed-size `Inspection` (more than
  `MAX_SEGMENTS = 16` executable segments → `TooManySegments`). No memory is
  allocated; `bad.elf` fails here.
- `map`: for each segment page: `memory::alloc_frame`, zero the whole frame
  through the HHDM, copy the `filesz` payload from the module slice at the
  matching file offset, then `space.map_page(vaddr_page, frame, flags)` with
  flags derived from `p_flags` and the kernel W^X policy:
  - executable → `USER_CODE` (read+execute, no write);
  - writable → `USER_DATA` (write+NX);
  - read-only data → `PRESENT | USER | NO_EXECUTE`.
  On any allocation/mapping failure, unmap and free every page mapped so far
  and return `OutOfMemory`/`MapFailed` (best effort; the space is then
  destroyed by the caller).
- `limine.rs` additions: full `limine_file` `#[repr(C)]` (112 bytes: revision,
  address, size, path, string, media_type, unused, tftp_ipv4, tftp_port,
  partition_index, mbr_disk_id, three UUIDs), `ModuleResponse`, and a
  `ModuleRequest` with the two request-revision-1 fields appended for ABI
  completeness while requesting **revision 0** (`internal_module_count = 0`,
  `internal_modules = null`), static in `.limine_requests` like the other
  requests. Helper `modules()` returns `None` for a NULL response;
  `module_by_path(path)` compares `limine::cstr(file.path)` exactly
  (`/boot/hello.elf` etc.; exact match chosen over suffix matching so a
  mis-staged ISO fails visibly).

## 7. Boot flow and serial contract (`kernel/src/user.rs`, `main.rs`)

`user::demo` is rewritten; the inline-assembly fixtures and their exports are
deleted (the ring-3 trampoline `project_os_enter_user_mode` is unchanged, but
is now called with the ELF entry address). Flow, in order, one program at a
time on its own scheduler thread, reusing a single address-space slot:

1. `limine::modules()` — NULL/no modules: print
   `user: no Limine modules present; skipping the ELF self-test` and return
   `Ok` with `elf = false` (marker keeps no `,elf`; the QEMU ELF test then
   fails on the missing marker/lines, so a skip can never pass unnoticed).
2. Print `elf: {n} modules present`.
3. Locate `/boot/hello.elf`, `/boot/fault.elf`, `/boot/bad.elf`. Any missing:
   print `user: ELF self-test skipped (`{path}` module missing)` and return
   `Ok` with `elf = false`.
4. `bad.elf` probe: `loader::inspect` must fail; print
   `user: /boot/bad.elf rejected ({label})` (label from `Reject`, e.g.
   `bad magic`). If it **succeeds**, return
   `Err("corrupt module bad.elf was accepted")` → boot fatal (test fails
   loudly).
5. `hello.elf`: `inspect` + `map` into a fresh `AddressSpace::new_like_kernel`,
   map the zeroed one-page stack at `STACK_VA` (`USER_DATA`), activate, spawn
   thread `user-hello`, enter ring 3 at `image.entry` with `STACK_TOP`. The
   thread publishes `(AddressSpace, Inspection)` into a static slot (the
   stage-5b `SPACE_*` slots generalize to one slot holding both, since
   programs run one at a time). After `wait_all_finished` the boot thread
   returns to the kernel space, unmaps every page in the `Inspection` segment
   ranges plus the stack page, and destroys the space. `demo` keeps the
   existing assertions: exit code 0, exactly one write, bytes > 0, zero
   rejected syscalls; report gains `elf: true`.
6. `fault.elf`: same lifecycle, thread `user-fault`; asserts exactly one
   additional isolated user fault; report `user_faults` counts it as today.
7. The summary line is unchanged in shape:
   `user: ring-3 program exited (code 0), 1 write(s), N bytes, 1 isolated
   fault(s), 0 rejected syscall(s)`; the verdict line becomes
   `user selftest: passed (ELF64 modules loaded, int 0x80 write/exit from
   CPL3, user fault isolated, corrupt module rejected)`. On a skip (`elf`
   false) neither line is printed; `main.rs` prints
   `user selftest: skipped (no ELF modules present)` instead, so a skipped
   self-test can never appear as a pass (see §12, deviation 4).
8. `main.rs`: the marker inserts `,elf` between `user` and `vfs` when
   `report.elf` — final no-disk marker for the full build:
   `checks=handoff,memmap,gdt,idt,exceptions,timer,allocator,heap,vm,sched,user,elf,vfs`
   (with disk: `...,elf,vfs,blk,persist`). Inserting before `vfs` keeps the
   stage-2 persistence test's `vfs,blk,persist` suffix assertion valid.

Existing lines that must remain byte-identical: `syscall: thread exits with
code 0`, `user fault isolated: vector=14 error=... rip=... cr2=... (thread
killed, kernel continues)`, `blk:`/`persist:` lines, the stage log lines.
`test_boot_qemu.sh` (marker + exit 33), `test_shell_qemu.py` (version line)
and `test_persistence_qemu.py` (marker suffix) are unaffected by the marker
insertion and are **not** modified except where this spec says so.

## 8. ISO staging

- `boot/limine.conf` gains three lines under `/PROJECT_OS`:
  `module_path: boot():/boot/hello.elf`, `module_path: boot():/boot/fault.elf`,
  `module_path: boot():/boot/bad.elf`. The interactive variant inherits them
  (the same config is copied and only `debug_exit` is stripped).
- `scripts/make_iso.sh` copies the three ELFs from `artifacts/` into
  `iso_root/boot/`, and — mirroring the Limine fetch behavior — invokes
  `scripts/build_user.sh` first if any of the three artifacts is missing.
  Missing userland sources/artifacts must fail with a clear `die`, not produce
  a module-less ISO silently.
- `scripts/verify_iso.py` is **not** extended: module presence is proven
  end-to-end by the ELF boot test (kernel reads them through Limine and
  asserts their content via exit codes); duplicating ISO9660 directory
  parsing here would add code without new evidence.

## 9. Build and test scripts

- `scripts/build_user.sh` (new): requires `python` (like the rest of the test
  harness); runs `cargo build --release` with cwd `userland/` and the userland
  flags in `RUSTFLAGS` (§3); copies
  `userland/target/x86_64-unknown-none/release/{hello,fault}` to
  `artifacts/{hello,fault}.elf`; generates `artifacts/bad.elf` by zeroing
  bytes 0..4 of `hello.elf`; runs `scripts/check_user_elf.py` on `hello.elf`
  and `fault.elf` (must pass) and on `bad.elf` (must fail with `bad magic`);
  logs sizes and SHA-256s. Follows `lib.sh` conventions and exit codes.
- `scripts/format.sh` and `scripts/lint.sh` are extended so the quality gates
  keep covering "all crates" (AGENTS.md): `cargo fmt --check` and
  `cargo clippy -- -D warnings` in the `userland/` workspace too (the
  `x86_64-unknown-none` build target comes from its own config).
- `scripts/check_user_elf.py` (new): dependency-free header checker: 64-bit
  LSB x86-64 `ET_EXEC`, `e_entry` inside an executable `PT_LOAD`, every
  `PT_LOAD` page-aligned, non-overlapping, `filesz <= memsz`, no W+X, all
  page ranges inside `0x400000..0x500000`. Prints a one-line summary
  (entry, segment list). Fast, clear diagnostics for build misconfiguration
  ahead of the QEMU run.
- `scripts/test_elf_qemu.sh` (new): reuses `test_boot_qemu.sh` (non-interactive
  ISO, BIOS, exit 33 expected) with `--serial-log artifacts/qemu-serial-elf.log`,
  then asserts, with `grep -q` per line and `die` on any miss:
  - `elf: 3 modules present`
  - `user: hello from ring 3 via int 0x80` (the write came from the ELF)
  - `syscall: thread exits with code 0`
  - `user fault isolated: vector=14`
  - `user: /boot/bad.elf rejected (bad magic)`
  - `user selftest: passed (ELF64 modules loaded`
  - the marker contains `sched,user,elf,vfs`
  No `PROJECT_OS_FATAL`/`PROJECT_OS_PANIC` in the log (already enforced by
  `test_boot_qemu.sh`).
- `scripts/test_all.sh`: adds `run userland "$ROOT_DIR/scripts/build_user.sh"`
  before the existing `iso` step (so the full-build ISO contains modules) and
  `run elf "$ROOT_DIR/scripts/test_elf_qemu.sh"` after the QEMU boot rows.
  Expected summary after this stage: PASS grows by 2 (userland, elf); SKIP set
  unchanged.
- `scripts/test_unit.sh` / `test_memory.sh` / `test_graphics_host.sh` /
  `test_persistence_qemu.sh` unchanged; persistence test still passes because
  `,elf` is inserted before `vfs`.
- `.gitignore`: add `/userland/target/`.

## 10. Documentation and evidence updates (on implementation)

- `docs/RESEARCH.md`: R-010 with the sources, claims and confidence above.
- `docs/DECISIONS.md`: ADR-0017 (native ELF64 loading, userland workspace,
  ET_EXEC-only, two-phase loader, corrupt-module runtime probe) including the
  alternatives rejected.
- `docs/ARCHITECTURE.md`: `userland/` workspace row, `loader.rs` row,
  `os_core::elf` row, Limine module request row, updated limitations.
- `docs/TESTING.md`: new `elf` row with the exact assertions; note that the
  no-disk marker now reads `...,user,elf,vfs` (stage-2 "byte-identical
  marker" wording gets a correction note, not a silent edit).
- `docs/ROADMAP.md`: Phase 6 stage 3 complete; remaining acceptance items.
- `progress.md`: milestone entry, exact commands, artifact table with sizes
  and SHA-256s (`hello.elf`, `fault.elf`, `bad.elf`, `kernel.elf`, ISOs),
  serial excerpts, defects, next action; `STOP.md` left `STOP=NO`.
- `LICENSES.md`: no change (no new third-party code or crates; userland uses
  the same Rust toolchain already documented).

## 11. Evidence and limits (claims this stage may and may not make)

May claim (after tests pass, with logs recorded): ELF64 executables built from
Rust source, shipped as Limine modules, are parsed by a host-tested parser,
loaded into a fresh paged user address space, executed at CPL3, and serviced
through the existing syscall gate; a corrupt module is rejected without
harming boot; fault isolation still holds. Evidence: `os_core` unit tests,
`check_user_elf.py`, `test_elf_qemu.sh` serial log, full `test_all.sh` run,
VirtualBox re-run.

May **not** claim: dynamic linking, PIE, ASLR, filesystem-backed exec, general
POSIX/Windows application support, multi-process semantics, 2 MiB pages, or
support for third-party x86-64 binaries. Limits to restate in docs: fixed VA
windows (code 1 MiB, stack one 4 KiB page), max 64 program headers / 16 load
segments per image, W^X per segment, one user thread at a time, modules from
the bootloader only, QEMU/VirtualBox only.

## 12. Deviations recorded on implementation (2026-10-18)

1. **Userland rustc flags via `RUSTFLAGS`, no `kernel/build.rs`.** Cargo joins
   `target.*.rustflags` arrays across ancestor config files (verified: the
   root `-Tkernel/linker.ld` leaked into userland builds), so §3's original
   "closest file wins per key" claim was wrong and a config-only fix does not
   exist. Moving the kernel link flags to a `build.rs` was implemented, then
   abandoned: build-script executables live under
   `target/release/build/<pkg>-<hash>/build_script_build-<hash>.exe`, whose
   absolute path reaches 260 characters in this checkout, so MSVC `link.exe`
   fails with `LNK1104` (no long-path support). Final state: root
   `.cargo/config.toml` unchanged from stage 2; `userland/.cargo/config.toml`
   holds only `[build] target` plus a comment; `scripts/build_user.sh` sets
   `RUSTFLAGS` (replaces config rustflags; verified by a clean rebuild
   producing the same artifact hashes).
2. **`limine_file` is 112 bytes, not 100**; header re-verified against
   `limine-protocol` trunk `include/limine.h` (`tftp_ipv4` is `uint8_t[4]`).
   §6 corrected; the kernel only reads `revision/address/size/path`.
3. **`Inspection` stores full `os_core::elf::Segment` values** instead of a
   separate `SegmentRange` type (private field, no behavioral change); page
   ranges are derived with `Segment::page_range()` for window checks and
   teardown. `map` therefore parses the image only once.
4. **Skip paths print honest lines.** `main.rs` gates both the ring-3 summary
   line and the ELF verdict line on `report.elf`; a skip prints
   `user selftest: skipped (no ELF modules present)`. The QEMU ELF test fails
   on any skip because the required lines and `,elf` marker are absent.
5. **`cargo clippy --all-targets` is impossible in `userland/`**: the `test`
   crate does not exist for `x86_64-unknown-none` (`E0463`). Lint uses
   `cargo clippy -- -D warnings` as §9 already specified.
