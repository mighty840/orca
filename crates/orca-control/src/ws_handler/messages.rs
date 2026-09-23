//! Dispatch of messages an agent sends over its control session.
//!
//! Split from `mod.rs`, which owns the connection lifecycle (upgrade,
//! session registration, read loop); this owns what each frame means.

use tokio::sync::mpsc;
use tracing::{error, info, warn};

use orca_core::ws_types::{AgentMessage, MasterMessage};

use super::heartbeat::handle_ws_heartbeat;
use crate::state::AppState;

/// Process a single message from the agent.
pub(super) async fn handle_agent_message(
    state: &AppState,
    node_id: u64,
    text: &str,
    _tx: &mpsc::Sender<MasterMessage>,
) -> anyhow::Result<()> {
    let msg: AgentMessage = serde_json::from_str(text)?;

    match msg {
        AgentMessage::Heartbeat {
            node_id: reported_id,
            workloads,
            stats,
        } => {
            // Attribute to the node this session connected as, never to the
            // id the message claims: otherwise one session could overwrite
            // another node's workload state (#201).
            if reported_id != node_id {
                warn!(
                    session_node = node_id,
                    reported_node = reported_id,
                    "heartbeat claims a different node id; attributing it to the session's node"
                );
            }
            handle_ws_heartbeat(state, node_id, &workloads, &stats).await;
        }
        AgentMessage::DomainDiscovered {
            service_name,
            domain,
            host_port,
        } => {
            info!(
                "Node {node_id} discovered domain {domain} for {service_name} (port {host_port})"
            );
            // Update the service's domain tracking on the master so the
            // TUI and status API reflect the domain(s) correctly. Fold the
            // newly-discovered domain into the existing set, normalizing to
            // the single `domain` field for one and `domains` for many (the
            // two are mutually exclusive — see ServiceConfig::validate).
            let mut services = state.services.write().await;
            if let Some(svc) = services.get_mut(&service_name) {
                let mut all = svc.config.all_domains();
                // Only fold discovered domains into services with NO
                // declared domains (adopted/unmanaged workloads). Mutating
                // a declared service's domain set makes `spec_matches`
                // diverge from the on-disk config, so the declarative loop
                // saw a permanent "change" and redeployed it every pass —
                // the 60s-cadence churn behind #120.
                if !all.is_empty() {
                    return Ok(());
                }
                if !all.contains(&domain) {
                    all.push(domain);
                    if all.len() == 1 {
                        svc.config.domain = all.pop();
                        svc.config.domains = Vec::new();
                    } else {
                        svc.config.domain = None;
                        svc.config.domains = all;
                    }
                }
            }
        }
        AgentMessage::DeployReceived { service_name } => {
            info!("Node {node_id}: agent acknowledged deploy of {service_name}, work started");
            if let Some(tx) = state
                .pending_deploy_acks
                .write()
                .await
                .remove(&service_name)
            {
                let _ = tx.send(());
            }
        }
        AgentMessage::DeployResult {
            service_name,
            success,
            error,
        } => {
            if success {
                info!("Node {node_id}: deploy of {service_name} succeeded");
                let mut services = state.services.write().await;
                if let Some(svc) = services.get_mut(&service_name) {
                    let placeholder_id = format!("remote-{node_id}");
                    if let Some(inst) = svc
                        .instances
                        .iter_mut()
                        .find(|i| i.handle.runtime_id == placeholder_id)
                    {
                        inst.status = orca_core::types::WorkloadStatus::Running;
                    }
                }
            } else {
                error!(
                    "Node {node_id}: deploy of {service_name} failed: {}",
                    error.as_deref().unwrap_or("unknown")
                );
            }
            let result = if success {
                Ok(())
            } else {
                Err(error.unwrap_or_else(|| "deploy failed".to_string()))
            };
            if let Some(tx) = state.pending_deploys.write().await.remove(&service_name) {
                let _ = tx.send(result);
            }
        }
        AgentMessage::LogChunk {
            request_id,
            service_name: _,
            data,
            done,
        } => {
            // Forward to any pending log stream listener.
            let listeners = state.log_listeners.read().await;
            if let Some(listener_tx) = listeners.get(&request_id) {
                let _ = listener_tx.send((data, done)).await;
            }
        }
        AgentMessage::BackupResult {
            node_id,
            success,
            message,
        } => {
            if success {
                info!("Node {node_id}: backup complete — {message}");
            } else {
                error!("Node {node_id}: backup failed — {message}");
                // Name the node by its hostname, not its numeric id.
                let node = state
                    .registered_nodes
                    .read()
                    .await
                    .get(&node_id)
                    .map(|n| {
                        n.address
                            .split(':')
                            .next()
                            .unwrap_or(&n.address)
                            .to_string()
                    })
                    .unwrap_or_else(|| format!("node {node_id}"));
                crate::alerts::alert_backup_failure(state, &node, &message).await;
            }
            // Cache for the cluster-backups dashboard so it can surface the
            // last-known failure without rescanning logs.
            state.last_backup_results.write().await.insert(
                node_id,
                crate::state::LastBackupResult {
                    success,
                    message,
                    recorded_at: chrono::Utc::now(),
                },
            );
        }
        AgentMessage::BackupStatusReport { request_id, data } => {
            if let Some(tx) = state.backup_listeners.read().await.get(&request_id) {
                let _ = tx.send(data).await;
            }
        }
        AgentMessage::NetworkStatusReport { request_id, data } => {
            if let Some(tx) = state.network_listeners.read().await.get(&request_id) {
                let _ = tx.send(data).await;
            }
        }
        AgentMessage::AdoptionReport { request_id, data } => {
            if let Some(tx) = state.adoption_listeners.read().await.get(&request_id) {
                let _ = tx.send(data).await;
            }
        }
        AgentMessage::ExecOutput { session_id, data } => {
            use base64::Engine as _;
            let bytes = match base64::engine::general_purpose::STANDARD.decode(&data) {
                Ok(b) => b,
                Err(e) => {
                    tracing::warn!("exec: bad base64 output for session {session_id}: {e}");
                    return Ok(());
                }
            };
            let sessions = state.exec_sessions.read().await;
            if let Some(tx) = sessions.get(&session_id) {
                let _ = tx.send(bytes).await;
            }
        }
        AgentMessage::ExecDone {
            session_id,
            exit_code,
        } => {
            info!("Node {node_id}: exec session {session_id} done (exit {exit_code})");
            state.exec_sessions.write().await.remove(&session_id);
        }
    }

    Ok(())
}
