//! #315: `orca rm` removes a service from the registry, the store and the
//! proxy, after stopping it.

use std::collections::HashMap;
use std::sync::Arc;

use orca_core::config::{ClusterConfig, ServiceConfig};
use orca_core::testing::{MockOpKind, MockRuntime};
use tokio::sync::RwLock;

use super::remove;
use crate::state::AppState;

fn config(name: &str) -> ServiceConfig {
    serde_json::from_value(serde_json::json!({
        "name": name,
        "image": "nginx:latest",
        "port": 8080,
        "volume": { "path": "/var/lib/data" },
        "mounts": ["/srv/blog:/data:ro"],
    }))
    .unwrap()
}

fn master(runtime: Arc<MockRuntime>, cluster: ClusterConfig) -> (AppState, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(crate::store::ClusterStore::open(&dir.path().join("s.redb")).unwrap());
    let state = AppState::new(
        cluster,
        runtime,
        None,
        Arc::new(RwLock::new(HashMap::new())),
        Arc::new(RwLock::new(Vec::new())),
    )
    .with_store(store);
    (state, dir)
}

async fn deploy(state: &AppState, name: &str) {
    crate::reconciler::reconcile_service(state, &config(name))
        .await
        .unwrap();
    let store = state.store.as_ref().unwrap();
    store.set_service(name, &config(name)).unwrap();
}

#[tokio::test]
async fn rm_stops_a_paused_or_running_service_and_forgets_it() {
    let runtime = Arc::new(MockRuntime::new());
    let (state, _dir) = master(runtime.clone(), ClusterConfig::default());
    deploy(&state, "dxblog-app").await;
    crate::operations::stop(&state, "dxblog-app").await.unwrap();
    deploy(&state, "web").await;

    for name in ["dxblog-app", "web"] {
        let kept = remove(&state, name, false).await.unwrap();
        assert_eq!(
            kept,
            [
                format!("volume orca-{name}-data"),
                "bind mount /srv/blog".to_string()
            ]
        );
    }

    assert!(state.services.read().await.is_empty());
    let store = state.store.as_ref().unwrap();
    assert!(store.get_all_services().unwrap().is_empty());
    assert!(store.get_stopped().unwrap().is_empty());
    assert_eq!(runtime.count(MockOpKind::Remove).await, 2);
}

#[tokio::test]
async fn rm_refuses_a_service_the_declarative_loop_would_redeploy() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("services.toml");
    std::fs::write(
        &file,
        "[[service]]\nname = \"web\"\nimage = \"nginx:latest\"\n",
    )
    .unwrap();
    let cluster = ClusterConfig {
        reconcile: Some(
            serde_json::from_value(serde_json::json!({ "config_dir": file.to_str().unwrap() }))
                .unwrap(),
        ),
        ..Default::default()
    };
    let (state, _store_dir) = master(Arc::new(MockRuntime::new()), cluster);
    deploy(&state, "web").await;

    let err = remove(&state, "web", false).await.unwrap_err().to_string();
    assert!(err.contains("still declared"), "{err}");
    assert!(state.services.read().await.contains_key("web"));

    remove(&state, "web", true).await.unwrap();
    assert!(state.services.read().await.is_empty());
}

#[tokio::test]
async fn rm_of_an_unknown_service_is_an_error() {
    let (state, _dir) = master(Arc::new(MockRuntime::new()), ClusterConfig::default());
    let err = remove(&state, "nope", false).await.unwrap_err().to_string();
    assert!(err.contains("not found"), "{err}");
}
