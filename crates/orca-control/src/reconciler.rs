//! Reconciler: ensures actual running containers/wasm instances match desired service config.

use std::time::Duration;

use tracing::{error, info};

use orca_core::config::ServiceConfig;
use orca_core::runtime::Runtime;
use orca_core::types::{Replicas, RuntimeKind, WorkloadSpec, WorkloadStatus};

use crate::routes::{service_config_to_spec, update_container_routes, update_wasm_triggers};
use crate::state::{AppState, InstanceState, ServiceState};

/// Reconcile all services: make reality match the desired config.
///
/// For each service, creates or removes workloads to match the desired replica count,
/// then updates the routing table (containers) or trigger table (wasm).
pub async fn reconcile(state: &AppState, services: &[ServiceConfig]) -> (Vec<String>, Vec<String>) {
    let mut deployed = Vec::new();
    let mut errors = Vec::new();

    for svc_config in services {
        match reconcile_service(state, svc_config).await {
            Ok(()) => {
                // Record successful deploy in history
                let mut history = state.deploy_history.write().await;
                history.record(svc_config);
                deployed.push(svc_config.name.clone());
            }
            Err(e) => errors.push(format!("{}: {e}", svc_config.name)),
        }
    }

    deployed
        .iter()
        .for_each(|name| info!("Deployed service: {name}"));

    (deployed, errors)
}

/// Get the appropriate runtime for a service config.
pub(crate) fn get_runtime(state: &AppState, kind: RuntimeKind) -> anyhow::Result<&dyn Runtime> {
    match kind {
        RuntimeKind::Container => Ok(state.container_runtime.as_ref()),
        RuntimeKind::Wasm => state
            .wasm_runtime
            .as_ref()
            .map(|r| r.as_ref() as &dyn Runtime)
            .ok_or_else(|| anyhow::anyhow!("Wasm runtime not available")),
    }
}

/// Reconcile a single service to match its desired state.
pub(crate) async fn reconcile_service(
    state: &AppState,
    config: &ServiceConfig,
) -> anyhow::Result<()> {
    let desired = match &config.replicas {
        Replicas::Fixed(n) => *n,
        Replicas::Auto => 1,
    };

    let spec = service_config_to_spec(config)?;
    let runtime = get_runtime(state, config.runtime)?;

    let mut services = state.services.write().await;
    let svc_state = services
        .entry(config.name.clone())
        .or_insert_with(|| ServiceState::from_config(config.clone()));

    svc_state.config = config.clone();
    svc_state.desired_replicas = desired;

    let current = svc_state.instances.len() as u32;

    if current < desired {
        let to_create = desired - current;
        info!(
            "Scaling up {} ({:?}): {} -> {} (+{})",
            config.name, config.runtime, current, desired, to_create
        );

        let mut failures = 0u32;
        for i in current..desired {
            let mut replica_spec = spec.clone();
            if desired > 1 {
                replica_spec.name = format!("{}-{i}", spec.name);
            }

            match create_and_start_instance(runtime, &replica_spec).await {
                Ok(instance) => {
                    svc_state.instances.push(instance);
                }
                Err(e) => {
                    error!("Failed to create instance {}-{i}: {e}", config.name);
                    failures += 1;
                }
            }
        }
        if failures > 0 {
            tracing::warn!("{failures}/{to_create} replicas failed for {}", config.name);
        }
    } else if current > desired {
        let to_remove = current - desired;
        info!(
            "Scaling down {} ({:?}): {} -> {} (-{})",
            config.name, config.runtime, current, desired, to_remove
        );

        for _ in 0..to_remove {
            if let Some(instance) = svc_state.instances.pop() {
                let _ = runtime
                    .stop(&instance.handle, Duration::from_secs(10))
                    .await;
                let _ = runtime.remove(&instance.handle).await;
            }
        }
    }

    // Refresh status of all instances
    for instance in &mut svc_state.instances {
        if let Ok(status) = runtime.status(&instance.handle).await {
            instance.status = status;
        }
    }

    drop(services);

    // Update routing based on runtime type
    match config.runtime {
        RuntimeKind::Container => update_container_routes(state, config).await,
        RuntimeKind::Wasm => update_wasm_triggers(state, config).await,
    }

    Ok(())
}

/// Create, start, and wait for a workload instance to be ready.
async fn create_and_start_instance(
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
        let network = super::routes::service_network_name(spec);
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
            tokio::time::sleep(std::time::Duration::from_secs(delay)).await;
        }
        wait_for_ready(&addr, path).await;
    }

    Ok(InstanceState {
        handle,
        status: WorkloadStatus::Running,
        host_port,
        container_address,
        health: orca_core::types::HealthState::Unknown,
    })
}

/// Wait for a container to accept connections before registering routes.
async fn wait_for_ready(addr: &str, path: &str) {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(2))
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
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
            }
        }
    }
    tracing::warn!("Container at {addr} not ready after 15s, registering route anyway");
}

// stop, stop_all, redeploy, rollback, scale moved to operations.rs
pub use crate::operations::{redeploy, rollback, scale, stop, stop_all};
