//! UDP datagrams (RFC 768) with optional IPv4 pseudo-header checksums.

use super::ipv4::Addr;

pub const HEADER_LEN: usize = 8;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Header {
    pub source_port: u16,
    pub destination_port: u16,
    pub length: u16,
}

impl Header {
    /// Parse a datagram header and return it with the payload.
    pub fn parse(datagram: &[u8]) -> Option<(Header, &[u8])> {
        if datagram.len() < HEADER_LEN {
            return None;
        }
        let length = u16::from_be_bytes([datagram[4], datagram[5]]);
        if (length as usize) < HEADER_LEN || (length as usize) > datagram.len() {
            return None;
        }
        let header = Header {
            source_port: u16::from_be_bytes([datagram[0], datagram[1]]),
            destination_port: u16::from_be_bytes([datagram[2], datagram[3]]),
            length,
        };
        Some((header, &datagram[HEADER_LEN..length as usize]))
    }

    /// Write a header whose length field covers the datagram. The checksum is
    /// left zero for the caller to fill with [`write_checksum`].
    pub fn write(datagram: &mut [u8], source_port: u16, destination_port: u16) -> Option<()> {
        if datagram.len() < HEADER_LEN || datagram.len() > u16::MAX as usize {
            return None;
        }
        let length = datagram.len() as u16;
        datagram[0..2].copy_from_slice(&source_port.to_be_bytes());
        datagram[2..4].copy_from_slice(&destination_port.to_be_bytes());
        datagram[4..6].copy_from_slice(&length.to_be_bytes());
        datagram[6..8].copy_from_slice(&0u16.to_be_bytes());
        Some(())
    }
}

/// Compute the IPv4/UDP checksum into bytes 6..8 (RFC 768 pseudo-header).
pub fn write_checksum(datagram: &mut [u8], source: Addr, destination: Addr) -> Option<()> {
    if datagram.len() < HEADER_LEN || datagram.len() > u16::MAX as usize {
        return None;
    }
    datagram[6..8].copy_from_slice(&0u16.to_be_bytes());
    let sum = fold(pseudo_sum(source, destination, datagram.len()) + sum_words(datagram));
    let sum = !sum;
    // A computed zero is transmitted as all ones (RFC 768).
    let sum = if sum == 0 { 0xFFFF } else { sum };
    datagram[6..8].copy_from_slice(&sum.to_be_bytes());
    Some(())
}

/// Validate a received datagram's checksum. A zero checksum means the sender
/// omitted it, which is legal for IPv4.
pub fn verify_checksum(datagram: &[u8], source: Addr, destination: Addr) -> bool {
    if datagram.len() < HEADER_LEN || datagram.len() > u16::MAX as usize {
        return false;
    }
    if u16::from_be_bytes([datagram[6], datagram[7]]) == 0 {
        return true;
    }
    let sum = fold(pseudo_sum(source, destination, datagram.len()) + sum_words(datagram));
    sum == 0xFFFF
}

/// One's-complement sum of the RFC 768 pseudo-header.
fn pseudo_sum(source: Addr, destination: Addr, udp_len: usize) -> u32 {
    u32::from(u16::from_be_bytes([source.0[0], source.0[1]]))
        + u32::from(u16::from_be_bytes([source.0[2], source.0[3]]))
        + u32::from(u16::from_be_bytes([destination.0[0], destination.0[1]]))
        + u32::from(u16::from_be_bytes([destination.0[2], destination.0[3]]))
        + u32::from(super::ipv4::PROTOCOL_UDP)
        + udp_len as u32
}

/// One's-complement sum of the buffer, padding an odd tail with a zero byte.
fn sum_words(bytes: &[u8]) -> u32 {
    let mut sum: u32 = 0;
    let (chunks, remainder) = bytes.as_chunks::<2>();
    for [high, low] in chunks {
        sum += u32::from(u16::from_be_bytes([*high, *low]));
    }
    if let [last] = remainder {
        sum += u32::from(*last) << 8;
    }
    sum
}

fn fold(mut sum: u32) -> u16 {
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    sum as u16
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: Addr = Addr([10, 0, 2, 15]);
    const DST: Addr = Addr([10, 0, 2, 2]);

    #[test]
    fn parse_round_trip() {
        let mut datagram = [0u8; 27]; // 8 header + 19 payload
        Header::write(&mut datagram, 7777, 5555).unwrap();
        datagram[8..27].copy_from_slice(b"PROJECT_OS-NET-TEST");
        let (header, payload) = Header::parse(&datagram).unwrap();
        assert_eq!(header.source_port, 7777);
        assert_eq!(header.destination_port, 5555);
        assert_eq!(header.length, 27);
        assert_eq!(payload, b"PROJECT_OS-NET-TEST");
    }

    #[test]
    fn checksum_round_trip() {
        let mut datagram = [0u8; 27];
        Header::write(&mut datagram, 7777, 5555).unwrap();
        datagram[8..27].copy_from_slice(b"PROJECT_OS-NET-TEST");
        write_checksum(&mut datagram, SRC, DST).unwrap();
        assert!(verify_checksum(&datagram, SRC, DST));
        assert_ne!(u16::from_be_bytes([datagram[6], datagram[7]]), 0);
        // A different address must not validate.
        assert!(!verify_checksum(&datagram, SRC, Addr([10, 0, 2, 3])));
    }

    #[test]
    fn zero_checksum_is_accepted() {
        let mut datagram = [0u8; 8];
        Header::write(&mut datagram, 1, 2).unwrap();
        assert!(verify_checksum(&datagram, SRC, DST));
    }

    #[test]
    fn checksum_never_transmits_zero() {
        // Chosen so the folded sum is exactly 0xFFFF: the complement is zero
        // and must be transmitted as all ones.
        let mut datagram = [0u8; 8];
        Header::write(&mut datagram, 0x0001, 0x0002).unwrap();
        let source = Addr([0xFF, 0xDB, 0, 0]);
        write_checksum(&mut datagram, source, Addr::UNSPECIFIED).unwrap();
        assert_eq!(u16::from_be_bytes([datagram[6], datagram[7]]), 0xFFFF);
        assert!(verify_checksum(&datagram, source, Addr::UNSPECIFIED));
    }

    #[test]
    fn odd_length_payload_pads_high_byte() {
        let mut datagram = [0u8; 9];
        Header::write(&mut datagram, 1, 2).unwrap();
        datagram[8] = 0xAB;
        write_checksum(&mut datagram, SRC, DST).unwrap();
        assert!(verify_checksum(&datagram, SRC, DST));
    }

    #[test]
    fn parse_rejects_bad_lengths() {
        assert!(Header::parse(&[0u8; 7]).is_none());
        let mut datagram = [0u8; 8];
        Header::write(&mut datagram, 1, 2).unwrap();
        datagram[4..6].copy_from_slice(&4u16.to_be_bytes()); // < header
        assert!(Header::parse(&datagram).is_none());
        datagram[4..6].copy_from_slice(&64u16.to_be_bytes()); // > buffer
        assert!(Header::parse(&datagram).is_none());
    }

    #[test]
    fn parse_truncates_to_length_field() {
        let mut datagram = [0u8; 16];
        Header::write(&mut datagram, 1, 2).unwrap(); // length 16
        datagram[4..6].copy_from_slice(&12u16.to_be_bytes());
        let (header, payload) = Header::parse(&datagram).unwrap();
        assert_eq!(header.length, 12);
        assert_eq!(payload.len(), 4);
    }

    #[test]
    fn write_checksum_rejects_small_buffer() {
        let mut datagram = [0u8; 7];
        assert!(write_checksum(&mut datagram, SRC, DST).is_none());
        assert!(!verify_checksum(&datagram, SRC, DST));
    }
}
