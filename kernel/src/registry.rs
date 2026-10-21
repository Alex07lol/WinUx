//! Win32 registry-shaped view over the config store (Phase 8, stages 3–5).
//!
//! `HKLM` is the only backed root; a "key" is a store path prefix with
//! children, a "value" is one store entry mapped to REG_SZ. Handles are four
//! synthetic slot values (`0x1000 + slot`); `user.rs` resets the table after
//! every user program's teardown, so handles die with the program. Lookups
//! are ASCII case-insensitive, matching the documented Windows key-name rule,
//! while the store itself stays byte-exact (POCF). Stage 4 adds mutations:
//! `create` (lazy — keys exist only while entries live under them),
//! `set_value` and `delete_value`, both persisted through `config::save()`.
//! Stage 5 adds deterministic canonical-order enumeration (`enum_key`,
//! `enum_value`) and non-recursive `delete_key` (child keys block deletion
//! with `AccessDenied`; callers delete children first, as Windows
//! prescribes). The full contracts are `docs/WIN32_REGISTRY.md`,
//! `docs/WIN32_REGISTRY_WRITE.md` and `docs/WIN32_REGISTRY_ENUM.md`; primary
//! sources are R-017/R-018/R-019.

use alloc::string::String;
use alloc::vec::Vec;

use os_core::config::key_is_valid;

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
    /// A path, value name or value the store could never accept.
    BadArgument,
    /// The store or its serialised file could not take the write.
    NoSpace,
    /// The enumeration index is past the last entry (`ERROR_NO_MORE_ITEMS`).
    NoMoreItems,
    /// `RegDeleteKeyA` on a key that still has child keys (our contract for
    /// the fetched "must not have subkeys" rule; code 5).
    AccessDenied,
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

/// The store key for the value `name` under `prefix` (or `name` alone at the
/// root).
fn joined_entry(prefix: &str, name: &str) -> String {
    if prefix.is_empty() {
        return String::from(name);
    }
    let mut joined = String::from(prefix);
    joined.push('/');
    joined.push_str(name);
    joined
}

/// Map a store mutation failure onto the registry error surface.
fn write_error(error: os_core::config::ConfigError) -> KeyError {
    use os_core::config::ConfigError;
    match error {
        ConfigError::BadKey | ConfigError::BadValue => KeyError::BadArgument,
        ConfigError::NoSuchKey => KeyError::NotFound,
        _ => KeyError::NoSpace,
    }
}

/// The value of the entry at `prefix + "/" + name` (or `name` alone for the
/// root), case-insensitively, as REG_SZ data.
fn lookup_value(prefix: &str, name: &str) -> Option<String> {
    let joined = joined_entry(prefix, name);
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

/// `RegCreateKeyExA` backend. Lazy: a key exists only while entries live
/// under it, so nothing is written (and nothing persists) here. The returned
/// flag is the `REG_CREATED_NEW_KEY` disposition (`true`) versus
/// `REG_OPENED_EXISTING_KEY` (`false`): a non-empty subkey is "new" when the
/// prefix has no children; an empty subkey opens the parent itself on a new
/// handle and always reports "existing".
pub fn create(parent: u64, subkey: &str) -> Result<(u64, bool), KeyError> {
    let prefix = prefix_of(parent)?;
    let target = if subkey.is_empty() {
        prefix
    } else if prefix.is_empty() {
        String::from(subkey)
    } else {
        let mut joined = prefix;
        joined.push('/');
        joined.push_str(subkey);
        joined
    };
    if !subkey.is_empty() && !key_is_valid(&target) {
        return Err(KeyError::BadArgument);
    }
    let created = !subkey.is_empty() && !prefix_has_children(&target);
    let mut keys = KEYS.lock();
    match keys.iter().position(Option::is_none) {
        Some(slot) => {
            keys[slot] = Some(target);
            Ok((HANDLE_BASE + slot as u64, created))
        }
        None => Err(KeyError::TooManyKeys),
    }
}

/// `RegSetValueExA` backend: write the (already-validated) REG_SZ payload at
/// `prefix + "/" + name` and persist the store into `/etc/config`.
pub fn set_value(parent: u64, name: &str, value: &str) -> Result<(), KeyError> {
    if name.is_empty() {
        return Err(KeyError::BadArgument);
    }
    let prefix = prefix_of(parent)?;
    let joined = joined_entry(&prefix, name);
    if !key_is_valid(&joined) {
        return Err(KeyError::BadArgument);
    }
    let stored = config::with(|store| store.set(&joined, value)).map_err(|_| KeyError::NoSpace)?;
    stored.map_err(write_error)?;
    config::save().map_err(|_| KeyError::NoSpace)
}

/// `RegDeleteValueA` backend: remove the entry at `prefix + "/" + name` and
/// persist the store. A missing entry is `NotFound`; when the last entry
/// under a prefix is removed the key ceases to exist (children-only rule).
pub fn delete_value(parent: u64, name: &str) -> Result<(), KeyError> {
    if name.is_empty() {
        return Err(KeyError::BadArgument);
    }
    let prefix = prefix_of(parent)?;
    let joined = joined_entry(&prefix, name);
    if !key_is_valid(&joined) {
        return Err(KeyError::BadArgument);
    }
    let removed = config::with(|store| store.remove(&joined)).map_err(|_| KeyError::NoSpace)?;
    removed.map_err(write_error)?;
    config::save().map_err(|_| KeyError::NoSpace)
}

/// Store entries under `prefix` (case-insensitively) in the store's
/// canonical ascending byte order, each with the tail after the `prefix/`
/// separator. The root (`""`) yields every entry with its full path as the
/// tail.
fn entries_under(prefix: &str) -> Vec<(String, String)> {
    let Ok(entries) = config::list(None) else {
        return Vec::new();
    };
    let mut matched = Vec::new();
    for (key, value) in entries {
        if prefix.is_empty() {
            matched.push((key, value));
            continue;
        }
        let bytes = key.as_bytes();
        if bytes.len() > prefix.len()
            && bytes[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
            && bytes[prefix.len()] == b'/'
        {
            matched.push((String::from(&key[prefix.len() + 1..]), value));
        }
    }
    matched
}

/// Distinct immediate child-key names of `prefix`, in the store's canonical
/// order with the stored spelling; a tail counts as a child key when it
/// contains a `/` (the flattened model's rule,
/// `docs/WIN32_REGISTRY_ENUM.md` §5).
fn child_names(prefix: &str) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    for (tail, _) in entries_under(prefix) {
        let Some((first, _)) = tail.split_once('/') else {
            continue;
        };
        if first.is_empty() {
            continue;
        }
        if !names.iter().any(|name| name.eq_ignore_ascii_case(first)) {
            names.push(String::from(first));
        }
    }
    names
}

/// `RegEnumKeyExA` backend: the child key name at `index`, or
/// `NoMoreItems` past the end. Handles resolve to their prefix as usual.
pub fn enum_key(parent: u64, index: u32) -> Result<String, KeyError> {
    let prefix = prefix_of(parent)?;
    let names = child_names(&prefix);
    names
        .get(index as usize)
        .cloned()
        .ok_or(KeyError::NoMoreItems)
}

/// `RegEnumValueA` backend: the `(name, REG_SZ payload)` at `index`, or
/// `NoMoreItems` past the end. Names are the full remaining tail, so a
/// `/`-containing value name is also visible as a child key (stated
/// consequence of the flattened store).
pub fn enum_value(parent: u64, index: u32) -> Result<(String, String), KeyError> {
    let prefix = prefix_of(parent)?;
    let entries = entries_under(&prefix);
    entries
        .get(index as usize)
        .cloned()
        .ok_or(KeyError::NoMoreItems)
}

/// `RegDeleteKeyA` backend: non-recursive key removal. The key must exist
/// and have no child keys (`AccessDenied` otherwise, per the fetched rule);
/// its direct entries are removed and the store is persisted. Open handles
/// to the deleted prefix stay valid but stale (queries miss, enumeration
/// ends), a stated deviation from Windows' deferred removal.
pub fn delete_key(parent: u64, subkey: &str) -> Result<(), KeyError> {
    if subkey.is_empty() {
        return Err(KeyError::BadArgument);
    }
    let prefix = prefix_of(parent)?;
    let target = if prefix.is_empty() {
        String::from(subkey)
    } else {
        let mut joined = prefix;
        joined.push('/');
        joined.push_str(subkey);
        joined
    };
    if !key_is_valid(&target) {
        return Err(KeyError::BadArgument);
    }
    let entries = entries_under(&target);
    if entries.is_empty() {
        return Err(KeyError::NotFound);
    }
    if entries.iter().any(|(tail, _)| tail.contains('/')) {
        return Err(KeyError::AccessDenied);
    }
    for (tail, _) in &entries {
        config::remove(&joined_entry(&target, tail)).map_err(|_| KeyError::NoSpace)?;
    }
    config::save().map_err(|_| KeyError::NoSpace)
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
