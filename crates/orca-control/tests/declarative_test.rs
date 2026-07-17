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

/// Reproduces the rc.4 churn: an erp-frontend-shaped service (mounts, cmd,
/// aliases, network, env, domain) must be left ALONE on the second reconcile
/// pass — i.e. its container is not replaced. We detect a needless redeploy by
/// the instance's runtime_id changing between passes.
#[tokio::test]
async fn erp_like_service_is_idempotent_across_reconciles() {
    let tmp = tempfile::tempdir().unwrap();
    write_service(
        tmp.path(),
        "erp",
        r#"
        [[service]]
        name = "erp-frontend"
        image = "ghcr.io/x/erpnext:v15"
        port = 8080
        domain = "erp.example.com"
        network = "erp-net"
        aliases = ["erp-frontend"]
        cmd = ["nginx-entrypoint.sh"]
        mounts = ["erp-data:/home/frappe/sites/erp.example.com"]
        replicas = 1
        [service.env]
        BACKEND = "erp-backend:8000"
        SOCKETIO = "erp-websocket:9000"
        "#,
    );

    let state = state();
    let dir = tmp.path().to_str().unwrap();

    apply_config_dir(&state, dir).await;
    let first_id = {
        let services = state.services.read().await;
        let svc = services.get("erp-frontend").expect("deployed");
        assert_eq!(svc.instances.len(), 1);
        svc.instances[0].handle.runtime_id.clone()
    };

    // Second pass with the IDENTICAL config must not redeploy → same container.
    apply_config_dir(&state, dir).await;
    {
        let services = state.services.read().await;
        let svc = services.get("erp-frontend").unwrap();
        assert_eq!(svc.instances.len(), 1, "must not duplicate");
        assert_eq!(
            svc.instances[0].handle.runtime_id, first_id,
            "identical config must NOT replace the container (churn bug)"
        );
    }
}

/// Regression for the rc.4 remote container-churn: a placement-pinned service
/// whose agent never acks the deploy must STILL have its declared config
/// recorded, so the next reconcile pass sees no spec change and does NOT
/// re-dispatch the deploy. Before the fix, `svc.config` was set only after a
/// successful `queue_remote_deploy`, so a missed ack left the service
/// unrecorded and every cycle re-deployed it — churning the container.
#[tokio::test]
async fn remote_deploy_without_ack_is_not_redeployed_every_cycle() {
    use orca_control::state::RegisteredNode;
    use orca_core::ws_types::MasterMessage;

    let tmp = tempfile::tempdir().unwrap();
    let db = tempfile::tempdir().unwrap();
    write_service(
        tmp.path(),
        "remote",
        r#"
        [[service]]
        name = "remote-web"
        image = "nginx:latest"
        port = 8080
        replicas = 1
        [service.placement]
        node = "node-7"
        "#,
    );

    // Fast ack timeout so the no-ack deploy errors quickly; real store for the
    // stopped-set the reconciler reads.
    let mut cfg = ClusterConfig::default();
    cfg.deploy.ack_timeout_secs = 1;
    let store = ClusterStore::open(&db.path().join("c.db")).unwrap();
    let state = Arc::new(
        AppState::new(
            cfg,
            Arc::new(MockRuntime::with_host_port(9000)),
            None,
            Arc::new(RwLock::new(HashMap::new())),
            Arc::new(RwLock::new(Vec::new())),
        )
        .with_store(Arc::new(store)),
    );

    // Node "node-7" + an agent that is connected but never acks (rx held open).
    state.registered_nodes.write().await.insert(
        7,
        RegisteredNode {
            node_id: 7,
            address: "node-7:6881".into(),
            labels: HashMap::new(),
            last_heartbeat: chrono::Utc::now(),
            drain: false,
            cpu_percent: 0.0,
            memory_bytes: 0,
            memory_total: 0,
            disk_used: 0,
            disk_total: 0,
            net_rx: 0,
            net_tx: 0,
        },
    );
    let (tx, mut rx) = tokio::sync::mpsc::channel::<MasterMessage>(16);
    state
        .ws_agents
        .write()
        .await
        .insert(7, orca_control::state::AgentSession::new(tx));

    let dir = tmp.path().to_str().unwrap();
    // Two reconcile passes — the agent never acks, so each deploy attempt errors,
    // but the config recorded on the first pass must stop the second from
    // re-dispatching.
    apply_config_dir(&state, dir).await;
    apply_config_dir(&state, dir).await;

    let mut deploys = 0;
    while let Ok(msg) = rx.try_recv() {
        if matches!(msg, MasterMessage::Deploy { .. }) {
            deploys += 1;
        }
    }
    assert_eq!(
        deploys, 1,
        "an unchanged remote spec must be dispatched once, not re-deployed every reconcile cycle"
    );
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
