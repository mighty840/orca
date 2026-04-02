//! Input validation edge-case tests using MockRuntime (no Docker needed).

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tokio::sync::RwLock;
use tower::ServiceExt;

use orca_control::api::router;
use orca_control::state::AppState;
use orca_core::config::{ClusterConfig, ClusterMeta};
use orca_core::testing::MockRuntime;

fn test_app_state() -> Arc<AppState> {
    let runtime = Arc::new(MockRuntime::with_host_port(9000));
    Arc::new(AppState::new(
        ClusterConfig {
            cluster: ClusterMeta {
                name: "validation-test".to_string(),
                api_port: 0,
                grpc_port: 0,
                ..Default::default()
            },
            ..Default::default()
        },
        runtime,
        None,
        Arc::new(RwLock::new(std::collections::HashMap::new())),
        Arc::new(RwLock::new(Vec::new())),
    ))
}

async fn deploy_json(
    state: &Arc<AppState>,
    body: &serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let app = router(state.clone());
    let req = Request::post("/api/v1/deploy")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(body).unwrap()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    (status, json)
}

#[tokio::test]
async fn test_deploy_empty_service_name() {
    let state = test_app_state();
    let body = serde_json::json!({
        "services": [{"name": "", "image": "nginx:latest", "replicas": 1, "port": 80}]
    });
    let (status, _json) = deploy_json(&state, &body).await;
    // Should not panic; either succeeds or returns an error status.
    assert!(
        status == StatusCode::OK || status.is_client_error() || status.is_server_error(),
        "empty name should not crash the server"
    );
}

#[tokio::test]
async fn test_deploy_service_name_special_chars() {
    let state = test_app_state();
    let body = serde_json::json!({
        "services": [{"name": "my service/foo bar\u{1F600}", "image": "nginx:latest", "replicas": 1, "port": 80}]
    });
    let (status, _json) = deploy_json(&state, &body).await;
    assert!(
        status == StatusCode::OK || status.is_client_error() || status.is_server_error(),
        "special chars in name should not crash the server"
    );
}

#[tokio::test]
async fn test_deploy_missing_image_and_module() {
    let state = test_app_state();
    let body = serde_json::json!({
        "services": [{"name": "no-image", "replicas": 1, "port": 80}]
    });
    let (status, json) = deploy_json(&state, &body).await;
    // Should return an error since there is no image, module, or build config.
    let has_error =
        status.is_server_error() || json["errors"].as_array().is_some_and(|e| !e.is_empty());
    assert!(
        has_error,
        "missing image/module/build should produce an error, got {status} {json}"
    );
}

#[tokio::test]
async fn test_deploy_port_zero() {
    let state = test_app_state();
    let body = serde_json::json!({
        "services": [{"name": "port-zero", "image": "nginx:latest", "replicas": 1, "port": 0}]
    });
    let (status, _json) = deploy_json(&state, &body).await;
    assert!(
        status == StatusCode::OK || status.is_client_error() || status.is_server_error(),
        "port=0 should not crash the server"
    );
}

#[tokio::test]
async fn test_deploy_port_too_high() {
    let state = test_app_state();
    // Port 99999 exceeds u16 max, so JSON deser should reject it or clamp.
    let body = serde_json::json!({
        "services": [{"name": "port-high", "image": "nginx:latest", "replicas": 1, "port": 99999}]
    });
    let app = router(state.clone());
    let req = Request::post("/api/v1/deploy")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    // 99999 > u16::MAX, so deserialization should fail with 422/400.
    // Either way, server must not crash.
    let status = resp.status();
    assert!(
        status.is_client_error() || status.is_server_error(),
        "port=99999 should be rejected, got {status}"
    );
}

#[tokio::test]
async fn test_deploy_duplicate_service_names() {
    let state = test_app_state();
    let body = serde_json::json!({
        "services": [
            {"name": "dup", "image": "nginx:latest", "replicas": 1, "port": 80},
            {"name": "dup", "image": "httpd:latest", "replicas": 2, "port": 8080}
        ]
    });
    let (status, _json) = deploy_json(&state, &body).await;
    assert!(
        status == StatusCode::OK || status == StatusCode::PARTIAL_CONTENT,
        "duplicate names should not crash, got {status}"
    );
    // Verify only one service entry exists (last one wins).
    let services = state.services.read().await;
    assert!(services.contains_key("dup"), "dup service should exist");
    // The second deploy overwrites the first, so we check instance count.
    let svc = &services["dup"];
    assert_eq!(svc.desired_replicas, 2, "last config should win");
}

#[tokio::test]
async fn test_deploy_replicas_zero() {
    let state = test_app_state();
    let body = serde_json::json!({
        "services": [{"name": "zero-rep", "image": "nginx:latest", "replicas": 0, "port": 80}]
    });
    let (status, _json) = deploy_json(&state, &body).await;
    assert_eq!(status, StatusCode::OK, "replicas=0 should succeed");
    let services = state.services.read().await;
    if let Some(svc) = services.get("zero-rep") {
        assert_eq!(
            svc.instances.len(),
            0,
            "replicas=0 should create no instances"
        );
    }
}

#[tokio::test]
async fn test_deploy_wasm_without_runtime() {
    // AppState has no wasm_runtime (None). Deploying a wasm service should error.
    let state = test_app_state();
    let body = serde_json::json!({
        "services": [{
            "name": "wasm-svc",
            "runtime": "wasm",
            "module": "test.wasm",
            "replicas": 1,
            "port": 80
        }]
    });
    let (status, json) = deploy_json(&state, &body).await;
    let has_error =
        status.is_server_error() || json["errors"].as_array().is_some_and(|e| !e.is_empty());
    assert!(
        has_error,
        "wasm deploy without runtime should error, got {status} {json}"
    );
}

#[tokio::test]
async fn test_scale_to_zero() {
    let state = test_app_state();
    // Deploy first with 2 replicas.
    let deploy_body = serde_json::json!({
        "services": [{"name": "scale-zero", "image": "nginx:latest", "replicas": 2, "port": 80}]
    });
    let (status, _) = deploy_json(&state, &deploy_body).await;
    assert_eq!(status, StatusCode::OK);

    // Scale to 0.
    let app = router(state.clone());
    let scale_body = serde_json::json!({"replicas": 0});
    let req = Request::post("/api/v1/services/scale-zero/scale")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&scale_body).unwrap()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "scale to 0 should succeed");

    let services = state.services.read().await;
    let svc = &services["scale-zero"];
    assert_eq!(svc.instances.len(), 0, "all instances should be stopped");
}
