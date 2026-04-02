//! Shared helpers for self-heal E2E tests.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;

use orca_control::state::AppState;
use orca_core::config::{ClusterConfig, ClusterMeta};

pub fn test_state(port: u16) -> Arc<AppState> {
    let runtime = Arc::new(orca_agent::docker::ContainerRuntime::new().expect("Docker required"));
    let config = ClusterConfig {
        cluster: ClusterMeta {
            name: "e2e-heal".into(),
            api_port: port,
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

pub async fn start_server_with_watchdog() -> (u16, Arc<AppState>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let state = test_state(port);

    // Spawn API
    let app = orca_control::api::router(state.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    // Spawn watchdog and health checker
    orca_control::watchdog::spawn_watchdog(state.clone());
    orca_control::health::spawn_health_checker(state.clone());

    tokio::time::sleep(Duration::from_millis(100)).await;
    (port, state)
}

pub struct Client {
    base: String,
    http: reqwest::Client,
}

impl Client {
    pub fn new(port: u16) -> Self {
        Self {
            base: format!("http://127.0.0.1:{port}"),
            http: reqwest::Client::new(),
        }
    }

    pub async fn deploy(&self, body: &serde_json::Value) -> reqwest::Response {
        self.http
            .post(format!("{}/api/v1/deploy", self.base))
            .json(body)
            .send()
            .await
            .unwrap()
    }
}

pub async fn cleanup(prefix: &str) {
    let docker = bollard::Docker::connect_with_local_defaults().unwrap();
    let opts = bollard::container::ListContainersOptions::<String> {
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
