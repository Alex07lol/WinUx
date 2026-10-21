# Driver registration and binding model (Phase 9, stage 2)

Status: implemented and verified (2026-10-20). Contract for
`os_core::driver`, `kernel/src/driver.rs`, the shell `drivers` command and
`scripts/test_driver_model_qemu.sh`. Evidence is in section 6; ADR-0028 and
R-021 record the decision and research.

## 1. Scope and non-claims

This stage adds a deterministic **registration, matching and lifecycle state
machine** over the PCI inventory from stage 1. It does **not** start any
driver through the model: bindings stay in state `Bound`, and the e1000
bring-up still runs directly in `net::init`. There is no MMIO/DMA/IRQ
ownership, no device reset through a driver interface, no teardown path, no
hotplug and no virtual GPU.

The model exists so later stages can move real drivers onto explicit
`Bound -> Running -> Stopped`/`Failed` transitions with host-tested rules
instead of ad-hoc bring-up code.

## 2. Model

`os_core::driver` is pure host-testable logic:

- `Device`: bus/device/function, vendor, device ID, class, subclass.
- `Match::VendorDevice` or `Match::Class`: exact match rules.
- `Descriptor`: stable `name`, one or more `Match` rules, and a
  `single_instance` flag.
- `Binding`: device, descriptor index and `State`.
- `State`: `Bound`, `Running`, `Failed(reason)`, `Stopped`.
- `bind(descriptors, devices)`: validates descriptors (non-empty unique
  names, at least one rule, at most 8 descriptors), then binds every device
  to the first matching descriptor in registration order. A single-instance
  descriptor binds at most one device per call. Output is deterministic;
  at most 64 bindings.
- Explicit transitions: `start` (`Bound -> Running`), `stop`
  (`Running -> Stopped`), `fail` (`Bound|Running -> Failed(reason)`). Every
  other transition returns `InvalidTransition`; there are no silent no-ops.
- `unmatched(devices, bindings)` reports unclaimed functions.

Limits are project contracts: 8 descriptors and 64 bindings. The host tests
cover matching, priority order, single-instance behavior, validation errors,
caps, deterministic binding and all lifecycle transitions including illegal
ones.

## 3. Kernel integration

`kernel/src/driver.rs` declares the only descriptor table:

| Driver | Match | Instances |
|---|---|---|
| `e1000` | vendor `8086`, device `100E` | single |

At boot, after the PCI inventory is recorded, `driver::init()` converts the
inventory snapshot into model devices, binds, prints each binding with its
state (always `Bound` in this stage), prints the unmatched count, and runs a
structural self-test: descriptor indices valid, no device bound twice, every
binding satisfies its descriptor's rules, single-instance honored, and a
second bind produces an identical result. A failure is logged but not fatal
and omits the `,driver` boot check.

Boot lines (pinned by the harness on q35 with a pinned e1000):

```text
driver: registered 1 descriptor(s)
driver: e1000 matched 00:02.0 8086:100e (state Bound)
driver: 5 function(s) unmatched (no registered driver)
driver selftest: passed (1 binding(s), 5 unmatched; match rules, uniqueness and single-instance verified)
```

The shell command `drivers` reprints the descriptor table and the current
binding plan. It is diagnostic only.

## 4. Why no e1000 rewrite yet

`net::init` already owns the NIC state (`NET`), ring allocation, MAC read,
loopback self-test and every polled operation. Moving that into a generic
driver interface in the same stage would mix "matching works" with
"start/stop ownership works" and would risk the verified network path with
no hardware-level test for the new interface. Stage 3 should define the
resource ownership contract (BAR mapping, DMA frames, IRQ vs polled mode,
stop/restart) and then move the e1000 path behind it with the network
harness as the regression gate. This stage claims only the host-tested
model and its kernel-side binding report.

## 5. Constraints

- Descriptor identity is compile-time; there is no dynamic loading.
- One device gets one descriptor; no driver stacks or class filters.
- `Bound` is not "working": no probe callback runs in the kernel yet.
- Failure reasons are static strings; no allocation or error codes.
- The model is not a Windows driver model or WDM claim; names are ours.

## 6. Evidence

- `os_core::driver` unit tests (10 new tests; host suite 280/280): exact
  match rules, priority order, single-instance, no-match, duplicate/empty/
  ruleless descriptors, caps, deterministic binding, valid and invalid
  transitions, failure from bound/running.
- `scripts/test_driver_model_qemu.sh` PASS on q35 with a pinned e1000: the
  four boot lines above, the `,driver` marker check and no panic; QEMU exit
  via isa-debug-exit (33).
- `scripts/test_all.sh` reports `PASS=28 FAIL=0 SKIP=2`.
- Recorded session: `artifacts/qemu-driver-session.log`.

## 7. Next stage (not claimed)

Phase 9 stage 3 should add the resource-ownership contract and the first
real lifecycle-managed driver (e1000 start/stop behind the model), with mock
device tests for MMIO/DMA/IRQ ownership and failures; only then should a
virtual GPU target be attempted.
