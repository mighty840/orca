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

/// Write `cluster.toml` into `home` and load it.
fn load_with(home: &TempHome, body: &str) -> ClusterConfig {
    let toml_path = home.path().join("cluster.toml");
    // Body first: top-level keys like `api_tokens` must precede any table.
    std::fs::write(&toml_path, format!("{body}\n[cluster]\nname = \"test\"\n")).unwrap();
    ClusterConfig::load(&toml_path).unwrap()
}

/// #226: a `[[token]]` whose secret is missing used to be loaded as the
/// literal `${secrets.X}`, a valid admin token for anyone who had read
/// cluster.toml. It must be dropped instead.
#[test]
fn token_with_a_missing_secret_is_dropped_not_literal() {
    let home = TempHome::new();
    let mut store = crate::secrets::SecretStore::open(crate::secrets::default_path()).unwrap();
    store.set("CI_TOKEN", "ci-secret-value").unwrap();
    drop(store);

    let config = load_with(
        &home,
        r#"
api_tokens = ["${secrets.LEGACY_MISSING}", "plain-legacy"]

[[token]]
name = "laptop"
value = "${secrets.ORCA_LAPTOP_TOKEN}"
role = "admin"

[[token]]
name = "ci"
value = "${secrets.CI_TOKEN}"
role = "deployer"

[[token]]
name = "empty"
value = ""
"#,
    );

    let tokens: Vec<(&str, &str)> = config
        .token
        .iter()
        .map(|t| (t.name.as_str(), t.value.as_str()))
        .collect();
    assert_eq!(tokens, [("ci", "ci-secret-value")]);
    assert_eq!(config.api_tokens, ["plain-legacy"]);
    assert!(
        !config.token.iter().any(|t| t.value.contains("${secrets.")),
        "no unresolved reference may survive as a token"
    );
}

/// A non-credential field keeps its value when the secret is missing: it
/// fails visibly where it's used instead of silently disappearing.
#[test]
fn non_token_field_with_a_missing_secret_is_left_as_is() {
    let home = TempHome::new();
    let _store = crate::secrets::SecretStore::open(crate::secrets::default_path()).unwrap();

    let config = load_with(
        &home,
        "\n[ai]\nprovider = \"openai\"\napi_key = \"${secrets.NOPE}\"\n",
    );
    assert_eq!(
        config.ai.unwrap().api_key.as_deref(),
        Some("${secrets.NOPE}")
    );
}
