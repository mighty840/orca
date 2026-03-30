//! Integration tests for the orca-control API endpoints using MockRuntime.

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

fn test_cluster_config() -> ClusterConfig {
    ClusterConfig {
        cluster: ClusterMeta {
            name: "test-cluster".to_string(),
            domain: None,
            acme_email: None,
            log_level: "info".to_string(),
            api_port: 0,
            grpc_port: 0,
        },
        node: Vec::new(),
        observability: None,
        ai: None,
    }
}

fn test_app_state() -> Arc<AppState> {
    let runtime = Arc::new(MockRuntime::with_host_port(9000));
    Arc::new(AppState::new(
        test_cluster_config(),
        runtime,
        None,
        Arc::new(RwLock::new(std::collections::HashMap::new())),
        Arc::new(RwLock::new(Vec::new())),
    ))
}

#[tokio::test]
async fn health_returns_ok() {
    let app = router(test_app_state());
    let req = Request::get("/api/v1/health").body(Body::empty()).unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["status"], "ok");
}

#[tokio::test]
async fn status_empty_cluster() {
    let app = router(test_app_state());
    let req = Request::get("/api/v1/status").body(Body::empty()).unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["cluster_name"], "test-cluster");
    assert!(json["services"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn deploy_creates_service() {
    let state = test_app_state();
    let app = router(state.clone());

    let deploy_body = serde_json::json!({
        "services": [{
            "name": "web",
            "image": "nginx:latest",
            "replicas": 2,
            "port": 8080
        }]
    });

    let req = Request::post("/api/v1/deploy")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&deploy_body).unwrap()))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(json["deployed"].as_array().unwrap().contains(&"web".into()));
    assert!(json["errors"].as_array().unwrap().is_empty());

    // Verify service was created in state
    let services = state.services.read().await;
    assert!(services.contains_key("web"));
    assert_eq!(services["web"].instances.len(), 2);
}

#[tokio::test]
async fn scale_nonexistent_service_returns_error() {
    let app = router(test_app_state());

    let scale_body = serde_json::json!({ "replicas": 5 });
    let req = Request::post("/api/v1/services/nonexistent/scale")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&scale_body).unwrap()))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn logs_nonexistent_service_returns_not_found() {
    let app = router(test_app_state());
    let req = Request::get("/api/v1/services/ghost/logs?tail=10")
        .body(Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
