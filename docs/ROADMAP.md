# Roadmap

Statuses are evidence-based. A phase is complete only when its acceptance tests
were run and observed. No dates or completion percentages are implied.

| Phase | Scope | Status |
|---|---|---|
| 0 | repository audit, toolchain, engineering controls, initial docs | **complete** (2026-10-17) |
| 1 | own kernel, Limine boot, serial/framebuffer, memory map, ISO, boot tests | **complete for available tooling** — QEMU BIOS (256 MiB/512 MiB/1 GiB) and VirtualBox BIOS verified; UEFI boot verification outstanding |
| 2 | GDT/IDT/exceptions, timer, diagnostics | **partial** — GDT/TSS/IST, 32 exception gates, int3 self-test, PIT IRQ0 through IOAPIC GSI2 (PIC fallback), xAPIC LINT0 ExtINT, IRQ-safe locking for interrupt-shared state; APIC timer, PCI IOAPIC routing, SMP, recoverable exceptions not started |
| 3 | physical/virtual memory, page tables, permissions | **partial (kernel-side VM manager done)** — bitmap frame allocator; kernel-owned 4-level address space with CR3 switch, map/unmap/translate, on-demand tables, W^X enforcement and guard refusals, all self-tested in QEMU/VirtualBox BIOS boots; user address spaces, guard-page and fragmentation stress tests outstanding |
| 4 | kernel heap, slab/size classes, performance harness | **partial** — fixed 8 MiB first-fit free-list heap with global allocator, IRQ-safe locking, poisoning, double-free detection, `validate()` walk, host tests (14) and in-kernel self-test; growth, slabs per measurement and the heap benchmark harness outstanding |
| 5 | tasks, user mode, syscalls, handles/IPC, isolation | **partial (threads + first ring-3 slice done)** — thread table, kernel stacks, register/RSP context switch, cooperative yield/exit, timer preemption, deferred stack reclaim; ring 3 with a DPL3 `int 0x80` gate, validated `write`/`exit` services, user-fault isolation and address-space teardown verified in QEMU/VirtualBox BIOS; handles/IPC, process model, ELF loading not started |
| 6 | VFS, storage, console/shell, persistence, networking | **complete** (stages 1–5 implemented and verified, 2026-10-18) |
| 7 | PE loader and minimal native Windows console compatibility | **complete (stages 1–3, acceptance met 2026-10-19)** — PE32+ parser/loader with DIR64 relocations, and a real import directory resolving eleven `kernel32.dll`-shaped services against our own kernel implementations (console surface plus program basics: `WriteFile`, per-thread `GetLastError`/`SetLastError`, `ExitThread`, synthetic IDs, `GetTickCount`, `GetCommandLineA`); broader API surface, real DLL loading, SEH, TLS and PE32 explicitly outside the phase and not started |
| 8 | larger Win32/desktop, registry-like config, GUI/text/input | **partial (stages 1–5, 2026-10-20)** — a persisted registry-like key/value store (`os_core::config` POCF codec + `kernel/src/config.rs` + shell `config` builtin) rides the existing checkpoint slots with no format change, a blocking line-mode console `ReadFile` (service 14) lets a PE fixture consume typed serial input with echo and a CRLF-inclusive count (`docs/WIN32_INPUT.md`, ADR-0023), a read-only registry surface (`advapi32.dll` `RegOpenKeyExA`/`RegQueryValueExA`/`RegCloseKey`, services 15–17, four-slot handle table over the config store, first six-argument stub) lets a PE fixture read REG_SZ values (`docs/WIN32_REGISTRY.md`, ADR-0024), and a registry write surface (`RegCreateKeyExA`/`RegSetValueExA`/`RegDeleteValueA`, services 18–20, first nine-argument stub, writes persisted through `config::save()`) lets a PE fixture create keys, set/update/delete REG_SZ values and survive a save/read-back (`docs/WIN32_REGISTRY_WRITE.md`, ADR-0025), and a registry enumeration/deletion surface (`RegDeleteKeyA`/`RegEnumKeyExA`/`RegEnumValueA`, services 21–23, first eight- and two-argument stubs, deterministic canonical-order enumeration, non-recursive key deletion with the enumerate-first rule) lets a PE fixture enumerate keys/values and delete keys (`docs/WIN32_REGISTRY_ENUM.md`, ADR-0026); larger Win32 surface, GUI and the compatibility matrix not started |
| 9 | graphics/driver framework, PCI, MMIO/DMA/IRQ ownership, VM display | partial (stages 1–8: PCI inventory, driver model, e1000 start/stop/restart + exact MMIO/DMA/IRQ claims, bounded IRQ-to-service RX queue, PIT/COM1 IOAPIC routing, 2026-10-22) — MSI, PCI INTx IOAPIC routing, MMIO unmap and VM display not started |
| 10 | one scoped open-source GPU acceleration target or documented block | not started (research only) |
| 11 | Windows WDDM/driver compatibility prototype or documented block | not started (research only) |

## Phase 0 acceptance (met)

An explainable plan, pinned toolchain/bootloader, reproducible prerequisite check
and canonical control files exist. No destructive host changes were made.

## Phase 1 acceptance (met with skips)

- Own x86-64 kernel source and ELF64 image built from source.
- Limine loads it; base revision 6 verified at runtime.
- Serial and framebuffer status output; memory map copied and summarised.
- BIOS+UEFI hybrid ISO created; both El Torito entries verified.
- QEMU BIOS boot reaches `PROJECT_OS_BOOT_SUCCESS:<id>` and exits 33 via
  `isa-debug-exit`; panic/fatal detection and negative test work.
- VirtualBox 7.2.20 BIOS boot reaches the same marker with 5 PIT ticks on the
  default VM profile; the isolated script creates and removes only its own VM.
- UEFI ISO entry verified but **UEFI boot SKIP** (no OVMF).
- Host tests (33), formatting, lint, ISO creation and memory tests pass.

Remaining Phase 1 evidence: run a UEFI QEMU boot with pinned OVMF and record it
in `progress.md`. VirtualBox EFI is a secondary follow-up.

## Phase 2 acceptance (partial)

Met: own GDT/TSS/IST, IDT with exception gates, fatal exception reporting,
breakpoint self-test, PIT periodic interrupt at 100 Hz delivered through IOAPIC GSI2 in QEMU and VirtualBox, APIC MMIO mapping, local-APIC EOI and LINT0 ExtINT for the PIC fallback.

Missing: APIC timer, PCI INTx IOAPIC routing, ACPI MADT/_PRT discovery, MSI, SMP bring-up, recoverable page-fault handling, TSC calibration independent of the bootloader.

## Phase 3 acceptance (partial)

Met: physical frame allocator restricted to usable RAM, low-memory reservation,
contiguous aligned allocation, strict free validation, host tests and in-kernel
HHDM read-back self-test, accounting restored after churn; a narrow uncached
MMIO leaf mapper with host-tested 4-level index/entry math; and the kernel's own
4-level address space (`kernel/src/vm.rs`): PML4 built from the kernel-half copy,
CR3 switch verified in QEMU BIOS (512 MiB) and VirtualBox BIOS, page
map/unmap/translate with on-demand intermediate tables, user-half teardown,
`invlpg` discipline, W^X policy host-tested, and self-test refusals for W^X
violations, double maps, misalignment, user pages in the kernel half and
non-canonical addresses.

Met since Phase 5b: user address spaces built from the kernel-half template are
exercised from ring 3 — map, run at CPL3, unmap, free frames and destroy page
tables all execute in every boot (ADR-0014).

Missing: guard pages around stacks, mapping-lifetime tests for shared pages,
exhaustion and fragmentation stress under multiple VM memory sizes, 5-level
paging, and reclaiming the bootloader's now-unused tables/frames.

## Phase 4 acceptance (partial)

Met: debug-mode-capable first-fit free-list heap with poisoning, double-free
detection, block/link magic, an O(n) `validate()` accounting walk, 14 host tests
including a 500-step churn that validates after every operation, and an in-kernel
self-test (Box, 10,000-element Vec growth, 1 MiB block, accounting restored)
that runs in every QEMU/VirtualBox boot.

Missing: heap growth beyond the fixed 8 MiB region, slab/per-size-class
allocators (deferred until a measured fragmentation/contention problem), guard
pages at heap edges, and the reproducible heap benchmark with recorded
environment/flags/iterations.

## Phase 5 acceptance (partial)

Met (kernel side): context switching and kernel threads are functional
(`kernel/src/sched.rs`); round-robin scheduling with cooperative yield, thread
exit and timer-driven preemption is verified in QEMU BIOS (three RAM sizes) and
VirtualBox BIOS. Exited-thread stacks are reclaimed at the next scheduling
point. No priority/sleep/SMP/final-thread-slot reuse yet.

Met (user side, first slice): a native ring-3 program executes in its own
address space built from the kernel-half template and reaches the kernel
through a DPL3 `int 0x80` gate (`kernel/src/user.rs`, `kernel/src/syscall.rs`).
`SYS_WRITE` output is validated per page against the live CR3 (user-half range,
USER bit, 4 KiB cap) and `SYS_EXIT` ends the thread; a user-mode page fault is
reported, counted and isolated (thread killed, kernel continues). Both paths
verified in QEMU BIOS (three RAM sizes) and VirtualBox BIOS with
`checks=...,heap,vm,sched,user`.

Missing (user side): programs are embedded fixtures, not loaded from a
filesystem (no ELF loader yet); one user thread at a time (single TSS `rsp0`);
no process model, handle table, or invalid-pointer fuzz corpus beyond the
single unmapped-write case; no guard pages around the user stack; no
`fork`/`exec`/`wait`; syscalls are console-write only.

## Phase 6 acceptance (met for the QEMU/VirtualBox BIOS scope)

Stage 1 (complete and verified): interactive serial console (IRQ4 receive ring
with line editing), in-memory VFS with a host-tested path parser and boot
self-test, and a kernel shell over the frame allocator, heap, scheduler,
timer, VFS and console. Scripted end-to-end shell session test
(`scripts/test_shell_qemu.sh`): 17 commands over a paced serial chardev, all
outputs asserted, `console` reports 0 overruns. Verified in QEMU BIOS (256
MiB / 512 MiB / 1 GiB) and VirtualBox BIOS; the marker is
`checks=...,user,vfs`.

Stage 2 (complete and verified): ATA PIO block device (PIIX primary master,
LBA28, polled; ADR-0016) and durable checkpoint persistence — a CRC32-checked
whole-tree image in two alternating 1 MiB slots, flushed write-through at
shell-command boundaries, loaded at boot with fallback to the older slot when
the newest fails its CRC. Four-boot harness (`scripts/test_persistence_qemu.sh`,
`-machine pc` + 8 MiB raw disk): format/save, reboot load plus further saves,
deliberate payload corruption → fallback, both slots corrupt → fresh format.
No-disk boots are unchanged (`checks` without `blk,persist`); with a disk the
marker is `checks=...,vfs,blk,persist`.

Stage 3 (complete and verified): native ELF64 program loading (ADR-0017). Three
Limine modules (`hello.elf`, `fault.elf`, `bad.elf`) are built from the
independent `userland/` Rust workspace; the kernel parses them with the
host-tested `os_core::elf` parser (77 → 96 unit tests), loads them into a fresh
user address space and runs them at CPL3, and rejects the corrupt module before
allocating anything. Evidence: `scripts/test_elf_qemu.sh` asserts
`elf: 3 modules present`, the exact ring-3 greeting, `syscall: thread exits with
code 0`, `user fault isolated: vector=14`, `user: /boot/bad.elf rejected
(bad magic)`, the ELF verdict line and the `sched,user,elf,vfs` marker; the
full suite is PASS=18 FAIL=0 SKIP=4 with VirtualBox BIOS and the four-boot
persistence test still green.

Stage 4 (complete and verified): framebuffer log console
(`docs/FRAMEBUFFER_CONSOLE.md`, research R-011). The serial log, shell echo and
shell output are mirrored to the Limine framebuffer through the host-tested
`os_core::textcon` grid model and `os_core::textrender` compositor (the original
5x7 font moved to `os_core::font`; unit tests 96 → 128). `kernel/src/fbcon.rs`
sizes the grid (1280×800 → 106×50 cells at scale 2; 1024×768 → 85×48), repaints
dirty rows with a cursor underline under an IRQ-safe lock, and runs a full
readback self-test after the heap exists (`sfence`, re-render, byte-compare).
The boot dashboard (`display.rs`) is deleted: the log stream is the screen.
Evidence: `scripts/test_console_qemu.sh` asserts the pinned `fbcon:` lines,
types `version`, and validates a QEMU monitor screendump
(`artifacts/console-screendump.ppm`) against the serial geometry; VirtualBox
BIOS logs the same self-test at 85×48. Full suite PASS=19 FAIL=0 SKIP=4.

Stage 5 (complete and verified): e1000 networking (`docs/NETWORK.md`, ADR-0018,
research R-012). A minimal PCI mechanism-#1 scan (bus 0, `8086:100E`, BAR0
sizing with write-back restore) feeds the 82540EM driver: MMIO through the
uncached 2 MiB mapper, reset, MAC from RAL/RAH with a bounded EEPROM fallback,
legacy descriptor rings (RX 32 × 2048 B, synchronous TX), polled receive and a
non-fatal MAC loopback check. Static `10.0.2.15/24` on the slirp/NAT defaults;
protocol codecs (Ethernet/ARP/IPv4/ICMP/UDP with RFC 1071 checksums) live in
host-tested `os_core::net` and `os_core::pci` (unit tests 128 → 179); the shell
gains `net`, `ping` and `udpecho`. Evidence: `scripts/test_net_qemu.sh` boots
q35 with a pinned `model=e1000` slirp NIC and asserts the pinned `net:` lines,
`net selftest: passed (arp resolved, icmp echo reply in ...)`, the `...,vfs,net`
marker, `net` counters, `ping 10.0.2.2 -c 3` (3/3 replies, 0% loss) and a
host-forwarded 19-byte UDP echo round trip (reproduced twice); full suite
PASS=20 FAIL=0 SKIP=4. QEMU logs `net: loopback selftest: unsupported` (its
e1000 model implements only PHY `BMCR` loopback — the design's assumption was
corrected in R-012), and VirtualBox detected the NIC but did not answer the
gateway ARP in the headless run; both are non-fatal and reported as observed.
Limits: polled, static addressing, one NIC on bus 0, no TCP/DHCP/DNS/IRQ
(`docs/NETWORK.md` §10).

All five Phase 6 scope items are implemented and verified for the
QEMU/VirtualBox BIOS scope; the limits recorded in each stage's spec still
apply (no filesystem beyond the checkpoint image, serial-only console, polled
e1000 with static addressing).

## Phase 7 stage 1 (2026-10-18): PE32+ loader

Implemented and verified: a host-tested PE32+ parser (`os_core::pe`), a kernel
loader (`kernel/src/pe_loader.rs`) that maps W^X sections into a fresh user
address space and applies DIR64 base relocations under a fixed 1 MiB window at
`0x400000`, and a genuine rust-lld PE fixture executed at CPL3 via `int 0x80`
(see R-013, ADR-0019, `docs/PE_LOADER.md`). A corrupt PE module is rejected
without harming boot; `test_all.sh` reports PASS=22 FAIL=0 SKIP=4.

## Phase 7 stage 2 (2026-10-18): native Win32 console surface

Implemented and verified: a genuine `raw-dylib` import directory in a rust-lld
fixture (`userland/pe/peconsole`) parsed by `os_core::pe`, resolved against an
exact-name allowlist of three `kernel32.dll` functions (`GetStdHandle`,
`WriteConsoleA`, `ExitProcess`) through a kernel stub page and `int 0x80`
dispatch, with the IAT patched via the HHDM before protection; two negative
mutants (`pebadimp`, `pebadfn`) are rejected before any allocation (see R-014,
ADR-0020, `docs/WIN32_CONSOLE.md`). Remaining Phase 7 items (broader API
surface, imports from real system DLLs, ordinals, forwarders, delay imports,
SEH/unwind, TLS, CLR, PE32) are explicitly deferred; no compatibility claim is
made.

## Phase 7 stage 3 (2026-10-19): Win32 program basics

Implemented and verified: the exact-name `kernel32.dll` allowlist grows from
three to eleven functions (`WriteFile`, `GetLastError`, `SetLastError`,
`ExitThread`, `GetCurrentProcessId`, `GetCurrentThreadId`, `GetTickCount`,
`GetCommandLineA` join the console surface), the stub page gains a 13-byte
five-argument form (`mov r10, [rsp+0x28]`) and a loader-owned command-line
tail, and per-thread `last_error` lives in the scheduler thread slot. A new
`pebasics.exe` fixture with nine `raw-dylib` imports exercises the documented
`WriteFile` failure values (0 + 6 / 0 + 87), error-state persistence, the
synthetic IDs, `GetTickCount` and the command line at CPL3; `pebadfn2.exe`
(`Sleep` substituted for `GetTickCount`) is rejected before allocation (see
R-015, ADR-0021, `docs/WIN32_PROGRAM.md`). Two bring-up defects were fixed:
the explicit command-line terminator and the `sched::wait_all_finished` TSC
budget removal. `test_all.sh` reports PASS=22 FAIL=0 SKIP=4. Broader API
surface, real DLL loading, ordinals, forwarders, delay imports, SEH/unwind,
TLS, CLR and PE32 remain explicitly deferred; no compatibility claim is
made. Phase 7 acceptance was reviewed and marked complete (2026-10-19).

## Phase 7 acceptance (met, 2026-10-19)

PE parser/loader handles malformed inputs safely; a named set of controlled
x86-64 Windows console fixtures runs repeatedly with recorded outcomes using our
own API services. Met by stages 1–3: three positive fixtures (`pehello.exe`,
`peconsole.exe`, `pebasics.exe`) execute at CPL3 through the native loader and
syscall gate, four rejection mutants (`pebad`, `pebadimp`, `pebadfn`,
`pebadfn2`) are refused before allocation, and the outcomes are pinned in
`scripts/test_pe_qemu.sh` (`artifacts/qemu-serial-pe.log`; 22 PASS / 0 FAIL /
4 SKIP in the final `test_all.sh` run). Evidence: R-013/R-014/R-015,
ADR-0019/0020/0021, `progress.md`. No general compatibility claim.

## Phase 8 stage 1 (2026-10-19): native configuration store

Implemented and verified: a registry-like key/value store serialised by the
host-tested `os_core::config` codec (`POCF` version 1: 8-byte header, ≤64
entries, path-shaped ASCII keys ≤96 bytes, printable values ≤128 bytes, blob
≤7984 bytes) into the ordinary VFS file `/etc/config`, so the checkpoint
format, slots, CRCs and fallback rule of Phase 6 are untouched
(`docs/CONFIG.md`, ADR-0022). Boot decodes the file when the checkpoint
carries it and otherwise seeds `System/Name`, `Display/Theme`, `Net/Mode` in
memory; the first `config set`/`config del` writes the file and the shell
boundary flush persists it exactly like file contents. The shell gains
`config [get|set|del|list]`, the boot self-test adds the pinned
`config selftest: passed (...)` line, and the marker gains `,config` after
`,vfs`. Evidence: `scripts/test_shell_qemu.sh` PASS (25 commands incl. the
store surface and the `error: bad key` path), `scripts/test_persistence_qemu.sh`
PASS (four boots: theme survives reboot, `dark` saved only to the corrupted
slot rolls back to `light` on fallback while `/tmp/notes/a.txt` survives,
fresh format reseeds defaults), `scripts/test_all.sh` PASS=22 FAIL=0 SKIP=4
(269 host unit tests). Limits: no Windows registry API, no typed values, no
ACLs/multi-user separation, no transactions or notifications
(`docs/CONFIG.md` §1/§8). No compatibility claim is made.

## Phase 8 stage 2 (2026-10-19): Win32 console input (`ReadFile` line mode)

Implemented and verified: `GetStdHandle(STD_INPUT_HANDLE)` returns the distinct
handle `2`; service 14 (`ReadFile`, five-argument stub, allowlist grows to
twelve names) blocks in the `int 0x80` handler with interrupts enabled and
`hlt` (no lock held), consumes serial-console bytes one at a time, echoes
printables with backspace/DEL editing, terminates on CR/LF with a CRLF-
inclusive line and count, and returns the documented `FALSE` + WinError values
(`0`+6 bad handle, `0`+87 overlapped or length outside `2..=4096`) with
`*lpNumberOfBytesRead` zeroed first (`docs/WIN32_INPUT.md`, ADR-0023, R-016).
The `peinput.exe` fixture ships only on `artifacts/project-os-input.iso`
(`make_iso.sh --input-fixture`); default ISOs, module counts and markers are
unchanged. Evidence: `scripts/test_input_qemu.sh` PASS (typed line echoed and
played back, `ReadFile(2, 64 bytes) -> 1 (14 bytes)`, error lines, selftest
line, QEMU exit 33), `scripts/test_pe_qemu.sh` / shell / persistence / boot
regressions unchanged, `scripts/test_all.sh` PASS=23 FAIL=0 SKIP=4 (269 host
unit tests). Limits: line mode with echo only; no `ReadConsole`,
`GetConsoleMode`/`SetConsoleMode`, Ctrl+C/Ctrl+Z semantics, raw mode,
Unicode/W variants, redirected stdin, overlapped/async I/O or multiple
readers; CRLF inclusion is a project contract, not vendor-verified
(`docs/WIN32_INPUT.md` §1/§7). No compatibility claim is made.

## Phase 8 stage 3 (2026-10-19): Win32 registry read surface (`advapi32.dll`)

Implemented and verified: the import allowlist becomes two DLLs
(`kernel32.dll` + `advapi32.dll`, fifteen names) and services 15–17
(`RegOpenKeyExA`, `RegQueryValueExA` with the first six-argument stub
forwarding `lpcbData` through `r11`, `RegCloseKey`) serve a case-insensitive,
read-only registry-shaped view over the config store: `HKLM` is the only root,
a key is a store prefix with children, a value is one store entry as
`bytes + NUL` REG_SZ, and a four-slot handle table (`0x1000 + slot`) returns
the documented error values (2/4/6/87/234) without killing the thread
(`docs/WIN32_REGISTRY.md`, ADR-0024, R-017). The `pereg.exe` fixture ships
only on `artifacts/project-os-registry.iso` (`make_iso.sh --registry-fixture`);
default ISOs, module counts and markers are unchanged. Evidence:
`scripts/test_registry_qemu.sh` PASS (session log 10,420 bytes, sha256
`02180701…`; handles 4096–4099, size query 11, `ERROR_MORE_DATA`,
`"PROJECT_OS"`/`"classic"` reads, double close, slot reuse, 33-byte summary,
QEMU exit 33), `pe`/`input`/shell/persistence/boot regressions unchanged,
`scripts/test_all.sh` PASS=24 FAIL=0 SKIP=4 (269 host unit tests, build id
`0.1.0-b1b8542c`). Limits: read-only; no hives, ACLs, transactions,
redirection, enumeration, write APIs, default/unnamed values, per-user or
class namespaces, typed values beyond REG_SZ, `W`/Unicode variants or remote
registry; four concurrent handles and always-NUL-terminated values are
project contracts, not vendor-verified. No compatibility claim is made.

## Phase 8 stage 4 (2026-10-19): Win32 registry write surface (`advapi32.dll`)

Implemented and verified: the two-DLL allowlist grows to eighteen names and
services 18–20 (`RegCreateKeyExA` with the first nine-argument stub forwarding
`phkResult`/`lpdwDisposition` through `r13`/`r14` and a 48-byte fixed stride,
`RegSetValueExA` via the existing `Args6` shape, `RegDeleteValueA`) mutate the
same config store the shell `config` builtin uses and persist through the
existing `config::save()`. Created keys report `REG_CREATED_NEW_KEY` (1) when
the prefix had no children, `REG_OPENED_EXISTING_KEY` (2) otherwise, and a key
whose last value is deleted ceases to exist; writes validate joined paths (87)
while reads keep their stage-3 miss behavior; `REG_SZ` only, NUL-terminated,
capped by the store (`docs/WIN32_REGISTRY_WRITE.md`, ADR-0025, R-018). The
`peregw.exe` fixture ships only on `artifacts/project-os-registry-write.iso`
(`make_iso.sh --registry-write-fixture`); default ISOs, module counts and
markers are unchanged. Evidence: `scripts/test_registry_write_qemu.sh` PASS
(session log 10,407 bytes, sha256 `4c03490b…`; create dispositions
`created`/`opened existing`/`created`, REG_SZ sets and updates, error values
2/6/87, delete-then-query → 2, slot reuse, `Persisted` write, 39-byte summary,
QEMU exit 33), store persistence verified by save/read-back/`Config::decode`
in the boot phase, all earlier regressions unchanged, `scripts/test_all.sh`
PASS=25 FAIL=0 SKIP=4 (269 host unit tests, build id `0.1.0-fe3c278e`).
Limits: no hives, ACLs, access checks, transactions, enumeration, key deletion
(`RegDeleteKey`), typed values beyond REG_SZ, default/null values, empty key
objects, namespace rules or remote registry; four concurrent handles and the
64-entry store are project contracts, not vendor-verified. No compatibility
claim is made.

## Phase 8 stage 5 (2026-10-20): Win32 registry enumeration and key deletion (`advapi32.dll`)

Implemented and verified (2026-10-20): the two-DLL allowlist
grows to twenty-one names and services 21–23 (`RegDeleteKeyA` via the new
two-argument stub, `RegEnumKeyExA`/`RegEnumValueA` via the new eight-argument
stub inside the unchanged 48-byte stride) enumerate and delete keys over the
same config store. Enumeration is the store's canonical ascending order over the
flattened namespace (child keys from tails containing `/`, values as direct
entries), with the documented consequence that a `/`-containing value name is
also visible as a child key; `RegDeleteKeyA` follows the fetched
non-recursive rule (a key with child keys returns 5, so callers enumerate
and delete children first) and entries disappear immediately (open handles
become stale) as a stated deviation from Windows' deferred removal. Buffer
semantics are pinned in `docs/WIN32_REGISTRY_ENUM.md` §6–§8 (234 with the
required size, 259 at the end, REG_SZ-only value data, ignored class/time
outputs). The `peregen.exe` fixture will ship only on
`artifacts/project-os-registry-enum.iso`
(`make_iso.sh --registry-enum-fixture`); evidence is
`scripts/test_registry_enum_qemu.sh` PASS plus
`scripts/test_all.sh` PASS=26 FAIL=0 SKIP=4. See ADR-0026 and R-019; limits
are restated there and in `docs/WINDOWS_COMPATIBILITY.md`.

## Phase 9 stage 1 (2026-10-20): PCI bus-0 inventory

Implemented and verified: `kernel/src/pci.rs` now records every present
bus-0 function (devices 0-31, functions 1-7 only for multifunction device 0)
with vendor/device IDs, class, header type and all implemented BARs, sized by
the standard all-ones write-back with immediate restore; header types 0/1/2
have 6/2/1 BAR slots, 64-bit BARs consume two dwords, and a 64-function cap
fails the self-test instead of dropping data. Boot prints the summary and
per-function lines, the shell gains `pci`, and the boot marker gains `,pci`
only on a passing self-test. The e1000 locator used the inventory with a
direct-scan fallback in this stage; stage 3 removed the fallback when the
driver model began passing the matched device to `net::init`. Evidence: `scripts/test_pci_qemu.sh` PASS (q35, pinned
e1000, six functions/seven BARs, host bridge/LPC/e1000+BAR0 lines, exit 33),
270/270 host tests, `scripts/test_all.sh` PASS=27 FAIL=0 SKIP=3. No bridge
traversal, resource assignment, IRQ routing, driver lifecycle or GPU claim.
See `docs/PCI.md`, ADR-0027, R-020.


## Phase 9 stage 2 (2026-10-20): driver registration and binding model

Implemented and verified: `os_core::driver` adds exact vendor/device and
class match rules, compile-time descriptors, deterministic first-match
binding in descriptor order, single-instance drivers, 8-descriptor/64-binding
caps, and an explicit `Bound -> Running -> Stopped` / `-> Failed(reason)`
state machine whose illegal transitions are errors (10 host tests; suite
280/280). The kernel registers the real e1000 descriptor, binds it against the
PCI inventory and prints `Bound` with the unmatched count plus a structural
self-test; `drivers` is a shell diagnostic and `,driver` joins the boot
checks. No hardware is started or stopped through the model yet: e1000
bring-up remains in `net::init`, and MMIO/DMA/IRQ ownership, stop/restart,
hotplug and WDM compatibility are explicitly not claimed. Evidence:
`scripts/test_driver_model_qemu.sh` PASS on q35 with a pinned e1000,
`scripts/test_all.sh` PASS=28 FAIL=0 SKIP=2. See `docs/DRIVER_MODEL.md`,
ADR-0028, R-021.


## Phase 9 stage 3 (2026-10-21): start and resource ownership

The e1000 now starts behind the driver model. `os_core::driver::Ledger` tracks
exclusive MMIO ranges, exclusive IRQ lines and per-owner DMA frame budgets
(16 claims, overlap/duplicate rejection, release); the kernel claims BAR0
(`0xfebc0000`, 128 KiB) and a 19-frame DMA budget **before** `net::init`,
moves the binding to `Running` on success, and releases claims with a
`Failed(reason)` binding on any failure. The model self-test cross-checks
that Running bindings own their MMIO and Failed bindings own nothing.
`net::init` accepts the matched `PciDevice` and returns a stable error reason.
Stop/restart is deliberately **not** implemented: unmapping, frame
reclamation and NET teardown do not exist, so only the host-tested state
machine covers `Stopped`/`release`. Evidence: 16 driver host tests (suite
286), `scripts/test_driver_model_qemu.sh` PASS (state Running, resources,
`,driver`, exit 33), `scripts/test_net_qemu.sh` PASS, `scripts/test_all.sh`
PASS=28 FAIL=0 SKIP=2. See `docs/DRIVER_MODEL.md`, ADR-0029, R-022.


## Phase 9 stage 4 (2026-10-21): e1000 stop and restart

`drivers stop` removes the network state, masks interrupts, disables RX/TX,
resets and waits, then frees the 16-page RX buffer run and three
descriptor/buffer frames (19 total) and releases both ledger claims; the
binding becomes `Stopped` and the HHDM MMIO window is retained (no unmap API).
A reset timeout leaks the rings deliberately and records `Failed`.
`drivers start` is allowed only from `Stopped`, re-claims BAR0 plus the
19-frame budget, re-runs `net::init` with the saved HHDM/TSC values and
returns the binding to `Running`. Evidence: host suite 287 (17 driver tests
including stop-releases/restart-reclaims); `scripts/test_driver_restart_qemu.sh`
PASS with exactly 19 frames freed and reallocated, zero claims while stopped,
network inactive/active around the cycle and ICMP after restart;
`scripts/test_all.sh` PASS=29 FAIL=0 SKIP=2. Not claimed: MMIO unmapping,
reset-timeout recovery, concurrent network use during restart, IRQs, hotplug,
power management or a virtual GPU. See `docs/DRIVER_MODEL.md`, ADR-0030,
R-023.

## Phase 9 stage 5 (2026-10-21): exact DMA page ownership and polled IRQ policy

The ledger's count-only DMA budget is replaced by `DmaPages { base, pages }`:
bases must be page-aligned, ranges must not overflow, and MMIO and DMA claims
are mutually overlap-checked. The e1000 allocates its rings, reports the four
real ranges (RX buffer run, TX descriptors, RX descriptors, TX buffer; 19
pages) to a claim callback, and only then enables RX/TX; a failed claim frees
the rings. The driver holds five claims (BAR0 plus four ranges), the Running
self-test requires all 19 pages, and stop/restart releases/reclaims them
exactly. Interrupts remain explicitly polled with `IMS` masked and no `Irq`
claim; `Resource::Irq` is modeled and host-tested but unused in the kernel.
Evidence: host suite 287 (17 driver tests including DMA/MMIO overlap);
`scripts/test_driver_model_qemu.sh` and `scripts/test_driver_restart_qemu.sh`
PASS; `scripts/test_net_qemu.sh` PASS; `scripts/test_all.sh` PASS=29 FAIL=0
SKIP=2. Not claimed: IOMMU isolation, MMIO unmapping, IRQ/MSI delivery,
allocation-failure injection, hotplug, power management or a virtual GPU. See
`docs/DRIVER_MODEL.md`, ADR-0031, R-024.

## Phase 9 stage 6 (2026-10-22): legacy INTx delivery and IRQ ownership

The PCI inventory decodes type-0 `INTERRUPT_LINE`/`INTERRUPT_PIN` at config
offset `0x3C`. A new `kernel/src/irq.rs` provides exclusive per-line handler
registration, 8259A mask/unmask (including the IRQ2 cascade), non-specific EOI
and dispatch; IDT gates for vectors 32-47 route device lines while IRQ0/IRQ4
keep their dedicated handlers. The driver claims `Irq { line }`, registers the
e1000 handler before `net::init`, enables `RXT0|RXDMT0` (with `RDTR`) only
after all DMA claims, and unmasks the line only after the NIC is running.
`E1000::handle_irq` reads `ICR` and increments a counter; it does not touch
rings or take locks. Stop masks the line, disables/shuts down the NIC,
unregisters the handler and releases six claims; restart repeats the claim and
unmasks again. `net` status reports the event count.

Evidence: q35 assigns line 11, two events before the harness ping and four
after the restart ping; `scripts/test_driver_model_qemu.sh`,
`scripts/test_driver_restart_qemu.sh` and `scripts/test_net_qemu.sh` PASS;
VirtualBox assigns line 9 and boots. `scripts/test_all.sh` PASS=29 FAIL=0
SKIP=2. Not claimed: interrupt-driven RX (protocol work remains polled),
MSI/IOAPIC, shared lines, coalescing tuning, MMIO unmapping, IOMMU isolation or
a virtual GPU. See `docs/DRIVER_MODEL.md`, `docs/NETWORK.md`, ADR-0032, R-025.

## Phase 9 stage 7 (2026-10-22): bounded interrupt-driven RX

The e1000 handler now owns the hardware RX ring. After the loopback selftest,
`enable_interrupts` publishes the MMIO/descriptor/buffer context and enables
`RXT0|RXDMT0`. Each interrupt reads `ICR` and drains at most one full
32-descriptor ring into a fixed 16-frame SPSC copy queue
(`os_core::queue::SpscQueue`); descriptors are re-armed immediately, including
when the queue is full. A full queue drops the newest frame and increments a
counter. `net::drain_rx` pops at most 8 frames per pass and dispatches them
under the existing network lock; the original polled ring path remains for
machines without a usable legacy line. Stop masks the line, clears the context
and resets the queue, so a restart cannot deliver a pre-stop frame. `net`
status reports depth, delivered and dropped counters.

Evidence: host suite 292 (5 queue tests including a 20,000-item two-thread
stress); QEMU q35 boot selftest delivered 2 frames; `ping 10.0.2.2 -c 3` raised
IRQ events 2 -> 5 and queue delivery 2 -> 5 with zero drops; stop/restart
freed/reclaimed 19 frames and delivered 2 frames after the restart ping; UDP
echo round-tripped 19 bytes. `scripts/test_driver_model_qemu.sh`,
`scripts/test_driver_restart_qemu.sh` and `scripts/test_net_qemu.sh` PASS;
`scripts/test_all.sh` PASS=29 FAIL=0 SKIP=2. Not claimed by stage 7: forced VM
queue-overflow, interrupt-storm or coalescing behaviour, MSI/IOAPIC routing, a
background protocol thread, SMP, MMIO unmapping or IOMMU isolation. See
`docs/NETWORK.md` section 13, `docs/DRIVER_MODEL.md`, ADR-0033, R-026.

## Phase 9 stage 8 (2026-10-22): IOAPIC bring-up and ISA routing

`kernel/src/arch/x86_64/ioapic.rs` maps the fixed `0xfec00000` window, probes
the ID/version/input count and masks every redirection entry before use.
`os_core::ioapic::RedirectionEntry` encodes the 64-bit entry and is host-tested
above the MMIO layer. `irq::init_ioapic` sends PIT GSI2 (edge, active-high) and
COM1 GSI4 (level, active-high) to the boot CPU's physical LAPIC ID. The timer
and COM1 IDT paths now call `irq::eoi`, which writes the local-APIC EOI for
IOAPIC routes and the 8259 EOI for PIC routes. If mapping/probing fails, or if
the timer does not start through the IOAPIC, the boot path masks IOAPIC 2/4 and
returns both lines to the 8259. The e1000 deliberately remains on PCI INTx line
11 through the PIC because QEMU q35's config line is not the ACPI GSI (the
e1000's slot-2 INTA maps to GSI22 in QEMU's `_PRT`), and ACPI MADT/`_PRT`
parsing is not implemented. MSI is deferred: the QEMU 82540EM e1000 exposes no
MSI capability.

Evidence: host suite 297 (5 IOAPIC encoding tests); QEMU q35 and VirtualBox
7.2.20 both log `ioapic: version 0x20, 24 pins, id 0; timer GSI2, console
GSI4`, report `pit: 100 Hz, IOAPIC GSI2`, and observe 7/6 ticks respectively
with no PIC fallback; shell input over COM1 works in the driver harness;
`test_boot_qemu.sh --require-ioapic`, the driver-model harness and VirtualBox
with `--require-ioapic` PASS. `scripts/test_all.sh` PASS=30 FAIL=0 SKIP=2.
Not claimed: ACPI MADT/`_PRT` discovery, PCI INTx IOAPIC routing, MSI/MSI-X,
APIC timer, SMP, level-triggered PCI lines or physical-hardware testing. See
ADR-0034, R-027.

## Phase 8 acceptance (planned)

Specific published UI/application scenarios pass a compatibility matrix. A
screenshot or demo alone is insufficient.

## Phase 9 acceptance (planned)

Host graphics/driver tests pass; VM display capability is precisely labelled;
driver lifecycle, resource ownership, MMIO/DMA/interrupt validation, timeout and
reset paths are tested with mock devices before physical GPUs.

## Phase 10 acceptance (planned)

One narrowly scoped accelerated target is demonstrated by repeatable tests, or a
source/license/dependency feasibility report documents why it is blocked. A
research document alone is not a driver.

## Phase 11 acceptance (planned)

A documented, explicitly limited WDDM/driver proof of concept with named GPU,
driver version, features and tests—or a `BLOCKED/RESEARCH` conclusion. No general
Windows driver compatibility claim.
