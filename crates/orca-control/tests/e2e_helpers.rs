//! Shared helpers for E2E tests.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;

use orca_control::state::AppState;
use orca_core::config::{ClusterConfig, ClusterMeta};

/// Create a test AppState with a real Docker container runtime.
pub async fn real_app_state(api_port: u16) -> Arc<AppState> {
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
pub async fn start_server() -> (u16, Arc<AppState>, tokio::task::JoinHandle<()>) {
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
pub struct TestClient {
    pub base: String,
    pub client: reqwest::Client,
}

impl TestClient {
    pub fn new(port: u16) -> Self {
        Self {
            base: format!("http://127.0.0.1:{port}"),
            client: reqwest::Client::new(),
        }
    }

    pub async fn get(&self, path: &str) -> reqwest::Response {
        self.client
            .get(format!("{}{path}", self.base))
            .send()
            .await
            .unwrap()
    }

    pub async fn post_json(&self, path: &str, body: &serde_json::Value) -> reqwest::Response {
        self.client
            .post(format!("{}{path}", self.base))
            .json(body)
            .send()
            .await
            .unwrap()
    }
}

/// Clean up containers created during tests.
pub async fn cleanup_containers(prefix: &str) {
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
