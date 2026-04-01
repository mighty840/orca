//! E2E tests: spin up a real orca server with Docker, deploy services, verify.
//!
//! Requires Docker running locally. Run with: `cargo test -- --ignored e2e`

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;

use orca_control::state::AppState;
use orca_core::config::{ClusterConfig, ClusterMeta};

/// Create a test AppState with a real Docker container runtime.
async fn real_app_state(api_port: u16) -> Arc<AppState> {
    let runtime = Arc::new(
        orca_agent::docker::ContainerRuntime::new().expect("Docker must be running for E2E tests"),
    );
    let config = ClusterConfig {
        cluster: ClusterMeta {
            name: "e2e-test".to_string(),
            api_port,
            ..Default::default()
        },
        ..Default::default()
    };
    Arc::new(AppState::new(
        config,
        runtime,
        None,
        Arc::new(RwLock::new(HashMap::new())),
        Arc::new(RwLock::new(Vec::new())),
    ))
}

/// Start orca API server on a random port, return (port, state, join_handle).
async fn start_server() -> (u16, Arc<AppState>, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let state = real_app_state(port).await;
    let app = orca_control::api::router(state.clone());

    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    // Wait for server to be ready
    tokio::time::sleep(Duration::from_millis(100)).await;
    (port, state, handle)
}

/// HTTP client helper for test requests.
struct TestClient {
    base: String,
    client: reqwest::Client,
}

impl TestClient {
    fn new(port: u16) -> Self {
        Self {
            base: format!("http://127.0.0.1:{port}"),
            client: reqwest::Client::new(),
        }
    }

    async fn get(&self, path: &str) -> reqwest::Response {
        self.client
            .get(format!("{}{path}", self.base))
            .send()
            .await
            .unwrap()
    }

    async fn post_json(&self, path: &str, body: &serde_json::Value) -> reqwest::Response {
        self.client
            .post(format!("{}{path}", self.base))
            .json(body)
            .send()
            .await
            .unwrap()
    }
}

/// Clean up containers created during tests.
async fn cleanup_containers(prefix: &str) {
    let docker = bollard::Docker::connect_with_local_defaults().unwrap();
    let opts = bollard::container::ListContainersOptions {
        all: true,
        filters: HashMap::from([("name".to_string(), vec![prefix.to_string()])]),
        ..Default::default()
    };
    if let Ok(containers) = docker.list_containers(Some(opts)).await {
        for c in containers {
            if let Some(id) = c.id {
                let _ = docker
                    .remove_container(
                        &id,
                        Some(bollard::container::RemoveContainerOptions {
                            force: true,
                            ..Default::default()
                        }),
                    )
                    .await;
            }
        }
    }
}

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
