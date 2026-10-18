# Research log

Access date for all entries: **2026-10-17**. Sources are primary where possible.
Confidence reflects both the source quality and the experiments run in this
repository. This file records conclusions in our own words; no large copyrighted
text is copied.

## Environment and pinned inputs

| Input | Version / identity | Verification |
|---|---|---|
| Rust | `rustc 1.99.1` stable (host `x86_64-pc-windows-msvc`; 1.99.0 was installed initially) | `rustc -vV` |
| Kernel target | `x86_64-unknown-none` | `rustup target list --installed` |
| Limine | 12.9.3 binary release | SHA-256 `e02a473c6dc6df25159697f94930272397f21fd86e5d4eeee61bba72c95ec8d2` |
| QEMU | 11.1.0 | `qemu-system-x86_64 --version` |
| cdrtools mkisofs | 3.02a09 (i686-pc-mingw32) | `mkisofs --version` |
| VirtualBox | 7.2.20r175154 | `VBoxManage --version`; BIOS boot passed |
| OVMF firmware | not available | UEFI boot not executed |

## R-001 — Limine handoff and request ABI

**Question.** What exactly does Limine guarantee at kernel entry, and how are
requests/responses laid out?

**Sources.**
- Limine protocol specification: <https://github.com/limine-bootloader/limine-protocol/blob/trunk/PROTOCOL.md>
- Limine 12.9.3 usage: <https://github.com/Limine-Bootloader/Limine/blob/v12.9.3/USAGE.md>
- Limine 12.9.3 configuration: <https://github.com/Limine-Bootloader/Limine/blob/v12.9.3/CONFIG.md>
- Limine releases: <https://github.com/Limine-Bootloader/Limine/releases/tag/v12.9.3>

**Findings.**
- Base revision 6 is current. Revisions 5 and 6 tighten x86-64/aarch64/RISC-V
  entry state; on x86-64 revision 6 has the same guarantees as 5.
- Segments are mapped in the higher half at/above `0xffffffff80000000`; PT_LOAD
  permissions are honoured (read always available, NX where supported).
- The bootloader provides a stack (at least 64 KiB, or the requested size) and
  pushes an invalid return address; all GPRs are zero; IF/DF are clear; the IDT
  is undefined.
- At base revision 5+, the local APIC is enabled and software-enabled, all LVT
  fixed/NMI/ExtINT entries are masked, legacy PICs are masked, and IOMMU
  translation/remapping is disabled for Intel VT-d / AMD-Vi disabled.
- Requests are ID-tagged structures anywhere in the loaded image, 8-byte
  aligned; the response pointer is writable bootloader-owned memory. The
  request delimiters must be honoured.
- Base revision 4+ maps reserved-mapped and ACPI regions in addition to
  usable/reclaimable/executable/framebuffer regions; base revision 5 additionally
  guarantees ACPI and SMBIOS tables are mapped.

**Confidence.** High. The kernel checks the base-revision tag, loads at the
expected address, and boots in QEMU with base revision 6.

**Experiment.** `readelf -lW` confirms the three PT_LOAD segments and writable
Limine requests. QEMU run prints `limine: requested base revision 6, loaded 6`,
the expected memory map, HHDM offset and framebuffer.

## R-002 — Rust freestanding target and linker behavior

**Question.** Which target/flags and linker produce a fixed-address ELF kernel on
a Windows host?

**Sources.**
- Rust platform support, `x86_64-unknown-none`: <https://doc.rust-lang.org/rustc/platform-support/x86_64-unknown-none.html>
- Target specification (linker, features, relocation model): <https://doc.rust-lang.org/stable/nightly-rustc/src/rustc_target/spec/targets/x86_64_unknown_none.rs.html>
- Codegen options: <https://doc.rust-lang.org/rustc/codegen-options/index.html>
- Cargo config: <https://doc.rust-lang.org/cargo/reference/config.html>

**Findings.**
- The built-in target selects `rust-lld` with the GNU/ELF flavor directly; no
  GCC/Clang driver is needed.
- It defaults to the kernel code model, disables the red zone, uses
  `panic=abort`, and is soft-float with no MMX/SSE/AVX by default.
- Its default relocation model is position-independent; a fixed-address image
  loaded without relocation processing needs `-C relocation-model=static`.
- A custom linker script is passed with `-C link-arg=-Tkernel/linker.ld`; the
  path is relative to Cargo's working directory (workspace root).

**Confidence.** High; the documented behavior matched the actual build after one
path correction (`-Tkernel/linker.ld` rather than `-Tlinker.ld`).

**Experiment.** Built ELF entry `0xffffffff80000000`, three LOAD segments,
`.limine_requests` in the RW segment; QEMU boots it.

## R-003 — Limine ISO creation without xorriso

**Question.** Limine's docs require `xorriso`; can a BIOS+UEFI hybrid ISO be
created with cdrtools `mkisofs` and booted?

**Sources.**
- Limine USAGE.md hybrid ISO section (xorriso recipe).
- cdrtools `mkisofs -help` on the installed 3.02a09 build.
- El Torito structures: edk2 `MdePkg/Include/IndustryStandard/ElTorito.h`
  <https://github.com/tianocore/edk2/blob/master/MdePkg/Include/IndustryStandard/ElTorito.h>
- Fedora bootable-CD guide (BIOS+UEFI catalog pattern):
  <https://fedoraproject.org/wiki/User:Pjones/BootableCDsForBIOSAndUEFI>

**Findings.**
- cdrtools supports `-eltorito-alt-boot`, `-eltorito-platform ID`, and repeated
  `-b`; it does **not** support the `-e` option used by xorriso, and this build
  accepts only a numeric platform id (`239` = 0xEF), not `efi`.
- El Torito section header id `0x90` means more section headers follow; `0x91`
  is the final section header. The initial parser assumed the reverse and failed
  on the real catalog; the edk2 header resolved it.
- `limine bios-install` refuses an ISO without an MBR. `--force` adds a hybrid
  MBR to the generated ISO file and succeeds. This is safe for a build artifact
  and never applied to a device.

**Confidence.** High for the tested cdrtools/QEMU path; medium for other mkisofs
builds, which may differ.

**Experiment.** `verify_iso.py` parses the PVD, boot record and catalog:
platform 0x00, 4 sectors; platform 0xEF, 6144 sectors; both no-emulation. QEMU
BIOS boots before and after the forced MBR install. UEFI boot is untested.

## R-004 — QEMU automated boot and debug exit

**Question.** How can a boot test terminate deterministically and be asserted
non-interactively?

**Sources.**
- QEMU invocation: <https://www.qemu.org/docs/master/system/invocation.html>
- QEMU `isa-debug-exit` implementation:
  <https://github.com/qemu/qemu/blob/master/hw/misc/debugexit.c>
- QEMU x86 system target: <https://www.qemu.org/docs/master/system/target-i386.html>

**Findings.**
- `-display none -monitor none -serial file:<path>` captures guest COM1 without a
  window or interactive monitor.
- The debug-exit device computes process status `(written_value << 1) | 1`. Its
  default I/O base is not 0xf4; the test supplies
  `-device isa-debug-exit,iobase=0xf4,iosize=0x04` explicitly.
- Our kernel writes only when `debug_exit` appears in its cmdline, and falls
  through to idle otherwise. QEMU exits 33 (`0x10` written).

**Confidence.** High; formula verified by observed exit status 33.

**Experiment.** Successful QEMU run; corrupted ISO negative test had no marker
and the test script returned 1.

## R-005 — x86-64 descriptor tables, exceptions, PIC and PIT

**Question.** What is the minimal correct bring-up for protected-mode descriptor
tables, exception reporting and a periodic interrupt?

**Sources.**
- Intel 64 and IA-32 Software Developer Manuals:
  <https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html>
- OSDev wiki (secondary, used as a cross-check only): <https://wiki.osdev.org/>
- Limine x86-64 machine-state guarantees (above).

**Findings.**
- A 64-bit kernel GDT needs null, code (L=1), data and a 16-byte TSS descriptor.
  TSS size is 104 bytes; `iomap_base >= sizeof(TSS)` means no I/O bitmap.
- IST1 gives the double-fault handler a clean stack even if the normal stack is
  unusable.
- Same-ring interrupt frames contain RIP/CS/RFLAGS only; user-mode frames add
  RSP/SS and need a different unwinder.
- The 8259 must be remapped away from exception vectors 0–31; IRQ0 uses vector
  0x20 and needs an EOI.
- The 8254 channel-0 divisor is `1193182 / frequency`; 100 Hz → 11931.

**Confidence.** High for the implemented subset; validated by QEMU (int3 handler
and 5 timer ticks). The APIC/IOAPIC path is not implemented.

**Note.** Because Limine enables the local APIC, the legacy-PIC path explicitly
clears `IA32_APIC_BASE.EN` before enabling interrupts. This is a documented
bring-up choice (ADR-0005), not a claim that the LAPIC is unusable.

## R-006 — Memory-management design references

**Question.** Which documented techniques should inform our own allocator and
future VM design, without copying Linux source?

**Sources.**
- Linux memory-management docs: <https://www.kernel.org/doc/html/latest/mm/>
- Memory-allocation guide: <https://docs.kernel.org/core-api/memory-allocation.html>
- Physical memory: <https://docs.kernel.org/mm/physical_memory.html>
- Slab allocator guide: <https://docs.kernel.org/admin-guide/mm/slab.html>
- Linux license rules: <https://github.com/torvalds/linux/blob/master/Documentation/process/license-rules.rst>

**Findings.**
- The layers must stay separate: boot allocator, physical page allocator, virtual
  memory/page tables, kernel heap/slab, user VM/heap, page cache/reclaim.
- Buddy allocation, per-CPU page sets and slab/size classes are design ideas to
  evaluate with workloads, not a single explanation for Linux performance.
- Linux kernel code is GPL-2.0-only; we implement our own code and cite ideas.

**Confidence.** High as documentation; implementation is ours and measured only
as a host bitmap model (see `docs/MEMORY_MANAGEMENT.md`).

## R-007 — Windows compatibility and graphics (future work)

**Sources.**
- PE/COFF: <https://learn.microsoft.com/en-us/windows/win32/debug/pe-format>
- Win32 API index: <https://learn.microsoft.com/en-us/windows/win32/apiindex/windows-api-list>
- Processes and threads: <https://learn.microsoft.com/en-us/windows/win32/procthread/about-processes-and-threads>
- WDDM architecture: <https://learn.microsoft.com/en-us/windows-hardware/drivers/display/windows-vista-and-later-display-driver-model-architecture>
- Windows driver docs: <https://learn.microsoft.com/en-us/windows-hardware/drivers/>
- NVIDIA open kernel modules: <https://github.com/NVIDIA/open-gpu-kernel-modules>
- NVIDIA README (Turing+, GSP requirements): <https://github.com/NVIDIA/open-gpu-kernel-modules/blob/main/README.md>
- Nouveau: <https://nouveau.freedesktop.org/>
- Mesa NVK (Linux kernel requirements): <https://docs.mesa3d.org/drivers/nvk.html>
- ReactOS architecture (scope reference only, no code reuse):
  <https://reactos.org/architecture/>

**Findings.** PE loading, NT-style object/process services, Win32/DXGI/Direct3D
runtime behavior and WDDM kernel-driver compatibility are distinct layers with
separate tests. NVIDIA's open modules are Linux kernel modules; NVK documents
Linux DRM dependencies. Neither can be treated as a drop-in driver for this
kernel. No compatibility claim is made in this milestone.

**Confidence.** High for the architectural separation; zero implementation
evidence for compatibility, and therefore no compatibility claim.

## R-008 — Legacy PIC delivery when the local APIC is enabled

**Question.** Why did IRQ0 stay pending in the 8259 on VirtualBox while QEMU
delivered it, and what is the correct bring-up sequence?

**Sources.**
- Intel 64 and IA-32 SDM, Vol. 3A, local APIC / LINT0 ExtINT delivery and the
  8259 programming model: <https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html>
- Limine protocol, HHDM and paging guarantees:
  <https://github.com/limine-bootloader/limine-protocol/blob/trunk/PROTOCOL.md>
- Oracle VirtualBox 7.2 user guide, APIC/x2APIC and firmware APIC options:
  <https://docs.oracle.com/en/virtualization/virtualbox/7.2/user/>

**Claim.** A 64-bit guest with an enabled APIC receives 8259 interrupts through
LAPIC LINT0. If LINT0 is masked, the PIC can hold an interrupt in IRR with
`ISR=0` while the CPU never takes it, even though IF=1. Configuring xAPIC mode
and setting LINT0 to ExtINT and unmasked restores delivery. The APIC MMIO page
(`0xfee00000`) is device memory and may not be covered by the HHDM, so it must
be mapped explicitly.

**Confidence.** High; confirmed by direct observation and a fix that makes both
QEMU and VirtualBox reach the marker. The exact LINT0 state left by firmware was
not read back, only inferred from the pending-IRR/zero-ISR symptom.

**Experiment.** VirtualBox 7.2.20 default BIOS profile: before the fix,
`pic_irr=0x01`, `pic_isr=0x00`, `ticks=0`, then
`PROJECT_OS_FATAL: timer did not deliver 5 interrupts (observed 0)`. After
mapping the APIC page and writing LINT0 ExtINT: `timer: 5 ticks observed` and
`PROJECT_OS_BOOT_SUCCESS:dev`.

**Conclusion.** Implemented as ADR-0011. The fallback path (disable LAPIC) is
kept only for hosts where mapping or LINT0 programming fails.

## R-009 — Block-device path without PCI, and QEMU machine choice

**Question.** Which block-device path is reachable before a PCI enumerator
exists, and does QEMU's `q35` machine expose the legacy ATA PIO ports?

**Sources.**
- QEMU source (master, accessed 2026-10-17): `hw/ide/piix.c` registers the
  legacy port pairs `{0x1f0, 0x3f6, 14}` and `{0x170, 0x376, 15}`;
  `hw/ide/ahci.c` and `hw/ide/ich.c` register no legacy ports (AHCI MMIO BAR5
  plus the ICH9 IDP index/data pair only); `hw/i386/pc_q35.c` attaches drives
  to `ich9-ahci`. <https://github.com/qemu/qemu>
- OSDev wiki, "ATA PIO Mode": register map, IDENTIFY/read/write/cache-flush
  sequences, LBA28, status bits (used as the protocol cross-check):
  <https://wiki.osdev.org/ATA_PIO_Mode>
- QEMU invocation docs (`-machine`, `-drive`):
  <https://www.qemu.org/docs/master/system/invocation.html>
- CRC-32 convention: IEEE 802.3 reflected polynomial 0xEDB88320, implemented
  from the polynomial in our own code; standard check value 0xCBF43926 for
  `"123456789"`.

**Claim.** On QEMU master, only the PIIX southbridge (i440fx `pc` machine)
registers the legacy primary/secondary IDE port ranges. `q35` (ICH9-AHCI)
requires an AHCI driver via PCI BAR discovery, which the kernel does not have.
A polling ATA PIO driver on the fixed 0x1F0/0x3F6 ports is therefore the only
block path reachable without PCI; the persistence harness runs `-machine pc`
while all other tests keep `q35`.

**Confidence.** Medium-high from source reading (QEMU master at access date);
the exact machine behavior could still differ, so boot 1 of the persistence
test asserts `blk: found ...` as the experiment.

**Experiment.** PASSED (QEMU 11.1.0, 2026-10-17): `-machine pc` with an 8 MiB
zeroed raw disk attached `if=ide,index=0,media=disk` and the BIOS ISO; serial
log shows `blk: found QEMU HARDDISK, 16384 sectors` and the boot marker with
isa-debug-exit status 33. The no-disk path (q35) logs
`blk: no ATA device on primary channel` and the marker is unchanged. If the
probe had failed, implementation would have stopped and the design been
revisited (minimal PCI + AHCI, or virtio).

**Conclusion.** Confirmed. Implemented in `kernel/src/blk.rs` (primary master,
LBA28, polled PIO); the persistence harness pins `-machine pc` while all other
tests keep `q35`; recorded as ADR-0016.

## R-010 — Limine modules and the ELF64 executable format

**Question.** What are the exact Limine module request/response semantics for a
base-revision-6 kernel (pinned Limine 12.9.3), and which ELF64 facts does a
minimal native executable loader need before it can load our own programs?

**Sources.**
- `limine-protocol` repository (trunk, accessed 2026-10-17): PROTOCOL.md
  "Module" feature and `include/limine.h`. The protocol documents and header
  live in the protocol repository, **not** in the Limine main repository tree.
  <https://github.com/Limine-Bootloader/limine-protocol>
- System V gABI (program header table, `PT_LOAD`, `p_offset/p_vaddr/p_filesz/
  p_memsz/p_flags/p_align`) and the x86-64 psABI (ELFCLASS64, little-endian,
  `EM_X86_64 = 62`, `ET_EXEC = 2`); our parser is written from these field
  definitions, no code copied.

**Claim.** Verified from the header and protocol text: the module request ID is
`{LIMINE_COMMON_MAGIC, 0x3e7e279702be32af, 0xca1c4f3bd1280cee}` with response
`{revision, module_count, limine_file **modules}`; **if no modules are
available, no response is provided** (NULL check required); each `limine_file`
is 112 bytes (`revision, address, size, path, string, media_type, unused,
tftp_ipv4[4], tftp_port, partition_index, mbr_disk_id, gpt_disk_uuid,
gpt_part_uuid, part_uuid`); `address` has the HHDM offset pre-applied, is at
least 4 KiB aligned, and every covered 4 KiB chunk belongs exclusively to the
module; `path` is NUL-terminated ASCII with a leading slash. The module request
struct carries request-revision-1 fields (`internal_module_count`,
`internal_modules`) that a revision-0 request leaves untouched.

**Confidence.** High for the ABI facts (read directly from the pinned header
and protocol document). Medium for "the kernel can simply slice
`address..address+size` through the HHDM" until the boot experiment — which is
exactly what the stage-3 boot lines assert.

**Experiment.** PASSED (QEMU 11.1.0 + VirtualBox 7.2.20, 2026-10-18): the ISO
carries three modules (`hello.elf`, `fault.elf`, `bad.elf`); serial shows
`elf: 3 modules present`, the exact ring-3 greeting from the ELF, a clean
rejection `user: /boot/bad.elf rejected (bad magic)`, one isolated user fault,
exit code 0, and the marker gains `,elf`. Evidence: `scripts/test_elf_qemu.sh`,
`artifacts/qemu-serial-elf.log`, full `scripts/test_all.sh` run.

**Conclusion.** Confirmed and implemented: `kernel/src/limine.rs` (module
request + `modules()`/`module_by_path()`), `os_core::elf` parser (19 host
tests), `kernel/src/loader.rs` (two-phase inspect/map), `userland/` workspace;
recorded as ADR-0017 with the config-merge and MAX_PATH deviations in
`docs/ELF_LOADER.md` §12.

## R-011 — Framebuffer readback and write-combining ordering

**Question.** Can the kernel read back what it wrote to the Limine linear
framebuffer and compare it byte-for-byte with a software re-render, and which
memory-ordering instruction makes that comparison valid if the mapping is
write-combining (WC)?

**Sources.**
- Intel 64 and IA-32 Architectures Software Developer's Manual, Volume 3A,
  §11.3 "Methods of Caching Available" / WC memory ordering: partially filled
  WC buffers are flushed by a serializing event such as `SFENCE` or `MFENCE`
  (accessed 2026-10-18 via the 2007 Intel "64 Architecture Memory Ordering"
  white paper and Intel community quotations of §11.3).
- Intel application note 244422, write-combining implementation guidelines
  (fetch/read-back rules around WC memory), accessed 2026-10-18.
- Limine protocol framebuffer response: the framebuffer is described by
  `address`, `width`, `height`, `pitch`, `bpp` and memory model; Limine maps it
  readable/writable and the address has the HHDM offset pre-applied
  (`limine-protocol` PROTOCOL.md, pinned Limine 12.9.3).

**Claim.** Under the bootloader-provided mapping, plain stores followed by
ordinary loads observe prior writes; if a platform maps the framebuffer WC, an
`SFENCE` before the read-back orders the flushed WC buffers against subsequent
reads. Pixel stores need no fence themselves (only the read-back self-test
does). Confidence high for the fence requirement, medium that QEMU/VirtualBox
mappings are WB (the experiment disambiguates empirically, not by assumption).

**Experiment.** PASSED (QEMU 11.1.0 std VGA, 1280×800, pitch 5120; VirtualBox
7.2.20 VBoxVGA, 1024×768, pitch 4096; 2026-10-18). The kernel re-renders the
whole grid plus cursor into a heap scratch buffer, executes `sfence`, and
compares scanline by scanline against the framebuffer: serial logs
`fbcon selftest: passed (4070400 bytes match after 1059 redraws)` in QEMU and
`fbcon selftest: passed (3133440 bytes match after 1088 redraws)` in VirtualBox.
A QEMU monitor `screendump` (`artifacts/console-screendump.ppm`) independently
shows the text on the scanout.

**Conclusion.** Confirmed and implemented: `arch::x86_64::sfence()` with the
SDM citation, `fbcon::verify()` doing the scratch re-render and pitch-aware
per-scanline comparison, fatal on mismatch; the framing memcmp variant (whole
contiguous buffer against a strided framebuffer) was rejected during bring-up
because it misaligns after the first scanline.

## Open questions / BLOCKED items

| Item | State | Next evidence needed |
|---|---|---|
| UEFI boot of the ISO | BLOCKED (no OVMF firmware) | Boot with a pinned OVMF image and record serial/exit status |
| VirtualBox EFI boot | NOT ATTEMPTED | Enable EFI in an isolated VM; record serial evidence |
| Real-hardware boot | NOT ATTEMPTED | Only on a disposable machine; no host disk/EFI changes |
| 5-level paging | NOT IMPLEMENTED | Kernel requests max 4-level; revisit with 5-level page-table code |
| APIC/IOAPIC/SMP | NOT IMPLEMENTED | LAPIC timer calibration, IOAPIC routing, SMP startup |
| KASLR | NOT REQUESTED | Needs relocatable kernel and slide handling |
| Windows API/driver compatibility | NOT IMPLEMENTED | PE loader, compatibility corpus, object/syscall design |
| NVIDIA acceleration | RESEARCH ONLY | Scoped port study and one named GPU target |
