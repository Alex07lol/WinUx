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
    /// `Bound | Stopped -> Running` after a successful start or restart.
    pub fn start(&mut self) -> Result<(), Error> {
        match self.state {
            State::Bound | State::Stopped => {
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
    TooManyResources,
    InvalidResource,
    ResourceConflict,
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
            Error::TooManyResources => "too many resource claims",
            Error::InvalidResource => "invalid resource",
            Error::ResourceConflict => "resource conflict",
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

/// Maximum resource claims tracked at once.
pub const MAX_RESOURCES: usize = 16;

/// A resource a running driver owns. MMIO ranges and DMA page ranges are
/// physical address claims and may not overlap each other; IRQ lines are
/// separate exclusive claims.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Resource {
    Mmio { base: u64, len: u64 },
    DmaPages { base: u64, pages: u32 },
    Irq { line: u8 },
}

/// One ledger entry. `owner` is the binding index.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Claim {
    pub owner: usize,
    pub resource: Resource,
}

/// Bounded, allocation-free ownership ledger. The kernel keeps one under a
/// spinlock; host tests exercise conflicts and release directly.
#[derive(Clone, Copy)]
pub struct Ledger {
    claims: [Option<Claim>; MAX_RESOURCES],
    used: usize,
}

impl Default for Ledger {
    fn default() -> Self {
        Self::new()
    }
}

impl Ledger {
    pub const fn new() -> Self {
        Self {
            claims: [None; MAX_RESOURCES],
            used: 0,
        }
    }

    /// Claim one resource for a binding. Physical ranges (MMIO and DMA
    /// pages) may not overlap any existing physical claim; IRQ lines are
    /// exclusive. Retrying the same owner/resource pair is a conflict, not a
    /// silent success.
    pub fn claim(&mut self, owner: usize, resource: Resource) -> Result<(), Error> {
        match resource {
            Resource::Mmio { base, len } => {
                if len == 0 || base.checked_add(len).is_none() {
                    return Err(Error::InvalidResource);
                }
            }
            Resource::DmaPages { base, pages } => {
                let page = crate::frame::PAGE_SIZE;
                if pages == 0 || !base.is_multiple_of(page) {
                    return Err(Error::InvalidResource);
                }
                let len = (pages as u64)
                    .checked_mul(page)
                    .ok_or(Error::InvalidResource)?;
                if base.checked_add(len).is_none() {
                    return Err(Error::InvalidResource);
                }
            }
            Resource::Irq { line } => {
                if self.irq_owner(line).is_some() {
                    return Err(Error::ResourceConflict);
                }
            }
        }
        if let Some((base, len)) = physical_range(&resource)
            && self
                .claims
                .iter()
                .flatten()
                .filter_map(|claim| physical_range(&claim.resource))
                .any(|(other, other_len)| ranges_overlap(base, len, other, other_len))
        {
            return Err(Error::ResourceConflict);
        }
        let Some(slot) = self.claims.iter_mut().find(|slot| slot.is_none()) else {
            return Err(Error::TooManyResources);
        };
        *slot = Some(Claim { owner, resource });
        self.used += 1;
        Ok(())
    }

    /// Drop every claim by `owner`; returns the number released.
    pub fn release(&mut self, owner: usize) -> usize {
        let mut released = 0;
        for slot in &mut self.claims {
            if slot.is_some_and(|claim| claim.owner == owner) {
                *slot = None;
                released += 1;
            }
        }
        self.used -= released;
        released
    }

    /// Binding that owns the MMIO range containing `address`, if any.
    pub fn mmio_owner(&self, address: u64) -> Option<usize> {
        self.claims
            .iter()
            .flatten()
            .find_map(|claim| match claim.resource {
                Resource::Mmio { base, len } if address >= base && address < base + len => {
                    Some(claim.owner)
                }
                _ => None,
            })
    }

    /// Binding that owns the DMA page containing `address`, if any.
    pub fn dma_owner(&self, address: u64) -> Option<usize> {
        self.claims
            .iter()
            .flatten()
            .find_map(|claim| match claim.resource {
                Resource::DmaPages { base, pages } => {
                    let end = base + pages as u64 * crate::frame::PAGE_SIZE;
                    (address >= base && address < end).then_some(claim.owner)
                }
                _ => None,
            })
    }

    /// Total DMA pages claimed by owner (0 if none).
    pub fn dma_pages(&self, owner: usize) -> u32 {
        self.claims
            .iter()
            .flatten()
            .filter_map(|claim| match claim.resource {
                Resource::DmaPages { pages, .. } if claim.owner == owner => Some(pages),
                _ => None,
            })
            .sum()
    }

    pub fn irq_owner(&self, line: u8) -> Option<usize> {
        self.claims
            .iter()
            .flatten()
            .find_map(|claim| match claim.resource {
                Resource::Irq { line: claimed } if claimed == line => Some(claim.owner),
                _ => None,
            })
    }

    pub fn len(&self) -> usize {
        self.used
    }

    pub fn is_empty(&self) -> bool {
        self.used == 0
    }
}

fn ranges_overlap(a_base: u64, a_len: u64, b_base: u64, b_len: u64) -> bool {
    a_base < b_base + b_len && b_base < a_base + a_len
}

fn physical_range(resource: &Resource) -> Option<(u64, u64)> {
    match *resource {
        Resource::Mmio { base, len } => Some((base, len)),
        Resource::DmaPages { base, pages } => Some((base, pages as u64 * crate::frame::PAGE_SIZE)),
        Resource::Irq { .. } => None,
    }
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
        binding.start().unwrap();
        assert_eq!(binding.state, State::Running);
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

    #[test]
    fn ledger_claims_mmio_and_finds_owner() {
        let mut ledger = Ledger::new();
        ledger
            .claim(
                1,
                Resource::Mmio {
                    base: 0x1000,
                    len: 0x2000,
                },
            )
            .unwrap();
        assert_eq!(ledger.len(), 1);
        assert_eq!(ledger.mmio_owner(0x1000), Some(1));
        assert_eq!(ledger.mmio_owner(0x2FFF), Some(1));
        assert_eq!(ledger.mmio_owner(0x3000), None);
        assert_eq!(ledger.mmio_owner(0x0FFF), None);
    }

    #[test]
    fn overlapping_mmio_is_rejected_even_for_the_same_owner() {
        let mut ledger = Ledger::new();
        ledger
            .claim(
                1,
                Resource::Mmio {
                    base: 0x1000,
                    len: 0x1000,
                },
            )
            .unwrap();
        assert_eq!(
            ledger.claim(
                2,
                Resource::Mmio {
                    base: 0x1800,
                    len: 0x10
                }
            ),
            Err(Error::ResourceConflict)
        );
        assert_eq!(
            ledger.claim(
                1,
                Resource::Mmio {
                    base: 0x1000,
                    len: 0x10
                }
            ),
            Err(Error::ResourceConflict)
        );
        ledger
            .claim(
                2,
                Resource::Mmio {
                    base: 0x2000,
                    len: 0x100,
                },
            )
            .unwrap();
    }

    #[test]
    fn irq_lines_are_exclusive_and_released() {
        let mut ledger = Ledger::new();
        ledger.claim(0, Resource::Irq { line: 11 }).unwrap();
        assert_eq!(ledger.irq_owner(11), Some(0));
        assert_eq!(
            ledger.claim(1, Resource::Irq { line: 11 }),
            Err(Error::ResourceConflict)
        );
        assert_eq!(ledger.release(0), 1);
        assert!(ledger.is_empty());
        ledger.claim(1, Resource::Irq { line: 11 }).unwrap();
        assert_eq!(ledger.irq_owner(11), Some(1));
    }

    #[test]
    fn invalid_and_duplicate_resources_are_rejected() {
        let mut ledger = Ledger::new();
        assert_eq!(
            ledger.claim(0, Resource::Mmio { base: 0, len: 0 }),
            Err(Error::InvalidResource)
        );
        assert_eq!(
            ledger.claim(
                0,
                Resource::Mmio {
                    base: u64::MAX,
                    len: 2
                }
            ),
            Err(Error::InvalidResource)
        );
        assert_eq!(
            ledger.claim(
                0,
                Resource::DmaPages {
                    base: 0x1000,
                    pages: 0
                }
            ),
            Err(Error::InvalidResource)
        );
        assert_eq!(
            ledger.claim(
                0,
                Resource::DmaPages {
                    base: 0x1234,
                    pages: 1
                }
            ),
            Err(Error::InvalidResource)
        );
        assert_eq!(
            ledger.claim(
                0,
                Resource::Mmio {
                    base: 0x10,
                    len: 0x10
                }
            ),
            Ok(())
        );
        assert_eq!(
            ledger.claim(
                0,
                Resource::Mmio {
                    base: 0x10,
                    len: 0x10
                }
            ),
            Err(Error::ResourceConflict)
        );
    }

    #[test]
    fn ledger_cap_and_release_are_bounded() {
        let mut ledger = Ledger::new();
        for index in 0..MAX_RESOURCES {
            ledger
                .claim(
                    index,
                    Resource::Mmio {
                        base: index as u64 * 0x100,
                        len: 0x10,
                    },
                )
                .unwrap();
        }
        assert_eq!(
            ledger.claim(MAX_RESOURCES, Resource::Irq { line: 1 }),
            Err(Error::TooManyResources)
        );
        assert_eq!(ledger.release(0), 1);
        assert_eq!(ledger.len(), MAX_RESOURCES - 1);
        assert_eq!(ledger.release(0), 0);
        ledger
            .claim(MAX_RESOURCES, Resource::Irq { line: 1 })
            .unwrap();
    }

    #[test]
    fn dma_pages_are_owned_and_overlap_is_rejected() {
        let mut ledger = Ledger::new();
        ledger
            .claim(
                0,
                Resource::DmaPages {
                    base: 0x10000,
                    pages: 16,
                },
            )
            .unwrap();
        assert_eq!(ledger.dma_owner(0x10000), Some(0));
        assert_eq!(ledger.dma_owner(0x1FFFF), Some(0));
        assert_eq!(ledger.dma_owner(0x20000), None);
        assert_eq!(
            ledger.claim(
                1,
                Resource::DmaPages {
                    base: 0x1F000,
                    pages: 2
                }
            ),
            Err(Error::ResourceConflict)
        );
        assert_eq!(
            ledger.claim(
                1,
                Resource::Mmio {
                    base: 0x10000,
                    len: 0x1000
                }
            ),
            Err(Error::ResourceConflict)
        );
        ledger
            .claim(
                1,
                Resource::DmaPages {
                    base: 0x20000,
                    pages: 1,
                },
            )
            .unwrap();
        assert_eq!(ledger.len(), 2);
        assert_eq!(ledger.release(0), 1);
        assert_eq!(ledger.dma_owner(0x10000), None);
    }

    #[test]
    fn stop_releases_resources_and_restart_reclaims_them() {
        let mut binding = bind(&[E1000], &[nic(0, 2, 0)]).unwrap()[0];
        binding.start().unwrap();
        let mut ledger = Ledger::new();
        ledger
            .claim(
                0,
                Resource::Mmio {
                    base: 0x1000,
                    len: 0x2000,
                },
            )
            .unwrap();
        ledger
            .claim(
                0,
                Resource::DmaPages {
                    base: 0x10000,
                    pages: 19,
                },
            )
            .unwrap();
        assert_eq!(ledger.mmio_owner(0x1000), Some(0));

        binding.stop().unwrap();
        assert_eq!(ledger.release(0), 2);
        assert_eq!(ledger.mmio_owner(0x1000), None);
        assert!(ledger.is_empty());

        binding.start().unwrap();
        assert_eq!(binding.state, State::Running);
        ledger
            .claim(
                0,
                Resource::Mmio {
                    base: 0x1000,
                    len: 0x2000,
                },
            )
            .unwrap();
        ledger
            .claim(
                0,
                Resource::DmaPages {
                    base: 0x10000,
                    pages: 19,
                },
            )
            .unwrap();
        assert_eq!(ledger.len(), 2);
    }
}
