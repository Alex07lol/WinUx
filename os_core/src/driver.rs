//! Driver registration, deterministic matching and the bind/start/stop state
//! machine (Phase 9, stage 2).
//!
//! Pure, allocation-light logic shared by the kernel and host tests: devices
//! come from the PCI inventory, descriptors declare vendor/device or class
//! match rules, and binding is first-match in descriptor order with optional
//! single-instance drivers. Starting and stopping are explicit transitions;
//! illegal transitions are errors, never silent no-ops. No hardware access and
//! no callbacks live here (the kernel still owns e1000 bring-up).
//!
//! Scope and limits: `docs/DRIVER_MODEL.md`, ADR-0028, R-021.

use alloc::vec::Vec;

/// Maximum registered descriptors.
pub const MAX_DRIVERS: usize = 8;
/// Maximum simultaneous bindings (devices).
pub const MAX_BINDINGS: usize = 64;

/// One PCI function as seen by the driver model.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Device {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub vendor: u16,
    pub device_id: u16,
    pub class: u8,
    pub subclass: u8,
}

/// A match rule: exact IDs or a class/subclass pair.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Match {
    VendorDevice { vendor: u16, device: u16 },
    Class { class: u8, subclass: u8 },
}

impl Match {
    pub fn matches(&self, device: &Device) -> bool {
        match *self {
            Match::VendorDevice { vendor, device: id } => {
                device.vendor == vendor && device.device_id == id
            }
            Match::Class { class, subclass } => {
                device.class == class && device.subclass == subclass
            }
        }
    }
}

/// A registered driver: stable name, one or more match rules, and whether it
/// may bind at most one device (single-instance drivers such as our NIC).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Descriptor {
    pub name: &'static str,
    pub matches: &'static [Match],
    pub single_instance: bool,
}

/// Binding lifecycle. `Failed` records a stable reason string.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Bound,
    Running,
    Failed(&'static str),
    Stopped,
}

/// One device claimed by one descriptor.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Binding {
    pub device: Device,
    pub driver: usize,
    pub state: State,
}

impl Binding {
    /// `Bound -> Running` after a successful start.
    pub fn start(&mut self) -> Result<(), Error> {
        match self.state {
            State::Bound => {
                self.state = State::Running;
                Ok(())
            }
            _ => Err(Error::InvalidTransition),
        }
    }

    /// `Bound | Running -> Failed(reason)`.
    pub fn fail(&mut self, reason: &'static str) -> Result<(), Error> {
        match self.state {
            State::Bound | State::Running => {
                self.state = State::Failed(reason);
                Ok(())
            }
            _ => Err(Error::InvalidTransition),
        }
    }

    /// `Running -> Stopped` after a clean stop.
    pub fn stop(&mut self) -> Result<(), Error> {
        match self.state {
            State::Running => {
                self.state = State::Stopped;
                Ok(())
            }
            _ => Err(Error::InvalidTransition),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Error {
    TooManyDrivers,
    TooManyBindings,
    EmptyName,
    DuplicateName,
    NoMatchRules,
    InvalidTransition,
}

impl Error {
    pub fn label(&self) -> &'static str {
        match self {
            Error::TooManyDrivers => "too many drivers",
            Error::TooManyBindings => "too many bindings",
            Error::EmptyName => "empty driver name",
            Error::DuplicateName => "duplicate driver name",
            Error::NoMatchRules => "driver has no match rules",
            Error::InvalidTransition => "invalid lifecycle transition",
        }
    }
}

/// Bind devices to descriptors: descriptor registration order is priority
/// order, each device gets at most one binding, and a single-instance driver
/// binds at most once per call. Deterministic for identical inputs.
pub fn bind(descriptors: &[Descriptor], devices: &[Device]) -> Result<Vec<Binding>, Error> {
    if descriptors.len() > MAX_DRIVERS {
        return Err(Error::TooManyDrivers);
    }
    for (index, descriptor) in descriptors.iter().enumerate() {
        if descriptor.name.is_empty() {
            return Err(Error::EmptyName);
        }
        if descriptor.matches.is_empty() {
            return Err(Error::NoMatchRules);
        }
        if descriptors[..index]
            .iter()
            .any(|earlier| earlier.name == descriptor.name)
        {
            return Err(Error::DuplicateName);
        }
    }

    let mut bindings: Vec<Binding> = Vec::new();
    for device in devices {
        for (driver, descriptor) in descriptors.iter().enumerate() {
            if !descriptor.matches.iter().any(|rule| rule.matches(device)) {
                continue;
            }
            if descriptor.single_instance && bindings.iter().any(|bound| bound.driver == driver) {
                continue;
            }
            if bindings.len() >= MAX_BINDINGS {
                return Err(Error::TooManyBindings);
            }
            bindings.push(Binding {
                device: *device,
                driver,
                state: State::Bound,
            });
            break;
        }
    }
    Ok(bindings)
}

/// Number of devices without a binding (devices minus bindings; bindings are
/// unique per device by construction).
pub fn unmatched(devices: usize, bindings: usize) -> usize {
    devices.saturating_sub(bindings)
}

#[cfg(test)]
mod tests {
    use super::*;

    const E1000: Descriptor = Descriptor {
        name: "e1000",
        matches: &[Match::VendorDevice {
            vendor: 0x8086,
            device: 0x100E,
        }],
        single_instance: true,
    };
    const VGA: Descriptor = Descriptor {
        name: "vga",
        matches: &[Match::Class {
            class: 0x03,
            subclass: 0x00,
        }],
        single_instance: false,
    };

    fn nic(bus: u8, device: u8, function: u8) -> Device {
        Device {
            bus,
            device,
            function,
            vendor: 0x8086,
            device_id: 0x100E,
            class: 0x02,
            subclass: 0x00,
        }
    }

    fn vga(bus: u8, device: u8) -> Device {
        Device {
            bus,
            device,
            function: 0,
            vendor: 0x1234,
            device_id: 0x1111,
            class: 0x03,
            subclass: 0x00,
        }
    }

    fn bridge() -> Device {
        Device {
            bus: 0,
            device: 0,
            function: 0,
            vendor: 0x8086,
            device_id: 0x29C0,
            class: 0x06,
            subclass: 0x00,
        }
    }

    #[test]
    fn match_rules_are_exact() {
        let device = nic(0, 2, 0);
        assert!(E1000.matches[0].matches(&device));
        assert!(VGA.matches[0].matches(&vga(0, 1)));
        assert!(!VGA.matches[0].matches(&device));
        assert!(!E1000.matches[0].matches(&vga(0, 1)));
    }

    #[test]
    fn binds_first_matching_descriptor_and_counts_unmatched() {
        let devices = [bridge(), vga(0, 1), nic(0, 2, 0)];
        let bindings = bind(&[E1000, VGA], &devices).unwrap();
        assert_eq!(bindings.len(), 2);
        assert_eq!(bindings[0].device, vga(0, 1));
        assert_eq!(bindings[0].driver, 1);
        assert_eq!(bindings[0].state, State::Bound);
        assert_eq!(bindings[1].device, nic(0, 2, 0));
        assert_eq!(bindings[1].driver, 0);
        assert_eq!(unmatched(devices.len(), bindings.len()), 1);
    }

    #[test]
    fn descriptor_order_is_priority_order() {
        let both = Descriptor {
            name: "both",
            matches: &[Match::Class {
                class: 0x03,
                subclass: 0x00,
            }],
            single_instance: false,
        };
        let bindings = bind(&[both, VGA], &[vga(0, 1)]).unwrap();
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].driver, 0);
    }

    #[test]
    fn single_instance_stops_after_one_device() {
        let bindings = bind(&[E1000], &[nic(0, 2, 0), nic(0, 3, 0)]).unwrap();
        assert_eq!(bindings.len(), 1);
        assert_eq!(bindings[0].device.device, 2);
        assert_eq!(unmatched(2, bindings.len()), 1);
    }

    #[test]
    fn no_matching_descriptor_leaves_device_unbound() {
        let bindings = bind(&[E1000], &[bridge(), vga(0, 1)]).unwrap();
        assert!(bindings.is_empty());
        assert_eq!(unmatched(2, 0), 2);
    }

    #[test]
    fn invalid_descriptors_are_rejected() {
        let empty = Descriptor {
            name: "",
            matches: E1000.matches,
            single_instance: false,
        };
        assert_eq!(bind(&[empty], &[]), Err(Error::EmptyName));
        let duplicate = Descriptor {
            name: "e1000",
            matches: VGA.matches,
            single_instance: false,
        };
        assert_eq!(bind(&[E1000, duplicate], &[]), Err(Error::DuplicateName));
        let no_rules = Descriptor {
            name: "nothing",
            matches: &[],
            single_instance: false,
        };
        assert_eq!(bind(&[no_rules], &[]), Err(Error::NoMatchRules));
    }

    #[test]
    fn binding_caps_are_enforced() {
        let devices: Vec<Device> = (0..=MAX_BINDINGS as u8).map(|i| vga(0, i)).collect();
        let descriptors: [Descriptor; MAX_DRIVERS + 1] = core::array::from_fn(|_| VGA);
        assert_eq!(bind(&descriptors, &devices), Err(Error::TooManyDrivers));
        assert_eq!(
            bind(&[VGA], &devices[..MAX_BINDINGS + 1]),
            Err(Error::TooManyBindings)
        );
    }

    #[test]
    fn lifecycle_transitions_are_explicit() {
        let mut binding = bind(&[E1000], &[nic(0, 2, 0)]).unwrap()[0];
        assert_eq!(binding.state, State::Bound);
        assert_eq!(binding.stop(), Err(Error::InvalidTransition));
        binding.start().unwrap();
        assert_eq!(binding.state, State::Running);
        assert_eq!(binding.start(), Err(Error::InvalidTransition));
        binding.stop().unwrap();
        assert_eq!(binding.state, State::Stopped);
        assert_eq!(binding.stop(), Err(Error::InvalidTransition));
        assert_eq!(binding.fail("late failure"), Err(Error::InvalidTransition));
        assert_eq!(binding.start(), Err(Error::InvalidTransition));
    }

    #[test]
    fn failure_can_follow_bound_or_running() {
        let mut bound = bind(&[E1000], &[nic(0, 2, 0)]).unwrap()[0];
        bound.fail("probe failed").unwrap();
        assert_eq!(bound.state, State::Failed("probe failed"));

        let mut running = bind(&[E1000], &[nic(0, 2, 0)]).unwrap()[0];
        running.start().unwrap();
        running.fail("runtime").unwrap();
        assert_eq!(running.state, State::Failed("runtime"));
        assert_eq!(running.fail("again"), Err(Error::InvalidTransition));
    }

    #[test]
    fn binding_is_deterministic() {
        let devices = [bridge(), vga(0, 1), nic(0, 2, 0)];
        let first = bind(&[VGA, E1000], &devices).unwrap();
        let second = bind(&[VGA, E1000], &devices).unwrap();
        assert_eq!(first, second);
        assert_eq!(first[0].driver, 0);
        assert_eq!(first[1].driver, 1);
    }
}
