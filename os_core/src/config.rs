//! Persistent configuration store format (Phase 8, stage 1).
//!
//! The kernel's registry-like key/value store serialises into one VFS file
//! (`/etc/config`, `POCF` version 1) whose contents the existing checkpoint
//! persistence already carries, verifies (CRC) and falls back for. This module
//! is host-tested so the kernel only stores already-validated data on top of a
//! known-good codec. The full contract is `docs/CONFIG.md`.
//!
//! Format (little-endian):
//!
//! ```text
//! magic "POCF" | version u16 = 1 | count u16 (0..=64)
//! count entries: key_len u16 (1..=96) | value_len u16 (0..=128) | key | value
//! ```
//!
//! The header is eight bytes, so an empty store serialises to eight bytes.
//! Keys are path-shaped ASCII (`[A-Za-z0-9_.-]` components separated by `/`,
//! each component 1..=32 bytes, whole key 1..=96 bytes). Values are 0..=128
//! printable ASCII bytes (0x20..=0x7E). `encode` writes entries sorted by key;
//! `decode` accepts any order but rejects duplicates and every cap violation.

use alloc::string::String;
use alloc::vec::Vec;

use crate::path::MAX_COMPONENT;

/// File magic at offset 0.
pub const MAGIC: [u8; 4] = *b"POCF";
/// On-disk format version this module reads and writes.
pub const VERSION: u16 = 1;
/// Header size in bytes (magic + version + entry count).
pub const HEADER_BYTES: usize = 8;
/// Maximum number of entries in one store.
pub const MAX_ENTRIES: usize = 64;
/// Maximum key length in bytes (including `/` separators).
pub const MAX_KEY_BYTES: usize = 96;
/// Maximum value length in bytes.
pub const MAX_VALUE_BYTES: usize = 128;
/// Maximum serialised size in bytes (stays inside the 8192-byte VFS file cap).
pub const MAX_BLOB_BYTES: usize = 7984;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ConfigError {
    BadMagic,
    BadVersion,
    Truncated,
    TrailingBytes,
    BadEntryCount,
    TooLarge,
    BadKey,
    BadValue,
    DuplicateKey,
    Full,
    NoSuchKey,
}

impl ConfigError {
    pub fn label(&self) -> &'static str {
        match self {
            ConfigError::BadMagic => "bad magic",
            ConfigError::BadVersion => "bad version",
            ConfigError::Truncated => "truncated",
            ConfigError::TrailingBytes => "trailing bytes",
            ConfigError::BadEntryCount => "bad entry count",
            ConfigError::TooLarge => "config too large",
            ConfigError::BadKey => "bad key",
            ConfigError::BadValue => "bad value",
            ConfigError::DuplicateKey => "duplicate key",
            ConfigError::Full => "entry table full",
            ConfigError::NoSuchKey => "no such key",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub key: String,
    pub value: String,
}

/// A key/value store with distinct, sorted keys.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Config {
    entries: Vec<Entry>,
}

fn component_is_valid(component: &str) -> bool {
    if component.is_empty() || component.len() > MAX_COMPONENT {
        return false;
    }
    component
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

/// A key is one or more non-empty components joined by `/`.
pub fn key_is_valid(key: &str) -> bool {
    if key.is_empty() || key.len() > MAX_KEY_BYTES {
        return false;
    }
    key.split('/').all(component_is_valid)
}

/// A value is at most [`MAX_VALUE_BYTES`] printable ASCII bytes.
pub fn value_is_valid(value: &str) -> bool {
    value.len() <= MAX_VALUE_BYTES && value.bytes().all(|byte| (0x20..=0x7e).contains(&byte))
}

fn encoded_size(entries: &[Entry]) -> usize {
    HEADER_BYTES
        + entries
            .iter()
            .map(|entry| 4 + entry.key.len() + entry.value.len())
            .sum::<usize>()
}

impl Config {
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    fn position(&self, key: &str) -> Result<usize, usize> {
        self.entries
            .binary_search_by(|entry| entry.key.as_str().cmp(key))
    }

    pub fn get(&self, key: &str) -> Option<&str> {
        match self.position(key) {
            Ok(index) => Some(&self.entries[index].value),
            Err(_) => None,
        }
    }

    pub fn contains(&self, key: &str) -> bool {
        self.position(key).is_ok()
    }

    pub fn set(&mut self, key: &str, value: &str) -> Result<(), ConfigError> {
        if !key_is_valid(key) {
            return Err(ConfigError::BadKey);
        }
        if !value_is_valid(value) {
            return Err(ConfigError::BadValue);
        }
        match self.position(key) {
            Ok(index) => {
                let old_len = self.entries[index].value.len();
                if encoded_size(&self.entries) - old_len + value.len() > MAX_BLOB_BYTES {
                    return Err(ConfigError::TooLarge);
                }
                self.entries[index].value = String::from(value);
            }
            Err(index) => {
                if self.entries.len() >= MAX_ENTRIES {
                    return Err(ConfigError::Full);
                }
                if encoded_size(&self.entries) + 4 + key.len() + value.len() > MAX_BLOB_BYTES {
                    return Err(ConfigError::TooLarge);
                }
                self.entries.insert(
                    index,
                    Entry {
                        key: String::from(key),
                        value: String::from(value),
                    },
                );
            }
        }
        Ok(())
    }

    pub fn remove(&mut self, key: &str) -> Result<(), ConfigError> {
        match self.position(key) {
            Ok(index) => {
                self.entries.remove(index);
                Ok(())
            }
            Err(_) => Err(ConfigError::NoSuchKey),
        }
    }

    /// Entries whose key equals `prefix` or starts with `prefix` plus a `/`.
    /// `None` lists everything. Sorted by key.
    pub fn list(&self, prefix: Option<&str>) -> Vec<(&str, &str)> {
        self.entries
            .iter()
            .filter(|entry| match prefix {
                None => true,
                Some(prefix) => {
                    entry.key == prefix
                        || (entry.key.len() > prefix.len()
                            && entry.key.starts_with(prefix)
                            && entry.key.as_bytes()[prefix.len()] == b'/')
                }
            })
            .map(|entry| (entry.key.as_str(), entry.value.as_str()))
            .collect()
    }

    /// Serialise in canonical (key-sorted) order.
    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(encoded_size(&self.entries));
        bytes.extend_from_slice(&MAGIC);
        bytes.extend_from_slice(&VERSION.to_le_bytes());
        bytes.extend_from_slice(&(self.entries.len() as u16).to_le_bytes());
        for entry in &self.entries {
            bytes.extend_from_slice(&(entry.key.len() as u16).to_le_bytes());
            bytes.extend_from_slice(&(entry.value.len() as u16).to_le_bytes());
            bytes.extend_from_slice(entry.key.as_bytes());
            bytes.extend_from_slice(entry.value.as_bytes());
        }
        bytes
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, ConfigError> {
        if bytes.len() > MAX_BLOB_BYTES {
            return Err(ConfigError::TooLarge);
        }
        if bytes.len() < HEADER_BYTES {
            return Err(ConfigError::Truncated);
        }
        if bytes[..4] != MAGIC {
            return Err(ConfigError::BadMagic);
        }
        if u16::from_le_bytes([bytes[4], bytes[5]]) != VERSION {
            return Err(ConfigError::BadVersion);
        }
        let count = u16::from_le_bytes([bytes[6], bytes[7]]) as usize;
        if count > MAX_ENTRIES {
            return Err(ConfigError::BadEntryCount);
        }

        let mut cursor = HEADER_BYTES;
        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            if cursor + 4 > bytes.len() {
                return Err(ConfigError::Truncated);
            }
            let key_len = u16::from_le_bytes([bytes[cursor], bytes[cursor + 1]]) as usize;
            let value_len = u16::from_le_bytes([bytes[cursor + 2], bytes[cursor + 3]]) as usize;
            cursor += 4;
            if key_len == 0 || key_len > MAX_KEY_BYTES {
                return Err(ConfigError::BadKey);
            }
            if value_len > MAX_VALUE_BYTES {
                return Err(ConfigError::BadValue);
            }
            if cursor + key_len + value_len > bytes.len() {
                return Err(ConfigError::Truncated);
            }
            let key = core::str::from_utf8(&bytes[cursor..cursor + key_len])
                .map_err(|_| ConfigError::BadKey)?;
            if !key_is_valid(key) {
                return Err(ConfigError::BadKey);
            }
            cursor += key_len;
            let value = core::str::from_utf8(&bytes[cursor..cursor + value_len])
                .map_err(|_| ConfigError::BadValue)?;
            if !value_is_valid(value) {
                return Err(ConfigError::BadValue);
            }
            cursor += value_len;
            entries.push(Entry {
                key: String::from(key),
                value: String::from(value),
            });
        }
        if cursor != bytes.len() {
            return Err(ConfigError::TrailingBytes);
        }

        entries.sort_by(|a, b| a.key.cmp(&b.key));
        if entries.windows(2).any(|pair| pair[0].key == pair[1].key) {
            return Err(ConfigError::DuplicateKey);
        }
        Ok(Self { entries })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_validation_table() {
        assert!(key_is_valid("System/Name"));
        assert!(key_is_valid("Display/Theme"));
        assert!(key_is_valid("a"));
        assert!(key_is_valid("A.A-b_c/1"));
        assert!(key_is_valid("Net/Mode-2"));

        assert!(!key_is_valid(""));
        assert!(!key_is_valid("/leading"));
        assert!(!key_is_valid("trailing/"));
        assert!(!key_is_valid("a//b"));
        assert!(!key_is_valid("bad key"));
        assert!(!key_is_valid("bad\tkey"));
        assert!(!key_is_valid("caf\u{e9}"));
        assert!(!key_is_valid("semi;colon"));
        assert!(!key_is_valid("star*"));
    }

    #[test]
    fn component_and_total_length_bounds() {
        let component = "a".repeat(MAX_COMPONENT);
        assert!(key_is_valid(&component));
        let too_long = "a".repeat(MAX_COMPONENT + 1);
        assert!(!key_is_valid(&too_long));

        let key = ["a".repeat(31), "b".repeat(31), "c".repeat(32)].join("/");
        assert_eq!(key.len(), MAX_KEY_BYTES);
        assert!(key_is_valid(&key));
        assert!(!key_is_valid(&(key + "d")));
    }

    #[test]
    fn value_validation_table() {
        assert!(value_is_valid(""));
        assert!(value_is_valid("classic"));
        assert!(value_is_valid(&"v".repeat(MAX_VALUE_BYTES)));
        assert!(value_is_valid("~printable trailing tilde~"));

        assert!(!value_is_valid(&"v".repeat(MAX_VALUE_BYTES + 1)));
        assert!(!value_is_valid("line\nbreak"));
        assert!(!value_is_valid("delete\x7f"));
        assert!(!value_is_valid("caf\u{e9}"));
    }

    #[test]
    fn set_get_overwrite_remove() {
        let mut config = Config::new();
        assert!(config.is_empty());
        config.set("System/Name", "PROJECT_OS").unwrap();
        assert_eq!(config.get("System/Name"), Some("PROJECT_OS"));
        assert!(config.contains("System/Name"));
        assert_eq!(config.get("System/Other"), None);
        assert_eq!(config.len(), 1);

        config.set("System/Name", "PROJECT_OS2").unwrap();
        assert_eq!(config.get("System/Name"), Some("PROJECT_OS2"));
        assert_eq!(config.len(), 1);

        config.remove("System/Name").unwrap();
        assert!(config.is_empty());
        assert_eq!(config.remove("System/Name"), Err(ConfigError::NoSuchKey));
    }

    #[test]
    fn set_rejects_invalid_key_and_value() {
        let mut config = Config::new();
        assert_eq!(config.set("bad//key", "x"), Err(ConfigError::BadKey));
        assert_eq!(config.set("ok", "line\nbreak"), Err(ConfigError::BadValue));
        assert_eq!(
            config.set("ok", &"v".repeat(129)),
            Err(ConfigError::BadValue)
        );
        assert!(config.is_empty());
    }

    #[test]
    fn entries_stay_sorted_across_insertions() {
        let mut config = Config::new();
        for key in ["Net/Mode", "Display/Theme", "System/Name"] {
            config.set(key, "x").unwrap();
        }
        let keys: Vec<&str> = config.list(None).iter().map(|(key, _)| *key).collect();
        assert_eq!(keys, ["Display/Theme", "Net/Mode", "System/Name"]);
    }

    #[test]
    fn list_filters_by_path_prefix() {
        let mut config = Config::new();
        for key in [
            "System/Name",
            "System/Version",
            "Net/Mode",
            "Net/Dns/Server",
        ] {
            config.set(key, "x").unwrap();
        }
        let system: Vec<&str> = config
            .list(Some("System"))
            .iter()
            .map(|(key, _)| *key)
            .collect();
        assert_eq!(system, ["System/Name", "System/Version"]);
        assert_eq!(config.list(Some("System/Name")).len(), 1);
        assert_eq!(config.list(Some("Net")).len(), 2);
        assert_eq!(config.list(Some("Sys")).len(), 0);
        assert_eq!(config.list(Some("Nope")).len(), 0);
        assert_eq!(config.list(None).len(), 4);
    }

    #[test]
    fn entry_count_is_capped() {
        let mut config = Config::new();
        for index in 0..MAX_ENTRIES {
            config.set(&alloc::format!("Key/{}", index), "x").unwrap();
        }
        assert_eq!(config.len(), MAX_ENTRIES);
        assert_eq!(config.set("Key/64", "x"), Err(ConfigError::Full));
    }

    #[test]
    fn blob_size_is_capped() {
        let value = "v".repeat(MAX_VALUE_BYTES);
        let mut config = Config::new();
        let mut rejected = false;
        for index in 0..MAX_ENTRIES {
            match config.set(&alloc::format!("K/{}", index), &value) {
                Ok(()) => {}
                Err(error) => {
                    assert_eq!(error, ConfigError::TooLarge);
                    rejected = true;
                    break;
                }
            }
        }
        assert!(rejected, "the blob cap must trigger before 64 fat entries");
        assert!(config.encode().len() <= MAX_BLOB_BYTES);

        // The store is close to its cap: one more full-size entry must be
        // rejected and a same-size overwrite of an existing key must still be
        // allowed.
        assert_eq!(config.set("Z/New", &value), Err(ConfigError::TooLarge));
        let key = alloc::format!("K/{}", config.len() - 1);
        let before = String::from(config.get(&key).unwrap());
        config.set(&key, &value).unwrap();
        assert_eq!(config.get(&key), Some(before.as_str()));
        assert!(config.encode().len() <= MAX_BLOB_BYTES);
    }

    #[test]
    fn encode_is_canonical_and_deterministic() {
        let mut first = Config::new();
        first.set("Net/Mode", "static").unwrap();
        first.set("Display/Theme", "classic").unwrap();
        first.set("System/Name", "PROJECT_OS").unwrap();

        let mut second = Config::new();
        second.set("System/Name", "PROJECT_OS").unwrap();
        second.set("Net/Mode", "static").unwrap();
        second.set("Display/Theme", "classic").unwrap();

        assert_eq!(first.encode(), second.encode());
        assert_eq!(first.encode(), first.encode());
    }

    #[test]
    fn round_trip_preserves_entries() {
        let mut config = Config::new();
        config.set("System/Name", "PROJECT_OS").unwrap();
        config.set("Display/Theme", "classic").unwrap();
        config.set("Empty/Value", "").unwrap();
        let blob = config.encode();
        let decoded = Config::decode(&blob).unwrap();
        assert_eq!(decoded, config);
        assert_eq!(decoded.encode(), blob);
    }

    #[test]
    fn empty_store_round_trips_to_header_only() {
        let config = Config::new();
        let blob = config.encode();
        assert_eq!(blob.len(), HEADER_BYTES);
        assert_eq!(&blob[..4], b"POCF");
        assert_eq!(Config::decode(&blob).unwrap(), config);
    }

    #[test]
    fn decode_accepts_unsorted_input() {
        let mut encoded = Vec::new();
        encoded.extend_from_slice(&MAGIC);
        encoded.extend_from_slice(&VERSION.to_le_bytes());
        encoded.extend_from_slice(&2u16.to_le_bytes());
        for (key, value) in [("b/key", "2"), ("a/key", "1")] {
            encoded.extend_from_slice(&(key.len() as u16).to_le_bytes());
            encoded.extend_from_slice(&(value.len() as u16).to_le_bytes());
            encoded.extend_from_slice(key.as_bytes());
            encoded.extend_from_slice(value.as_bytes());
        }
        let decoded = Config::decode(&encoded).unwrap();
        let keys: Vec<&str> = decoded.list(None).iter().map(|(key, _)| *key).collect();
        assert_eq!(keys, ["a/key", "b/key"]);
    }

    #[test]
    fn decode_rejects_header_errors() {
        let blob = Config::new().encode();

        let mut bad_magic = blob.clone();
        bad_magic[0] = b'X';
        assert_eq!(Config::decode(&bad_magic), Err(ConfigError::BadMagic));

        let mut bad_version = blob.clone();
        bad_version[4] = 2;
        assert_eq!(Config::decode(&bad_version), Err(ConfigError::BadVersion));

        for cut in 0..HEADER_BYTES {
            assert_eq!(Config::decode(&blob[..cut]), Err(ConfigError::Truncated));
        }

        let mut bad_count = blob.clone();
        bad_count[6] = (MAX_ENTRIES + 1) as u8;
        assert_eq!(Config::decode(&bad_count), Err(ConfigError::BadEntryCount));

        let mut oversized = blob.clone();
        oversized.resize(MAX_BLOB_BYTES + 1, 0);
        assert_eq!(Config::decode(&oversized), Err(ConfigError::TooLarge));
    }

    #[test]
    fn decode_rejects_truncated_and_trailing_bytes() {
        let mut config = Config::new();
        config.set("Key/One", "value").unwrap();
        let blob = config.encode();

        for cut in [8, 9, 11, blob.len() - 1] {
            assert_eq!(Config::decode(&blob[..cut]), Err(ConfigError::Truncated));
        }

        let mut trailing = blob.clone();
        trailing.push(0);
        assert_eq!(Config::decode(&trailing), Err(ConfigError::TrailingBytes));
    }

    #[test]
    fn decode_rejects_bad_entries() {
        fn craft(entries: &[(&str, &str)]) -> Vec<u8> {
            let mut bytes = Vec::new();
            bytes.extend_from_slice(&MAGIC);
            bytes.extend_from_slice(&VERSION.to_le_bytes());
            bytes.extend_from_slice(&(entries.len() as u16).to_le_bytes());
            for (key, value) in entries {
                bytes.extend_from_slice(&(key.len() as u16).to_le_bytes());
                bytes.extend_from_slice(&(value.len() as u16).to_le_bytes());
                bytes.extend_from_slice(key.as_bytes());
                bytes.extend_from_slice(value.as_bytes());
            }
            bytes
        }

        assert_eq!(
            Config::decode(&craft(&[("bad//key", "x")])),
            Err(ConfigError::BadKey)
        );
        assert_eq!(
            Config::decode(&craft(&[("key", "line\nbreak")])),
            Err(ConfigError::BadValue)
        );
        assert_eq!(
            Config::decode(&craft(&[("same", "1"), ("same", "2")])),
            Err(ConfigError::DuplicateKey)
        );

        let mut zero_key = craft(&[("key", "x")]);
        zero_key[8] = 0;
        zero_key[9] = 0;
        assert_eq!(Config::decode(&zero_key), Err(ConfigError::BadKey));
    }

    #[test]
    fn error_labels_are_stable() {
        let rows = [
            (ConfigError::BadMagic, "bad magic"),
            (ConfigError::BadVersion, "bad version"),
            (ConfigError::Truncated, "truncated"),
            (ConfigError::TrailingBytes, "trailing bytes"),
            (ConfigError::BadEntryCount, "bad entry count"),
            (ConfigError::TooLarge, "config too large"),
            (ConfigError::BadKey, "bad key"),
            (ConfigError::BadValue, "bad value"),
            (ConfigError::DuplicateKey, "duplicate key"),
            (ConfigError::Full, "entry table full"),
            (ConfigError::NoSuchKey, "no such key"),
        ];
        for (error, label) in rows {
            assert_eq!(error.label(), label);
        }
    }
}
