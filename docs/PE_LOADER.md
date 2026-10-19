# Native PE32+ loading (Phase 7, stage 1) — design spec

Status: **draft for implementation 2026-10-18.** This document is the contract
for the stage: code, tests and logs must match it, and any deviation must be
recorded here or in `docs/DECISIONS.md`.

## 1. Goal and non-goals

Goal: load and execute a **real PE32+ AMD64 console image** built from Rust
source with the project's own toolchain, at CPL3, under the existing `int 0x80`
syscall gate. The boot self-test must prove, from serial evidence:

- a well-formed `pehello.exe` is parsed by a host-tested `os_core::pe` parser,
  mapped into a fresh user address space with W^X section permissions, has its
  **base relocations applied** (the image is deliberately loaded at a different
  base than it was linked at, and the fixture itself verifies the relocated
  pointer at runtime), writes to the console and exits 0;
- a deliberately corrupt `pebad.exe` module is **rejected cleanly** (no panic,
  boot continues);
- the success marker appends `,pe` only when the PE self-test passed.

Deliberately **not** in this stage:

- no imports/IAT processing, no DLLs, no exports — the fixture reaches the
  kernel only through the existing syscall gate (the gate was already shaped
  Windows-style: `rax` service, `rcx/rdx/r8/r9` arguments; stage 2 adds
  import-directory parsing and IAT resolution for three allowlisted
  `kernel32.dll` names — `docs/WIN32_CONSOLE.md`, ADR-0020; stage 3 grows the
  same allowlist to eleven names, adds the five-argument stub form and the
  stub-page command-line tail — `docs/WIN32_PROGRAM.md`, ADR-0021);
- no Win32 API services yet (that is stage 2 of Phase 7: a named console
  fixture set against our own API surface);
- no exception directory (`.pdata`) processing, no SEH, no unwind — the
  directory is ignored, not interpreted;
- no TLS, no CLR, no delay imports, no COFF object/symbol tables, no
  certificates, no resources;
- no ASLR/randomization: the loader always loads at the fixed code window base;
- no loading from the VFS; Limine modules only, one user thread at a time;
- no 32-bit PE (PE32) — PE32+ only, matching the x86-64 target;
- **no Wine, no MinGW, no Windows SDK, no MSVC**: fixtures are linked by
  `rust-lld` (shipped with the Rust toolchain) from `rustc`-produced COFF
  objects for the `x86_64-pc-windows-msvc` target, freestanding (`/nodefaultlib`,
  our own entry, no CRT).

## 2. Research and primary sources (R-013)

Recorded fully in `docs/RESEARCH.md` as R-013. Summary of verified facts
(Microsoft *PE Format* specification, fetched 2026-09-10 revision on
2026-10-18):

- **MS-DOS stub + signature**: offset `0x3C` holds `e_lfanew`, the file offset
  of the 4-byte `PE\0\0` signature; headers follow.
- **COFF file header**: `Machine` (AMD64 = `0x8664`), `NumberOfSections`,
  `TimeDateStamp`, `PointerToSymbolTable`, `NumberOfSymbols`,
  `SizeOfOptionalHeader`, `Characteristics` (`IMAGE_FILE_EXECUTABLE_IMAGE`
  `0x0002`, `IMAGE_FILE_RELOCS_STRIPPED` `0x0001`, `IMAGE_FILE_DLL` `0x2000`).
- **Optional header (PE32+)**: magic `0x20B`; `AddressOfEntryPoint` (RVA),
  `ImageBase` (multiple of 64 KiB), `SectionAlignment` (>= `FileAlignment`),
  `FileAlignment` (power of two, 512..64K), `SizeOfImage` (multiple of
  `SectionAlignment`), `SizeOfHeaders`, `Subsystem` (CUI = 3),
  `NumberOfRvaAndSizes` and the directory table; `SizeOfOptionalHeader` bounds
  every data-directory probe.
- **Section table**: 40-byte entries immediately after the optional header;
  `VirtualSize`/`VirtualAddress` (multiple of `SectionAlignment`, ascending,
  adjacent), `SizeOfRawData` (multiple of `FileAlignment`; may exceed
  `VirtualSize`), `PointerToRawData` (multiple of `FileAlignment`, zero for
  uninitialized sections); memory size is the page-rounded `VirtualSize`, with
  `SizeOfRawData < VirtualSize` zero-filled; flags `IMAGE_SCN_MEM_EXECUTE`
  `0x20000000`, `MEM_READ` `0x40000000`, `MEM_WRITE` `0x80000000`,
  `CNT_INITIALIZED_DATA` `0x40`.
- **Base relocations** (data directory 5; PE32+ directory offset 152): a
  sequence of blocks, each `{ Page RVA: u32, Block Size: u32 }` followed by
  2-byte entries whose high 4 bits are the type and low 12 bits an offset in
  the page. Type `0` (`ABSOLUTE`) is skipped padding; type `3` (`HIGHLOW`,
  32-bit) and type `10` (`DIR64`, 64-bit) add the delta between the actual and
  preferred image base to the field at `ImageBase + PageRVA + offset`. The
  delta is zero when the image loads at its preferred base.
- **Windows loader limits**: at most 96 sections; this stage caps far lower
  (§5, §11).
- **Toolchain facts (verified by probe, 2026-10-18)**: `rustc 1.99.1` with the
  `x86_64-pc-windows-msvc` target and `rust-lld -flavor link` link a `no_std`
  binary with `/nodefaultlib /entry:_start /subsystem:console` into a valid
  PE32+ image (probe: 3584 bytes, 5 sections, entry RVA `0x1000`, image base
  `0x140000000`, relocation directory with one DIR64 block). No MSVC, MinGW or
  Windows SDK is involved.

Decision (with alternatives rejected):

- **Real linker output, not hand-emitted images.** A hand-built PE writer
  (`tools/pegen`) was considered and rejected: it would test our own
  simplification instead of the format as produced by a real toolchain
  (`SizeOfHeaders`, `.pdata`, `.reloc` padding, EDATA quirks). `rust-lld` is
  already in the pinned toolchain, so no new tool is introduced.
- **Load at the fixed code window base (`0x400000`), always relocate.**
  Loading at the image's preferred base would make the relocation path
  unreachable in practice; the fixture is linked at the lld default base
  (`0x140000000`) precisely so the harness exercises a nonzero delta.
- **Two-phase loader API** (`inspect` then `map`), mirroring the ELF loader:
  `pebad.exe` must be rejected before any page is allocated.
- **Reject-by-directory policy for unprocessed tables.** Imports, delay
  imports, TLS and CLR directories must be zero; a fixture that needs them
  fails loudly instead of running with silently missing semantics. Export,
  resource, exception, debug, load-config and certificate directories are
  ignored (their absence of effect is documented, not asserted).

## 3. Artifact layout

```
userland/pe/                   # independent cargo workspace (excluded from the
│                              # userland/ workspace; different target and link
│                              # flags must not inherit the kernel ones)
├── Cargo.toml                 # [workspace], member: pehello; panic=abort
├── .gitignore                 # target/
└── pehello/
    └── src/main.rs            # #![no_std] #![no_main] fixture
```

The fixture crate is built for `x86_64-pc-windows-msvc` (the same target as
the Windows host toolchain; on Linux hosts `rustup target add
x86_64-pc-windows-msvc`). rustc flags (set by `scripts/build_user_pe.sh` via
`RUSTFLAGS`):

```
-C linker=<sysroot>/lib/rustlib/<host>/bin/rust-lld[.exe]
-C link-arg=/nodefaultlib
-C link-arg=/entry:_start
-C link-arg=/subsystem:console
```

Built artifacts staged into `artifacts/`:

| Artifact | Source | Purpose |
|---|---|---|
| `artifacts/pehello.exe` | `userland/pe/pehello` | valid PE32+ console image: two console writes (greeting + relocation verdict), exit 0 |
| `artifacts/pebad.exe` | `pehello.exe` with the 4 signature bytes at `e_lfanew` zeroed | loader rejection probe (`bad signature`) |

`pebad.exe` is generated deterministically by `scripts/build_user_pe.sh`
(Python), never authored by hand; its transform is documented here and in the
script.

## 4. The fixture (`userland/pe/pehello`)

- `#![no_std] #![no_main]`, no allocator, no external crates; `panic` handler
  loops (never expected; the harness treats any hang as failure via timeouts).
- Entry `#[unsafe(no_mangle)] extern "C" fn _start() -> !` performs exactly two
  `int 0x80` writes (`rax=1`, `rcx=1`, `rdx=buffer`, `r8=len`) and one exit
  (`rax=2`, `rcx=code`):
  1. `pe: hello from a PE32+ image via int 0x80\n` (ASCII, `\n`);
  2. `pe: base relocation verified\n` — **only** if the relocated pointer
     matches, otherwise `pe: base relocation mismatch\n` and exit code 1.
- The relocation probe: a `global_asm!` slot in `.data`
  (`marker_slot: .quad marker`) creates a genuine absolute relocation in the
  real toolchain output. At runtime the fixture compares
  `read_volatile(marker_slot)` (base-relocated by our kernel loader) with the
  runtime address of `marker` (a rip-relative `lea` the loader never touches).
  Equal proves the loader computed and applied the DIR64 relocation correctly
  under a nonzero delta; unequal fails the boot self-test via exit code 1 and
  the mismatch line.
- Programs run single-threaded, one at a time, on the existing user-thread
  machinery; the stack is the same single zeroed page at `0x50_0000` the ELF
  fixtures use.

## 5. `os_core::pe` — host-tested parser (new)

`os_core/src/pe.rs`, `no_std`, pure functions over `&[u8]` (little-endian
reads, host-testable). Caps (documented, stricter than the Windows loader
limit of 96 sections): `MAX_SECTIONS = 32`, `MAX_RELOCATIONS = 256` DIR64
entries per image (bounded so the parser and the kernel load plan stay small
enough for the 16 KiB kernel thread stacks).

API (contract):

```rust
pub const MAX_SECTIONS: usize = 32;
pub const MAX_RELOCATIONS: usize = 256;
pub const MAX_IMPORT_DLLS: usize = 2;
pub const MAX_IMPORTS: usize = 16;
pub const MAX_IMPORT_DLL_NAME: usize = 16;
pub const MAX_IMPORT_FUNCTION_NAME: usize = 32;
pub const IMAGE_FILE_RELOCS_STRIPPED: u16 = 0x0001;
pub const IMAGE_SCN_MEM_EXECUTE: u32 = 0x2000_0000;
pub const IMAGE_SCN_MEM_READ: u32 = 0x4000_0000;
pub const IMAGE_SCN_MEM_WRITE: u32 = 0x8000_0000;

pub struct Section {
    pub name: [u8; 8], pub virtual_address: u32, pub virtual_size: u32,
    pub raw_offset: u32, pub raw_size: u32, pub characteristics: u32,
}
impl Section {
    pub fn is_executable(&self) -> bool;   // MEM_EXECUTE
    pub fn is_writable(&self) -> bool;     // MEM_WRITE
    pub fn is_readable(&self) -> bool;     // MEM_READ
    pub fn memory_size(&self) -> u64;      // max(virtual_size, raw_size)
    pub fn rva_range(&self) -> core::ops::Range<u64>; // va .. va + memory_size
}

pub struct ImportSlot { /* dll + function name + iat_rva */ }
impl ImportSlot {
    pub const EMPTY: ImportSlot;
    pub fn dll(&self) -> &[u8];
    pub fn name(&self) -> &[u8];
}

pub struct PeImage<'a> { /* bytes + validated header facts + copies */ }
impl<'a> PeImage<'a> {
    pub fn parse(bytes: &'a [u8]) -> Result<PeImage<'a>, PeError>;
    pub fn image_base(&self) -> u64;
    pub fn entry_rva(&self) -> u32;
    pub fn size_of_image(&self) -> u32;
    pub fn relocation_stripped(&self) -> bool;
    pub fn sections(&self) -> &[Section];
    pub fn relocations(&self) -> &[u32];   // DIR64 target RVAs, ABSOLUTE skipped
    pub fn imports(&self) -> &[ImportSlot]; // stage 2, see §6.1
}

pub enum PeError { /* one variant per rejection, stable labels */ }
impl PeError { pub fn label(&self) -> &'static str; }
```

Validation performed by `parse` (stable labels in the style of
`os_core::elf`):

| Check | Error label |
|---|---|
| shorter than `0x40`, header/table/directory slices outside the file | `truncated` |
| `[0..2] != "MZ"` | `bad dos magic` |
| signature at `e_lfanew` != `PE\0\0` (including `e_lfanew` out of bounds) | `bad signature` |
| optional-header magic != `0x20B` | `not PE32+` |
| `Machine != 0x8664` | `wrong machine` |
| `IMAGE_FILE_EXECUTABLE_IMAGE` not set | `not an executable image` |
| `IMAGE_FILE_DLL` set | `is a dll` |
| `SizeOfOptionalHeader < 112`, or directories do not fit it | `bad optional header` |
| `NumberOfRvaAndSizes < 6` | `bad data directories` |
| `FileAlignment` not a power of two in 512..=64K, or `SectionAlignment` not a power of two >= `FileAlignment` | `bad file alignment` / `bad section alignment` |
| `SizeOfImage == 0`, not a multiple of `SectionAlignment` | `bad size of image` |
| `NumberOfSections == 0` / `> MAX_SECTIONS` | `no sections` / `too many sections` |
| section table extends past the file | `section table out of file` |
| section `VirtualAddress` unaligned, sections overlapping or out of ascending order, `memory_size == 0`, `raw_size > 0` with `raw_offset % FileAlignment != 0` or `raw_size % FileAlignment != 0`, `raw_size == 0` with `raw_offset != 0`, section range not inside `SizeOfImage` | `bad section layout` |
| raw data range extends past the file | `section out of file` |
| `AddressOfEntryPoint` outside every executable section (`rva_range`) | `entry not executable` |
| `Subsystem != IMAGE_SUBSYSTEM_WINDOWS_CUI (3)` | `unsupported subsystem` |
| import directory malformed: `RVA == 0` with nonzero size, size `< 20` or not a multiple of 20, descriptors outside one section's raw data, no all-zero terminator, `Name`/`FirstThunk` zero, ILT/thunk/name outside one section's raw data | `bad import table` |
| import by ordinal (thunk bit 63 set) | `import by ordinal is not supported` |
| DLL name or function name empty, unterminated inside its section, or longer than `MAX_IMPORT_DLL_NAME` / `MAX_IMPORT_FUNCTION_NAME` | `import name too long` |
| more than `MAX_IMPORT_DLLS` descriptors | `too many import dlls` |
| more than `MAX_IMPORTS` name slots | `too many imports` |
| delay-import directory non-empty | `delay imports unsupported` |
| TLS directory non-empty | `tls unsupported` |
| CLR directory non-empty | `clr unsupported` |
| base-relocation directory malformed: block smaller than 8 bytes, odd block size, block past the directory, `PageRVA + offset` overflow | `bad relocation table` |
| relocation type other than 0 or 10 | `unsupported relocation type` |
| more than `MAX_RELOCATIONS` DIR64 entries | `too many relocations` |

Contract points:

- `pebad.exe` (signature zeroed) is rejected by the `bad signature` row; the
  ELF-style probe in §7 asserts the exact label.
- `IMAGE_FILE_RELOCS_STRIPPED` is **not** a parse error: it is exposed via
  `relocation_stripped()` and consumed by kernel policy (§6).
- DIR64 target RVAs are stored compactly; `ABSOLUTE` entries are padding and
  skipped. `HIGHLOW` is rejected by machine (PE32+ AMD64 images use DIR64).
- The relocation directory is walked even when `RELOCS_STRIPPED` is set (the
  flag is authoritative for policy, not for parsing); a stripped image with a
  nonzero delta is rejected by the kernel loader.
- The parser is address-space agnostic; the fixed load window and W^X policy
  live in the kernel loader.
- Import parsing (stage 2, contract in `docs/WIN32_CONSOLE.md` §4): the
  descriptor table must terminate with an all-zero record inside one section's
  raw data; thunks are 8 bytes with the ordinal bit rejected; `OriginalFirstThunk
  == 0` falls back to `FirstThunk`; each 64-bit name thunk points at a 16-bit
  hint followed by a NUL-terminated name; the slot's `iat_rva` is
  `FirstThunk + index * 8`. Slots are produced for imported *names* only;
  duplicate names are kept and deduplicated by the kernel loader. All names and
  thunks must stay inside one section's raw data.

Host tests (in-module, added to the `os_core` suite; a synthetic image builder
emits valid minimal images with per-field overrides): one positive test per
accessor plus a corruption vector for every row above, including truncated
`e_lfanew`, section table exactly at EOF and one byte past, `raw_size > 0`
with zero `raw_offset`, two overlapping sections, entry at section start, entry
one byte past an executable section, `VirtualSize > SizeOfRawData` zero-fill
range (accepted), `SizeOfRawData > VirtualSize` (accepted), relocation block
with `ABSOLUTE` padding (skipped), multi-block relocations, `e_lfanew`
pointing inside the file but past the headers, and — for imports — a valid
descriptor with ILT names (accepted, names and `iat_rva` verified), the
`OriginalFirstThunk == 0` IAT fallback, the zero-terminated descriptor rule,
size not a multiple of 20, an ordinal thunk, an over-long name, two DLL
descriptors accepted while three are rejected, sixteen imports accepted while
seventeen are rejected, and thunks/names outside section raw data. The suite
count is 252 tests after stage 2 (`scripts/test_all.sh`, `unit` step).

## 6. Kernel loader (new `kernel/src/pe_loader.rs`)

Constants: the existing code window `loader::CODE_BASE..loader::CODE_END`
(`0x40_0000..0x50_0000`, 1 MiB) is the load base for the whole image
(`loaded_base = CODE_BASE`); the stack stays at `STACK_VA = 0x50_0000`
(owned by `user.rs`). Kernel section/relocation caps: `MAX_SECTIONS = 16`,
`MAX_RELOCATIONS = 256` (the parser copies are filtered down here).

API (contract):

```rust
pub enum Reject {
    Pe(os_core::pe::PeError), OutsideWindow, TooManySections,
    WriteExecuteSection, StrippedRelocations, RelocationTarget,
    UnknownImportDll, UnknownImportFunction, ImportTarget,
    OutOfMemory, MapFailed,
}
pub struct Inspection {
    entry_rva, sections: [Section; 16], count,
    relocations: [u32; 256], relocation_count,
    imports: [ImportSlot; 16], import_count,
}
pub struct Loaded { pub image: Inspection, pub stub_page: u64, pub applied_relocations: u32 }
pub fn inspect(module: &limine::File) -> Result<Inspection, Reject>;
pub fn map(space: &mut AddressSpace, module: &limine::File, image: &Inspection)
    -> Result<Loaded, Reject>;   // stub_page is 0 when there are no imports
```

- `inspect`: slices the module bytes through the HHDM, calls
  `os_core::pe::PeImage::parse`, then policy-checks:
  - every section's page-rounded range `CODE_BASE + rva .. CODE_BASE +
    align_up(memory_size, 4K)` lies inside `CODE_BASE..CODE_END`
    (`OutsideWindow`);
  - at most 16 sections (`TooManySections`);
  - no section with `MEM_EXECUTE | MEM_WRITE` (`WriteExecuteSection`, W^X);
  - when `loaded_base != image_base` the image must be relocatable:
    `RELOCS_STRIPPED` or an empty relocation list is rejected
    (`StrippedRelocations`). No memory is allocated; `pebad.exe` fails here.
- `map`: for each section page: `memory::alloc_frame`, zero the whole frame
  through the HHDM, copy the file payload (`min(remaining, PAGE_SIZE)` from
  `raw_offset + (page - section_va)`, only while inside `raw_size`), then
  `space.map_page(CODE_BASE + page, frame, flags)` with flags derived from
  section characteristics and the kernel W^X policy:
  - executable → `USER_CODE` (read+execute);
  - writable → `USER_DATA` (write+NX);
  - otherwise → `PRESENT | USER | NO_EXECUTE`.
  On any failure, unmap and free every page mapped so far
  (`OutOfMemory`/`MapFailed`; the caller then destroys the space).
- Base relocations are applied after mapping, before the space is activated:
  `delta = CODE_BASE.wrapping_sub(image_base)`; for each stored DIR64 RVA:
  - the 8-byte target must lie inside **one** section whose
    `MEM_WRITE` is set, and must not cross a page boundary
    (`RelocationTarget`);
  - translate the target address with `space.translate`, read the u64 through
    the HHDM, add `delta` (wrapping), write it back, `invlpg` the page.
    `translate` returns the physical address **including the in-page offset**
    (frame base for page-aligned inputs), which the write requires.
  Returns the applied count for the boot log; a nonzero delta with zero
  applied relocations cannot occur (rejected in `inspect` except when
  relocations exist precisely to be applied).
- Import policy (`docs/WIN32_CONSOLE.md` §5; `docs/WIN32_PROGRAM.md` §5, stage
  3): `inspect` rejects any import whose DLL name is not exactly
  `kernel32.dll` (`UnknownImportDll`) or whose function name is not in the
  fixed service table (`UnknownImportFunction`) — three entries in stage 2,
  eleven in stage 3 — and any IAT slot that is not an 8-byte, in-page
  location inside a readable section of the window (`ImportTarget`). No
  allocation happens before these checks, so the negative fixtures are
  rejected cleanly.
- `map` resolves imports after relocations: one stub per unique function name
  (first-occurrence order) is written as 16-byte records into a single
  read+execute page mapped at `WIN32_STUB_VA = 0x51_0000`, and every IAT slot
  is patched to the stub address. The 4-argument stub form is
  `mov eax, imm32; int 0x80; ret; nop*5`; the 5-argument form (stage 3, for
  `WriteFile`) is `mov r10, [rsp+0x28]; mov eax, imm32; int 0x80; ret`
  (`rsp+0x28` = return address + shadow space, the documented position of the
  first stack argument; `InterruptFrame` already captures `r10`), 13 bytes
  with 3 padding bytes instead of 8. `map` also writes the NUL-terminated
  module path at `COMMAND_LINE_VA = WIN32_STUB_VA + 0xF00` (capacity 0x100;
  explicit terminator because the frame is `int3`-filled) and fails with
  `Reject::CommandLineTooLong` beyond 255 path bytes. The IAT lives in
  read-only `.rdata` (R-014), so — exactly like relocations — each slot is
  written through its physical frame via the HHDM (the Windows
  fixup-before-protection order) and the mapping flags are never widened.
  Post-mapping failures unmap the stub page and free its frame
  (`Loaded::stub_page` is what `user.rs` unmaps during teardown).

## 7. Boot flow and serial contract (`kernel/src/user.rs`, `main.rs`)

`user::demo` keeps the ELF phase exactly as it is and adds a PE phase, in
order, one program at a time, reusing the single address-space slot (the slot
now holds an enum of ELF/PE load plans for teardown):

1. Module counting: `elf: {n} modules present` now counts only modules whose
   path ends in `.elf` (unchanged for the ELF fixtures); a new line
   `pe: {m} modules present` counts `.exe` modules.
2. Locate `/boot/pehello.exe` and `/boot/pebad.exe`. Any missing:
   `user: PE self-test skipped (`{path}` module missing)`; report `pe = false`.
3. `pebad.exe` probe: `pe_loader::inspect` must fail; print
   `user: /boot/pebad.exe rejected (bad signature)`. If it succeeds, return
   `Err("corrupt module pebad.exe was accepted")` → boot fatal.
4. `pehello.exe`: `inspect` + `map` into a fresh `AddressSpace::new_like_kernel`,
   map the zeroed one-page stack at `STACK_VA`, print the pinned load line
   `pe: pehello.exe loaded at {base:#x} (entry {entry:#x}, {n} sections,
   {r} relocation{s} applied)`, activate, spawn thread `user-pe`, run at CPL3.
   After `wait_all_finished`, return to the kernel space, unmap every section
   page plus the stack page, destroy the space. Assertions: exit code 0,
   exactly two additional writes, bytes > 0, no additional user faults, zero
   rejected syscalls; report `pe = true`.
5. Verdict line when `report.pe`:
   `pe selftest: passed (PE32+ module loaded, base relocation applied,
   int 0x80 write from CPL3, corrupt module rejected)`;
   otherwise `pe selftest: skipped (no PE modules present)`, so a skip can
   never appear as a pass.
6. `main.rs`: the marker inserts `,pe` after `,elf`:
   `checks=...,sched,user,elf,pe,vfs[,blk][,persist][,net]`. The persistence
   test's `vfs,blk,persist` suffix and the net test's `,net` check stay valid.

Existing lines that must remain byte-identical: `elf: 3 modules present`,
`user: hello from ring 3 via int 0x80`, `syscall: thread exits with code 0`,
`user fault isolated: vector=14`, `user: /boot/bad.elf rejected (bad magic)`,
`user selftest: passed (ELF64 modules loaded`, the `blk:`/`persist:` lines and
the stage log lines. The `user: ring-3 program exited (...)` summary line now
reports cumulative counters over both fixtures sets (its shape is unchanged;
counts grow).

## 8. ISO staging and build scripts

- `boot/limine.conf` gains `module_path: boot():/boot/pehello.exe` and
  `module_path: boot():/boot/pebad.exe`; the interactive variant inherits them.
  (Stage 2 adds `peconsole.exe`, `pebadimp.exe` and `pebadfn.exe`; see
  `docs/WIN32_CONSOLE.md` §3.)
- `scripts/make_iso.sh` copies both `.exe` artifacts into `iso_root/boot/` and
  invokes `scripts/build_user_pe.sh` first if either is missing (mirroring the
  ELF block); a module-less or PE-less ISO must fail loudly. (Stage 2: five
  fixtures.)
- `scripts/build_user_pe.sh` (new): checks the `x86_64-pc-windows-msvc` target
  and `rust-lld` presence with actionable messages; builds the `userland/pe`
  workspace with the flags in §3; copies `pehello.exe`; generates
  `pebad.exe` by zeroing the 4 signature bytes at `e_lfanew`; runs
  `scripts/check_user_pe.py` on `pehello.exe` (must pass) and `pebad.exe`
  (must fail with `bad signature`); logs sizes and SHA-256s. (Stage 2 adds
  `peconsole.exe` plus the `pebadimp`/`pebadfn` single-pattern mutants.)
- `scripts/check_user_pe.py` (new, dependency-free): verifies the artifact
  against the §5/§6 contract before QEMU: PE32+/AMD64/CUI/EXECUTABLE, section
  layout inside `0x400000..0x500000`, no W+X, entry inside an executable
  section, raw ranges inside the file, and — because the fixture is linked at
  `0x140000000` — `RELOCS_STRIPPED` clear with every DIR64 target inside a
  writable section; prints a one-line summary (entry, sections, relocations).
  (Stage 2 adds `--imports <dll:fn,fn,...>` / `--imports -` checks: descriptor
  termination, exact DLL name, ordinal-free ILT, name-set equality and
  patchable IAT slots.)
- `scripts/lint.sh` and `scripts/format.sh` extend to the `userland/pe`
  workspace (`cargo clippy --target x86_64-pc-windows-msvc -- -D warnings`,
  `cargo fmt --all -- --check`).
- `scripts/bootstrap.sh` adds a check for the `x86_64-pc-windows-msvc` target
  (fix hint: `rustup target add x86_64-pc-windows-msvc`) and for `rust-lld`
  in the toolchain.
- `scripts/test_pe_qemu.sh` (new): reuses `test_boot_qemu.sh`
  (`--serial-log artifacts/qemu-serial-pe.log`), then asserts with `grep -Fq`
  per line and `die` on any miss. Stage-2 assertion set (current):
  - `pe: 5 modules present`
  - `pe: pehello.exe loaded at 0x400000 (`
  - `pe: hello from a PE32+ image via int 0x80`
  - `pe: base relocation verified`
  - `user: /boot/pebad.exe rejected (bad signature)`
  - `pe: peconsole.exe loaded at 0x400000 (entry 0x401000, 5 sections, 1 relocation applied)`
  - `user: /boot/pebadimp.exe rejected (unknown import dll)`
  - `user: /boot/pebadfn.exe rejected (unknown import function)`
  - the seven `win32:` lines of `docs/WIN32_CONSOLE.md` §7
  - `pe selftest: passed (PE32+ module loaded`
  - the marker contains `sched,user,elf,pe,win32,vfs`
- `scripts/test_elf_qemu.sh`: the marker assertion is
  `sched,user,elf,pe,win32,vfs` (stage 1 changed it to `...,pe,vfs`; stage 2
  inserted `win32`).
- `scripts/test_all.sh`: adds `run userland-pe "$ROOT_DIR/scripts/build_user_pe.sh"`
  after the `userland` step and `run pe "$ROOT_DIR/scripts/test_pe_qemu.sh"`
  after the `elf` step. Expected summary after stage 2: PASS=22 FAIL=0
  SKIP=4; SKIP set unchanged.

## 9. Evidence and limits (claims this stage may and may not make)

May claim (after tests pass, with logs recorded): a genuine PE32+ AMD64 console
image produced by `rustc` + `rust-lld` with no MSVC/MinGW/Windows SDK is parsed
by a host-tested parser, mapped with W^X section permissions into a fresh user
address space, base-relocated (nonzero delta, DIR64) and executed at CPL3 under
the existing syscall gate; a corrupt PE module is rejected without harming
boot. Evidence: `os_core` unit tests, `check_user_pe.py`, `test_pe_qemu.sh`
serial log, full `test_all.sh` run, VirtualBox re-run.

May **not** claim (stage 1 scope): any Windows API compatibility, general
import resolution/DLL loading of real system DLLs (stages 2–3 and Phase 8
stage 2 resolve only the twelve-name `kernel32.dll` allowlist, see
`docs/WIN32_CONSOLE.md` §9, `docs/WIN32_PROGRAM.md` §9 and
`docs/WIN32_INPUT.md`), PE32 (32-bit), SEH/unwind, TLS, CLR, resources,
dynamic code, ASLR, filesystem-backed exec, or running third-party Windows
binaries. Limits to restate: fixed 1 MiB code window at `0x400000`, one 4 KiB
stack page, max 16 sections / 256 relocations per image at load, W^X enforced
per section, relocation targets must not cross a page, one user thread at a
time, modules from the bootloader only, QEMU/VirtualBox only.

## 10. Documentation and evidence updates (on implementation)

- `docs/RESEARCH.md`: R-013 (sources, toolchain probe, claims, confidence).
- `docs/DECISIONS.md`: ADR-0019 (real-linker PE fixtures via rust-lld, fixed
  load base with always-on relocation, reject-by-directory policy, two-phase
  loader, unsupported-directory policy) including alternatives rejected.
- `docs/ARCHITECTURE.md`: `os_core::pe` row, `pe_loader.rs` row, userland/pe
  row, updated limitations and Windows-compatibility wording (loader ≠ API
  compatibility).
- `docs/TESTING.md`: `pe` row with exact assertions, unit-test count update,
  marker documentation update (`...,user,elf,pe,vfs`).
- `docs/ROADMAP.md`: Phase 7 stage 1 complete; remaining Phase 7 items
  (Win32 console surface) explicitly deferred — as written for stage 1; stage 2
  later added the three-name console surface (`docs/WIN32_CONSOLE.md`,
  ADR-0020) and updated the roadmap again; stage 3 grew the surface to eleven
  names with the five-argument stub form and the command-line tail
  (`docs/WIN32_PROGRAM.md`, ADR-0021).
- `progress.md`: milestone entry, exact commands, artifact table with sizes
  and SHA-256s (`pehello.exe`, `pebad.exe`, `kernel.elf`, ISOs), serial
  excerpts, defects, next action.
- `LICENSES.md`: no new third-party code; note that fixtures are built by the
  already-documented Rust toolchain (`rust-lld` ships with it) and that the
  Windows-target core/compiler-builtins crates are the same Rust runtime
  already documented.
