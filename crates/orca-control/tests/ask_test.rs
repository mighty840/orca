//! Integration tests for `POST /api/v1/ask`.
//!
//! Covers the surfaces that don't need a real LLM: the 503 path when
//! `[ai]` is unconfigured, the 400 path on empty input, and the contract
//! between the TUI chat client and the server's response shape.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tokio::sync::RwLock;
use tower::ServiceExt;

use orca_control::api::router;
use orca_control::state::AppState;
use orca_core::config::{AiConfig, ClusterConfig, ClusterMeta};
use orca_core::testing::MockRuntime;

fn cluster_config_no_ai() -> ClusterConfig {
    ClusterConfig {
        cluster: ClusterMeta {
            name: "test-cluster".to_string(),
            api_port: 0,
            grpc_port: 0,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn cluster_config_with_ai() -> ClusterConfig {
    let mut cfg = cluster_config_no_ai();
    cfg.ai = Some(AiConfig {
        provider: "ollama".into(),
        endpoint: Some("http://127.0.0.1:1".into()), // intentionally unreachable
        model: Some("test-model".into()),
        api_key: None,
        alerts: None,
        auto_remediate: None,
    });
    cfg
}

fn build_state(cfg: ClusterConfig) -> Arc<AppState> {
    let runtime = Arc::new(MockRuntime::with_host_port(9000));
    Arc::new(AppState::new(
        cfg,
        runtime,
        None,
        Arc::new(RwLock::new(std::collections::HashMap::new())),
        Arc::new(RwLock::new(Vec::new())),
    ))
}

#[tokio::test]
async fn ask_returns_503_when_ai_is_not_configured() {
    let app = router(build_state(cluster_config_no_ai()));
    let req = Request::post("/api/v1/ask")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"question":"why?"}"#))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(
        json["error"]
            .as_str()
            .unwrap_or("")
            .contains("AI is not configured"),
        "503 body must explain how to fix the config — got {json}"
    );
}

#[tokio::test]
async fn ask_returns_400_on_empty_question() {
    let app = router(build_state(cluster_config_with_ai()));
    let req = Request::post("/api/v1/ask")
        .header("content-type", "application/json")
        .body(Body::from(r#"{"question":"   "}"#))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn ask_returns_400_when_history_missing_role_field() {
    let app = router(build_state(cluster_config_with_ai()));
    // History entries must have role + content; bad shape fails deserialization
    // before the handler runs, surfacing as 4xx via axum's JSON extractor.
    let req = Request::post("/api/v1/ask")
        .header("content-type", "application/json")
        .body(Body::from(
            r#"{"question":"hi","history":[{"content":"missing role"}]}"#,
        ))
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert!(
        resp.status().is_client_error(),
        "malformed history should produce a 4xx, got {}",
        resp.status()
    );
}
