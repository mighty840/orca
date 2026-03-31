use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::Write;
use std::path::{Path, PathBuf};

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

/// XOR `data` with a repeating `key`.
fn xor_bytes(data: &[u8], key: &[u8]) -> Vec<u8> {
    if key.is_empty() {
        return data.to_vec();
    }
    data.iter()
        .enumerate()
        .map(|(i, b)| b ^ key[i % key.len()])
        .collect()
}

/// Hex-encode bytes to a string.
fn base64_encode(data: &[u8]) -> String {
    let mut s = String::with_capacity(data.len() * 2);
    for b in data {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Hex-decode a string to bytes.
fn base64_decode(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .filter_map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect()
}

/// Load or generate the master key at the given path.
fn load_or_create_key(path: &Path) -> Result<Vec<u8>> {
    if path.exists() {
        std::fs::read(path).context("failed to read master key")
    } else {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).context("failed to create key directory")?;
        }
        let mut key = vec![0u8; 32];
        use std::io::Read;
        std::fs::File::open("/dev/urandom")
            .context("failed to open /dev/urandom")?
            .read_exact(&mut key)
            .context("failed to read random bytes")?;
        std::fs::write(path, &key).context("failed to write master key")?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(path, perms).context("failed to set key permissions")?;
        }
        Ok(key)
    }
}

/// Simple file-backed secret store.
///
/// Secrets are stored as a JSON file with restrictive file permissions (0600).
/// Values are XOR-encrypted with a master key to prevent plaintext exposure.
#[derive(Debug, Serialize, Deserialize)]
pub struct SecretStore {
    #[serde(skip)]
    path: PathBuf,
    #[serde(skip)]
    master_key: Vec<u8>,
    secrets: HashMap<String, String>,
}

impl SecretStore {
    /// Open an existing secrets file or create a new empty one.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        Self::open_with_key(path, &default_master_key_path())
    }

    /// Open with a specific master key path (useful for testing).
    pub fn open_with_key(path: impl AsRef<Path>, key_path: &Path) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        let master_key = load_or_create_key(key_path)?;

        if path.exists() {
            let data = std::fs::read_to_string(&path).context("failed to read secrets file")?;
            let mut store: SecretStore =
                serde_json::from_str(&data).context("failed to parse secrets file")?;
            store.path = path;
            store.master_key = master_key.clone();
            // Decrypt values in memory
            store.secrets = store
                .secrets
                .into_iter()
                .map(|(k, v)| {
                    let encrypted = base64_decode(&v);
                    let decrypted = xor_bytes(&encrypted, &master_key);
                    (k, String::from_utf8_lossy(&decrypted).to_string())
                })
                .collect();
            Ok(store)
        } else {
            let store = SecretStore {
                path,
                master_key,
                secrets: HashMap::new(),
            };
            store.save()?;
            Ok(store)
        }
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

    /// Replace `${secrets.KEY}` patterns in env-var values with actual secret values.
    ///
    /// Unknown keys are left as-is so the caller can detect unresolved references.
    pub fn resolve_env(&self, env: &HashMap<String, String>) -> HashMap<String, String> {
        env.iter()
            .map(|(k, v)| (k.clone(), self.resolve_value(v)))
            .collect()
    }

    /// Persist secrets to disk with restrictive permissions.
    /// Values are XOR-encrypted with the master key before serialization.
    fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).context("failed to create secrets directory")?;
        }

        // Encrypt values for serialization
        let encrypted_secrets: HashMap<String, String> = self
            .secrets
            .iter()
            .map(|(k, v)| {
                let encrypted = xor_bytes(v.as_bytes(), &self.master_key);
                (k.clone(), base64_encode(&encrypted))
            })
            .collect();

        let on_disk = serde_json::json!({ "secrets": encrypted_secrets });
        let data = serde_json::to_string_pretty(&on_disk).context("failed to serialize secrets")?;
        std::fs::write(&self.path, &data).context("failed to write secrets file")?;

        // Set file permissions to 0600 on Unix
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            std::fs::set_permissions(&self.path, perms)
                .context("failed to set secrets file permissions")?;
        }

        Ok(())
    }

    /// Resolve `${secrets.KEY}` patterns in a single string value.
    fn resolve_value(&self, value: &str) -> String {
        let mut result = value.to_string();
        let mut search_from = 0;
        loop {
            let Some(start) = result[search_from..].find("${secrets.") else {
                break;
            };
            let abs_start = search_from + start;
            let after_prefix = abs_start + "${secrets.".len();
            let Some(end) = result[after_prefix..].find('}') else {
                break;
            };
            let key = result[after_prefix..after_prefix + end].to_string();
            if let Some(secret_value) = self.secrets.get(&key) {
                result = format!(
                    "{}{}{}",
                    &result[..abs_start],
                    secret_value,
                    &result[after_prefix + end + 1..]
                );
                // Don't advance search_from — replacement might be shorter
            } else {
                // Key not found, skip past this pattern to avoid infinite loop
                search_from = after_prefix + end + 1;
            }
        }
        result
    }
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
