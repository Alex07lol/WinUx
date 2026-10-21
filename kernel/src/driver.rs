//! Driver registration and binding model over the PCI inventory (Phase 9,
//! stage 2).
//!
//! The descriptor table below is the only place a driver's identity is
//! declared. `init` converts the boot PCI snapshot into `os_core::driver`
//! devices, binds them deterministically and validates the result. **No
//! hardware is started or stopped here**: bindings stay in the `Bound` state
//! and the e1000 bring-up still runs directly in `net::init`. Wiring
//! start/stop and resource ownership is stage 3, and the host-tested state
//! machine exists for it (`docs/DRIVER_MODEL.md`, ADR-0028, R-021).

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

use os_core::driver::{self, Binding, Descriptor, Device, Match};

use crate::pci;

const E1000_NAME: &str = "e1000";

static DESCRIPTORS: &[Descriptor] = &[Descriptor {
    name: E1000_NAME,
    matches: &[Match::VendorDevice {
        vendor: pci::VENDOR_INTEL,
        device: pci::DEVICE_82540EM,
    }],
    single_instance: true,
}];

static SELFTEST_PASSED: AtomicBool = AtomicBool::new(false);

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

fn log_bindings(bindings: &[Binding], devices: usize) {
    for binding in bindings {
        crate::kprintln!(
            "driver: {} matched {:02x}:{:02x}.{} {:04x}:{:04x} (state {:?})",
            DESCRIPTORS[binding.driver].name,
            binding.device.bus,
            binding.device.device,
            binding.device.function,
            binding.device.vendor,
            binding.device.device_id,
            binding.state
        );
    }
    crate::kprintln!(
        "driver: {} function(s) unmatched (no registered driver)",
        driver::unmatched(devices, bindings.len())
    );
}

/// Structural checks over one bind result. Returns (bindings, unmatched).
fn check(devices: &[Device], bindings: &[Binding]) -> Result<(usize, usize), &'static str> {
    let mut bound_devices: Vec<(u8, u8, u8)> = Vec::new();
    let mut per_driver = [0usize; driver::MAX_DRIVERS];
    for binding in bindings {
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
        if binding.state != os_core::driver::State::Bound {
            return Err("stage 2 bindings must stay Bound");
        }
        per_driver[binding.driver] += 1;
        if DESCRIPTORS[binding.driver].single_instance && per_driver[binding.driver] > 1 {
            return Err("single-instance driver bound twice");
        }
    }
    let second = driver::bind(DESCRIPTORS, devices).map_err(|_| "rebinding failed")?;
    if second != bindings {
        return Err("binding is not deterministic");
    }
    Ok((
        bindings.len(),
        driver::unmatched(devices.len(), bindings.len()),
    ))
}

/// Bind the registered descriptors to the boot PCI inventory and log the
/// result. A model failure is logged, not fatal; the boot marker records it.
pub fn init() {
    crate::kprintln!("driver: registered {} descriptor(s)", DESCRIPTORS.len());
    let functions = pci::snapshot();
    let devices: Vec<Device> = functions.iter().map(device_of).collect();
    match driver::bind(DESCRIPTORS, &devices) {
        Ok(bindings) => {
            log_bindings(&bindings, devices.len());
            match check(&devices, &bindings) {
                Ok((bound, unmatched)) => {
                    SELFTEST_PASSED.store(true, Ordering::SeqCst);
                    crate::kprintln!(
                        "driver selftest: passed ({} binding(s), {} unmatched; match rules, uniqueness and single-instance verified)",
                        bound,
                        unmatched
                    );
                }
                Err(error) => crate::kprintln!("driver selftest: FAILED ({})", error),
            }
        }
        Err(error) => crate::kprintln!("driver: model failed ({})", error.label()),
    }
}

pub fn selftest_passed() -> bool {
    SELFTEST_PASSED.load(Ordering::SeqCst)
}

/// Shell `drivers`: descriptor table plus the current binding plan.
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
    match driver::bind(DESCRIPTORS, &devices) {
        Ok(bindings) => log_bindings(&bindings, devices.len()),
        Err(error) => crate::kprintln!("drivers: model failed ({})", error.label()),
    }
}
