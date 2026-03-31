use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// Simple file-backed secret store.
///
/// Secrets are stored as a JSON file with restrictive file permissions (0600).
/// Encryption will be added in a future milestone.
#[derive(Debug, Serialize, Deserialize)]
pub struct SecretStore {
    #[serde(skip)]
    path: PathBuf,
    secrets: HashMap<String, String>,
}

impl SecretStore {
    /// Open an existing secrets file or create a new empty one.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();

        if path.exists() {
            let data = std::fs::read_to_string(&path).context("failed to read secrets file")?;
            let mut store: SecretStore =
                serde_json::from_str(&data).context("failed to parse secrets file")?;
            store.path = path;
            Ok(store)
        } else {
            let store = SecretStore {
                path,
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
    fn save(&self) -> Result<()> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent).context("failed to create secrets directory")?;
        }

        let data = serde_json::to_string_pretty(&self).context("failed to serialize secrets")?;
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
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn temp_store() -> (SecretStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secrets.json");
        let store = SecretStore::open(&path).unwrap();
        (store, dir)
    }

    #[test]
    fn set_and_get() {
        let (mut store, _dir) = temp_store();
        store.set("DB_PASS", "hunter2").unwrap();
        assert_eq!(store.get("DB_PASS"), Some("hunter2"));
        assert_eq!(store.get("MISSING"), None);
    }

    #[test]
    fn set_overwrites() {
        let (mut store, _dir) = temp_store();
        store.set("KEY", "v1").unwrap();
        store.set("KEY", "v2").unwrap();
        assert_eq!(store.get("KEY"), Some("v2"));
    }

    #[test]
    fn remove_secret() {
        let (mut store, _dir) = temp_store();
        store.set("KEY", "val").unwrap();
        assert!(store.remove("KEY").unwrap());
        assert!(!store.remove("KEY").unwrap());
        assert_eq!(store.get("KEY"), None);
    }

    #[test]
    fn list_keys_sorted() {
        let (mut store, _dir) = temp_store();
        store.set("BETA", "2").unwrap();
        store.set("ALPHA", "1").unwrap();
        store.set("GAMMA", "3").unwrap();
        assert_eq!(store.list(), vec!["ALPHA", "BETA", "GAMMA"]);
    }

    #[test]
    fn resolve_env_replaces_patterns() {
        let (mut store, _dir) = temp_store();
        store.set("DB_PASS", "s3cret").unwrap();
        store.set("API_KEY", "abc123").unwrap();

        let mut env = HashMap::new();
        env.insert(
            "DATABASE_URL".into(),
            "postgres://user:${secrets.DB_PASS}@db/app".into(),
        );
        env.insert("KEY".into(), "${secrets.API_KEY}".into());
        env.insert("PLAIN".into(), "no-secrets-here".into());
        env.insert("UNKNOWN".into(), "${secrets.MISSING}".into());

        let resolved = store.resolve_env(&env);
        assert_eq!(resolved["DATABASE_URL"], "postgres://user:s3cret@db/app");
        assert_eq!(resolved["KEY"], "abc123");
        assert_eq!(resolved["PLAIN"], "no-secrets-here");
        assert_eq!(resolved["UNKNOWN"], "${secrets.MISSING}");
    }

    #[test]
    fn persistence_across_opens() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secrets.json");

        {
            let mut store = SecretStore::open(&path).unwrap();
            store.set("PERSIST", "yes").unwrap();
        }

        let store2 = SecretStore::open(&path).unwrap();
        assert_eq!(store2.get("PERSIST"), Some("yes"));
    }

    #[cfg(unix)]
    #[test]
    fn file_permissions_are_600() {
        use std::os::unix::fs::PermissionsExt;
        let (store, _dir) = temp_store();
        let meta = std::fs::metadata(&store.path).unwrap();
        assert_eq!(meta.permissions().mode() & 0o777, 0o600);
    }
}
