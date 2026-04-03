//! Integration tests for canary deployment and promote operations.

use std::collections::HashMap;
use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use tokio::sync::RwLock;
use tower::ServiceExt;

use orca_control::api::router;
use orca_control::state::AppState;
use orca_core::config::{ClusterConfig, ClusterMeta, ServiceConfig};
use orca_core::testing::MockRuntime;
use orca_core::types::{DeployKind, DeployStrategy, Replicas};

fn test_app_state() -> Arc<AppState> {
    let runtime = Arc::new(MockRuntime::with_host_port(9000));
    let config = ClusterConfig {
        cluster: ClusterMeta {
            name: "canary-test".to_string(),
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

fn canary_config(name: &str, image: &str) -> ServiceConfig {
    ServiceConfig {
        name: name.to_string(),
        project: None,
        runtime: Default::default(),
        image: Some(image.to_string()),
        module: None,
        replicas: Replicas::Fixed(1),
        port: Some(8080),
        domain: Some("test.example.com".to_string()),
        health: None,
        readiness: None,
        liveness: None,
        env: HashMap::new(),
        resources: None,
        volume: None,
        deploy: Some(DeployStrategy {
            strategy: DeployKind::Canary,
            max_unavailable: None,
            canary_weight: 20,
        }),
        placement: None,
        network: None,
        aliases: vec![],
        mounts: vec![],
        routes: vec![],
        host_port: None,
        triggers: Vec::new(),
        assets: None,
        build: None,
        tls_cert: None,
        tls_key: None,
        internal: false,
        depends_on: vec![],
    }
}

/// Deploy with canary strategy keeps old instances and adds canary instances.
#[tokio::test]
async fn test_canary_deploy_keeps_old_instances() {
    let state = test_app_state();

    // Deploy v1 with rolling strategy first
    let mut v1 = canary_config("web", "nginx:1.0");
    v1.deploy = None; // Regular deploy for v1
    let body = serde_json::json!({ "services": [v1] });
    let app = router(state.clone());
    let req = Request::post("/api/v1/deploy")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Verify 1 instance running
    let services = state.services.read().await;
    assert_eq!(services["web"].instances.len(), 1);
    assert!(!services["web"].instances[0].is_canary);
    drop(services);

    // Deploy v2 with canary strategy
    let v2 = canary_config("web", "nginx:2.0");
    let body = serde_json::json!({ "services": [v2] });
    let app = router(state.clone());
    let req = Request::post("/api/v1/deploy")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // Should have 2 instances: 1 stable + 1 canary
    let services = state.services.read().await;
    let svc = &services["web"];
    assert_eq!(svc.instances.len(), 2);

    let stable_count = svc.instances.iter().filter(|i| !i.is_canary).count();
    let canary_count = svc.instances.iter().filter(|i| i.is_canary).count();
    assert_eq!(stable_count, 1);
    assert_eq!(canary_count, 1);
}

/// Promote removes old instances and marks canary as stable.
#[tokio::test]
async fn test_promote_removes_old() {
    let state = test_app_state();

    // Deploy v1 (regular)
    let mut v1 = canary_config("api", "app:1.0");
    v1.deploy = None;
    let body = serde_json::json!({ "services": [v1] });
    let app = router(state.clone());
    let req = Request::post("/api/v1/deploy")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap();
    app.oneshot(req).await.unwrap();

    // Deploy v2 (canary)
    let v2 = canary_config("api", "app:2.0");
    let body = serde_json::json!({ "services": [v2] });
    let app = router(state.clone());
    let req = Request::post("/api/v1/deploy")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap();
    app.oneshot(req).await.unwrap();

    // Verify we have stable + canary
    {
        let services = state.services.read().await;
        assert_eq!(services["api"].instances.len(), 2);
    }

    // Promote
    let app = router(state.clone());
    let req = Request::post("/api/v1/services/api/promote")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    // After promote: only canary instances remain, all marked stable
    let services = state.services.read().await;
    let svc = &services["api"];
    assert_eq!(svc.instances.len(), 1);
    assert!(!svc.instances[0].is_canary);
}

/// Promote on a service with no canary instances returns error.
#[tokio::test]
async fn test_promote_no_canary_returns_error() {
    let state = test_app_state();

    // Deploy regular service
    let mut config = canary_config("svc", "img:1.0");
    config.deploy = None;
    let body = serde_json::json!({ "services": [config] });
    let app = router(state.clone());
    let req = Request::post("/api/v1/deploy")
        .header("content-type", "application/json")
        .body(Body::from(serde_json::to_string(&body).unwrap()))
        .unwrap();
    app.oneshot(req).await.unwrap();

    // Try to promote without canary
    let app = router(state.clone());
    let req = Request::post("/api/v1/services/svc/promote")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::INTERNAL_SERVER_ERROR);

    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("no canary instances"));
}
