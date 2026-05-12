use super::*;
use std::sync::Mutex;

/// Global mutex to serialize tests that mutate $HOME.
static HOME_LOCK: Mutex<()> = Mutex::new(());

/// Helper: set HOME env var (unsafe in Rust 2024 edition).
unsafe fn set_home(path: &std::path::Path) {
    unsafe { std::env::set_var("HOME", path) };
}

/// Helper: restore HOME env var.
unsafe fn restore_home(prev: Option<String>) {
    unsafe {
        match prev {
            Some(h) => std::env::set_var("HOME", h),
            None => std::env::remove_var("HOME"),
        }
    }
}

#[test]
fn resolve_secrets_in_cluster_config() {
    let _lock = HOME_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();

    // Point HOME to tempdir so default_path() => tempdir/.orca/secrets.json
    let prev_home = std::env::var("HOME").ok();
    // SAFETY: we hold HOME_LOCK so no other test mutates HOME concurrently.
    unsafe { set_home(dir.path()) };

    // Set up a secret in the store at the default path
    let secrets_path = crate::secrets::default_path();
    let mut store = crate::secrets::SecretStore::open(&secrets_path).unwrap();
    store.set("MY_API_KEY", "sk-test-12345").unwrap();
    drop(store);

    // Write a cluster.toml that references the secret
    let toml_path = dir.path().join("cluster.toml");
    std::fs::write(
        &toml_path,
        r#"
[cluster]
name = "test"

[ai]
provider = "openai"
api_key = "${secrets.MY_API_KEY}"
model = "gpt-4"
"#,
    )
    .unwrap();

    let config = ClusterConfig::load(&toml_path).unwrap();

    // SAFETY: restoring HOME to original value under lock.
    unsafe { restore_home(prev_home) };

    let ai = config.ai.unwrap();
    assert_eq!(ai.api_key.as_deref(), Some("sk-test-12345"));
    assert_eq!(ai.model.as_deref(), Some("gpt-4"));
}

#[test]
fn resolve_secrets_leaves_plain_values_intact() {
    let _lock = HOME_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();

    let prev_home = std::env::var("HOME").ok();
    // SAFETY: we hold HOME_LOCK so no other test mutates HOME concurrently.
    unsafe { set_home(dir.path()) };

    // Ensure default secrets path exists (empty store)
    let secrets_path = crate::secrets::default_path();
    let _store = crate::secrets::SecretStore::open(&secrets_path).unwrap();

    let toml_path = dir.path().join("cluster.toml");
    std::fs::write(
        &toml_path,
        r#"
[cluster]
name = "test"

[ai]
provider = "ollama"
endpoint = "http://localhost:11434"
"#,
    )
    .unwrap();

    let config = ClusterConfig::load(&toml_path).unwrap();

    // SAFETY: restoring HOME to original value under lock.
    unsafe { restore_home(prev_home) };

    let ai = config.ai.unwrap();
    assert_eq!(ai.endpoint.as_deref(), Some("http://localhost:11434"));
    assert_eq!(ai.provider, "ollama");
}

#[test]
fn resolve_secrets_resolves_backup_s3_credentials() {
    let _lock = HOME_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();

    let prev_home = std::env::var("HOME").ok();
    unsafe { set_home(dir.path()) };

    let secrets_path = crate::secrets::default_path();
    let mut store = crate::secrets::SecretStore::open(&secrets_path).unwrap();
    store.set("S3_ACCESS_KEY", "AKID123").unwrap();
    store.set("S3_SECRET_KEY", "SECRET456").unwrap();
    drop(store);

    let toml_path = dir.path().join("cluster.toml");
    std::fs::write(
        &toml_path,
        r#"
[cluster]
name = "test"

[backup]
schedule = "0 0 3 * * *"
retention_days = 14

[[backup.targets]]
type = "s3"
bucket = "my-bucket"
region = "us-east-1"
access_key = "${secrets.S3_ACCESS_KEY}"
secret_key = "${secrets.S3_SECRET_KEY}"
"#,
    )
    .unwrap();

    let config = ClusterConfig::load(&toml_path).unwrap();

    unsafe { restore_home(prev_home) };

    let backup = config.backup.unwrap();
    match &backup.targets[0] {
        BackupTarget::S3 {
            access_key,
            secret_key,
            ..
        } => {
            assert_eq!(access_key.as_deref(), Some("AKID123"));
            assert_eq!(secret_key.as_deref(), Some("SECRET456"));
        }
        _ => panic!("expected S3 target"),
    }
}
