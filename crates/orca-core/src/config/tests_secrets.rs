use super::test_home::TempHome;
use super::*;

#[test]
fn resolve_secrets_in_cluster_config() {
    let home = TempHome::new();

    // Set up a secret in the store at the default path
    let secrets_path = crate::secrets::default_path();
    let mut store = crate::secrets::SecretStore::open(&secrets_path).unwrap();
    store.set("MY_API_KEY", "sk-test-12345").unwrap();
    drop(store);

    // Write a cluster.toml that references the secret
    let toml_path = home.path().join("cluster.toml");
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

    let ai = config.ai.unwrap();
    assert_eq!(ai.api_key.as_deref(), Some("sk-test-12345"));
    assert_eq!(ai.model.as_deref(), Some("gpt-4"));
}

#[test]
fn resolve_secrets_leaves_plain_values_intact() {
    let home = TempHome::new();

    // Ensure default secrets path exists (empty store)
    let secrets_path = crate::secrets::default_path();
    let _store = crate::secrets::SecretStore::open(&secrets_path).unwrap();

    let toml_path = home.path().join("cluster.toml");
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

    let ai = config.ai.unwrap();
    assert_eq!(ai.endpoint.as_deref(), Some("http://localhost:11434"));
    assert_eq!(ai.provider, "ollama");
}

#[test]
fn resolve_secrets_resolves_backup_s3_credentials() {
    let home = TempHome::new();

    let secrets_path = crate::secrets::default_path();
    let mut store = crate::secrets::SecretStore::open(&secrets_path).unwrap();
    store.set("S3_ACCESS_KEY", "AKID123").unwrap();
    store.set("S3_SECRET_KEY", "SECRET456").unwrap();
    drop(store);

    let toml_path = home.path().join("cluster.toml");
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
