//! ARP (RFC 826) for IPv4 over Ethernet, plus the small cache the kernel
//! stack uses to resolve next-hop MAC addresses.

use super::eth::MacAddr;
use super::ipv4::Addr;
use super::{read_be_u16, write_be_u16};

pub const HEADER_LEN: usize = 28;
pub const HARDWARE_ETHERNET: u16 = 1;
pub const OPERATION_REQUEST: u16 = 1;
pub const OPERATION_REPLY: u16 = 2;
/// IPv4 addresses are 4 bytes in ARP's protocol field.
const PROTOCOL_IPV4: u16 = 0x0800;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Packet {
    pub operation: u16,
    pub sender_mac: MacAddr,
    pub sender_ip: Addr,
    pub target_mac: MacAddr,
    pub target_ip: Addr,
}

impl Packet {
    pub fn parse(payload: &[u8]) -> Option<Packet> {
        if payload.len() < HEADER_LEN {
            return None;
        }
        let hardware = read_be_u16(&payload[0..2]);
        let protocol = read_be_u16(&payload[2..4]);
        let hardware_len = payload[4];
        let protocol_len = payload[5];
        if hardware != HARDWARE_ETHERNET || protocol != PROTOCOL_IPV4 {
            return None;
        }
        if hardware_len != 6 || protocol_len != 4 {
            return None;
        }
        let operation = read_be_u16(&payload[6..8]);
        let mut sender_mac = [0u8; 6];
        sender_mac.copy_from_slice(&payload[8..14]);
        let mut sender_ip = [0u8; 4];
        sender_ip.copy_from_slice(&payload[14..18]);
        let mut target_mac = [0u8; 6];
        target_mac.copy_from_slice(&payload[18..24]);
        let mut target_ip = [0u8; 4];
        target_ip.copy_from_slice(&payload[24..28]);
        Some(Packet {
            operation,
            sender_mac: MacAddr(sender_mac),
            sender_ip: Addr(sender_ip),
            target_mac: MacAddr(target_mac),
            target_ip: Addr(target_ip),
        })
    }

    fn write_common(
        payload: &mut [u8],
        operation: u16,
        sender_mac: MacAddr,
        sender_ip: Addr,
        target_mac: MacAddr,
        target_ip: Addr,
    ) -> Option<()> {
        if payload.len() < HEADER_LEN {
            return None;
        }
        write_be_u16(&mut payload[0..2], HARDWARE_ETHERNET);
        write_be_u16(&mut payload[2..4], PROTOCOL_IPV4);
        payload[4] = 6;
        payload[5] = 4;
        write_be_u16(&mut payload[6..8], operation);
        payload[8..14].copy_from_slice(&sender_mac.octets());
        payload[14..18].copy_from_slice(&sender_ip.octets());
        payload[18..24].copy_from_slice(&target_mac.octets());
        payload[24..28].copy_from_slice(&target_ip.octets());
        Some(())
    }

    /// "Who has `target_ip`? Tell `sender_ip` at `sender_mac`."
    pub fn write_request(
        payload: &mut [u8],
        sender_mac: MacAddr,
        sender_ip: Addr,
        target_ip: Addr,
    ) -> Option<()> {
        Self::write_common(
            payload,
            OPERATION_REQUEST,
            sender_mac,
            sender_ip,
            MacAddr::ZERO,
            target_ip,
        )
    }

    /// "`sender_ip` is at `sender_mac`." (Sent back to the requester.)
    pub fn write_reply(
        payload: &mut [u8],
        sender_mac: MacAddr,
        sender_ip: Addr,
        target_mac: MacAddr,
        target_ip: Addr,
    ) -> Option<()> {
        Self::write_common(
            payload,
            OPERATION_REPLY,
            sender_mac,
            sender_ip,
            target_mac,
            target_ip,
        )
    }
}

/// Cache entry lifetime in timer ticks (100 Hz, so 30 s).
pub const EXPIRY_TICKS: u64 = 3000;

pub struct Cache<const N: usize> {
    entries: [Option<(Addr, MacAddr, u64)>; N],
}

impl<const N: usize> Default for Cache<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> Cache<N> {
    pub fn new() -> Self {
        Self { entries: [None; N] }
    }

    /// Record a mapping, refreshing any existing entry for `ip`. When the
    /// cache is full the oldest (expired or not) slot is replaced.
    pub fn insert(&mut self, ip: Addr, mac: MacAddr, now: u64) {
        if let Some(entry) = self
            .entries
            .iter_mut()
            .find(|e| matches!(e, Some((a, _, _)) if *a == ip))
        {
            *entry = Some((ip, mac, now));
            return;
        }
        if let Some(slot) = self.entries.iter_mut().find(|e| e.is_none()) {
            *slot = Some((ip, mac, now));
            return;
        }
        let oldest = self
            .entries
            .iter_mut()
            .min_by_key(|e| e.map(|(_, _, t)| t).unwrap_or(0))
            .expect("cache is non-empty");
        *oldest = Some((ip, mac, now));
    }

    /// Look up a live entry; expired mappings resolve as `None`.
    pub fn lookup(&self, ip: Addr, now: u64) -> Option<MacAddr> {
        for (cached_ip, mac, tick) in self.entries.iter().flatten() {
            if *cached_ip == ip {
                if now.wrapping_sub(*tick) <= EXPIRY_TICKS {
                    return Some(*mac);
                }
                return None;
            }
        }
        None
    }

    pub fn len(&self) -> usize {
        self.entries.iter().filter(|e| e.is_some()).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn capacity(&self) -> usize {
        N
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MAC_A: MacAddr = MacAddr([0x52, 0x54, 0x00, 0x00, 0x00, 0x01]);
    const MAC_B: MacAddr = MacAddr([0x52, 0x54, 0x00, 0x00, 0x00, 0x02]);
    const IP_A: Addr = Addr([10, 0, 2, 15]);
    const IP_B: Addr = Addr([10, 0, 2, 2]);

    #[test]
    fn request_round_trip() {
        let mut payload = [0u8; HEADER_LEN];
        Packet::write_request(&mut payload, MAC_A, IP_A, IP_B).unwrap();
        let packet = Packet::parse(&payload).unwrap();
        assert_eq!(packet.operation, OPERATION_REQUEST);
        assert_eq!(packet.sender_mac, MAC_A);
        assert_eq!(packet.sender_ip, IP_A);
        assert_eq!(packet.target_mac, MacAddr::ZERO);
        assert_eq!(packet.target_ip, IP_B);
    }

    #[test]
    fn request_bytes_match_rfc_layout() {
        let mut payload = [0u8; HEADER_LEN];
        Packet::write_request(&mut payload, MAC_A, IP_A, IP_B).unwrap();
        assert_eq!(&payload[0..2], &[0x00, 0x01]);
        assert_eq!(&payload[2..4], &[0x08, 0x00]);
        assert_eq!(payload[4], 6);
        assert_eq!(payload[5], 4);
        assert_eq!(&payload[6..8], &[0x00, 0x01]);
        assert_eq!(&payload[8..14], &MAC_A.octets());
        assert_eq!(&payload[14..18], &IP_A.octets());
        assert_eq!(&payload[18..24], &[0; 6]);
        assert_eq!(&payload[24..28], &IP_B.octets());
    }

    #[test]
    fn reply_round_trip() {
        let mut payload = [0u8; HEADER_LEN];
        Packet::write_reply(&mut payload, MAC_B, IP_B, MAC_A, IP_A).unwrap();
        let packet = Packet::parse(&payload).unwrap();
        assert_eq!(packet.operation, OPERATION_REPLY);
        assert_eq!(packet.sender_mac, MAC_B);
        assert_eq!(packet.sender_ip, IP_B);
        assert_eq!(packet.target_mac, MAC_A);
        assert_eq!(packet.target_ip, IP_A);
    }

    #[test]
    fn parse_rejects_short_and_wrong_family() {
        assert!(Packet::parse(&[0u8; HEADER_LEN - 1]).is_none());
        let mut payload = [0u8; HEADER_LEN];
        Packet::write_request(&mut payload, MAC_A, IP_A, IP_B).unwrap();
        payload[2] = 0x86; // IPv6 protocol type
        assert!(Packet::parse(&payload).is_none());
        payload[2..4].copy_from_slice(&[0x08, 0x00]);
        payload[5] = 3; // protocol length != 4
        assert!(Packet::parse(&payload).is_none());
    }

    #[test]
    fn write_rejects_small_buffer() {
        let mut payload = [0u8; HEADER_LEN - 1];
        assert!(Packet::write_request(&mut payload, MAC_A, IP_A, IP_B).is_none());
    }

    #[test]
    fn cache_insert_lookup() {
        let mut cache: Cache<8> = Cache::new();
        assert!(cache.is_empty());
        cache.insert(IP_B, MAC_B, 100);
        assert_eq!(cache.lookup(IP_B, 100), Some(MAC_B));
        assert_eq!(cache.lookup(IP_A, 100), None);
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.capacity(), 8);
    }

    #[test]
    fn cache_refresh_keeps_one_entry() {
        let mut cache: Cache<8> = Cache::new();
        cache.insert(IP_B, MAC_B, 100);
        cache.insert(IP_B, MAC_A, 200);
        assert_eq!(cache.len(), 1);
        assert_eq!(cache.lookup(IP_B, 250), Some(MAC_A));
    }

    #[test]
    fn cache_expires_after_ttl() {
        let mut cache: Cache<8> = Cache::new();
        cache.insert(IP_B, MAC_B, 1000);
        assert_eq!(cache.lookup(IP_B, 1000 + EXPIRY_TICKS), Some(MAC_B));
        assert_eq!(cache.lookup(IP_B, 1001 + EXPIRY_TICKS), None);
        assert_eq!(cache.len(), 1); // expired entries stay until replaced
    }

    #[test]
    fn cache_evicts_oldest_when_full() {
        let mut cache: Cache<2> = Cache::new();
        cache.insert(Addr([10, 0, 2, 1]), MAC_A, 10);
        cache.insert(Addr([10, 0, 2, 2]), MAC_B, 20);
        cache.insert(Addr([10, 0, 2, 3]), MAC_A, 30);
        assert_eq!(cache.len(), 2);
        assert_eq!(cache.lookup(Addr([10, 0, 2, 1]), 30), None);
        assert_eq!(cache.lookup(Addr([10, 0, 2, 2]), 30), Some(MAC_B));
        assert_eq!(cache.lookup(Addr([10, 0, 2, 3]), 30), Some(MAC_A));
    }
}
