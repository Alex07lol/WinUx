# Driver registration, matching, lifecycle and resource ownership

Status: stages 2–3 implemented and verified (2026-10-21). Contract for
`os_core::driver`, `kernel/src/driver.rs`, `kernel/src/net.rs`, the shell
`drivers` command and `scripts/test_driver_model_qemu.sh`. Evidence is in
section 7; ADR-0028/0029 and R-021/022 record the decisions and research.

## 1. Scope and non-claims

The model registers compile-time descriptors, matches them against the PCI
inventory, binds deterministically, starts the matched e1000 through
`net::init`, records the resources the binding owns, and exposes explicit
`Bound -> Running -> Failed(reason)` transitions. A later stage may add
`Stopped` in the kernel.

Not claimed: stop/restart in the kernel, device teardown, MMIO unmapping,
DMA frame reclamation, hotplug, interrupts/MSI (the driver is polled), power
management, IRPs, device stacks, WDM compatibility, dynamic driver loading,
class filters, multiple NICs (the ledger and boot state track one e1000),
and any virtual GPU.

## 2. Model (`os_core::driver`)

- `Device`: bus/device/function, vendor, device ID, class, subclass.
- `Match::VendorDevice` or `Match::Class`: exact rules.
- `Descriptor`: stable `name`, one or more rules, `single_instance`.
- `Binding`: device, descriptor index, `State`.
- `State`: `Bound`, `Running`, `Failed(reason)`, `Stopped`.
- `bind`: validates descriptors (unique non-empty names, at least one rule,
  at most 8 descriptors), binds every device to the first matching
  descriptor in registration order, honors single-instance, deterministic,
  at most 64 bindings.
- Transitions: `start` (`Bound -> Running`), `stop` (`Running -> Stopped`),
  `fail` (`Bound|Running -> Failed(reason)`); illegal transitions are
  `InvalidTransition`, never no-ops.
- `Ledger`: allocation-free resource ownership. `Mmio { base, len }` ranges
  are exclusive and overlap-checked; `Irq { line }` is exclusive; `DmaFrames
  { count }` is an accounting budget owned by the allocator. 16 claims max;
  MMIO zero/overflow/overlap and IRQ reuse are errors; `release(owner)`
  drops all of a binding's claims and returns the count; `mmio_owner(addr)`
  and `irq_owner(line)` resolve ownership.
- `unmatched(devices, bindings)` reports unclaimed functions.

The host tests cover matching, priority, single-instance, validation, caps,
determinism, all lifecycle transitions, ledger claims, overlap, IRQ
exclusivity, invalid resources, caps, release and DMA accounting.

## 3. Kernel lifecycle

Boot order: `pci::init()` records bus-0 functions; `driver::init(hhdm, tsc)`
then:

1. converts the snapshot to model devices and binds them;
2. for each binding, finds the PCI function and claims BAR0's MMIO range and
   the e1000 DMA frame budget (`3` descriptor/buffer frames + `RX_PAGES`
   buffer pages = 19 frames) in the ledger **before** touching hardware;
3. calls `net::init(&PciDevice, ...)`, which enables memory/bus-master,
   resets/configures the e1000, allocates the rings, reads the MAC, runs the
   polled loopback selftest and installs the network state;
4. on success moves the binding to `Running`; on any failure releases the
   claims and moves it to `Failed(reason)`;
5. logs the binding, final state, resources, unmatched count and a
   structural self-test: descriptor indices, one binding per device, rules
   hold, single-instance honored, every matched device reached Running or
   Failed, Running bindings own their BAR0 MMIO range, Failed bindings own
   nothing, and rebinding produces the same plan.

A device is never started without a successful claim, and a failed start
leaves no ledger entry. MMIO access is HHDM-mapped for the kernel lifetime;
the e1000 uses polled I/O with no IRQ claim. `net::selftest()` still runs
after driver init and reports the gateway/ARP/ICMP outcome independently.

Pinned q35 boot lines (pinned e1000 at 00:02.0):

```text
driver: registered 1 descriptor(s)
driver: e1000 matched 00:02.0 8086:100e
driver: e1000 state Running
driver: e1000 resources mmio 0xfebc0000..0xfebe0000 (131072 B), 19 DMA frame(s), polled
driver: 5 function(s) unmatched (no registered driver)
driver selftest: passed (1 binding(s), 5 unmatched; match rules, uniqueness, single-instance and resource ownership verified)
```

On a machine with no supported e1000 (e.g. QEMU's default `e1000e`) there
are no bindings, the unmatched count equals the inventory size, the model
self-test still passes and the marker has no `,net`. Shell `drivers`
reprints the descriptor table, binding state, resources and claim count.

## 4. Stop/restart and why it is absent

`E1000` owns rings allocated from the global frame allocator, a device
mapping obtained through the HHDM and global `NET` state. A correct stop
needs an unmap API, frame reclamation, NET teardown and a way to re-enable
the device without leaking or double-freeing; none exist yet. Implementing
those in the same stage as the first start would mix claims with untested
teardown. `State::Stopped` and `Ledger::release` are host-tested but unused
in the kernel; that limit is stated in the boot report and tests. A future
stage must add mock teardown tests, then a QEMU stop/restart harness, before
claiming it.

## 5. Constraints

- Compile-time descriptor identity; no dynamic loading.
- One device gets one descriptor; no driver stacks or class filters.
- One global NIC state; a second e1000 is intentionally unbound
  (single-instance) and tracked as unmatched, not hidden.
- Failure reasons are static strings; no allocation in the model.
- This is not WDM and makes no Windows-driver compatibility claim.

## 6. Why the e1000 moved behind the model now

Start is the first lifecycle transition that can fail in real hardware code,
and the network path already had a strong regression gate
(`test_net_qemu.sh`). Passing the matched `PciDevice` into `net::init`
removes a second PCI scan, makes ownership explicit (BAR0 claim before
start), and keeps the existing network selftest as an independent check.
Stop remains out of scope (section 4).

## 7. Evidence

- Host suite 286 tests (280 + 6 ledger tests); `os_core::driver` now has 16
  tests covering binding and ownership.
- `scripts/test_driver_model_qemu.sh` PASS on q35 with a pinned e1000: the
  six lines above, `,driver`, no panic, QEMU exit 33.
- `scripts/test_net_qemu.sh` PASS: `state Running` did not regress ARP/ICMP
  or the loopback selftest.
- `scripts/test_all.sh` PASS=28 FAIL=0 SKIP=2.
- Recorded session: `artifacts/qemu-driver-session.log`.

## 8. Next stage (not claimed)

Add kernel stop/restart with mock teardown tests, then extend the ledger to
real DMA page ownership and IRQ allocation. Only after that should a
virtual GPU target be attempted.
