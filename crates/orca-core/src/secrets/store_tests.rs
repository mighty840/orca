use super::*;
use std::collections::HashMap;

fn temp_store() -> (SecretStore, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("secrets.json");
    let key_path = dir.path().join("master.key");
    let store = SecretStore::open_with_key(&path, &key_path).unwrap();
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
    let key_path = dir.path().join("master.key");
    {
        let mut store = SecretStore::open_with_key(&path, &key_path).unwrap();
        store.set("PERSIST", "yes").unwrap();
    }
    let store2 = SecretStore::open_with_key(&path, &key_path).unwrap();
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
