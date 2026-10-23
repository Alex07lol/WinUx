//! 82093AA-compatible I/O APIC access (Phase 9 stage 8).
//!
//! The controller is reached through two MMIO registers: `IOREGSEL` selects a
//! 32-bit indirect register and `IOWIN` reads or writes it. Entries are
//! programmed with the host-tested [`os_core::ioapic`] encoding.
//!
//! The ACPI MADT supplies the controller's physical base. `PHYSICAL_BASE` is
//! the fixed fallback used when ACPI discovery is unavailable. Limine does not
//! necessarily map MMIO through the HHDM, so the caller installs a 2 MiB MMIO
//! window first. Only the boot CPU programs entries, and interrupt handlers
//! never touch the indirect registers, so the selector/window pair needs no
//! lock.

use core::ptr::{read_volatile, write_volatile};

use os_core::ioapic::{MASK_BIT, RedirectionEntry};

use super::paging::PageTables;

/// Fixed physical address used when ACPI discovery is unavailable.
pub const PHYSICAL_BASE: u32 = 0xfec0_0000;

const REG_SELECT: usize = 0x00;
const REG_WINDOW: usize = 0x10;
const REG_ID: u32 = 0x00;
const REG_VERSION: u32 = 0x01;
const REDIRECTION_BASE: u32 = 0x10;
const TWO_MIB: u64 = 2 * 1024 * 1024;

/// A probed I/O APIC window.
pub struct IoApic {
    base: *mut u32,
    id: u8,
    version: u8,
    pins: u8,
}

impl IoApic {
    /// Probe a mapped I/O APIC and validate its register file.
    pub fn new(base: *mut u32) -> Result<Self, &'static str> {
        // SAFETY: the caller supplies the kernel virtual address of a mapped,
        // 4 KiB-aligned I/O APIC register window.
        let id = (read_reg(base, REG_ID) >> 24) as u8;
        // SAFETY: as above; the version register is inside the same window.
        let version = read_reg(base, REG_VERSION);
        if version == 0 || version == u32::MAX {
            return Err("IOAPIC version register is not readable");
        }
        let pins = (((version >> 16) & 0xff) + 1) as u8;
        if pins < 16 {
            return Err("IOAPIC reports fewer than 16 inputs");
        }
        Ok(Self {
            base,
            id,
            version: (version & 0xff) as u8,
            pins,
        })
    }

    pub fn id(&self) -> u8 {
        self.id
    }

    pub fn version(&self) -> u8 {
        self.version
    }

    pub fn pins(&self) -> u8 {
        self.pins
    }

    /// Program one redirection-table entry.
    pub fn write_entry(&self, gsi: u8, entry: RedirectionEntry) -> Result<(), &'static str> {
        if gsi >= self.pins {
            return Err("interrupt is outside the IOAPIC input range");
        }
        let register = REDIRECTION_BASE + 2 * gsi as u32;
        // SAFETY: `register` and `register + 1` are the low/high halves of a
        // redirection entry selected through this probed controller.
        write_reg(self.base, register, entry.low());
        // SAFETY: as above; the high half is the next indirect register.
        write_reg(self.base, register + 1, entry.high());
        Ok(())
    }

    /// Mask every input. The IOAPIC may arrive with firmware routes active, so
    /// this runs before any entry is unmasked.
    pub fn mask_all(&self) -> Result<(), &'static str> {
        for gsi in 0..self.pins {
            // SAFETY: `gsi` is inside the probed input range.
            let register = REDIRECTION_BASE + 2 * gsi as u32;
            // SAFETY: the redirection registers are part of the mapped window;
            // a fully masked entry has no useful vector or destination.
            write_reg(self.base, register, MASK_BIT);
            // SAFETY: as above.
            write_reg(self.base, register + 1, 0);
        }
        Ok(())
    }

    /// Mask one input without changing its vector/destination.
    pub fn mask(&self, gsi: u8) -> Result<(), &'static str> {
        self.update_mask(gsi, true)
    }

    /// Unmask one input without changing its vector/destination.
    pub fn unmask(&self, gsi: u8) -> Result<(), &'static str> {
        self.update_mask(gsi, false)
    }

    fn update_mask(&self, gsi: u8, masked: bool) -> Result<(), &'static str> {
        if gsi >= self.pins {
            return Err("interrupt is outside the IOAPIC input range");
        }
        let register = REDIRECTION_BASE + 2 * gsi as u32;
        // SAFETY: the low half is a valid redirection register of this mapped
        // controller; read-modify-write preserves vector and polarity bits.
        let low = read_reg(self.base, register);
        let low = if masked {
            low | MASK_BIT
        } else {
            low & !MASK_BIT
        };
        // SAFETY: as above; only the mask bit changes.
        write_reg(self.base, register, low);
        Ok(())
    }
}

/// Map the selected IOAPIC window through the HHDM and return its virtual base.
pub fn map(hhdm_offset: u64, physical_base: u32) -> Result<*mut u32, &'static str> {
    let physical = u64::from(physical_base);
    if !physical.is_multiple_of(4096) {
        return Err("IOAPIC base is not page aligned");
    }
    let aligned = physical & !(TWO_MIB - 1);
    let offset = physical - aligned;
    if offset + 4096 > TWO_MIB {
        return Err("IOAPIC register window crosses the 2 MiB mapping");
    }
    let virtual_base = hhdm_offset
        .checked_add(aligned)
        .ok_or("IOAPIC HHDM address overflowed")?;
    PageTables::new(hhdm_offset).map_mmio_2m(aligned, virtual_base)?;
    let address = virtual_base
        .checked_add(offset)
        .ok_or("IOAPIC register address overflowed")?;
    Ok(address as *mut u32)
}

#[inline]
fn read_reg(base: *mut u32, register: u32) -> u32 {
    // SAFETY: `base` points at a mapped IOAPIC window; the selector and window
    // offsets are 4-byte aligned and within the first page.
    unsafe {
        write_volatile(base.add(REG_SELECT / 4), register);
        read_volatile(base.add(REG_WINDOW / 4))
    }
}

#[inline]
fn write_reg(base: *mut u32, register: u32, value: u32) {
    // SAFETY: as above; selecting then writing the window is the documented
    // indirect-register access sequence.
    unsafe {
        write_volatile(base.add(REG_SELECT / 4), register);
        write_volatile(base.add(REG_WINDOW / 4), value);
    }
}
