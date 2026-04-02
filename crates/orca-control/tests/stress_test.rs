//! Stress tests for orca-control (non-Docker).
//!
//! The concurrent deploys test uses MockRuntime and runs without Docker.
//! Docker-based stress tests are in `stress_docker_test.rs`.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::sync::RwLock;

use orca_control::api::router;
use orca_control::state::AppState;
use orca_core::config::{ClusterConfig, ClusterMeta};

fn mock_app_state() -> Arc<AppState> {
    // Use MockRuntime without host_port to skip wait_for_ready() probes.
    let runtime = Arc::new(orca_core::testing::MockRuntime::new());
    let config = ClusterConfig {
        cluster: ClusterMeta {
            name: "stress-test".into(),
            api_port: 0,
            grpc_port: 0,
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

// ---------------------------------------------------------------------------
// 4. Concurrent deploy requests (MockRuntime — no Docker)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn stress_concurrent_deploys() {
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use http_body_util::BodyExt;
    use tower::ServiceExt;

    let state = mock_app_state();
    let mut handles = Vec::new();

    for i in 0..10 {
        let s = state.clone();
        handles.push(tokio::spawn(async move {
            let app = router(s);
            let body = serde_json::json!({
                "services": [{
                    "name": format!("conc-{i}"),
                    "image": "nginx:latest",
                    "replicas": 2,
                    "port": 8080
                }]
            });
            let req = Request::post("/api/v1/deploy")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_string(&body).unwrap()))
                .unwrap();
            let resp = app.oneshot(req).await.unwrap();
            assert_eq!(resp.status(), StatusCode::OK);
            let bytes = resp.into_body().collect().await.unwrap().to_bytes();
            let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
            assert!(json["errors"].as_array().unwrap().is_empty());
        }));
    }

    for h in handles {
        h.await.expect("task panicked");
    }

    let services = state.services.read().await;
    assert_eq!(services.len(), 10, "expected 10 services");
    for i in 0..10 {
        let name = format!("conc-{i}");
        let svc = services
            .get(&name)
            .unwrap_or_else(|| panic!("missing {name}"));
        assert_eq!(svc.instances.len(), 2, "{name} should have 2 replicas");
    }
}
