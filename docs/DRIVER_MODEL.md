# Driver registration, matching, lifecycle, resource ownership and restart

Status: stages 2-6 implemented and verified (2026-10-22). Contract for
`os_core::driver`, `kernel/src/driver.rs`, `kernel/src/net.rs`,
`kernel/src/e1000.rs`, `kernel/src/irq.rs`, the shell `drivers [stop|start]`
command and the QEMU harnesses. Evidence is in section 8;
ADR-0028/0029/0030/0031/0032 and R-021/022/023/024/025 record the decisions
and research.

## 1. Scope and non-claims

The model registers compile-time descriptors, matches them against the PCI
inventory, binds deterministically, starts the matched e1000 through
`net::init`, records the exact BAR0 and DMA page ranges the binding owns, and
exposes explicit `Bound -> Running -> Stopped` and `-> Failed(reason)`
transitions. Stage 6 adds legacy INTx delivery: firmware's PCI interrupt
line is claimed exclusively, the e1000 handler is registered before the line
is unmasked, and a handler acknowledges `ICR` and counts events without
touching the rings. Stop masks and unregisters the line, disables the device,
resets it, returns all ring frames, removes network state and releases every
claim; start repeats the same path. A stop/restart cycle is exercised end to
end on QEMU, including ICMP and new interrupt events after the restart.

Not claimed: MMIO unmapping (the HHDM mapping is kernel-lifetime and reused),
MSI/IOAPIC routing (only legacy 8259 INTx is implemented), interrupt-driven
networking (RX frames are still drained by `poll()`; the handler only
acknowledges and counts), hotplug, power management, IRPs, device stacks, WDM
compatibility, dynamic driver loading, class filters, multiple NICs (one
e1000; a second is unbound) and any virtual GPU. A reset
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
- `Ledger`: allocation-free resource ownership. `Mmio { base, len }` and
  `DmaPages { base, pages }` are physical range claims; all physical claims
  are mutually overlap-checked, so a DMA page cannot alias MMIO or another
  driver's pages. DMA bases must be page-aligned, `pages` non-zero and the
  range must not overflow. `Irq { line }` is a separate exclusive claim;
  on QEMU/VirtualBox the e1000 now holds one alongside BAR0 and the four DMA
  ranges. 16 claims max; `release(owner)` drops all of a binding's claims and
  returns the count; `mmio_owner(addr)`, `dma_owner(addr)`, `dma_pages(owner)`
  and `irq_owner(line)` resolve ownership.
- `unmatched(devices, bindings)` reports unclaimed functions.

The host tests cover matching, priority, single-instance, validation, caps,
determinism, all lifecycle transitions (including restart from `Stopped`),
ledger claims, physical overlap (including MMIO vs DMA), IRQ exclusivity,
invalid resources, caps, release, exact DMA page accounting and
stop-releases/restart-reclaims.

## 3. Kernel startup lifecycle

Boot order: `pci::init()` records bus-0 functions; `driver::init(hhdm, tsc)`
then:

1. converts the snapshot to model devices and binds them;
2. for each binding, finds the PCI function and claims BAR0's MMIO range
   **before** touching hardware;
3. if firmware assigned a usable legacy line (pin 1-4, line 1-15), registers
   the exclusive e1000 handler and claims `Irq { line }` in the ledger;
4. calls `net::init(&PciDevice, ..., irq_line, claim)`, which enables
   memory/bus-master, resets the e1000, allocates the RX/TX rings, then
   invokes the claim callback with the four physical ranges (RX buffer run,
   TX descriptors, RX descriptors, TX buffer; 19 pages) **before**
   `RCTL`/`TCTL` enable the device. A claim failure frees the just-allocated
   rings and aborts start. On success it programs `RDTR`, publishes the MMIO
   base and enables `RXT0|RXDMT0` in `IMS`; the PIC line is still masked;
5. after `net::init` returns, unmasks the line, moves the binding to
   `Running`; on any failure masks/unregisters the IRQ, frees the rings and
   releases all claims, then moves it to `Failed(reason)`;
6. logs the binding, final state, resources, unmatched count and a
   structural self-test: descriptor indices, one binding per device, rules
   hold, single-instance honored, every matched device reached Running or
   Failed, Running bindings own BAR0, exactly 19 DMA pages and (when a line
   exists) the IRQ, Failed bindings own nothing, and rebinding produces the
   same plan.

A device is never started without a successful claim, and a failed start
leaves no ledger entry. The e1000's IRQ claim describes delivery only: RX/TX
completion remains polled, so an interrupt does not run protocol code.

Pinned q35 boot lines (pinned e1000 at 00:02.0):

```text
driver: registered 1 descriptor(s)
driver: e1000 matched 00:02.0 8086:100e
net: e1000 at 00:02.0 bar0 0xfebc0000 size 128 KiB
net: mac 52:54:00:12:34:56
net: ip 10.0.2.15/24 gw 10.0.2.2
net: e1000 irq 11 (legacy INTx, RXT0/RXDMT0; RX drain remains polled)
net: loopback selftest: unsupported
driver: e1000 state Running
driver: e1000 resources mmio 0xfebc0000..0xfebe0000 (131072 B), 4 DMA range(s)/19 page(s), irq 11 (legacy INTx)
driver: 5 function(s) unmatched (no registered driver)
driver selftest: passed (1 binding(s), 5 unmatched; match rules, uniqueness, single-instance and resource ownership verified)
```

On a machine with no supported e1000 (e.g. QEMU's default `e1000e`) there
are no bindings, the unmatched count equals the inventory size, the model
self-test still passes and the marker has no `,net`. Shell `drivers`
reprints the descriptor table, state, resources and claim count.

## 4. Stop and restart

`drivers stop` (shell, only from `Running`):

1. masks the IRQ line first, then `net::shutdown` removes and takes the
   network state, disables RX/TX with `IMC`/`RCTL=0`/`TCTL=0`, resets the
   device and waits for the reset to complete. A reset timeout returns an
   error **without freeing the rings** (the device may still DMA); the driver
   still unregisters the handler, releases its ledger claims and records
   `Failed`.
2. On success the RX buffer run (16 pages) and the TX/RX descriptor and TX
   buffer frames (3) are returned with `memory::free_pages`, logging
   `net: stopped (19 DMA frame(s) released)`.
3. The driver unregisters the handler and releases its 6 claims (`Mmio`,
   four `DmaPages` ranges and `Irq`), records `Stopped`, and logs
   `driver: e1000 state Stopped` plus
   `driver: e1000 resources released (6 claim(s); mmio mapping retained)`.
   The HHDM MMIO mapping is retained for the kernel lifetime because there is
   no unmap API; pages backing the mapping are firmware MMIO, not allocator
   frames.

`drivers start` (only from `Stopped`) re-runs the same claim and
`net::init` path: BAR0 is claimed again, the device is reset, rings are
reallocated, their four exact ranges are claimed (19 pages total) before
RX/TX enable, the IRQ handler is registered and the line claimed again, the
MAC is re-read, the loopback selftest runs, the line is unmasked, and the
binding returns to `Running` with
`driver: e1000 restarted (stop/start cycle ok)`. A machine without a run
state or a failed driver is rejected, never silently restarted.

The QEMU harness proves the accounting end to end: `mem` free frames increase
by exactly 19 after stop and return to the pre-stop value after restart,
`net` is inactive while stopped, `drivers` shows zero claims, and
`ping 10.0.2.2 -c 1` succeeds after the restart and the e1000 IRQ event
count grows (2 -> 4 in the recorded run). The ledger log names six claims,
and the kernel self-test rejects a Running binding whose DMA pages do not sum
to 19 or whose IRQ line is owned by another claim.

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
  `net`, `mem`, `drivers stop`, `net`, `drivers`, `drivers start`,
  `ping 10.0.2.2 -c 1`, `net` over paced serial. It asserts exactly 19 frames
  are freed and reallocated and that the e1000 IRQ event count grows after the
  restart, so a leak, double-free or unregistered handler fails the test.
- Host tests cannot exercise the allocator or hardware; they cover the
  state/ledger rules, physical overlap, exact DMA page accounting and the
  stop/restart accounting contract.

## 8. Evidence

- Host suite 287 tests; `os_core::driver` has 17 tests covering binding,
  exact page ownership, physical overlap, teardown and restart.
- `scripts/test_driver_model_qemu.sh` PASS on q35 with a pinned e1000: the
  six boot lines, `,driver`, no panic, QEMU exit 33.
- `scripts/test_driver_restart_qemu.sh` PASS: stop freed 19 frames, restart
  reclaimed them, `net` inactive/active, ping after restart.
- `scripts/test_net_qemu.sh` PASS: the restart-capable driver did not
  regress ARP/ICMP/UDP, and the e1000 IRQ event count grows after `ping`
  (boot baseline 2). VirtualBox assigns line 9 and boots; its isolated NAT has
  no gateway reply, so no delivery count is claimed there.
- `scripts/test_all.sh` PASS=29 FAIL=0 SKIP=2 after the stage-6 changes.
- Recorded sessions: `artifacts/qemu-driver-session.log`,
  `artifacts/qemu-driver-restart-session.log`, `artifacts/qemu-net-session.log`,
  `artifacts/vbox-serial.log`.

## 9. Next stage (not claimed)

A useful interrupt-driven RX path (deferred ring drain with bounded work),
then APIC/IOAPIC/MSI routing, then a virtual GPU. The stage-6 handler is a
delivery/ownership proof only: it reads `ICR` and counts, while protocol work
remains polled. MMIO unmapping and IOMMU isolation remain absent; DMA claims
detect overlap in our ledger, they do not enforce isolation in hardware.
