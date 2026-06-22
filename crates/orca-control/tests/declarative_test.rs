//! Tests for the declarative reconciler: the master applies a directory of
//! service configs without a manual `orca deploy`.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use orca_control::declarative::apply_config_dir;
use orca_control::state::AppState;
use orca_core::config::ClusterConfig;
use orca_core::testing::MockRuntime;

fn state() -> Arc<AppState> {
    Arc::new(AppState::new(
        ClusterConfig::default(),
        Arc::new(MockRuntime::with_host_port(9000)),
        None,
        Arc::new(RwLock::new(HashMap::new())),
        Arc::new(RwLock::new(Vec::new())),
    ))
}

/// Write `<dir>/<project>/service.toml` with the given body.
fn write_service(dir: &std::path::Path, project: &str, body: &str) {
    let sub = dir.join(project);
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(sub.join("service.toml"), body).unwrap();
}

#[tokio::test]
async fn applies_new_services_from_dir_and_is_idempotent() {
    let tmp = tempfile::tempdir().unwrap();
    write_service(
        tmp.path(),
        "web",
        r#"
        [[service]]
        name = "web"
        image = "nginx:latest"
        port = 8080
        replicas = 1
        "#,
    );

    let state = state();
    let dir = tmp.path().to_str().unwrap();

    // First pass deploys the new service.
    apply_config_dir(&state, dir).await;
    {
        let services = state.services.read().await;
        let svc = services.get("web").expect("web should be deployed");
        assert_eq!(svc.config.image.as_deref(), Some("nginx:latest"));
        assert_eq!(svc.instances.len(), 1, "one instance created");
    }

    // Second pass is a no-op (spec unchanged) — no duplicate instances.
    apply_config_dir(&state, dir).await;
    {
        let services = state.services.read().await;
        assert_eq!(
            services.get("web").unwrap().instances.len(),
            1,
            "unchanged service must not be re-deployed"
        );
    }
}

#[tokio::test]
async fn invalid_config_is_skipped_and_recorded() {
    let tmp = tempfile::tempdir().unwrap();
    write_service(
        tmp.path(),
        "bad",
        r#"
        [[service]]
        name = "bad"
        image = "nginx:latest"
        domain = "a.com"
        domains = ["b.com"]
        "#,
    );

    let state = state();
    apply_config_dir(&state, tmp.path().to_str().unwrap()).await;

    // Not deployed...
    assert!(
        !state.services.read().await.contains_key("bad"),
        "invalid config must not deploy"
    );
    // ...but the reason is recorded for `orca status`.
    let failures = state.last_failures.read().await;
    let f = failures.get("bad").expect("failure should be recorded");
    assert!(
        f.message.contains("domain") && f.message.contains("domains"),
        "failure should explain the conflict: {}",
        f.message
    );
}
