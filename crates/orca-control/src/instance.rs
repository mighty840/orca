//! Instance creation and readiness helpers for the reconciler.

use std::time::Duration;

use orca_core::runtime::Runtime;
use orca_core::types::{WorkloadSpec, WorkloadStatus};

use crate::state::InstanceState;

/// Create, start, and wait for a workload instance to be ready.
pub(crate) async fn create_and_start_instance(
    runtime: &dyn Runtime,
    spec: &WorkloadSpec,
) -> anyhow::Result<InstanceState> {
    let handle = runtime.create(spec).await?;
    runtime.start(&handle).await?;

    let host_port = if let Some(port) = spec.port {
        runtime
            .resolve_host_port(&handle, port)
            .await
            .ok()
            .flatten()
    } else {
        None
    };

    let container_address = if let Some(port) = spec.port {
        let network = crate::routes::service_network_name(spec);
        runtime
            .resolve_container_address(&handle, port, &network)
            .await
            .ok()
            .flatten()
    } else {
        None
    };

    // Wait for container to be ready before registering routes.
    // Uses readiness probe if configured, falls back to health path, then port check.
    if let Some(port) = host_port {
        let addr = format!("127.0.0.1:{port}");
        let (path, delay) = if let Some(probe) = &spec.readiness {
            (probe.path.as_str(), probe.initial_delay_secs)
        } else {
            (spec.health.as_deref().unwrap_or("/"), 2)
        };
        if delay > 0 {
            tokio::time::sleep(Duration::from_secs(delay)).await;
        }
        wait_for_ready(&addr, path).await;
    }

    // If no health/liveness probe is configured, mark as NoCheck so the
    // instance is immediately routable. If probes exist, the health checker
    // will update the state after its first check.
    let initial_health = if spec.health.is_none() && spec.liveness.is_none() {
        orca_core::types::HealthState::NoCheck
    } else {
        orca_core::types::HealthState::Healthy
    };

    Ok(InstanceState {
        handle,
        status: WorkloadStatus::Running,
        host_port,
        container_address,
        health: initial_health,
        is_canary: false,
    })
}

/// Wait for a container to accept connections before registering routes.
pub(crate) async fn wait_for_ready(addr: &str, path: &str) {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .no_proxy()
        .build()
        .unwrap();
    let url = format!("http://{addr}{path}");

    for attempt in 1..=30 {
        match client.get(&url).send().await {
            Ok(resp) if resp.status().is_success() || resp.status().is_redirection() => {
                tracing::debug!("Container ready at {addr} (attempt {attempt})");
                return;
            }
            _ => {
                tokio::time::sleep(Duration::from_millis(500)).await;
            }
        }
    }
    tracing::warn!("Container at {addr} not ready after 15s, registering route anyway");
}
