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

use crate::state::{AppState, ServiceState};

/// How long to wait for the agent to confirm. Its stop waits behind any
/// deploy of the same service, and gives each container 10s to exit.
const CONFIRM_TIMEOUT: Duration = Duration::from_secs(60);

/// How a stop on an agent ended, short of an error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum StopOutcome {
    /// The agent said its containers are down.
    Confirmed,
    /// The node is offline; it gets the stop when it reconnects.
    Queued,
    /// The stop was sent but not confirmed in time.
    Unconfirmed,
}

/// The agent node a stop of `config` goes to: where its placeholder says it
/// runs, else the node its pin names, drained or not (a node drained before
/// a migration must still be stoppable; deploy targeting refuses drained
/// nodes). `None` when it runs on the master.
pub(crate) async fn stop_target(
    state: &AppState,
    config: &ServiceConfig,
) -> anyhow::Result<Option<u64>> {
    let placeholder = state.services.read().await.get(&config.name).and_then(|s| {
        s.instances
            .iter()
            .find_map(|i| i.handle.runtime_id.strip_prefix("remote-")?.parse().ok())
    });
    if placeholder.is_some() {
        return Ok(placeholder);
    }
    let Some(pin) = config.placement.as_ref().and_then(|p| p.node.as_deref()) else {
        return Ok(None);
    };
    if crate::placement::pin_matches_master(pin) {
        return Ok(None);
    }
    let nodes = state.registered_nodes.read().await;
    let matches: Vec<&crate::state::RegisteredNode> = nodes
        .values()
        .filter(|n| crate::placement::node_matches_pin(n, pin))
        .collect();
    match matches.as_slice() {
        [n] if n.labels.get("role").is_some_and(|r| r == "master") => Ok(None),
        [n] => Ok(Some(n.node_id)),
        [] => match pin.parse() {
            Ok(id) => Ok(Some(id)), // adoption pins by node id
            Err(_) => anyhow::bail!(
                "no registered node matches the pin {pin:?} of {} (offline for a while?), \
                 so the stop cannot be sent; stop it on that node by hand",
                config.name
            ),
        },
        _ => anyhow::bail!("the pin {pin:?} of {} matches several nodes", config.name),
    }
}

/// Stop `config`'s containers on `node_id` and wait until the agent says
/// they are down. An offline agent gets the stop when it reconnects.
pub(crate) async fn stop_on_agent(
    state: &AppState,
    config: &ServiceConfig,
    node_id: u64,
) -> anyhow::Result<StopOutcome> {
    let name = &config.name;
    let tx = state.ws_agents.read().await.get(&node_id).cloned();
    let outcome = match tx {
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
                Ok(Ok(Ok(()))) => {
                    info!("Stopped {name} on node {node_id}");
                    StopOutcome::Confirmed
                }
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
                    StopOutcome::Unconfirmed
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
            StopOutcome::Queued
        }
    };

    // Recorded only now: a stop that failed leaves the service as it was.
    // An unconfirmed one is recorded too: the stop was delivered, and
    // keeping a "running" placeholder would make `orca start` a no-op.
    // No placeholder left: `orca start` then deploys again instead of
    // finding a "running" placeholder with the same spec.
    let mut services = state.services.write().await;
    let svc = services
        .entry(name.clone())
        .or_insert_with(|| ServiceState::from_config(config.clone()));
    svc.config = config.clone();
    svc.desired_replicas = 0;
    svc.instances.clear();
    Ok(outcome)
}

#[cfg(test)]
#[path = "remote_stop_tests.rs"]
mod tests;
