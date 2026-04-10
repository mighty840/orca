//! Integration test for the WebSocket agent↔master channel.
//!
//! Spins up a real axum server with the WS endpoint, connects a
//! tokio-tungstenite client, and verifies:
//! - Auth rejection with bad token
//! - Successful upgrade with valid token
//! - Master sends Ack on connect
//! - Agent heartbeat is received and updates node stats
//! - Master pushes deploy commands via ws_agents channel
//! - Domain discovery messages from agent are processed

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::sync::RwLock;
use tokio_tungstenite::tungstenite;

use orca_core::config::ClusterConfig;
use orca_core::ws_types::{AgentMessage, HostStats, MasterMessage};

/// Create a minimal AppState for testing.
fn test_state() -> Arc<orca_control::state::AppState> {
    let container_runtime = Arc::new(orca_core::testing::MockRuntime::new());
    let route_table = Arc::new(RwLock::new(HashMap::new()));
    let wasm_triggers = Arc::new(RwLock::new(Vec::new()));
    let mut cluster_config = ClusterConfig::default();
    cluster_config.api_tokens = vec!["test-token-123".to_string()];

    let state = orca_control::state::AppState::new(
        cluster_config,
        container_runtime,
        None,
        route_table,
        wasm_triggers,
    );
    Arc::new(state)
}

/// Build a minimal router with just the WS endpoint.
fn test_router(state: Arc<orca_control::state::AppState>) -> axum::Router {
    use axum::routing::get;
    axum::Router::new()
        .route(
            "/api/v1/ws/agent",
            get(orca_control::ws_handler::ws_agent_handler),
        )
        .with_state(state)
}

/// Start a test server and return its address.
async fn start_server(state: Arc<orca_control::state::AppState>) -> std::net::SocketAddr {
    let app = test_router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    // Give server a moment to start
    tokio::time::sleep(Duration::from_millis(50)).await;
    addr
}

#[tokio::test]
async fn ws_rejects_bad_token() {
    let state = test_state();
    let addr = start_server(state).await;

    let url = format!("ws://{addr}/api/v1/ws/agent?token=wrong&node_id=1");
    let result = tokio_tungstenite::connect_async(&url).await;

    // Should fail — server returns 401 before upgrade
    assert!(result.is_err(), "should reject invalid token");
}

#[tokio::test]
async fn ws_accepts_valid_token_and_sends_ack() {
    let state = test_state();
    let addr = start_server(state).await;

    let url = format!("ws://{addr}/api/v1/ws/agent?token=test-token-123&node_id=42");
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();

    // Should receive Ack as first message
    let msg = tokio::time::timeout(Duration::from_secs(2), ws.next())
        .await
        .expect("timeout waiting for ack")
        .expect("stream ended")
        .expect("ws error");

    let text = msg.into_text().unwrap();
    let parsed: MasterMessage = serde_json::from_str(&text).unwrap();
    assert!(
        matches!(parsed, MasterMessage::Ack { node_id: 42 }),
        "expected Ack, got {parsed:?}"
    );

    ws.close(None).await.ok();
}

#[tokio::test]
async fn ws_heartbeat_updates_node_stats() {
    let state = test_state();

    // Pre-register the node so heartbeat has something to update
    {
        let mut nodes = state.registered_nodes.write().await;
        nodes.insert(
            42,
            orca_control::state::RegisteredNode {
                node_id: 42,
                address: "test:6881".into(),
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
            },
        );
    }

    let addr = start_server(state.clone()).await;
    let url = format!("ws://{addr}/api/v1/ws/agent?token=test-token-123&node_id=42");
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();

    // Consume the Ack
    let _ = ws.next().await;

    // Send a heartbeat
    let heartbeat = AgentMessage::Heartbeat {
        node_id: 42,
        workloads: vec![],
        stats: HostStats {
            cpu_percent: 55.5,
            memory_bytes: 8_000_000,
            memory_total: 16_000_000,
            disk_used: 100_000,
            disk_total: 500_000,
            net_rx: 1000,
            net_tx: 2000,
            domains: vec!["test.example.com".into()],
        },
    };
    let json = serde_json::to_string(&heartbeat).unwrap();
    ws.send(tungstenite::Message::Text(json.into()))
        .await
        .unwrap();

    // Give the server time to process
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Verify stats were updated
    let nodes = state.registered_nodes.read().await;
    let node = nodes.get(&42).unwrap();
    assert!((node.cpu_percent - 55.5).abs() < 0.1);
    assert_eq!(node.memory_bytes, 8_000_000);
    assert_eq!(node.memory_total, 16_000_000);

    ws.close(None).await.ok();
}

#[tokio::test]
async fn ws_master_pushes_deploy_via_channel() {
    let state = test_state();
    let addr = start_server(state.clone()).await;

    let url = format!("ws://{addr}/api/v1/ws/agent?token=test-token-123&node_id=99");
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();

    // Consume the Ack
    let _ = ws.next().await;

    // Give the server time to register the ws_agent sender
    tokio::time::sleep(Duration::from_millis(50)).await;

    // Push a deploy command via the ws_agents channel
    let spec = orca_core::types::WorkloadSpec {
        name: "test-svc".into(),
        runtime: orca_core::types::RuntimeKind::Container,
        image: "nginx:latest".into(),
        replicas: orca_core::types::Replicas::Fixed(1),
        port: Some(80),
        host_port: None,
        domain: Some("test.example.com".into()),
        routes: vec![],
        health: None,
        readiness: None,
        liveness: None,
        env: HashMap::new(),
        resources: None,
        volume: None,
        deploy: None,
        placement: None,
        network: None,
        aliases: vec![],
        mounts: vec![],
        triggers: vec![],
        build: None,
        tls_cert: None,
        tls_key: None,
        internal: false,
        cmd: vec![],
        extra_ports: vec![],
        strip_prefix: None,
        pull_policy: Default::default(),
    };

    {
        let agents = state.ws_agents.read().await;
        let tx = agents.get(&99).expect("agent sender should be registered");
        tx.send(MasterMessage::Deploy {
            spec: Box::new(spec),
        })
        .await
        .unwrap();
    }

    // Agent should receive the deploy message
    let msg = tokio::time::timeout(Duration::from_secs(2), ws.next())
        .await
        .expect("timeout waiting for deploy")
        .expect("stream ended")
        .expect("ws error");

    let text = msg.into_text().unwrap();
    let parsed: MasterMessage = serde_json::from_str(&text).unwrap();
    assert!(
        matches!(parsed, MasterMessage::Deploy { .. }),
        "expected Deploy, got {parsed:?}"
    );

    ws.close(None).await.ok();
}

#[tokio::test]
async fn ws_domain_discovered_updates_service_config() {
    let state = test_state();

    // Add a service entry
    {
        let mut services = state.services.write().await;
        let config = orca_core::config::ServiceConfig {
            name: "dashboard".into(),
            project: None,
            runtime: Default::default(),
            image: Some("myapp:latest".into()),
            module: None,
            replicas: Default::default(),
            port: Some(8000),
            host_port: None,
            domain: None, // no domain yet
            routes: vec![],
            health: None,
            readiness: None,
            liveness: None,
            env: HashMap::new(),
            resources: None,
            volume: None,
            deploy: None,
            placement: None,
            network: None,
            aliases: vec![],
            mounts: vec![],
            triggers: vec![],
            assets: None,
            build: None,
            tls_cert: None,
            tls_key: None,
            internal: false,
            depends_on: vec![],
            cmd: vec![],
            extra_ports: vec![],
            strip_prefix: None,
            pull_policy: Default::default(),
        };
        services.insert(
            "dashboard".into(),
            orca_control::state::ServiceState::from_config(config),
        );
    }

    let addr = start_server(state.clone()).await;
    let url = format!("ws://{addr}/api/v1/ws/agent?token=test-token-123&node_id=7");
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();

    // Consume Ack
    let _ = ws.next().await;

    // Send domain discovery
    let msg = AgentMessage::DomainDiscovered {
        service_name: "dashboard".into(),
        domain: "yt.example.com".into(),
        host_port: 35000,
    };
    let json = serde_json::to_string(&msg).unwrap();
    ws.send(tungstenite::Message::Text(json.into()))
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_millis(100)).await;

    // Verify domain was set on the service config
    let services = state.services.read().await;
    let svc = services.get("dashboard").unwrap();
    assert_eq!(svc.config.domain.as_deref(), Some("yt.example.com"));

    ws.close(None).await.ok();
}

#[tokio::test]
async fn ws_cleanup_on_disconnect() {
    let state = test_state();
    let addr = start_server(state.clone()).await;

    let url = format!("ws://{addr}/api/v1/ws/agent?token=test-token-123&node_id=55");
    let (ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();

    tokio::time::sleep(Duration::from_millis(50)).await;

    // Verify sender is registered
    {
        let agents = state.ws_agents.read().await;
        assert!(agents.contains_key(&55), "agent should be registered");
    }

    // Close connection
    drop(ws);
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Verify sender is removed
    {
        let agents = state.ws_agents.read().await;
        assert!(
            !agents.contains_key(&55),
            "agent sender should be cleaned up after disconnect"
        );
    }
}

#[tokio::test]
async fn ws_sends_reconcile_on_connect() {
    let state = test_state();

    // Register node 77
    {
        let mut nodes = state.registered_nodes.write().await;
        nodes.insert(
            77,
            orca_control::state::RegisteredNode {
                node_id: 77,
                address: "contabo-host:6881".into(),
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
            },
        );
    }

    // Add a service placed on this node
    {
        let mut services = state.services.write().await;
        let config = orca_core::config::ServiceConfig {
            name: "remote-web".into(),
            project: None,
            runtime: Default::default(),
            image: Some("nginx:latest".into()),
            module: None,
            replicas: Default::default(),
            port: Some(80),
            host_port: None,
            domain: Some("web.example.com".into()),
            routes: vec![],
            health: None,
            readiness: None,
            liveness: None,
            env: HashMap::new(),
            resources: None,
            volume: None,
            deploy: None,
            placement: Some(orca_core::types::PlacementConstraint {
                node: Some("contabo-host".into()),
                labels: None,
                requires_gpu: None,
            }),
            network: None,
            aliases: vec![],
            mounts: vec![],
            triggers: vec![],
            assets: None,
            build: None,
            tls_cert: None,
            tls_key: None,
            internal: false,
            depends_on: vec![],
            cmd: vec![],
            extra_ports: vec![],
            strip_prefix: None,
            pull_policy: Default::default(),
        };
        services.insert(
            "remote-web".into(),
            orca_control::state::ServiceState::from_config(config),
        );
    }

    let addr = start_server(state).await;
    let url = format!("ws://{addr}/api/v1/ws/agent?token=test-token-123&node_id=77");
    let (mut ws, _) = tokio_tungstenite::connect_async(&url).await.unwrap();

    // First message: Ack
    let ack_msg = tokio::time::timeout(Duration::from_secs(2), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let ack: MasterMessage = serde_json::from_str(&ack_msg.into_text().unwrap()).unwrap();
    assert!(matches!(ack, MasterMessage::Ack { .. }));

    // Second message should be Reconcile with expected services
    let reconcile_msg = tokio::time::timeout(Duration::from_secs(2), ws.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let reconcile: MasterMessage =
        serde_json::from_str(&reconcile_msg.into_text().unwrap()).unwrap();

    match reconcile {
        MasterMessage::Reconcile { expected } => {
            assert_eq!(expected.len(), 1);
            assert_eq!(expected[0].name, "remote-web");
            assert_eq!(expected[0].domain.as_deref(), Some("web.example.com"));
        }
        other => panic!("expected Reconcile, got {other:?}"),
    }

    ws.close(None).await.ok();
}
