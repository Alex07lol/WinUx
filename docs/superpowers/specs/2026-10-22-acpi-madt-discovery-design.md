# ACPI MADT discovery for IOAPIC and ISA routes

Status: proposed design, Phase 9 stage 9
Date: 2026-10-22
Scope: ACPI discovery only; PCI `_PRT`, MSI/MSI-X, APIC timer and SMP are
explicitly out of scope.

## 1. Outcome

The kernel must stop assuming that the first IOAPIC lives at `0xfec00000` and
that the PIT and COM1 always use GSIs 2 and 4. On firmware that provides ACPI,
the kernel will:

- obtain the RSDP through Limine;
- validate the RSDP, XSDT/RSDT and MADT instead of trusting bootloader or
  firmware pointers;
- discover the IOAPIC base and GSI base;
- apply ISA interrupt-source overrides for IRQ0 (PIT) and IRQ4 (COM1);
- keep the stage-8 fixed IOAPIC base and GSI2/GSI4 path as a logged fallback if
  discovery fails.

Success is a QEMU and VirtualBox BIOS boot that reaches
`PROJECT_OS_BOOT_SUCCESS`, uses the ACPI-derived IOAPIC and routes, emits the
ACPI diagnostics, appends `,acpi` to the boot checks, receives PIT ticks with no
PIC fallback, and keeps COM1 receive interrupts working. The existing suite must
remain green.

## 2. Current state and why this stage exists

Phase 9 stage 8 routes the PIT and COM1 through the IOAPIC and uses the local
APIC for end-of-interrupt. That work is verified, but it hard-codes:

- IOAPIC physical base `0xfec00000`;
- PIT GSI 2 and COM1 GSI 4;
- edge/high for the PIT and level/high for COM1.

The code says so directly in `kernel/src/arch/x86_64/ioapic.rs` and
`kernel/src/irq.rs`. QEMU q35 and VirtualBox happen to match those assumptions,
but they are not derived from the platform. ACPI MADT is the authoritative
source for the IOAPIC address, its GSI base and ISA interrupt-source overrides.

The e1000 remains on legacy PIC INTx. QEMU q35's `_PRT` maps the e1000's
slot-2 INTA to GSI22, but q35 emits `_PRT` as an AML method returning a package
with `GSIG` links. Parsing it requires an AML evaluator. This stage therefore
does not route PCI interrupts and does not claim GSI22.

## 3. Goals

1. Add a Limine RSDP request and consume the response without assuming the
   address is already a safe physical pointer.
2. Validate ACPI table addresses against the Limine memory map before any table
   byte is read.
3. Parse RSDP, XSDT or RSDT, and MADT with checksums, length and entry-bound
   checks.
4. Discover the first IOAPIC, its MMIO base and its GSI base.
5. Resolve IRQ0 and IRQ4 through MADT interrupt-source overrides when present.
6. Program the discovered IOAPIC base and routes, preserving the stage-8
   fallback when ACPI is absent or rejected.
7. Test malformed inputs on the host and assert the real routes in QEMU and
   VirtualBox.
8. Record the evidence, limits and `_PRT` deferral in project documentation.

## 4. Non-goals

- No AML interpreter and no `_PRT` evaluation.
- No PCI INTx IOAPIC routing and no e1000 GSI22 change.
- No MSI or MSI-X.
- No APIC timer, HPET, SMP, MCFG, DMAR, SRAT or NUMA work.
- No ACPI table reclaiming, no AML namespace, no ACPI power management.
- No attempt to use the MADT local-APIC address in place of the CPUID/xAPIC
  address already used for LINT0 and EOI.
- No physical-hardware claim. Evidence remains QEMU 11.1.0 q35 and VirtualBox
  7.2.20 unless those runs actually succeed.

## 5. Approaches considered

### A. MADT-only discovery with fixed fallback (selected)

Add a bounded ACPI parser, discover the IOAPIC and ISA overrides, and leave PCI
routing for a later stage. This removes the two hard-coded IOAPIC assumptions
without requiring an AML interpreter. The existing stage-8 path remains the
fallback, so a malformed or absent table does not turn a working boot into a
panic.

### B. MADT plus `_PRT` in the same stage

This would add an AML parser/evaluator for the q35 `_PRT` method and route the
e1000 through GSI22. It is substantially larger, needs AML namespace and method
semantics, and is not required to make timer/COM1 discovery correct. It is
deferred.

### C. Keep the fixed IOAPIC path

This is the current state. It avoids new code but leaves the kernel dependent on
two hypervisor coincidences and does not satisfy the Phase 9 acceptance target.
It is not selected.

## 6. Architecture

### 6.1 Components

| Component | Responsibility | Dependencies |
|---|---|---|
| `os_core/src/acpi.rs` | Host-testable RSDP, SDT-list and MADT parsing; checksums, bounds, entries, overrides; no allocation or hardware access | `os_core::ioapic` flag types only |
| `kernel/src/limine.rs` | Declare the Limine RSDP request and response | Limine protocol ABI |
| `kernel/src/acpi.rs` | Convert the Limine HHDM pointer, validate table ranges against the memory map, find the MADT, select the IOAPIC and build the two ISA routes | `os_core::acpi`, `limine`, `memmap`, `sync` |
| `kernel/src/arch/x86_64/ioapic.rs` | Map a caller-supplied physical IOAPIC window, not only `0xfec00000` | paging |
| `kernel/src/irq.rs` | Accept dynamic GSI values and trigger/polarity for the PIT and COM1 | `os_core::ioapic` |
| `kernel/src/main.rs` | Run discovery, log the result or fallback, pass discovered routes to IRQ setup, append `,acpi` only on success | all of the above |
| Boot scripts | Assert ACPI discovery and the unchanged IOAPIC delivery path | serial logs |

### 6.2 Data flow

```text
Limine RSDP response (HHDM virtual)
  -> subtract HHDM offset to physical address
  -> memory-map range validation (ACPI reclaimable / ACPI NVS / reserved mapped)
  -> RSDP signature + checksums + revision
  -> XSDT (preferred) or RSDT
  -> table list with 8-byte or 4-byte physical entries
  -> validate each SDT header and length
  -> first MADT ("APIC")
  -> parse IOAPIC entries and interrupt-source overrides
  -> resolve IRQ0 and IRQ4
  -> select one IOAPIC covering both GSIs
  -> map that physical window through the HHDM
  -> irq::init_ioapic(dynamic base, dynamic routes)
```

`os_core::acpi` returns owned, fixed-size values. It never holds a borrow across
the kernel discovery step and never uses `alloc`.

## 7. Parser contract

### 7.1 RSDP

`parse_rsdp(bytes) -> Result<Rsdp, AcpiError>`:

- require at least 20 bytes and the exact signature `RSD PTR `;
- require the first-20-byte checksum to sum to zero;
- revision is byte 15; RSDT address is the 32-bit value at offset 16;
- for revision 2 or later, require a 36-byte minimum, a length of at least 36,
  the declared length present in the slice, a valid extended checksum, and the
  64-bit XSDT address at offset 24;
- expose `xsdt_address: Option<u64>`; a zero XSDT address is treated as absent
  so the RSDT can be used;
- reject when both root addresses are zero.

### 7.2 Root table list

`parse_table_list(bytes) -> Result<TableList, AcpiError>` accepts only `XSDT`
(8-byte entries) or `RSDT` (4-byte entries):

- validate the common 36-byte SDT header and checksum;
- require `length >= 36` and `length <= bytes.len()`;
- require the entry area to divide exactly by the entry size;
- expose `len()` and `address(index)`;
- cap the count implicitly by table length, with no unbounded scan.

### 7.3 MADT

`parse_madt(bytes) -> Result<Madt, AcpiError>`:

- require the `APIC` signature and a valid SDT header;
- require `length >= 44`;
- read the 32-bit local-APIC address and flags but do not use them for xAPIC
  setup in this stage;
- walk entries from offset 44:
  - every entry must have at least a 2-byte header;
  - every entry length must be at least 2 and must stay inside the table;
  - an entry of type 1 must be at least 12 bytes: ID, 32-bit MMIO address,
    32-bit GSI base;
  - an entry of type 2 must be at least 10 bytes: bus, source IRQ, 32-bit GSI,
    16-bit flags;
  - unknown entry types are skipped after bounds validation;
  - a truncated entry, zero/one-byte length, or out-of-table length is an
    error.
- Store at most 4 IOAPICs and 16 bus-0 ISA overrides. Crossing either cap is an
  error, not silent truncation.
- Reject a zero or non-4-KiB-aligned IOAPIC address, a duplicate IOAPIC address,
  a duplicate bus-0 source override, and reserved polarity/trigger encodings.
- Only bus 0 overrides are stored and participate in ISA route lookup; entries
  for other buses are bounds-checked and otherwise ignored by this stage.

`Madt` provides iterators over the stored entries and `override_for(source)`.
The stored flags preserve the ACPI meaning:

- polarity bits 0..1: `00` conform, `01` active high, `11` active low;
- trigger bits 2..3: `00` conform, `01` edge, `11` level;
- `10` in either field is reserved and rejected.

For a conforming override, the kernel applies the device-specific fallback:
PIT edge/high, COM1 level/high.

## 8. Memory-map validation and safety

The bootloader's RSDP address is an HHDM virtual address for the requested
Limine base revision 6. The kernel must not dereference it until it has:

1. checked `address >= hhdm_offset` and computed `physical = address - hhdm`;
2. found a memory-map region containing the complete table range;
3. required that region to be `AcpiReclaimable`, `AcpiNvs` or
   `ReservedMapped`;
4. checked `hhdm_offset + physical` and `physical + length` for overflow.

The RSDP is probed conservatively: the kernel builds a slice only up to the end
of its containing region, capped at 4096 bytes, and lets the parser validate the
declared length. Root and MADT addresses get the same range check before the
full table is exposed as a byte slice. A table that crosses a region boundary is
rejected.

The only raw-pointer use is `core::slice::from_raw_parts` after those checks.
Each `unsafe` block states that the range is mapped and inside a validated
region. The parser itself contains no unsafe code and cannot read outside the
slice it receives.

ACPI tables remain unavailable to the frame allocator; this stage does not
reclaim or free them.

## 9. Route selection

For IRQ0 (PIT):

- use the bus-0 MADT override if present;
- otherwise retain GSI2, edge, active high (the stage-8 verified route);
- for conforming flags use edge/high.

For IRQ4 (COM1):

- use the bus-0 MADT override if present;
- otherwise retain GSI4, level, active high (the UART's asserted line);
- for conforming flags use level/high.

One IOAPIC must cover both resulting GSIs, and both GSIs must fit the `u8`
index used by the IOAPIC redirection table. If either condition fails, or if the
GSI is outside the probed pin count, IOAPIC bring-up fails and the kernel uses
the existing PIC fallback. The kernel does not silently route a line to a
different GSI.

The MADT's local-APIC address is logged for diagnosis but is not used to replace
`apic::enable_xapic()` or the existing xAPIC page mapping.

## 10. Fallback behavior

- **RSDP missing, table malformed, checksum bad, no MADT, no IOAPIC, or route
  unresolvable:** log one `acpi: unavailable (<reason>)` line and continue with
  the stage-8 fixed base `0xfec00000`, GSI2/GSI4, edge/high for the PIT and
  level/high for COM1.
- **ACPI discovery succeeds but the discovered IOAPIC cannot be mapped or
  probed, or the GSI is outside its pins:** do not mix a discovered route with a
  guessed base. Log `ioapic: unavailable (...)`, keep the 8259 path, and let the
  existing timer fallback logic decide whether the PIC works.
- **ACPI discovery succeeds and the IOAPIC probes:** program the discovered
  entries; on timer timeout, the existing `fallback_to_pic` path masks those
  dynamic GSIs and returns both lines to the 8259.

The fallback is a safety path, not a claim of ACPI success. `,acpi` is appended
only when discovery, route selection, mapping and probing all succeed.

## 11. Diagnostics and boot marker

Successful discovery emits, before the existing `ioapic:` line:

```text
acpi: RSDP revision 2, XSDT with 9 table(s)
acpi: MADT revision 3, 1 IOAPIC, 5 ISA override(s)
acpi: IOAPIC 0 at 0xfec00000, GSI base 0
acpi: PIT IRQ0 -> GSI2 (edge, active-high); COM1 IRQ4 -> GSI4 (level, active-high)
acpi selftest: passed (checksums, table bounds, MADT entries, ISA routes)
```

The table and override counts are diagnostic and may differ between firmware.
The `PIT`/`COM1` line is the route assertion. On QEMU and VirtualBox the values
are expected to remain GSI2 and GSI4.

The existing IOAPIC line keeps its current format so the stage-8 harness
assertions remain valid:

```text
ioapic: version 0x20, 24 pins, id 0; timer GSI2, console GSI4
```

The boot marker appends `,acpi` only after the ACPI self-test succeeds. It is
appended after the optional `,net` component so the pinned persistence suffix
`vfs,config,blk,persist` and the existing `,net` token remain substrings of the
marker. Documentation must show the new optional tail rather than claiming the
old marker is byte-identical.

## 12. Interfaces

### 12.1 `os_core::acpi`

```rust
pub enum AcpiError { ... }
pub struct Rsdp { pub revision: u8, pub rsdt_address: u32, pub xsdt_address: Option<u64> }
pub struct TableList<'a> { ... }
pub struct IoApicEntry { pub id: u8, pub address: u32, pub gsi_base: u32 }
pub struct IsaOverride { pub bus: u8, pub source: u8, pub gsi: u32, pub flags: u16 }
pub struct Madt { ... }

pub fn parse_rsdp(bytes: &[u8]) -> Result<Rsdp, AcpiError>;
pub fn parse_table_list(bytes: &[u8]) -> Result<TableList<'_>, AcpiError>;
pub fn parse_madt(bytes: &[u8]) -> Result<Madt, AcpiError>;
```

`AcpiError::label()` returns a short static reason for serial logs. `Madt`
exposes `ioapics()`, `overrides()`, `override_for(source)`, and the decoded
polarity/trigger helpers. No method allocates.

### 12.2 `kernel::acpi`

```rust
pub struct IsaRoute {
    pub gsi: u8,
    pub trigger: TriggerMode,
    pub polarity: Polarity,
}

pub struct AcpiSummary {
    pub rsdp_revision: u8,
    pub table_count: usize,
    pub madt_revision: u8,
    pub local_apic_address: u32,
    pub ioapic_id: u8,
    pub ioapic_address: u32,
    pub ioapic_gsi_base: u32,
    pub override_count: usize,
    pub timer: IsaRoute,
    pub console: IsaRoute,
}

pub enum Error {
    Parser(os_core::acpi::AcpiError),
    RsdpMissing,
    RsdpAddress,
    TableRange,
    NoMadt,
    MultipleMadt,
    NoIoApic,
    RouteUnsupported,
}

pub fn init(hhdm: u64, regions: &[MemoryRegion]) -> Result<AcpiSummary, Error>;
pub fn info() -> Option<AcpiSummary>;
```

`Error::label()` returns a short static reason for the serial log. `init` stores
a copy of the accepted summary. `info` returns a copy so callers do not hold a
lock across mapping or IOAPIC programming. A single-core
`SpinLock<Option<AcpiSummary>>` is sufficient.

### 12.3 IOAPIC and IRQ changes

`arch::x86_64::ioapic::map(hhdm, physical_base)` replaces the fixed-only helper.
It maps the 2 MiB-aligned window containing the controller, returns
`hhdm + physical_base`, and rejects a register window that crosses the 2 MiB
boundary. `PHYSICAL_BASE` remains as the documented fallback constant.

`irq::init_ioapic(base, lapic, timer: IsaRoute, console: IsaRoute)` writes the
two entries at the supplied GSIs, stores those GSIs in `ROUTES`, and reports
them in `IoApicInfo` for the boot line. `fallback_to_pic` masks the stored
timer/console GSIs rather than the old constants. Other lines keep the PIC route
until a later PCI routing stage.

## 13. Error handling

All parser failures are values, not panics. The kernel logs the label and the
fixed fallback reason. There is no `unwrap` on bootloader, memory-map or table
data in the new path.

The parser rejects:

- short slices, bad signatures, bad checksums, declared lengths outside the
  slice, and lengths below the structure minimum;
- entry lengths below 2, entries beyond the table, and root-list entry areas
  that do not divide exactly;
- duplicate IOAPIC addresses and duplicate bus-0 source overrides;
- more than 4 IOAPICs or 16 stored ISA overrides;
- zero/non-page-aligned IOAPIC addresses and reserved flag encodings;
- both root table addresses zero.

The kernel rejects an RSDP that is not HHDM-relative, a table outside an allowed
memory-map region, an overflowed physical/virtual address, no MADT, multiple
MADTs, no usable IOAPIC, and routes that do not fit one IOAPIC.

## 14. Testing

### 14.1 Host tests

Add synthetic-table tests to `os_core/src/acpi.rs`, covering at least:

- valid ACPI 1.0 RSDP with RSDT and valid ACPI 2.0+ RSDP with XSDT;
- bad RSDP signature, short v1/v2 RSDP, bad base checksum, bad extended
  checksum, length larger than the slice, and both root addresses zero;
- valid RSDT and XSDT lists, wrong SDT signature, bad checksum, short length,
  unaligned entry area, and address readout for both entry widths;
- valid MADT with one IOAPIC and multiple overrides;
- truncated MADT header, bad checksum, entry length 0/1, entry crossing the
  table end, too many IOAPICs, too many overrides;
- duplicate IOAPIC address, duplicate source override, non-page-aligned
  IOAPIC address, reserved polarity/trigger flags;
- override lookup for IRQ0/IRQ4, absent override, non-zero bus ignored for ISA
  lookup, and conforming flag behavior.

The suite is run with `scripts/test_unit.sh`; the exact new total is recorded in
`progress.md` only after the run.

### 14.2 Boot tests

- `scripts/test_boot_qemu.sh --require-acpi` asserts the ACPI diagnostics, the
  dynamic GSI lines, `,acpi`, ticks and no PIC fallback. It also keeps the
  existing IOAPIC assertions.
- `scripts/test_virtualbox.sh --require-acpi` performs the same assertions on
  VirtualBox.
- `scripts/test_all.sh` adds a `boot-qemu-acpi` step and passes
  `--require-acpi` to the existing VirtualBox step. Expected summary moves from
  `PASS=30` to `PASS=31` with the same two SKIPs; the exact run is recorded.
- A negative parser case is covered on the host. The fixed fallback remains
  covered by the existing stage-8 code path and by code inspection; no boot is
  claimed to exercise it unless a real ACPI-disabled boot is run.

### 14.3 Evidence to record

- exact unit count and `test_all.sh` summary;
- QEMU and VirtualBox `acpi:` lines and `,acpi` marker;
- PIT ticks with no `fell back to 8259`;
- artifact sizes and SHA-256 values for the kernel, ISO and serial logs;
- the QEMU/VirtualBox versions and build ID;
- any SKIP, especially UEFI and virtual GPU.

## 15. Acceptance criteria

1. Host parser tests pass, including all listed rejection cases.
2. `scripts/format.sh` and `scripts/lint.sh` pass with no warnings.
3. QEMU BIOS boot reaches the marker with `,acpi`, the MADT/IOAPIC/route lines,
   PIT ticks through the ACPI-derived GSI2/GSI4 and no PIC fallback.
4. VirtualBox BIOS boot has the same ACPI evidence.
5. The existing boot matrix, driver, network, persistence, shell and PCI tests
   remain green; the two known SKIPs remain SKIP, not PASS.
6. `os_core` remains host-testable and free of hardware access; `kernel` remains
   the only crate touching ACPI tables.
7. Docs record the dynamic discovery, the fixed fallback, the absence of
   `_PRT`/PCI routing and the fact that e1000 remains on PIC INTx.
8. No claim of physical-hardware, UEFI, MSI, APIC timer, SMP or Windows
   interrupt compatibility is made.

## 16. Risks and mitigations

| Risk | Mitigation |
|---|---|
| Firmware table in an unexpected memory-map type | Accept the three Limine ACPI/reserved-mapped types; log and use the fixed fallback otherwise |
| HHDM/RSDP address mismatch on another bootloader revision | Request base revision 6 and validate the address against HHDM; treat a mismatch as unavailable |
| Table length lies or crosses a region | Validate the complete range against one region before creating a slice |
| Multiple IOAPICs or unusual GSI bases | Select one controller covering both routes; otherwise fall back to the PIC rather than guess |
| PIT override absent on some firmware | Retain the verified GSI2 fallback for IRQ0; still derive the IOAPIC base from MADT |
| COM1 override changes trigger mode | Honor explicit flags; use level/high only when the override is absent or conforming |
| Boot marker churn breaks existing test suffixes | Append `,acpi` after `,net`; keep the persistence suffix and existing tokens intact |
| Scope creep into `_PRT`/AML | Keep the non-goals and defer PCI routing; e1000 stays on PIC |

## 17. Documentation changes

- New `docs/ACPI.md`: structures, discovery flow, validation, routes, fallback,
  limits and evidence.
- `docs/ARCHITECTURE.md`: component table, interrupt path, boundaries and known
  limitations.
- `docs/ROADMAP.md`: Phase 9 stage 9 status and the `_PRT` follow-up.
- `docs/TESTING.md`: ACPI commands, assertions, boot marker and evidence.
- `docs/DECISIONS.md`: ADR-0035 for MADT-only discovery and fixed fallback.
- `docs/RESEARCH.md`: R-028 with Limine/ACPI/QEMU sources, confidence and the
  `_PRT` AML finding.
- `progress.md`: commands, results, artifacts, defects, limits and next action.
- `LICENSES.md`: no new dependency is expected; update only if one is added.

## 18. References

- Limine boot protocol, base revision 6, RSDP and memory-map features:
  <https://github.com/limine-bootloader/limine/blob/trunk/PROTOCOL.md>
- ACPI Specification 6.6, RSDP, XSDT/RSDT, MADT and interrupt-source override
  flags: <https://uefi.org/specifications>
- QEMU 11.1.0 `hw/i386/acpi-build.c` and `hw/i386/acpi-common.c` for q35 MADT
  and `_PRT` generation (inspected locally; no QEMU source is copied).
- `docs/RESEARCH.md` R-027 for the stage-8 IOAPIC evidence and the q35 GSI22
  `_PRT` finding.
