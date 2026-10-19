# Testing

No test below is marked passing unless it was executed and its output observed.
Skipped tests are listed separately from passes.

## Environment used for the recorded results

| Item | Value |
|---|---|
| Host | Windows 10/11 (MSYS/Git Bash), x86-64 |
| Rust | stable 1.99.1 (commit 58cf8f9c), target `x86_64-unknown-none` |
| QEMU | 11.1.0 (`qemu-system-x86_64.exe`) |
| mkisofs | cdrtools 3.02a09 |
| Limine | 12.9.3, SHA-256 pinned |
| QEMU machine | `q35`, 512 MiB RAM, 1 CPU, `-display none`, `-monitor none` |
| VirtualBox | 7.2.20r175154 (`VBoxManage` 7.2.20) |
| OVMF | not available |

## Entry point

```sh
scripts/test_all.sh
```

It runs every feasible check and prints a summary with `PASS`, `FAIL` and `SKIP`.
Exit codes used by individual scripts: `0` = pass, `2` = skipped (tool/feature
missing), any other = failure. `test_all.sh` treats `2` as SKIP, never PASS.

## Commands and what they prove

| Command | Proves | Recorded result |
|---|---|---|
| `scripts/test_all.sh` | orchestrates everything below and summarises | PASS: 24 PASS, 0 FAIL, 4 SKIP |
| `scripts/bootstrap.sh` | required tools and Rust targets (incl. `x86_64-pc-windows-msvc` and `rust-lld`) exist | PASS |
| `scripts/format.sh` | `rustfmt --check` clean (kernel workspace, userland, `userland/pe`) | PASS |
| `scripts/lint.sh` | Clippy `-D warnings` for os_core, membench, kernel, userland, `userland/pe` | PASS |
| `scripts/test_unit.sh` | 269 host tests: memmap, framebuffer, font/text-grid/renderer, allocator, PRNG, paging math, heap, path validation, checkpoint format (pofs), configuration store codec (POCF incl. all rejection vectors), ELF64 parser incl. all rejection vectors, PE32+ parser incl. all rejection vectors, relocation walk and import directory (descriptors, ILT/IAT fallback, names, ordinals, caps), PCI config/BAR math, Ethernet/ARP/IPv4/ICMP/UDP codecs and checksums | PASS (269/269) |
| `scripts/build.sh` | kernel builds; ELF entry and Limine requests verified | PASS |
| `scripts/build_user.sh` | userland workspace builds; `hello`/`fault` pass the header checker; `bad.elf` fails with `bad magic` | PASS |
| `scripts/build_user_pe.sh` | `userland/pe` builds via rust-lld; `pehello.exe`/`peconsole.exe`/`pebasics.exe`/`peinput.exe` (Phase 8 stage 2, `ReadFile` imports) and `pereg.exe` (Phase 8 stage 3, `advapi32.dll`+`kernel32.dll` imports) pass `check_user_pe.py` (imports included); `pebad.exe` fails with `bad signature`; the `pebadimp`/`pebadfn`/`pebadfn2` mutants patch a unique byte pattern each and stay structurally valid | PASS (9 artifacts) |
| `scripts/make_iso.sh` | ISO built (kernel + three ELF modules + seven PE modules); both El Torito entries verified; `--input-fixture` variant adds `peinput.exe` as an eighth PE module (`project-os-input.iso`); `--registry-fixture` adds `pereg.exe` instead (`project-os-registry.iso`) | PASS |
| `scripts/test_boot_qemu.sh` | BIOS boot at 256 MiB, 512 MiB and 1 GiB; marker; no panic; debug-exit 33 | PASS (all three) |
| `scripts/test_boot_qemu.sh --firmware uefi` | UEFI boot | SKIP (no `OVMF_FD`) |
| `scripts/test_elf_qemu.sh` | ELF64 module self-test: 3 modules present, ring-3 greeting from the ELF, exit 0, one isolated fault, `bad.elf` rejected, `,elf` marker | PASS |
| `scripts/test_pe_qemu.sh` | PE32+/Win32 self-test (stages 1–3): 7 modules present, pehello load line (4 sections, 1 relocation) + greeting + relocation verified, `pebad.exe` rejected (`bad signature`), peconsole load line (5 sections, 1 relocation), the seven `win32:` console lines, `pebadimp.exe` rejected (`unknown import dll`), `pebadfn.exe` rejected (`unknown import function`), `win32 selftest: passed`, pebasics load line (5 sections, 1 relocation), the full `win32:` program-basics block (incl. `WriteFile(0) -> 0 (invalid handle)`, `WriteFile(1, 28 bytes, overlapped) -> 0 (invalid parameter)`, `GetCommandLineA() -> "/boot/pebasics.exe"`), `pebadfn2.exe` rejected (`unknown import function`), `win32 program selftest: passed`, `pe`+`win32` markers | PASS |
| `scripts/test_memory.sh` | allocator host tests + in-VM allocator self-test | PASS (both) |
| `scripts/test_vfs.sh` | host path tests + in-VM VFS self-test | PASS (both) |
| `scripts/test_shell_qemu.sh` | scripted interactive shell session over a paced TCP serial chardev | PASS (25 commands) |
| `scripts/test_input_qemu.sh` | Win32 console-input self-test (Phase 8 stage 2, `project-os-input.iso`): `pe: peinput.exe loaded ... 5 sections, 1 relocation applied`, `GetStdHandle(-10) -> 2`, `ReadFile(1) -> 0` + `GetLastError() -> 6`, overlapped read `-> 0 (invalid parameter)` + `GetLastError() -> 87`, `ReadFile(2, 64 bytes) waiting for input`, harness line echoed and played back, `ReadFile(2, 64 bytes) -> 1 (14 bytes)`, `input surface verified`, `win32 input selftest: passed`, marker `win32,vfs`, QEMU exit 33 | PASS |
| `scripts/test_registry_qemu.sh` | Win32 registry read-surface self-test (Phase 8 stage 3, `project-os-registry.iso`): `pe: pereg.exe loaded ... 5 sections, 1 relocation applied`, `RegOpenKeyExA(0x1, "System") -> 6`, `-> 87` (nonzero options), `-> 2` (missing key), handles 4096–4099 with slot reuse after close, `-> 4 (too many open keys)`, size query `-> 0 (REG_SZ, 11 bytes)`, `-> 234 (more data, 11 bytes needed)`, `"PROJECT_OS"` and `"classic"` full reads, NULL-subkey → `0x80000002`, `RegCloseKey` 0/6, `registry surface verified` (33 bytes), `win32 registry selftest: passed`, marker `win32,vfs`, QEMU exit 33 | PASS |
| `scripts/test_console_qemu.sh` | framebuffer console: pinned `fbcon:` lines, selftest, shell output mirror and a validated QEMU screendump | PASS (106x50 at scale 2; screendump matches) |
| `scripts/test_persistence_qemu.sh` | four-boot durability on a raw 8 MiB disk (format/save, reboot load, CRC fallback incl. configuration rollback, fresh format with defaults reseeded) | PASS (4 boots) |
| `scripts/test_net_qemu.sh` | e1000 end-to-end on q35 with a pinned `model=e1000` slirp NIC: pinned `net:` lines, `,net` marker, `net` counters, `ping 10.0.2.2 -c 3` (3/3), host-forwarded 19-byte UDP echo round trip | PASS (reproduced twice) |
| `scripts/test_graphics_host.sh` | framebuffer geometry/colour tests | PASS |
| `scripts/test_virtualbox.sh` | VirtualBox BIOS boot with an Intel 82540EM NAT NIC attached; kernel detects the NIC (`net: e1000 at 00:03.0`), loopback `unsupported`, gateway ARP unanswered — non-fatal | PASS (7.2.20; marker + 5 ticks) |
| `scripts/test_pci.sh` | PCI enumeration | SKIP (not implemented, Phase 9) |
| `scripts/test_driver_model.sh` | driver lifecycle model | SKIP (not implemented, Phase 9) |
| `scripts/test_gpu_vm.sh` | virtual GPU driver | SKIP (framebuffer only) |
| `scripts/bench_graphics.sh` | accelerated graphics benchmark | SKIP (no implementation) |
| `scripts/bench_memory.sh` | host allocator performance | PASS (measurement, not a correctness gate) |

## QEMU boot test details

The runner invokes approximately:

```sh
qemu-system-x86_64 \
  -machine q35 -m 512M -smp 1 \
  -cdrom artifacts/project-os.iso -boot d \
  -display none -monitor none \
  -serial file:artifacts/qemu-serial-512m.log \
  -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
  -no-reboot
```

The kernel prints a unique marker only after all checks:

```text
PROJECT_OS_BOOT_SUCCESS:<build-id> checks=handoff,memmap,gdt,idt,exceptions,timer,allocator,heap,vm,sched,user,elf,pe,win32,vfs,config
```

The `,elf` component is inserted between `user` and `vfs` only when the ELF64
module self-test ran (ADR-0017), `,pe` after `,elf` only when the PE32+
self-test ran (ADR-0019), and `,win32` after `,pe` only when the native Win32
console self-test ran (ADR-0020). `,config` is appended directly after `,vfs`
when the configuration-store self-test ran (ADR-0022); it is unconditional on
the current boot path. Correction note: stage 2 described the
no-disk marker as byte-identical to stage 1; the segment between `user` and
`vfs` now carries `elf` (and, with PE modules present, `pe`, and with the
Win32 fixtures `win32`), so the byte-identical
wording applies to the `vfs`-onward suffix (`vfs,config,blk,persist`) that the
persistence test pins, not to the full line.

Failure conditions:

- marker absent within the timeout;
- `PROJECT_OS_PANIC` or `PROJECT_OS_FATAL` present;
- QEMU does not exit after the marker;
- QEMU exit status is not 33.

The script copies the serial log to `artifacts/qemu-serial.log` by default; use
`--serial-log <path>` to keep it elsewhere. `test_all.sh` records one log per
RAM size (`qemu-serial-256m.log`, `-512m.log`, `-1g.log`). The memory sizes are
selected with `--memory` (default 512M). A corrupt-ISO negative test was run:
the marker was absent and the script exited 1.

### Exit status 33

The kernel writes `0x10` to I/O port `0xf4` only when its cmdline contains
`debug_exit`. QEMU's `isa-debug-exit` device maps the value to
`(value << 1) | 1` = 33. Without the device (VirtualBox), the write is ignored
and the kernel idles, so the same ISO can be booted manually.

## What the kernel verifies before the marker

1. Limine base revision 6 was honoured.
2. Memory-map response exists, is sorted, usable regions do not overlap, no
   address overflow, at least some usable RAM, usable regions page-aligned.
3. Framebuffer geometry (if present) validates: non-zero dimensions, supported
   bpp, pitch >= visible line, no size overflow.
4. The memory map was copied into kernel-owned storage.
5. Frame allocator initialised from usable RAM only; low 1 MiB reserved.
6. Allocator self-test: single and 4-frame allocation, natural alignment,
   HHDM write/read-back, free, accounting restored, double-free rejected,
   non-power-of-two rejected.
7. GDT/TSS/IST1 loaded; IDT loaded; deliberate `int3` reached the Rust handler.
8. PIC remapped/masked, PIT programmed, IRQ0 unmasked, the APIC MMIO page mapped,
   and LINT0 left in ExtINT delivery so the 8259 can reach the CPU.
9. Five PIT ticks observed within a 2 s TSC-based bound (or bounded spin fallback).
10. Kernel heap self-test: box allocation, `Vec` growth, a 1 MiB block, accounting restored.
11. VM self-test: own 4-level tables active, map/unmap/translate, W^X and guard refusals, CR3 swap.
12. Scheduler self-test: two yielding workers interleave; a CPU-bound spinner finishes under timer preemption.
13. Ring-3 self-test: three Limine ELF64 modules located; `bad.elf` rejected
    before any allocation; `hello.elf` performs an `int 0x80` write and exits 0
    from CPL3; `fault.elf` produces one isolated user page fault that kills
    only the user thread.
14. PE self-test (when `.exe` modules are present): `pebad.exe` rejected before
    any allocation (`bad signature`); `pehello.exe` parsed, mapped with W^X
    section flags, base-relocated with a nonzero DIR64 delta, executed at CPL3
    performing exactly two `int 0x80` writes and exiting 0 — including the
    fixture's own relocated-pointer comparison; no new user faults.
15. Win32 console self-test (when the `peconsole`/`pebadimp`/`pebadfn`
    modules are present): both negative mutants rejected before any allocation
    (`unknown import dll` / `unknown import function`); `peconsole.exe`'s real
    import directory parsed, its IAT slots patched to the kernel's stub page,
    `GetStdHandle`/`WriteConsoleA`/`ExitProcess` reached at CPL3 through the
    resolved imports with exactly 1/2/1 calls, two console writes (76 bytes),
    exit 0, no new user faults (`docs/WIN32_CONSOLE.md` §7).
16. Win32 program-basics self-test (when the `pebasics`/`pebadfn2` modules are
    present): `pebadfn2.exe` rejected before any allocation (`unknown import
    function`); `pebasics.exe`'s nine imports resolved through the same stub
    page (one 13-byte five-argument stub for `WriteFile`), the fixture's error
    ladder observed (`GetLastError` keeps 42 across a successful `WriteFile`,
    then 6 for a bad handle and 87 for a non-null overlapped pointer),
    synthetic IDs `1`/`1`, wrap-safe `GetTickCount` pair, the loader-owned
    command line `/boot/pebasics.exe`, and `ExitThread(0)` ending the thread;
    exactly two additional writes, 59 bytes, no new user faults
    (`docs/WIN32_PROGRAM.md` §7).
17. Configuration-store self-test: `/etc/config` decoded when the checkpoint
    carries it (otherwise defaults seeded in memory); a temporary
    `SelfCheck/Value` entry set/get/list/remove round trip, invalid-key
    rejection, and an encode/decode round trip. The self-test never writes the
    file, so an idle boot leaves the checkpoint byte-identical.
18. VFS self-test: create/read/append/overwrite/remove/list and the error paths.
19. Framebuffer console self-test: the whole grid plus cursor is re-rendered in
    host-testable code, `sfence` is executed, and the framebuffer bytes are read
    back and compared scanline by scanline; any mismatch is a `PROJECT_OS_FATAL`.
    The console also logs its activation line (`fbcon: enabled ...` or the
    disabled line) right after framebuffer validation.

After the timer is verified, the kernel probes the PIIX primary master
(`blk`), then loads the best checkpoint slot or formats a fresh disk
(`persist`), then loads or seeds the configuration store (`config`). The
disk steps are conditional: without a disk the kernel logs
`blk: no ATA device on primary channel` / `persist: inactive (no ATA disk)`
and the marker is `checks=...,user,elf,pe,win32,vfs,config` (the `win32`
component requires the Win32 fixtures; without them it is
`...user,elf,pe,vfs,config`). With a usable disk the marker gains
`,blk,persist`.

## Interactive shell test details

`scripts/test_shell_qemu.sh` builds `artifacts/project-os-interactive.iso`
(`make_iso.sh --interactive` strips `debug_exit` from the Limine cmdline, so
the kernel reaches the shell instead of exiting) and drives it with
`scripts/test_shell_qemu.py`:

```text
qemu-system-x86_64 -machine q35 -m 512M -smp 1 \
  -cdrom artifacts/project-os-interactive.iso -boot d \
  -display none -monitor none \
  -serial tcp:127.0.0.1:<port>,server=on,wait=off -no-reboot
```

The Python driver connects to the TCP chardev, waits for the shell banner,
then types 25 commands at 2 ms per byte (a UART-realistic rate) and asserts
each response, the returning prompt, the boot marker, and `console`'s
`0 overruns`. The transcript is written to
`artifacts/qemu-shell-session.log`.

**Why not `-serial stdio`:** QEMU's 16550 model accepts an unthrottled host
burst straight into the 16-byte receive FIFO. A piped command can overflow
that FIFO before the guest vCPU wakes from `hlt`, so bytes are dropped
regardless of guest correctness (the kernel counts real overruns via LSR bit
1 and reports them through the `console` builtin). The TCP chardev lets the
harness pick the delivery rate, which is what a real serial line or a human
at a terminal produces. A large paste into a stdio serial can still overrun;
that limitation is emulation-side and documented, not hidden.

## Framebuffer console test details

`scripts/test_console_qemu.sh` (driver `scripts/test_console_qemu.py`,
checker `scripts/check_screendump.py`) boots the interactive ISO on `q35` with
two TCP chardevs and a different serial mode than the shell test:

```text
qemu-system-x86_64 -machine q35 -m 512M -smp 1 \
  -cdrom artifacts/project-os-interactive.iso -boot d \
  -display none -monitor tcp:127.0.0.1:<monitor>,server=on,wait=off \
  -serial tcp:127.0.0.1:<port>,server=on,wait=on -no-reboot
```

`wait=on` holds the guest until the harness has connected, so the transcript
includes the early `framebuffer:`/`fbcon:` lines (with `wait=off` QEMU drops
serial bytes emitted before the client connects). The driver asserts the
`fbcon: enabled {cols}x{rows} cells at scale {scale}` line, the
`fbcon selftest: passed (...)` line and the boot marker, waits for the shell,
types `version` (its output must appear on the same serial stream that feeds the
mirror), then asks the monitor for `screendump
artifacts/console-screendump.ppm`. The checker parses the PPM, requires its
dimensions to match the serial `framebuffer:` line, requires at least 500
foreground pixels inside the grid rectangle from the `fbcon:` line, and
requires every pixel outside that rectangle to be the pinned background
colour. The byte-exact screen assertion is the kernel's own readback
self-test; the screendump proves the scanout content is a real screen and is
kept as an artifact. VirtualBox BIOS boots run the same self-test at
1024×768 (85×48 cells at scale 2).

## Persistence test details

`scripts/test_persistence_qemu.sh` (driver `scripts/test_persistence_qemu.py`)
reuses the paced-session machinery but boots `-machine pc` with a raw 8 MiB
disk, because the ATA PIO driver needs the PIIX legacy ports that `q35` does
not expose (R-009 in `docs/RESEARCH.md`):

```text
qemu-system-x86_64 -machine pc -m 512M -smp 1 \
  -cdrom artifacts/project-os-interactive.iso -boot d \
  -drive file=artifacts/persist-disk.img,format=raw,if=ide,index=0,media=disk \
  -display none -monitor none \
  -serial tcp:127.0.0.1:<port>,server=on,wait=off -no-reboot
```

Four boots against one image, each transcript kept
(`artifacts/qemu-persist-boot{1..4}.log`):

1. fresh disk → `formatted slot A seq=0`; `write /tmp/hello.txt hello world` →
   `saved slot B seq=1`; `config get Display/Theme` → `classic` (defaults,
   nothing written); `config set Display/Theme light` → `saved slot A seq=2`;
2. reboot → `loaded slot A seq=2`, theme reads `light`, file readable,
   `mkdir /tmp/notes` → B seq3, `write /tmp/notes/a.txt` → A seq4, then
   `config set Display/Theme dark` → `saved slot B seq=5`;
3. the driver corrupts the newest slot's first payload sector (header intact,
   zeroed range) → `slot B invalid (payload crc mismatch)`, fallback to A
   seq4: theme rolls back to `light` (the `dark` value existed only in the
   corrupt slot) while `/tmp/notes/a.txt` survives (it was already in A seq4);
4. both payloads corrupted → both slots reported invalid, fresh format, old
   file gone, theme back to `classic` (defaults reseeded).

The corruption step is byte-level on the image file, not another guest write,
so it cannot be fooled by the guest's own consistency.

## Configuration store details

`docs/CONFIG.md` is the contract; ADR-0022 records the decision. The store is
serialised by `os_core::config` (`POCF` version 1: 8-byte header plus
key/value entries; ≤64 entries, keys ≤96 bytes in `/`-separated
`[A-Za-z0-9_.-]` components of ≤32 bytes, values ≤128 printable ASCII bytes,
blob ≤7984 bytes) into the ordinary VFS file `/etc/config`, so the checkpoint
format is untouched and the store rides the existing slot rotation, CRC check
and fallback rule. Boot reads the file when present and otherwise seeds
`System/Name=PROJECT_OS`, `Display/Theme=classic`, `Net/Mode=static` in
memory; the first `config set`/`config del` writes the file and the shell
boundary flush persists it. The shell session exercises `config`,
`config get`, `config set`, `config list`, `config del` and the `error: bad
key` path; the persistence session pins the reboot-load, rollback and
fresh-format cases. Limits are enforced in the codec (host tests) and in the
shell, never by truncation.

## Network test details

`scripts/test_net_qemu.sh` (driver `scripts/test_net_qemu.py`) boots the
interactive ISO on `q35` with an explicitly pinned NIC and a user-mode network:

```text
qemu-system-x86_64 -machine q35 -m 512M -smp 1 \
  -cdrom artifacts/project-os-interactive.iso -boot d \
  -display none -monitor none \
  -nic user,model=e1000,hostfwd=udp:127.0.0.1:<host_port>-:7777 \
  -serial tcp:127.0.0.1:<port>,server=on,wait=off -no-reboot
```

`q35` is pinned because its default NIC is an unsupported e1000e; `model=e1000`
selects the 82540EM the driver targets. The driver picks a free host UDP port
for `hostfwd` before starting QEMU and then asserts on serial: the
`net: e1000 at ... bar0 ... size ... KiB` line, `net: mac 52:54:00:` (slirp's
default OUI), the IP line, a `net: loopback selftest:` line (any of the three
documented outcomes is accepted; QEMU reports `unsupported`),
`net selftest: passed (arp resolved, icmp echo reply in ...)` and `,net` in the
marker. It then types `net` (status tokens including `arp 1/8 entries`),
`ping 10.0.2.2 -c 3` (three replies and `0% loss`) and `udpecho 7777`, sends
`PROJECT_OS-NET-TEST` (19 bytes) over UDP to the host port, and asserts the
identical payload returns to the same socket and the guest printed
`udpecho: echoed 19 bytes to 10.0.2.2:<port>`. Transcripts:
`artifacts/qemu-net-session.log` and `artifacts/qemu-net.log`.

## VirtualBox

`scripts/test_virtualbox.sh` **was executed here and passed** with VirtualBox
7.2.20r175154 on the default VM profile (BIOS, 512 MiB, one CPU, PIIX3), now
with `--nic1 nat --nictype1 82540EM` so the kernel's e1000 probe runs there.
The first run exposed a real kernel defect rather than a VirtualBox bug: IRQ0 was
pending in the PIC (`IRR=0x01`, `ISR=0x00`) while IF=1, because LINT0 was
masked and the APIC MMIO page is not part of Limine's HHDM. The kernel now maps
that page through the live 4-level tables and configures LINT0 ExtINT; the
serial log then shows `timer: 5 ticks observed` and the success marker.

Networking in that VM is reported as observed, not claimed green: the kernel
detected the NIC (`net: e1000 at 00:03.0 bar0 0xf0000000 size 128 KiB`, MAC
`08:00:27:c4:5d:0a`), logged `net: loopback selftest: unsupported`, and the
headless NAT did not answer the gateway ARP
(`net selftest: no gateway reply (arp timeout)`). That path is non-fatal by
design: the boot reached the unchanged marker without `,net`.
`docs/NETWORK.md` §11 records the exact lines.

The script is deliberately defensive:

- creates a VM named `PROJECT_OS-test-<pid>` in `artifacts/vbox`;
- attaches only the generated ISO, no host disk;
- captures COM1 to a file;
- on completion or failure, powers off and deletes **only that VM**;
- refuses to reuse an existing VM name;
- if `VBoxManage` is missing, prints exact manual steps and exits 2.

Manual settings for a BIOS boot: Other/Unknown 64-bit, 512 MiB, no hard disk,
optical drive attached to the ISO, COM1 raw-file logging. For EFI, enable EFI in
System → Motherboard. A VirtualBox result must be recorded separately from QEMU.

## UEFI

The ISO is verified to contain a UEFI El Torito entry (platform 0xEF, no
emulation, 6144 sectors). **That is not a UEFI boot test.** To run one:

```sh
OVMF_FD=/path/to/OVMF.fd scripts/test_boot_qemu.sh --firmware uefi
```

Install/obtain OVMF from a trusted, pinned source; record its version and checksum
in `progress.md` before claiming a UEFI boot.

## Troubleshooting

| Symptom | Likely cause | Action |
|---|---|---|
| `cannot find linker script` | Cargo not invoked from the workspace root | run scripts from the repo root |
| `rust-lld`/target missing | target not installed | `rustup target add x86_64-unknown-none` |
| ISO not found | build skipped | `scripts/make_iso.sh` |
| No marker, no serial output | wrong COM1 configuration or boot failure before serial init | check QEMU log; try `-d int` or a graphical run |
| QEMU does not exit | cmdline lacks `debug_exit` or device flag missing | use the provided script |
| Exit status 1 from the boot script | marker absent/panic | inspect `artifacts/qemu-serial.log` |
| Host-policy toolchain update | `rust-toolchain.toml` uses the `stable` channel | pin an exact version in CI if required |

## Evidence limitations

- QEMU is TCG software emulation on this host; timings are not hardware timings.
- The ATA/checkpoint path is verified on QEMU `-machine pc` with a raw image
  only: no AHCI, virtio, DMA, real hardware or VirtualBox disk test exists yet,
  and VirtualBox still boots the diskless profile.
- QEMU BIOS was exercised at 256 MiB, 512 MiB and 1 GiB. Larger sizes,
  hotplug/NUMA and physical hardware were not run.
- VirtualBox BIOS is now verified (marker with `user` checks); VirtualBox EFI,
  UEFI/OVMF, real hardware, SMP, Windows apps and GPUs are untested or
  unimplemented and are not claimed.
- The e1000 path is verified on QEMU slirp only. The VirtualBox run attached
  the 82540EM NAT NIC and the kernel detected it, but the headless NAT did not
  answer the gateway ARP within the 3 s budget; no VirtualBox network claim is
  made (`docs/NETWORK.md` §11).
