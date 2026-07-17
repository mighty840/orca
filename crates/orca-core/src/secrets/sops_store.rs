//! SOPS/age-encrypted secrets backend (#109).
//!
//! Secrets live as a SOPS-format JSON file in the config repo: keys stay
//! plaintext (readable `git diff`), values are AES-256-GCM encrypted with a
//! data key wrapped for each age recipient. Decryption happens in-process
//! via the `rops` crate — no external `sops`/`age` binary at runtime.
//!
//! Saves reuse the file's existing data key and per-value nonces
//! (`encrypt_with_saved_parameters`), so untouched values stay
//! byte-identical across mutations and diffs show only what changed.
//!
//! The age identity is supplied through the environment: `rops` reads
//! `ROPS_AGE` (comma-separated identities) or `ROPS_AGE_KEY_FILE` — note
//! these are NOT the `SOPS_AGE_*` names. `secrets::configure` bridges the
//! `[secrets].age_key_file` setting to `ROPS_AGE` at startup.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::str::FromStr;

use anyhow::{Context, Result};
use rops::cryptography::cipher::AES256GCM;
use rops::cryptography::hasher::SHA512;
use rops::file::RopsFile;
use rops::file::builder::RopsFileBuilder;
use rops::file::format::{JsonFileFormat, RopsFileFormatMap};
use rops::file::state::{DecryptedFile, EncryptedFile};
use rops::integration::{AgeIntegration, Integration};

type EncryptedRops = RopsFile<EncryptedFile<AES256GCM, SHA512>, JsonFileFormat>;
type DecryptedRops = RopsFile<DecryptedFile<SHA512>, JsonFileFormat>;

/// File-level backend: load-and-decrypt / re-encrypt-and-save a flat
/// `KEY → value` map. The in-memory model stays `SecretStore`'s plain
/// `HashMap`; this type only owns the on-disk envelope.
#[derive(Debug, Clone)]
pub(crate) struct SopsBackend {
    pub(crate) path: PathBuf,
    recipients: Vec<String>,
    git_autocommit: bool,
}

impl SopsBackend {
    pub(crate) fn new(path: PathBuf, recipients: Vec<String>, git_autocommit: bool) -> Self {
        Self {
            path,
            recipients,
            git_autocommit,
        }
    }

    /// Decrypt the file into a flat map. A missing file is an empty store
    /// (created on first mutation).
    pub(crate) fn load(&self) -> Result<HashMap<String, String>> {
        if !self.path.exists() {
            return Ok(HashMap::new());
        }
        let raw = std::fs::read_to_string(&self.path)
            .with_context(|| format!("failed to read {}", self.path.display()))?;
        let encrypted = EncryptedRops::from_str(&raw)
            .map_err(|e| anyhow::anyhow!("failed to parse {}: {e}", self.path.display()))?;
        let decrypted: DecryptedRops = encrypted.decrypt::<JsonFileFormat>().map_err(|e| {
            anyhow::anyhow!(
                "failed to decrypt {}: {e} — is a matching age identity available \
                 ([secrets].age_key_file, ROPS_AGE, or ROPS_AGE_KEY_FILE)?",
                self.path.display()
            )
        })?;
        Ok(decrypted
            .into_inner_map()
            .into_iter()
            .map(|(k, v)| {
                let val = match v {
                    serde_json::Value::String(s) => s,
                    other => other.to_string(),
                };
                (k, val)
            })
            .collect())
    }

    /// Re-encrypt the full map and write it back. Reuses the existing data
    /// key + nonces when the file exists (stable diffs); creates a fresh
    /// file encrypted to the configured recipients otherwise.
    pub(crate) fn save(&self, secrets: &HashMap<String, String>) -> Result<()> {
        // Sort keys so the serialized file is deterministic — diffs must
        // reflect value changes, never map-iteration order.
        let sorted: BTreeMap<&String, &String> = secrets.iter().collect();
        let mut json_map = serde_json::Map::new();
        for (k, v) in sorted {
            json_map.insert(k.clone(), serde_json::Value::String(v.clone()));
        }

        let serialized = if self.path.exists() {
            let raw = std::fs::read_to_string(&self.path)
                .with_context(|| format!("failed to read {}", self.path.display()))?;
            let encrypted = EncryptedRops::from_str(&raw)
                .map_err(|e| anyhow::anyhow!("failed to parse {}: {e}", self.path.display()))?;
            let (decrypted, saved_params) = encrypted
                .decrypt_and_save_parameters::<JsonFileFormat>()
                .map_err(|e| anyhow::anyhow!("failed to decrypt for update: {e}"))?;
            let updated = decrypted
                .set_map(RopsFileFormatMap::from_inner_map(json_map))
                .map_err(|e| anyhow::anyhow!("failed to apply secrets update: {e}"))?;
            updated
                .encrypt_with_saved_parameters::<AES256GCM, JsonFileFormat>(saved_params)
                .map_err(|e| anyhow::anyhow!("failed to re-encrypt secrets: {e}"))?
                .to_string()
        } else {
            anyhow::ensure!(
                !self.recipients.is_empty(),
                "cannot create {}: no [secrets].age_recipients configured — \
                 add at least the master's age public key",
                self.path.display()
            );
            let mut builder = RopsFileBuilder::<JsonFileFormat>::from_map(json_map);
            for recipient in &self.recipients {
                let key_id = AgeIntegration::parse_key_id(recipient).map_err(|e| {
                    anyhow::anyhow!("invalid age recipient {recipient:?} in [secrets]: {e}")
                })?;
                builder = builder.add_integration_key::<AgeIntegration>(key_id);
            }
            builder
                .encrypt::<AES256GCM, SHA512>()
                .map_err(|e| anyhow::anyhow!("failed to encrypt new secrets file: {e}"))?
                .to_string()
        };

        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).context("failed to create secrets directory")?;
        }
        std::fs::write(&self.path, &serialized)
            .with_context(|| format!("failed to write {}", self.path.display()))?;

        if self.git_autocommit {
            super::git_sync::autocommit_and_push(&self.path);
        }
        Ok(())
    }
}
