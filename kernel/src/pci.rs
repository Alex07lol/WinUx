//! PCI configuration-space access (mechanism #1) and a bus-0 scan for the
//! Intel 82540EM NIC (Phase 6, stage 5).
//!
//! Deliberately not a general PCI framework: one scan function and the two
//! register reads/writes the e1000 bring-up needs. Fun facts and the choice of
//! mechanism #1 over MMCONFIG are recorded in `docs/NETWORK.md` and R-012.

use os_core::pci;

use crate::arch::x86_64::io::{inl, inw, outl, outw};

pub const VENDOR_INTEL: u16 = 0x8086;
pub const DEVICE_82540EM: u16 = 0x100E;

/// One located function with its decoded BAR0.
#[derive(Clone, Copy, Debug)]
pub struct PciDevice {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub bar0: pci::Bar,
    pub bar0_size: u64,
}

fn config_address(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    pci::config_address(bus, device, function, offset)
}

pub fn read_u32(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    outl(
        pci::CONFIG_ADDRESS_PORT,
        config_address(bus, device, function, offset),
    );
    inl(pci::CONFIG_DATA_PORT)
}

pub fn write_u32(bus: u8, device: u8, function: u8, offset: u8, value: u32) {
    outl(
        pci::CONFIG_ADDRESS_PORT,
        config_address(bus, device, function, offset),
    );
    outl(pci::CONFIG_DATA_PORT, value);
}

/// The command register is 16 bits at offset 4; the data port returns the low
/// half of the selected dword, so 16-bit port I/O is the exact width.
fn read_command(bus: u8, device: u8, function: u8) -> u16 {
    outl(
        pci::CONFIG_ADDRESS_PORT,
        config_address(bus, device, function, 0x04),
    );
    inw(pci::CONFIG_DATA_PORT)
}

fn write_command(bus: u8, device: u8, function: u8, value: u16) {
    outl(
        pci::CONFIG_ADDRESS_PORT,
        config_address(bus, device, function, 0x04),
    );
    outw(pci::CONFIG_DATA_PORT, value);
}

/// Enable memory-space decoding and bus mastering for a function. Both are
/// required before the e1000 MMIO window or its DMA can work.
pub fn enable_device(bus: u8, device: u8, function: u8) {
    let command =
        read_command(bus, device, function) | pci::COMMAND_MEMORY_SPACE | pci::COMMAND_BUS_MASTER;
    write_command(bus, device, function, command);
}

/// Size BAR0 with the standard all-ones write-back probe and restore the
/// original value immediately. Returns the decoded BAR and its region size.
fn bar0_info(bus: u8, device: u8, function: u8) -> (pci::Bar, u64) {
    const BAR0: u8 = 0x10;
    const BAR1: u8 = 0x14;

    let original_lo = read_u32(bus, device, function, BAR0);
    let is_64_bit = pci::decode_bar(original_lo, 0).kind == pci::BarKind::Memory64;
    let original_hi = if is_64_bit {
        read_u32(bus, device, function, BAR1)
    } else {
        0
    };

    write_u32(bus, device, function, BAR0, 0xFFFF_FFFF);
    if is_64_bit {
        write_u32(bus, device, function, BAR1, 0xFFFF_FFFF);
    }
    let mask_lo = read_u32(bus, device, function, BAR0);
    let mask_hi = if is_64_bit {
        Some(read_u32(bus, device, function, BAR1))
    } else {
        None
    };

    write_u32(bus, device, function, BAR0, original_lo);
    if is_64_bit {
        write_u32(bus, device, function, BAR1, original_hi);
    }

    (
        pci::decode_bar(original_lo, original_hi),
        pci::bar_size(mask_lo, mask_hi),
    )
}

/// Scan bus 0 (all devices, function 0) for the first Intel 82540EM.
pub fn find_e1000() -> Option<PciDevice> {
    for device in 0..32u8 {
        let id = read_u32(0, device, 0, 0x00);
        let (vendor, device_id) = pci::vendor_device(id);
        if vendor == 0xFFFF {
            continue;
        }
        if vendor == VENDOR_INTEL && device_id == DEVICE_82540EM {
            let (bar0, bar0_size) = bar0_info(0, device, 0);
            return Some(PciDevice {
                bus: 0,
                device,
                function: 0,
                bar0,
                bar0_size,
            });
        }
    }
    None
}
