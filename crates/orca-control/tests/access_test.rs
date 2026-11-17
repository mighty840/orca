//! #80 PR 1: `whoami`, the audit log, and the masked service detail, over
//! the real router with its auth middleware.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use orca_control::state::{AppState, ServiceState};
use orca_core::config::{ApiToken, ClusterConfig, ClusterMeta, Role, ServiceConfig};
use serde_json::Value;
use tokio::sync::RwLock;

const ADMIN: &str = "cluster-token";
const VIEWER: &str = "viewer-token";
const DEPLOYER: &str = "deployer-token";
const SENTINEL: &str = "pw-sentinel-9f2";

async fn start() -> (u16, Arc<AppState>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let token = |name: &str, value: &str, role| ApiToken {
        name: name.into(),
        value: value.into(),
        role,
    };
    let config = ClusterConfig {
        cluster: ClusterMeta {
            name: "access".into(),
            api_port: port,
            ..Default::default()
        },
        api_tokens: vec![ADMIN.into()],
        token: vec![
            token("dashboard", VIEWER, Role::Viewer),
            token("gitea-ci", DEPLOYER, Role::Deployer),
        ],
        ..Default::default()
    };
    let state = Arc::new(AppState::new(
        config,
        Arc::new(orca_core::testing::MockRuntime::new()),
        None,
        Arc::new(RwLock::new(HashMap::new())),
        Arc::new(RwLock::new(Vec::new())),
    ));
    let web: ServiceConfig = serde_json::from_value(serde_json::json!({
        "name": "web",
        "image": "nginx:1",
        "env": { "DB_PASSWORD": SENTINEL, "API_KEY": "${secrets.API_KEY}" },
    }))
    .unwrap();
    state.deploy_history.write().await.record(&web);
    let mut newer = web.clone();
    newer.image = Some("nginx:2".into());
    state.deploy_history.write().await.record(&newer);
    state
        .services
        .write()
        .await
        .insert("web".into(), ServiceState::from_config(newer));
    let app = orca_control::api::router(state.clone());
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap()
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    (port, state)
}

async fn call(port: u16, method: &str, path: &str, token: Option<&str>) -> (u16, String) {
    let client = reqwest::Client::new();
    let url = format!("http://127.0.0.1:{port}{path}");
    let mut req = match method {
        "GET" => client.get(url),
        _ => client.post(url).json(&serde_json::json!({ "replicas": 2 })),
    };
    if let Some(t) = token {
        req = req.bearer_auth(t).header("x-orca-client", "test");
    }
    let resp = req.send().await.unwrap();
    (resp.status().as_u16(), resp.text().await.unwrap())
}

#[tokio::test]
async fn whoami_names_the_token_and_its_actions() {
    let (port, _) = start().await;

    let (_, body) = call(port, "GET", "/api/v1/whoami", Some(VIEWER)).await;
    let me: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(me["name"], "dashboard");
    assert_eq!(me["role"], "viewer");
    assert_eq!(me["legacy"], false);
    assert_eq!(
        me["actions"],
        serde_json::json!(["status", "logs", "cluster_info"])
    );

    let (_, body) = call(port, "GET", "/api/v1/whoami", Some(ADMIN)).await;
    let me: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(
        (me["name"].as_str(), me["legacy"].as_bool()),
        (Some("legacy-1"), Some(true))
    );

    assert_eq!(call(port, "GET", "/api/v1/whoami", None).await.0, 401);
}

#[tokio::test]
async fn mutations_and_denials_are_audited_reads_are_not() {
    let (port, _) = start().await;
    call(port, "POST", "/api/v1/services/web/scale", Some(DEPLOYER)).await;
    let (denied, _) = call(port, "POST", "/api/v1/services/web/scale", Some(VIEWER)).await;
    assert_eq!(denied, 403);
    call(port, "POST", "/api/v1/services/web/scale", None).await;
    call(port, "GET", "/api/v1/status", Some(VIEWER)).await;

    assert_eq!(
        call(port, "GET", "/api/v1/audit", Some(VIEWER)).await.0,
        403
    );
    let (status, body) = call(port, "GET", "/api/v1/audit", Some(ADMIN)).await;
    assert_eq!(status, 200);
    let events: Vec<Value> = serde_json::from_str(&body).unwrap();
    let seen: Vec<(Value, u64)> = events
        .iter()
        .map(|e| (e["principal"].clone(), e["status"].as_u64().unwrap()))
        .collect();
    // Newest first; the status read is not there, nor the audit reads.
    assert_eq!(seen.len(), 3, "{events:?}");
    assert_eq!(seen[0], (Value::Null, 401));
    assert_eq!(seen[1], (Value::from("dashboard"), 403));
    assert_eq!(seen[2].0, Value::from("gitea-ci"));
    assert_eq!(events[2]["route"], "/api/v1/services/{name}/scale");
    assert_eq!(events[2]["target"], "web");
    assert_eq!(events[2]["client"], "test");
}

#[tokio::test]
async fn a_viewer_reads_a_service_and_its_history_without_secrets() {
    let (port, _) = start().await;

    let (status, body) = call(port, "GET", "/api/v1/services/web", Some(VIEWER)).await;
    assert_eq!(status, 200, "{body}");
    assert!(!body.contains(SENTINEL), "{body}");
    let detail: Value = serde_json::from_str(&body).unwrap();
    assert_eq!(detail["status"]["name"], "web");
    assert_eq!(detail["config"]["env"]["DB_PASSWORD"], "***");
    assert_eq!(detail["config"]["env"]["API_KEY"], "${secrets.API_KEY}");

    let (status, body) = call(port, "GET", "/api/v1/services/web/history", Some(VIEWER)).await;
    assert_eq!(status, 200);
    assert!(!body.contains(SENTINEL), "{body}");
    let history: Vec<Value> = serde_json::from_str(&body).unwrap();
    let images: Vec<&str> = history
        .iter()
        .map(|r| r["config"]["image"].as_str().unwrap())
        .collect();
    assert_eq!(images, ["nginx:2", "nginx:1"], "newest first");

    assert_eq!(
        call(port, "GET", "/api/v1/services/nope", Some(VIEWER))
            .await
            .0,
        404
    );
}
