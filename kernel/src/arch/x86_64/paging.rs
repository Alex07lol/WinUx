//! Narrow page-table support for mapping device MMIO.
//!
//! Limine leaves the kernel running on bootloader-owned 4-level page tables and
//! its HHDM does not necessarily cover device MMIO (the local APIC page is one
//! such hole). This module walks those tables through the HHDM and installs a
//! single uncached 2 MiB mapping. It is deliberately not a general VM manager:
//! it never splits huge pages, never unmaps and never changes CR3.
//!
//! Mappings installed here are permanent and single-CPU; interrupts are still
//! disabled at the point of use.

use core::arch::asm;
use core::sync::atomic::{Ordering, fence};

use os_core::paging as page_math;

use crate::limine;
use crate::memory;

const CR3_ADDRESS_MASK: u64 = 0x000f_ffff_ffff_f000;
const TWO_MIB: u64 = 2 * 1024 * 1024;

fn read_cr3() -> u64 {
    let value: u64;
    // SAFETY: CR3 is readable in ring 0.
    unsafe { asm!("mov {}, cr3", out(reg) value, options(nomem, nostack, preserves_flags)) };
    value
}

fn flush(virtual_address: u64) {
    // SAFETY: `invlpg` is valid in ring 0 for any address; it only drops a TLB
    // entry.
    unsafe {
        asm!(
            "invlpg [{}]",
            in(reg) virtual_address,
            options(nostack, preserves_flags)
        )
    };
    fence(Ordering::SeqCst);
}

pub struct PageTables {
    hhdm: u64,
}

impl PageTables {
    pub const fn new(hhdm: u64) -> Self {
        Self { hhdm }
    }

    fn table(&self, physical: u64) -> Result<*mut u64, &'static str> {
        let virtual_address = self
            .hhdm
            .checked_add(physical)
            .ok_or("page-table HHDM address overflowed")?;
        Ok(virtual_address as *mut u64)
    }

    /// Allocate and zero one page-table frame.
    fn allocate_table(&self) -> Result<u64, &'static str> {
        let frame = memory::alloc_frame()?;
        let physical = frame.address();
        let table = self.table(physical)?;
        // SAFETY: the frame is exclusively owned by this mapping, at least
        // 4 KiB long, and HHDM maps usable RAM.
        unsafe {
            for index in 0..512 {
                core::ptr::write_volatile(table.add(index), 0);
            }
        }
        Ok(physical)
    }

    fn entry(table: *mut u64, index: usize) -> u64 {
        // SAFETY: callers pass a valid table with 512 entries and an index
        // masked to 9 bits.
        unsafe { core::ptr::read_volatile(table.add(index)) }
    }

    fn set_entry(table: *mut u64, index: usize, value: u64) {
        // SAFETY: as above.
        unsafe { core::ptr::write_volatile(table.add(index), value) };
    }

    /// Install an uncached 2 MiB mapping for `physical` at `virtual_address`.
    ///
    /// Both addresses must be 2 MiB aligned. An existing 4 KiB page table at
    /// the PD level is handled by installing a 4 KiB mapping instead of
    /// splitting anything.
    pub fn map_mmio_2m(&self, physical: u64, virtual_address: u64) -> Result<(), &'static str> {
        let paging_mode = limine::get()
            .paging_mode
            .get()
            .map(|response| response.mode);
        if paging_mode != Some(limine::PAGING_MODE_X86_64_4LVL) {
            return Err("MMIO mapping requires 4-level paging");
        }
        if !physical.is_multiple_of(TWO_MIB) || !virtual_address.is_multiple_of(TWO_MIB) {
            return Err("MMIO mapping is not 2 MiB aligned");
        }
        if !page_math::is_canonical_4level(virtual_address) {
            return Err("MMIO mapping virtual address is not canonical");
        }

        let indices = page_math::indices_4level(virtual_address);
        let mut table_physical = read_cr3() & CR3_ADDRESS_MASK;

        for (index, level) in [
            (indices.pml4, "PML4"),
            (indices.pdpt, "PDPT"),
            (indices.pd, "PD"),
        ] {
            let table = self.table(table_physical)?;
            let entry = Self::entry(table, index);
            if page_math::entry_present(entry) {
                if level == "PDPT" && page_math::entry_huge(entry) {
                    // Already covered by a 1 GiB page mapping.
                    return Ok(());
                }
                if level == "PD" && page_math::entry_huge(entry) {
                    // Already covered by a 2 MiB page mapping.
                    return Ok(());
                }
                table_physical = page_math::entry_address(entry);
                continue;
            }

            if level == "PD" {
                let new_entry = page_math::make_entry(physical, page_math::MMIO_FLAGS_2M)
                    .ok_or("invalid 2 MiB MMIO entry")?;
                Self::set_entry(table, index, new_entry);
                fence(Ordering::SeqCst);
                flush(virtual_address);
                return Ok(());
            }

            let new_table = self.allocate_table()?;
            let new_entry =
                page_math::make_entry(new_table, page_math::PRESENT | page_math::WRITABLE)
                    .ok_or("invalid page-table entry")?;
            Self::set_entry(table, index, new_entry);
            fence(Ordering::SeqCst);
            table_physical = new_table;
        }

        // The PD entry exists as a page table: install a 4 KiB mapping.
        let table = self.table(table_physical)?;
        let entry = Self::entry(table, indices.pt);
        if page_math::entry_present(entry) {
            return Ok(());
        }
        let new_entry = page_math::make_entry(physical, page_math::MMIO_FLAGS)
            .ok_or("invalid 4 KiB MMIO entry")?;
        Self::set_entry(table, indices.pt, new_entry);
        fence(Ordering::SeqCst);
        flush(virtual_address);
        Ok(())
    }
}
