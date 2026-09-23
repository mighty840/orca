use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[cfg(test)]
use super::crypto::hex_encode;
use super::crypto::{
    StoredFormat, aes_decrypt, aes_encrypt, classify, create_key, hex_decode, read_key, xor_bytes,
};

/// Default master key path.
fn default_master_key_path() -> PathBuf {
    dirs_or_home().join("master.key")
}

/// Returns `~/.orca` or falls back to current dir.
fn dirs_or_home() -> PathBuf {
    std::env::var("HOME")
        .map(|h| PathBuf::from(h).join(".orca"))
        .unwrap_or_else(|_| PathBuf::from(".orca"))
}

/// Suffix of the untouched copy kept before a legacy-format migration.
const PRE_MIGRATION_SUFFIX: &str = "pre-aes-migration";

/// File-backed secret store using AES-256-GCM encryption.
///
/// Secrets are stored as JSON with restrictive file permissions (0600).
/// Values are encrypted with a 32-byte master key. Legacy XOR-encrypted
/// values are auto-migrated to AES-256-GCM on first open.
#[derive(Debug, Serialize, Deserialize)]
pub struct SecretStore {
    #[serde(skip)]
    path: PathBuf,
    #[serde(skip)]
    master_key: Vec<u8>,
    pub(super) secrets: HashMap<String, String>,
    /// When set, this store reads/writes the SOPS/age-encrypted file
    /// (#109) instead of the legacy AES file. The in-memory model and the
    /// public API are identical either way.
    #[serde(skip)]
    sops: Option<super::sops_store::SopsBackend>,
}

impl SecretStore {
    /// Open an existing secrets file or create a new empty one.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with_key(path, &default_master_key_path())
    }

    /// Open the SOPS/age-encrypted backend (#109). Prefer
    /// [`super::open_configured`], which picks the backend from cluster.toml.
    pub(super) fn open_sops(backend: super::sops_store::SopsBackend) -> Result<Self> {
        let secrets = backend.load()?;
        Ok(SecretStore {
            path: backend.path.clone(),
            master_key: Vec::new(),
            secrets,
            sops: Some(backend),
        })
    }

    /// Copy every secret from `other` into this store with a single save —
    /// the migration path from the legacy AES store into the encrypted
    /// file (one commit, not one per key).
    pub fn import_from(&mut self, other: &SecretStore) -> Result<usize> {
        let mut count = 0;
        for key in other.list() {
            if let Some(value) = other.get(&key) {
                self.secrets.insert(key, value.to_string());
                count += 1;
            }
        }
        self.save()?;
        Ok(count)
    }

    /// Open with a specific master key path (useful for testing).
    ///
    /// Fails, and leaves `path` untouched, when the key can't decrypt the
    /// store: a wrong key, a damaged key, or no key next to existing secrets
    /// (#196). Only a missing key with no secrets to protect creates a new one.
    pub fn open_with_key(path: impl AsRef<Path>, key_path: &Path) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let stored: HashMap<String, String> = if path.exists() {
            let data = std::fs::read_to_string(&path).context("failed to read secrets file")?;
            serde_json::from_str::<SecretStore>(&data)
                .context("failed to parse secrets file")?
                .secrets
        } else {
            HashMap::new()
        };

        let master_key = if key_path.exists() {
            read_key(key_path)?
        } else if !stored.is_empty() {
            bail!(
                "master key {} is missing, but {} holds {} encrypted secret(s). A new key \
                 cannot decrypt them: restore the original master.key (orca's backups do \
                 not include it) and start again. {} was not modified.",
                key_path.display(),
                path.display(),
                stored.len(),
                path.display()
            );
        } else {
            create_key(key_path)?
        };

        let mut secrets = HashMap::with_capacity(stored.len());
        let mut legacy = 0usize;
        for (name, value) in stored {
            let plain = match classify(&value) {
                StoredFormat::Aes => aes_decrypt(&value, &master_key).map_err(|e| {
                    anyhow::anyhow!(
                        "cannot decrypt secret '{name}' in {} with master key {} ({e}): the \
                         key is wrong or damaged. Restore the original master.key. {} was \
                         not modified.",
                        path.display(),
                        key_path.display(),
                        path.display()
                    )
                })?,
                StoredFormat::LegacyXor => {
                    legacy += 1;
                    xor_bytes(&hex_decode(&value)?, &master_key)
                }
                StoredFormat::Unknown => bail!(
                    "secret '{name}' in {} is neither AES (nonce:ciphertext) nor legacy hex; \
                     the file is damaged. It was not modified.",
                    path.display()
                ),
            };
            secrets.insert(name, String::from_utf8_lossy(&plain).into_owned());
        }

        let store = SecretStore {
            path,
            master_key,
            secrets,
            sops: None,
        };
        if legacy > 0 {
            store.migrate_legacy(legacy)?;
        } else if !store.path.exists() {
            store.save()?;
        }
        Ok(store)
    }

    /// Re-encrypt legacy XOR values with AES. XOR carries no integrity check,
    /// so a wrong key decrypts legacy values to garbage without any error.
    /// The original file is therefore kept first, never overwritten, so a
    /// migration under the wrong key can be undone by restoring it.
    fn migrate_legacy(&self, count: usize) -> Result<()> {
        let original = std::fs::read(&self.path).context("failed to re-read secrets file")?;
        let keep = PathBuf::from(format!("{}.{PRE_MIGRATION_SUFFIX}", self.path.display()));
        crate::fsutil::create_private_new(&keep, &original)
            .with_context(|| format!("failed to keep {}", keep.display()))?;
        tracing::warn!(
            "Migrating {count} legacy XOR-encrypted secret(s) in {} to AES-256-GCM; the \
             original file is kept at {}",
            self.path.display(),
            keep.display()
        );
        self.save()
    }

    /// Add or update a secret.
    pub fn set(&mut self, key: impl Into<String>, value: impl Into<String>) -> Result<()> {
        self.secrets.insert(key.into(), value.into());
        self.save()
    }

    /// Retrieve a secret by key.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.secrets.get(key).map(|s| s.as_str())
    }

    /// Remove a secret by key. Returns whether the key existed.
    pub fn remove(&mut self, key: &str) -> Result<bool> {
        let existed = self.secrets.remove(key).is_some();
        if existed {
            self.save()?;
        }
        Ok(existed)
    }

    /// List all secret key names (values are not exposed).
    pub fn list(&self) -> Vec<String> {
        let mut keys: Vec<String> = self.secrets.keys().cloned().collect();
        keys.sort();
        keys
    }

    /// Persist secrets to disk, owner-only and atomically (temp file +
    /// rename), so a crash mid-write can't leave a truncated store (#183).
    fn save(&self) -> Result<()> {
        if let Some(backend) = &self.sops {
            return backend.save(&self.secrets);
        }
        let mut encrypted_secrets = HashMap::with_capacity(self.secrets.len());
        for (k, v) in &self.secrets {
            encrypted_secrets.insert(k.clone(), aes_encrypt(v.as_bytes(), &self.master_key)?);
        }
        let on_disk = serde_json::json!({ "secrets": encrypted_secrets });
        let data = serde_json::to_string_pretty(&on_disk).context("failed to serialize secrets")?;
        crate::fsutil::write_private(&self.path, data.as_bytes())
            .context("failed to write secrets file")
    }
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "store_key_tests.rs"]
mod key_tests;
