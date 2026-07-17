//! Agent control-session lifecycle (#131).
//!
//! A session wraps the master→agent sender with a generation id and a kill
//! switch; presence in `AppState.ws_agents` is the master's definition of
//! "reachable". The read-idle deadline in `ws_handler` guarantees entries
//! are live by construction — a session that stops producing traffic is
//! torn down, taking the node's reachability and placeholders with it.

use std::sync::Arc;

use crate::state::AppState;

/// One agent control session (#131). Wraps the master→agent sender with a
/// generation id and a kill switch so session lifecycle is explicit:
///
/// - `session_id` disambiguates reconnects — a superseded session's cleanup
///   must never remove its replacement's map entry or placeholders.
/// - `shutdown` wakes the session's read loop for immediate teardown
///   (superseded by a reconnect, or killed after a deploy-ACK timeout).
#[derive(Debug, Clone)]
pub struct AgentSession {
    pub tx: crate::ws_handler::AgentSender,
    pub session_id: u64,
    pub shutdown: Arc<tokio::sync::Notify>,
}

impl AgentSession {
    /// Send a message to the agent over this session's channel. Delegates
    /// to the inner sender so call sites treat a session like the sender
    /// they always held.
    pub async fn send(
        &self,
        msg: orca_core::ws_types::MasterMessage,
    ) -> Result<(), tokio::sync::mpsc::error::SendError<orca_core::ws_types::MasterMessage>> {
        self.tx.send(msg).await
    }

    pub fn new(tx: crate::ws_handler::AgentSender) -> Self {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        AgentSession {
            tx,
            session_id: COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            shutdown: Arc::new(tokio::sync::Notify::new()),
        }
    }
}

impl AppState {
    /// Clone the current control-session sender for a node, if connected.
    pub async fn agent_tx(&self, node_id: u64) -> Option<crate::ws_handler::AgentSender> {
        self.ws_agents
            .read()
            .await
            .get(&node_id)
            .map(|s| s.tx.clone())
    }

    /// Deregister a session's map entry iff it still owns it (#131).
    /// Returns whether the caller was the owner — a superseded session
    /// exiting late gets `false` and must not touch its replacement's
    /// placeholders.
    pub async fn deregister_agent_session(&self, node_id: u64, session_id: u64) -> bool {
        let mut senders = self.ws_agents.write().await;
        if senders
            .get(&node_id)
            .is_some_and(|s| s.session_id == session_id)
        {
            senders.remove(&node_id);
            true
        } else {
            false
        }
    }
}
