//! IOAPIC redirection-entry encoding (Phase 9 stage 8).
//!
//! This module models the 64-bit redirection-table entry described by the
//! Intel 64 SDM and the 82093AA I/O APIC specification. Only the subset the
//! kernel currently programs is exposed: fixed delivery, physical destination,
//! and an 8-bit destination APIC ID. Edge/level trigger and active-high/low
//! polarity are explicit so the PIT and COM1 routes cannot be confused.
//!
//! The kernel supplies the MMIO side; keeping the bit layout here makes the
//! encoding host-testable before it is written to hardware.

/// Lowest vector the kernel uses for hardware interrupts.
pub const MIN_VECTOR: u8 = 32;

/// Mask bit in the low redirection-entry word.
pub const MASK_BIT: u32 = 1 << 16;

const POLARITY_LOW_BIT: u32 = 1 << 13;
const TRIGGER_LEVEL_BIT: u32 = 1 << 15;

/// How the IOAPIC input is triggered.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TriggerMode {
    Edge,
    Level,
}

/// Electrical polarity of the IOAPIC input.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Polarity {
    ActiveHigh,
    ActiveLow,
}

/// A fixed-delivery, physical-destination IOAPIC redirection entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RedirectionEntry {
    low: u32,
    high: u32,
}

impl RedirectionEntry {
    /// Build an entry for `vector` and `destination` (local APIC ID).
    ///
    /// Returns `None` for vectors below [`MIN_VECTOR`]; the architecture
    /// reserves 0..=31 for exceptions and the spurious vector.
    pub const fn fixed(
        vector: u8,
        destination: u8,
        trigger: TriggerMode,
        polarity: Polarity,
        masked: bool,
    ) -> Option<Self> {
        if vector < MIN_VECTOR {
            return None;
        }
        let mut low = vector as u32;
        if matches!(polarity, Polarity::ActiveLow) {
            low |= POLARITY_LOW_BIT;
        }
        if matches!(trigger, TriggerMode::Level) {
            low |= TRIGGER_LEVEL_BIT;
        }
        if masked {
            low |= MASK_BIT;
        }
        Some(Self {
            low,
            high: (destination as u32) << 24,
        })
    }

    /// Low 32 bits (`IOREDTBL` even register).
    pub const fn low(self) -> u32 {
        self.low
    }

    /// High 32 bits (`IOREDTBL` odd register); bits 24..31 are the destination.
    pub const fn high(self) -> u32 {
        self.high
    }

    /// Interrupt vector.
    pub const fn vector(self) -> u8 {
        (self.low & 0xff) as u8
    }

    /// Destination local-APIC ID.
    pub const fn destination(self) -> u8 {
        (self.high >> 24) as u8
    }

    /// Whether the entry is masked.
    pub const fn is_masked(self) -> bool {
        self.low & MASK_BIT != 0
    }

    /// Copy of this entry with the mask bit set or cleared.
    pub const fn with_mask(self, masked: bool) -> Self {
        let low = if masked {
            self.low | MASK_BIT
        } else {
            self.low & !MASK_BIT
        };
        Self { low, ..self }
    }
}

#[cfg(test)]
mod tests {
    use super::{MASK_BIT, Polarity, RedirectionEntry, TriggerMode};

    #[test]
    fn encodes_edge_high_timer_entry() {
        let entry =
            RedirectionEntry::fixed(32, 0, TriggerMode::Edge, Polarity::ActiveHigh, false).unwrap();
        assert_eq!(entry.low(), 0x20);
        assert_eq!(entry.high(), 0);
        assert_eq!(entry.vector(), 32);
        assert_eq!(entry.destination(), 0);
        assert!(!entry.is_masked());
    }

    #[test]
    fn encodes_level_low_pci_entry() {
        let entry =
            RedirectionEntry::fixed(43, 0xa5, TriggerMode::Level, Polarity::ActiveLow, false)
                .unwrap();
        assert_eq!(entry.low(), 0xa02b);
        assert_eq!(entry.high(), 0xa500_0000);
        assert_eq!(entry.vector(), 43);
        assert_eq!(entry.destination(), 0xa5);
    }

    #[test]
    fn mask_bit_is_independent_of_trigger_and_polarity() {
        let entry =
            RedirectionEntry::fixed(36, 1, TriggerMode::Level, Polarity::ActiveHigh, true).unwrap();
        assert_eq!(entry.low(), 36 | (1 << 15) | MASK_BIT);
        assert!(entry.is_masked());
        let unmasked = entry.with_mask(false);
        assert_eq!(unmasked.low(), 36 | (1 << 15));
        assert_eq!(unmasked.high(), 1 << 24);
    }

    #[test]
    fn rejects_reserved_vectors() {
        assert!(
            RedirectionEntry::fixed(31, 0, TriggerMode::Edge, Polarity::ActiveHigh, false)
                .is_none()
        );
        assert!(
            RedirectionEntry::fixed(0, 0, TriggerMode::Edge, Polarity::ActiveHigh, false).is_none()
        );
    }

    #[test]
    fn accepts_every_hardware_vector_and_destination() {
        for vector in 32..=255 {
            let entry = RedirectionEntry::fixed(
                vector,
                255,
                TriggerMode::Edge,
                Polarity::ActiveHigh,
                false,
            )
            .unwrap();
            assert_eq!(entry.vector(), vector);
            assert_eq!(entry.destination(), 255);
        }
    }
}
