# Driver registration, matching, lifecycle, resource ownership and restart

Status: stages 2–4 implemented and verified (2026-10-21). Contract for
`os_core::driver`, `kernel/src/driver.rs`, `kernel/src/net.rs`,
`kernel/src/e1000.rs`, the shell `drivers [stop|start]` command and the QEMU
harnesses. Evidence is in section 8; ADR-0028/0029/0030 and
R-021/022/023 record the decisions and research.

## 1. Scope and non-claims

The model registers compile-time descriptors, matches them against the PCI
inventory, binds deterministically, starts the matched e1000 through
`net::init`, records the resources the binding owns, and exposes explicit
`Bound -> Running -> Stopped` and `-> Failed(reason)` transitions. Stop
disables the device, resets it, returns all ring frames to the frame
allocator, removes the network state and releases the ledger claims; start
repeats the claim/start path. A stop/restart cycle is exercised end to end on
QEMU, including ICMP after the restart.

Not claimed: MMIO unmapping (the HHDM mapping is kernel-lifetime and reused),
interrupts/MSI (the driver is polled), hotplug, power management, IRPs,
device stacks, WDM compatibility, dynamic driver loading, class filters,
multiple NICs (one e1000; a second is unbound) and any virtual GPU. A reset
timeout during stop intentionally leaks the ring frames rather than freeing
memory the device might still DMA into; that path is reported as failure and
leaves the binding `Failed`, so restart requires a clean stop.

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
- Transitions: `start` (`Bound|Stopped -> Running`, so a cleanly stopped
  binding can restart), `stop` (`Running -> Stopped`), `fail`
  (`Bound|Running -> Failed(reason)`); illegal transitions are
  `InvalidTransition`, never no-ops. `Failed` is terminal without rebinding.
- `Ledger`: allocation-free resource ownership. `Mmio { base, len }` ranges
  are exclusive and overlap-checked; `Irq { line }` is exclusive; `DmaFrames
  { count }` is an accounting budget owned by the allocator. 16 claims max;
  MMIO zero/overflow/overlap and IRQ reuse are errors; `release(owner)`
  drops all of a binding's claims and returns the count; `mmio_owner(addr)`
  and `irq_owner(line)` resolve ownership.
- `unmatched(devices, bindings)` reports unclaimed functions.

The host tests cover matching, priority, single-instance, validation, caps,
determinism, all lifecycle transitions (including restart from `Stopped`),
ledger claims, overlap, IRQ exclusivity, invalid resources, caps, release,
DMA accounting and stop-releases/restart-reclaims.

## 3. Kernel startup lifecycle

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
leaves no ledger entry. The e1000 uses polled I/O with no IRQ claim.

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
reprints the descriptor table, state, resources and claim count.

## 4. Stop and restart

`drivers stop` (shell, only from `Running`):

1. `net::shutdown` removes and takes the network state first, then disables
   RX/TX with `IMC`/`RCTL=0`/`TCTL=0`, resets the device and waits for the
   reset to complete. A reset timeout returns an error **without freeing the
   rings** (the device may still DMA); the driver still releases its ledger
   claims and records `Failed`.
2. On success the RX buffer run (16 pages) and the TX/RX descriptor and TX
   buffer frames (3) are returned with `memory::free_pages`, logging
   `net: stopped (19 DMA frame(s) released)`.
3. The driver releases its 2 claims (`Mmio`, `DmaFrames`), records
   `Stopped`, and logs `driver: e1000 state Stopped` plus
   `driver: e1000 resources released (2 claim(s); mmio mapping retained)`.
   The HHDM MMIO mapping is retained for the kernel lifetime because there is
   no unmap API; pages backing the mapping are firmware MMIO, not allocator
   frames.

`drivers start` (only from `Stopped`) re-runs the same claim and
`net::init` path: BAR0 and the 19-frame budget are claimed again, the device
is reset/configured, rings are allocated, the MAC is re-read, the loopback
selftest runs, and the binding returns to `Running` with
`driver: e1000 restarted (stop/start cycle ok)`. A machine without a run
state or a failed driver is rejected, never silently restarted.

The QEMU harness proves the accounting end to end: `mem` free frames increase
by exactly 19 after stop and return to the pre-stop value after restart,
`net` is inactive while stopped, `drivers` shows zero claims, and
`ping 10.0.2.2 -c 1` succeeds after the restart.

## 5. Why the e1000 moved behind the model

Start is the first lifecycle transition that can fail in real hardware code,
and the network path already had a strong regression gate
(`test_net_qemu.sh`). Passing the matched `PciDevice` into `net::init`
removes a second PCI scan, makes ownership explicit (BAR0 claim before
start), and keeps the existing network selftest as an independent check.
Stop/restart now exercises the same path in reverse.

## 6. Constraints

- Compile-time descriptor identity; no dynamic loading.
- One device gets one descriptor; no driver stacks or class filters.
- One global NIC state; a second e1000 is intentionally unbound
  (single-instance) and tracked as unmatched, not hidden.
- Failure reasons are static strings; no allocation in the model.
- Stop/restart assumes no other thread is using the NIC: the shell command
  runs after boot and network calls are synchronous/polled. There is no
  concurrency guard against a future preempting network caller; the network
  lock only protects individual operations, not the stop/restart sequence.
- This is not WDM and makes no Windows-driver compatibility claim.

## 7. Testing notes

- `scripts/test_driver_model_qemu.sh` boots with debug-exit and checks the
  boot lifecycle lines; it does not stop/restart.
- `scripts/test_driver_restart_qemu.sh` boots the interactive ISO and drives
  `mem`, `drivers stop`, `net`, `drivers`, `drivers start`,
  `ping 10.0.2.2 -c 1` over paced serial. It asserts exactly 19 frames are
  freed and reallocated, so a leak or double-free fails the test.
- Host tests cannot exercise the allocator or hardware; they cover the
  state/ledger rules and the stop/restart accounting contract.

## 8. Evidence

- Host suite 287 tests; `os_core::driver` has 17 tests covering binding,
  ownership, teardown and restart.
- `scripts/test_driver_model_qemu.sh` PASS on q35 with a pinned e1000: the
  six boot lines, `,driver`, no panic, QEMU exit 33.
- `scripts/test_driver_restart_qemu.sh` PASS: stop freed 19 frames, restart
  reclaimed them, `net` inactive/active, ping after restart.
- `scripts/test_net_qemu.sh` PASS: the restart-capable driver did not
  regress ARP/ICMP/UDP.
- `scripts/test_all.sh` PASS=29 FAIL=0 SKIP=2.
- Recorded sessions: `artifacts/qemu-driver-session.log`,
  `artifacts/qemu-driver-restart-session.log`.

## 9. Next stage (not claimed)

Extend ownership to real DMA page tracking and IRQ allocation, then consider
a virtual GPU. MMIO unmapping remains absent; a stop/restart does not remap
or release the HHDM window.
