//! Win32 registry-shaped read-only view over the config store (Phase 8,
//! stage 3).
//!
//! `HKLM` is the only backed root; a "key" is a store path prefix with
//! children, a "value" is one store entry read as REG_SZ. Handles are four
//! synthetic slot values (`0x1000 + slot`); `user.rs` resets the table after
//! every user program's teardown, so handles die with the program. Lookups
//! are ASCII case-insensitive, matching the documented Windows key-name rule,
//! while the store itself stays byte-exact (POCF). The full contract is
//! `docs/WIN32_REGISTRY.md`; primary sources are in R-017.

use alloc::string::String;

use crate::config;
use crate::sync::IrqSpinLock;

/// `HKEY_LOCAL_MACHINE`, the only predefined root with a backing store.
pub const HKEY_LOCAL_MACHINE: u64 = 0x8000_0002;

/// Handle value of slot 0: `HANDLE_BASE + slot`.
const HANDLE_BASE: u64 = 0x1000;

/// Concurrent open-key limit. A documented stage value, not a Windows
/// property; the fixture proves the exhaustion error and slot reuse.
const MAX_KEYS: usize = 4;

/// Other predefined roots (`HKEY_CLASSES_ROOT`, `HKEY_CURRENT_USER`, ...)
/// are recognized but have no namespace here.
const PREDEFINED_MIN: u64 = 0x8000_0000;
const PREDEFINED_MAX: u64 = 0x8000_000F;

/// Why a registry call failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyError {
    /// Not `HKLM`, another predefined root, or a live slot handle.
    InvalidHandle,
    /// No key with the requested prefix, or no such value in the store.
    NotFound,
    /// All [`MAX_KEYS`] slots are in use.
    TooManyKeys,
}

static KEYS: IrqSpinLock<[Option<String>; MAX_KEYS]> = IrqSpinLock::new([None, None, None, None]);

fn slot_of(hkey: u64) -> Option<usize> {
    let slot = hkey.checked_sub(HANDLE_BASE)?;
    usize::try_from(slot).ok().filter(|slot| *slot < MAX_KEYS)
}

fn is_other_predefined(hkey: u64) -> bool {
    (PREDEFINED_MIN..=PREDEFINED_MAX).contains(&hkey) && hkey != HKEY_LOCAL_MACHINE
}

/// The key-path prefix behind `hkey` (`""` for the root).
pub fn prefix_of(hkey: u64) -> Result<String, KeyError> {
    if hkey == HKEY_LOCAL_MACHINE {
        return Ok(String::new());
    }
    if is_other_predefined(hkey) {
        return Err(KeyError::NotFound);
    }
    let slot = slot_of(hkey).ok_or(KeyError::InvalidHandle)?;
    KEYS.lock()[slot].clone().ok_or(KeyError::InvalidHandle)
}

/// Does at least one store entry live under `prefix` (a key node exists)?
fn prefix_has_children(prefix: &str) -> bool {
    config::with(|config| {
        config.list(None).iter().any(|(key, _)| {
            let key = key.as_bytes();
            key.len() > prefix.len()
                && key[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
                && key[prefix.len()] == b'/'
        })
    })
    .unwrap_or(false)
}

/// The value of the entry at `prefix + "/" + name` (or `name` alone for the
/// root), case-insensitively, as REG_SZ data.
fn lookup_value(prefix: &str, name: &str) -> Option<String> {
    let joined = if prefix.is_empty() {
        String::from(name)
    } else {
        let mut joined = String::from(prefix);
        joined.push('/');
        joined.push_str(name);
        joined
    };
    config::with(|config| {
        config
            .list(None)
            .iter()
            .find(|(key, _)| key.as_bytes().eq_ignore_ascii_case(joined.as_bytes()))
            .map(|(_, value)| String::from(*value))
    })
    .ok()
    .flatten()
}

/// `RegOpenKeyExA` backend. `subkey` empty means the parent itself: the root
/// refresh returns `HKLM` itself (no slot), a live handle opens a second
/// handle to the same prefix.
pub fn open(parent: u64, subkey: &str) -> Result<u64, KeyError> {
    let prefix = prefix_of(parent)?;
    let prefix = if subkey.is_empty() {
        if parent == HKEY_LOCAL_MACHINE {
            return Ok(HKEY_LOCAL_MACHINE);
        }
        prefix
    } else if prefix.is_empty() {
        String::from(subkey)
    } else {
        let mut joined = prefix;
        joined.push('/');
        joined.push_str(subkey);
        joined
    };
    if !prefix.is_empty() && !prefix_has_children(&prefix) {
        return Err(KeyError::NotFound);
    }
    let mut keys = KEYS.lock();
    match keys.iter().position(Option::is_none) {
        Some(slot) => {
            keys[slot] = Some(prefix);
            Ok(HANDLE_BASE + slot as u64)
        }
        None => Err(KeyError::TooManyKeys),
    }
}

/// `RegQueryValueExA` backend: the REG_SZ bytes (without terminator).
pub fn value(hkey: u64, name: &str) -> Result<String, KeyError> {
    if name.is_empty() {
        // The unnamed/default value is not modeled.
        return Err(KeyError::NotFound);
    }
    let prefix = prefix_of(hkey)?;
    lookup_value(&prefix, name).ok_or(KeyError::NotFound)
}

/// `RegCloseKey` backend. `HKLM` closes as a no-op (predefined handles are
/// not ours); only a live slot frees; a closed handle stays invalid.
pub fn close(hkey: u64) -> Result<(), KeyError> {
    if hkey == HKEY_LOCAL_MACHINE {
        return Ok(());
    }
    let slot = slot_of(hkey).ok_or(KeyError::InvalidHandle)?;
    let mut keys = KEYS.lock();
    if keys[slot].take().is_some() {
        Ok(())
    } else {
        Err(KeyError::InvalidHandle)
    }
}

/// Drop every open handle; called from the per-program teardown path so
/// handles never outlive the program that opened them.
pub fn reset() {
    for slot in KEYS.lock().iter_mut() {
        *slot = None;
    }
}
