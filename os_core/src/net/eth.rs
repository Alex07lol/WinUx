//! Ethernet II framing (Phase 6, stage 5).

use super::{read_be_u16, write_be_u16};

pub const ETHERTYPE_IPV4: u16 = 0x0800;
pub const ETHERTYPE_ARP: u16 = 0x0806;
pub const HEADER_LEN: usize = 14;
/// Minimum frame on the wire, excluding the FCS.
pub const MIN_FRAME_LEN: usize = 60;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct MacAddr(pub [u8; 6]);

impl MacAddr {
    pub const BROADCAST: MacAddr = MacAddr([0xFF; 6]);
    pub const ZERO: MacAddr = MacAddr([0; 6]);

    pub const fn new(bytes: [u8; 6]) -> Self {
        Self(bytes)
    }

    pub fn is_broadcast(&self) -> bool {
        self.0 == [0xFF; 6]
    }

    pub fn octets(&self) -> [u8; 6] {
        self.0
    }
}

impl core::fmt::Display for MacAddr {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let [a, b, c, d, e, g] = self.0;
        write!(
            f,
            "{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
            a, b, c, d, e, g
        )
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Header {
    pub destination: MacAddr,
    pub source: MacAddr,
    pub ethertype: u16,
}

impl Header {
    pub fn parse(frame: &[u8]) -> Option<Header> {
        if frame.len() < HEADER_LEN {
            return None;
        }
        let mut destination = [0u8; 6];
        destination.copy_from_slice(&frame[0..6]);
        let mut source = [0u8; 6];
        source.copy_from_slice(&frame[6..12]);
        Some(Header {
            destination: MacAddr(destination),
            source: MacAddr(source),
            ethertype: read_be_u16(&frame[12..14]),
        })
    }

    /// Write the 14-byte header; the caller fills the payload after it.
    pub fn write(
        frame: &mut [u8],
        destination: MacAddr,
        source: MacAddr,
        ethertype: u16,
    ) -> Option<()> {
        if frame.len() < HEADER_LEN {
            return None;
        }
        frame[0..6].copy_from_slice(&destination.0);
        frame[6..12].copy_from_slice(&source.0);
        write_be_u16(&mut frame[12..14], ethertype);
        Some(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mac_display() {
        let mac = MacAddr([0x52, 0x54, 0x00, 0x12, 0x34, 0x56]);
        assert_eq!(mac.to_string(), "52:54:00:12:34:56");
        assert_eq!(format!("{}", MacAddr::BROADCAST), "ff:ff:ff:ff:ff:ff");
        assert_eq!(
            format!("{}", MacAddr([0, 1, 2, 3, 4, 5])),
            "00:01:02:03:04:05"
        );
    }

    #[test]
    fn parse_broadcast_arp_frame() {
        let mut frame = [0u8; 60];
        Header::write(
            &mut frame,
            MacAddr::BROADCAST,
            MacAddr([1, 2, 3, 4, 5, 6]),
            ETHERTYPE_ARP,
        )
        .unwrap();
        let header = Header::parse(&frame).unwrap();
        assert!(header.destination.is_broadcast());
        assert_eq!(header.source, MacAddr([1, 2, 3, 4, 5, 6]));
        assert_eq!(header.ethertype, ETHERTYPE_ARP);
    }

    #[test]
    fn parse_round_trip_bytes() {
        let mut frame = [0u8; 14];
        Header::write(
            &mut frame,
            MacAddr([0xAA; 6]),
            MacAddr([0xBB; 6]),
            ETHERTYPE_IPV4,
        )
        .unwrap();
        assert_eq!(&frame[0..6], &[0xAA; 6]);
        assert_eq!(&frame[6..12], &[0xBB; 6]);
        assert_eq!(&frame[12..14], &[0x08, 0x00]);
    }

    #[test]
    fn parse_rejects_truncated() {
        assert!(Header::parse(&[0u8; 13]).is_none());
        assert!(Header::parse(&[]).is_none());
    }

    #[test]
    fn write_rejects_small_buffer() {
        let mut frame = [0u8; 13];
        assert!(
            Header::write(
                &mut frame,
                MacAddr::BROADCAST,
                MacAddr::ZERO,
                ETHERTYPE_IPV4
            )
            .is_none()
        );
    }

    #[test]
    fn zero_mac_is_not_broadcast() {
        assert!(!MacAddr::ZERO.is_broadcast());
        assert_eq!(MacAddr::ZERO.octets(), [0; 6]);
    }
}
