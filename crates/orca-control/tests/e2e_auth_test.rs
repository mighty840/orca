//! E2E test: bearer token authentication enforcement.
//!
//! Regression coverage for `auth_middleware`: when `api_tokens` is configured,
//! requests without a bearer must 401, requests with the wrong bearer must
//! 401, and requests with the configured bearer must 200. The unauthenticated
//! exempt list (`/api/v1/health`) keeps responding either way.
//!
//! Run with: `cargo test -p orca-control --test e2e_auth_test -- --ignored`

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;

use orca_control::state::AppState;
use orca_core::config::{ClusterConfig, ClusterMeta};

const TOKEN: &str = "e2e-auth-token";

async fn start_authed_server() -> (u16, Arc<AppState>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();

    let runtime = Arc::new(
        orca_agent::docker::ContainerRuntime::new().expect("Docker must be running for E2E tests"),
    );
    let config = ClusterConfig {
        cluster: ClusterMeta {
            name: "e2e-auth".into(),
            api_port: port,
            ..Default::default()
        },
        api_tokens: vec![TOKEN.into()],
        ..Default::default()
    };
    let state = Arc::new(AppState::new(
        config,
        runtime,
        None,
        Arc::new(RwLock::new(HashMap::new())),
        Arc::new(RwLock::new(Vec::new())),
    ));
    let app = orca_control::api::router(state.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    tokio::time::sleep(Duration::from_millis(100)).await;
    (port, state)
}

#[tokio::test]
#[ignore]
async fn e2e_auth_rejects_missing_bearer() {
    let (port, _state) = start_authed_server().await;
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://127.0.0.1:{port}/api/v1/status"))
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        401,
        "expected 401 when Authorization header is absent"
    );
}

#[tokio::test]
#[ignore]
async fn e2e_auth_rejects_bad_bearer() {
    let (port, _state) = start_authed_server().await;
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://127.0.0.1:{port}/api/v1/status"))
        .bearer_auth("not-the-right-token")
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        401,
        "expected 401 for an unrecognized bearer token"
    );
}

#[tokio::test]
#[ignore]
async fn e2e_auth_accepts_valid_bearer() {
    let (port, _state) = start_authed_server().await;
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://127.0.0.1:{port}/api/v1/status"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "configured bearer should authenticate");
}

/// `/api/v1/health` is on the auth-exempt list so liveness probes don't need
/// to know the cluster token. This test would catch a regression that
/// accidentally removed the exemption.
#[tokio::test]
#[ignore]
async fn e2e_auth_health_endpoint_is_exempt() {
    let (port, _state) = start_authed_server().await;
    let client = reqwest::Client::new();
    let resp = client
        .get(format!("http://127.0.0.1:{port}/api/v1/health"))
        .send()
        .await
        .unwrap();
    assert!(
        resp.status().is_success(),
        "health endpoint must respond without auth, got {}",
        resp.status()
    );
}
