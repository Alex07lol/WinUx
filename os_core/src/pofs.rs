//! "POFS" checkpoint-image format (Phase 6, stage 2).
//!
//! This module owns the on-disk format of the persistent VFS checkpoint: the
//! 512-byte slot header (magic, version, sequence, payload length and CRC32)
//! and the binary payload that serialises a whole filesystem tree. It is
//! `no_std`-compatible, allocates only through `alloc`, and never touches
//! hardware, so the format and every rejection path is host-testable.
//!
//! The format contract is documented in `docs/PERSISTENCE.md`; keep this file
//! and that document in sync. Nodes are stored in depth-first pre-order with
//! remapped parent indices, so a parent always precedes its children in the
//! stream and decoding never needs pointer fixups.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::str;

use crate::path;

/// Slot header magic, `"POFS"` in the first four bytes.
pub const MAGIC: [u8; 4] = *b"POFS";
/// Format version stored in the header.
pub const VERSION: u16 = 1;
/// Header size at the start of a slot; the payload begins right after it.
pub const HEADER_BYTES: usize = 512;
/// Size of one checkpoint slot.
pub const MAX_SLOT_BYTES: usize = 1_048_576;
/// Largest representable payload inside a slot.
pub const MAX_PAYLOAD_BYTES: usize = MAX_SLOT_BYTES - HEADER_BYTES;
/// Maximum number of nodes in a checkpoint (mirrors `kernel::vfs::MAX_NODES`).
pub const MAX_NODES: usize = 64;
/// Maximum size of one file's data (mirrors `kernel::vfs::MAX_FILE_BYTES`).
pub const MAX_FILE_BYTES: usize = 8192;
/// Parent value for the root node.
pub const NO_PARENT: u32 = u32::MAX;

/// Kind byte for directories.
pub const KIND_DIR: u8 = 0;
/// Kind byte for files.
pub const KIND_FILE: u8 = 1;

/// Format errors with stable labels used in kernel logs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PofsError {
    BadMagic,
    BadVersion,
    BadReserved,
    BadHeaderCrc,
    BadPayloadCrc,
    PayloadTooLarge,
    Truncated,
    TrailingBytes,
    BadNodeCount,
    BadKind,
    BadRoot,
    BadParent,
    BadName,
    BadDataLength,
    DuplicateName,
    BadPath,
    Empty,
}

impl PofsError {
    pub fn label(&self) -> &'static str {
        match self {
            PofsError::BadMagic => "bad magic",
            PofsError::BadVersion => "bad version",
            PofsError::BadReserved => "reserved field is not zero",
            PofsError::BadHeaderCrc => "bad header crc",
            PofsError::BadPayloadCrc => "payload crc mismatch",
            PofsError::PayloadTooLarge => "payload too large",
            PofsError::Truncated => "truncated",
            PofsError::TrailingBytes => "trailing bytes",
            PofsError::BadNodeCount => "bad node count",
            PofsError::BadKind => "bad kind",
            PofsError::BadRoot => "bad root",
            PofsError::BadParent => "bad parent",
            PofsError::BadName => "bad name",
            PofsError::BadDataLength => "bad data length",
            PofsError::DuplicateName => "duplicate name",
            PofsError::BadPath => "bad path",
            PofsError::Empty => "empty slot",
        }
    }
}

/// Decoded slot header fields (the CRC fields are consumed by validation).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlotHeader {
    pub seq: u64,
    pub payload_len: u32,
    pub payload_crc: u32,
}

/// One filesystem node in stream order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeRecord {
    pub kind: u8,
    pub parent: u32,
    pub name: String,
    pub data: Vec<u8>,
}

/// CRC-32 (IEEE 802.3, reflected polynomial 0xEDB88320), computed bitwise so
/// no table or lazy static is required. Check value: `"123456789"`.
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &byte in bytes {
        crc ^= byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

/// True when a slot header is entirely zero, i.e. never written.
pub fn is_empty_header(header: &[u8]) -> bool {
    header.iter().all(|&byte| byte == 0)
}

/// Build a 512-byte slot header for `payload`.
pub fn encode_header(seq: u64, payload_len: u32, payload_crc: u32) -> [u8; HEADER_BYTES] {
    let mut header = [0u8; HEADER_BYTES];
    header[0..4].copy_from_slice(&MAGIC);
    header[4..6].copy_from_slice(&VERSION.to_le_bytes());
    header[8..16].copy_from_slice(&seq.to_le_bytes());
    header[16..20].copy_from_slice(&payload_len.to_le_bytes());
    header[20..24].copy_from_slice(&payload_crc.to_le_bytes());
    let header_crc = crc32(&header[0..24]);
    header[24..28].copy_from_slice(&header_crc.to_le_bytes());
    header
}

/// Validate and decode a slot header. Failures carry the label used in logs.
pub fn parse_header(header: &[u8]) -> Result<SlotHeader, PofsError> {
    if header.len() < HEADER_BYTES {
        return Err(PofsError::Truncated);
    }
    if header[0..4] != MAGIC {
        return Err(PofsError::BadMagic);
    }
    if u16::from_le_bytes([header[4], header[5]]) != VERSION {
        return Err(PofsError::BadVersion);
    }
    if header[6] != 0 || header[7] != 0 {
        return Err(PofsError::BadReserved);
    }
    let stored_crc = u32::from_le_bytes([header[24], header[25], header[26], header[27]]);
    if crc32(&header[0..24]) != stored_crc {
        return Err(PofsError::BadHeaderCrc);
    }
    let payload_len = u32::from_le_bytes([header[16], header[17], header[18], header[19]]);
    if payload_len < 4 {
        return Err(PofsError::Truncated);
    }
    if payload_len as usize > MAX_PAYLOAD_BYTES {
        return Err(PofsError::PayloadTooLarge);
    }
    Ok(SlotHeader {
        seq: u64::from_le_bytes([
            header[8], header[9], header[10], header[11], header[12], header[13], header[14],
            header[15],
        ]),
        payload_len,
        payload_crc: u32::from_le_bytes([header[20], header[21], header[22], header[23]]),
    })
}

fn name_is_valid(name: &str) -> bool {
    !name.is_empty() && path::component_is_valid(name).is_ok()
}

fn kind_is_valid(kind: u8) -> bool {
    kind == KIND_DIR || kind == KIND_FILE
}

/// Shared structural rules between encoding and decoding.
fn record_shape(index: usize, record: &NodeRecord) -> Result<(), PofsError> {
    if !kind_is_valid(record.kind) {
        return Err(PofsError::BadKind);
    }
    if index == 0 {
        let root_ok = record.kind == KIND_DIR
            && record.parent == NO_PARENT
            && record.name.is_empty()
            && record.data.is_empty();
        return if root_ok {
            Ok(())
        } else {
            Err(PofsError::BadRoot)
        };
    }
    if record.parent == NO_PARENT || record.parent as usize >= index {
        return Err(PofsError::BadParent);
    }
    if !name_is_valid(&record.name) {
        return Err(PofsError::BadName);
    }
    match record.kind {
        KIND_DIR if !record.data.is_empty() => Err(PofsError::BadDataLength),
        KIND_FILE if record.data.len() > MAX_FILE_BYTES => Err(PofsError::BadDataLength),
        _ => Ok(()),
    }
}

/// Serialise records (depth-first pre-order, parent before child) into the
/// payload byte stream.
pub fn encode_payload(records: &[NodeRecord]) -> Result<Vec<u8>, PofsError> {
    if records.is_empty() || records.len() > MAX_NODES {
        return Err(PofsError::BadNodeCount);
    }
    let mut payload = Vec::with_capacity(payload_size(records));
    payload.extend_from_slice(&(records.len() as u32).to_le_bytes());
    for (index, record) in records.iter().enumerate() {
        record_shape(index, record)?;
        payload.push(record.kind);
        payload.extend_from_slice(&record.parent.to_le_bytes());
        payload.extend_from_slice(&(record.name.len() as u16).to_le_bytes());
        payload.extend_from_slice(record.name.as_bytes());
        payload.extend_from_slice(&(record.data.len() as u32).to_le_bytes());
        payload.extend_from_slice(&record.data);
    }
    if payload.len() > MAX_PAYLOAD_BYTES {
        return Err(PofsError::PayloadTooLarge);
    }
    Ok(payload)
}

fn payload_size(records: &[NodeRecord]) -> usize {
    records
        .iter()
        .map(|record| 1 + 4 + 2 + record.name.len() + 4 + record.data.len())
        .sum::<usize>()
        + 4
}

struct Cursor<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Cursor<'a> {
    fn take(&mut self, count: usize) -> Result<&'a [u8], PofsError> {
        let end = self
            .position
            .checked_add(count)
            .ok_or(PofsError::Truncated)?;
        if end > self.bytes.len() {
            return Err(PofsError::Truncated);
        }
        let slice = &self.bytes[self.position..end];
        self.position = end;
        Ok(slice)
    }

    fn u8(&mut self) -> Result<u8, PofsError> {
        Ok(self.take(1)?[0])
    }

    fn u16(&mut self) -> Result<u16, PofsError> {
        let bytes = self.take(2)?;
        Ok(u16::from_le_bytes([bytes[0], bytes[1]]))
    }

    fn u32(&mut self) -> Result<u32, PofsError> {
        let bytes = self.take(4)?;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }
}

/// Decode and fully validate a payload: structure, node rules, reconstructed
/// paths and sibling uniqueness. Returns records in stream order.
pub fn decode_payload(bytes: &[u8]) -> Result<Vec<NodeRecord>, PofsError> {
    let mut cursor = Cursor { bytes, position: 0 };
    let node_count = cursor.u32()? as usize;
    if node_count == 0 || node_count > MAX_NODES {
        return Err(PofsError::BadNodeCount);
    }

    let mut records: Vec<NodeRecord> = Vec::with_capacity(node_count);
    for index in 0..node_count {
        let kind = cursor.u8()?;
        let parent = cursor.u32()?;
        let name_len = cursor.u16()? as usize;
        let name_bytes = cursor.take(name_len)?;
        let data_len = cursor.u32()? as usize;
        let data = cursor.take(data_len)?.to_vec();
        let name = match str::from_utf8(name_bytes) {
            Ok(name) => String::from(name),
            Err(_) => return Err(PofsError::BadName),
        };
        let record = NodeRecord {
            kind,
            parent,
            name,
            data,
        };
        record_shape(index, &record)?;
        records.push(record);
    }
    if cursor.position != bytes.len() {
        return Err(PofsError::TrailingBytes);
    }

    let mut paths: Vec<String> = Vec::with_capacity(records.len());
    for index in 0..records.len() {
        let mut parts: Vec<&str> = Vec::new();
        let mut current = index;
        while current != 0 {
            let node = &records[current];
            parts.push(node.name.as_str());
            current = node.parent as usize;
        }
        parts.reverse();
        let path = format!("/{}", parts.join("/"));
        if path.len() > path::MAX_PATH || path::validate(&path).is_err() {
            return Err(PofsError::BadPath);
        }
        paths.push(path);
    }
    for (first, path) in paths.iter().enumerate() {
        if paths[first + 1..].iter().any(|other| other == path) {
            return Err(PofsError::DuplicateName);
        }
    }

    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloc::vec;

    fn record(kind: u8, parent: u32, name: &str, data: &[u8]) -> NodeRecord {
        NodeRecord {
            kind,
            parent,
            name: String::from(name),
            data: data.to_vec(),
        }
    }

    fn sample_tree() -> Vec<NodeRecord> {
        vec![
            record(KIND_DIR, NO_PARENT, "", b""),
            record(KIND_DIR, 0, "etc", b""),
            record(KIND_FILE, 1, "motd", b"hello"),
            record(KIND_FILE, 1, "version", b"v1"),
            record(KIND_DIR, 0, "tmp", b""),
            record(KIND_FILE, 4, "a.txt", b"gamma"),
        ]
    }

    fn build_raw(records: &[(u8, u32, &[u8], &[u8])]) -> Vec<u8> {
        let mut out = (records.len() as u32).to_le_bytes().to_vec();
        for (kind, parent, name, data) in records {
            out.push(*kind);
            out.extend_from_slice(&parent.to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(name);
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(data);
        }
        out
    }

    #[test]
    fn crc32_matches_standard_vectors() {
        assert_eq!(crc32(b""), 0x0000_0000);
        assert_eq!(crc32(b"a"), 0xE8B7_BE43);
        assert_eq!(crc32(b"123456789"), 0xCBF4_3926);
        assert_eq!(
            crc32(b"The quick brown fox jumps over the lazy dog"),
            0x414F_A339
        );
    }

    #[test]
    fn header_round_trips_and_detects_empty() {
        let header = encode_header(42, 1234, 0xDEAD_BEEF);
        let parsed = parse_header(&header).expect("valid header");
        assert_eq!(
            parsed,
            SlotHeader {
                seq: 42,
                payload_len: 1234,
                payload_crc: 0xDEAD_BEEF
            }
        );
        assert!(is_empty_header(&[0u8; HEADER_BYTES]));
        assert!(!is_empty_header(&header));
    }

    #[test]
    fn header_rejects_corruption() {
        let header = encode_header(1, 8, 0);
        let mut bad_magic = header;
        bad_magic[0] = b'X';
        assert_eq!(parse_header(&bad_magic), Err(PofsError::BadMagic));

        let mut bad_version = header;
        bad_version[4] = 9;
        let version_crc = crc32(&bad_version[0..24]);
        bad_version[24..28].copy_from_slice(&version_crc.to_le_bytes());
        assert_eq!(parse_header(&bad_version), Err(PofsError::BadVersion));

        let mut bad_reserved = header;
        bad_reserved[7] = 1;
        let reserved_crc = crc32(&bad_reserved[0..24]);
        bad_reserved[24..28].copy_from_slice(&reserved_crc.to_le_bytes());
        assert_eq!(parse_header(&bad_reserved), Err(PofsError::BadReserved));

        let mut bad_crc = header;
        bad_crc[25] ^= 0xFF;
        assert_eq!(parse_header(&bad_crc), Err(PofsError::BadHeaderCrc));

        let mut bad_len = header;
        bad_len[16..20].copy_from_slice(&3u32.to_le_bytes());
        let len_crc = crc32(&bad_len[0..24]);
        bad_len[24..28].copy_from_slice(&len_crc.to_le_bytes());
        assert_eq!(parse_header(&bad_len), Err(PofsError::Truncated));

        let mut huge_len = header;
        huge_len[16..20].copy_from_slice(&(MAX_PAYLOAD_BYTES as u32 + 1).to_le_bytes());
        let huge_crc = crc32(&huge_len[0..24]);
        huge_len[24..28].copy_from_slice(&huge_crc.to_le_bytes());
        assert_eq!(parse_header(&huge_len), Err(PofsError::PayloadTooLarge));

        assert_eq!(parse_header(&header[0..10]), Err(PofsError::Truncated));
    }

    #[test]
    fn payload_round_trips_a_small_tree() {
        let records = sample_tree();
        let payload = encode_payload(&records).expect("encode");
        assert_eq!(decode_payload(&payload).expect("decode"), records);
    }

    #[test]
    fn payload_round_trips_worst_case_size() {
        let mut records = vec![record(KIND_DIR, NO_PARENT, "", b"")];
        for index in 1..MAX_NODES {
            let name = format!("file{index:02}");
            records.push(record(KIND_FILE, 0, &name, &vec![0xAB; MAX_FILE_BYTES]));
        }
        let payload = encode_payload(&records).expect("encode");
        assert!(payload.len() <= MAX_PAYLOAD_BYTES);
        assert!(payload.len() > 500_000);
        assert_eq!(decode_payload(&payload).expect("decode"), records);
    }

    #[test]
    fn decode_rejects_empty_and_truncated_payloads() {
        assert_eq!(decode_payload(&[]), Err(PofsError::Truncated));
        assert_eq!(decode_payload(&[1, 0, 0]), Err(PofsError::Truncated));

        let payload = encode_payload(&sample_tree()).expect("encode");
        assert_eq!(
            decode_payload(&payload[0..payload.len() - 1]),
            Err(PofsError::Truncated)
        );
    }

    #[test]
    fn decode_rejects_trailing_bytes() {
        let mut payload = encode_payload(&sample_tree()).expect("encode");
        payload.push(0);
        assert_eq!(decode_payload(&payload), Err(PofsError::TrailingBytes));
    }

    #[test]
    fn decode_rejects_bad_node_counts() {
        assert_eq!(
            decode_payload(&0u32.to_le_bytes()),
            Err(PofsError::BadNodeCount)
        );
        let mut payload = encode_payload(&sample_tree()).expect("encode");
        payload[0..4].copy_from_slice(&(MAX_NODES as u32 + 1).to_le_bytes());
        assert_eq!(decode_payload(&payload), Err(PofsError::BadNodeCount));
    }

    #[test]
    fn decode_rejects_bad_kind_and_root() {
        let bad_kind = build_raw(&[(7, NO_PARENT, b"", b"")]);
        assert_eq!(decode_payload(&bad_kind), Err(PofsError::BadKind));

        let root_file = build_raw(&[(KIND_FILE, NO_PARENT, b"", b"x")]);
        assert_eq!(decode_payload(&root_file), Err(PofsError::BadRoot));

        let root_parent = build_raw(&[(KIND_DIR, 1, b"", b"")]);
        assert_eq!(decode_payload(&root_parent), Err(PofsError::BadRoot));

        let root_named = build_raw(&[(KIND_DIR, NO_PARENT, b"root", b"")]);
        assert_eq!(decode_payload(&root_named), Err(PofsError::BadRoot));
    }

    #[test]
    fn decode_rejects_bad_parents() {
        let orphan = build_raw(&[
            (KIND_DIR, NO_PARENT, b"", b""),
            (KIND_FILE, NO_PARENT, b"file", b"x"),
        ]);
        assert_eq!(decode_payload(&orphan), Err(PofsError::BadParent));

        let forward = build_raw(&[
            (KIND_DIR, NO_PARENT, b"", b""),
            (KIND_FILE, 2, b"file", b"x"),
        ]);
        assert_eq!(decode_payload(&forward), Err(PofsError::BadParent));
    }

    #[test]
    fn decode_rejects_bad_names() {
        let empty = build_raw(&[(KIND_DIR, NO_PARENT, b"", b""), (KIND_FILE, 0, b"", b"x")]);
        assert_eq!(decode_payload(&empty), Err(PofsError::BadName));

        let too_long_name = "a".repeat(path::MAX_COMPONENT + 1);
        let long = build_raw(&[
            (KIND_DIR, NO_PARENT, b"", b""),
            (KIND_FILE, 0, too_long_name.as_bytes(), b"x"),
        ]);
        assert_eq!(decode_payload(&long), Err(PofsError::BadName));

        let slash = build_raw(&[
            (KIND_DIR, NO_PARENT, b"", b""),
            (KIND_FILE, 0, b"a/b", b"x"),
        ]);
        assert_eq!(decode_payload(&slash), Err(PofsError::BadName));

        let non_ascii = build_raw(&[
            (KIND_DIR, NO_PARENT, b"", b""),
            (KIND_FILE, 0, b"caf\xc3\xa9", b"x"),
        ]);
        assert_eq!(decode_payload(&non_ascii), Err(PofsError::BadName));
    }

    #[test]
    fn decode_rejects_bad_data_lengths() {
        let dir_with_data =
            build_raw(&[(KIND_DIR, NO_PARENT, b"", b""), (KIND_DIR, 0, b"sub", b"x")]);
        assert_eq!(
            decode_payload(&dir_with_data),
            Err(PofsError::BadDataLength)
        );

        let oversized = build_raw(&[
            (KIND_DIR, NO_PARENT, b"", b""),
            (KIND_FILE, 0, b"big", &vec![0u8; MAX_FILE_BYTES + 1]),
        ]);
        assert_eq!(decode_payload(&oversized), Err(PofsError::BadDataLength));
    }

    #[test]
    fn decode_rejects_duplicate_names() {
        let duplicates = build_raw(&[
            (KIND_DIR, NO_PARENT, b"", b""),
            (KIND_FILE, 0, b"same", b"a"),
            (KIND_FILE, 0, b"same", b"b"),
        ]);
        assert_eq!(decode_payload(&duplicates), Err(PofsError::DuplicateName));
    }

    #[test]
    fn decode_rejects_overlong_paths() {
        let component = "a".repeat(path::MAX_COMPONENT);
        let deep = build_raw(&[
            (KIND_DIR, NO_PARENT, b"", b""),
            (KIND_DIR, 0, component.as_bytes(), b""),
            (KIND_DIR, 1, component.as_bytes(), b""),
            (KIND_DIR, 2, component.as_bytes(), b""),
            (KIND_DIR, 3, component.as_bytes(), b""),
            (KIND_FILE, 4, component.as_bytes(), b"x"),
        ]);
        assert_eq!(decode_payload(&deep), Err(PofsError::BadPath));
    }
}
