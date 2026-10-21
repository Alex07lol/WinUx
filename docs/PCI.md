# PCI bus-0 inventory (Phase 9, stage 1)

Status: implemented and verified (2026-10-20). Contract for
`os_core::pci` helper additions, `kernel/src/pci.rs`, the boot inventory,
the shell `pci` command and `scripts/test_pci_qemu.sh`. Evidence is in
section 7; ADR-0027 and R-020 record the decision and research.

## 1. Scope

This stage turns the e1000-only PCI scan into a general **bus-0 inventory**:
every present function is recorded with its vendor/device IDs, class,
header type and decoded BARs. It is deliberately not a driver framework and
not a resource manager.

In scope:

- Configuration mechanism #1 (`0xCF8`/`0xCFC`), 32-bit config dwords.
- Bus 0, devices 0–31, function 0 always; functions 1–7 only when the
  multifunction bit (header byte bit 7) is set on function 0.
- Header types 0x00 (6 BARs), 0x01 (2 BARs) and 0x02 (1 BAR); other header
  types are recorded without BAR probing.
- BAR sizing by the standard all-ones write-back with immediate restore,
  for 32-bit memory, 64-bit memory (consuming the high dword slot) and I/O
  BARs.
- A fixed-capacity inventory (64 functions) with an explicit truncation
  result, a boot log, a self-test and the shell `pci` command.

Out of scope: bridge traversal beyond bus 0, MMCONFIG/ECAM, resource
assignment or rebalancing, interrupt routing, capability lists, MSI,
power management, hotplug, driver matching/lifecycle and any DMA policy.
Those belong to later Phase 9 stages.

## 2. Inventory model

`kernel/src/pci.rs` records `Function { bus, device, function, vendor,
device_id, class, subclass, interface, header, bar_count, bars[6] }`.
`bars` keeps the config-space index, kind, base and size of each
implemented BAR; a 64-bit BAR uses two dword slots and therefore leaves
the following entry `None`. A function is present iff its vendor is not
`0xFFFF`.

The inventory lives in a `SpinLock<Vec<Function>>`, is rebuilt only by
`pci::init()` at boot, and is read by the self-test, the boot printer, the
shell command and `find_e1000`. Capacity is 64; exceeding it sets a
truncation flag and the self-test fails instead of silently dropping data.

## 3. Enumeration and BAR sizing

1. For device 0–31, read the function-0 vendor dword; absent devices are
   skipped without further probing.
2. Read the header dword at `0x0C`: bits 23:16 give the 7-bit header type,
   bit 23 is the multifunction flag (`os_core::pci::header_type`).
3. Record vendor/device (`0x00`), class/subclass/interface (`0x08`) and
   the masked header type.
4. For each implemented BAR slot, save the original low (and high) dword,
   write all ones, read the mask, then restore the original value before
   doing anything else. Size is the two's complement of the mask with the
   low four address bits ignored; size zero means the BAR is not
   implemented and is not recorded. A 64-bit memory BAR consumes the next
   slot so its high dword is never mistaken for a second BAR.
5. Functions 1–7 are scanned only for multifunction device 0 functions.

BAR probing happens on every boot, including devices the kernel does not
drive. The write/restore window is a few port accesses with interrupts
disabled by the caller's boot context; no driver is bound yet, so no
concurrent access exists. The self-test additionally requires every
recorded size to be a power of two, a matching `bar_count`, no duplicate
BDF and no recorded `0xFFFF` vendor.

## 4. Log and shell surface

Boot prints the summary `pci: enumerated N function(s), M BAR(s) on bus 0`
(plus `(truncated)` when the cap is hit), one line per function, one line
per BAR, then `pci selftest: passed (...)` or `pci selftest: FAILED (...)`
— a failure is logged but not fatal. The boot marker gains `,pci` only
when the self-test passed.

```text
pci: 00:02.0 8086:100e class 02:00:00 hdr 00
pci:   bar0 mem32 base=0xfebc0000 size=0x20000
pci:   bar1 io base=0xc000 size=0x40
```

The shell command `pci` reprints the same inventory on demand. There is no
interactive editing or resource assignment.

## 5. e1000 integration

`find_e1000()` searches the inventory for `8086:100E` and uses BAR0 from
the recorded function. The previous direct bus-0 scan remains as a fallback
when the inventory is empty, so a failed enumeration cannot silently break
networking. No other driver uses the inventory yet.

## 6. Constraints and honesty

- Only bus 0 is enumerated. A NIC behind a PCIe root port on another bus
  would not be found; that is a stated limit, not a bridge claim.
- No resource assignment: BAR values are read as firmware left them.
- No interrupt routing or MSI: the e1000 path remains polled.
- The inventory is boot-time snapshot; hotplug is not observed.
- 64 functions is a project cap; truncation fails the self-test.
- The `pci` shell command and the boot printer are diagnostics, not a
  driver framework.

## 7. Evidence

- `os_core::pci` gains `header_type` with unit tests; host suite 270/270.
- `scripts/test_pci_qemu.sh` boots q35 with a pinned e1000 and asserts the
  summary (6 functions, 7 BARs), the q35 host bridge (`8086:29c0`), LPC
  (`8086:2918`), the e1000 at `00:02.0 8086:100e` with BAR0
  `mem32 base=0xfebc0000 size=0x20000`, the self-test line and the marker's
  `pci` check; QEMU exits via isa-debug-exit (33).
- `scripts/test_all.sh` reports `PASS=27 FAIL=0 SKIP=3` with every previous
  test unchanged; the full suite runs the same boot (which now also prints
  the inventory) at 256 MiB, 512 MiB and 1 GiB.
- Recorded session: `artifacts/qemu-pci-session.log`.

## 8. Next stage (not claimed)

Phase 9 stage 2 should add a driver-registration/lifecycle model over this
inventory (match key, probe/start/stop, resource ownership for MMIO/DMA/IRQ,
mock-device host tests) and then the VM display target. This document makes
no driver, GPU or interrupt claim.
