//! Heartbeat handling for the WS handler: update node and service status
//! from agent heartbeats.

use crate::state::AppState;

/// Process a heartbeat received over WebSocket (same logic as HTTP handler).
pub(super) async fn handle_ws_heartbeat(
    state: &AppState,
    node_id: u64,
    workloads: &[orca_core::ws_types::WorkloadReport],
    stats: &orca_core::ws_types::HostStats,
) {
    let mut nodes = state.registered_nodes.write().await;
    if let Some(node) = nodes.get_mut(&node_id) {
        node.last_heartbeat = chrono::Utc::now();
        node.cpu_percent = stats.cpu_percent;
        node.memory_bytes = stats.memory_bytes;
        node.memory_total = stats.memory_total;
        node.disk_used = stats.disk_used;
        node.disk_total = stats.disk_total;
        node.net_rx = stats.net_rx;
        node.net_tx = stats.net_tx;
    }
    drop(nodes);

    // Update service statuses + per-container stats
    if !workloads.is_empty() {
        let mut services = state.services.write().await;
        let mut stats_cache = state.container_stats.write().await;

        for report in workloads {
            if let Some(svc) = services.get_mut(&report.service_name) {
                let status = match report.status.as_str() {
                    "running" => orca_core::types::WorkloadStatus::Running,
                    "stopped" => orca_core::types::WorkloadStatus::Stopped,
                    "failed" => orca_core::types::WorkloadStatus::Failed,
                    _ => orca_core::types::WorkloadStatus::Stopped,
                };
                // Update only the placeholder instance for this node, or the single
                // instance if there is only one. Blanket-overwriting all instances
                // masks individual replica failures on multi-replica services.
                let placeholder_id = format!("remote-{node_id}");
                if let Some(inst) = svc
                    .instances
                    .iter_mut()
                    .find(|i| i.handle.runtime_id == placeholder_id)
                {
                    inst.status = status;
                } else if svc.instances.len() == 1 {
                    svc.instances[0].status = status;
                }
            }

            // Cache per-container stats from remote agents
            if report.memory_bytes > 0 || report.cpu_percent > 0.0 {
                stats_cache.insert(
                    report.service_name.clone(),
                    crate::stats::ContainerStats {
                        memory_usage: crate::stats::format_bytes(report.memory_bytes),
                        cpu_percent: report.cpu_percent,
                    },
                );
            }
        }
    }
}
