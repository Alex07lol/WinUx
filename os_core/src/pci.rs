//! PCI configuration-space addressing and BAR decoding (Phase 6, stage 5).
//!
//! Pure arithmetic shared by the kernel (which performs the actual port I/O
//! through `0xCF8`/`0xCFC`, PCI mechanism #1) and host tests. Only what the
//! e1000 bring-up needs: config addresses, BAR kind/base/size decoding and
//! vendor/device extraction.

/// Mechanism #1 address port.
pub const CONFIG_ADDRESS_PORT: u16 = 0xCF8;
/// Mechanism #1 data port.
pub const CONFIG_DATA_PORT: u16 = 0xCFC;

/// Configuration command register bits used by the network bring-up.
pub const COMMAND_MEMORY_SPACE: u16 = 1 << 1;
pub const COMMAND_BUS_MASTER: u16 = 1 << 2;

/// Build a mechanism #1 configuration address for one dword register.
///
/// Bit 31 is set (enable), `offset` is masked to a dword boundary, and the
/// device/function fields are masked to their legal ranges so a bad caller
/// cannot alias another device.
pub fn config_address(bus: u8, device: u8, function: u8, offset: u8) -> u32 {
    1 << 31
        | ((bus as u32) << 16)
        | (((device & 0x1F) as u32) << 11)
        | (((function & 0x07) as u32) << 8)
        | ((offset & 0xFC) as u32)
}

/// Split a dword read at configuration offset 0 into `(vendor, device)`.
pub fn vendor_device(dword: u32) -> (u16, u16) {
    (dword as u16, (dword >> 16) as u16)
}

/// Split a dword read at configuration offset 8 into `(base_class,
/// subclass, interface)`.
pub fn class_revision(dword: u32) -> (u8, u8, u8) {
    ((dword >> 24) as u8, (dword >> 16) as u8, (dword >> 8) as u8)
}

/// Split a dword read at configuration offset 12 into the 7-bit header-type
/// value and the multifunction flag (bit 7 of the header-type byte).
pub fn header_type(dword: u32) -> (u8, bool) {
    let header = ((dword >> 16) & 0xFF) as u8;
    (header & 0x7F, header & 0x80 != 0)
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BarKind {
    Memory32,
    Memory64,
    Io,
}

/// A decoded Base Address Register. `base` is the zeroed alignment base;
/// the region size comes from [`memory_bar_size`] after an all-ones probe.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Bar {
    pub kind: BarKind,
    pub base: u64,
}

/// Decode BAR low (and high for 64-bit) dwords.
///
/// `hi` is only read for 64-bit memory BARs; pass 0 otherwise.
pub fn decode_bar(lo: u32, hi: u32) -> Bar {
    if lo & 0x1 != 0 {
        // I/O space BAR: bits 31:2 are the base.
        Bar {
            kind: BarKind::Io,
            base: (lo & !0x3) as u64,
        }
    } else if lo & 0x4 != 0 {
        // 10b at bits 2:1: 64-bit memory BAR.
        Bar {
            kind: BarKind::Memory64,
            base: ((lo as u64) & !0xF) | ((hi as u64) << 32),
        }
    } else {
        Bar {
            kind: BarKind::Memory32,
            base: ((lo as u64) & !0xF),
        }
    }
}

/// Region size from the value read back after writing all ones.
///
/// Address bits that are hardwired zero read back as zero; the size is the
/// two's complement of the remaining mask. `hi` participates only for 64-bit
/// memory BARs (`None` for 32-bit/I/O). Returns 0 for an unimplemented BAR.
pub fn bar_size(mask_lo: u32, mask_hi: Option<u32>) -> u64 {
    match mask_hi {
        None => {
            // 32-bit (memory or I/O): wrap the two's complement in 32 bits.
            let size = (!(mask_lo & !0xF)).wrapping_add(1);
            size as u64
        }
        Some(hi) => {
            let combined = ((hi as u64) << 32) | ((mask_lo as u64) & !0xF);
            if combined == 0 {
                return 0;
            }
            (!combined).wrapping_add(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_address_layout() {
        let address = config_address(0, 3, 0, 0x10);
        assert_eq!(address & (1 << 31), 1 << 31);
        assert_eq!((address >> 16) & 0xFF, 0);
        assert_eq!((address >> 11) & 0x1F, 3);
        assert_eq!((address >> 8) & 0x07, 0);
        assert_eq!(address & 0xFF, 0x10);
    }

    #[test]
    fn config_address_masks_fields() {
        // Device 35 folds into 5 bits, function 9 into 3, offset to dword.
        let address = config_address(1, 0x23, 0x09, 0x13);
        assert_eq!((address >> 16) & 0xFF, 1);
        assert_eq!((address >> 11) & 0x1F, 0x03);
        assert_eq!((address >> 8) & 0x07, 0x01);
        assert_eq!(address & 0xFF, 0x10);
    }

    #[test]
    fn vendor_device_split() {
        assert_eq!(vendor_device(0x100E_8086), (0x8086, 0x100E));
        assert_eq!(vendor_device(0xFFFF_FFFF), (0xFFFF, 0xFFFF));
    }

    #[test]
    fn class_revision_split() {
        let (base, subclass, interface) = class_revision(0x0200_0003);
        assert_eq!((base, subclass, interface), (0x02, 0x00, 0x00));
    }

    #[test]
    fn header_type_split() {
        assert_eq!(header_type(0x0000_0000), (0x00, false));
        assert_eq!(header_type(0x0080_0000), (0x00, true));
        assert_eq!(header_type(0x0081_0000), (0x01, true));
        assert_eq!(header_type(0x00FF_0000), (0x7F, true));
    }

    #[test]
    fn decode_32_bit_memory_bar() {
        let bar = decode_bar(0xFEBC_0004 & !0x4, 0);
        assert_eq!(bar.kind, BarKind::Memory32);
        assert_eq!(bar.base, 0xFEBC_0000);
    }

    #[test]
    fn decode_64_bit_memory_bar() {
        let bar = decode_bar(0x0000_0004, 0x0000_0001);
        assert_eq!(bar.kind, BarKind::Memory64);
        assert_eq!(bar.base, 0x1_0000_0000);
    }

    #[test]
    fn decode_io_bar() {
        let bar = decode_bar(0x0000_C001, 0);
        assert_eq!(bar.kind, BarKind::Io);
        assert_eq!(bar.base, 0xC000);
    }

    #[test]
    fn bar_size_128_kib() {
        // 128 KiB: bits 31:17 set, bits 16:4 zero, type bits read zero.
        assert_eq!(bar_size(0xFFFE_0000, None), 128 * 1024);
    }

    #[test]
    fn bar_size_64_bit() {
        assert_eq!(bar_size(0xFFFE_0000, Some(0xFFFF_FFFF)), 128 * 1024);
        assert_eq!(bar_size(0xFFFF_F000, Some(0xFFFF_FFFF)), 4096);
    }

    #[test]
    fn bar_size_minimum_region() {
        // 16 bytes is the smallest memory BAR (alignment fields read zero).
        assert_eq!(bar_size(0xFFFF_FFF0, None), 16);
    }

    #[test]
    fn bar_size_unimplemented() {
        assert_eq!(bar_size(0, None), 0);
        assert_eq!(bar_size(0x0000_000F, None), 0);
    }
}
