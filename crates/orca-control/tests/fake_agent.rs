//! Fake agent fixture for E2E tests of the master's cluster fan-out RPCs.
//!
//! Connects to the master's `/api/v1/ws/agent` endpoint over a real
//! WebSocket via tokio-tungstenite, then answers two `MasterMessage`
//! variants with synthetic data:
//!
//! - `BackupStatusRequest` → `AgentMessage::BackupStatusReport`
//! - `NetworkStatusRequest` → `AgentMessage::NetworkStatusReport`
//!
//! Every other inbound message (Ack, Reconcile, StatusPing, …) is silently
//! ignored — the fixture only emulates enough behavior to validate the
//! master's fan-out + listener-map + timeout pattern from #17 and #35.
//!
//! Used by `e2e_cluster_networks_with_agent_test.rs` and
//! `e2e_cluster_backups_with_agent_test.rs`.

use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpStream;
use tokio::sync::oneshot;
use tokio::task::JoinHandle;
use tokio_tungstenite::tungstenite::Message;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream};

use orca_core::api_types::DockerNetwork;
use orca_core::backup::BackupSnapshotSummary;
use orca_core::ws_types::{
    AgentMessage, BackupStatusReportData, MasterMessage, NetworkStatusReportData,
};

/// Synthetic data the fake agent replies with.
#[derive(Clone)]
pub struct FakeAgentReplies {
    pub hostname: String,
    pub snapshots: Vec<BackupSnapshotSummary>,
    pub networks: Vec<DockerNetwork>,
}

/// Handle for a running fake agent. Drop ends the connection; calling
/// [`FakeAgent::wait_connected`] blocks until the master has Ack'd so callers
/// can sequence the fan-out request after the agent appears in
/// `state.ws_agents`.
pub struct FakeAgent {
    handle: JoinHandle<()>,
}

impl FakeAgent {
    /// Connect a fake agent to the master at `127.0.0.1:<port>` and run the
    /// reply loop. The returned future resolves only once the master has
    /// sent its initial `Ack` — at that point the agent is registered in
    /// `state.ws_agents` and the test can issue cluster RPCs.
    pub async fn connect(port: u16, token: &str, node_id: u64, replies: FakeAgentReplies) -> Self {
        let url = format!(
            "ws://127.0.0.1:{port}/api/v1/ws/agent?token={token}&node_id={node_id}&address=127.0.0.1%3A{node_id}"
        );
        let (ws, _resp) = tokio_tungstenite::connect_async(&url)
            .await
            .expect("fake agent: ws connect");

        let (ack_tx, ack_rx) = oneshot::channel();
        let handle = tokio::spawn(run_loop(ws, node_id, replies, Some(ack_tx)));

        // Wait for Ack before returning so the master's ws_agents map is
        // guaranteed to include this node before the test issues a fan-out
        // RPC. Without this the master's `collect_agents` would see an
        // empty map and short-circuit.
        tokio::time::timeout(Duration::from_secs(3), ack_rx)
            .await
            .expect("fake agent: timed out waiting for master Ack")
            .expect("fake agent: ack channel dropped");

        FakeAgent { handle }
    }
}

impl Drop for FakeAgent {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

async fn run_loop(
    mut ws: WebSocketStream<MaybeTlsStream<TcpStream>>,
    node_id: u64,
    replies: FakeAgentReplies,
    mut ack_tx: Option<oneshot::Sender<()>>,
) {
    while let Some(Ok(msg)) = ws.next().await {
        let Message::Text(text) = msg else {
            continue;
        };
        let parsed: Result<MasterMessage, _> = serde_json::from_str(&text);
        let Ok(parsed) = parsed else {
            // Master sometimes sends frames whose variant we don't model
            // (e.g. new MasterMessage cases added after this fixture was
            // written). Ignore — the fixture is intentionally minimal.
            continue;
        };
        match parsed {
            MasterMessage::Ack { .. } => {
                if let Some(tx) = ack_tx.take() {
                    let _ = tx.send(());
                }
            }
            MasterMessage::BackupStatusRequest { request_id } => {
                let reply = AgentMessage::BackupStatusReport {
                    request_id,
                    data: BackupStatusReportData {
                        node_id,
                        hostname: replies.hostname.clone(),
                        snapshots: replies.snapshots.clone(),
                    },
                };
                if send(&mut ws, &reply).await.is_err() {
                    break;
                }
            }
            MasterMessage::NetworkStatusRequest { request_id } => {
                let reply = AgentMessage::NetworkStatusReport {
                    request_id,
                    data: NetworkStatusReportData {
                        node_id,
                        hostname: replies.hostname.clone(),
                        networks: replies.networks.clone(),
                    },
                };
                if send(&mut ws, &reply).await.is_err() {
                    break;
                }
            }
            _ => {
                // StatusPing, Reconcile, Deploy, Stop, exec — none of these
                // need a reply for the fan-out tests.
            }
        }
    }
}

async fn send(
    ws: &mut WebSocketStream<MaybeTlsStream<TcpStream>>,
    msg: &AgentMessage,
) -> Result<(), tokio_tungstenite::tungstenite::Error> {
    let text = serde_json::to_string(msg).expect("AgentMessage should always serialize");
    ws.send(Message::Text(text.into())).await
}
