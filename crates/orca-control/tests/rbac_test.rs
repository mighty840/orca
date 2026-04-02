//! RBAC tests: role-based access control for API endpoints.

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tokio::sync::RwLock;
use tower::ServiceExt;

use orca_control::api::router;
use orca_control::state::AppState;
use orca_core::config::{ApiToken, ClusterConfig, ClusterMeta, Role};
use orca_core::testing::MockRuntime;

fn state_with_tokens(tokens: Vec<ApiToken>) -> Arc<AppState> {
    let runtime = Arc::new(MockRuntime::new());
    let config = ClusterConfig {
        cluster: ClusterMeta {
            name: "rbac-test".into(),
            ..Default::default()
        },
        token: tokens,
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

fn admin_token() -> ApiToken {
    ApiToken {
        name: "admin".into(),
        value: "admin-tok".into(),
        role: Role::Admin,
    }
}

fn deployer_token() -> ApiToken {
    ApiToken {
        name: "ci".into(),
        value: "deploy-tok".into(),
        role: Role::Deployer,
    }
}

fn viewer_token() -> ApiToken {
    ApiToken {
        name: "dash".into(),
        value: "view-tok".into(),
        role: Role::Viewer,
    }
}

async fn req(state: &Arc<AppState>, method: &str, path: &str, token: &str) -> StatusCode {
    let app = router(state.clone());
    let req = match method {
        "GET" => Request::get(path)
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap(),
        "POST" => Request::post(path)
            .header("authorization", format!("Bearer {token}"))
            .header("content-type", "application/json")
            .body(Body::from("{}"))
            .unwrap(),
        "DELETE" => Request::delete(path)
            .header("authorization", format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap(),
        _ => panic!("unsupported method"),
    };
    app.oneshot(req).await.unwrap().status()
}

#[tokio::test]
async fn admin_can_deploy() {
    let state = state_with_tokens(vec![admin_token(), deployer_token(), viewer_token()]);
    let status = req(&state, "POST", "/api/v1/deploy", "admin-tok").await;
    // May be 200 or 206 (partial) depending on empty services — but not 401/403
    assert_ne!(status, StatusCode::UNAUTHORIZED);
    assert_ne!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn deployer_can_deploy() {
    let state = state_with_tokens(vec![admin_token(), deployer_token()]);
    let status = req(&state, "POST", "/api/v1/deploy", "deploy-tok").await;
    assert_ne!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn viewer_cannot_deploy() {
    let state = state_with_tokens(vec![viewer_token()]);
    let status = req(&state, "POST", "/api/v1/deploy", "view-tok").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn viewer_can_read_status() {
    let state = state_with_tokens(vec![viewer_token()]);
    let status = req(&state, "GET", "/api/v1/status", "view-tok").await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn viewer_cannot_stop() {
    let state = state_with_tokens(vec![viewer_token()]);
    let status = req(&state, "DELETE", "/api/v1/services/web", "view-tok").await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn deployer_can_stop() {
    let state = state_with_tokens(vec![deployer_token()]);
    // Will return 500 (service not found) but NOT 403
    let status = req(&state, "DELETE", "/api/v1/services/web", "deploy-tok").await;
    assert_ne!(status, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn invalid_token_rejected() {
    let state = state_with_tokens(vec![admin_token()]);
    let status = req(&state, "GET", "/api/v1/status", "wrong-tok").await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn no_token_rejected() {
    let state = state_with_tokens(vec![admin_token()]);
    let app = router(state);
    let req = Request::get("/api/v1/status").body(Body::empty()).unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn health_skips_auth_with_rbac() {
    let state = state_with_tokens(vec![admin_token()]);
    let app = router(state);
    let req = Request::get("/api/v1/health").body(Body::empty()).unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn metrics_skips_auth_with_rbac() {
    let state = state_with_tokens(vec![admin_token()]);
    let app = router(state);
    let req = Request::get("/metrics").body(Body::empty()).unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

#[tokio::test]
async fn role_can_method() {
    assert!(Role::Admin.can("deploy"));
    assert!(Role::Admin.can("stop"));
    assert!(Role::Deployer.can("deploy"));
    assert!(Role::Deployer.can("stop"));
    assert!(Role::Deployer.can("scale"));
    assert!(Role::Deployer.can("status"));
    assert!(!Role::Viewer.can("deploy"));
    assert!(!Role::Viewer.can("stop"));
    assert!(Role::Viewer.can("status"));
    assert!(Role::Viewer.can("logs"));
}
