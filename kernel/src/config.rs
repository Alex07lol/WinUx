//! Registry-like configuration store (Phase 8, stage 1).
//!
//! A small hierarchical key/value store whose serialised form (`POCF` version
//! 1, `os_core::config`) lives in the ordinary VFS file `/etc/config`. The
//! existing checkpoint persistence carries, verifies and falls back for that
//! file with no format change: `config set`/`config del` write the file, which
//! marks the VFS dirty so the shell boundary flushes one snapshot.
//!
//! Boot seeds the defaults in memory when no file exists and never writes, so
//! a boot with no shell activity leaves the checkpoint byte-identical. The
//! full contract is `docs/CONFIG.md`.

use alloc::string::String;

use os_core::config::{Config, ConfigError};

use crate::sync::IrqSpinLock;
use crate::vfs;

/// Path of the serialised store inside the VFS.
pub const CONFIG_PATH: &str = "/etc/config";

/// Seeded in memory when no file exists; never written at boot.
const DEFAULTS: [(&str, &str); 3] = [
    ("System/Name", "PROJECT_OS"),
    ("Display/Theme", "classic"),
    ("Net/Mode", "static"),
];

/// Temporary key used by the boot self-test; never persisted.
const SELFTEST_KEY: &str = "SelfCheck/Value";

static STORE: IrqSpinLock<Option<Config>> = IrqSpinLock::new(None);

fn default_config() -> Result<Config, &'static str> {
    let mut config = Config::new();
    for (key, value) in DEFAULTS {
        config
            .set(key, value)
            .map_err(|_| "config: default entry rejected")?;
    }
    Ok(config)
}

/// Boot: load `/etc/config` when present, otherwise seed defaults in memory.
pub fn init() -> Result<(), &'static str> {
    if STORE.lock().is_some() {
        return Err("config: already initialised");
    }
    let config = match vfs::with(|vfs| vfs.read_file(CONFIG_PATH).map(|data| data.to_vec()))? {
        Ok(bytes) => Config::decode(&bytes).map_err(|error| error.label())?,
        Err(crate::vfs::VfsError::NotFound) => default_config()?,
        Err(error) => return Err(error.label()),
    };
    *STORE.lock() = Some(config);
    Ok(())
}

/// Run `operation` with the locked store (the VFS pattern).
pub fn with<R>(operation: impl FnOnce(&mut Config) -> R) -> Result<R, &'static str> {
    let mut slot = STORE.lock();
    let config = slot.as_mut().ok_or("config: not initialised")?;
    Ok(operation(config))
}

/// Current entry count and serialised size, for the shell status line.
pub fn status() -> Result<(usize, usize), &'static str> {
    with(|config| (config.len(), config.encode().len()))
}

/// Serialise the store into `/etc/config`; marks the VFS dirty.
pub fn save() -> Result<(), &'static str> {
    let blob = with(|config| config.encode())?;
    vfs::with(|vfs| vfs.write_file(CONFIG_PATH, &blob))?.map_err(|error| error.label())
}

/// Boot self-test: exercises the full API surface and key error paths without
/// touching the file (no VFS dirty bit, no disk write).
pub fn self_test() -> Result<(usize, usize), &'static str> {
    with(|config| -> Result<(usize, usize), &'static str> {
        let before = config.len();
        if config.contains(SELFTEST_KEY) {
            return Err("selftest key already present");
        }
        config
            .set(SELFTEST_KEY, "ok")
            .map_err(|error| error.label())?;
        if config.get(SELFTEST_KEY) != Some("ok") {
            return Err("selftest get mismatch");
        }
        let listed = config.list(Some("SelfCheck"));
        if listed.len() != 1 || listed[0].0 != SELFTEST_KEY {
            return Err("selftest list mismatch");
        }
        if config.set("bad//key", "x") != Err(ConfigError::BadKey) {
            return Err("selftest accepted an invalid key");
        }
        config.remove(SELFTEST_KEY).map_err(|error| error.label())?;
        if config.len() != before || config.contains(SELFTEST_KEY) {
            return Err("selftest did not restore the entry count");
        }
        let blob = config.encode();
        let round_trip = Config::decode(&blob).map_err(|error| error.label())?;
        if round_trip != *config {
            return Err("selftest round-trip mismatch");
        }
        Ok((config.len(), blob.len()))
    })?
}

/// Helper so shell callers can keep `String` values out of the lock guard.
pub fn get(key: &str) -> Result<Option<String>, &'static str> {
    with(|config| config.get(key).map(String::from))
}

pub fn set(key: &str, value: &str) -> Result<(), &'static str> {
    with(|config| config.set(key, value))?.map_err(|error| error.label())
}

pub fn remove(key: &str) -> Result<(), &'static str> {
    with(|config| config.remove(key))?.map_err(|error| error.label())
}

pub fn list(prefix: Option<&str>) -> Result<alloc::vec::Vec<(String, String)>, &'static str> {
    with(|config| {
        config
            .list(prefix)
            .into_iter()
            .map(|(key, value)| (String::from(key), String::from(value)))
            .collect()
    })
}
