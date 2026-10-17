//! x86-64 4-level page-table index and entry math.
//!
//! This module is pure arithmetic so the kernel's page-table walker can be
//! host-tested. It deliberately does not touch memory: mapping, TLB
//! invalidation and CR3 access stay in the kernel.

/// Standard 4 KiB page size.
pub const PAGE_SIZE: u64 = 4096;

/// Physical-address field of a page-table entry (bits 12..51).
pub const ENTRY_ADDRESS_MASK: u64 = 0x000f_ffff_ffff_f000;

pub const PRESENT: u64 = 1 << 0;
pub const WRITABLE: u64 = 1 << 1;
pub const WRITE_THROUGH: u64 = 1 << 3;
pub const CACHE_DISABLE: u64 = 1 << 4;
pub const HUGE_PAGE: u64 = 1 << 7;

/// Flags for an uncached MMIO mapping.
pub const MMIO_FLAGS: u64 = PRESENT | WRITABLE | WRITE_THROUGH | CACHE_DISABLE;
/// Flags for a 2 MiB uncached MMIO mapping.
pub const MMIO_FLAGS_2M: u64 = MMIO_FLAGS | HUGE_PAGE;

/// Indices into PML4, PDPT, PD and PT for a 4-level virtual address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TableIndices {
    pub pml4: usize,
    pub pdpt: usize,
    pub pd: usize,
    pub pt: usize,
}

pub fn indices_4level(address: u64) -> TableIndices {
    TableIndices {
        pml4: ((address >> 39) & 0x1ff) as usize,
        pdpt: ((address >> 30) & 0x1ff) as usize,
        pd: ((address >> 21) & 0x1ff) as usize,
        pt: ((address >> 12) & 0x1ff) as usize,
    }
}

/// A 4-level x86-64 address is canonical when bits 48..63 repeat bit 47.
pub fn is_canonical_4level(address: u64) -> bool {
    let sign = address >> 47;
    sign == 0 || sign == 0x1_ffff
}

pub fn entry_address(entry: u64) -> u64 {
    entry & ENTRY_ADDRESS_MASK
}

pub fn entry_present(entry: u64) -> bool {
    entry & PRESENT != 0
}

pub fn entry_huge(entry: u64) -> bool {
    entry & HUGE_PAGE != 0
}

/// Build a page-table entry from a physical address and flags.
///
/// Returns `None` for unaligned addresses or addresses outside the supported
/// physical range, so callers cannot install a malformed entry.
pub fn make_entry(physical: u64, flags: u64) -> Option<u64> {
    if !physical.is_multiple_of(PAGE_SIZE) || physical > ENTRY_ADDRESS_MASK {
        return None;
    }
    Some(physical | flags)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_4level_indices() {
        assert_eq!(
            indices_4level(0),
            TableIndices {
                pml4: 0,
                pdpt: 0,
                pd: 0,
                pt: 0
            }
        );
        // HHDM virtual base plus the LAPIC page: 0xffff8000_fee00000.
        let hhdm_lapic = 0xffff_8000_fee0_0000u64;
        let indices = indices_4level(hhdm_lapic);
        assert_eq!(indices.pml4, 256);
        assert_eq!(indices.pdpt, 3);
        assert_eq!(indices.pd, 503);
        assert_eq!(indices.pt, 0);
        assert!(is_canonical_4level(hhdm_lapic));
    }

    #[test]
    fn rejects_non_canonical_addresses() {
        assert!(!is_canonical_4level(0x0000_8000_0000_0000));
        assert!(is_canonical_4level(0xffff_ffff_ffff_ffff));
    }

    #[test]
    fn builds_entries_only_for_aligned_physical_addresses() {
        assert_eq!(
            make_entry(0xfe00_0000, MMIO_FLAGS_2M),
            Some(0xfe00_0000 | MMIO_FLAGS_2M)
        );
        assert_eq!(make_entry(0xfe00_0001, MMIO_FLAGS), None);
        assert_eq!(make_entry(!0xfff_u64, MMIO_FLAGS), None);
        // Bits above the 52-bit physical-address field are masked off.
        assert_eq!(entry_address(0x1234_5678_9abc_d000), 0x0004_5678_9abc_d000);
        assert_eq!(
            entry_address(0x0000_0000_0abc_dfff) & !ENTRY_ADDRESS_MASK,
            0
        );
    }

    #[test]
    fn huge_and_mmio_flag_composition_is_explicit() {
        assert_eq!(MMIO_FLAGS_2M & HUGE_PAGE, HUGE_PAGE);
        assert_eq!(MMIO_FLAGS_2M & CACHE_DISABLE, CACHE_DISABLE);
        assert_eq!(MMIO_FLAGS_2M & WRITE_THROUGH, WRITE_THROUGH);
        assert_eq!(MMIO_FLAGS & HUGE_PAGE, 0);
    }
}
