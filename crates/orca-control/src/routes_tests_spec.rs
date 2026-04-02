use super::*;
use std::collections::HashMap;
use std::sync::Mutex;

use orca_core::config::ServiceConfig;
use orca_core::types::Replicas;

/// Mutex to serialize tests that use set_current_dir (process-global state).
static CWD_LOCK: Mutex<()> = Mutex::new(());

fn minimal_config(image: Option<String>, module: Option<String>) -> ServiceConfig {
    ServiceConfig {
        name: "test-svc".to_string(),
        project: None,
        runtime: Default::default(),
        image,
        module,
        replicas: Replicas::Fixed(1),
        port: Some(8080),
        domain: Some("test.example.com".to_string()),
        health: None,
        readiness: None,
        liveness: None,
        env: HashMap::new(),
        resources: None,
        volume: None,
        deploy: None,
        placement: None,
        network: None,
        aliases: vec![],
        mounts: vec![],
        routes: vec![],
        host_port: None,
        triggers: Vec::new(),
        assets: None,
        build: None,
        tls_cert: None,
        tls_key: None,
        internal: false,
    }
}

#[test]
fn config_to_spec_with_image() {
    let config = minimal_config(Some("nginx:latest".to_string()), None);
    let spec = service_config_to_spec(&config).unwrap();
    assert_eq!(spec.name, "test-svc");
    assert_eq!(spec.image, "nginx:latest");
    assert_eq!(spec.port, Some(8080));
    assert_eq!(spec.domain.as_deref(), Some("test.example.com"));
}

#[test]
fn config_to_spec_errors_without_image_or_module() {
    let config = minimal_config(None, None);
    let result = service_config_to_spec(&config);
    assert!(result.is_err());
    let err_msg = result.unwrap_err().to_string();
    assert!(
        err_msg.contains("no image, module, or build config"),
        "unexpected error: {err_msg}"
    );
}

#[test]
fn config_to_spec_with_build_config() {
    let mut config = minimal_config(None, None);
    config.build = Some(orca_core::config::BuildConfig {
        repo: "git@github.com:org/repo.git".to_string(),
        branch: None,
        dockerfile: None,
        context: None,
    });
    let spec = service_config_to_spec(&config).unwrap();
    assert!(spec.image.starts_with("orca-build-test-svc:"));
    assert!(spec.build.is_some());
}

/// Secret patterns in env vars must be resolved by service_config_to_spec.
#[test]
fn config_to_spec_resolves_secrets() {
    let _lock = CWD_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let secrets_path = dir.path().join("secrets.json");

    let mut store = orca_core::secrets::SecretStore::open(&secrets_path).unwrap();
    store.set("DB_PASS", "hunter2").unwrap();
    drop(store);

    let original_dir = std::env::current_dir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();

    let mut config = minimal_config(Some("postgres:16".into()), None);
    config
        .env
        .insert("POSTGRES_PASSWORD".into(), "${secrets.DB_PASS}".into());
    config.env.insert("PLAIN".into(), "unchanged".into());

    let spec = service_config_to_spec(&config).unwrap();
    assert_eq!(spec.env["POSTGRES_PASSWORD"], "hunter2");
    assert_eq!(spec.env["PLAIN"], "unchanged");

    std::env::set_current_dir(original_dir).unwrap();
}

/// When no secrets file exists, env vars pass through unchanged.
#[test]
fn config_to_spec_no_secrets_file_passes_env_through() {
    let _lock = CWD_LOCK.lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let original_dir = std::env::current_dir().unwrap();
    std::env::set_current_dir(dir.path()).unwrap();

    let mut config = minimal_config(Some("nginx:latest".into()), None);
    config
        .env
        .insert("SECRET_VAR".into(), "${secrets.MISSING}".into());

    let spec = service_config_to_spec(&config).unwrap();
    assert_eq!(spec.env["SECRET_VAR"], "${secrets.MISSING}");

    std::env::set_current_dir(original_dir).unwrap();
}
