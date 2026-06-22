//! Deploy handling for the agent WS client.
//!
//! Run on a spawned task (not inline in the receive loop) so a long image
//! pull does not head-of-line-block other master→agent commands such as
//! `Stop` or subsequent `Deploy`s (#88).

use std::sync::Arc;

use tokio::sync::mpsc;
use tracing::{error, info};

use orca_core::runtime::{Runtime, WorkloadHandle};
use orca_core::types::WorkloadSpec;
use orca_core::ws_types::AgentMessage;

use crate::grpc::AgentClient;

/// Run a deploy spec and report the terminal result back to the master.
///
/// The agent has already sent `DeployReceived` by the time this runs, so the
/// master knows work is in flight. This function performs the (potentially
/// multi-minute) image pull + container create, notifies the proxy of any
/// discovered domain, and finally sends `DeployResult` carrying the real
/// success/error — never a bare timeout.
pub async fn deploy_and_report(
    runtime: Arc<dyn Runtime>,
    agent: Arc<AgentClient>,
    domain_tx: mpsc::Sender<(String, String, u16)>,
    out_tx: mpsc::Sender<AgentMessage>,
    spec: Box<WorkloadSpec>,
) {
    let result = agent.deploy_spec(runtime.as_ref(), &spec).await;
    let success = result.is_ok();
    let error = result.err().map(|e| e.to_string());

    // Notify the proxy of every domain for this service (each → same backend).
    let domains = spec.all_domains();
    if success
        && !domains.is_empty()
        && let Ok(Some(port)) = runtime
            .resolve_host_port(
                &WorkloadHandle {
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

    if success {
        info!("WS: deploy of {} succeeded", spec.name);
    } else {
        error!(
            "WS: deploy of {} failed: {}",
            spec.name,
            error.as_deref().unwrap_or("unknown")
        );
    }
    let _ = out_tx
        .send(AgentMessage::DeployResult {
            service_name: spec.name.clone(),
            success,
            error,
        })
        .await;
}
