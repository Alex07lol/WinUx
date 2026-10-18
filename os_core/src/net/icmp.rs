//! ICMP echo (RFC 792) — enough for `ping`.

use super::checksum;

pub const HEADER_LEN: usize = 8;
pub const TYPE_ECHO_REQUEST: u8 = 8;
pub const TYPE_ECHO_REPLY: u8 = 0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Message {
    pub type_: u8,
    pub identifier: u16,
    pub sequence: u16,
}

impl Message {
    /// Parse a message and return the header plus the echoed data. The
    /// checksum must be valid.
    pub fn parse(bytes: &[u8]) -> Option<(Message, &[u8])> {
        if bytes.len() < HEADER_LEN {
            return None;
        }
        if checksum(bytes) != 0 {
            return None;
        }
        let message = Message {
            type_: bytes[0],
            identifier: u16::from_be_bytes([bytes[4], bytes[5]]),
            sequence: u16::from_be_bytes([bytes[6], bytes[7]]),
        };
        Some((message, &bytes[HEADER_LEN..]))
    }

    /// Write a message with `payload.len()` bytes of echo data after it and a
    /// valid checksum. Returns the payload slice offset.
    pub fn write(
        bytes: &mut [u8],
        type_: u8,
        identifier: u16,
        sequence: u16,
        payload_len: usize,
    ) -> Option<()> {
        let total = HEADER_LEN.checked_add(payload_len)?;
        if bytes.len() < total {
            return None;
        }
        bytes[0] = type_;
        bytes[1] = 0;
        bytes[2..4].copy_from_slice(&0u16.to_be_bytes());
        bytes[4..6].copy_from_slice(&identifier.to_be_bytes());
        bytes[6..8].copy_from_slice(&sequence.to_be_bytes());
        let sum = checksum(&bytes[..total]);
        bytes[2..4].copy_from_slice(&sum.to_be_bytes());
        Some(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::checksum;

    #[test]
    fn write_then_parse_round_trip() {
        let mut bytes = [0u8; 16];
        Message::write(&mut bytes, TYPE_ECHO_REQUEST, 0xBEEF, 3, 8).unwrap();
        let (message, payload) = Message::parse(&bytes).unwrap();
        assert_eq!(message.type_, TYPE_ECHO_REQUEST);
        assert_eq!(message.identifier, 0xBEEF);
        assert_eq!(message.sequence, 3);
        assert_eq!(payload, &[0u8; 8]);
    }

    #[test]
    fn checksum_valid_after_write() {
        let mut bytes = [0u8; 8];
        Message::write(&mut bytes, TYPE_ECHO_REPLY, 1, 1, 0).unwrap();
        assert_eq!(checksum(&bytes), 0);
        assert_eq!(u16::from_be_bytes([bytes[2], bytes[3]]), 0xFFFD);
    }

    #[test]
    fn parse_rejects_short_and_bad_checksum() {
        assert!(Message::parse(&[0u8; 7]).is_none());
        let mut bytes = [0u8; 8];
        Message::write(&mut bytes, TYPE_ECHO_REQUEST, 7, 9, 0).unwrap();
        bytes[2] ^= 0x01;
        assert!(Message::parse(&bytes).is_none());
    }

    #[test]
    fn write_rejects_small_buffer() {
        let mut bytes = [0u8; 8];
        assert!(Message::write(&mut bytes, TYPE_ECHO_REQUEST, 1, 1, 1).is_none());
    }
}
