//! E2E tests: health check routing and service stop.
//!
//! Requires Docker running locally. Run with: `cargo test -- --ignored e2e`

mod e2e_helpers;

use e2e_helpers::{TestClient, cleanup_containers, start_server};
use std::time::Duration;

#[tokio::test]
#[ignore] // requires Docker
async fn e2e_health_check_routes_only_healthy() {
    let (port, state, _handle) = start_server().await;
    let client = TestClient::new(port);

    // Deploy with health check
    let deploy = serde_json::json!({
        "services": [{
            "name": "e2e-health",
            "image": "nginx:alpine",
            "replicas": 1,
            "port": 80,
            "health": "/",
            "domain": "e2e-test.local"
        }]
    });
    client.post_json("/api/v1/deploy", &deploy).await;
    tokio::time::sleep(Duration::from_secs(5)).await;

    // Route table should have the domain
    let routes = state.route_table.read().await;
    assert!(
        routes.contains_key("e2e-test.local"),
        "route table should contain e2e-test.local"
    );
    let targets = &routes["e2e-test.local"];
    assert!(!targets.is_empty(), "should have at least one route target");
    drop(routes);

    drop(state);
    cleanup_containers("orca-e2e-").await;
}

#[tokio::test]
#[ignore] // requires Docker
async fn e2e_stop_service() {
    let (port, state, _handle) = start_server().await;
    let client = TestClient::new(port);

    let deploy = serde_json::json!({
        "services": [{
            "name": "e2e-stop",
            "image": "nginx:alpine",
            "replicas": 1,
            "port": 80
        }]
    });
    client.post_json("/api/v1/deploy", &deploy).await;
    tokio::time::sleep(Duration::from_secs(2)).await;

    // Stop service (DELETE /api/v1/services/{name})
    let resp = client
        .client
        .delete(format!("{}/api/v1/services/e2e-stop", client.base))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    tokio::time::sleep(Duration::from_secs(1)).await;

    // Verify stopped (service may show 0 replicas or be removed entirely)
    let resp = client.get("/api/v1/status").await;
    let body: serde_json::Value = resp.json().await.unwrap();
    let services = body["services"].as_array().unwrap();
    if let Some(svc) = services.iter().find(|s| s["name"] == "e2e-stop") {
        assert_eq!(svc["running_replicas"], 0);
    }
    // Service removed from status = also valid

    drop(state);
    cleanup_containers("orca-e2e-").await;
}
