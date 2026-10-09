//! Restart containers whose own health check (Docker `HEALTHCHECK`) fails
//! (#294).
//!
//! The master's health checker skips agent workloads, and Docker's restart
//! policy only acts when a container exits. A container whose main process
//! died while another one keeps it running (clamd OOM-killed, freshclam
//! alive) stayed up and unhealthy until someone noticed. Docker only reports
//! unhealthy after `retries` failed checks, and a restart resets the result
//! to starting for the start period, so the check paces the restarts.

use std::time::Duration;

use tracing::warn;

use orca_core::runtime::{ContainerHealth, Runtime, WorkloadHandle};

use super::service_queue::{ServiceOp, ServiceQueues};
use crate::grpc::AgentClient;

/// How often the agent looks at its containers' health checks.
pub(super) const INTERVAL: Duration = Duration::from_secs(30);

/// Queue a restart for every workload whose health check fails. The restart
/// waits behind any deploy or stop of the same service.
pub(super) async fn queue_unhealthy(
    runtime: &dyn Runtime,
    agent: &AgentClient,
    ops: &ServiceQueues,
) {
    for (id, service) in agent.workload_ids().await {
        if unhealthy(runtime, &handle(&id, &service)).await {
            ops.submit(&service, ServiceOp::RestartIfUnhealthy(id));
        }
    }
}

/// Restart the container if its health check still fails: a deploy queued
/// before may have replaced it, or an earlier restart may have fixed it.
/// Stop and start keep its id, so the agent's and master's handles stay valid.
pub(super) async fn restart_if_unhealthy(runtime: &dyn Runtime, service: &str, id: &str) {
    let handle = handle(id, service);
    if !unhealthy(runtime, &handle).await {
        return;
    }
    warn!(
        service,
        container = id,
        "the container's health check fails while it runs — restarting it"
    );
    if let Err(e) = runtime.stop(&handle, Duration::from_secs(10)).await {
        warn!(service, "failed to stop the unhealthy container: {e}");
    }
    if let Err(e) = runtime.start(&handle).await {
        warn!(
            service,
            "failed to start the unhealthy container again: {e}"
        );
    }
}

async fn unhealthy(runtime: &dyn Runtime, handle: &WorkloadHandle) -> bool {
    matches!(
        runtime.health(handle).await,
        Ok(Some(ContainerHealth::Unhealthy))
    )
}

fn handle(id: &str, service: &str) -> WorkloadHandle {
    WorkloadHandle {
        runtime_id: id.to_string(),
        name: format!("orca-{service}"),
        metadata: Default::default(),
    }
}

#[cfg(test)]
#[path = "health_watch_tests.rs"]
mod tests;
