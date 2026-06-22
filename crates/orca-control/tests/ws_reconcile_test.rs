//! Integration test for WebSocket reconciliation on agent reconnect.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt;
use tokio::sync::RwLock;

use orca_core::config::ClusterConfig;
use orca_core::ws_types::MasterMessage;

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

async fn start_server(state: Arc<orca_control::state::AppState>) -> std::net::SocketAddr {
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
        axum::serve(listener, app).await.unwrap();
    });
    tokio::time::sleep(Duration::from_millis(50)).await;
    addr
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
            domains: vec![],
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
            backup: None,
        };
        services.insert(
            "remote-web".into(),
            orca_control::state::ServiceState::from_config(config),
        );
    }

    let addr = start_server(state).await;
    let url = format!(
        "ws://{addr}/api/v1/ws/agent?token=test-token-123&node_id=77&address=contabo-host%3A6881"
    );
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
