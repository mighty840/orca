//! Tests for project-level API operations: status filtering, stop by project.

use std::collections::HashMap;
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

fn test_state() -> Arc<AppState> {
    let runtime = Arc::new(MockRuntime::new());
    Arc::new(AppState::new(
        ClusterConfig {
            cluster: ClusterMeta {
                name: "proj-test".into(),
                ..Default::default()
            },
            ..Default::default()
        },
        runtime,
        None,
        Arc::new(RwLock::new(HashMap::new())),
        Arc::new(RwLock::new(Vec::new())),
    ))
}

async fn deploy(state: &Arc<AppState>, services: serde_json::Value) {
    let app = router(state.clone());
    let req = Request::post("/api/v1/deploy")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&services).unwrap()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

/// Status with project filter should only return services from that project.
#[tokio::test]
async fn status_filtered_by_project() {
    let state = test_state();

    // Deploy services from two different "projects"
    deploy(
        &state,
        serde_json::json!({
            "services": [
                {"name": "web-api", "image": "nginx:latest", "port": 80, "project": "frontend"},
                {"name": "web-admin", "image": "nginx:latest", "port": 81, "project": "frontend"},
                {"name": "db-main", "image": "postgres:16", "port": 5432, "project": "backend"},
            ]
        }),
    )
    .await;

    // Get status filtered by project=frontend
    let app = router(state.clone());
    let req = Request::get("/api/v1/status?project=frontend")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let services = json["services"].as_array().unwrap();

    assert_eq!(services.len(), 2, "should only return frontend services");
    assert!(
        services
            .iter()
            .all(|s| s["name"].as_str().unwrap().starts_with("web-"))
    );
}

/// Status without filter returns all services.
#[tokio::test]
async fn status_no_filter_returns_all() {
    let state = test_state();

    deploy(
        &state,
        serde_json::json!({
            "services": [
                {"name": "svc-a", "image": "nginx:latest", "port": 80, "project": "proj-a"},
                {"name": "svc-b", "image": "nginx:latest", "port": 81, "project": "proj-b"},
            ]
        }),
    )
    .await;

    let app = router(state.clone());
    let req = Request::get("/api/v1/status").body(Body::empty()).unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["services"].as_array().unwrap().len(), 2);
}

/// Stop by project should only stop services in that project.
#[tokio::test]
async fn stop_by_project() {
    let state = test_state();

    deploy(
        &state,
        serde_json::json!({
            "services": [
                {"name": "keep-svc", "image": "nginx:latest", "port": 80, "project": "keep"},
                {"name": "stop-svc", "image": "nginx:latest", "port": 81, "project": "remove"},
            ]
        }),
    )
    .await;

    // Stop project "remove"
    let app = router(state.clone());
    let req = Request::delete("/api/v1/projects/remove")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // "keep" project should still be running
    let services = state.services.read().await;
    assert!(services.contains_key("keep-svc"), "keep-svc should remain");
}

/// Metrics should include project label on instance counts.
#[tokio::test]
async fn metrics_include_project_label() {
    let state = test_state();

    deploy(
        &state,
        serde_json::json!({
            "services": [
                {"name": "met-svc", "image": "nginx:latest", "port": 80, "project": "myproj"},
            ]
        }),
    )
    .await;

    let app = router(state.clone());
    let req = Request::get("/metrics").body(Body::empty()).unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8(body.to_vec()).unwrap();

    assert!(
        text.contains("project="),
        "metrics should include project label"
    );
}
