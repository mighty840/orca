//! Authentication and authorization of the agent control-plane WebSocket.
//!
//! The agent channel carries resolved secrets for every service pinned to the
//! node and trusts the node's reports, so these tests pin down who may open it
//! and what a session may claim once open (#201).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::RwLock;
use tokio_tungstenite::tungstenite::{Error as WsError, Message};

use orca_control::state::{AppState, RegisteredNode};
use orca_core::config::{ApiToken, ClusterConfig, Role};
use orca_core::testing::MockRuntime;
use orca_core::ws_types::{AgentMessage, HostStats};

const ADMIN: &str = "legacy-admin-token";
const VIEWER: &str = "dashboard-viewer-token";
const DEPLOYER: &str = "ci-deployer-token";

fn test_state() -> Arc<AppState> {
    let config = ClusterConfig {
        api_tokens: vec![ADMIN.to_string()],
        token: vec![
            ApiToken {
                name: "dashboard".into(),
                value: VIEWER.into(),
                role: Role::Viewer,
            },
            ApiToken {
                name: "ci".into(),
                value: DEPLOYER.into(),
                role: Role::Deployer,
            },
        ],
        ..Default::default()
    };
    Arc::new(AppState::new(
        config,
        Arc::new(MockRuntime::new()),
        None,
        Arc::new(RwLock::new(HashMap::new())),
        Arc::new(RwLock::new(Vec::new())),
    ))
}

async fn start_server(state: Arc<AppState>) -> std::net::SocketAddr {
    use axum::routing::get;
    let app = axum::Router::new()
        .route(
            "/api/v1/ws/agent",
            get(orca_control::ws_handler::ws_agent_handler),
        )
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(
            listener,
            app.into_make_service_with_connect_info::<std::net::SocketAddr>(),
        )
        .await
        .unwrap();
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    addr
}

/// The HTTP status the server answered a refused upgrade with.
fn refusal_status(err: WsError) -> u16 {
    match err {
        WsError::Http(resp) => resp.status().as_u16(),
        other => panic!("expected an HTTP refusal, got {other:?}"),
    }
}

fn node(node_id: u64) -> RegisteredNode {
    RegisteredNode {
        node_id,
        address: format!("node-{node_id}:6881"),
        peer_ip: None,
        labels: HashMap::new(),
        last_heartbeat: chrono::Utc::now(),
        drain: false,
        cpu_percent: 0.0,
        memory_bytes: 0,
        memory_total: 0,
        disk_used: 0,
        disk_total: 0,
        net_rx: 0,
        net_tx: 0,
    }
}

#[tokio::test]
async fn admin_token_opens_the_agent_channel() {
    // The cluster token agents join with is a legacy admin token. Requiring
    // admin must not lock out an agent that has not been upgraded.
    let addr = start_server(test_state()).await;
    let url = format!("ws://{addr}/api/v1/ws/agent?token={ADMIN}&node_id=1");
    assert!(tokio_tungstenite::connect_async(&url).await.is_ok());
}

#[tokio::test]
async fn viewer_token_is_forbidden_from_the_agent_channel() {
    // A dashboard token must not be able to pose as a node and receive that
    // node's resolved secrets.
    let addr = start_server(test_state()).await;
    let url = format!("ws://{addr}/api/v1/ws/agent?token={VIEWER}&node_id=1");
    let err = tokio_tungstenite::connect_async(&url).await.unwrap_err();
    assert_eq!(refusal_status(err), 403);
}

#[tokio::test]
async fn deployer_token_is_forbidden_from_the_agent_channel() {
    let addr = start_server(test_state()).await;
    let url = format!("ws://{addr}/api/v1/ws/agent?token={DEPLOYER}&node_id=1");
    let err = tokio_tungstenite::connect_async(&url).await.unwrap_err();
    assert_eq!(refusal_status(err), 403);
}

#[tokio::test]
async fn unknown_and_empty_tokens_are_unauthorized() {
    let addr = start_server(test_state()).await;
    for token in ["not-a-token", ""] {
        let url = format!("ws://{addr}/api/v1/ws/agent?token={token}&node_id=1");
        let err = tokio_tungstenite::connect_async(&url).await.unwrap_err();
        assert_eq!(refusal_status(err), 401, "token {token:?}");
    }
}

#[tokio::test]
async fn heartbeat_is_attributed_to_the_session_node_not_the_claimed_one() {
    // A session connected as node 42 sends a heartbeat claiming to be node 99.
    // Before #201 the claimed id was trusted, so one session could overwrite
    // another node's reported state.
    let state = test_state();
    {
        let mut nodes = state.registered_nodes.write().await;
        nodes.insert(99, node(99));
    }
    let addr = start_server(state.clone()).await;
    let url = format!("ws://{addr}/api/v1/ws/agent?token={ADMIN}&node_id=42");
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    // Consume the Ack so the session is fully registered.
    let _ = tokio::time::timeout(Duration::from_secs(2), ws.next()).await;

    let spoof = AgentMessage::Heartbeat {
        node_id: 99,
        workloads: vec![],
        stats: HostStats {
            cpu_percent: 77.7,
            memory_bytes: 1,
            memory_total: 2,
            disk_used: 3,
            disk_total: 4,
            net_rx: 5,
            net_tx: 6,
            domains: vec![],
        },
    };
    ws.send(Message::Text(serde_json::to_string(&spoof).unwrap().into()))
        .await
        .unwrap();

    // Wait for the master to process the frame.
    let mut attributed = false;
    for _ in 0..40 {
        tokio::time::sleep(Duration::from_millis(25)).await;
        let nodes = state.registered_nodes.read().await;
        if nodes.get(&42).is_some_and(|n| n.cpu_percent == 77.7) {
            attributed = true;
            break;
        }
    }
    let nodes = state.registered_nodes.read().await;
    assert!(
        attributed,
        "heartbeat must update the session's own node 42"
    );
    assert_eq!(
        nodes.get(&99).map(|n| n.cpu_percent),
        Some(0.0),
        "the claimed node 99 must be untouched"
    );
}
