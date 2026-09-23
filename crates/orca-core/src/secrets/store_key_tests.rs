//! Opening a store with a wrong, missing or damaged master key (#196) must
//! fail with an actionable error and leave `secrets.json` byte-for-byte
//! untouched.

use super::*;

struct Fixture {
    dir: tempfile::TempDir,
    path: PathBuf,
    key_path: PathBuf,
}

impl Fixture {
    /// A store holding two AES-encrypted secrets under a fresh key.
    fn with_secrets() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("secrets.json");
        let key_path = dir.path().join("master.key");
        let mut store = SecretStore::open_with_key(&path, &key_path).unwrap();
        store.set("DB_PASSWORD", "hunter2").unwrap();
        store.set("SMTP_PASS", "13-byte-value").unwrap();
        Self {
            dir,
            path,
            key_path,
        }
    }

    fn file(&self) -> Vec<u8> {
        std::fs::read(&self.path).unwrap()
    }

    fn open_err(&self) -> String {
        SecretStore::open_with_key(&self.path, &self.key_path)
            .unwrap_err()
            .to_string()
    }
}

#[test]
fn wrong_key_is_an_error_naming_the_key_not_a_panic() {
    let f = Fixture::with_secrets();
    let before = f.file();
    std::fs::write(&f.key_path, [9u8; 32]).unwrap();

    let err = f.open_err();
    assert!(err.contains("key is wrong or damaged"), "{err}");
    assert!(err.contains("master.key"), "{err}");
    assert_eq!(f.file(), before, "secrets.json must not be modified");
}

#[test]
fn missing_key_with_existing_secrets_is_refused_not_regenerated() {
    // The disaster-recovery path: secrets.json restored from backup, but
    // master.key isn't in backups. A new key must not be generated (it
    // would persist and replace the one needed to recover).
    let f = Fixture::with_secrets();
    let before = f.file();
    std::fs::remove_file(&f.key_path).unwrap();

    let err = f.open_err();
    assert!(err.contains("is missing"), "{err}");
    assert!(err.contains("2 encrypted secret(s)"), "{err}");
    assert!(!f.key_path.exists(), "no replacement key may be written");
    assert_eq!(f.file(), before);
}

#[test]
fn truncated_key_is_an_error_not_a_panic() {
    let f = Fixture::with_secrets();
    let before = f.file();
    std::fs::write(&f.key_path, []).unwrap();

    let err = f.open_err();
    assert!(err.contains("0 bytes, expected 32"), "{err}");
    assert_eq!(f.file(), before);
}

#[test]
fn damaged_value_is_an_error_naming_the_secret() {
    let f = Fixture::with_secrets();
    let raw = serde_json::json!({ "secrets": { "BROKEN": "not a secret" } });
    std::fs::write(&f.path, raw.to_string()).unwrap();
    let before = f.file();

    let err = f.open_err();
    assert!(err.contains("'BROKEN'"), "{err}");
    assert_eq!(f.file(), before);
}

#[test]
fn missing_key_without_secrets_still_bootstraps_a_new_store() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("secrets.json");
    let key_path = dir.path().join("master.key");
    let mut store = SecretStore::open_with_key(&path, &key_path).unwrap();
    store.set("A", "1").unwrap();
    assert!(key_path.exists());
    // An empty-but-present file also counts as "nothing to protect".
    let empty = dir.path().join("empty.json");
    std::fs::write(&empty, r#"{"secrets":{}}"#).unwrap();
    let fresh_key = dir.path().join("fresh.key");
    assert!(SecretStore::open_with_key(&empty, &fresh_key).is_ok());
    assert!(fresh_key.exists());
}

#[test]
fn right_key_still_opens_after_the_error_cases() {
    // The whole point: after a failed open, restoring the right key works.
    let f = Fixture::with_secrets();
    let good = std::fs::read(&f.key_path).unwrap();
    std::fs::write(&f.key_path, [9u8; 32]).unwrap();
    assert!(SecretStore::open_with_key(&f.path, &f.key_path).is_err());

    std::fs::write(&f.key_path, &good).unwrap();
    let store = SecretStore::open_with_key(&f.path, &f.key_path).unwrap();
    assert_eq!(store.get("DB_PASSWORD"), Some("hunter2"));
    assert_eq!(store.get("SMTP_PASS"), Some("13-byte-value"));
    drop(f.dir);
}
