//! Wait for a just-started container to be ready before the agent reports
//! the deploy as done (#300).
//!
//! The master restarts a service's dependents once its redeploy returns. It
//! used to return while the new container was still starting, so an app that
//! exits on its first failed connect (a Go service and its postgres) stayed
//! down, and the SigNoz collector came up against a ClickHouse that was still
//! loading and never wrote anything.
//!
//! Ready means:
//! - the container's own health check (Docker `HEALTHCHECK`) passes, or
//! - without one, its port accepts TCP connections on its address on the
//!   service network. Not on the published host port: docker-proxy accepts
//!   there before the container listens. Postgres and ClickHouse open their
//!   ports only once they can serve.
//!
//! Without either signal, or after [`READY_BUDGET`], the deploy is reported
//! as before; readiness never fails a deploy.

use std::time::Duration;

use orca_core::runtime::{ContainerHealth, Runtime, WorkloadHandle};
use orca_core::types::{WorkloadSpec, WorkloadStatus};
use tokio::time::Instant;
use tracing::{debug, warn};

/// Longest wait for a container to become ready.
pub(crate) const READY_BUDGET: Duration = Duration::from_secs(120);
const POLL: Duration = Duration::from_millis(500);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(1);

/// How a wait for readiness ended.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Readiness {
    /// Health check passed, or the port accepted a connection.
    Ready,
    /// The health check failed: running, but not going to get ready.
    Unhealthy,
    /// The container stopped while we waited.
    NotRunning,
    /// No health check and no address to probe: nothing to wait for.
    NoSignal,
    /// Still not ready after the budget.
    TimedOut,
}

/// Wait until the container `handle` of `spec` is ready, for at most
/// [`READY_BUDGET`].
pub(crate) async fn wait_until_ready(
    runtime: &dyn Runtime,
    handle: &WorkloadHandle,
    spec: &WorkloadSpec,
) -> Readiness {
    let address = match spec.port {
        Some(port) => runtime
            .resolve_container_address(handle, port, &crate::docker::network_name(spec))
            .await
            .ok()
            .flatten(),
        None => None,
    };
    let started = Instant::now();
    let result = wait_for(runtime, handle, address.as_deref(), READY_BUDGET).await;
    match result {
        Readiness::Ready => debug!(
            "{} ready after {}ms",
            spec.name,
            started.elapsed().as_millis()
        ),
        Readiness::NoSignal => {}
        Readiness::Unhealthy | Readiness::NotRunning | Readiness::TimedOut => warn!(
            service = %spec.name,
            outcome = ?result,
            waited_secs = started.elapsed().as_secs(),
            "container not ready; reporting the deploy anyway"
        ),
    }
    result
}

/// [`wait_until_ready`] with the address to probe and the budget given.
pub(crate) async fn wait_for(
    runtime: &dyn Runtime,
    handle: &WorkloadHandle,
    address: Option<&str>,
    budget: Duration,
) -> Readiness {
    let started = Instant::now();
    loop {
        match runtime.health(handle).await {
            Ok(Some(ContainerHealth::Healthy)) => return Readiness::Ready,
            Ok(Some(ContainerHealth::Unhealthy)) => return Readiness::Unhealthy,
            Ok(Some(ContainerHealth::Starting)) => {}
            Ok(None) | Err(_) => {
                let Some(address) = address else {
                    return Readiness::NoSignal;
                };
                if accepts(address).await {
                    return Readiness::Ready;
                }
            }
        }
        if !matches!(runtime.status(handle).await, Ok(WorkloadStatus::Running)) {
            return Readiness::NotRunning;
        }
        if started.elapsed() >= budget {
            return Readiness::TimedOut;
        }
        tokio::time::sleep(POLL).await;
    }
}

/// Whether `address` accepts a TCP connection.
async fn accepts(address: &str) -> bool {
    matches!(
        tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::TcpStream::connect(address)).await,
        Ok(Ok(_))
    )
}

#[cfg(test)]
#[path = "readiness_tests.rs"]
mod tests;
