//! Integration test: concurrent deploys do not panic and leave consistent state.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;

use orca_control::api::router;
use orca_control::state::AppState;
use orca_core::config::{ClusterConfig, ClusterMeta};
use orca_core::testing::MockRuntime;

fn test_app_state() -> Arc<AppState> {
    let runtime = Arc::new(MockRuntime::new());
    let config = ClusterConfig {
        cluster: ClusterMeta {
            name: "concurrent-test".to_string(),
            api_port: 0,
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

/// Spawn 5 concurrent deploy requests, verify no panics and state is consistent.
#[tokio::test]
async fn test_concurrent_deploys_no_panic() {
    let state = test_app_state();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let app = router(state.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    tokio::time::sleep(Duration::from_millis(50)).await;

    let client = reqwest::Client::new();
    let mut handles = Vec::new();

    for i in 0..5 {
        let c = client.clone();
        let url = format!("http://127.0.0.1:{port}/api/v1/deploy");
        let handle = tokio::spawn(async move {
            let body = serde_json::json!({
                "services": [{
                    "name": format!("concurrent-svc-{i}"),
                    "image": "nginx:latest",
                    "replicas": 1,
                    "port": 8080 + i
                }]
            });
            let resp = c.post(&url).json(&body).send().await.unwrap();
            assert!(
                resp.status().is_success(),
                "deploy {i} failed with status {}",
                resp.status()
            );
        });
        handles.push(handle);
    }

    // Wait for all deploys to complete
    for h in handles {
        h.await.expect("deploy task should not panic");
    }

    // Verify all 5 services exist in state
    let services = state.services.read().await;
    for i in 0..5 {
        let name = format!("concurrent-svc-{i}");
        assert!(
            services.contains_key(&name),
            "service {name} should exist after concurrent deploy"
        );
        assert_eq!(
            services[&name].instances.len(),
            1,
            "service {name} should have 1 instance"
        );
    }
}
