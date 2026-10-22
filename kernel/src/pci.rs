//! PCI configuration-space access (mechanism #1), the bus-0 device inventory
//! command-register enable (Phase 6 stage 5; inventory added in Phase 9 stage
//! 1; the e1000 locator was removed in stage 3 when the driver model began
//! passing the matched function to `net::init`).
//!
//! The inventory scans bus 0, honours the multifunction bit and probes BARs
//! with the standard all-ones write-back (restoring every value immediately).
//! It is deliberately not a driver framework: no bridge traversal, resource
//! assignment, interrupt routing or device matching lives here yet. Scope,
//! limits and evidence are in `docs/PCI.md`, R-020 and ADR-0027.

use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering};

use os_core::pci;

use crate::arch::x86_64::io::{inl, inw, outl, outw};
use crate::sync::SpinLock;

pub const VENDOR_INTEL: u16 = 0x8086;
pub const DEVICE_82540EM: u16 = 0x100E;

/// Upper bound on recorded functions; exceeding it is reported, never hidden.
pub const MAX_FUNCTIONS: usize = 64;
const BAR_SLOTS: usize = 6;

static INVENTORY: SpinLock<Vec<Function>> = SpinLock::new(Vec::new());
static TRUNCATED: AtomicBool = AtomicBool::new(false);
static SELFTEST_PASSED: AtomicBool = AtomicBool::new(false);

/// One decoded Base Address Register.
#[derive(Clone, Copy, Debug)]
pub struct BarInfo {
    pub index: u8,
    pub bar: pci::Bar,
    pub size: u64,
}

/// One present PCI function (vendor != 0xFFFF).
#[derive(Clone, Copy, Debug)]
pub struct Function {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub vendor: u16,
    pub device_id: u16,
    pub class: u8,
    pub subclass: u8,
    pub interface: u8,
    pub header: u8,
    pub bar_count: u8,
    pub bars: [Option<BarInfo>; BAR_SLOTS],
    /// PCI interrupt line (0..15) and pin (1=A..4=D); both zero when the
    /// function has no legacy INTx assignment.
    pub interrupt_line: u8,
    pub interrupt_pin: u8,
}

impl Function {
    /// Legacy INTx line if the firmware assigned a usable line (1..15) and
    /// the function declares an interrupt pin.
    pub fn irq_line(&self) -> Option<u8> {
        (self.interrupt_pin != 0
            && self.interrupt_pin <= 4
            && self.interrupt_line > 0
            && self.interrupt_line < 16)
            .then_some(self.interrupt_line)
    }
}

/// One located function with its decoded BAR0 (e1000 bring-up shape).
#[derive(Clone, Copy, Debug)]
pub struct PciDevice {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub bar0: pci::Bar,
    pub bar0_size: u64,
    /// Legacy INTx line when the firmware assigned one, else None.
    pub irq_line: Option<u8>,
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

/// Size one BAR with the all-ones write-back probe and restore the original
/// value immediately. Returns `None` for an unimplemented BAR (size zero).
fn probe_bar(bus: u8, device: u8, function: u8, index: u8) -> Option<BarInfo> {
    let offset = 0x10 + index * 4;
    let original_lo = read_u32(bus, device, function, offset);
    let is_64_bit = pci::decode_bar(original_lo, 0).kind == pci::BarKind::Memory64;
    let original_hi = if is_64_bit {
        read_u32(bus, device, function, offset + 4)
    } else {
        0
    };

    write_u32(bus, device, function, offset, 0xFFFF_FFFF);
    if is_64_bit {
        write_u32(bus, device, function, offset + 4, 0xFFFF_FFFF);
    }
    let mask_lo = read_u32(bus, device, function, offset);
    let mask_hi = if is_64_bit {
        Some(read_u32(bus, device, function, offset + 4))
    } else {
        None
    };

    write_u32(bus, device, function, offset, original_lo);
    if is_64_bit {
        write_u32(bus, device, function, offset + 4, original_hi);
    }

    let size = pci::bar_size(mask_lo, mask_hi);
    if size == 0 {
        return None;
    }
    Some(BarInfo {
        index,
        bar: pci::decode_bar(original_lo, original_hi),
        size,
    })
}

/// Read one function into the inventory. `None` means no device is present
/// (vendor 0xFFFF). BAR probing happens only for present functions.
fn scan_function(bus: u8, device: u8, function: u8) -> Option<Function> {
    let id = read_u32(bus, device, function, 0x00);
    let (vendor, device_id) = pci::vendor_device(id);
    if vendor == 0xFFFF {
        return None;
    }
    let (header, _multifunction) = pci::header_type(read_u32(bus, device, function, 0x0C));
    let (class, subclass, interface) = pci::class_revision(read_u32(bus, device, function, 0x08));
    let bar_slots = match header {
        0x00 => 6,
        0x01 => 2,
        0x02 => 1,
        _ => 0,
    };
    let mut bars = [None; BAR_SLOTS];
    let mut index = 0;
    let mut recorded = 0u8;
    while index < bar_slots {
        let info = probe_bar(bus, device, function, index);
        let is_64_bit = match info {
            Some(info) => {
                bars[index as usize] = Some(info);
                recorded += 1;
                info.bar.kind == pci::BarKind::Memory64
            }
            None => {
                // An unimplemented BAR still occupies its slot; stop there.
                false
            }
        };
        index += if is_64_bit { 2 } else { 1 };
    }
    let (interrupt_line, interrupt_pin) = if header == 0 {
        interrupt_assignment(bus, device, function)
    } else {
        (0, 0)
    };
    Some(Function {
        bus,
        device,
        function,
        vendor,
        device_id,
        class,
        subclass,
        interface,
        header,
        bar_count: recorded,
        bars,
        interrupt_line,
        interrupt_pin,
    })
}

/// Decode the type-0 INTERRUPT_LINE/INTERRUPT_PIN bytes at 0x3C.
fn interrupt_assignment(bus: u8, device: u8, function: u8) -> (u8, u8) {
    let value = read_u32(bus, device, function, 0x3C);
    ((value & 0xFF) as u8, ((value >> 8) & 0xFF) as u8)
}

fn push(function: Function) -> bool {
    let mut inventory = INVENTORY.lock();
    if inventory.len() >= MAX_FUNCTIONS {
        TRUNCATED.store(true, Ordering::SeqCst);
        return false;
    }
    inventory.push(function);
    true
}

/// Enumerate bus 0 (all 32 devices, functions only beyond 0 when the
/// multifunction bit is set), record the inventory and run its self-test.
pub fn init() {
    {
        let mut inventory = INVENTORY.lock();
        inventory.clear();
    }
    TRUNCATED.store(false, Ordering::SeqCst);
    for device in 0..32u8 {
        let Some(first) = scan_function(0, device, 0) else {
            continue;
        };
        let multifunction = pci::header_type(read_u32(0, device, 0, 0x0C)).1;
        if !push(first) {
            break;
        }
        if multifunction {
            for function in 1..8u8 {
                if let Some(function) = scan_function(0, device, function)
                    && !push(function)
                {
                    break;
                }
            }
        }
    }

    let (functions, bars) = {
        let inventory = INVENTORY.lock();
        (
            inventory.len(),
            inventory
                .iter()
                .map(|f| f.bar_count as usize)
                .sum::<usize>(),
        )
    };
    let truncated = TRUNCATED.load(Ordering::SeqCst);
    crate::kprintln!(
        "pci: enumerated {} function(s), {} BAR(s) on bus 0{}",
        functions,
        bars,
        if truncated { " (truncated)" } else { "" }
    );

    print_devices();

    match selftest() {
        Ok((functions, bars)) => {
            SELFTEST_PASSED.store(true, Ordering::SeqCst);
            crate::kprintln!(
                "pci selftest: passed ({} function(s), {} BAR(s); vendors present, BAR sizes power-of-two, no duplicate function)",
                functions,
                bars
            );
        }
        Err(error) => crate::kprintln!("pci selftest: FAILED ({})", error),
    }
}

/// Verify the recorded inventory's invariants and return its totals.
pub fn selftest() -> Result<(usize, usize), &'static str> {
    if TRUNCATED.load(Ordering::SeqCst) {
        return Err("inventory truncated");
    }
    let inventory = INVENTORY.lock();
    if inventory.is_empty() {
        return Err("no PCI functions on bus 0");
    }
    let mut bars = 0usize;
    for (index, function) in inventory.iter().enumerate() {
        if function.vendor == 0xFFFF {
            return Err("missing vendor was recorded");
        }
        if inventory[..index].iter().any(|other| {
            (other.bus, other.device, other.function)
                == (function.bus, function.device, function.function)
        }) {
            return Err("duplicate function was recorded");
        }
        let mut recorded = 0u8;
        for info in function.bars.iter().flatten() {
            if info.size == 0 || !info.size.is_power_of_two() {
                return Err("BAR size is not a power of two");
            }
            recorded += 1;
            bars += 1;
        }
        if recorded != function.bar_count {
            return Err("BAR count does not match the recorded bars");
        }
    }
    Ok((inventory.len(), bars))
}

pub fn selftest_passed() -> bool {
    SELFTEST_PASSED.load(Ordering::SeqCst)
}

/// Copy of the boot inventory for consumers that must not hold the lock
/// (currently the driver model).
pub fn snapshot() -> Vec<Function> {
    INVENTORY.lock().clone()
}

fn bar_kind_name(kind: pci::BarKind) -> &'static str {
    match kind {
        pci::BarKind::Memory32 => "mem32",
        pci::BarKind::Memory64 => "mem64",
        pci::BarKind::Io => "io",
    }
}

/// Print every recorded function and its BARs (boot log and shell `pci`).
pub fn print_devices() {
    let inventory = INVENTORY.lock();
    for function in inventory.iter() {
        crate::kprintln!(
            "pci: {:02x}:{:02x}.{} {:04x}:{:04x} class {:02x}:{:02x}:{:02x} hdr {:02x}",
            function.bus,
            function.device,
            function.function,
            function.vendor,
            function.device_id,
            function.class,
            function.subclass,
            function.interface,
            function.header
        );
        for info in function.bars.iter().flatten() {
            crate::kprintln!(
                "pci:   bar{} {} base={:#x} size={:#x}",
                info.index,
                bar_kind_name(info.bar.kind),
                info.bar.base,
                info.size
            );
        }
    }
    crate::kprintln!(
        "pci: {} function(s), {} BAR(s) on bus 0",
        inventory.len(),
        inventory
            .iter()
            .map(|f| f.bar_count as usize)
            .sum::<usize>()
    );
}
