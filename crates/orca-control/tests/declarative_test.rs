//! Tests for the declarative reconciler: the master applies a directory of
//! service configs without a manual `orca deploy`.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use orca_control::declarative::apply_config_dir;
use orca_control::state::AppState;
use orca_control::store::ClusterStore;
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

/// State backed by a real redb store (needed for the stopped-set + prune purge).
fn state_with_store(db_path: &std::path::Path) -> Arc<AppState> {
    let store = ClusterStore::open(db_path).unwrap();
    Arc::new(
        AppState::new(
            ClusterConfig::default(),
            Arc::new(MockRuntime::with_host_port(9000)),
            None,
            Arc::new(RwLock::new(HashMap::new())),
            Arc::new(RwLock::new(Vec::new())),
        )
        .with_store(Arc::new(store)),
    )
}

const WEB: &str = r#"
    [[service]]
    name = "web"
    image = "nginx:latest"
    port = 8080
    replicas = 1
"#;

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

#[tokio::test]
async fn prunes_service_removed_from_config() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    write_service(tmp.path(), "web", WEB);
    write_service(
        tmp.path(),
        "api",
        r#"
        [[service]]
        name = "api"
        image = "nginx:latest"
        port = 9090
        replicas = 1
        "#,
    );
    let state = state_with_store(&db.path().join("c.db"));
    let dir = tmp.path().to_str().unwrap();

    apply_config_dir(&state, dir).await;
    {
        let s = state.services.read().await;
        assert!(
            s.contains_key("web") && s.contains_key("api"),
            "both deployed"
        );
    }

    // Drop "api" from the config dir → next reconcile must prune it.
    std::fs::remove_dir_all(tmp.path().join("api")).unwrap();
    apply_config_dir(&state, dir).await;

    let s = state.services.read().await;
    assert!(s.contains_key("web"), "still-declared service kept");
    assert!(
        !s.contains_key("api"),
        "service removed from service.toml must be pruned"
    );
}

#[tokio::test]
async fn empty_config_dir_does_not_prune() {
    // Guard: a transient/empty load must never mass-delete declared services.
    let tmp = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    write_service(tmp.path(), "web", WEB);
    let state = state_with_store(&db.path().join("c.db"));
    let dir = tmp.path().to_str().unwrap();
    apply_config_dir(&state, dir).await;
    assert!(state.services.read().await.contains_key("web"));

    // Remove ALL service.toml files, then reconcile — must NOT prune "web".
    std::fs::remove_dir_all(tmp.path().join("web")).unwrap();
    apply_config_dir(&state, dir).await;
    assert!(
        state.services.read().await.contains_key("web"),
        "empty/garbled config must not trigger a prune"
    );
}

#[tokio::test]
async fn paused_service_is_not_pruned_or_restarted() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    write_service(tmp.path(), "web", WEB);
    let state = state_with_store(&db.path().join("c.db"));
    let dir = tmp.path().to_str().unwrap();

    apply_config_dir(&state, dir).await;
    orca_control::reconciler::stop(&state, "web").await.unwrap();
    assert!(state.services.read().await.get("web").unwrap().stopped);

    // Reconcile again with "web" still declared — it must stay paused, not be
    // restarted, and not be pruned.
    apply_config_dir(&state, dir).await;
    let s = state.services.read().await;
    let web = s
        .get("web")
        .expect("paused + still-declared service must be kept");
    assert!(
        web.stopped,
        "paused service must stay paused (not restarted)"
    );
    assert_eq!(web.running_count(), 0, "paused service must not be running");
}

#[tokio::test]
async fn stop_pauses_then_start_resumes() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    write_service(tmp.path(), "web", WEB);
    let state = state_with_store(&db.path().join("c.db"));
    apply_config_dir(&state, tmp.path().to_str().unwrap()).await;
    assert_eq!(
        state
            .services
            .read()
            .await
            .get("web")
            .unwrap()
            .running_count(),
        1
    );

    // stop = pause: kept in registry, marked stopped + persisted, 0 running.
    orca_control::reconciler::stop(&state, "web").await.unwrap();
    {
        let s = state.services.read().await;
        let w = s.get("web").expect("paused service stays in the registry");
        assert!(w.stopped);
        assert_eq!(w.running_count(), 0);
    }
    assert!(
        state
            .store
            .as_ref()
            .unwrap()
            .get_stopped()
            .unwrap()
            .contains("web"),
        "stopped mark persisted"
    );

    // start = resume: cleared + back to configured replicas.
    orca_control::reconciler::start(&state, "web")
        .await
        .unwrap();
    {
        let s = state.services.read().await;
        let w = s.get("web").unwrap();
        assert!(!w.stopped);
        assert_eq!(w.running_count(), 1, "resumed to configured replicas");
    }
    assert!(
        !state
            .store
            .as_ref()
            .unwrap()
            .get_stopped()
            .unwrap()
            .contains("web"),
        "stopped mark cleared on start"
    );
}
