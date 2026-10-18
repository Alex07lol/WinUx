//! IPv4 header parsing/building (RFC 791) — only what the stage-5 stack
//! needs: options-free headers, no fragmentation, no reassembly.

use super::checksum;

pub const HEADER_LEN: usize = 20;
pub const IHL_VERSION: u8 = 0x45;
pub const PROTOCOL_ICMP: u8 = 1;
pub const PROTOCOL_UDP: u8 = 17;
pub const DEFAULT_TTL: u8 = 64;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Addr(pub [u8; 4]);

impl Addr {
    pub const UNSPECIFIED: Addr = Addr([0; 4]);
    pub const BROADCAST: Addr = Addr([0xFF; 4]);

    pub const fn new(octets: [u8; 4]) -> Self {
        Self(octets)
    }

    pub fn octets(&self) -> [u8; 4] {
        self.0
    }

    pub const fn is_unspecified(&self) -> bool {
        self.0[0] == 0 && self.0[1] == 0 && self.0[2] == 0 && self.0[3] == 0
    }
}

impl core::fmt::Display for Addr {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let [a, b, c, d] = self.0;
        write!(f, "{}.{}.{}.{}", a, b, c, d)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Header {
    pub source: Addr,
    pub destination: Addr,
    pub protocol: u8,
    pub ttl: u8,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ParseError {
    TooShort,
    NotIpv4,
    OptionsUnsupported,
    Fragment,
    BadChecksum,
}

impl Header {
    /// Parse a 20-byte header and return it with the payload slice.
    pub fn parse(packet: &[u8]) -> Result<(Header, &[u8]), ParseError> {
        if packet.len() < HEADER_LEN {
            return Err(ParseError::TooShort);
        }
        let version_ihl = packet[0];
        if version_ihl >> 4 != 4 {
            return Err(ParseError::NotIpv4);
        }
        if version_ihl & 0x0F != 5 {
            return Err(ParseError::OptionsUnsupported);
        }
        let total_length = u16::from_be_bytes([packet[2], packet[3]]) as usize;
        if total_length < HEADER_LEN || total_length > packet.len() {
            return Err(ParseError::TooShort);
        }
        // Fragment offset or MF set: this stack does not reassemble.
        let flags_fragment = u16::from_be_bytes([packet[6], packet[7]]);
        if flags_fragment & 0x3FFF != 0 {
            return Err(ParseError::Fragment);
        }
        if super::checksum(&packet[..HEADER_LEN]) != 0 {
            return Err(ParseError::BadChecksum);
        }
        let mut source = [0u8; 4];
        source.copy_from_slice(&packet[12..16]);
        let mut destination = [0u8; 4];
        destination.copy_from_slice(&packet[16..20]);
        Ok((
            Header {
                source: Addr(source),
                destination: Addr(destination),
                protocol: packet[9],
                ttl: packet[8],
            },
            &packet[HEADER_LEN..total_length],
        ))
    }

    /// Write a 20-byte header with `total_length = payload.len() + 20` and a
    /// valid header checksum. Returns the payload slice offset.
    pub fn write(
        packet: &mut [u8],
        source: Addr,
        destination: Addr,
        protocol: u8,
        identification: u16,
        payload_len: usize,
    ) -> Option<()> {
        let total_length = HEADER_LEN.checked_add(payload_len)?;
        if total_length > u16::MAX as usize || packet.len() < total_length {
            return None;
        }
        packet[0] = IHL_VERSION;
        packet[1] = 0;
        packet[2..4].copy_from_slice(&(total_length as u16).to_be_bytes());
        packet[4..6].copy_from_slice(&identification.to_be_bytes());
        packet[6..8].copy_from_slice(&0u16.to_be_bytes());
        packet[8] = DEFAULT_TTL;
        packet[9] = protocol;
        packet[10..12].copy_from_slice(&0u16.to_be_bytes());
        packet[12..16].copy_from_slice(&source.octets());
        packet[16..20].copy_from_slice(&destination.octets());
        let sum = checksum(&packet[..HEADER_LEN]);
        packet[10..12].copy_from_slice(&sum.to_be_bytes());
        Some(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRC: Addr = Addr([10, 0, 2, 15]);
    const DST: Addr = Addr([10, 0, 2, 2]);
    const ICMP_PAYLOAD: [u8; 8] = [8, 0, 0, 0, 0x12, 0x34, 0x00, 0x01];

    fn build() -> [u8; 28] {
        let mut packet = [0u8; 28];
        Header::write(&mut packet, SRC, DST, PROTOCOL_ICMP, 0x1234, 8).unwrap();
        packet[20..28].copy_from_slice(&ICMP_PAYLOAD);
        packet
    }

    #[test]
    fn addr_display() {
        assert_eq!(SRC.to_string(), "10.0.2.15");
        assert_eq!(Addr::BROADCAST.to_string(), "255.255.255.255");
        assert_eq!(Addr([0, 0, 0, 0]).to_string(), "0.0.0.0");
    }

    #[test]
    fn addr_unspecified() {
        assert!(Addr::UNSPECIFIED.is_unspecified());
        assert!(!SRC.is_unspecified());
        assert_eq!(Addr::UNSPECIFIED.octets(), [0, 0, 0, 0]);
    }

    #[test]
    fn write_then_parse_round_trip() {
        let packet = build();
        let (header, payload) = Header::parse(&packet).unwrap();
        assert_eq!(header.source, SRC);
        assert_eq!(header.destination, DST);
        assert_eq!(header.protocol, PROTOCOL_ICMP);
        assert_eq!(header.ttl, DEFAULT_TTL);
        assert_eq!(payload, &ICMP_PAYLOAD);
    }

    #[test]
    fn header_fields_match_rfc() {
        let packet = build();
        assert_eq!(packet[0], 0x45);
        assert_eq!(&packet[2..4], &[0x00, 0x1C]);
        assert_eq!(&packet[4..6], &[0x12, 0x34]);
        assert_eq!(&packet[6..8], &[0x00, 0x00]);
        assert_eq!(packet[8], 64);
        assert_eq!(packet[9], 1);
        assert_eq!(checksum(&packet[..20]), 0);
    }

    #[test]
    fn known_checksum_value() {
        // Hand-computed reference for the fixed header above.
        let packet = build();
        assert_eq!(u16::from_be_bytes([packet[10], packet[11]]), 0x509D);
    }

    #[test]
    fn parse_rejects_short() {
        assert_eq!(Header::parse(&[0u8; 19]), Err(ParseError::TooShort));
    }

    #[test]
    fn parse_rejects_non_ipv4() {
        let mut packet = build();
        packet[0] = 0x65;
        assert_eq!(Header::parse(&packet), Err(ParseError::NotIpv4));
    }

    #[test]
    fn parse_rejects_options() {
        let mut packet = [0u8; 24];
        Header::write(&mut packet, SRC, DST, PROTOCOL_UDP, 1, 4).unwrap();
        packet[0] = 0x46;
        assert_eq!(Header::parse(&packet), Err(ParseError::OptionsUnsupported));
    }

    #[test]
    fn parse_rejects_fragments() {
        let mut packet = build();
        packet[6] = 0x20; // MF set
        assert_eq!(Header::parse(&packet), Err(ParseError::Fragment));
        let mut packet = build();
        packet[7] = 0x01; // fragment offset
        assert_eq!(Header::parse(&packet), Err(ParseError::Fragment));
    }

    #[test]
    fn parse_rejects_bad_checksum() {
        let mut packet = build();
        packet[10] ^= 0xFF;
        assert_eq!(Header::parse(&packet), Err(ParseError::BadChecksum));
    }

    #[test]
    fn parse_rejects_impossible_total_length() {
        let mut packet = build();
        packet[2..4].copy_from_slice(&(19u16).to_be_bytes());
        assert_eq!(Header::parse(&packet), Err(ParseError::TooShort));
        let mut packet = build();
        packet[2..4].copy_from_slice(&(100u16).to_be_bytes());
        assert_eq!(Header::parse(&packet), Err(ParseError::TooShort));
    }

    #[test]
    fn write_rejects_oversized() {
        let mut packet = [0u8; 20];
        assert!(Header::write(&mut packet, SRC, DST, PROTOCOL_UDP, 0, 40).is_none());
    }

    #[test]
    fn parse_accepts_extra_trailing_bytes_and_truncates_payload() {
        let packet = build();
        let mut padded = [0u8; 32];
        padded[..28].copy_from_slice(&packet);
        let (_, payload) = Header::parse(&padded).unwrap();
        assert_eq!(payload.len(), 8);
    }
}
