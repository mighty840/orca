//! Stopping a service on the agent that runs it (#309).
//!
//! `orca stop` scales a service to 0. For a service on an agent the master
//! used to skip that as "same spec", mark the service paused and drop its
//! routes, while the agent's containers kept running and writing. An
//! operator trusting `orca status` then took a "cold" backup of live
//! databases.

use std::time::Duration;

use orca_core::config::ServiceConfig;
use orca_core::ws_types::MasterMessage;
use tracing::{info, warn};

use crate::reconciler::ReconcileOutcome;
use crate::state::{AppState, ServiceState};

/// How long to wait for the agent to confirm. Its stop waits behind any
/// deploy of the same service, and gives each container 10s to exit.
const CONFIRM_TIMEOUT: Duration = Duration::from_secs(60);

/// Stop `config`'s containers on `node_id` and wait until the agent says
/// they are down. An offline agent gets the stop when it reconnects.
pub(crate) async fn stop_on_agent(
    state: &AppState,
    config: &ServiceConfig,
    node_id: u64,
) -> anyhow::Result<ReconcileOutcome> {
    let name = &config.name;
    let tx = state.ws_agents.read().await.get(&node_id).cloned();
    match tx {
        Some(tx) => {
            let (done_tx, done_rx) = tokio::sync::oneshot::channel();
            state
                .pending_stops
                .write()
                .await
                .insert(name.clone(), done_tx);
            let sent = tx
                .send(MasterMessage::Stop {
                    service_name: name.clone(),
                })
                .await;
            if let Err(e) = sent {
                state.pending_stops.write().await.remove(name);
                anyhow::bail!("agent {node_id} channel closed: {e}");
            }
            match tokio::time::timeout(CONFIRM_TIMEOUT, done_rx).await {
                Ok(Ok(Ok(()))) => info!("Stopped {name} on node {node_id}"),
                Ok(Ok(Err(e))) => anyhow::bail!("stopping {name} on node {node_id} failed: {e}"),
                // Dropped or timed out. An agent older than v0.3.2 runs the
                // stop but never confirms it.
                Ok(Err(_)) | Err(_) => {
                    state.pending_stops.write().await.remove(name);
                    warn!(
                        "node {node_id} did not confirm the stop of {name} within {}s; \
                         an agent older than v0.3.2 never does. Check it with `docker ps` there",
                        CONFIRM_TIMEOUT.as_secs()
                    );
                }
            }
        }
        None => {
            // Replayed by `drain_pending_commands` when the node reconnects;
            // its re-sync leaves paused services out, so nothing starts it again.
            state
                .pending_commands
                .write()
                .await
                .entry(node_id)
                .or_default()
                .push(serde_json::json!({ "action": "stop", "service_name": name }));
            warn!("node {node_id} is offline: {name} will be stopped there when it reconnects");
        }
    }

    // Recorded only now: a stop that failed leaves the service as it was.
    // No placeholder left: `orca start` then deploys again instead of
    // finding a "running" placeholder with the same spec.
    let mut services = state.services.write().await;
    let svc = services
        .entry(name.clone())
        .or_insert_with(|| ServiceState::from_config(config.clone()));
    svc.config = config.clone();
    svc.desired_replicas = 0;
    svc.instances.clear();
    Ok(ReconcileOutcome::Changed)
}

#[cfg(test)]
#[path = "remote_stop_tests.rs"]
mod tests;
