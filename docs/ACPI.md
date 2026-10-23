# ACPI MADT discovery for the IOAPIC and ISA routes

Status: **implemented and verified on QEMU 11.1.0 and VirtualBox 7.2.20,
BIOS, 2026-10-23.** This document is the contract for Phase 9 stage 9. It
describes only the firmware discovery and routing change; PCI `_PRT` and AML
remain unimplemented.

## 1. Goal and non-goals

The stage replaces the fixed stage-8 IOAPIC base and PIT/COM1 GSI assumptions
with values discovered from ACPI firmware:

- obtain the RSDP through Limine;
- validate the RSDP, root table and every consumed table against the firmware
  memory map;
- walk an XSDT or RSDT and find exactly one MADT;
- select an IOAPIC that covers both the PIT and COM1 routes;
- resolve the two legacy ISA interrupt-source overrides;
- program the discovered IOAPIC base and routes, retaining the existing PIC
  fallback if discovery or controller bring-up fails.

Deliberately not in this stage:

- no AML interpreter and no PCI `_PRT` evaluation;
- no PCI INTx GSI routing; the e1000 remains on legacy 8259 INTx;
- no MSI/MSI-X, APIC timer, HPET, SMP or IRQ model change;
- no ACPI table reclaim, namespace walk, power management, sleep states or
  general ACPI driver;
- no physical-hardware validation.

QEMU q35 exposes `_PRT` as an AML method returning packages with `GSIG` links.
That is not the same thing as PCI config-space `INTERRUPT_LINE`; the e1000's
q35 line 11 is not its GSI22 route. Without an AML evaluator the NIC must stay
on the PIC path.

## 2. Primary sources

- ACPI Specification, version 6.6, sections on the RSDP, RSDT/XSDT, MADT
  (`APIC`) and MADT interrupt-source overrides:
  <https://uefi.org/specifications>.
- Intel 64 and IA-32 Architectures Software Developer's Manual, Volume 3A,
  local APIC and I/O APIC chapters (APIC MMIO, LAPIC EOI, IOAPIC indirect
  registers and redirection-entry layout):
  <https://www.intel.com/content/www/us/en/developer/articles/technical/intel-sdm.html>.
- Limine protocol, base revision 6, RSDP feature and response:
  <https://github.com/limine-bootloader/limine/blob/v12.x/PROTOCOL.md>.
- QEMU 11.1.0 and VirtualBox 7.2.20 were exercised as guests. Their logs are
  evidence for the supported profiles, not specifications. No Linux, Wine or
  ReactOS implementation code was read or copied.

The research record is R-028 in `docs/RESEARCH.md`; the decision is ADR-0035 in
`docs/DECISIONS.md`.

## 3. Discovery flow

`kernel/src/acpi.rs::init(hhdm, regions)` performs the following:

1. Read Limine's RSDP response. A null response is `MissingRsdp`.
2. Convert the HHDM virtual address to a physical address by subtracting the
   HHDM offset. An underflow or physical address zero is rejected.
3. Verify that the first 20 bytes lie in a memory-map region of type
   `AcpiReclaimable`, `AcpiNvs`, or `ReservedMapped`, then make at most 4096
   bytes available to the parser from that region.
4. Parse and validate the RSDP at the reported address. The parser accepts
   revision 0/1 (20-byte) and revision 2+ (36-byte) RSDPs, checks the base
   checksum, and for v2 checks the extended checksum and declared length.
5. Prefer a nonzero XSDT address; otherwise use the RSDT address. A root table
   with no address is rejected.
6. Read the root table's declared length from its 8-byte prefix, validate the
   complete table range, parse and checksum its 36-byte SDT header, then walk
   its 8-byte XSDT or 4-byte RSDT entries.
7. Read each pointed-to table's declared length, validate the complete range,
   checksum its header, and remember the `APIC` table. Zero MADTs is
   `NoMadt`; two or more is `MultipleMadt`.
8. Parse the MADT, resolve the PIT and COM1 routes, select one covering
   IOAPIC, publish an `AcpiSummary`, and return it.

The caller logs the summary and uses the selected base/routes. The `INFO`
spinlock holds the last successful summary for diagnostics and the boot
self-test; it is `None` after any discovery failure.

### HHDM and lifetime invariants

Limine maps the RSDP as an HHDM address, not a physical address. The kernel
never dereferences a firmware physical address directly. `table_bytes` checks
the complete byte range against an allowed Limine region and only then forms a
slice at `hhdm + physical`. The slice is treated as kernel-lifetime because
this kernel does not reclaim ACPI memory. If ACPI reclaim is implemented later,
the tables must be copied or the borrowed lifetime must be narrowed before the
reclaim. Every addition/subtraction in this path is checked; a range that
crosses the end of its region is rejected rather than read.

Only ACPI reclaimable, ACPI NVS and reserved-mapped regions are accepted.
Usable RAM and bootloader-reclaimable memory are not trusted as table storage.

## 4. Parser validation

`os_core/src/acpi.rs` is allocation-free, pointer-free and host-tested. It
validates every structure before exposing it:

- RSDP signatures, minimum length, v2 length, base checksum and extended
  checksum;
- SDT header length (at least 36), full-table checksum, and declared length
  fitting the supplied slice;
- RSDT/XSDT signature and payload divisibility by the entry size;
- MADT signature, minimum 44-byte length, and payload entries that fit;
- MADT entry header lengths (at least 2) and per-record minimum lengths;
- IOAPIC address nonzero and 4 KiB aligned, no duplicate IOAPIC address;
- interrupt-source override flags: reserved polarity or trigger value 2 is
  rejected for the whole MADT.

Capacity is deliberately bounded in kernel memory:

| Limit | Value | Behaviour |
|---|---|---|
| `MAX_IOAPICS` | 4 | a fifth IOAPIC is rejected |
| `MAX_ISA_OVERRIDES` | 16 | a seventeenth bus-0 override is rejected |
| `RSDP_PROBE_LEN` | 4096 bytes | maximum prefix made available to the RSDP parser |

The parser consumes only IOAPIC records (type 1) and interrupt-source
overrides (type 2). Other MADT records, including local APIC and interrupt
source records, are skipped by length. Only bus-0 overrides are stored; other
buses are ignored. A duplicate bus-0 source is rejected.

`read_sdt_length` exists because the kernel needs the declared table length
before it can validate the full range. It requires at least 8 bytes and a
declared length of at least 36; the complete header is validated separately
once the full table is available.

## 5. IOAPIC and route selection

Both routes must fit one selected IOAPIC's 256-input interface. The kernel
chooses the first MADT IOAPIC with a nonzero address where:

```text
gsi >= ioapic.gsi_base && gsi - ioapic.gsi_base < 256
```

for both the timer and console GSIs. A GSI above 255 is rejected as
`RouteUnsupported`. If no IOAPIC covers both, discovery fails with `NoIoApic`
and the fixed stage-8 configuration is used.

For each legacy source, a bus-0 override replaces the default GSI. With no
override, the local defaults are PIT IRQ0 -> GSI2 edge/active-high and COM1
IRQ4 -> GSI4 level/active-high.

Override flags are handled field by field, not all-or-nothing:

- polarity bits `1:0`: `0` means "conforming" and the local default is
  active-high, `1` means active-high, `3` means active-low; `2` is reserved and
  rejected;
- trigger bits `3:2`: `0` means "conforming" and the local default is used
  (PIT edge, COM1 level), `1` means edge, `3` means level; `2` is reserved and
  rejected.

This is deliberately more tolerant than `IsaOverride::conforms()`, which
requires all four flag bits to be zero. A firmware entry that specifies only
one attribute still supplies the other default, so the kernel does not discard
a usable GSI merely because one field means "conforming".

The selected IOAPIC's MADT ID is reported, but routing destinations use the
boot CPU's physical local-APIC ID. The IOAPIC ID register is a separate value;
VirtualBox's MADT reports IOAPIC ID 1 while its hardware ID register reads 0.

## 6. Kernel integration and fallback

`kernel/src/arch/x86_64/ioapic.rs` now takes the physical base from the
caller. It rejects a base that is not 4 KiB aligned and a 4 KiB MMIO window
that crosses a 2 MiB page boundary. The mapping uses the existing narrow
uncached mapper. All redirection entries are masked before any route is
programmed.

`kernel/src/irq.rs` owns the kernel-side `LegacyRoute { gsi, trigger,
polarity }`. `init_ioapic(base, lapic, timer_route, console_route)`:

- rejects the PIC marker (`0xff`) for an IOAPIC route;
- rejects a shared GSI between the timer and console;
- programs both routes to the boot CPU and records `timer_gsi`/`console_gsi`
  in `IoApicInfo`;
- masks every programmed GSI when returning to the PIC.

The boot sequence has three distinct outcomes:

1. **ACPI success and IOAPIC success:** discovered base/routes are used; the
   kernel logs the summary and the ACPI self-test.
2. **ACPI failure (any `acpi::init` error):** log
   `acpi: unavailable (<reason>); using fixed IOAPIC/ISA routes`, then use
   base `0xfec00000`, PIT GSI2 edge/high and COM1 GSI4 level/high.
3. **ACPI success but controller bring-up failure:** log
   `ioapic: unavailable (<reason>); using 8259` and unmask PIC IRQ0/IRQ4.
   This is not a mixed configuration: the kernel does not program guessed
   discovered GSIs after a controller failure.

A timer timeout after IOAPIC bring-up also calls `irq::fallback_to_pic()`,
masks the programmed GSIs and returns to PIC IRQ0/IRQ4. That path is inherited
from stage 8 and was not forced in the final VM runs.

The `,acpi` boot marker is appended only when the IOAPIC started and a
validated ACPI summary exists. It is appended after the optional `,net`
component. The self-test compares the discovered timer/console GSIs with the
values the IOAPIC layer actually programmed and is fatal on mismatch. It does
not currently compare trigger or polarity.

## 7. Diagnostics

Successful QEMU q35 boot (serial):

```text
acpi: RSDP revision 0, RSDT with 5 table(s)
acpi: MADT revision 3, local APIC 0xfee00000, 1 IOAPIC(s), 5 ISA override(s)
acpi: IOAPIC 0 at 0xfec00000, GSI base 0
acpi: PIT IRQ0 -> GSI2 (edge, active-high); COM1 IRQ4 -> GSI4 (level, active-high)
ioapic: version 0x20, 24 pins, id 0; timer GSI2, console GSI4
acpi selftest: passed (timer GSI2, console GSI4)
```

Failure diagnostics are single-line and bounded:

```text
acpi: unavailable (no MADT table); using fixed IOAPIC/ISA routes
ioapic: unavailable (IOAPIC base is not page aligned); using 8259
```

The ACPI and IOAPIC logs are separate: ACPI reports the firmware tables, while
the IOAPIC line reports the controller register ID, version and pin count.

## 8. Evidence

Host parsing tests: 15 ACPI tests in the 312-test `os_core` suite, including
RSDP v1/v2, invalid checksums/lengths, truncated entries, duplicate and
misaligned IOAPICs, capacity limits, reserved flags, ignored non-bus-0
overrides, XSDT/RSDT address parsing, and `read_sdt_length`.

QEMU 11.1.0, q35, BIOS, 512 MiB, QEMU serial log:

```text
acpi: RSDP revision 0, RSDT with 5 table(s)
acpi: MADT revision 3, local APIC 0xfee00000, 1 IOAPIC(s), 5 ISA override(s)
acpi: IOAPIC 0 at 0xfec00000, GSI base 0
acpi: PIT IRQ0 -> GSI2 (edge, active-high); COM1 IRQ4 -> GSI4 (level, active-high)
ioapic: version 0x20, 24 pins, id 0; timer GSI2, console GSI4
acpi selftest: passed (timer GSI2, console GSI4)
pit: 100 Hz, IOAPIC GSI2
console: COM1 receive interrupt enabled (IOAPIC GSI4)
```

VirtualBox 7.2.20, BIOS, 512 MiB:

```text
acpi: RSDP revision 2, XSDT with 3 table(s)
acpi: MADT revision 2, local APIC 0xfee00000, 1 IOAPIC(s), 2 ISA override(s)
acpi: IOAPIC 1 at 0xfec00000, GSI base 0
acpi: PIT IRQ0 -> GSI2 (edge, active-high); COM1 IRQ4 -> GSI4 (level, active-high)
ioapic: version 0x20, 24 pins, id 0; timer GSI2, console GSI4
acpi selftest: passed (timer GSI2, console GSI4)
```

The two firmware profiles differ in root-table kind (RSDT versus XSDT),
revision, table/override counts, and MADT IOAPIC ID, but select the same base
and routes. This exercises both root encodings and both RSDP revisions.

Harness coverage:

- `scripts/test_boot_qemu.sh --require-acpi` asserts the RSDP/root line, MADT
  line, IOAPIC line, both ISA routes, the self-test, the `,acpi` marker, and
  the absence of `acpi: unavailable`.
- `scripts/test_virtualbox.sh --require-ioapic --require-acpi` applies the
  same ACPI assertions in the isolated VirtualBox VM.
- `scripts/test_all.sh` runs the new `boot-qemu-acpi` step and the VirtualBox
  step with both require flags.

Full suite result: `PASS=31 FAIL=0 SKIP=2`; unit suite `312/312`; QEMU BIOS
256 MiB/512 MiB/1 GiB, IOAPIC, ACPI, PE/ELF, memory, VFS, graphics, network,
persistence, PCI and driver tests pass. The two SKIPs remain UEFI (no OVMF)
and the virtual-GPU test (not implemented).

Bare QEMU boot logs report `net selftest: skipped (no NIC)` because the boot
harness does not attach a NIC; their marker therefore has no `,net`. The
network path is covered by the separate `scripts/test_net_qemu.sh` step, which
passes and requires `,net`.

## 9. Limits and non-claims

- Only the first MADT and the first covering IOAPIC are used. The kernel does
  not build a full interrupt-controller topology, route PCI interrupts or
  handle IOAPIC hotplug.
- Only bus-0 ISA overrides are stored. PCI `_PRT`, AML, interrupt links and
  GSIs above 255 are unsupported.
- ACPI is not reclaimed; tables are borrowed for the kernel lifetime. There is
  no ACPI namespace, power management, sleep/wake or general ACPI driver.
- The IOAPIC is still programmed only for PIT and COM1. The e1000 remains on
  legacy PIC INTx; MSI/MSI-X and the APIC timer are not implemented.
- The LAPIC is still left with LINT0 ExtINT for the PIC path. The MADT PCAT
  compatibility flag and 8259 presence are not used to choose a different
  interrupt model.
- Only QEMU q35 and VirtualBox BIOS were exercised. UEFI boot is blocked by
  the missing OVMF firmware; physical hardware, SMP and PCI GSI routing are
  untested.
- The `acpi selftest` compares GSIs only. It does not independently verify the
  redirection entry's trigger, polarity, destination or vector.
- The fallback paths are implemented but were not forced in the final VM runs;
  the successful runs use discovered ACPI values.

## 10. Files

| File | Role |
|---|---|
| `os_core/src/acpi.rs` | allocation-free RSDP/RSDT/XSDT/MADT parser and 15 tests |
| `os_core/src/lib.rs` | exports `acpi` |
| `kernel/src/acpi.rs` | Limine RSDP, memory-map validation, discovery, route policy, summary |
| `kernel/src/limine.rs` | RSDP request/response binding |
| `kernel/src/arch/x86_64/ioapic.rs` | dynamic base mapping/probe and redirection access |
| `kernel/src/irq.rs` | `LegacyRoute`, dynamic route programming and PIC fallback |
| `kernel/src/main.rs` | ACPI logging, route selection, self-test, `,acpi` marker |
| `scripts/test_boot_qemu.sh` | `--require-acpi` assertions |
| `scripts/test_virtualbox.sh` | `--require-acpi` assertions |
| `scripts/test_all.sh` | `boot-qemu-acpi` step and VirtualBox flags |
| `docs/RESEARCH.md` R-028 | sources, claim, experiment and conclusion |
| `docs/DECISIONS.md` ADR-0035 | design decision and consequences |
