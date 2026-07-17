mod git_sync;
mod refs;
mod sops_store;
#[cfg(test)]
#[path = "sops_store_tests.rs"]
mod sops_store_tests;
mod store;

pub use refs::{SecretReference, extract_refs};
pub use store::SecretStore;

use std::path::Path;
use std::sync::RwLock;

use sops_store::SopsBackend;

/// Backend selected by `[secrets]` in cluster.toml, installed at config
/// load. `RwLock` (not `OnceLock`) so `orca reload` can re-configure.
static SOPS: RwLock<Option<SopsBackend>> = RwLock::new(None);

/// Canonical on-disk location for the legacy secrets store:
/// `~/.orca/secrets.json`.
///
/// Falls back to `./secrets.json` only if `$HOME` is unset (test/CI fallback).
/// All CLI and server code should resolve secrets via this path so that
/// `orca secrets set` and `orca server` always agree on a single file,
/// regardless of the current working directory.
pub fn default_path() -> std::path::PathBuf {
    match std::env::var("HOME") {
        Ok(home) => {
            let dir = std::path::PathBuf::from(home).join(".orca");
            let _ = std::fs::create_dir_all(&dir);
            dir.join("secrets.json")
        }
        Err(_) => std::path::PathBuf::from("secrets.json"),
    }
}

/// Install the SOPS/age backend from `[secrets]` in cluster.toml (#109).
/// `base_dir` is the directory containing cluster.toml — relative
/// `encrypted_file` paths resolve against it, keeping the store inside the
/// config repo. Called by `ClusterConfig::load`; callers then get the
/// encrypted backend from [`open_configured`].
pub fn configure(cfg: &crate::config::SecretsConfig, base_dir: &Path) {
    let path = {
        let p = Path::new(&cfg.encrypted_file);
        if p.is_absolute() {
            p.to_path_buf()
        } else {
            base_dir.join(p)
        }
    };

    // Bridge [secrets].age_key_file to the env var rops actually reads.
    // rops looks for ROPS_AGE / ROPS_AGE_KEY_FILE — not the SOPS_AGE_*
    // names — and offers no API to pass an identity explicitly.
    if let Some(key_file) = &cfg.age_key_file
        && std::env::var("ROPS_AGE").is_err()
    {
        match std::fs::read_to_string(key_file) {
            Ok(contents) => {
                let identities: Vec<&str> = contents
                    .lines()
                    .map(str::trim)
                    .filter(|l| l.starts_with("AGE-SECRET-KEY-"))
                    .collect();
                if identities.is_empty() {
                    tracing::error!(
                        key_file,
                        "no age identities found in [secrets].age_key_file"
                    );
                } else {
                    // SAFETY: configure() runs during startup config load
                    // (single-threaded, before the async runtime spawns
                    // worker threads that might read the environment).
                    unsafe { std::env::set_var("ROPS_AGE", identities.join(",")) };
                }
            }
            Err(e) => {
                tracing::error!(key_file, "failed to read [secrets].age_key_file: {e}");
            }
        }
    }

    let backend = SopsBackend::new(path, cfg.age_recipients.clone(), cfg.git_autocommit);
    *SOPS.write().expect("secrets backend lock poisoned") = Some(backend);
}

/// Whether the SOPS/age backend is active (i.e. `[secrets]` was configured).
pub fn sops_configured() -> bool {
    SOPS.read()
        .expect("secrets backend lock poisoned")
        .is_some()
}

/// Open the configured secrets backend: the SOPS/age store when `[secrets]`
/// is set in cluster.toml, the legacy machine-local AES store otherwise.
/// This is the entry point every production caller should use.
pub fn open_configured() -> anyhow::Result<SecretStore> {
    let backend = SOPS.read().expect("secrets backend lock poisoned").clone();
    match backend {
        Some(b) => SecretStore::open_sops(b),
        None => SecretStore::open(default_path()),
    }
}
