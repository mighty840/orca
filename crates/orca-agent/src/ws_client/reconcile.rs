//! Reconcile expected services on this agent node against running state.

use std::sync::Arc;

use tokio::sync::mpsc;
use tracing::{error, info};

use orca_core::runtime::Runtime;
use orca_core::ws_types::AgentMessage;

use crate::grpc::AgentClient;

/// Reconcile: compare expected services from master against what's actually
/// running locally. Deploy any missing services, skip ones already running.
#[allow(clippy::vec_box)]
pub(super) async fn reconcile_services(
    expected: Vec<Box<orca_core::types::WorkloadSpec>>,
    runtime: &Arc<dyn Runtime>,
    agent: &Arc<AgentClient>,
    domain_tx: &mpsc::Sender<(String, String, u16)>,
    out_tx: &mpsc::Sender<AgentMessage>,
) {
    let running = agent.collect_workload_reports(runtime.as_ref()).await;
    // Only containers in "running" state count — exited/dead containers must be redeployed.
    let running_names: std::collections::HashSet<String> = running
        .iter()
        .filter(|r| r.status == "running")
        .map(|r| r.service_name.clone())
        .collect();

    let mut deployed = 0u32;
    let mut skipped = 0u32;

    for spec in &expected {
        if running_names.contains(&spec.name) {
            skipped += 1;
            continue;
        }

        // In-memory state is empty after an agent restart. Check Docker
        // directly so we don't force-remove a running container.
        let probe_handle = orca_core::runtime::WorkloadHandle {
            runtime_id: format!("orca-{}", spec.name),
            name: format!("orca-{}", spec.name),
            metadata: Default::default(),
        };
        if runtime
            .status(&probe_handle)
            .await
            .unwrap_or(orca_core::types::WorkloadStatus::Stopped)
            == orca_core::types::WorkloadStatus::Running
        {
            agent
                .update_workload_status(
                    &probe_handle.runtime_id,
                    &spec.name,
                    orca_core::types::WorkloadStatus::Running,
                )
                .await;
            skipped += 1;
            info!("Reconcile: {} already running, adopted", spec.name);
            continue;
        }

        info!("Reconcile: deploying missing service {}", spec.name);
        match agent.deploy_spec(runtime.as_ref(), spec).await {
            Ok(()) => {
                deployed += 1;
                let _ = out_tx
                    .send(AgentMessage::DeployResult {
                        service_name: spec.name.clone(),
                        success: true,
                        error: None,
                    })
                    .await;
                // Notify domain discovery for every domain (each → same backend).
                let domains = spec.all_domains();
                if !domains.is_empty()
                    && let Ok(Some(port)) = runtime
                        .resolve_host_port(
                            &orca_core::runtime::WorkloadHandle {
                                runtime_id: format!("orca-{}", spec.name),
                                name: format!("orca-{}", spec.name),
                                metadata: Default::default(),
                            },
                            spec.port.unwrap_or(80),
                        )
                        .await
                {
                    for domain in domains {
                        let _ = domain_tx.send((spec.name.clone(), domain, port)).await;
                    }
                }
            }
            Err(e) => {
                error!("Reconcile: failed to deploy {}: {e}", spec.name);
                let _ = out_tx
                    .send(AgentMessage::DeployResult {
                        service_name: spec.name.clone(),
                        success: false,
                        error: Some(e.to_string()),
                    })
                    .await;
            }
        }
    }

    info!("Reconcile complete: {deployed} deployed, {skipped} already running");
}
