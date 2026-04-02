use super::*;

#[test]
fn load_dir_discovers_services() {
    let dir = tempfile::tempdir().unwrap();
    // Create two service subdirs
    let svc_a = dir.path().join("alpha");
    let svc_b = dir.path().join("beta");
    std::fs::create_dir_all(&svc_a).unwrap();
    std::fs::create_dir_all(&svc_b).unwrap();

    std::fs::write(
        svc_a.join("service.toml"),
        r#"
[[service]]
name = "alpha"
image = "nginx:latest"
port = 80
"#,
    )
    .unwrap();

    std::fs::write(
        svc_b.join("service.toml"),
        r#"
[[service]]
name = "beta-db"
image = "postgres:16"
port = 5432

[[service]]
name = "beta-app"
image = "myapp:latest"
port = 3000
"#,
    )
    .unwrap();

    let config = ServicesConfig::load_dir(dir.path()).unwrap();
    assert_eq!(config.service.len(), 3);
    assert_eq!(config.service[0].name, "alpha");
    assert_eq!(config.service[1].name, "beta-db");
    assert_eq!(config.service[2].name, "beta-app");
}

#[test]
fn load_dir_resolves_per_service_secrets() {
    let dir = tempfile::tempdir().unwrap();
    let svc_dir = dir.path().join("myapp");
    std::fs::create_dir_all(&svc_dir).unwrap();

    std::fs::write(
        svc_dir.join("service.toml"),
        r#"
[[service]]
name = "myapp"
image = "myapp:latest"
port = 3000

[service.env]
DB_PASS = "${secrets.DB_PASS}"
PLAIN = "hello"
"#,
    )
    .unwrap();

    // Create secrets.json in the service dir
    let secrets_path = svc_dir.join("secrets.json");
    let mut store = crate::secrets::SecretStore::open(&secrets_path).unwrap();
    store.set("DB_PASS", "s3cret").unwrap();
    drop(store);

    let config = ServicesConfig::load_dir(dir.path()).unwrap();
    assert_eq!(config.service[0].env["DB_PASS"], "s3cret");
    assert_eq!(config.service[0].env["PLAIN"], "hello");
}

#[test]
fn load_dir_errors_when_empty() {
    let dir = tempfile::tempdir().unwrap();
    let result = ServicesConfig::load_dir(dir.path());
    assert!(result.is_err());
}
