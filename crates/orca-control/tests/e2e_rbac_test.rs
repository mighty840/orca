//! E2E test: RBAC named-token enforcement.
//!
//! Configure three `[[token]]` entries with `viewer`, `deployer`, and `admin`
//! roles, then exercise the matrix of (token, endpoint) combinations against
//! the live auth middleware:
//!
//! | role     | GET /status | POST /deploy | GET /secrets |
//! | -------- | ----------- | ------------ | ------------ |
//! | viewer   | 200         | 403          | 403          |
//! | deployer | 200         | 200          | 403          |
//! | admin    | 200         | 200          | 200          |
//!
//! Regression coverage for `auth::Role::can` and `auth_middleware`. The most
//! likely break is a default-role drift (e.g. adding a new action category
//! and forgetting to lock it out of viewer/deployer).
//!
//! Run with: `cargo test -p orca-control --test e2e_rbac_test -- --ignored`

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;

use orca_control::state::AppState;
use orca_core::config::{ApiToken, ClusterConfig, ClusterMeta, Role};

const VIEWER: &str = "viewer-token-xyz";
const DEPLOYER: &str = "deployer-token-xyz";
const ADMIN: &str = "admin-token-xyz";

async fn start_rbac_server() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let runtime = Arc::new(
        orca_agent::docker::ContainerRuntime::new().expect("Docker must be running for E2E tests"),
    );
    let config = ClusterConfig {
        cluster: ClusterMeta {
            name: "e2e-rbac".into(),
            api_port: port,
            ..Default::default()
        },
        token: vec![
            ApiToken {
                name: "v".into(),
                value: VIEWER.into(),
                role: Role::Viewer,
            },
            ApiToken {
                name: "d".into(),
                value: DEPLOYER.into(),
                role: Role::Deployer,
            },
            ApiToken {
                name: "a".into(),
                value: ADMIN.into(),
                role: Role::Admin,
            },
        ],
        ..Default::default()
    };
    let state = Arc::new(AppState::new(
        config,
        runtime,
        None,
        Arc::new(RwLock::new(HashMap::new())),
        Arc::new(RwLock::new(Vec::new())),
    ));
    let app = orca_control::api::router(state);
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap()
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    port
}

async fn status(port: u16, token: &str) -> u16 {
    reqwest::Client::new()
        .get(format!("http://127.0.0.1:{port}/api/v1/status"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

async fn deploy(port: u16, token: &str, name: &str) -> u16 {
    let body = serde_json::json!({
        "services": [{
            "name": name,
            "image": "nginx:alpine",
            "replicas": 1,
            "port": 80
        }]
    });
    reqwest::Client::new()
        .post(format!("http://127.0.0.1:{port}/api/v1/deploy"))
        .bearer_auth(token)
        .json(&body)
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

async fn list_secrets(port: u16, token: &str) -> u16 {
    reqwest::Client::new()
        .get(format!("http://127.0.0.1:{port}/api/v1/secrets"))
        .bearer_auth(token)
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

#[tokio::test]
#[ignore]
async fn e2e_rbac_viewer_allowed_only_for_status() {
    let port = start_rbac_server().await;
    assert_eq!(status(port, VIEWER).await, 200);
    assert_eq!(
        deploy(port, VIEWER, "e2e-rbac-viewer-deploy").await,
        403,
        "viewer must not be allowed to deploy"
    );
    assert_eq!(
        list_secrets(port, VIEWER).await,
        403,
        "viewer must not be allowed to list secrets"
    );
}

#[tokio::test]
#[ignore]
async fn e2e_rbac_deployer_can_deploy_but_not_secrets() {
    let port = start_rbac_server().await;
    assert_eq!(status(port, DEPLOYER).await, 200);
    let dep = deploy(port, DEPLOYER, "e2e-rbac-deployer-deploy").await;
    assert!(
        dep == 200 || dep == 206,
        "deployer should be allowed to deploy, got {dep}"
    );
    assert_eq!(
        list_secrets(port, DEPLOYER).await,
        403,
        "deployer must not be allowed to list secrets"
    );

    // Cleanup
    let _ = reqwest::Client::new()
        .delete(format!(
            "http://127.0.0.1:{port}/api/v1/services/e2e-rbac-deployer-deploy"
        ))
        .bearer_auth(ADMIN)
        .send()
        .await;
}

#[tokio::test]
#[ignore]
async fn e2e_rbac_admin_can_everything() {
    let port = start_rbac_server().await;
    assert_eq!(status(port, ADMIN).await, 200);
    assert_eq!(list_secrets(port, ADMIN).await, 200);
    let dep = deploy(port, ADMIN, "e2e-rbac-admin-deploy").await;
    assert!(
        dep == 200 || dep == 206,
        "admin should be allowed to deploy, got {dep}"
    );

    // Cleanup
    let _ = reqwest::Client::new()
        .delete(format!(
            "http://127.0.0.1:{port}/api/v1/services/e2e-rbac-admin-deploy"
        ))
        .bearer_auth(ADMIN)
        .send()
        .await;
}
