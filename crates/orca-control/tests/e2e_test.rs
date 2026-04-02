//! E2E tests: spin up a real orca server with Docker, deploy services, verify.
//!
//! Requires Docker running locally. Run with: `cargo test -- --ignored e2e`

mod e2e_helpers;

use e2e_helpers::{TestClient, cleanup_containers, start_server};
use std::time::Duration;

#[tokio::test]
#[ignore] // requires Docker
async fn e2e_deploy_nginx_and_verify() {
    let (port, state, _handle) = start_server().await;
    let client = TestClient::new(port);

    // Deploy a simple nginx service
    let deploy = serde_json::json!({
        "services": [{
            "name": "e2e-nginx",
            "image": "nginx:alpine",
            "replicas": 1,
            "port": 80
        }]
    });

    let resp = client.post_json("/api/v1/deploy", &deploy).await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert!(
        body["deployed"]
            .as_array()
            .unwrap()
            .contains(&"e2e-nginx".into())
    );

    // Wait for container to start
    tokio::time::sleep(Duration::from_secs(3)).await;

    // Verify status shows running
    let resp = client.get("/api/v1/status").await;
    assert_eq!(resp.status(), 200);
    let body: serde_json::Value = resp.json().await.unwrap();
    let services = body["services"].as_array().unwrap();
    let nginx = services.iter().find(|s| s["name"] == "e2e-nginx").unwrap();
    assert_eq!(nginx["running_replicas"], 1);
    assert_eq!(nginx["status"], "running");

    // Verify container actually exists in Docker
    let docker = bollard::Docker::connect_with_local_defaults().unwrap();
    let info = docker
        .inspect_container("orca-e2e-nginx", None)
        .await
        .unwrap();
    assert!(info.state.unwrap().running.unwrap());

    // Clean up
    drop(state);
    cleanup_containers("orca-e2e-").await;
}

#[tokio::test]
#[ignore] // requires Docker
async fn e2e_deploy_duplicate_is_idempotent() {
    let (port, state, _handle) = start_server().await;
    let client = TestClient::new(port);

    let deploy = serde_json::json!({
        "services": [{
            "name": "e2e-idem",
            "image": "nginx:alpine",
            "replicas": 1,
            "port": 80
        }]
    });

    // Deploy twice
    client.post_json("/api/v1/deploy", &deploy).await;
    tokio::time::sleep(Duration::from_secs(2)).await;
    client.post_json("/api/v1/deploy", &deploy).await;
    tokio::time::sleep(Duration::from_secs(1)).await;

    // Should still have exactly 1 instance
    let services = state.services.read().await;
    let svc = services.get("e2e-idem").unwrap();
    assert_eq!(svc.instances.len(), 1);
    drop(services);

    drop(state);
    cleanup_containers("orca-e2e-").await;
}

#[tokio::test]
#[ignore] // requires Docker
async fn e2e_scale_up_and_down() {
    let (port, state, _handle) = start_server().await;
    let client = TestClient::new(port);

    // Deploy with 1 replica
    let deploy = serde_json::json!({
        "services": [{
            "name": "e2e-scale",
            "image": "nginx:alpine",
            "replicas": 1,
            "port": 80
        }]
    });
    client.post_json("/api/v1/deploy", &deploy).await;
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Scale to 3
    let scale = serde_json::json!({ "replicas": 3 });
    let resp = client
        .post_json("/api/v1/services/e2e-scale/scale", &scale)
        .await;
    assert_eq!(resp.status(), 200);
    tokio::time::sleep(Duration::from_secs(3)).await;

    let services = state.services.read().await;
    assert_eq!(services["e2e-scale"].instances.len(), 3);
    drop(services);

    // Scale back to 1
    let scale = serde_json::json!({ "replicas": 1 });
    client
        .post_json("/api/v1/services/e2e-scale/scale", &scale)
        .await;
    tokio::time::sleep(Duration::from_secs(2)).await;

    let services = state.services.read().await;
    assert_eq!(services["e2e-scale"].instances.len(), 1);
    drop(services);

    drop(state);
    cleanup_containers("orca-e2e-").await;
}
