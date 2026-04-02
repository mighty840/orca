//! Tests for persistent state: deploy persists, stop removes, restore on load.

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use tokio::sync::RwLock;
use tower::ServiceExt;

use orca_control::api::router;
use orca_control::state::AppState;
use orca_control::store::ClusterStore;
use orca_core::config::{ClusterConfig, ClusterMeta};
use orca_core::testing::MockRuntime;

fn test_state_with_store(store: Arc<ClusterStore>) -> Arc<AppState> {
    let runtime = Arc::new(MockRuntime::new());
    let state = AppState::new(
        ClusterConfig {
            cluster: ClusterMeta {
                name: "persist-test".into(),
                ..Default::default()
            },
            ..Default::default()
        },
        runtime,
        None,
        Arc::new(RwLock::new(HashMap::new())),
        Arc::new(RwLock::new(Vec::new())),
    )
    .with_store(store);
    Arc::new(state)
}

fn open_store(dir: &std::path::Path) -> Arc<ClusterStore> {
    Arc::new(ClusterStore::open(&dir.join("test.db")).unwrap())
}

async fn deploy_service(state: &Arc<AppState>, name: &str, image: &str) {
    let app = router(state.clone());
    let body = serde_json::json!({
        "services": [{"name": name, "image": image, "port": 80, "replicas": 1}]
    });
    let req = Request::post("/api/v1/deploy")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn test_deploy_persists_to_store() {
    let dir = tempfile::tempdir().unwrap();
    let store = open_store(dir.path());
    let state = test_state_with_store(store.clone());

    deploy_service(&state, "web", "nginx:latest").await;

    // Verify store has the service
    let stored = store.get_service("web").unwrap();
    assert!(stored.is_some(), "service should be persisted");
    assert_eq!(stored.unwrap().image.unwrap(), "nginx:latest");
}

#[tokio::test]
async fn test_stop_keeps_in_store() {
    let dir = tempfile::tempdir().unwrap();
    let store = open_store(dir.path());
    let state = test_state_with_store(store.clone());

    deploy_service(&state, "web", "nginx:latest").await;
    assert!(store.get_service("web").unwrap().is_some());

    // Stop via DELETE — should stop containers but keep config in store
    let app = router(state.clone());
    let req = Request::delete("/api/v1/services/web")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Config should still be in store (stop != delete)
    let stored = store.get_service("web").unwrap();
    assert!(stored.is_some(), "stop should not remove from store");
}

#[tokio::test]
async fn test_redeploy_updates_store() {
    let dir = tempfile::tempdir().unwrap();
    let store = open_store(dir.path());
    let state = test_state_with_store(store.clone());

    deploy_service(&state, "web", "nginx:1.0").await;
    assert_eq!(
        store.get_service("web").unwrap().unwrap().image.unwrap(),
        "nginx:1.0"
    );

    // Redeploy with new image
    deploy_service(&state, "web", "nginx:2.0").await;
    assert_eq!(
        store.get_service("web").unwrap().unwrap().image.unwrap(),
        "nginx:2.0"
    );
}

#[tokio::test]
async fn test_store_survives_new_appstate() {
    let dir = tempfile::tempdir().unwrap();
    let store_path = dir.path().join("test.db");

    // Deploy with first AppState
    {
        let store = Arc::new(ClusterStore::open(&store_path).unwrap());
        let state = test_state_with_store(store);
        deploy_service(&state, "persistent-svc", "nginx:latest").await;
    }

    // Open new store from same path — data should be there
    let store2 = ClusterStore::open(&store_path).unwrap();
    let services = store2.get_all_services().unwrap();
    assert!(services.contains_key("persistent-svc"));
    assert_eq!(
        services["persistent-svc"].image.as_deref(),
        Some("nginx:latest")
    );
}

#[tokio::test]
async fn test_multiple_services_persist() {
    let dir = tempfile::tempdir().unwrap();
    let store = open_store(dir.path());
    let state = test_state_with_store(store.clone());

    deploy_service(&state, "web", "nginx:latest").await;
    deploy_service(&state, "api", "httpd:latest").await;
    deploy_service(&state, "db", "postgres:16").await;

    let all = store.get_all_services().unwrap();
    assert_eq!(all.len(), 3);
    assert!(all.contains_key("web"));
    assert!(all.contains_key("api"));
    assert!(all.contains_key("db"));
}
