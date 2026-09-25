//! Startup restore of persisted services: re-attach to containers that are
//! still running, register placeholders for agent-hosted services, and fully
//! reconcile anything else.

use orca_core::types::WorkloadStatus;
use tracing::info;

use crate::state::{self, AppState, InstanceState};
use crate::{reconciler, routes};

/// Check if Docker containers already exist for a persisted service.
/// If they do, populate in-memory state from existing containers.
/// Otherwise, fall back to full reconciliation.
pub(crate) async fn restore_or_reconcile(
    state: &AppState,
    config: &orca_core::config::ServiceConfig,
) -> anyhow::Result<()> {
    // Remote services run on an agent node whose WS connection isn't open yet
    // at startup. Register a placeholder so send_reconcile includes this service
    // when the agent connects — the agent will skip deployment if the container
    // is already running. A pin naming the master itself is local (#151): as a
    // placeholder, its running container was never re-attached, so it showed
    // 0/1, lost health checks and routes, and raised a "down" alert.
    if config
        .placement
        .as_ref()
        .and_then(|p| p.node.as_deref())
        .is_some_and(|pin| !crate::placement::pin_matches_master(pin))
    {
        let desired = match &config.replicas {
            orca_core::types::Replicas::Fixed(n) => *n,
            orca_core::types::Replicas::Auto => 1,
        };
        let mut services = state.services.write().await;
        let svc_state = services
            .entry(config.name.clone())
            .or_insert_with(|| state::ServiceState::from_config(config.clone()));
        svc_state.config = config.clone();
        svc_state.desired_replicas = desired;
        info!(service = %config.name, "Registered remote service placeholder");
        return Ok(());
    }

    // Local service: try to re-attach existing containers first.
    let cr = state
        .container_runtime
        .as_any()
        .downcast_ref::<orca_agent::docker::ContainerRuntime>();

    if let Some(container_rt) = cr {
        let existing = container_rt.find_existing(&config.name).await?;
        if !existing.is_empty() {
            info!(
                service = %config.name,
                count = existing.len(),
                "Re-attached to existing containers, skipping reconciliation"
            );
            populate_state_from_existing(state, config, existing).await;
            return Ok(());
        }
    }

    reconciler::reconcile_service(state, config)
        .await
        .map(|_| ())
}

/// Populate in-memory `ServiceState` from already-running Docker containers.
async fn populate_state_from_existing(
    state: &AppState,
    config: &orca_core::config::ServiceConfig,
    handles: Vec<orca_core::runtime::WorkloadHandle>,
) {
    // Re-attached containers are already running — mark Healthy so the
    // route filter accepts them. Health checker will correct on next probe.
    let initial_health = if config.health.is_some() || config.liveness.is_some() {
        orca_core::types::HealthState::Healthy
    } else {
        orca_core::types::HealthState::NoCheck
    };

    // Resolve host_port via the runtime (more reliable than metadata extraction)
    let runtime = state.container_runtime.as_ref();
    let mut instances: Vec<InstanceState> = Vec::new();
    for handle in handles {
        // Always resolve host_port using the configured container port —
        // metadata's first-port-binding heuristic is unreliable when extra_ports
        // are present (e.g. gitea SSH on 22222 vs HTTP on 3000).
        let mut host_port = if let Some(p) = config.port {
            runtime.resolve_host_port(&handle, p).await.ok().flatten()
        } else {
            None
        };
        if host_port.is_none() {
            host_port = handle
                .metadata
                .get("host_port")
                .and_then(|p| p.parse::<u16>().ok());
        }
        info!(
            service = %config.name,
            runtime_id = %&handle.runtime_id[..12],
            ?host_port,
            "Restored container instance"
        );
        instances.push(InstanceState {
            handle,
            status: WorkloadStatus::Running,
            host_port,
            container_address: None,
            health: initial_health,
            is_canary: false,
            started_at: std::time::Instant::now(),
        });
    }

    let desired = match &config.replicas {
        orca_core::types::Replicas::Fixed(n) => *n,
        orca_core::types::Replicas::Auto => 1,
    };

    let mut services = state.services.write().await;
    let svc_state = services
        .entry(config.name.clone())
        .or_insert_with(|| state::ServiceState::from_config(config.clone()));
    svc_state.instances = instances;
    svc_state.desired_replicas = desired;
    drop(services);

    // Update routing table for the restored service
    match config.runtime {
        orca_core::types::RuntimeKind::Container => {
            routes::update_container_routes(state, config).await;
        }
        orca_core::types::RuntimeKind::Wasm => {
            routes::update_wasm_triggers(state, config).await;
        }
    }
}

#[cfg(test)]
#[path = "restore_tests.rs"]
mod tests;
