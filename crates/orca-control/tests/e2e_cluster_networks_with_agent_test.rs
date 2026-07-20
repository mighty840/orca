//! E2E test: cluster networks dashboard merges responses from joined agents.
//!
//! Connects a [`fake_agent::FakeAgent`] to the master over a real WebSocket,
//! then calls `GET /api/v1/cluster/networks` and asserts the agent's
//! synthetic data appears alongside the master's own row.
//!
//! Regression coverage for the cluster fan-out path in
//! `crates/orca-control/src/api/handlers/ops/networks.rs::collect_agents`:
//! request_id correlation, listener map insertion/removal, mpsc drain, and
//! the `NETWORK_REPORT_TIMEOUT` deadline. Existing `e2e_cluster_networks_test`
//! only exercises the master branch (no agents joined) so this fills in the
//! other half.
//!
//! Run with: `cargo test -p orca-control --test e2e_cluster_networks_with_agent_test -- --ignored`

#[path = "fake_agent.rs"]
mod fake_agent;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;

use fake_agent::{FakeAgent, FakeAgentReplies};
use orca_control::state::AppState;
use orca_core::api_types::{ClusterNetworksResponse, DockerNetwork, NetworkService};
use orca_core::config::{ClusterConfig, ClusterMeta};

const TOKEN: &str = "fake-agent-test-token";
const AGENT_NODE_ID: u64 = 42;

async fn start_authed_server() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let runtime = Arc::new(
        orca_agent::docker::ContainerRuntime::new().expect("Docker must be running for E2E tests"),
    );
    let config = ClusterConfig {
        cluster: ClusterMeta {
            name: "e2e-net-agent".into(),
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

#[tokio::test]
#[ignore]
async fn e2e_cluster_networks_includes_agent_response() {
    let port = start_authed_server().await;

    let replies = FakeAgentReplies {
        hostname: "fake-agent-host".into(),
        snapshots: Vec::new(),
        networks: vec![DockerNetwork {
            name: "orca-fake-app".into(),
            services: vec![NetworkService {
                name: "orca-fake-svc".into(),
                aliases: vec!["fake-svc".into(), "app".into()],
                missing_aliases: Vec::new(),
            }],
        }],
    };
    let _agent = FakeAgent::connect(port, TOKEN, AGENT_NODE_ID, replies).await;

    // Master should now see one connected agent. Issue the fan-out RPC.
    let resp = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{port}/api/v1/cluster/networks"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "cluster_networks failed");
    let body: ClusterNetworksResponse = resp.json().await.unwrap();

    // Two rows: master (node_id None) + the fake agent (Some(42)).
    assert_eq!(
        body.nodes.len(),
        2,
        "expected master + 1 agent, got {body:?}"
    );
    let agent_row = body
        .nodes
        .iter()
        .find(|n| n.node_id == Some(AGENT_NODE_ID))
        .expect("fake agent row missing from response");
    assert!(agent_row.reachable, "fake agent should be marked reachable");
    assert_eq!(agent_row.hostname, "fake-agent-host");
    assert_eq!(agent_row.networks.len(), 1);
    let bridge = &agent_row.networks[0];
    assert_eq!(bridge.name, "orca-fake-app");
    assert_eq!(bridge.services.len(), 1);
    assert_eq!(bridge.services[0].name, "orca-fake-svc");
    assert_eq!(
        bridge.services[0].aliases,
        vec!["fake-svc".to_string(), "app".to_string()]
    );
}
