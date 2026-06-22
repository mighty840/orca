//! End-to-end test for the orphan-adoption reconciler (#95).
//!
//! Spins up the real `ws_agent_handler`, connects a fake agent that answers an
//! `AdoptionScanRequest` with one running `orca.managed` container the master
//! has never heard of, drives one adoption cycle, and asserts the master
//! registered the orphan with a `remote-<node_id>` placeholder.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::RwLock;
use tokio_tungstenite::tungstenite;

use orca_core::config::ClusterConfig;
use orca_core::testing::MockRuntime;
use orca_core::types::WorkloadStatus;
use orca_core::ws_types::{AdoptionReportData, AgentMessage, ManagedContainer, MasterMessage};

fn test_state() -> Arc<orca_control::state::AppState> {
    let mut cfg = ClusterConfig::default();
    cfg.api_tokens = vec!["tok".to_string()];
    Arc::new(orca_control::state::AppState::new(
        cfg,
        Arc::new(MockRuntime::new()),
        None,
        Arc::new(RwLock::new(HashMap::new())),
        Arc::new(RwLock::new(Vec::new())),
    ))
}

async fn start_server(state: Arc<orca_control::state::AppState>) -> std::net::SocketAddr {
    let app = axum::Router::new()
        .route(
            "/api/v1/ws/agent",
            axum::routing::get(orca_control::ws_handler::ws_agent_handler),
        )
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    tokio::time::sleep(Duration::from_millis(50)).await;
    addr
}

#[tokio::test]
async fn adoption_cycle_registers_orphan_from_agent() {
    let state = test_state();
    let addr = start_server(state.clone()).await;

    let url = format!("ws://{addr}/api/v1/ws/agent?token=tok&node_id=7");
    let (ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();

    // Fake agent: reply to AdoptionScanRequest with one running orphan.
    let agent = tokio::spawn(async move {
        let (mut wtx, mut wrx) = ws.split();
        while let Some(Ok(msg)) = wrx.next().await {
            let tungstenite::Message::Text(text) = msg else {
                continue;
            };
            if let Ok(MasterMessage::AdoptionScanRequest { request_id }) =
                serde_json::from_str::<MasterMessage>(&text)
            {
                let reply = AgentMessage::AdoptionReport {
                    request_id,
                    data: AdoptionReportData {
                        node_id: 7,
                        hostname: "agent-1".into(),
                        containers: vec![ManagedContainer {
                            service_name: "orphan-svc".into(),
                            image: "nginx:1.27".into(),
                            status: "running".into(),
                            container_id: "deadbeef".into(),
                            port: Some(8080),
                            domain: Some("orphan.example.com".into()),
                            network: None,
                            routes: vec![],
                            strip_prefix: None,
                        }],
                    },
                };
                wtx.send(tungstenite::Message::Text(
                    serde_json::to_string(&reply).unwrap().into(),
                ))
                .await
                .unwrap();
            }
        }
    });

    // Let the master register the agent's ws sender.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Drive one cycle: fan-out → agent reply → adopt.
    orca_control::adoption::run_adoption_cycle(&state).await;

    let services = state.services.read().await;
    let svc = services
        .get("orphan-svc")
        .expect("orphan should be adopted into the registry");
    assert_eq!(svc.config.image.as_deref(), Some("nginx:1.27"));
    assert_eq!(svc.config.domain.as_deref(), Some("orphan.example.com"));
    assert_eq!(
        svc.config
            .placement
            .as_ref()
            .and_then(|p| p.node.as_deref()),
        Some("7"),
        "adopted service must be pinned to the reporting node"
    );
    assert_eq!(svc.instances.len(), 1);
    assert_eq!(svc.instances[0].handle.runtime_id, "remote-7");
    assert_eq!(svc.instances[0].status, WorkloadStatus::Running);

    agent.abort();
}

#[tokio::test]
async fn adoption_cycle_is_noop_with_no_agents() {
    // No connected agents → cycle must do nothing and not panic.
    let state = test_state();
    orca_control::adoption::run_adoption_cycle(&state).await;
    assert!(state.services.read().await.is_empty());
}
