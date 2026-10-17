//! Limine memory-map classification and accounting.
//!
//! The bootloader hands us a sorted array of `(base, length, type)` entries.
//! Nothing in this module frees memory or assumes that a region is mapped; it
//! only classifies and accounts for the entries so the kernel can make an
//! explicit policy decision afterwards.

pub const PAGE_SIZE: u64 = 4096;

/// Region types defined by the Limine boot protocol (PROTOCOL.md, memory map
/// feature). Unknown values are preserved rather than silently treated as
/// usable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MemoryType {
    Usable,
    Reserved,
    AcpiReclaimable,
    AcpiNvs,
    BadMemory,
    BootloaderReclaimable,
    ExecutableAndModules,
    Framebuffer,
    ReservedMapped,
    Unknown(u64),
}

impl MemoryType {
    pub const fn from_raw(raw: u64) -> Self {
        match raw {
            0 => Self::Usable,
            1 => Self::Reserved,
            2 => Self::AcpiReclaimable,
            3 => Self::AcpiNvs,
            4 => Self::BadMemory,
            5 => Self::BootloaderReclaimable,
            6 => Self::ExecutableAndModules,
            7 => Self::Framebuffer,
            8 => Self::ReservedMapped,
            other => Self::Unknown(other),
        }
    }

    pub const fn raw(self) -> u64 {
        match self {
            Self::Usable => 0,
            Self::Reserved => 1,
            Self::AcpiReclaimable => 2,
            Self::AcpiNvs => 3,
            Self::BadMemory => 4,
            Self::BootloaderReclaimable => 5,
            Self::ExecutableAndModules => 6,
            Self::Framebuffer => 7,
            Self::ReservedMapped => 8,
            Self::Unknown(value) => value,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Usable => "usable",
            Self::Reserved => "reserved",
            Self::AcpiReclaimable => "acpi-reclaimable",
            Self::AcpiNvs => "acpi-nvs",
            Self::BadMemory => "bad-memory",
            Self::BootloaderReclaimable => "bootloader-reclaimable",
            Self::ExecutableAndModules => "kernel-and-modules",
            Self::Framebuffer => "framebuffer",
            Self::ReservedMapped => "reserved-mapped",
            Self::Unknown(_) => "unknown",
        }
    }

    /// Only explicitly usable RAM may be handed to the frame allocator. In
    /// particular bootloader-reclaimable memory is *not* free until its
    /// contents have been copied and it is deliberately reclaimed later.
    pub const fn is_usable(self) -> bool {
        matches!(self, Self::Usable)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoryRegion {
    pub base: u64,
    pub length: u64,
    pub kind: MemoryType,
}

impl MemoryRegion {
    pub const fn new(base: u64, length: u64, kind: MemoryType) -> Self {
        Self { base, length, kind }
    }

    pub const fn checked_end(&self) -> Option<u64> {
        self.base.checked_add(self.length)
    }

    pub const fn is_page_aligned(&self) -> bool {
        self.base.is_multiple_of(PAGE_SIZE) && self.length.is_multiple_of(PAGE_SIZE)
    }
}

/// Aggregated facts about the boot memory map. All additions are checked; if
/// an entry overflows the 64-bit address space, `overflowed` is set and the
/// affected total saturates instead of wrapping.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemorySummary {
    pub entries: u64,
    pub total_bytes: u64,
    pub usable_bytes: u64,
    pub usable_regions: u64,
    pub bootloader_reclaimable_bytes: u64,
    pub acpi_bytes: u64,
    pub framebuffer_bytes: u64,
    pub executable_and_modules_bytes: u64,
    pub highest_end: u64,
    pub zero_length_regions: u64,
    pub unaligned_usable_regions: u64,
    pub overflowed: bool,
}

fn saturating_account(total: &mut u64, overflowed: &mut bool, value: u64) {
    match total.checked_add(value) {
        Some(sum) => *total = sum,
        None => {
            *overflowed = true;
            *total = u64::MAX;
        }
    }
}

/// Summarise a memory map. Entries are not required to be sorted for the
/// totals to be correct, but `highest_end` is the maximum end address.
pub fn summarize<I>(regions: I) -> MemorySummary
where
    I: IntoIterator<Item = MemoryRegion>,
{
    let mut summary = MemorySummary::default();

    for region in regions {
        summary.entries = summary.entries.saturating_add(1);
        saturating_account(
            &mut summary.total_bytes,
            &mut summary.overflowed,
            region.length,
        );

        if region.length == 0 {
            summary.zero_length_regions = summary.zero_length_regions.saturating_add(1);
        }

        match region.kind {
            MemoryType::Usable => {
                summary.usable_regions = summary.usable_regions.saturating_add(1);
                saturating_account(
                    &mut summary.usable_bytes,
                    &mut summary.overflowed,
                    region.length,
                );
                if !region.is_page_aligned() {
                    summary.unaligned_usable_regions =
                        summary.unaligned_usable_regions.saturating_add(1);
                }
            }
            MemoryType::BootloaderReclaimable => saturating_account(
                &mut summary.bootloader_reclaimable_bytes,
                &mut summary.overflowed,
                region.length,
            ),
            MemoryType::AcpiReclaimable | MemoryType::AcpiNvs => saturating_account(
                &mut summary.acpi_bytes,
                &mut summary.overflowed,
                region.length,
            ),
            MemoryType::Framebuffer => saturating_account(
                &mut summary.framebuffer_bytes,
                &mut summary.overflowed,
                region.length,
            ),
            MemoryType::ExecutableAndModules => saturating_account(
                &mut summary.executable_and_modules_bytes,
                &mut summary.overflowed,
                region.length,
            ),
            _ => {}
        }

        match region.checked_end() {
            Some(end) => summary.highest_end = summary.highest_end.max(end),
            None => {
                summary.overflowed = true;
                summary.highest_end = u64::MAX;
            }
        }
    }

    summary
}

/// True when `regions` is sorted by base address, as guaranteed for a Limine
/// map. Kept as an explicit runtime check because a bootloader handoff is an
/// external input.
pub fn is_sorted_by_base(regions: &[MemoryRegion]) -> bool {
    regions.windows(2).all(|pair| pair[0].base <= pair[1].base)
}

/// Check that usable regions do not overlap each other. Non-usable entries are
/// allowed to overlap in the Limine protocol, usable ones are not.
pub fn usable_regions_do_not_overlap(regions: &[MemoryRegion]) -> bool {
    let mut usable: [MemoryRegion; 64] = [MemoryRegion::new(0, 0, MemoryType::Reserved); 64];
    let mut count = 0usize;
    for region in regions {
        if !region.kind.is_usable() {
            continue;
        }
        if count == usable.len() {
            return false;
        }
        usable[count] = *region;
        count += 1;
    }
    for i in 1..count {
        if usable[i - 1]
            .checked_end()
            .is_none_or(|end| end > usable[i].base)
        {
            return false;
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn region(base: u64, length: u64, raw: u64) -> MemoryRegion {
        MemoryRegion::new(base, length, MemoryType::from_raw(raw))
    }

    #[test]
    fn classifies_all_known_types() {
        for raw in 0..=8u64 {
            assert_eq!(MemoryType::from_raw(raw).raw(), raw);
        }
        assert_eq!(MemoryType::from_raw(99), MemoryType::Unknown(99));
        assert_eq!(MemoryType::Unknown(99).raw(), 99);
    }

    #[test]
    fn only_usable_type_is_allocatable() {
        for raw in 0..=8u64 {
            let kind = MemoryType::from_raw(raw);
            assert_eq!(kind.is_usable(), raw == 0);
        }
    }

    #[test]
    fn summarises_mixed_map() {
        let map = [
            region(0, 0x1000, 0),
            region(0x1000, 0x9f000, 0),
            region(0x100000, 0x100000, 0),
            region(0x200000, 0x2000, 5),
            region(0x202000, 0x1000, 7),
            region(0x300000, 0x8000, 2),
            region(0x400000, 0x4000, 3),
            region(0x500000, 0x1000, 9),
        ];
        let summary = summarize(map);
        assert_eq!(summary.entries, 8);
        assert_eq!(summary.usable_regions, 3);
        assert_eq!(summary.usable_bytes, 0x1000 + 0x9f000 + 0x100000);
        assert_eq!(summary.bootloader_reclaimable_bytes, 0x2000);
        assert_eq!(summary.framebuffer_bytes, 0x1000);
        assert_eq!(summary.acpi_bytes, 0xc000);
        assert_eq!(summary.highest_end, 0x501000);
        assert!(!summary.overflowed);
        assert_eq!(summary.zero_length_regions, 0);
        assert_eq!(summary.unaligned_usable_regions, 0);
    }

    #[test]
    fn accounts_kernel_and_modules_bytes() {
        let summary = summarize([region(0x100000, 0x22000, 6), region(0, 0x1000, 0)]);
        assert_eq!(summary.executable_and_modules_bytes, 0x22000);
        assert_eq!(summary.usable_bytes, 0x1000);
    }

    #[test]
    fn detects_unaligned_and_zero_length_usable_regions() {
        let summary = summarize([region(1, 0, 0)]);
        assert_eq!(summary.zero_length_regions, 1);
        assert_eq!(summary.unaligned_usable_regions, 1);
        assert_eq!(summary.usable_bytes, 0);
    }

    #[test]
    fn detects_address_space_overflow() {
        let summary = summarize([
            MemoryRegion::new(u64::MAX - 0x10, 0x100, MemoryType::Usable),
            MemoryRegion::new(0, u64::MAX, MemoryType::Reserved),
        ]);
        assert!(summary.overflowed);
        assert_eq!(summary.total_bytes, u64::MAX);
        assert_eq!(summary.highest_end, u64::MAX);
    }

    #[test]
    fn sortedness_and_usable_overlap_checks() {
        let sorted = [
            region(0, 0x1000, 0),
            region(0x2000, 0x1000, 0),
            region(0x3000, 0x1000, 1),
            region(0x3000, 0x1000, 5),
        ];
        assert!(is_sorted_by_base(&sorted));
        assert!(usable_regions_do_not_overlap(&sorted));

        let overlapping = [region(0, 0x2000, 0), region(0x1000, 0x2000, 0)];
        assert!(is_sorted_by_base(&overlapping));
        assert!(!usable_regions_do_not_overlap(&overlapping));

        let unsorted = [region(0x2000, 0x1000, 0), region(0, 0x1000, 0)];
        assert!(!is_sorted_by_base(&unsorted));
    }
}
