mod refs;
mod store;

pub use refs::{SecretReference, extract_refs};
pub use store::SecretStore;

/// Canonical on-disk location for the secrets store: `~/.orca/secrets.json`.
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
