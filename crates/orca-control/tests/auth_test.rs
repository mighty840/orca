//! Integration tests for API authentication.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tokio::sync::RwLock;
use tower::ServiceExt;

use orca_control::api::router;
use orca_control::state::AppState;
use orca_core::config::ClusterConfig;
use orca_core::testing::MockRuntime;

fn config_with_tokens(tokens: Vec<String>) -> ClusterConfig {
    ClusterConfig {
        api_tokens: tokens,
        ..Default::default()
    }
}

fn app_with_tokens(tokens: Vec<String>) -> axum::Router {
    let runtime = Arc::new(MockRuntime::new());
    let state = Arc::new(AppState::new(
        config_with_tokens(tokens),
        runtime,
        None,
        Arc::new(RwLock::new(std::collections::HashMap::new())),
        Arc::new(RwLock::new(Vec::new())),
    ));
    router(state)
}

#[tokio::test]
async fn no_tokens_allows_all_requests() {
    let app = app_with_tokens(vec![]);
    let req = Request::get("/api/v1/status").body(Body::empty()).unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn health_skips_auth() {
    let app = app_with_tokens(vec!["secret123".into()]);
    let req = Request::get("/api/v1/health").body(Body::empty()).unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn missing_token_returns_401() {
    let app = app_with_tokens(vec!["secret123".into()]);
    let req = Request::get("/api/v1/status").body(Body::empty()).unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body[..], b"missing bearer token");
}

#[tokio::test]
async fn wrong_token_returns_401() {
    let app = app_with_tokens(vec!["secret123".into()]);
    let req = Request::get("/api/v1/status")
        .header("Authorization", "Bearer wrongtoken")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(&body[..], b"invalid bearer token");
}

#[tokio::test]
async fn valid_token_allows_request() {
    let app = app_with_tokens(vec!["secret123".into()]);
    let req = Request::get("/api/v1/status")
        .header("Authorization", "Bearer secret123")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn deploy_with_valid_token_works() {
    let app = app_with_tokens(vec!["mytoken".into()]);
    let body = serde_json::json!({
        "services": [{
            "name": "test",
            "image": "nginx:latest",
            "replicas": 1,
            "port": 80
        }]
    });
    let req = Request::post("/api/v1/deploy")
        .header("Authorization", "Bearer mytoken")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}
