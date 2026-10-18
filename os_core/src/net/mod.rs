//! Host-tested network protocol codecs (Phase 6, stage 5).
//!
//! Pure packet parsing/building for Ethernet, ARP, IPv4, ICMP and UDP, plus
//! the small state machines the kernel stack needs (the ARP cache). No
//! hardware access and no allocation: every buffer is caller-owned, which is
//! what lets the kernel driver feed received frames straight into these
//! parsers.

pub mod arp;
pub mod eth;
pub mod icmp;
pub mod ipv4;
pub mod udp;

/// Read a big-endian `u16` from the first two bytes (callers check length).
pub fn read_be_u16(bytes: &[u8]) -> u16 {
    debug_assert!(bytes.len() >= 2);
    u16::from_be_bytes([bytes[0], bytes[1]])
}

/// Write a big-endian `u16` into the first two bytes (callers check length).
pub fn write_be_u16(bytes: &mut [u8], value: u16) {
    debug_assert!(bytes.len() >= 2);
    bytes[..2].copy_from_slice(&value.to_be_bytes());
}

/// One's-complement 16-bit checksum (RFC 1071), used by IPv4, ICMP and UDP.
///
/// Returned in network order. The caller is responsible for zeroing any
/// checksum field before calling this to compute, and for the
/// "sum-including-field equals zero" convention when validating.
pub fn checksum(bytes: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let (chunks, remainder) = bytes.as_chunks::<2>();
    for [high, low] in chunks {
        sum += u16::from_be_bytes([*high, *low]) as u32;
    }
    if let [last] = remainder {
        sum += (*last as u32) << 8;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}
