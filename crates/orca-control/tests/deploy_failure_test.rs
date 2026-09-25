//! #174: a failed deploy must fail loudly, keep the old instance, and not be
//! retried every declarative pass.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use orca_control::declarative::apply_config_dir;
use orca_control::reconciler;
use orca_control::state::AppState;
use orca_control::store::ClusterStore;
use orca_core::config::{ClusterConfig, ServiceConfig};
use orca_core::testing::{MockOpKind, MockRuntime};

fn state(runtime: Arc<MockRuntime>, db: Option<&std::path::Path>) -> Arc<AppState> {
    let s = AppState::new(
        ClusterConfig::default(),
        runtime,
        None,
        Arc::new(RwLock::new(HashMap::new())),
        Arc::new(RwLock::new(Vec::new())),
    );
    Arc::new(match db {
        Some(p) => s.with_store(Arc::new(ClusterStore::open(p).unwrap())),
        None => s,
    })
}

fn web(env: &str) -> ServiceConfig {
    serde_json::from_value(serde_json::json!({
        "name": "web", "image": "nginx:latest", "port": 8080,
        "env": { "VERSION": env },
    }))
    .unwrap()
}

async fn instance_ids(state: &AppState) -> Vec<String> {
    state.services.read().await["web"]
        .instances
        .iter()
        .map(|i| i.handle.runtime_id.clone())
        .collect()
}

#[tokio::test]
async fn a_failed_redeploy_keeps_the_old_instance_and_config() {
    let runtime = Arc::new(MockRuntime::with_host_port(9000));
    let state = state(runtime.clone(), None);
    reconciler::reconcile(&state, &[web("1")]).await;
    let before = instance_ids(&state).await;
    assert_eq!(before.len(), 1);

    runtime.fail_next(MockOpKind::Start).await;
    let result = reconciler::redeploy(&state, "web").await;

    assert!(result.is_err(), "a failed redeploy must report failure");
    assert_eq!(
        instance_ids(&state).await,
        before,
        "the old instance is kept"
    );
    assert_eq!(
        state.services.read().await["web"].config.env["VERSION"],
        "1"
    );
}

#[tokio::test]
async fn a_failed_rolling_update_does_not_stop_the_old_instance() {
    let runtime = Arc::new(MockRuntime::with_host_port(9000));
    let state = state(runtime.clone(), None);
    reconciler::reconcile(&state, &[web("1")]).await;
    runtime.clear_ops().await;

    runtime.fail_next(MockOpKind::Start).await;
    let (deployed, errors) = reconciler::reconcile(&state, &[web("2")]).await;

    assert!(deployed.is_empty() && errors.len() == 1, "{errors:?}");
    assert_eq!(
        runtime.count(MockOpKind::Stop).await,
        0,
        "the old instance must not be stopped after a failed update"
    );
}

#[tokio::test]
async fn a_failed_spec_is_not_retried_every_pass_but_a_fix_is_applied() {
    let tmp = tempfile::tempdir().unwrap();
    let runtime = Arc::new(MockRuntime::with_host_port(9000));
    let state = state(runtime.clone(), Some(&tmp.path().join("c.db")));
    let dir = tmp.path().join("services");
    let write = |version: &str| {
        std::fs::create_dir_all(dir.join("web")).unwrap();
        std::fs::write(
            dir.join("web/service.toml"),
            format!(
                "[[service]]\nname = \"web\"\nimage = \"nginx:latest\"\nport = 8080\n\
                 [service.env]\nVERSION = \"{version}\"\n"
            ),
        )
        .unwrap();
    };
    write("1");
    apply_config_dir(&state, dir.to_str().unwrap()).await;

    // v2 fails to start.
    write("2");
    runtime.fail_next(MockOpKind::Start).await;
    apply_config_dir(&state, dir.to_str().unwrap()).await;
    let attempts = runtime.count(MockOpKind::Create).await;

    // The next pass must not try the same spec again right away.
    apply_config_dir(&state, dir.to_str().unwrap()).await;
    assert_eq!(
        runtime.count(MockOpKind::Create).await,
        attempts,
        "cooldown"
    );

    // The failed spec was never recorded as applied.
    let stored = state
        .store
        .as_ref()
        .unwrap()
        .get_service("web")
        .unwrap()
        .unwrap();
    assert_eq!(
        stored.env["VERSION"], "1",
        "v2 failed, v1 is still what runs"
    );

    // A changed spec (the fix) goes through immediately.
    write("3");
    apply_config_dir(&state, dir.to_str().unwrap()).await;
    assert!(
        runtime.count(MockOpKind::Create).await > attempts,
        "fix applied"
    );
}
