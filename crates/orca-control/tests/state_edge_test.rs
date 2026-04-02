//! State corruption edge-case tests using MockRuntime (no Docker needed).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;

use orca_control::api::router;
use orca_control::state::AppState;
use orca_core::config::{ClusterConfig, ClusterMeta};
use orca_core::testing::MockRuntime;

fn test_app_state() -> Arc<AppState> {
    let runtime = Arc::new(MockRuntime::with_host_port(9000));
    Arc::new(AppState::new(
        ClusterConfig {
            cluster: ClusterMeta {
                name: "state-edge-test".to_string(),
                api_port: 0,
                grpc_port: 0,
                ..Default::default()
            },
            ..Default::default()
        },
        runtime,
        None,
        Arc::new(RwLock::new(HashMap::new())),
        Arc::new(RwLock::new(Vec::new())),
    ))
}

/// Deploy the same service from 2 concurrent requests. Verify consistent state.
#[tokio::test]
async fn test_concurrent_deploys_same_service() {
    let state = test_app_state();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = router(state.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let client = reqwest::Client::new();
    let mut handles = Vec::new();
    for _ in 0..2 {
        let c = client.clone();
        let url = format!("http://127.0.0.1:{port}/api/v1/deploy");
        handles.push(tokio::spawn(async move {
            let body = serde_json::json!({
                "services": [{
                    "name": "same-svc",
                    "image": "nginx:latest",
                    "replicas": 1,
                    "port": 8080
                }]
            });
            c.post(&url).json(&body).send().await.unwrap()
        }));
    }

    for h in handles {
        let resp = h.await.expect("task should not panic");
        assert!(
            resp.status().is_success(),
            "deploy should not fail: {}",
            resp.status()
        );
    }

    // Verify exactly 1 instance, not duplicated.
    let services = state.services.read().await;
    assert!(services.contains_key("same-svc"));
    assert_eq!(
        services["same-svc"].instances.len(),
        1,
        "concurrent deploys of same service should result in 1 instance"
    );
}

/// Stop a service that does not exist. Should error gracefully, not panic.
#[tokio::test]
async fn test_stop_nonexistent_service() {
    let state = test_app_state();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = router(state.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let client = reqwest::Client::new();
    let resp = client
        .delete(format!(
            "http://127.0.0.1:{port}/api/v1/services/ghost-service"
        ))
        .send()
        .await
        .unwrap();
    assert!(
        resp.status().is_server_error() || resp.status().is_client_error(),
        "stopping nonexistent service should return error, got {}",
        resp.status()
    );
}

/// Deploy then immediately stop before containers finish starting.
#[tokio::test]
async fn test_deploy_then_immediately_stop() {
    let state = test_app_state();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = router(state.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{port}");

    // Deploy
    let deploy = serde_json::json!({
        "services": [{"name": "fast-stop", "image": "nginx:latest", "replicas": 1, "port": 80}]
    });
    let resp = client
        .post(format!("{base}/api/v1/deploy"))
        .json(&deploy)
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());

    // Immediately stop (no sleep)
    let resp = client
        .delete(format!("{base}/api/v1/services/fast-stop"))
        .send()
        .await
        .unwrap();
    assert!(
        resp.status().is_success(),
        "immediate stop should succeed, got {}",
        resp.status()
    );

    // Verify service is gone from state (no orphans).
    let services = state.services.read().await;
    assert!(
        !services.contains_key("fast-stop"),
        "service should be removed after stop"
    );
}

/// Deploy with 3 replicas, redeploy with 1. Verify exactly 1 remains.
#[tokio::test]
async fn test_redeploy_with_fewer_replicas() {
    let state = test_app_state();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = router(state.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{port}");

    // Deploy with 3 replicas
    let deploy3 = serde_json::json!({
        "services": [{"name": "shrink", "image": "nginx:latest", "replicas": 3, "port": 80}]
    });
    let resp = client
        .post(format!("{base}/api/v1/deploy"))
        .json(&deploy3)
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());

    {
        let services = state.services.read().await;
        assert_eq!(services["shrink"].instances.len(), 3);
    }

    // Redeploy with 1 replica
    let deploy1 = serde_json::json!({
        "services": [{"name": "shrink", "image": "nginx:latest", "replicas": 1, "port": 80}]
    });
    let resp = client
        .post(format!("{base}/api/v1/deploy"))
        .json(&deploy1)
        .send()
        .await
        .unwrap();
    assert!(resp.status().is_success());

    let services = state.services.read().await;
    assert_eq!(
        services["shrink"].instances.len(),
        1,
        "redeploy with fewer replicas should scale down"
    );
}
