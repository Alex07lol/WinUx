//! Driver registration, binding and the first real lifecycle-managed device
//! (Phase 9, stages 2–3).
//!
//! The descriptor table below is the only place a driver's identity is
//! declared. `init` converts the boot PCI snapshot into `os_core::driver`
//! devices, binds them deterministically, starts the matched e1000 through
//! `net::init`, and records the MMIO range and DMA frame budget it owns in
//! the resource ledger. A successful start moves the binding to `Running`;
//! a failed start moves it to `Failed(reason)`. There is no stop/restart yet:
//! the MMIO mapping, ring frames and polled network state are kernel-lifetime
//! owned. Scope and limits: `docs/DRIVER_MODEL.md`, ADR-0028/0029, R-021/022.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};

use os_core::driver::{self, Binding, Descriptor, Device, Ledger, Match, Resource, State};

use crate::pci;
use crate::sync::SpinLock;

const E1000_NAME: &str = "e1000";

/// `STATE_NONE` means no driver has started (no binding, or pre-start).
const STATE_NONE: u8 = 0;
const STATE_RUNNING: u8 = 1;
const STATE_FAILED: u8 = 2;
const STATE_STOPPED: u8 = 3;

static DESCRIPTORS: &[Descriptor] = &[Descriptor {
    name: E1000_NAME,
    matches: &[Match::VendorDevice {
        vendor: pci::VENDOR_INTEL,
        device: pci::DEVICE_82540EM,
    }],
    single_instance: true,
}];

static SELFTEST_PASSED: AtomicBool = AtomicBool::new(false);
static STATE: AtomicU8 = AtomicU8::new(STATE_NONE);
static RUNNING_OWNER: AtomicUsize = AtomicUsize::new(usize::MAX);
static HHDM: AtomicU64 = AtomicU64::new(0);
static TSC_HZ: AtomicU64 = AtomicU64::new(0);
static LEDGER: SpinLock<Ledger> = SpinLock::new(Ledger::new());

fn device_of(function: &pci::Function) -> Device {
    Device {
        bus: function.bus,
        device: function.device,
        function: function.function,
        vendor: function.vendor,
        device_id: function.device_id,
        class: function.class,
        subclass: function.subclass,
    }
}

fn pci_device_of(function: &pci::Function) -> Option<pci::PciDevice> {
    let bar0 = function.bars.iter().flatten().find(|bar| bar.index == 0)?;
    Some(pci::PciDevice {
        bus: function.bus,
        device: function.device,
        function: function.function,
        bar0: bar0.bar,
        bar0_size: bar0.size,
    })
}

fn find_function<'a>(
    functions: &'a [pci::Function],
    binding: &Binding,
) -> Option<&'a pci::Function> {
    functions.iter().find(|function| {
        function.bus == binding.device.bus
            && function.device == binding.device.device
            && function.function == binding.device.function
    })
}

/// Claim the matched device's MMIO range and DMA frame budget for the
/// binding. Partial claims are rolled back on error.
fn claim_resources(owner: usize, function: &pci::Function) -> Result<(), driver::Error> {
    let Some(bar0) = function.bars.iter().flatten().find(|bar| bar.index == 0) else {
        return Err(driver::Error::InvalidResource);
    };
    let mut ledger = LEDGER.lock();
    ledger.claim(
        owner,
        Resource::Mmio {
            base: bar0.bar.base,
            len: bar0.size,
        },
    )?;
    if let Err(error) = ledger.claim(
        owner,
        Resource::DmaFrames {
            count: crate::e1000::DMA_FRAME_BUDGET,
        },
    ) {
        ledger.release(owner);
        return Err(error);
    }
    Ok(())
}

fn log_state(name: &str, binding: &Binding) {
    match binding.state {
        State::Running => crate::kprintln!("driver: {} state Running", name),
        State::Failed(reason) => {
            crate::kprintln!("driver: {} state Failed ({})", name, reason)
        }
        State::Bound => crate::kprintln!("driver: {} state Bound", name),
        State::Stopped => crate::kprintln!("driver: {} state Stopped", name),
    }
}

fn log_resources(name: &str, function: &pci::Function) {
    let Some(bar0) = function.bars.iter().flatten().find(|bar| bar.index == 0) else {
        return;
    };
    crate::kprintln!(
        "driver: {} resources mmio {:#x}..{:#x} ({} B), {} DMA frame(s), polled",
        name,
        bar0.bar.base,
        bar0.bar.base + bar0.size,
        bar0.size,
        crate::e1000::DMA_FRAME_BUDGET
    );
}

fn log_unmatched(bindings: &[Binding], devices: usize) {
    crate::kprintln!(
        "driver: {} function(s) unmatched (no registered driver)",
        driver::unmatched(devices, bindings.len())
    );
}

/// Structural checks over one bind/start result. Returns (bindings, unmatched).
fn check(
    devices: &[Device],
    functions: &[pci::Function],
    bindings: &[Binding],
) -> Result<(usize, usize), &'static str> {
    let mut bound_devices: Vec<(u8, u8, u8)> = Vec::new();
    let mut per_driver = [0usize; driver::MAX_DRIVERS];
    for (index, binding) in bindings.iter().enumerate() {
        if binding.driver >= DESCRIPTORS.len() {
            return Err("binding refers to an unknown descriptor");
        }
        let bdf = (
            binding.device.bus,
            binding.device.device,
            binding.device.function,
        );
        if bound_devices.contains(&bdf) {
            return Err("device was bound twice");
        }
        bound_devices.push(bdf);
        if !DESCRIPTORS[binding.driver]
            .matches
            .iter()
            .any(|rule| rule.matches(&binding.device))
        {
            return Err("binding device does not satisfy the descriptor rules");
        }
        if !matches!(binding.state, State::Running | State::Failed(_)) {
            return Err("matched binding was not started or failed");
        }
        per_driver[binding.driver] += 1;
        if DESCRIPTORS[binding.driver].single_instance && per_driver[binding.driver] > 1 {
            return Err("single-instance driver bound twice");
        }
        let function = find_function(functions, binding)
            .ok_or("binding device is missing from the PCI inventory")?;
        let bar0 = function
            .bars
            .iter()
            .flatten()
            .find(|bar| bar.index == 0)
            .ok_or("binding device has no BAR0")?;
        let owner = LEDGER.lock().mmio_owner(bar0.bar.base);
        match binding.state {
            State::Running if owner != Some(index) => {
                return Err("running binding does not own its MMIO range");
            }
            State::Failed(_) if owner == Some(index) => {
                return Err("failed binding still owns its MMIO range");
            }
            _ => {}
        }
    }
    let rebound = driver::bind(DESCRIPTORS, devices).map_err(|_| "rebinding failed")?;
    let same_plan = rebound.len() == bindings.len()
        && rebound.iter().zip(bindings).all(|(fresh, started)| {
            fresh.device == started.device && fresh.driver == started.driver
        });
    if !same_plan {
        return Err("binding is not deterministic");
    }
    Ok((
        bindings.len(),
        driver::unmatched(devices.len(), bindings.len()),
    ))
}

/// Bind the registered descriptors to the boot PCI inventory, start matched
/// devices and log the result. A model or start failure is logged, not fatal;
/// the boot marker records the model self-test outcome.
pub fn init(hhdm_offset: u64, tsc_frequency: Option<u64>) {
    HHDM.store(hhdm_offset, Ordering::SeqCst);
    TSC_HZ.store(tsc_frequency.unwrap_or(0), Ordering::SeqCst);
    crate::kprintln!("driver: registered {} descriptor(s)", DESCRIPTORS.len());
    let functions = pci::snapshot();
    let devices: Vec<Device> = functions.iter().map(device_of).collect();
    let mut bindings = match driver::bind(DESCRIPTORS, &devices) {
        Ok(bindings) => bindings,
        Err(error) => {
            crate::kprintln!("driver: model failed ({})", error.label());
            return;
        }
    };

    for (index, binding) in bindings.iter_mut().enumerate() {
        let name = DESCRIPTORS[binding.driver].name;
        crate::kprintln!(
            "driver: {} matched {:02x}:{:02x}.{} {:04x}:{:04x}",
            name,
            binding.device.bus,
            binding.device.device,
            binding.device.function,
            binding.device.vendor,
            binding.device.device_id
        );
        match start_at(index, binding, &functions, hhdm_offset, tsc_frequency) {
            Ok(()) => STATE.store(STATE_RUNNING, Ordering::SeqCst),
            Err(reason) => start_failed(binding, name, reason),
        }
    }

    log_unmatched(&bindings, devices.len());
    match check(&devices, &functions, &bindings) {
        Ok((bound, unmatched)) => {
            SELFTEST_PASSED.store(true, Ordering::SeqCst);
            crate::kprintln!(
                "driver selftest: passed ({} binding(s), {} unmatched; match rules, uniqueness, single-instance and resource ownership verified)",
                bound,
                unmatched
            );
        }
        Err(error) => crate::kprintln!("driver selftest: FAILED ({})", error),
    }
}

/// Claim a binding's resources and start its device. On failure the claims are
/// rolled back and the caller records `Failed(reason)`.
fn start_at(
    index: usize,
    binding: &mut Binding,
    functions: &[pci::Function],
    hhdm_offset: u64,
    tsc_frequency: Option<u64>,
) -> Result<(), &'static str> {
    let name = DESCRIPTORS[binding.driver].name;
    let function =
        find_function(functions, binding).ok_or("matched function missing from inventory")?;
    let device = pci_device_of(function).ok_or("matched function has no usable BAR0")?;
    claim_resources(index, function).map_err(|error| match error {
        driver::Error::InvalidResource => "invalid resource descriptor",
        _ => "resource conflict",
    })?;
    if let Err(reason) = crate::net::init(&device, hhdm_offset, tsc_frequency) {
        LEDGER.lock().release(index);
        return Err(reason);
    }
    let _ = binding.start();
    RUNNING_OWNER.store(index, Ordering::SeqCst);
    log_state(name, binding);
    log_resources(name, function);
    Ok(())
}

/// Shell `drivers stop`: stop the NIC, release its claims and leave the
/// binding `Stopped`. The HHDM mapping is retained for a later restart.
pub fn stop() -> Result<(), &'static str> {
    if STATE.load(Ordering::SeqCst) != STATE_RUNNING {
        return Err("driver is not running");
    }
    let owner = RUNNING_OWNER.load(Ordering::SeqCst);
    let result = crate::net::shutdown();
    let released = LEDGER.lock().release(owner);
    RUNNING_OWNER.store(usize::MAX, Ordering::SeqCst);
    if let Err(reason) = result {
        STATE.store(STATE_FAILED, Ordering::SeqCst);
        crate::kprintln!("driver: e1000 stop FAILED ({})", reason);
        return Err(reason);
    }
    STATE.store(STATE_STOPPED, Ordering::SeqCst);
    crate::kprintln!("driver: e1000 state Stopped");
    crate::kprintln!(
        "driver: e1000 resources released ({} claim(s); mmio mapping retained)",
        released
    );
    Ok(())
}

/// Shell `drivers start`: restart a stopped NIC through the same claim/start
/// path used at boot. Only a cleanly stopped driver may be restarted.
pub fn start() -> Result<(), &'static str> {
    if STATE.load(Ordering::SeqCst) == STATE_RUNNING {
        return Err("driver is already running");
    }
    if STATE.load(Ordering::SeqCst) != STATE_STOPPED {
        return Err("driver is not stopped");
    }
    let functions = pci::snapshot();
    let devices: Vec<Device> = functions.iter().map(device_of).collect();
    let mut bindings = driver::bind(DESCRIPTORS, &devices).map_err(|_| "model failed")?;
    let Some((index, binding)) = bindings.iter_mut().enumerate().next() else {
        return Err("no registered device to start");
    };
    let name = DESCRIPTORS[binding.driver].name;
    let tsc = match TSC_HZ.load(Ordering::SeqCst) {
        0 => None,
        hz => Some(hz),
    };
    match start_at(index, binding, &functions, HHDM.load(Ordering::SeqCst), tsc) {
        Ok(()) => {
            STATE.store(STATE_RUNNING, Ordering::SeqCst);
            crate::kprintln!("driver: {} restarted (stop/start cycle ok)", name);
            Ok(())
        }
        Err(reason) => {
            start_failed(binding, name, reason);
            Err(reason)
        }
    }
}

fn start_failed(binding: &mut Binding, name: &str, reason: &'static str) {
    if binding.fail(reason).is_ok() {
        crate::kprintln!("driver: {} start failed ({})", name, reason);
    }
    STATE.store(STATE_FAILED, Ordering::SeqCst);
    log_state(name, binding);
}

pub fn selftest_passed() -> bool {
    SELFTEST_PASSED.load(Ordering::SeqCst)
}

/// Shell `drivers`: descriptor table plus the current binding and ownership
/// report. The state shown is the boot-time result of the start attempt.
pub fn print_report() {
    crate::kprintln!("drivers: {} descriptor(s)", DESCRIPTORS.len());
    for descriptor in DESCRIPTORS {
        crate::kprintln!(
            "drivers: {} single-instance {} match rules {}",
            descriptor.name,
            descriptor.single_instance,
            descriptor.matches.len()
        );
    }
    let functions = pci::snapshot();
    let devices: Vec<Device> = functions.iter().map(device_of).collect();
    let mut bindings = match driver::bind(DESCRIPTORS, &devices) {
        Ok(bindings) => bindings,
        Err(error) => {
            crate::kprintln!("drivers: model failed ({})", error.label());
            return;
        }
    };
    for binding in &mut bindings {
        match STATE.load(Ordering::SeqCst) {
            STATE_RUNNING => binding.state = State::Running,
            STATE_FAILED => binding.state = State::Failed("boot start failed"),
            STATE_STOPPED => binding.state = State::Stopped,
            _ => {}
        }
        let name = DESCRIPTORS[binding.driver].name;
        log_state(name, binding);
        if let Some(function) =
            find_function(&functions, binding).filter(|_| binding.state == State::Running)
        {
            log_resources(name, function);
        }
    }
    log_unmatched(&bindings, devices.len());
    crate::kprintln!("drivers: {} resource claim(s)", LEDGER.lock().len());
}
