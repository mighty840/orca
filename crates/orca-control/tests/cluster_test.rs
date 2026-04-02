//! Integration tests for cluster drain/undrain functionality.

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tokio::sync::RwLock;
use tower::ServiceExt;

use orca_control::api::router;
use orca_control::state::{AppState, RegisteredNode};
use orca_core::config::ClusterConfig;
use orca_core::testing::MockRuntime;

fn test_app() -> (axum::Router, Arc<AppState>) {
    let runtime = Arc::new(MockRuntime::new());
    let state = Arc::new(AppState::new(
        ClusterConfig::default(),
        runtime,
        None,
        Arc::new(RwLock::new(HashMap::new())),
        Arc::new(RwLock::new(Vec::new())),
    ));
    let app = router(state.clone());
    (app, state)
}

async fn register_node(state: &AppState, node_id: u64) {
    let node = RegisteredNode {
        node_id,
        address: format!("10.0.0.{node_id}:6880"),
        labels: HashMap::new(),
        last_heartbeat: chrono::Utc::now(),
        drain: false,
    };
    let mut nodes = state.registered_nodes.write().await;
    nodes.insert(node_id, node);
}

#[tokio::test]
async fn test_drain_node() {
    let (app, state) = test_app();
    register_node(&state, 42).await;

    let req = Request::post("/api/v1/cluster/nodes/42/drain")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let nodes = state.registered_nodes.read().await;
    assert!(nodes[&42].drain, "node should be drained");
}

#[tokio::test]
async fn test_undrain_node() {
    let (app, state) = test_app();
    register_node(&state, 42).await;

    // Drain first
    {
        let mut nodes = state.registered_nodes.write().await;
        nodes.get_mut(&42).unwrap().drain = true;
    }

    let req = Request::post("/api/v1/cluster/nodes/42/undrain")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let nodes = state.registered_nodes.read().await;
    assert!(!nodes[&42].drain, "node should be undrained");
}

#[tokio::test]
async fn test_drain_nonexistent_returns_404() {
    let (app, _state) = test_app();

    let req = Request::post("/api/v1/cluster/nodes/999/drain")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json["error"].as_str().unwrap().contains("not found"));
}

#[tokio::test]
async fn test_drained_node_skipped_by_scheduler() {
    let (_app, state) = test_app();

    // Register a node and drain it
    register_node(&state, 10).await;
    {
        let mut nodes = state.registered_nodes.write().await;
        nodes.get_mut(&10).unwrap().drain = true;
    }

    // Deploy a service with placement targeting the drained node
    let deploy_body = serde_json::json!({
        "services": [{
            "name": "drain-test-svc",
            "image": "nginx:latest",
            "replicas": 1,
            "port": 80,
            "placement": { "node": "10" }
        }]
    });

    let services: Vec<orca_core::config::ServiceConfig> =
        serde_json::from_value(deploy_body["services"].clone()).unwrap();
    let (deployed, _errors) = orca_control::reconciler::reconcile(&state, &services).await;

    // The service should be "deployed" (reconcile returns it) but
    // since the target node is drained, nothing should be queued
    // for remote dispatch.
    let pending = state.pending_commands.read().await;
    let cmds = pending.get(&10).cloned().unwrap_or_default();
    assert!(
        cmds.is_empty(),
        "drained node should not receive deploy commands, got {cmds:?}"
    );

    // The service name should still appear as deployed because
    // reconcile_service falls through to local deploy when no
    // remote target is found.
    assert!(
        deployed.contains(&"drain-test-svc".to_string()),
        "service should still be deployed locally"
    );
}
