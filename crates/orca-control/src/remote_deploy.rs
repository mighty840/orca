//! Dispatching a deploy to an agent over its WebSocket session and waiting
//! for its receipt ACK and completion.

use std::time::Duration;

use tracing::info;

use orca_core::types::WorkloadSpec;

use crate::state::AppState;

/// Send a deploy command to a remote agent node and await the result.
///
/// The wait is split into two phases so a slow image pull is never reported as
/// an unreachable agent (#88/#94):
///
/// 1. **Receipt ACK** (`deploy.ack_timeout_secs`, default 10s): the agent
///    confirms it received the command and started work. A miss here means the
///    WS session is dead / the agent is unreachable — fail fast with a clear
///    message.
/// 2. **Completion** (`deploy.completion_timeout_secs`, default 600s): the
///    agent reports the deploy finished. Long enough to cover multi-GB
///    first-time pulls. On a real failure the agent's pull error surfaces
///    verbatim; on timeout the message says the pull may still be running.
///
/// Returns an error if the agent is not connected via WebSocket, the WS channel
/// is closed, either phase times out, or the agent reports a deploy failure.
pub(crate) async fn queue_remote_deploy(
    state: &AppState,
    node_id: u64,
    spec: &WorkloadSpec,
) -> anyhow::Result<()> {
    let tx = {
        let agents = state.ws_agents.read().await;
        agents
            .get(&node_id)
            .ok_or_else(|| anyhow::anyhow!("agent {node_id} not connected via WebSocket"))?
            .clone()
    };

    let (ack_tx, ack_rx) = tokio::sync::oneshot::channel::<()>();
    let (result_tx, result_rx) = tokio::sync::oneshot::channel::<Result<(), String>>();
    state
        .pending_deploy_acks
        .write()
        .await
        .insert(spec.name.clone(), ack_tx);
    state
        .pending_deploys
        .write()
        .await
        .insert(spec.name.clone(), result_tx);

    if let Err(e) = tx
        .send(orca_core::ws_types::MasterMessage::Deploy {
            spec: Box::new(spec.clone()),
        })
        .await
    {
        clear_pending_deploy(state, &spec.name).await;
        return Err(anyhow::anyhow!("agent {node_id} channel closed: {e}"));
    }

    info!("Sent deploy via WS to node {node_id}: {}", spec.name);

    let ack_timeout = Duration::from_secs(state.cluster_config.deploy.ack_timeout_secs);
    let completion_timeout =
        Duration::from_secs(state.cluster_config.deploy.completion_timeout_secs);

    // Phase 1: receipt ACK — distinguishes a dead/unreachable agent from a
    // slow pull. This is the failure mode that used to masquerade as the
    // misleading "is `orca server` running?" timeout.
    match tokio::time::timeout(ack_timeout, ack_rx).await {
        Ok(Ok(())) => {}
        Ok(Err(_)) => {
            clear_pending_deploy(state, &spec.name).await;
            anyhow::bail!("deploy receipt channel dropped for agent {node_id}");
        }
        Err(_) => {
            clear_pending_deploy(state, &spec.name).await;
            // A missed receipt-ACK is proof the control session is dead
            // (#131): the agent ACKs instantly when the channel works. Tear
            // the session down so the node stops looking reachable and
            // subsequent deploys fail fast with the real story — instead of
            // every deploy re-timing-out against the same zombie tx for
            // days. The agent's own idle deadline triggers its reconnect;
            // a truly-gone node is stale-pruned 60s later.
            crate::ws_handler::kill_agent_session(state, node_id, "deploy ACK timeout").await;
            anyhow::bail!(
                "agent {node_id} did not acknowledge deploy of {} within {}s — control                  session closed; node is unreachable until it rejoins",
                spec.name,
                ack_timeout.as_secs()
            );
        }
    }

    // Phase 2: completion — long enough for multi-GB first-time pulls. The
    // agent's real pull error surfaces here instead of a bare timeout.
    match tokio::time::timeout(completion_timeout, result_rx).await {
        Ok(Ok(Ok(()))) => Ok(()),
        Ok(Ok(Err(msg))) => anyhow::bail!("deploy failed on agent {node_id}: {msg}"),
        Ok(Err(_)) => anyhow::bail!("deploy result channel dropped for agent {node_id}"),
        Err(_) => {
            state.pending_deploys.write().await.remove(&spec.name);
            anyhow::bail!(
                "deploy of {} did not complete within {}s on agent {node_id} (image pull may still be running — raise deploy.completion_timeout_secs if the image is large)",
                spec.name,
                completion_timeout.as_secs()
            )
        }
    }
}

/// Remove both pending-deploy waiter entries for a service. Used on send
/// failure and on receipt-ACK timeout so neither map leaks an orphan entry.
async fn clear_pending_deploy(state: &AppState, name: &str) {
    state.pending_deploy_acks.write().await.remove(name);
    state.pending_deploys.write().await.remove(name);
}
