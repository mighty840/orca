//! Tests for depends_on service startup ordering.

use std::collections::HashMap;

use orca_control::topo_sort::topo_sort;
use orca_core::config::ServiceConfig;
use orca_core::types::Replicas;

fn make_service(name: &str, depends_on: Vec<&str>) -> ServiceConfig {
    ServiceConfig {
        restart_policy: None,
        name: name.to_string(),
        project: None,
        runtime: Default::default(),
        image: Some("nginx:latest".to_string()),
        module: None,
        replicas: Replicas::Fixed(1),
        port: Some(80),
        host_port: None,
        domain: None,
        domains: vec![],
        routes: vec![],
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
        triggers: vec![],
        assets: None,
        build: None,
        tls_cert: None,
        tls_key: None,
        internal: false,
        cmd: vec![],
        extra_ports: vec![],
        strip_prefix: None,
        pull_policy: Default::default(),
        backup: None,
        depends_on: depends_on.into_iter().map(String::from).collect(),
    }
}

/// Services with deps are reconciled after their dependencies.
#[test]
fn test_depends_on_ordering() {
    let services = vec![
        make_service("app", vec!["postgres", "redis"]),
        make_service("postgres", vec![]),
        make_service("redis", vec![]),
    ];

    let ordered = topo_sort(&services);
    let names: Vec<&str> = ordered.iter().map(|s| s.name.as_str()).collect();

    // postgres and redis must come before app
    let pg_pos = names.iter().position(|n| *n == "postgres").unwrap();
    let redis_pos = names.iter().position(|n| *n == "redis").unwrap();
    let app_pos = names.iter().position(|n| *n == "app").unwrap();
    assert!(pg_pos < app_pos, "postgres must come before app");
    assert!(redis_pos < app_pos, "redis must come before app");
}

/// Circular deps don't hang, services still deploy.
#[test]
fn test_circular_dependency_handled() {
    let services = vec![
        make_service("a", vec!["b"]),
        make_service("b", vec!["a"]),
        make_service("c", vec![]),
    ];

    let ordered = topo_sort(&services);
    assert_eq!(ordered.len(), 3, "all services must be present");

    // c has no deps, should be first
    let c_pos = ordered.iter().position(|s| s.name == "c").unwrap();
    assert_eq!(c_pos, 0, "c should be sorted first (no deps)");
}

/// Missing dependency doesn't cause a hang.
#[test]
fn test_missing_dependency_handled() {
    let services = vec![
        make_service("app", vec!["nonexistent"]),
        make_service("db", vec![]),
    ];

    let ordered = topo_sort(&services);
    assert_eq!(ordered.len(), 2, "all services must be present");
}

/// A recreated dependency puts its direct dependents in the restart set.
#[test]
fn test_restart_set_direct_dependent() {
    let services = vec![
        make_service("postgres", vec![]),
        make_service("valkey", vec![]),
        make_service("app", vec!["postgres", "valkey"]),
    ];

    let restarts =
        orca_control::dependents::restart_set(&services, &["valkey".to_string()]);
    assert_eq!(restarts, vec!["app".to_string()]);
}

/// The cascade is transitive: restarting a dependent counts as a change for
/// its own dependents, in dependency order.
#[test]
fn test_restart_set_transitive() {
    let services = vec![
        make_service("db", vec![]),
        make_service("api", vec!["db"]),
        make_service("frontend", vec!["api"]),
    ];

    let restarts = orca_control::dependents::restart_set(&services, &["db".to_string()]);
    assert_eq!(
        restarts,
        vec!["api".to_string(), "frontend".to_string()],
        "api must restart before frontend"
    );
}

/// Services already recreated in the deploy are never restarted again, and
/// unrelated services are untouched.
#[test]
fn test_restart_set_skips_changed_and_unrelated() {
    let services = vec![
        make_service("db", vec![]),
        make_service("app", vec!["db"]),
        make_service("other", vec![]),
    ];

    // Both db and app were just recreated by the deploy itself — nothing to do.
    let restarts = orca_control::dependents::restart_set(
        &services,
        &["db".to_string(), "app".to_string()],
    );
    assert!(restarts.is_empty());

    // Nothing changed at all.
    let restarts = orca_control::dependents::restart_set(&services, &[]);
    assert!(restarts.is_empty());
}
