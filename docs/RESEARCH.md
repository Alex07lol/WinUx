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

## R-012 — e1000 networking, slirp behaviour and QEMU loopback semantics

**Question.** What register/descriptor model does the Intel 82540EM need for a
minimal polled driver (reset, MAC acquisition, TX/RX legacy rings); which
loopback mechanisms do the emulated NICs actually implement; and what does QEMU
user-mode (slirp) networking guarantee that an automated harness can depend on?

**Sources.**
- *8254x Family of Gigabit Ethernet Controllers Software Developer's Manual*
  (Intel, public documentation): `CTRL.RST` self-clearing reset, `RAL/RAH`,
  `RCTL` (EN/BAM/SECRC, `LBM` bits 7:6), `TCTL` (EN/PSP/RTLC/CT/COLD), `TIPG`,
  legacy 16-byte descriptors (`CMD.EOP/IFCS/RS`, `STA.DD`), RX tail/head
  protocol, EEPROM read via `EERD`.
- QEMU source `hw/net/e1000.c` (qemu master, read 2026-10-18):
  `e1000_send_packet` loops a frame back to the receive path only when the PHY
  register `MII_BMCR_LOOPBACK` is set; RCTL `LBM` writes are stored but never
  acted on. This corrects the design-time assumption in `docs/NETWORK.md` §2.
- QEMU system-emulation documentation for slirp: guest `10.0.2.15/24`, gateway
  and host `10.0.2.2`, ARP answering for the gateway, ICMP echo to it, and
  `hostfwd=udp:127.0.0.1:<port>-:7777` forwarding host datagrams to a guest
  port.
- OSDev wiki 8254x page: VirtualBox does not implement `EERD`; a 4-wire EEPROM
  clock fallback is needed for MAC reads there.
- PCI Local Bus Specification 3.0: mechanism #1 config access (`0xCF8`/`0xCFC`),
  BAR sizing by all-ones write-back with restore.

**Claim.** A polled driver over legacy descriptors is sufficient for a bring-up
stack (`RCTL = EN | BAM | SECRC`, `TCTL = EN | PSP | CT | COLD | RTLC`,
synchronous TX with `STA.DD` polling, RX tail advance after copy). QEMU
auto-loads the MAC into `RAL/RAH` at reset; VirtualBox needs the EEPROM
fallback. Slirp answers ARP for the gateway and echoes ICMP locally, so a
harness can prove ARP + ICMP + UDP end to end with no external network.
Confidence high for the SDM-derived register model and slirp behaviour (observed
end to end); the loopback claim is settled by source inspection: emulated e1000
does **not** honour `RCTL.LBM`, so the loopback selftest legitimately reports
`unsupported` on both QEMU and VirtualBox.

**Experiment.** PASSED (QEMU 11.1.0, `q35`, pinned `-nic
user,model=e1000,hostfwd=udp:...`, 2026-10-18; `scripts/test_net_qemu.sh`).
Observed boot lines: `net: e1000 at 00:02.0 bar0 0xfebc0000 size 128 KiB`,
`net: mac 52:54:00:12:34:56`, `net: ip 10.0.2.15/24 gw 10.0.2.2`,
`net: loopback selftest: unsupported`,
`net selftest: passed (arp resolved, icmp echo reply in 0 ms)`, marker
`...,vfs,net`. Shell: `net` reported `tx 3 frames, rx 2 frames, arp 1/8
entries, errors 0`; `ping 10.0.2.2 -c 3` → three `ttl=255` replies, `0% loss`;
a host-forwarded 19-byte UDP datagram was echoed back byte-for-byte
(`udpecho: echoed 19 bytes to 10.0.2.2:<port>`). VirtualBox 7.2.20 with
`--nic1 nat --nictype1 82540EM`: NIC detected (`00:03.0`, MAC
`08:00:27:c4:5d:0a`) but the headless NAT did not answer the gateway ARP
(`net selftest: no gateway reply (arp timeout)`), non-fatal. A bring-up defect
was also exposed and fixed: the ICMP echo request/reply paths wrote the payload
after the checksum had been computed, so checksums never matched the wire
(details in `docs/NETWORK.md` §11).

**Conclusion.** Confirmed and implemented: `os_core::pci` + `os_core::net`
(host-tested, 128 → 179 unit tests), `kernel/src/pci.rs`,
`kernel/src/e1000.rs` (MMIO reset, RAL/RAH with EEPROM fallback, legacy rings,
polled RX/TX, non-fatal loopback check), `kernel/src/net.rs` (ARP cache,
IPv4/ICMP/UDP dispatch, `ping`, `udpecho`), shell builtins, and the q35 harness
step in `test_all.sh`. Recorded as ADR-0018; the corrected loopback finding is
also noted in `docs/NETWORK.md` §2 and §11. Limits (polled, static addressing,
one NIC, no TCP/DHCP/IRQ) stay as stated in `docs/NETWORK.md` §10.

## R-013 — PE32+ image layout, base relocations and a no-SDK fixture pipeline

**Question.** What does the PE32+ (AMD64) on-disk layout require of a loader:
which header fields are load-bearing, how are sections placed and mapped, how
does the `.reloc` base-relocation table encode DIR64 fixups, and can genuine
PE fixture binaries be produced with the pinned Rust toolchain alone — no
MSVC, no MinGW, no Windows SDK, no Wine?

**Sources.**
- Microsoft *PE Format* specification (Win32 docs, document revision dated
  2026-09-10; fetched 2026-10-18): MS-DOS stub with `e_lfanew` at `0x3C`;
  `PE\0\0` signature; COFF file header (`Machine`, `NumberOfSections`,
  `SizeOfOptionalHeader`, `Characteristics`: `EXECUTABLE_IMAGE` `0x0002`,
  `RELOCS_STRIPPED` `0x0001`, `DLL` `0x2000`); PE32+ optional header (magic
  `0x20B`, `AddressOfEntryPoint`, `ImageBase` 64 KiB-aligned,
  `SectionAlignment` ≥ `FileAlignment`, `FileAlignment` power of two
  512..64K, `SizeOfImage` multiple of `SectionAlignment`, `Subsystem` CUI = 3,
  `NumberOfRvaAndSizes`); section table (40-byte entries: `VirtualSize`,
  `VirtualAddress` aligned/ascending, `SizeOfRawData` multiple of
  `FileAlignment`, `PointerToRawData`, characteristics
  `MEM_EXECUTE/WRITE/READ`); data directories (base relocation = index 5);
  base-relocation blocks (`Page RVA`, `Block Size`, 2-byte entries with
  4-bit type — `0` ABSOLUTE skipped padding, `3` HIGHLOW (32-bit), `10`
  DIR64 (64-bit) — and 12-bit in-page offset); delta rule (fixups applied
  only when the load base differs from the preferred base); Windows loader
  limit of 96 sections.
- Toolchain probe (this checkout, 2026-10-18): `rustc 1.99.1` with the
  `x86_64-pc-windows-msvc` target links a `no_std` binary with
  `rust-lld -flavor link /nodefaultlib /entry:_start /subsystem:console`;
  rust-lld ships inside the sysroot
  (`lib/rustlib/x86_64-pc-windows-msvc/bin/rust-lld.exe`). Probe output:
  valid PE32+, 3584 bytes, 5 sections (`.text` RX, `.rdata` R, `.data` RW,
  `.pdata` R, `.reloc` R), entry RVA `0x1000`, image base `0x140000000`,
  subsystem 3, relocation directory RVA `0x5000` size 12 (one block, one
  DIR64 entry from a `global_asm!` `.quad marker` slot). A Rust `static`
  with an absolute function-pointer initializer is impossible (pointer→int
  const-eval is rejected), which is why the slot is emitted via
  `global_asm!`.

**Claim.** A PE32+ loader needs the header/section/relocation validation as
specified, can map sections per their alignment and flags with W^X derived
from `MEM_EXECUTE`/`MEM_WRITE`, and can implement the delta rule by walking
4 KiB relocation blocks and adding `loaded_base - image_base` to each DIR64
target. Genuine fixture binaries are buildable from Rust source with the
pinned toolchain (rustc COFF codegen + rust-lld PE linking, no CRT, our own
entry); `e_lfanew`-targeted corruption (zeroed signature) yields a
deterministic reject vector for the parser probe. Confidence high for the
layout and delta rule (spec text); the fixture pipeline is verified by the
probe above and end-to-end execution under the kernel loader (see the stage
evidence in `docs/PE_LOADER.md`).

**Experiment.** PASSED (2026-10-18). Commands: `scripts/build_user_pe.sh`,
`scripts/test_pe_qemu.sh`, `scripts/test_all.sh` (PASS=22 FAIL=0 SKIP=4; the
skip set is unchanged and unrelated). The final rust-lld fixture is 3072 bytes
with **4** sections (`.text` RX, `.rdata` R, `.data` RW, `.reloc` R — no
`.pdata`: the fixture has no unwind info, unlike the larger 5-section probe),
entry RVA `0x1000`, image base `0x140000000`, one DIR64 entry. Observed serial
lines (`artifacts/qemu-serial-pe.log`): `pe: 2 modules present`;
`pe: pehello.exe loaded at 0x400000 (entry 0x401000, 4 sections, 1 relocation
applied)`; `pe: hello from a PE32+ image via int 0x80`;
`pe: base relocation verified`; `user: /boot/pebad.exe rejected (bad
signature)`; `pe selftest: passed (PE32+ module loaded, base relocation
applied, int 0x80 write from CPL3, corrupt module rejected)`; marker
`checks=...,sched,user,elf,pe,vfs`. The ELF stage-3 lines and the full suite
stayed green in the same run.

**Conclusion.** Implemented: `os_core::pe` (host-tested parser, stable
rejection labels), `kernel/src/pe_loader.rs` (window/W^X/relocation policy),
`userland/pe/pehello` fixture workspace, `scripts/build_user_pe.sh`,
`scripts/check_user_pe.py`, `scripts/test_pe_qemu.sh` and the `test_all.sh`
`pe` step; recorded as ADR-0019. Limits (no imports/DLL/TLS/CLR/SEH, fixed
load base, no third-party binaries) stay as stated in `docs/PE_LOADER.md`
§1 and §9.

## R-014 — PE import tables, a raw-dylib fixture pipeline and three Win32 console contracts

**Question.** How does a PE32+ image declare DLL imports (descriptor array,
ILT/IAT, hint/name entries, ordinals), how does the Windows loader resolve
them into the IAT, can genuine import tables be produced by the pinned Rust
toolchain without an import library or any Windows SDK component, and what
are the exact documented contracts of `GetStdHandle`, `WriteConsoleA` and
`ExitProcess`?

**Sources.**
- Microsoft *PE Format* specification (Win32 docs; import-directory section,
  same revision family as R-013): 20-byte import descriptors terminated by an
  all-zero record; `OriginalFirstThunk` (ILT) with fallback to `FirstThunk`
  (IAT); 64-bit thunks whose bit 63 marks ordinal imports; `IMAGE_IMPORT_BY_
  NAME` = 16-bit hint + NUL-terminated name; the loader writes each resolved
  function address into the respective IAT slot.
- Microsoft Learn `GetStdHandle` (fetched 2026-10-18, Console-Docs revision
  425fa47): `HANDLE GetStdHandle(DWORD nStdHandle)`; `STD_OUTPUT_HANDLE` is
  `((DWORD)-11)` = `0xFFFF_FFF5`; failure returns `INVALID_HANDLE_VALUE`; the
  function itself performs no handle validation.
- Microsoft Learn `WriteConsole` (fetched 2026-10-18, same revision):
  `BOOL WriteConsole(HANDLE, const VOID*, DWORD, LPDWORD, LPVOID)` with the
  ANSI name `WriteConsoleA`; nonzero on success; `lpReserved` must be NULL;
  the A-variant writes bytes.
- Microsoft Learn `ExitProcess` (fetched 2026-10-18): `VOID
  ExitProcess(UINT uExitCode)`; ends the calling process; its thread/DLL
  bookkeeping steps presuppose objects this kernel does not have.
- Rust `raw-dylib` linkage: `#[link(name = "kernel32", kind =
  "raw-dylib")]` is stable in the pinned 1.99.1 toolchain and makes
  `rustc`/`rust-lld` synthesize the import descriptor, ILT and IAT without
  any import library file or Windows SDK tool.
- Toolchain probe (this checkout, 2026-10-18, temp workspace, same flags as
  stage 1): the probe linked as a 2560-byte PE32+ with sections `.text`,
  `.rdata`, `.data`, `.pdata`, `.reloc`; one import descriptor
  (`kernel32.dll`, ILT RVA `0x2088`, IAT RVA `0x20A8`, `TimeDateStamp` 0,
  `ForwarderChain` 0) and three name entries (`ExitProcess`, `GetStdHandle`,
  `WriteConsoleA`, all hint 0); one DIR64 relocation from the `global_asm!`
  marker slot. The IAT and the name strings live in read-only `.rdata`.

**Claim.** Import resolution requires only the descriptor/ILT/name walk with
the documented fallback and ordinal rule, plus an IAT write for each slot;
the fixed allowlist against our own native implementations is sufficient for
a first console surface. Genuine import tables need no SDK: `raw-dylib`
plus `rust-lld` emits them, and because the IAT lands in a read-only
section the loader must apply fixups through the physical frame (as it
already does for base relocations), which is also the Windows fixup-before-
protection order. Confidence high for the format and API contracts
(specification/doc text); the fixture pipeline is verified by the probe and
must additionally be verified end-to-end under our loader (see
`docs/WIN32_CONSOLE.md`).

**Experiment.** PASSED (2026-10-18). `scripts/build_user_pe.sh` built and
checked five fixtures (peconsole.exe 3584 bytes, SHA-256
`1e72a7b3d32421a5883b1c8344b1b1e149aa35a81b18648fcdba1e54e74eb406`; the two
mutants patched a single unique byte pattern each and stayed structurally
valid). `scripts/test_pe_qemu.sh` (QEMU 11.1.0, BIOS, 512 MiB, serial log
`artifacts/qemu-serial-pe.log`) observed, in order:
`pe: 5 modules present`,
`pe: peconsole.exe loaded at 0x400000 (entry 0x401000, 5 sections, 1 relocation applied)`,
`win32: GetStdHandle(-11) -> 1`,
`win32: hello from the Win32 console surface`,
`win32: WriteConsoleA(1, 44 bytes) -> 1`,
`win32: console surface verified`,
`win32: WriteConsoleA(1, 32 bytes) -> 1`,
`win32: ExitProcess(0)`,
`user: /boot/pebadimp.exe rejected (unknown import dll)`,
`user: /boot/pebadfn.exe rejected (unknown import function)`,
`win32 selftest: passed (kernel32.dll imports resolved, WriteConsoleA wrote to the console fd, ExitProcess ended the thread)`,
marker `checks=handoff,memmap,gdt,idt,exceptions,timer,allocator,heap,vm,sched,user,elf,pe,win32,vfs`,
and exit via isa-debug-exit. The full `scripts/test_all.sh` run finished
PASS=22 FAIL=0 SKIP=4 with the `pe` step covering the win32 assertions.
Bring-up defects found and fixed (all recorded in `progress.md`):
`pe_loader::map` iterated the full `Inspection::imports` array instead of
`imports[..import_count]` (a stale stage-1-shaped loop), and
`AddressSpace::translate` returned the frame base without the in-page offset,
which broke the unaligned IAT-slot fixups at `0x2108`/`0x2110` and would have
misplaced any unaligned write done through a translated address (the stage-1
relocation target happened to be page-aligned, so the defect stayed latent);
both fixed before the PASS, and the Windows-fixup-before-protection order
(HHDM write into read-only `.rdata`) held exactly as predicted.

**Conclusion.** Implemented: `os_core::pe` import parsing (allowlisted names,
named rejection labels), `pe_loader` resolution with a user-space stub page
and HHDM IAT patch, syscall services 3–5, the `peconsole` fixture plus two
deterministic negative mutants, and the extended fixture checker; recorded
as ADR-0020. Limits (three functions, one console handle, no DLL loading,
no ordinals/forwarders, no general compatibility) stay as stated in
`docs/WIN32_CONSOLE.md` §1 and §9.

## R-015 — Five-argument Win32 calls, per-thread last-error and a loader-owned command line

**Question.** What are the exact documented contracts of `WriteFile`,
`GetLastError`, `SetLastError`, `ExitThread`, `GetCurrentProcessId`,
`GetCurrentThreadId`, `GetTickCount` and `GetCommandLineA`; where does the
x64 ABI place the fifth integer argument (and does our stub-page dispatch
capture it already); and which error values do `WriteFile` and the
last-error APIs use?

**Sources.**
- Microsoft Learn `WriteFile` (fetched 2026-10-19, ms.date 2025-02-28):
  `BOOL WriteFile(HANDLE, LPCVOID, DWORD, LPDWORD, LPOVERLAPPED)` — the
  fifth parameter is the first stack argument; `lpNumberOfBytesWritten`
  receives the count; failure is `0` plus `GetLastError`; console handles
  paint at the cursor position through the same call.
- Microsoft Learn `GetLastError` (fetched 2026-10-19, ms.date 2024-02-02):
  the last-error code is maintained on a per-thread basis; functions set it
  on failure; some functions also set it on success and not all clear it,
  so callers must not assume a success resets it.
- Microsoft Learn `SetLastError` (fetched 2026-10-19, ms.date 2024-02-02):
  `VOID SetLastError(DWORD)`; value is kept in thread-local storage.
- Microsoft Learn `ExitThread` (fetched 2026-10-19, ms.date 2018-12-05):
  `VOID ExitThread(DWORD)`; ends the calling thread with the exit code; the
  last thread also terminates the process; per-thread stack cleanup is
  documented.
- Microsoft Learn `GetCurrentProcessId` / `GetCurrentThreadId` (fetched
  2026-10-19, ms.date 2018-12-05 / 2024-02-02): both return `DWORD`
  identifiers that uniquely identify the process/thread until termination.
- Microsoft Learn `GetTickCount` (fetched 2026-10-19, ms.date 2018-12-05):
  milliseconds elapsed since system start; resolution limited by the system
  timer, typically 10–16 ms; stored as a `DWORD`, wraps after 49.7 days;
  the docs explicitly advise overflow-aware comparisons.
- Microsoft Learn `GetCommandLineA` (fetched 2026-10-19, ms.date
  2024-10-23): returns a pointer to the process command-line string whose
  lifetime is managed by the system; applications must not free or modify
  it.
- Microsoft Learn *System Error Codes (0–499)* (fetched 2026-10-19,
  ms.date 2025-07-14): `ERROR_INVALID_HANDLE` = 6, `ERROR_INVALID_PARAMETER`
  = 87.
- Microsoft Learn *x64 calling convention* (reused from R-014): at function
  entry the first stack argument sits at `[rsp+0x28]` — 32 bytes of shadow
  space plus the 8-byte return address.

**Claim.** A stub of the shape `mov r10, [rsp+0x28]; mov eax, service;
int 0x80; ret` forwards a fifth integer argument to the kernel without any
entry-stub change, because `InterruptFrame` already captures `r10`
(`kernel/src/arch/x86_64/idt.rs:22-23`) and the sequence fits the 16-byte
stub stride. Per-thread `last_error` state in the scheduler thread slot is
the documented semantics of `GetLastError`/`SetLastError`; `WriteFile`'s
documented failure contract is `0` plus a WinError.h code (6 for a bad
handle, 87 for an invalid parameter); `GetTickCount`'s documented shape is
milliseconds in a wrapping `DWORD`, which a 100 Hz PIT × 10 reproduces with
10 ms resolution; `GetCommandLineA` returns a system-owned string, which the
loader's stub-page tail can hold for the module's real path. Confidence high
for the ABI position and API contracts (vendor documentation plus an
existing frame field); the end-to-end behaviour is verified only by the
stage-3 tests (see `docs/WIN32_PROGRAM.md`).

**Experiment.** PASSED (2026-10-19, QEMU 11.1.0, build `0.1.0-ec04e74f`).
`scripts/build_user_pe.sh` built and checker-verified `pebasics.exe`
(4096 bytes, SHA-256
`4a4aa956a3f5519875b43d8a31e91177e3392fa4316ef2ed4d98d70bdea8341b`, 5
sections, 1 relocation, 9 `kernel32.dll` imports) plus the `pebadfn2.exe`
mutant (`Sleep` substituted for `GetTickCount`; 4096 bytes, SHA-256
`9d6082b9792859eba6d4b4ca1eae70c09c46314e79588757cbb74c37e6cd473d`).
`scripts/test_pe_qemu.sh` reached
`PROJECT_OS_BOOT_SUCCESS:0.1.0-ec04e74f checks=handoff,memmap,gdt,idt,exceptions,timer,allocator,heap,vm,sched,user,elf,pe,win32,vfs`
with the pinned §7 lines observed (`pe: pebasics.exe loaded at 0x400000
(entry 0x401000, 5 sections, 1 relocation applied)`, the full `win32:`
program-basics sequence incl. `WriteFile(1, 28 bytes, overlapped) -> 0
(invalid parameter)`, `win32: GetCommandLineA() -> "/boot/pebasics.exe"`,
`win32 program selftest: passed (...)`) and
`user: /boot/pebadfn2.exe rejected (unknown import function)`; full
`scripts/test_all.sh`: `PASS=22 FAIL=0 SKIP=4`. Two defects were found and
fixed during bring-up, both recorded in `progress.md`: the command-line tail
was assumed zeroed but the stub page is `int3`-filled, so the terminator is
now written explicitly; and `sched::wait_all_finished` no longer uses its
TSC-frequency budget (Limine's reported frequency disagrees with real
`rdtsc` under QEMU, so the budget expired while the stage-3 program was still
making progress), bounding the wait by PIT ticks only. No divergence from the
documented API contracts above was observed; the deltas from Windows
semantics are the ones stated in `docs/WIN32_PROGRAM.md` §9.

**Conclusion.** PASSED. A 13-byte `mov r10, [rsp+0x28]; mov eax, service;
int 0x80; ret` stub forwarded the fifth stack argument through
`InterruptFrame.r10` with no entry-stub change; per-thread `last_error`
round-tripped (`SetLastError(42)` → `GetLastError() -> 42`, still 42 after a
successful `WriteFile`); the documented `WriteFile` failure values (0 + 6 /
0 + 87) and the loader-owned command line (`/boot/pebasics.exe`) behaved as
claimed. Confidence: high for the observed eleven-function surface in
QEMU; Windows compatibility is not claimed.

## R-016 — Win32 console input: blocking `ReadFile` line mode, echo and byte counts

**Question.** What are the documented contracts of `ReadFile` on a console
input handle in the default line-input mode — blocking, echo, byte count and
error behavior — what does `GetStdHandle(STD_INPUT_HANDLE)` return, and how
can a blocking read be implemented inside our `int 0x80` service without
deadlocking a single-CPU scheduler whose gate enters with interrupts
disabled?

**Sources.**
- Microsoft Learn `ReadFile` (fetched 2026-10-19, ms.date 2024-10-02):
  success is nonzero, failure is `0` plus `GetLastError`;
  `lpNumberOfBytesRead` is set "to zero before doing any work or error
  checking"; with a synchronous handle and NULL `lpOverlapped`, `ReadFile`
  "does not return until the operation is complete"; "Characters can be read
  from the console input buffer by using ReadFile with a handle to console
  input. The console mode determines the exact behavior. By default, the
  console mode is ENABLE_LINE_INPUT, which indicates that ReadFile should
  read until it reaches a carriage return."
- Microsoft Learn *High-Level Console Input and Output Functions* (fetched
  2026-10-19, ms.date 2018-07-12): in line-input mode `ReadFile` and
  `ReadConsole` "do not return to the calling application until the ENTER key
  is pressed"; "all available characters are read until either no more keys
  are available or the specified number of characters has been read. Unread
  characters are buffered until the next read operation. The functions report
  the total number of characters actually read. If echo input mode is
  enabled, characters read by these functions are written to the active
  screen buffer."
- Microsoft Learn `ReadConsole` (fetched 2026-10-19, ms.date 2018-07-12):
  behaves like `ReadFile`; "Cooked Mode is when ENABLE_LINE_INPUT is set ...
  calls to ReadFile or ReadConsole will not return until the enter key is
  pressed"; the console input handle must have `GENERIC_READ`.
- Microsoft Learn *Console Input Buffer* (fetched 2026-10-19, ms.date
  2018-07-12): input records queue behind a console input buffer; the
  high-level functions filter them down to a character stream (no function,
  arrow or mouse events).
- `GetStdHandle` (`STD_INPUT_HANDLE` is `(DWORD)-10`) and the WinError values
  (`ERROR_INVALID_HANDLE` = 6, `ERROR_INVALID_PARAMETER` = 87) are reused
  from the R-014/R-015 source set.

**Claim.** Our console model can serve `GetStdHandle(STD_INPUT_HANDLE)` with
a handle distinct from the output handle and a synchronous, line-mode
`ReadFile` that blocks until CR/LF, echoes accepted printable bytes, stores a
CRLF-terminated line in the user buffer and returns `TRUE` with the byte
count. `*lpNumberOfBytesRead` is zeroed before argument checks (documented);
a wrong handle returns `FALSE` + 6; a non-NULL overlapped pointer or a length
outside `2..=4096` returns `FALSE` + 87; unmapped pointers kill the thread
(memory-safety policy). CRLF inclusion in the line and count is our normative
choice: the fetched docs pin the return point and "characters actually read"
but not the terminator bytes for `ReadFile`. On one CPU the service can
block safely by enabling interrupts in the handler and waiting with `hlt`:
the serial RX IRQ supplies bytes, the PIT keeps preemption alive, and no lock
is held across the wait. Confidence high for the API shape, blocking/echo/
count statements and the `hlt` mechanism; medium for CRLF inclusion (project
contract, covered by our own tests, not vendor-verified).

**Experiment.** Run 2026-10-19: `scripts/test_input_qemu.sh` (QEMU 11.1.0,
q35, 512 MiB, `isa-debug-exit`, paced TCP serial; kernel build id
`0.1.0-58f98e48`, kernel SHA-256
`56bd754ea1f7f302417940cbe3da280fae1669b800d335d34fcafd438a9e06af`; the
input ISO was rebuilt once more when the runner re-ran the full suite after
the final comment-only source touch-up, so the recorded ISO SHA-256 is
`e5b8165347b33fb45769477488783804812d96f649622b69953d7f21d0c0d834`;
`peinput.exe` 4,096 bytes, SHA-256
`a2ab21aa4ea7e282c79e2ed1b437c72cee0a21e7b706ba4d0b007a46d5167de0`) passed:
with the fixture blocked in `ReadFile`, the harness typed `hello, win32`;
the transcript (`artifacts/qemu-input-session.log`) shows the echo, the
playback, `win32: ReadFile(2, 64 bytes) -> 1 (14 bytes)`, the error lines
for handle `1` (`-> 0 (invalid handle)`, `GetLastError() -> 6`) and a
non-NULL overlapped pointer (`-> 0 (invalid parameter)`, `GetLastError() ->
87`), the self-test line and the boot marker; QEMU then exited 33
(isa-debug-exit). One harness iteration failed on the CRLF expectation:
serial output expands LF to CRLF, so both the echo and the playback appear
as `hello, win32\r\r\n` while the buffer holds 14 bytes ending CRLF; the
harness expectation was corrected to the display form. A second iteration
failed because the marker line was asserted before its terminator arrived;
the harness now waits for the marker line to end. `scripts/test_all.sh`:
`PASS=23 FAIL=0 SKIP=4`.

**Conclusion.** Confirmed in QEMU for the observed surface: the genuine
PE32+ fixture receives a typed line through its own `ReadFile` import, the
read blocks without spinning (interrupts enabled + `hlt`, no lock held), the
count line reports the CRLF-inclusive length, `*lpNumberOfBytesRead` is
zeroed before checks, and the documented failure values (0 + 6, 0 + 87) and
serial lines match the contract. The CRLF-inclusion choice, the minimum
length 2 and line-mode-only echo behavior remain our documented contract,
not vendor-verified statements. Confidence: high within the recorded
QEMU scope; Windows console compatibility beyond the listed surface is not
claimed.

## R-017 — Registry read surface: `RegOpenKeyExA`/`RegQueryValueExA`/`RegCloseKey` over the config store

**Question.** What are the documented contracts of the three read-only
`advapi32.dll` registry functions — argument shapes, documented error codes,
buffer-size semantics and handle rules — and can our Phase 8 config store back
a registry-shaped view for a genuine PE32+ importer without over-claiming
Windows registry behavior?

**Sources.**
- Microsoft Learn `RegOpenKeyExA` (fetched 2026-10-19, ms.date 2018-12-05):
  "Opens the specified registry key. Note that key names are not case
  sensitive."; exported by `Advapi32.dll` (requirements table); five arguments
  `(hKey, lpSubKey, ulOptions, samDesired, phkResult)`; "If the lpSubKey
  parameter is NULL or a pointer to an empty string, and if hKey is a
  predefined key, then the system refreshes the predefined key, and phkResult
  receives the same hKey handle passed into the function."; `ulOptions` "Set
  this parameter to zero or" `REG_OPTION_OPEN_LINK`; `samDesired` is an access
  mask and "The function fails if the security descriptor of the key does not
  permit the requested access"; "Unlike the RegCreateKeyEx function, the
  RegOpenKeyEx function does not create the specified key if the key does not
  exist"; the example maps `ERROR_FILE_NOT_FOUND` to "Key not found."
- Microsoft Learn `RegQueryValueExA` (fetched 2026-10-19, ms.date
  2018-12-05): six arguments `(hKey, lpValueName, lpReserved, lpType, lpData,
  lpcbData)`; "If lpValueName is NULL or an empty string, "", the function
  retrieves the type and data for the key's unnamed or default value"; "If
  lpValueName specifies a value that is not in the registry, the function
  returns ERROR_FILE_NOT_FOUND"; `lpReserved` "is reserved and must be NULL";
  `lpType` "can be NULL if the type code is not required"; `lpData` "can be
  NULL if the data is not required"; `lpcbData` "can be NULL only if lpData is
  NULL"; "If the buffer specified by lpData parameter is not large enough to
  hold the data, the function returns ERROR_MORE_DATA and stores the required
  buffer size in the variable pointed to by lpcbData"; "If lpData is NULL, and
  lpcbData is non-NULL, the function returns ERROR_SUCCESS and stores the size
  of the data, in bytes, in the variable pointed to by lpcbData"; for REG_SZ
  "this size includes any terminating null character"; the documented warning
  states a returned REG_SZ is NOT guaranteed to be null-terminated.
- Microsoft Learn `RegCloseKey` (fetched 2026-10-19, ms.date 2018-12-05):
  one argument `(hKey)`; `Advapi32.dll`; success is `ERROR_SUCCESS`, failure a
  WinError.h code; "The handle for a specified key should not be used after it
  has been closed, because it will no longer be valid."
- WinError numeric values: `ERROR_SUCCESS` 0, `ERROR_FILE_NOT_FOUND` 2,
  `ERROR_INVALID_HANDLE` 6, `ERROR_INVALID_PARAMETER` 87 reused from the
  R-014/R-015 source set; `ERROR_MORE_DATA` 234 and
  `ERROR_TOO_MANY_OPEN_FILES` 4 also from WinError.h (used by our contracts).

**Claim.** The config store can back a read-only registry view: `HKLM`
(`0x80000002`) is the only backed root; `RegOpenKeyExA` opens a key whose
"children" exist in the store (`prefix/...` entries), an empty/NULL subkey of
`HKLM` returns the same handle per the fetched statement, and each other open
uses one of four kernel slots with synthetic handles `0x1000 + slot`. Key
existence and value lookup are ASCII case-insensitive, matching "key names are
not case sensitive". `RegQueryValueExA` treats the joined `prefix/name` as a
store key, models REG_SZ values as `bytes + NUL`, returns the size (including
the NUL) for a NULL-data query and `ERROR_MORE_DATA` plus the required size for
a short buffer. Documented argument errors return the WinError values above
without killing the thread; unmapped pointers kill, as everywhere else.
Deviations we state, not hide: `samDesired` is ignored (the store has no
security descriptors; read-only anyway), `REG_OPTION_OPEN_LINK` is rejected
with 87, unnamed/default values are not modeled (NULL/empty value name → 2),
a NULL `lpcbData` returns 87 (the fetched docs permit it only with NULL
`lpData` and do not define the result), values are always NUL-terminated
(stronger than the documented warning), other predefined roots return
`ERROR_FILE_NOT_FOUND` because no per-user/class namespace is backed,
`ERROR_TOO_MANY_OPEN_FILES` (4) is our contract for slot exhaustion with four
slots, and a failed open zeroes `*phkResult` (project contract; Windows leaves
it undefined). Confidence: high for the argument shapes, documented error
codes, handle and buffer-size statements; medium for the slot-count and
zeroing choices, which our own tests define.

**Experiment.** `scripts/test_registry_qemu.sh` booted
`artifacts/project-os-registry.iso` (BIOS, QEMU 11.1.0) and `pereg.exe`
exercised the surface through genuine `advapi32.dll` raw-dylib imports: bad
handle → 6, nonzero `ulOptions` → 87, missing key → 2 with `*phkResult`
zeroed, opens of `System`/`display` → handles 4096–4099, size query → 11,
short buffer → 234 + required 11 with the buffer untouched, full reads →
`PROJECT_OS\0` (11 B) and `classic\0` (8 B via the root with the path-shaped
name `Display/Theme`), NULL/empty subkey of HKLM → HKLM itself, non-NULL
`lpReserved` and NULL `lpcbData` → 87, fifth concurrent open → 4, close once →
0, double close → 6, slot reuse → 4096 again, summary write 33 B. The
transcript is `artifacts/qemu-registry-session.log` (10,420 bytes, sha256
`021807014f814d557227c3774d7231a3d47386ec33b023f843b72d736a8e5945`); all phase
counter assertions (open +10, query +7, close +7, WriteFile +1, console
writes +1 / 33 bytes, exit 0, no user faults) passed and
`scripts/test_all.sh` reported `PASS=24 FAIL=0 SKIP=4` at build id
`0.1.0-b1b8542c`.

**Conclusion.** All fetched statements the claim relies on were reproduced:
the five- and six-argument shapes, `ERROR_FILE_NOT_FOUND` for a value not in
the registry, `ERROR_MORE_DATA` plus the required size (including the
terminating NUL) for a short buffer, the NULL-data size query, the
`lpReserved`/`lpcbData` rules, case-insensitive key names, the HKLM
refresh-same-handle behavior, and close-once handle invalidation. The
contract choices our own tests define (four slots → `ERROR_TOO_MANY_OPEN_FILES`
4, zeroed `*phkResult` on failure, always-NUL-terminated values,
`REG_OPTION_OPEN_LINK` → 87, unnamed/default values → 2) held exactly. No
statement about a real Windows registry beyond the fetched ones is claimed.

## R-018 — Registry write surface: `RegCreateKeyExA`/`RegSetValueExA`/`RegDeleteValueA` over the config store

**Question.** What are the documented contracts of the three mutating
`advapi32.dll` registry functions — argument shapes and order, reserved and
ignorable parameters, disposition semantics, value-size and null-termination
rules — and can the Phase 8 config store back a registry-shaped write
surface for genuine PE32+ importers without over-claiming Windows registry
behavior? (Stage 3's R-017 covers the read trio; this stage adds writes.)

**Sources.**
- Microsoft Learn `RegCreateKeyExA` (fetched 2026-10-19, ms.date
  2018-12-05): nine arguments `(hKey, lpSubKey, Reserved, lpClass,
  dwOptions, samDesired, lpSecurityAttributes, phkResult, lpdwDisposition)`;
  exported by `Advapi32.dll`; "If the key already exists, the function opens
  it"; "If lpSubKey is a pointer to an empty string, phkResult receives a new
  handle to the key specified by hKey"; "This parameter cannot be NULL";
  "Reserved ... must be zero"; `lpClass` "can be NULL"
  and "This parameter may be ignored" and "can be NULL"; `dwOptions`:
  `REG_OPTION_BACKUP_RESTORE` (0x00000004), `REG_OPTION_CREATE_LINK`
  (0x00000002), `REG_OPTION_NON_VOLATILE` (0x00000000), `REG_OPTION_VOLATILE`
  (0x00000001); `samDesired` "is checked against the security descriptor of
  the registry key, not the access mask specified when the handle was
  obtained"; "If this parameter is NULL, the key will not be inherited";
  `phkResult` "receives a handle to the opened or created key"; "If the
  function fails, the handle is zero"; `lpdwDisposition` "can be NULL" and
  receives `REG_CREATED_NEW_KEY` (0x00000001) or
  `REG_OPENED_EXISTING_KEY` (0x00000002); remarks: "The key that the
  RegCreateKeyEx function creates has no values; value names are added to the
  key using the RegSetValueEx function"; "An application cannot create a key
  that is a direct child of HKEY_USERS or HKEY_LOCAL_MACHINE."
- Microsoft Learn `RegSetValueExA` (fetched 2026-10-19, ms.date 2018-12-05):
  six arguments `(hKey, lpValueName, Reserved, dwType, lpData, cbData)`;
  "If this parameter is NULL or an empty string, "", the function retrieves
  the type and data for the key's unnamed or default value"; "Reserved ...
  must be zero"; "For string-based types, such as REG_SZ, the string must be
  null-terminated"; `lpData` "can be NULL if cbData is zero" (a null value);
  "If data is of type REG_SZ ... then cbData must include the size of the
  terminating null character".
- Microsoft Learn `RegDeleteValueA` (fetched 2026-10-19, ms.date
  2018-12-05): two arguments `(hKey, lpValueName)`; "If this parameter is
  NULL or an empty string, "", the function removes the unnamed or default
  value"; "Value names are not case sensitive"; the page states failure
  returns a nonzero error code but does not enumerate a missing-value code.
- WinError numeric values: `ERROR_SUCCESS` 0, `ERROR_FILE_NOT_FOUND` 2,
  `ERROR_TOO_MANY_OPEN_FILES` 4, `ERROR_INVALID_HANDLE` 6,
  `ERROR_NOT_ENOUGH_MEMORY` 8, `ERROR_INVALID_PARAMETER` 87 from the
  R-014/R-015/R-017 source set (8 additionally confirmed on the fetched
  `RegSetValueExA`/`RegCreateKeyExA` error tables as
  `ERROR_NOT_ENOUGH_MEMORY`).

**Claim.** The config store can back a registry-shaped write surface:
`RegCreateKeyExA` maps its nine arguments through the first nine-argument
stub (`r10`–`r14` forwarded by the entry stub; `isr_common` already saves
them); `reserved != 0` and `dwOptions != 0` → 87; NULL `lpSubKey` is treated
as empty (a documented deviation: the fetched page says the parameter
"cannot be NULL"; we keep stage 3's NULL-is-empty rule and state it), a
non-empty subkey must join to a valid store path (87 otherwise) and exists
iff the store has children under that prefix (an empty subkey instead
returns a new slot handle to the same key per the fetched statement — a
newer handle, unlike stage 3's `RegOpenKeyExA` refresh — and reports
`REG_OPENED_EXISTING_KEY`);
`phkResult` is validated and zeroed on failure (project contract; the
fetched page says only "If the function fails, the handle is zero");
`lpdwDisposition` is optional and receives 1 (no children before the call)
or 2 (children existed); `lpClass` and `lpSecurityAttributes` are ignored
and never dereferenced (the fetched page says `lpClass` may be ignored);
`samDesired` is ignored because creation access is documented as checked
against the key's security descriptor, which we do not have; the fetched
direct-HKLM-child prohibition is not enforced (no namespace policy).
`RegSetValueExA` maps through the existing `Args6` shape: NULL/empty name
(default value) → 87 because default values are not modeled; `reserved != 0`
→ 87; only `REG_SZ` (1) is accepted; NULL `lpData` → 87 (null values not
modeled); `cbData` must be 1..=129 with a terminating NUL and a
printable-ASCII payload ≤128 bytes (the store's documented cap); the joined
entry is written and `config::save()` persists it. `RegDeleteValueA` maps
through `Args4`: NULL/empty name → 87; missing value → 2 (our contract,
consistent with the fetched `RegQueryValueExA` missing-value statement; the
`RegDeleteValueA` page does not enumerate a code — confidence medium, noted);
case-insensitive matching per the fetched "value names are not case
sensitive". Keys are children-only: a created key with no values leaves no
trace and a key whose last value is deleted ceases to exist (deviation from
the fetched "creates ... no values" state, which POCF cannot represent; it
is stated, not hidden). Store-full/save failures map to 8
(`ERROR_NOT_ENOUGH_MEMORY`). Deviations we state: `samDesired` ignored,
default values rejected, null values rejected, other predefined roots → 2,
typed values beyond REG_SZ rejected, no namespace rules, no empty keys, no
`RegDeleteKey`. Confidence: high for argument shapes, reserved/ignore rules,
disposition values, null-termination/cbData rules and error codes; medium
for the missing-value delete code (2) and the resource-exhaustion mapping
(8), which our own contracts define.

**Experiment.** PASSED (QEMU 11.1.0, 2026-10-19). `scripts/test_registry_write_qemu.sh`
booted `artifacts/project-os-registry-write.iso` and `peregw.exe` exercised the
surface through genuine `advapi32.dll` raw-dylib imports: first create of
`Session/Current` → handle 4096 `created`; `RegSetValueExA` `Flag` set (then
read back `enabled`), updated to `on`, size query 3 bytes; `REG_DWORD` type
→ 87; NULL data → 87; bad handle → 6; invalid joined name → 87; NULL/empty
value name → 87; `RegDeleteValueA` missing → 2, bad handle → 6, success → 0
with a follow-up query now → 2 (key vanished with its last value);
re-create → 4097 `created` (no children anymore, slot reused); write of
`Persisted` = `yes`; both handles closed; summary written (39 bytes); exit 0.
The phase counter assertions (create +3, set +8, delete +3, query +3, close
+3, `WriteFile` +1, console writes +1 / 39 bytes, no user faults) and the
`user.rs` persist/read-back/decode/restore assertions passed. The first
fixture order (repeat create before the first write) correctly reported
`created` under the children-only model and was fixed by reordering the
fixture; the re-run matched every pinned line. Transcript:
`artifacts/qemu-registry-write-session.log` (10,407 bytes, sha256
`4c03490bed58031e65eea434a5ba4b0cb2698d49919fb1726af68535c7a16077`);
`scripts/test_all.sh` reported `PASS=25 FAIL=0 SKIP=4` at build id
`0.1.0-fe3c278e`.

**Conclusion.** All fetched statements the transcript exercises were
reproduced: the nine-argument create shape (seventh–ninth arguments through
`r12`–`r14`), the disposition pair (1 with no children, 2 once a value
existed, 1 again after the last value was deleted), the children-only
namespace, REG_SZ set/update semantics, the missing-value delete → 2
(confidence medium, our contract), bad-handle → 6 and invalid-argument → 87
paths, and persistence through the same `/etc/config` codec the checkpoint
carries. The contract choices our own tests define (NULL subkey = empty key,
zeroed `*phkResult` on failure, default/null values rejected, `samDesired`
and namespace rules ignored, store-full/save failure → 8) held exactly as
written. Create paths not exercised by the fixture (nonzero `Reserved`/
`dwOptions`) rest on the fetched statements only. No statement about a real
Windows registry beyond the fetched ones is claimed.

## R-019 — Registry enumeration and key deletion (`RegEnumKeyExA`, `RegEnumValueA`, `RegDeleteKeyA`)

**Question.** Can the flattened Phase 8 config store back subkey/value
enumeration and the documented non-recursive deletion rule with the fetched
Windows argument shapes and error codes — and what deviation does the
flattened namespace force?

**Sources** (fetched 2026-10-20).

- Microsoft Learn `RegEnumKeyExA` (ms.date 2018-12-05): signature
  `(hKey, dwIndex, lpName, lpcchName, lpReserved, lpClass, lpcchClass,
  lpftLastWriteTime)`; "Enumerates the subkeys of the specified open registry
  key. The function retrieves information about one subkey each time it is
  called."; "Because subkeys are not ordered, any new subkey will have an
  arbitrary index."; `lpName` receives "the name of the subkey, including the
  terminating null character ... only the name of the subkey, not the full key
  hierarchy"; "If the function fails, no information is copied to this
  buffer."; `lpcchName`: "specifies the size of the buffer ... in characters.
  This size should include the terminating null character. If the function
  succeeds, the variable ... contains the number of characters stored in the
  buffer, not including the terminating null character."; `lpReserved` "is
  reserved and must be NULL"; `lpClass` may be NULL; `lpcchClass` "can be NULL
  only if lpClass is NULL"; `lpftLastWriteTime` optional; returns
  `ERROR_NO_MORE_ITEMS` when no more subkeys and `ERROR_MORE_DATA` when
  `lpName` is too small; the handle needs `KEY_ENUMERATE_SUB_KEYS`; the page
  warns against changing the key during enumeration.
- Microsoft Learn `RegEnumValueA` (ms.date 2018-12-05): signature
  `(hKey, dwIndex, lpValueName, lpcchValueName, lpReserved, lpType, lpData,
  lpcbData)`; "copies one indexed value name and data block"; values are not
  ordered; `lpValueName` receives "the name of the value as a null-terminated
  string"; `lpcchValueName` in: buffer size in characters, out: "the number of
  characters stored in the buffer, not including the terminating null
  character", and on a too-small buffer "returns ERROR_MORE_DATA and the
  buffer size ... is not changed. In this case, the contents of
  lpcchValueName are undefined." (the page names `lpValueName` in that
  sentence; recorded as fetched); `lpReserved` "must be NULL"; `lpType`
  optional; `lpData` "can be NULL if the data is not required"; "If lpData is
  NULL and lpcbData is non-NULL, the function stores the size of the data, in
  bytes, in the variable pointed to by lpcbData."; `lpcbData` "can be NULL
  only if lpData is NULL"; for REG_SZ the size "includes any terminating null
  character"; a too-small data buffer returns `ERROR_MORE_DATA` and stores the
  required size in `*lpcbData`; `ERROR_NO_MORE_ITEMS` at the end.
- Microsoft Learn `RegDeleteKeyA` (ms.date 2018-12-05): "Deletes a subkey and
  its values. Note that key names are not case sensitive."; `lpSubKey` "must
  be a subkey of the key that hKey identifies, but it cannot have subkeys.
  This parameter cannot be NULL."; "The access rights of this key do not
  affect the delete operation."; "A deleted key is not removed until the last
  handle to it is closed."; "The subkey to be deleted must not have subkeys.
  To delete a key and all its subkeys, you need to enumerate the subkeys and
  delete them individually."; the page does not enumerate specific error
  codes.
- WinError numeric values: `ERROR_ACCESS_DENIED` 5,
  `ERROR_MORE_DATA` 234 and `ERROR_NO_MORE_ITEMS` 259 (5 and 259 newly used
  here; 234 already used in R-017).

**Claim.** Enumeration maps onto the stage-3/4 flattened model: the children
of a key prefix are the distinct first path components of tails that contain
a `/`; the values of a prefix are its direct entries (full remaining tail as
the value name); order is store insertion order, which is a documented
deviation from the fetched "arbitrary index" (we need deterministic tests).
The same entry can be visible as a value and as a child key when the value
name contains `/` — impossible in Windows' separate namespaces, so it is a
stated consequence of the flattened store. `RegDeleteKeyA` implements the
fetched non-recursive rule: it removes a key's direct entries only, and
reports `ERROR_ACCESS_DENIED` (5) when any child key exists (the page says
"must not have subkeys" but does not name the code; 5 is our contract,
confidence medium). An empty/NULL subkey is 87 (a subkey cannot be NULL;
`HKLM` itself is not deletable); invalid joined paths are 87; a missing key is
2; bad handles 6. The fetch's "not removed until the last handle is closed"
is **not** modelled: entries disappear immediately and open handles become
stale (queries 2, enumeration 259) — stated deviation. `RegEnumKeyExA`
ignores `lpClass`/`lpcchClass`/`lpftLastWriteTime` (never dereferenced; the
page defines class/time outputs we do not have) and rejects nonzero
`lpReserved` with 87; `lpcchName` is a mandatory in/out (NULL or size 0 →
87); too-small names return 234 with the required size including the NUL in
`*lpcchName` (our contract; the page leaves the failure size undefined) and
success writes the NUL-terminated name with `*lpcchName = len` excluding it
(fetched). `RegEnumValueA` mirrors that for the name; `lpType` receives
REG_SZ (1) when non-NULL; `lpData == NULL` with `lpcbData != NULL` is a
supported size query (`*lpcbData = len + 1`); NULL `lpData` with NULL
`lpcbData` returns name/type only; data too small returns 234 with the
required size (fetched); success writes the payload plus NUL and sets
`*lpcbData = len + 1`. End-of-enumeration is 259. Both services use new
eight-argument stubs (`Args8`: `r10`–`r13` forwarded; 27 stub bytes ≤ the
48-byte fixed stride) and `RegDeleteKeyA` the new two-argument shape
(`Args2`). Confidence: high for argument shapes, reserved rules, name
success semantics, the 234/259 conditions and the deletion precondition;
medium for 5 as the has-subkeys code and our required-size-on-234 contracts.

**Experiment.** PENDING. Planned: `scripts/test_registry_enum_qemu.sh` boots
`artifacts/project-os-registry-enum.iso` with `peregen.exe` exercising create/
set/enumerate/delete (including the 5-on-subkeys path, slot reuse, stale
handles, 234/259 paths) and clean-up back to the seeded defaults;
`scripts/test_all.sh` expected to report `PASS=26 FAIL=0 SKIP=4`.

**Conclusion.** PENDING until the harness run is recorded in
`docs/WIN32_REGISTRY_ENUM.md` §14.

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
