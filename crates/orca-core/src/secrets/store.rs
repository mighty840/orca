use aes_gcm::aead::{Aead, OsRng};
use aes_gcm::{AeadCore, Aes256Gcm, Key, KeyInit, Nonce};
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

/// Hex-encode bytes to a string.
fn hex_encode(data: &[u8]) -> String {
    let mut s = String::with_capacity(data.len() * 2);
    for b in data {
        let _ = write!(s, "{b:02x}");
    }
    s
}

/// Hex-decode a string to bytes.
fn hex_decode(hex: &str) -> Vec<u8> {
    (0..hex.len())
        .step_by(2)
        .filter_map(|i| u8::from_str_radix(&hex[i..i + 2], 16).ok())
        .collect()
}

/// XOR `data` with a repeating `key` (legacy, used only for migration).
fn xor_bytes(data: &[u8], key: &[u8]) -> Vec<u8> {
    if key.is_empty() {
        return data.to_vec();
    }
    data.iter()
        .enumerate()
        .map(|(i, b)| b ^ key[i % key.len()])
        .collect()
}

/// Encrypt plaintext with AES-256-GCM. Returns `"nonce_hex:ciphertext_hex"`.
fn aes_encrypt(plaintext: &[u8], key: &[u8]) -> Result<String> {
    let key = Key::<Aes256Gcm>::from_slice(key);
    let cipher = Aes256Gcm::new(key);
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, plaintext)
        .map_err(|e| anyhow::anyhow!("AES encrypt failed: {e}"))?;
    Ok(format!(
        "{}:{}",
        hex_encode(&nonce),
        hex_encode(&ciphertext)
    ))
}

/// Decrypt `"nonce_hex:ciphertext_hex"` with AES-256-GCM.
fn aes_decrypt(encoded: &str, key: &[u8]) -> Result<Vec<u8>> {
    let (nonce_hex, ct_hex) = encoded
        .split_once(':')
        .ok_or_else(|| anyhow::anyhow!("missing nonce:ciphertext separator"))?;
    let nonce_bytes = hex_decode(nonce_hex);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = hex_decode(ct_hex);
    let key = Key::<Aes256Gcm>::from_slice(key);
    let cipher = Aes256Gcm::new(key);
    cipher
        .decrypt(nonce, ciphertext.as_ref())
        .map_err(|e| anyhow::anyhow!("AES decrypt failed: {e}"))
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

/// Decrypt a stored value. Tries AES-256-GCM first, falls back to legacy XOR.
/// Returns `(plaintext, was_legacy)`.
fn decrypt_value(stored: &str, key: &[u8]) -> (String, bool) {
    if let Ok(plain) = aes_decrypt(stored, key) {
        (String::from_utf8_lossy(&plain).to_string(), false)
    } else {
        let encrypted = hex_decode(stored);
        let decrypted = xor_bytes(&encrypted, key);
        (String::from_utf8_lossy(&decrypted).to_string(), true)
    }
}

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
            let mut needs_migration = false;
            store.secrets = store
                .secrets
                .into_iter()
                .map(|(k, v)| {
                    let (plain, was_legacy) = decrypt_value(&v, &master_key);
                    if was_legacy {
                        needs_migration = true;
                    }
                    (k, plain)
                })
                .collect();
            if needs_migration {
                store.save()?;
            }
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
    pub fn resolve_env(&self, env: &HashMap<String, String>) -> HashMap<String, String> {
        env.iter()
            .map(|(k, v)| (k.clone(), self.resolve_value(v)))
            .collect()
    }

    /// Persist secrets to disk with restrictive permissions.
    fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).context("failed to create secrets directory")?;
        }
        let encrypted_secrets: HashMap<String, String> = self
            .secrets
            .iter()
            .map(|(k, v)| {
                let enc = aes_encrypt(v.as_bytes(), &self.master_key)
                    .expect("AES encryption must not fail with valid key");
                (k.clone(), enc)
            })
            .collect();
        let on_disk = serde_json::json!({ "secrets": encrypted_secrets });
        let data = serde_json::to_string_pretty(&on_disk).context("failed to serialize secrets")?;
        std::fs::write(&self.path, &data).context("failed to write secrets file")?;
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
            } else {
                search_from = after_prefix + end + 1;
            }
        }
        result
    }
}

#[cfg(test)]
#[path = "store_tests.rs"]
mod tests;
