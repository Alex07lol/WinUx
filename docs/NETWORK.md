# e1000 networking (Phase 6, stage 5) — design spec

Status: **approved 2026-10-18 (scope chosen by the user); implemented and
verified 2026-10-18.** Phase 9 stage 6 (2026-10-22) adds legacy INTx delivery
and ownership; stage 7 (2026-10-22) adds a bounded IRQ-to-service RX queue
(sections 12-13). Protocol dispatch still runs from the service path. This
document is the contract for the stage: code, tests and logs must match it,
and any deviation is recorded in §11 (observed results, a corrected loopback
assumption and one harness-found defect are recorded there).

## 1. Goal and non-goals

Goal: the kernel talks on a real network device — an Intel 82540EM ("e1000")
Ethernet controller — with our own driver and our own protocol code, and proves
it end to end: ARP resolution of the slirp gateway, ICMP echo request/reply
(`ping`), and a UDP datagram round-trip driven by the test harness. Protocol
codecs and state machines live in host-tested `os_core` modules; the kernel
side is a polled driver plus a thin stack binding.

Deliberately **not** in this stage:

- no DHCP (static `10.0.2.15/24`, gateway `10.0.2.2` — the QEMU slirp and
  VirtualBox NAT defaults), no DNS, no routing beyond the local subnet;
- no TCP, no sockets API: UDP is exercised through a bounded shell fixture
  (`udpecho`), not a general socket layer;
- no background packet-processing thread and no MSI: stage 7's IRQ handler
  copies completed RX descriptors into a bounded queue, but parsing and replies
  run only while a command services the queue; legacy INTx only;
- no IPv6, no fragment reassembly (fragmented packets are rejected), no
  checksum offload, no VLAN, no jumbo frames;
- one NIC, bus 0 only, device `8086:100E` only (`e1000e` 82574L and others are
  explicitly unsupported and logged as inactive);
- no general PCI device framework: just enough config-space access to find and
  configure the NIC (the `pci` test step remains a planned SKIP for a real
  driver model).

## 2. Research and primary sources (R-012)

Recorded fully in `docs/RESEARCH.md` as R-012. Summary:

- **8254x register/descriptor model** from the *8254x Family of Gigabit
  Ethernet Controllers Software Developer's Manual* (Intel, public): CTRL
  reset (`CTRL.RST`, bit 26, self-clearing) and set-link-up/full-duplex,
  RAL/RAH MAC address registers, RCTL receive-enable/broadcast-accept/CRC-strip
  and `RCTL.LBM` loopback bits (7:6), TCTL transmit enable/pad-short-packets,
  legacy 16-byte TX/RX descriptors (buffer address, length, `CMD.EOP/IFCS/RS`,
  `STA.DD`), 2048-byte receive buffers, and the RX ring tail/head protocol.
- **MAC address source**: reset auto-loads RAL/RAH from the EEPROM (QEMU does
  this); VirtualBox's `EERD` register is unimplemented (OSDev wiki, 8254x
  page), so a 4-wire EEPROM read of words 0–2 is implemented as fallback.
- **Loopback**: the internal MAC loopback is enabled via `RCTL.LBM` (bits 7:6,
  full duplex required). The original research claim that QEMU's e1000 family
  wraps TX descriptors back to the RX queue when `RCTL.LBM_MAC` is set was
  **wrong**: QEMU's `hw/net/e1000.c` loops only when the PHY `BMCR` loopback bit
  is set (source read 2026-10-18), so QEMU 11.1.0 and VirtualBox 7.2.20 both
  report `net: loopback selftest: unsupported`. Corrected in R-012; the check
  remains a non-fatal bring-up aid.
- **slirp**: QEMU user-mode networking maps the guest to `10.0.2.15/24`, the
  gateway/host to `10.0.2.2`, answers ARP for the gateway and ICMP echo to it,
  and `hostfwd` UDP forwards host datagrams to a guest port
  (QEMU system emulation documentation). The harness depends on exactly these
  documented behaviours.
- **PCI config space**: legacy mechanism #1 (`0xCF8` address / `0xCFC` data),
  BAR sizing via all-ones write-back and restore (PCI Local Bus
  Specification 3.0, public).

## 3. Changes overview

- New `os_core::pci` — pure config-space address construction and BAR decode
  (kind, base, size) with host tests.
- New `os_core::net` (directory module) with submodules:
  - `eth` — MAC type, EtherType, frame header parse/build;
  - `arp` — request/reply parse/build, 8-entry cache with expiry;
  - `ipv4` — address type, header parse/build, ones'-complement checksum,
    fragment rejection;
  - `icmp` — echo request/reply parse/build, checksum;
  - `udp` — header parse/build, pseudo-header checksum, zero-checksum rules.
- New `kernel/src/pci.rs` — config-space reads/writes, bus-0 scan for
  `8086:100E`, BAR0 read/size, memory-space + bus-master enable.
- New `kernel/src/e1000.rs` — real driver: MMIO register access through a
  2 MiB uncached mapping, reset, MAC acquisition (RAL/RAH, EEPROM fallback),
  RX ring (32 × 2048-byte legacy descriptors), TX ring (8 descriptors, one
  shared 2048-byte buffer, synchronous send), polled receive, loopback check,
  counters, PIT-deadline watchdogs.
- New `kernel/src/net.rs` — stack binding: interface state (MAC/IP/gateway),
  ARP resolution, frame dispatch, `ping`, UDP echo fixture, serial log lines.
- `kernel/src/arch/x86_64/io.rs` gains `inl`; `kernel/src/main.rs` runs
  `net::init` + `net::selftest` after the persistence step and appends `,net`
  to the marker when the probe passed; `kernel/src/shell.rs` gains `net`,
  `ping` and `udpecho` builtins.
- New `scripts/test_net_qemu.sh` + `scripts/test_net_qemu.py`; `net` step in
  `test_all.sh`; `scripts/test_virtualbox.sh` attaches an Intel PRO/1000 MT
  Desktop NAT NIC (82540EM) so VirtualBox exercises the same driver.

## 4. Host-tested core (`os_core`)

Pinned shapes (fields may be private where a host test does not need them):

- `pci::config_address(bus, device, function, offset) -> u32` — bit 31 set,
  offset masked to 2 bits; `pci::Bar::decode(lo, hi) -> Option<Bar>`
  (`BarKind::Memory32 | Memory64 | Io`), `pci::bar_size(readback) -> u64`
  (all-ones mask → size, 0 when absent); `pci::vendor_device(dword)`,
  `pci::device_class(dword)`.
- `net::eth::MacAddr` — `[u8; 6]`, `Display` as `aa:bb:cc:dd:ee:ff`,
  `BROADCAST`, `is_broadcast`; `net::eth::Header { dst, src, ethertype }`
  parse from a frame, `write(frame, src, dst, ethertype) -> usize`;
  `ETHERTYPE_IPV4 = 0x0800`, `ETHERTYPE_ARP = 0x0806`.
- `net::arp::Packet` — hardware type/protocol/hlen/plen pinned (1/0x0800/6/4),
  ops 1/2, parse and build for request/reply, 28-byte payload.
  `net::arp::Cache<N>` — fixed capacity, `insert(mac, ip, now)` refreshes and
  evicts the oldest entry when full, `lookup(ip, now)` honours a 30 s lifetime
  (`EXPIRY_TICKS = 3000` at 100 Hz), `len()`/`iter()` for status.
- `net::ipv4::Addr` — `[u8; 4]`, `Display` dotted decimal, `BROADCAST`/
  `is_broadcast`; `net::ipv4::Header` parse (version 4, IHL 5 only, total
  length, protocol, TTL, source, destination, header checksum verified) and
  build (ttl 64, id 0, no flags, checksum computed); `net::ipv4::checksum`.
  Fragmented packets (MF or fragment offset ≠ 0) are a parse error.
- `net::icmp` — `Message` parse for echo request (type 8) / reply (type 0),
  id/seq/payload access, `build_echo_request(id, seq, payload)`,
  `build_echo_reply(...)` with checksums; `CHECKSUM` validation on parse.
- `net::udp::Header` parse (source/destination port, length bounds against the
  IPv4 payload, checksum validation against the pseudo-header with the
  documented zero = "no checksum" acceptance) and `build` computing the
  checksum from (source IP, destination IP, payload).

Host tests (target ≥ 45 new, 128 → ≥ 173 total): golden byte vectors for every
parser/builder, checksum known-answer vectors, cache insertion/refresh/expiry/
eviction, rejection vectors (bad version, IHL ≠ 5, MF set, truncated payloads,
bad checksums, zero-length UDP payload boundary), MAC formatting.

## 5. PCI and device discovery (`kernel/src/pci.rs`)

- Config mechanism #1: `outl(0xCF8, address)`, `inl(0xCFC)`; writes restore
  the original BAR value immediately after sizing.
- Enumerate bus 0, devices 0..31, functions 0..7 for multifunction device
  0; every present function is recorded with vendor/device IDs, class,
  header type and sized BARs (Phase 9 stage 1).
- `kernel/src/driver.rs` matches `8086:100E` from that inventory and passes
  the matched `PciDevice` to `net::init`; there is no separate direct-scan
  fallback anymore (removed in stage 3).
- Enable command register bits: memory space (bit 1) and bus master (bit 2).
- BAR0 must be a memory BAR of at least 128 KiB, else the device is rejected.
- One pinned log line from `net::init` (below).

## 6. Driver (`kernel/src/e1000.rs`)

- MMIO: BAR0 aligned down to 2 MiB, mapped uncached with the existing
  `PageTables::map_mmio_2m` at `hhdm + aligned` (already-mapped returns Ok);
  register access through that virtual base.
- Reset: `IMC = all ones`; set `CTRL.RST`, poll until clear (1 s PIT
  deadline); `CTRL = FD | ASDE | SLU`; zero the multicast table array;
  `RAL/RAH` → MAC; if zero, 4-wire EEPROM read of words 0–2 with a bounded
  clock loop; a still-zero MAC fails `init`.
- Rings from the frame allocator, physical addresses via the HHDM offset:
  - RX: one 4 KiB descriptor frame (32 × 16 B), 32 × 2048 B buffer frames,
    `RDBAL/RDBAH/RDLEN`, `RDH = 0`, `RDT = 31`;
  - TX: one 4 KiB descriptor frame (8 × 16 B), one 2048 B buffer frame,
    `TDBAL/TDBAH/TDLEN`, `TDH = TDT = 0`.
- Before RX/TX enable, the four allocated physical ranges are claimed by the
  driver model (RX buffer run, TX descriptors, RX descriptors, TX buffer; 19
  pages). `RCTL`/`TCTL` are written only after the claim succeeds; a claim
  failure frees the rings without enabling the device.
- Control registers after rings: `RCTL = EN | BAM | SECRC` (unicast filtering
  by RAL/RAH stays on), `TCTL = EN | PSP | RTLC | CT(0x10) | COLD(0x40)`.
- `send(frame)`: wait for the previous descriptor's `STA.DD` (1 s deadline),
  copy into the shared buffer, set length and `CMD = EOP | IFCS | RS`, clear
  `STA`, advance `TDT`, poll `STA.DD` with `TDH` wrap (1 s deadline); returns
  a labelled error on timeout.
- `receive_into(&mut [u8]) -> Option<usize>`: the polled fallback used when
  no legacy line is available. It reads the next descriptor, and if `STA.DD`
  is set copies the frame, clears `STA`, advances `RDT` and returns; hardware
  errors and non-EOP descriptors are counted and dropped. When the line is
  live, the IRQ handler owns this ring and the service path pops the software
  queue instead.
- Loopback check: set `RCTL.LBM_MAC`, send a crafted 60-byte frame (EtherType
  0x88B5, fixed pattern), poll RX up to 500 ms for the frame, clear the LBM
  bit. Outcomes (never fatal in themselves): identical frame →
  `net: loopback selftest: passed`; no frame → `net: loopback selftest:
  unsupported`; a frame that does not match → `net: loopback selftest:
  unexpected frame` (counted as an error). The ARP/ICMP probe is the gate.
- A device that answers the PCI scan but fails driver bring-up logs
  `net: e1000 init failed ({reason})` and the interface stays inactive
  (reason ∈ {reset timeout, mac unavailable, ring allocation, loopback
  state}); the probe is then `net selftest: skipped (no NIC)`.
- Counters: `frames_tx`, `frames_rx`, `errors` (timeouts, descriptor errors).
- No IRQ in the original stage: `IMS` stayed masked and `poll()` was called
  from the stack when a command waited. Phase 9 stage 6 enables `RXT0|RXDMT0`
  and claims the legacy line. Stage 7 runs the loopback selftest first, then
  publishes the ring context and enables the interrupt; the handler reads
  `ICR` and drains up to 32 completed descriptors into a 16-frame SPSC queue.
  A full queue drops new frames and counts them. See sections 12-13, ADR-0033
  and R-026.

## 7. Stack and shell (`kernel/src/net.rs`, `kernel/src/shell.rs`)

- `net::init(device, hhdm, tsc, irq_line, claim) -> Result` runs from the
  driver model after the PCI inventory, MMIO/DMA claims and IRQ registration:
  - no binding/claim: `net: inactive (no supported e1000 NIC)`; every entry
    point becomes a no-op (except `net` status printing inactive);
  - started NIC: pinned line
    `net: e1000 at {bus:02x}:{dev:02x}.{fn} bar0 {:#x} size {} KiB`, then
    `net: mac {mac}`, `net: ip 10.0.2.15/24 gw 10.0.2.2`, the loopback outcome,
    and either
    `net: e1000 irq {line} (legacy INTx, RXT0/RXDMT0; RX queue 16 frames, drop-newest)`
    or `net: e1000 irq unavailable; polled only`.
- `net::selftest() -> Selftest`, exactly one line:
  - `net selftest: skipped (no NIC)`;
  - `net selftest: no gateway reply ({reason})` — non-fatal (a NIC on a
    network without a slirp gateway must not kill the boot);
  - `net selftest: passed (arp resolved, icmp echo reply in {n} ms)`.
  - `{reason}` ∈ {arp timeout, echo timeout}. The loopback outcomes are
    logged separately as `net: loopback selftest: passed` / `unsupported` /
    `unexpected frame` (§6).
- Marker: `,net` is appended (after `,persist`) only when the selftest passed.
- `net::ping(target, count, ..)` — ARP resolve (up to 3 requests, 1 s), then
  one ICMP echo per sequence number, 1 s each, polling between; TTL and RTT
  from the reply (TSC when the bootloader reported a frequency, else the PIT
  tick bound). Serial output (from the shell builtin):
  `reply from {ip}: seq={n} ttl={ttl} time={ms} ms` and
  `ping: {sent} sent, {received} received, {loss}% loss`; unresolved ARP is
  `ping: arp timeout for {ip}` as an error.
- `net::udpecho(port)` — bind in the single echo slot, poll until one
  datagram arrives (10 s), reply with the identical payload from
  `10.0.2.15:{port}` to the sender, then unbind:
  `udpecho: listening on 10.0.2.15:{port} for one datagram`,
  `udpecho: echoed {n} bytes to {src_ip}:{src_port}`; timeout is an error
  (`udpecho: no datagram within 10 s`). Payloads over 1024 bytes are dropped.
- `net` status builtin:
  `net: mac {mac}, ip {ip}/24 gw {gw}; tx {tx} frames, rx {rx} frames, arp {used}/8 entries, errors {errors}, irq {events} event(s), rxq {depth}/{capacity} ({delivered} delivered, {dropped} dropped)`
  (inactive NIC prints the one inactive line). `rx` includes frames produced by
  the IRQ handler; `delivered` counts queue pops.
- Dispatch drops (counted, never logged per packet): non-IPv4/ARP EtherTypes,
  fragmented IPv4, ICMP non-echo, UDP to unbound ports, ARP that is not for
  our IP/gateway, frames shorter than their headers claim.

## 7a. Stop and restart

`net::shutdown()` takes and removes the network state first, disables RX/TX,
resets the e1000, frees the 16-page RX buffer run and the three TX/RX
descriptor/buffer frames (19 total), and logs
`net: stopped (19 DMA frame(s) released)`. `net::init` can then run again
through `drivers start`: the HHDM BAR0 mapping is retained and reused, rings
are reallocated, the MAC is re-read and the loopback selftest reruns.
A reset timeout does not free the rings (the device may still DMA); it is
reported as a stop failure. See `docs/DRIVER_MODEL.md` section 4.

## 8. Pinned serial lines (evidence contract)

From §6–§7, exactly one of each group:

```text
net: inactive (no supported e1000 NIC)
net: e1000 at {bb:dd.f} bar0 {:#x} size {} KiB
net: e1000 init failed ({reason})
net: mac {aa:bb:cc:dd:ee:ff}
net: ip 10.0.2.15/24 gw 10.0.2.2
net: loopback selftest: passed
net: loopback selftest: unsupported
net: loopback selftest: unexpected frame
net: e1000 irq {line} (legacy INTx, RXT0/RXDMT0; RX queue 16 frames, drop-newest)
net: e1000 irq unavailable; polled only
net selftest: skipped (no NIC)
net selftest: no gateway reply ({reason})
net selftest: passed (arp resolved, icmp echo reply in {n} ms)
```

The boot-success marker's checks list does not change the meaning of existing
entries; `,net` appears only when the selftest passed.

## 9. Testing plan

Host tests: as §4.

`scripts/test_net_qemu.sh` (wired into `test_all.sh` as `net`):

1. Builds `artifacts/project-os-interactive.iso`.
2. Boots it with the shared TCP-serial session driver on `q35` plus
   `-nic user,model=e1000,hostfwd=udp:127.0.0.1:{host_port}-:7777`; the
   driver picks a free host UDP port itself.
3. Asserts on serial: the `net: e1000 at`, `net: mac 52:54:00:` (slirp's
   default OUI), `net: e1000 irq 11` (queue handoff), `net selftest: passed`
   lines and `,net` in the marker.
4. Types `net` (status tokens including `rxq`), `ping 10.0.2.2 -c 3` (three
   replies, `0% loss`), `net` again and `udpecho 7777`. The harness requires
   both the IRQ event count and the queue's delivered-frame count to grow
   after the ping (recorded: 2 -> 5 events and 2 -> 5 frames).
5. Sends `PROJECT_OS-NET-TEST` (19 bytes) to `127.0.0.1:{host_port}` over
   UDP and asserts the identical payload arrives back on the same socket
   (the guest replied) and the guest printed
   `udpecho: echoed 19 bytes`.
6. Transcript `artifacts/qemu-net-session.log`; QEMU log `artifacts/qemu-net.log`.

VirtualBox: `scripts/test_virtualbox.sh` gains
`--nic1 nat --nictype1 82540EM`; the existing PASS conditions are unchanged
(the net selftest is non-fatal). Observed `net:` lines are reported in
`progress.md` exactly as seen — no claim is made until the run is observed.

## 10. Limits (restated honestly)

- Static addressing and no routing: only the slirp/NAT subnet is reachable.
- IRQ-assisted RX, service-path dispatch: the handler copies completed
  descriptors into a 16-frame queue; a full queue drops new frames (counted)
  and a burst larger than 32 descriptors per interrupt waits for the next
  RXT0/RXDMT0 cause. Packets are parsed only while a command services the
  queue; there is no background thread. MSI/IOAPIC are absent.
- One datagram at a time for `udpecho`; no sockets, no TCP, no DHCP/DNS.
- Fragmented IPv4 is rejected; UDP payloads over 1024 bytes are dropped;
  frames larger than the 2048-byte RX buffer are dropped by the hardware.
- Tested on QEMU slirp; VirtualBox NAT is reported as observed, best-effort.
- `e1000e` and non-Intel NICs are not supported; bus 0 only.

## 11. Deviations and implementation notes

- **ICMP checksum-ordering defect found by the end-to-end harness** (first
  `test_net_qemu.py` run, 2026-10-18): `send_echo_request` and `send_echo_reply`
  filled the echo payload *after* `icmp::Message::write` had computed the
  checksum, so the transmitted checksum never matched the wire bytes and the
  ICMP echo request was dropped (boot log: `net selftest: no gateway reply
  (echo timeout)`; ARP, which has no checksum, resolved fine). Fixed by writing
  the payload first; the same run then logged `net selftest: passed (arp
  resolved, icmp echo reply in 0 ms)`. Host tests cannot catch this class: the
  bug was call order in the kernel glue, not the host-tested codec.
- **QEMU loopback** (corrected assumption, see §2 and R-012): QEMU's e1000
  model implements only PHY `BMCR` loopback, not `RCTL.LBM`, so
  `net: loopback selftest: unsupported` is the expected QEMU outcome. The
  contract's separate non-fatal outcome line exists for exactly this; ARP/ICMP
  is the gate and passes.
- **VirtualBox observed** (`artifacts/vbox-serial.log`, 7.2.20, 82540EM NAT):
  NIC found (`net: e1000 at 00:03.0 bar0 0xf0000000 size 128 KiB`, MAC
  `08:00:27:c4:5d:0a`), `net: loopback selftest: unsupported`, then
  `net selftest: no gateway reply (arp timeout)` — the headless NAT did not
  answer the gateway ARP within the 3 s budget. Non-fatal by design: the boot
  still reaches the marker (`...,vfs`, without `,net`). The QEMU slirp harness
  is this stage's network evidence; no VirtualBox networking claim is made.
- Counter interpretation for the `net` builtin: the loopback selftest frame is
  a real transmit, so the first `net` output reads `tx 3 frames` (loopback
  frame, ARP request, echo request) and `rx 2 frames` (ARP reply, echo reply)
  with `arp 1/8 entries, errors 0` — expected, recorded for interpretability.
- Host tests: 51 new (`128 → 179`), above the §4 target of 45.

## 12. Phase 9 stage 6: legacy INTx delivery and ownership

This section records the delivery-only step; section 13 supersedes the
handler's no-ring-work behaviour.

The PCI inventory decodes the type-0 `INTERRUPT_LINE`/`INTERRUPT_PIN` bytes at
config offset `0x3C`; a line is usable only with pin 1-4 and line 1-15, so the
reserved timer (0) and COM1 (4) lines are never handed to a device. q35 assigns
the pinned e1000 line 11; VirtualBox assigns line 9.

`kernel/src/irq.rs` owns one handler per line, masks/unmasks through the
8259A (unmasking IRQ2 for slave lines) and sends a non-specific EOI (slave
first). `idt.rs` installs stubs for vectors 32-47; vectors 32 (PIT) and 36
(COM1) keep dedicated paths, the rest dispatch to the registered handler.

The driver claims `Irq { line }` before `net::init`, registers
`E1000::handle_irq`, and unmasks only after the NIC is running. The handler
reads `ICR` (read-to-clear) and increments an event counter; it takes no
lock and does not touch the rings. `net` status appends
`irq {events} event(s)`. Stop masks the line, shuts the NIC down, unregisters
the handler and releases 6 claims; restart registers/claims/unmasks again.

Verified on QEMU q35 with the pinned e1000: 2 interrupt events before the
harness ping, 4 after the restart ping; the restart harness fails if the count
does not grow. VirtualBox boots with line 9 but its isolated NAT has no
gateway, so no delivery count is claimed there. Not claimed: MSI, IOAPIC
routing, interrupt coalescing tuning or shared interrupt lines.

## 13. Phase 9 stage 7: bounded interrupt-driven RX

The stage-6 handler only acknowledged `ICR`. Stage 7 makes it the sole owner
of the hardware RX ring:

- `net::init` runs the loopback selftest **before** publishing the IRQ
  context, so the polled selftest and the handler never advance `rx_next`
  independently.
- `E1000::enable_interrupts` copies the MMIO base, descriptor base, buffer
  table and current tail into a static context, resets the software queue and
  counters, publishes the context, then enables `RXT0|RXDMT0`.
- `E1000::handle_irq` reads `ICR` (read-to-clear), then drains at most one
  full ring (32 descriptors). Each valid completed frame is copied into a
  fixed 1536-byte slot; each descriptor is cleared and returned through `RDT`.
  Hardware errors and non-EOP descriptors are counted and dropped. The loop
  is bounded, takes no lock, allocates nothing and never calls the protocol
  stack.
- The handoff is `os_core::queue::SpscQueue<RxFrame, 16>`: one producer (IRQ)
  and one consumer (`net::drain_rx`), monotonic head/tail counters, release
  publication and acquire observation. A full queue rejects the newest frame;
  the handler counts the drop and keeps returning descriptors. The queue holds
  copies, so a descriptor is recycled immediately after the copy.
- `net::drain_rx` pops at most 8 frames per service pass and dispatches them
  under the existing network lock. ARP/ICMP/UDP handlers are unchanged. With
  no usable IRQ the original `receive_into` polled path is retained.
- Stop masks the line, disables the device, clears the handler context and
  resets the queue, so a restart cannot deliver a pre-stop frame. The `net`
  status line exposes depth, delivered and dropped counters.

**Backlog and drop policy.** The software queue is 16 frames (about 24 KiB of
static BSS). The hardware ring is 32 descriptors. A single interrupt copies at
most 32 frames; if the queue is full, new frames are dropped before a copy and
`dropped` increments. There is no backpressure into the device and no
allocation. Frames already queued are always delivered before newer ones. The
consumer drains 8 per pass, so a full queue takes two passes to empty.

**Evidence (2026-10-22).** Host suite 292 (5 new queue tests, including a
20,000-item two-thread SPSC stress run). QEMU q35: boot selftest delivers 2
frames, `ping 10.0.2.2 -c 3` raises queue delivery 2 -> 5 and IRQ events
2 -> 5 with 0 drops; stop/restart frees/reclaims 19 frames, resets the queue
counters and delivers 2 frames after the restart ping; UDP echo still
round-trips 19 bytes. `scripts/test_driver_model_qemu.sh`,
`scripts/test_driver_restart_qemu.sh`, `scripts/test_net_qemu.sh` and
`scripts/test_all.sh` PASS (29 PASS / 0 FAIL / 2 SKIP). A queue-overflow burst
is proven only by the host queue test; the VM run did not force a drop.
