//! E2E test: cluster backups dashboard merges responses from joined agents.
//!
//! Connects a [`fake_agent::FakeAgent`] to the master over a real WebSocket,
//! then calls `GET /api/v1/cluster/backups` and asserts the agent's
//! synthetic snapshot list appears alongside the master's row.
//!
//! Regression coverage for `crates/orca-control/src/api/handlers/ops/backups.rs`
//! fan-out — request_id correlation, listener map insertion/removal, mpsc
//! drain, and the per-agent timeout. The existing `e2e_backup_test`
//! exercises only the local volume backup CLI flow; this fills in the
//! cluster-aggregation half.
//!
//! Run with: `cargo test -p orca-control --test e2e_cluster_backups_with_agent_test -- --ignored`

#[path = "fake_agent.rs"]
mod fake_agent;

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;

use fake_agent::{FakeAgent, FakeAgentReplies};
use orca_control::state::AppState;
use orca_core::api_types::{ClusterBackupsResponse, NodeRole};
use orca_core::backup::{BackupFileEntry, BackupSnapshotSummary};
use orca_core::config::{ClusterConfig, ClusterMeta};

const TOKEN: &str = "fake-agent-backup-token";
const AGENT_NODE_ID: u64 = 77;

async fn start_authed_server() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let runtime = Arc::new(
        orca_agent::docker::ContainerRuntime::new().expect("Docker must be running for E2E tests"),
    );
    let config = ClusterConfig {
        cluster: ClusterMeta {
            name: "e2e-backup-agent".into(),
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
async fn e2e_cluster_backups_includes_agent_response() {
    let port = start_authed_server().await;

    let snapshot = BackupSnapshotSummary {
        epoch_secs: 1_700_000_000,
        total_size_bytes: 1024,
        files: vec![BackupFileEntry {
            name: "fake-vol.tar.gz".into(),
            size_bytes: 1024,
        }],
    };
    let replies = FakeAgentReplies {
        hostname: "fake-backup-host".into(),
        snapshots: vec![snapshot.clone()],
        networks: Vec::new(),
    };
    let _agent = FakeAgent::connect(port, TOKEN, AGENT_NODE_ID, replies).await;

    let resp = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{port}/api/v1/cluster/backups"))
        .bearer_auth(TOKEN)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200, "cluster_backups failed");
    let body: ClusterBackupsResponse = resp.json().await.unwrap();

    // Two rows: master (node_id None) + the fake agent.
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
    assert_eq!(agent_row.hostname, "fake-backup-host");
    assert_eq!(agent_row.role, NodeRole::Agent);
    assert_eq!(agent_row.snapshots.len(), 1);
    assert_eq!(agent_row.snapshots[0], snapshot);
}
