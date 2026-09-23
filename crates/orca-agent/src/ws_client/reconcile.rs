//! Reconcile expected services on this agent node against running state.

use std::sync::Arc;

use tokio::sync::mpsc;
use tracing::{error, info, warn};

use orca_core::runtime::Runtime;
use orca_core::ws_types::AgentMessage;

use crate::grpc::AgentClient;

/// Reconcile: compare expected services from master against what's actually
/// running locally. Deploy missing services, and recreate running ones whose
/// spec changed while this agent was unreachable (#213). Leave the rest alone.
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
        let probe_handle = orca_core::runtime::WorkloadHandle {
            runtime_id: format!("orca-{}", spec.name),
            name: format!("orca-{}", spec.name),
            metadata: Default::default(),
        };
        let tracked = running_names.contains(&spec.name);
        // In-memory state is empty after an agent restart. Check Docker
        // directly so we don't force-remove a running container.
        let running = tracked
            || runtime
                .status(&probe_handle)
                .await
                .unwrap_or(orca_core::types::WorkloadStatus::Stopped)
                == orca_core::types::WorkloadStatus::Running;

        if running {
            if spec_drifted(runtime.as_ref(), &probe_handle, spec).await {
                info!(
                    "Reconcile: {} is running an outdated spec, recreating",
                    spec.name
                );
            } else {
                if !tracked {
                    agent
                        .update_workload_status(
                            &probe_handle.runtime_id,
                            &spec.name,
                            orca_core::types::WorkloadStatus::Running,
                        )
                        .await;
                    info!("Reconcile: {} already running, adopted", spec.name);
                }
                skipped += 1;
                continue;
            }
        } else {
            info!("Reconcile: deploying missing service {}", spec.name);
        }
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

/// Whether the running container was created from a different spec than the
/// master now expects. Only a definite mismatch counts: a spec from an older
/// master (no fingerprint) or a container created before fingerprinting (no
/// label) is left running and gets stamped at its next deploy, so upgrading
/// the agent does not recreate every container on the node.
async fn spec_drifted(
    runtime: &dyn Runtime,
    handle: &orca_core::runtime::WorkloadHandle,
    spec: &orca_core::types::WorkloadSpec,
) -> bool {
    let Some(want) = spec.fingerprint.as_deref() else {
        return false;
    };
    match runtime.spec_fingerprint(handle).await {
        Ok(Some(have)) => have != want,
        Ok(None) => false,
        Err(e) => {
            warn!(
                "Reconcile: cannot read the spec fingerprint of {}: {e}",
                spec.name
            );
            false
        }
    }
}

#[cfg(test)]
#[path = "reconcile_tests.rs"]
mod tests;
