//! E2E tests for proxy request flow: deploy a service with a domain,
//! verify the route table is populated correctly.
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
            name: "e2e-proxy-test".to_string(),
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

/// Start orca API server on a random port.
async fn start_server() -> (u16, Arc<AppState>, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let state = real_app_state(port).await;
    let app = orca_control::api::router(state.clone());

    let handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });

    tokio::time::sleep(Duration::from_millis(100)).await;
    (port, state, handle)
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
async fn e2e_proxy_route_table_populated_on_deploy() {
    let (port, state, _handle) = start_server().await;
    let client = reqwest::Client::new();
    let base = format!("http://127.0.0.1:{port}");

    // Deploy nginx with a domain so the route table gets populated
    let deploy = serde_json::json!({
        "services": [{
            "name": "e2e-proxy-rt",
            "image": "nginx:alpine",
            "replicas": 1,
            "port": 80,
            "domain": "proxy-test.local",
            "health": "/"
        }]
    });

    let resp = client
        .post(format!("{base}/api/v1/deploy"))
        .json(&deploy)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    // Wait for container and health check
    tokio::time::sleep(Duration::from_secs(5)).await;

    // Verify the route table has the domain with targets
    let routes = state.route_table.read().await;
    assert!(
        routes.contains_key("proxy-test.local"),
        "route table should contain proxy-test.local, got keys: {:?}",
        routes.keys().collect::<Vec<_>>()
    );
    let targets = &routes["proxy-test.local"];
    assert!(
        !targets.is_empty(),
        "should have at least one route target for proxy-test.local"
    );
    // Verify target has correct service name
    assert_eq!(targets[0].service_name, "e2e-proxy-rt");
    // Verify target address is non-empty
    assert!(
        !targets[0].address.is_empty(),
        "target address should be populated"
    );
    drop(routes);

    drop(state);
    cleanup_containers("orca-e2e-proxy-").await;
}
