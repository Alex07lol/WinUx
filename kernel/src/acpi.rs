//! ACPI MADT discovery for the IOAPIC and legacy ISA routes.
//!
//! Limine returns the RSDP as an HHDM virtual address. Every table is checked
//! against the firmware memory map before it is dereferenced. The parser lives
//! in `os_core`; this module only performs discovery and selects the controller
//! and routes the kernel will program. A failure is recoverable: the caller
//! keeps the fixed stage-8 IOAPIC/ISA configuration and, if that also fails,
//! the 8259 path.

use core::slice;

use os_core::acpi::{self, AcpiError, IoApicEntry, Madt};
use os_core::ioapic::{Polarity, TriggerMode};
use os_core::memmap::{MemoryRegion, MemoryType};

use crate::limine;
use crate::sync::SpinLock;

pub const FALLBACK_TIMER_GSI: u8 = 2;
pub const FALLBACK_CONSOLE_GSI: u8 = 4;

const MADT_SIGNATURE: [u8; 4] = *b"APIC";
const RSDP_PROBE_LEN: u64 = 4096;

static INFO: SpinLock<Option<AcpiSummary>> = SpinLock::new(None);

/// A discovered legacy ISA route.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IsaRoute {
    pub gsi: u8,
    pub trigger: TriggerMode,
    pub polarity: Polarity,
}

impl IsaRoute {
    pub const fn trigger_label(self) -> &'static str {
        match self.trigger {
            TriggerMode::Edge => "edge",
            TriggerMode::Level => "level",
        }
    }

    pub const fn polarity_label(self) -> &'static str {
        match self.polarity {
            Polarity::ActiveHigh => "active-high",
            Polarity::ActiveLow => "active-low",
        }
    }
}

/// The validated ACPI facts needed by the interrupt path and diagnostics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AcpiSummary {
    pub rsdp_revision: u8,
    pub uses_xsdt: bool,
    pub table_count: usize,
    pub madt_revision: u8,
    pub local_apic_address: u32,
    pub ioapic_count: usize,
    pub ioapic_id: u8,
    pub ioapic_address: u32,
    pub ioapic_gsi_base: u32,
    pub override_count: usize,
    pub timer: IsaRoute,
    pub console: IsaRoute,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Parser(AcpiError),
    MissingRsdp,
    RsdpAddress,
    AddressOverflow,
    TableRange,
    NoMadt,
    MultipleMadt,
    NoIoApic,
    RouteUnsupported,
}

impl Error {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Parser(error) => error.label(),
            Self::MissingRsdp => "bootloader did not provide the RSDP",
            Self::RsdpAddress => "RSDP is not an HHDM address",
            Self::AddressOverflow => "ACPI address arithmetic overflowed",
            Self::TableRange => "ACPI table is outside an allowed memory-map region",
            Self::NoMadt => "no MADT table",
            Self::MultipleMadt => "multiple MADT tables",
            Self::NoIoApic => "no IOAPIC covers the PIT and COM1 routes",
            Self::RouteUnsupported => "ISA GSI does not fit the IOAPIC interface",
        }
    }
}

/// Discover and validate ACPI tables, returning the selected IOAPIC and routes.
pub fn init(hhdm: u64, regions: &[MemoryRegion]) -> Result<AcpiSummary, Error> {
    let response = limine::get().rsdp.get().ok_or(Error::MissingRsdp)?;
    if response.address.is_null() {
        return Err(Error::MissingRsdp);
    }
    let virtual_address = response.address as u64;
    let physical = virtual_address
        .checked_sub(hhdm)
        .ok_or(Error::RsdpAddress)?;
    if physical == 0 {
        return Err(Error::RsdpAddress);
    }

    let region =
        region_for(regions, physical, acpi::RSDP_MIN_LEN as u64).ok_or(Error::TableRange)?;
    let region_end = region.checked_end().ok_or(Error::AddressOverflow)?;
    let available = region_end - physical;
    let probe_length = core::cmp::min(available, RSDP_PROBE_LEN) as usize;
    let rsdp_bytes = table_bytes(hhdm, regions, physical, probe_length)?;
    let rsdp = acpi::parse_rsdp(rsdp_bytes).map_err(Error::Parser)?;

    let uses_xsdt = matches!(rsdp.xsdt_address, Some(address) if address != 0);
    let root_physical = match rsdp.xsdt_address {
        Some(address) if address != 0 => address,
        _ => u64::from(rsdp.rsdt_address),
    };
    if root_physical == 0 {
        return Err(Error::Parser(AcpiError::NoRootTable));
    }

    let root_length = table_length(hhdm, regions, root_physical)?;
    let root_bytes = table_bytes(hhdm, regions, root_physical, root_length)?;
    let tables = acpi::parse_table_list(root_bytes).map_err(Error::Parser)?;
    let table_count = tables.len();

    let mut madt = None;
    let mut madt_revision = 0;
    for index in 0..tables.len() {
        let address = tables.address(index).ok_or(Error::TableRange)?;
        let length = table_length(hhdm, regions, address)?;
        let bytes = table_bytes(hhdm, regions, address, length)?;
        let header = acpi::parse_sdt_header(bytes).map_err(Error::Parser)?;
        if header.signature == MADT_SIGNATURE {
            if madt.is_some() {
                return Err(Error::MultipleMadt);
            }
            madt_revision = header.revision;
            madt = Some(acpi::parse_madt(bytes).map_err(Error::Parser)?);
        }
    }

    let madt: Madt = madt.ok_or(Error::NoMadt)?;
    let timer = resolve_route(0, FALLBACK_TIMER_GSI, TriggerMode::Edge, &madt)?;
    let console = resolve_route(4, FALLBACK_CONSOLE_GSI, TriggerMode::Level, &madt)?;

    let ioapic = madt
        .ioapics()
        .find(|entry| entry.address != 0 && covers(entry, timer.gsi) && covers(entry, console.gsi))
        .ok_or(Error::NoIoApic)?;

    let summary = AcpiSummary {
        rsdp_revision: rsdp.revision,
        uses_xsdt,
        table_count,
        madt_revision,
        local_apic_address: madt.local_apic_address(),
        ioapic_count: madt.ioapic_count(),
        ioapic_id: ioapic.id,
        ioapic_address: ioapic.address,
        ioapic_gsi_base: ioapic.gsi_base,
        override_count: madt.override_count(),
        timer,
        console,
    };
    *INFO.lock() = Some(summary);
    Ok(summary)
}

pub fn info() -> Option<AcpiSummary> {
    *INFO.lock()
}

fn resolve_route(
    source: u8,
    fallback_gsi: u8,
    fallback_trigger: TriggerMode,
    madt: &Madt,
) -> Result<IsaRoute, Error> {
    match madt.override_for(source) {
        Some(entry) => {
            let gsi = u8::try_from(entry.gsi).map_err(|_| Error::RouteUnsupported)?;
            let trigger = if entry.flags & 0x0c == 0 {
                fallback_trigger
            } else {
                entry.trigger()
            };
            let polarity = if entry.flags & 0x03 == 0 {
                Polarity::ActiveHigh
            } else {
                entry.polarity()
            };
            Ok(IsaRoute {
                gsi,
                trigger,
                polarity,
            })
        }
        None => Ok(IsaRoute {
            gsi: fallback_gsi,
            trigger: fallback_trigger,
            polarity: Polarity::ActiveHigh,
        }),
    }
}

fn covers(entry: &IoApicEntry, gsi: u8) -> bool {
    let gsi = u32::from(gsi);
    gsi >= entry.gsi_base && gsi - entry.gsi_base < 256
}

fn table_memory(kind: MemoryType) -> bool {
    matches!(
        kind,
        MemoryType::AcpiReclaimable | MemoryType::AcpiNvs | MemoryType::ReservedMapped
    )
}

fn region_for(regions: &[MemoryRegion], physical: u64, length: u64) -> Option<&MemoryRegion> {
    if length == 0 {
        return None;
    }
    let end = physical.checked_add(length)?;
    regions.iter().find(|region| {
        if !table_memory(region.kind) || region.base > physical {
            return false;
        }
        match region.checked_end() {
            Some(region_end) => end <= region_end,
            None => false,
        }
    })
}

fn table_length(hhdm: u64, regions: &[MemoryRegion], physical: u64) -> Result<usize, Error> {
    let prefix = table_bytes(hhdm, regions, physical, 8)?;
    let length = acpi::read_sdt_length(prefix).map_err(Error::Parser)?;
    Ok(length as usize)
}

fn table_bytes(
    hhdm: u64,
    regions: &[MemoryRegion],
    physical: u64,
    length: usize,
) -> Result<&'static [u8], Error> {
    if physical == 0 || length == 0 {
        return Err(Error::TableRange);
    }
    let length_u64 = u64::try_from(length).map_err(|_| Error::AddressOverflow)?;
    if region_for(regions, physical, length_u64).is_none() {
        return Err(Error::TableRange);
    }
    let virtual_address = hhdm.checked_add(physical).ok_or(Error::AddressOverflow)?;
    virtual_address
        .checked_add(length_u64)
        .ok_or(Error::AddressOverflow)?;

    // SAFETY: `region_for` proved that the complete byte range lies inside an
    // ACPI reclaimable/NVS or reserved-mapped region from Limine. Limine maps
    // physical memory at `hhdm + physical`, and this kernel never reclaims
    // ACPI memory, so the returned slice stays valid for its lifetime.
    Ok(unsafe { slice::from_raw_parts(virtual_address as *const u8, length) })
}
