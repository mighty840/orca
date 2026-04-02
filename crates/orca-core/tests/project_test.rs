//! Tests for project isolation: directory-based service grouping.

use std::collections::HashMap;

use orca_core::config::ServicesConfig;

/// load_dir should set the `project` field from the directory name.
#[test]
fn load_dir_sets_project_from_directory_name() {
    let dir = tempfile::tempdir().unwrap();
    let proj_dir = dir.path().join("myapp");
    std::fs::create_dir_all(&proj_dir).unwrap();
    std::fs::write(
        proj_dir.join("service.toml"),
        r#"
[[service]]
name = "myapp-web"
image = "nginx:latest"
port = 80

[[service]]
name = "myapp-db"
image = "postgres:16"
port = 5432
"#,
    )
    .unwrap();

    let config = ServicesConfig::load_dir(dir.path()).unwrap();
    assert_eq!(config.service.len(), 2);
    for svc in &config.service {
        assert_eq!(
            svc.project.as_deref(),
            Some("myapp"),
            "project should be set from directory name"
        );
    }
}

/// Services in different directories get different project names.
#[test]
fn different_directories_get_different_projects() {
    let dir = tempfile::tempdir().unwrap();
    for name in &["alpha", "beta"] {
        let proj = dir.path().join(name);
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(
            proj.join("service.toml"),
            format!(
                r#"
[[service]]
name = "{name}-svc"
image = "nginx:latest"
port = 80
"#
            ),
        )
        .unwrap();
    }

    let config = ServicesConfig::load_dir(dir.path()).unwrap();
    assert_eq!(config.service.len(), 2);
    assert_eq!(config.service[0].project.as_deref(), Some("alpha"));
    assert_eq!(config.service[1].project.as_deref(), Some("beta"));
}

/// When loading a single file (not dir), project should be None.
#[test]
fn load_single_file_has_no_project() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("service.toml");
    std::fs::write(
        &file,
        r#"
[[service]]
name = "standalone"
image = "nginx:latest"
port = 80
"#,
    )
    .unwrap();

    let config = ServicesConfig::load(&file).unwrap();
    assert!(config.service[0].project.is_none());
}

/// Services without explicit network should default to orca-{project}.
#[test]
fn project_sets_default_network() {
    let dir = tempfile::tempdir().unwrap();
    let proj = dir.path().join("infisical");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(
        proj.join("service.toml"),
        r#"
[[service]]
name = "infisical-db"
image = "postgres:16"
port = 5432

[[service]]
name = "infisical-redis"
image = "redis:7"
port = 6379
"#,
    )
    .unwrap();

    let config = ServicesConfig::load_dir(dir.path()).unwrap();
    // Both services should get network = "infisical" (from project)
    for svc in &config.service {
        assert_eq!(
            svc.network.as_deref(),
            Some("infisical"),
            "network should default to project name"
        );
    }
}

/// Explicit network should NOT be overridden by project.
#[test]
fn explicit_network_not_overridden_by_project() {
    let dir = tempfile::tempdir().unwrap();
    let proj = dir.path().join("myapp");
    std::fs::create_dir_all(&proj).unwrap();
    std::fs::write(
        proj.join("service.toml"),
        r#"
[[service]]
name = "myapp-svc"
image = "nginx:latest"
port = 80
network = "custom-net"
"#,
    )
    .unwrap();

    let config = ServicesConfig::load_dir(dir.path()).unwrap();
    assert_eq!(config.service[0].network.as_deref(), Some("custom-net"));
}

/// Services in different projects cannot share a network by default.
#[test]
fn different_projects_get_isolated_networks() {
    let dir = tempfile::tempdir().unwrap();
    for name in &["proj-a", "proj-b"] {
        let proj = dir.path().join(name);
        std::fs::create_dir_all(&proj).unwrap();
        std::fs::write(
            proj.join("service.toml"),
            format!(
                r#"
[[service]]
name = "{name}-svc"
image = "nginx:latest"
port = 80
"#
            ),
        )
        .unwrap();
    }

    let config = ServicesConfig::load_dir(dir.path()).unwrap();
    let net_a = config.service[0].network.as_deref().unwrap();
    let net_b = config.service[1].network.as_deref().unwrap();
    assert_ne!(
        net_a, net_b,
        "different projects must have different networks"
    );
}
