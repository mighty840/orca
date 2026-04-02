//! Integration tests for webhook-triggered deploys.
//!
//! Tests the full flow: register webhook config, deploy a service,
//! send a mock webhook POST, verify reconciliation happens.

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use hmac::{Hmac, Mac};
use http_body_util::BodyExt;
use sha2::Sha256;
use tokio::sync::RwLock;
use tower::ServiceExt;

use orca_control::api::router;
use orca_control::state::AppState;
use orca_core::config::{ClusterConfig, ClusterMeta};
use orca_core::testing::MockRuntime;

type HmacSha256 = Hmac<Sha256>;

fn test_state() -> Arc<AppState> {
    let runtime = Arc::new(MockRuntime::with_host_port(9000));
    Arc::new(AppState::new(
        ClusterConfig {
            cluster: ClusterMeta {
                name: "test".to_string(),
                api_port: 0,
                grpc_port: 0,
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

fn github_push_payload(repo: &str, branch: &str) -> serde_json::Value {
    serde_json::json!({
        "ref": format!("refs/heads/{branch}"),
        "repository": { "full_name": repo },
        "head_commit": { "id": "abc12345", "message": "deploy test" }
    })
}

fn sign_payload(secret: &str, body: &[u8]) -> String {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(body);
    let sig = hex::encode(mac.finalize().into_bytes());
    format!("sha256={sig}")
}

/// Deploy a service via the API so it exists in state for redeploy.
async fn deploy_service(state: &Arc<AppState>, name: &str) {
    let app = router(state.clone());
    let body = serde_json::json!({
        "services": [{ "name": name, "image": "nginx:latest", "replicas": 1, "port": 80 }]
    });
    let req = Request::post("/api/v1/deploy")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
}

/// Register a webhook config via the API.
async fn register_webhook(
    state: &Arc<AppState>,
    repo: &str,
    service: &str,
    branch: &str,
    secret: Option<&str>,
) {
    let app = router(state.clone());
    let mut body = serde_json::json!({
        "repo": repo,
        "service_name": service,
        "branch": branch,
    });
    if let Some(s) = secret {
        body["secret"] = serde_json::Value::String(s.to_string());
    }
    let req = Request::post("/api/v1/webhooks")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
}

/// Send a webhook push POST and return (status, body_json).
async fn send_webhook(
    state: &Arc<AppState>,
    payload: &serde_json::Value,
    signature: Option<&str>,
) -> (StatusCode, serde_json::Value) {
    let app = router(state.clone());
    let body_bytes = serde_json::to_vec(payload).unwrap();
    let mut builder =
        Request::post("/api/v1/webhooks/github").header("content-type", "application/json");
    if let Some(sig) = signature {
        builder = builder.header("X-Hub-Signature-256", sig);
    }
    let req = builder.body(Body::from(body_bytes)).unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&bytes);
    let json = serde_json::from_str(&text).unwrap_or(serde_json::json!({"raw": text.to_string()}));
    (status, json)
}

#[tokio::test]
async fn test_webhook_triggers_reconcile() {
    let state = test_state();

    // 1. Deploy a service so it exists in state
    deploy_service(&state, "myapp").await;

    // 2. Register a webhook (no secret for simplicity)
    register_webhook(&state, "org/repo", "myapp", "main", None).await;

    // 3. Send a push webhook
    let payload = github_push_payload("org/repo", "main");
    let (status, body) = send_webhook(&state, &payload, None).await;

    assert_eq!(status, StatusCode::OK);
    let deployed = body["deployed"].as_array().unwrap();
    assert!(deployed.contains(&serde_json::json!("myapp")));

    // 4. Verify the service still exists (was redeployed, not removed)
    let services = state.services.read().await;
    assert!(services.contains_key("myapp"));
}

#[tokio::test]
async fn test_webhook_invalid_signature_rejected() {
    let state = test_state();
    let secret = "super-secret-key";

    // Deploy service and register webhook with a secret
    deploy_service(&state, "secured").await;
    register_webhook(&state, "org/secured", "secured", "main", Some(secret)).await;

    // Send webhook with WRONG signature
    let payload = github_push_payload("org/secured", "main");
    let (status, _body) = send_webhook(&state, &payload, Some("sha256=badbad")).await;

    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn test_webhook_valid_signature_accepted() {
    let state = test_state();
    let secret = "super-secret-key";

    deploy_service(&state, "secured").await;
    register_webhook(&state, "org/secured", "secured", "main", Some(secret)).await;

    let payload = github_push_payload("org/secured", "main");
    let body_bytes = serde_json::to_vec(&payload).unwrap();
    let sig = sign_payload(secret, &body_bytes);
    let (status, body) = send_webhook(&state, &payload, Some(&sig)).await;

    assert_eq!(status, StatusCode::OK);
    assert!(
        body["deployed"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("secured"))
    );
}

#[tokio::test]
async fn test_webhook_unknown_repo_ignored() {
    let state = test_state();

    // Register a webhook for org/known, but send push for org/unknown
    deploy_service(&state, "myapp").await;
    register_webhook(&state, "org/known", "myapp", "main", None).await;

    let payload = github_push_payload("org/unknown", "main");
    let (status, _body) = send_webhook(&state, &payload, None).await;

    // Unknown repo returns 200 (ignored, not an error)
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn test_webhook_wrong_branch_ignored() {
    let state = test_state();

    deploy_service(&state, "myapp").await;
    register_webhook(&state, "org/repo", "myapp", "main", None).await;

    // Push to "develop" branch — should be ignored
    let payload = github_push_payload("org/repo", "develop");
    let (status, _body) = send_webhook(&state, &payload, None).await;

    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn test_webhook_list_returns_registered() {
    let state = test_state();

    register_webhook(&state, "org/api", "api-svc", "main", None).await;
    register_webhook(&state, "org/web", "web-svc", "prod", None).await;

    let app = router(state.clone());
    let req = Request::get("/api/v1/webhooks")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let hooks = json["webhooks"].as_array().unwrap();
    assert_eq!(hooks.len(), 2);
}
