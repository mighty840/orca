//! Master self-registration and the node-liveness prune loop.
//!
//! The master appears in `registered_nodes` with its REAL identity (#134):
//! hostname-based address + `hostname`/`role=master` labels, so placement
//! pins naming the master resolve to this entry and `find_target_node`
//! deploys locally. The heartbeat loop keeps the entry fresh and prunes
//! nodes that are both stale and have no live control session (#84/#85 —
//! sound since #131 made `ws_agents` membership imply liveness).

use std::collections::HashMap;
use std::sync::Arc;

use tracing::info;

use crate::state;

/// Compute a deterministic node ID from the system hostname.
pub(crate) fn master_node_id() -> u64 {
    use std::hash::{Hash, Hasher};
    let hostname = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "orca-master".to_string());
    let mut hasher = std::hash::DefaultHasher::new();
    hostname.hash(&mut hasher);
    hasher.finish()
}

/// Register the master node in the cluster node map.
///
/// The entry carries the master's REAL identity (#134): hostname-based
/// address and a `hostname` label, so placement pins naming the master
/// resolve to this entry — and `find_target_node` recognizes the
/// `role=master` label and deploys locally instead of dispatching the
/// deploy over a WS session the master doesn't have. The old
/// `localhost:{port}` address made a `localhost` pin resolve here and
/// try a remote dispatch to ourselves.
pub(crate) async fn register_master_node(state: &state::AppState, api_port: u16) {
    let node_id = master_node_id();
    let hostname = crate::placement::master_hostname();
    let mut labels = HashMap::new();
    labels.insert("role".to_string(), "master".to_string());
    labels.insert("hostname".to_string(), hostname.clone());
    let node = state::RegisteredNode {
        node_id,
        address: format!("{hostname}:{api_port}"),
        labels,
        peer_ip: None,
        last_heartbeat: chrono::Utc::now(),
        drain: false,
        cpu_percent: 0.0,
        memory_bytes: 0,
        memory_total: 0,
        disk_used: 0,
        disk_total: 0,
        net_rx: 0,
        net_tx: 0,
    };
    let mut nodes = state.registered_nodes.write().await;
    nodes.insert(node_id, node);
    info!(node_id, "Master node self-registered");
}

/// How long a node may go without a heartbeat before it is eligible for
/// pruning — but only if it also has no live WS connection (see
/// [`should_keep_node`]).
const STALE_AFTER: chrono::Duration = chrono::Duration::seconds(60);

/// Decide whether a node should remain in the cluster node map.
///
/// The master is always kept. A node with a live WS control channel
/// (`connected`) is always kept, regardless of heartbeat age: an open
/// connection means the agent is alive, and a single late heartbeat (lock
/// contention, load spike, jitter) must not orphan it — pruning a connected
/// node leaves the master and agent permanently desynced because the agent's
/// WS never drops and so never re-registers. Only a node that is both stale
/// *and* has no live connection is dropped.
fn should_keep_node(
    id: u64,
    master_id: u64,
    connected: &std::collections::HashSet<u64>,
    last_heartbeat: chrono::DateTime<chrono::Utc>,
    now: chrono::DateTime<chrono::Utc>,
) -> bool {
    if id == master_id || connected.contains(&id) {
        return true;
    }
    now - last_heartbeat < STALE_AFTER
}

/// Spawn a periodic task that samples host stats and writes them onto the
/// master node's entry in the cluster node map. Joined nodes push their own
/// stats via the heartbeat; the master has no heartbeat to piggyback on so
/// it does this in-process instead. The same loop also prunes zombie
/// nodes — entries that are both stale (no heartbeat for >60s) and have no
/// live WS connection — which keeps the cluster/info endpoint clean after a
/// joined node disconnects for good.
pub(crate) fn spawn_master_heartbeat(state: Arc<state::AppState>) {
    let node_id = master_node_id();
    let collector = Arc::new(orca_agent::host_stats::HostStatsCollector::new());
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            let sample = collector.sample();
            let now = chrono::Utc::now();

            // Snapshot the nodes that still hold a live WS control channel. A node
            // with an open connection is alive even if a single heartbeat was merely
            // delayed (lock contention, load spike, or network jitter that stalls but
            // does not drop the socket). Pruning such a node orphans the agent: its WS
            // stays up so it never re-registers, and the only recovery is a full
            // restart of every node. Read this lock first and drop it before taking
            // the registered_nodes write lock to avoid holding both at once.
            let connected: std::collections::HashSet<u64> =
                state.ws_agents.read().await.keys().copied().collect();

            let mut nodes = state.registered_nodes.write().await;
            if let Some(node) = nodes.get_mut(&node_id) {
                node.last_heartbeat = now;
                node.cpu_percent = sample.cpu_percent;
                node.memory_bytes = sample.memory_bytes;
                node.memory_total = sample.memory_total;
                node.disk_used = sample.disk_used;
                node.disk_total = sample.disk_total;
                node.net_rx = sample.net_rx;
                node.net_tx = sample.net_tx;
            }
            nodes.retain(|id, node| {
                let keep = should_keep_node(*id, node_id, &connected, node.last_heartbeat, now);
                if !keep {
                    tracing::warn!(
                        node_id = *id,
                        age_secs = (now - node.last_heartbeat).num_seconds(),
                        "Pruning stale node: no heartbeat for >60s and no live WS connection"
                    );
                }
                keep
            });
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    const MASTER: u64 = 1;
    const AGENT: u64 = 2;

    fn ago(now: chrono::DateTime<chrono::Utc>, secs: i64) -> chrono::DateTime<chrono::Utc> {
        now - chrono::Duration::seconds(secs)
    }

    #[test]
    fn master_is_never_pruned_even_when_stale() {
        let now = chrono::Utc::now();
        let connected = HashSet::new();
        assert!(should_keep_node(
            MASTER,
            MASTER,
            &connected,
            ago(now, 600),
            now
        ));
    }

    #[test]
    fn fresh_agent_is_kept() {
        let now = chrono::Utc::now();
        let connected = HashSet::new();
        assert!(should_keep_node(
            AGENT,
            MASTER,
            &connected,
            ago(now, 5),
            now
        ));
    }

    #[test]
    fn stale_agent_without_connection_is_pruned() {
        let now = chrono::Utc::now();
        let connected = HashSet::new();
        assert!(!should_keep_node(
            AGENT,
            MASTER,
            &connected,
            ago(now, 120),
            now
        ));
    }

    #[test]
    fn stale_agent_with_live_ws_is_kept() {
        // Regression: a delayed heartbeat on a still-connected agent must not
        // orphan it. A connected-but-stale node was the cause of the
        // master-forgets-agent desync that required restarting every node.
        let now = chrono::Utc::now();
        let connected = HashSet::from([AGENT]);
        assert!(should_keep_node(
            AGENT,
            MASTER,
            &connected,
            ago(now, 600),
            now
        ));
    }
}
