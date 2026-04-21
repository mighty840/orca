pub mod api;
pub mod auth;
pub mod backup_scheduler;
pub(crate) mod canary;
pub mod cluster_api;
pub(crate) mod cluster_handlers;
pub mod cluster_state;
pub mod deploy_history;
pub mod health;
pub(crate) mod instance;
pub mod metrics;
pub(crate) mod operations;
pub mod proto;
pub mod raft;
pub mod reconciler;
pub mod routes;
pub mod scheduler;
pub mod state;
pub mod stats;
pub mod store;
pub mod topo_sort;
pub mod watchdog;
pub mod webhook;
pub mod ws_handler;

use std::collections::HashMap;
use std::sync::Arc;

use orca_core::config::ClusterConfig;
use orca_core::runtime::Runtime;
use orca_core::types::WorkloadStatus;
use tracing::info;

use crate::state::{AppState, InstanceState, SharedRouteTable, SharedWasmTriggers};

/// Start the orca control plane (API server).
///
/// # Errors
///
/// Returns an error if the server fails to bind or encounters a fatal error.
pub async fn run_server(
    cluster_config: ClusterConfig,
    container_runtime: Arc<dyn Runtime>,
    wasm_runtime: Option<Arc<dyn Runtime>>,
    route_table: SharedRouteTable,
    wasm_triggers: SharedWasmTriggers,
) -> anyhow::Result<()> {
    run_server_with_acme(
        cluster_config,
        container_runtime,
        wasm_runtime,
        route_table,
        wasm_triggers,
        None,
        None,
    )
    .await
}

/// Start the orca control plane with optional ACME hot-provisioning.
pub async fn run_server_with_acme(
    cluster_config: ClusterConfig,
    container_runtime: Arc<dyn Runtime>,
    wasm_runtime: Option<Arc<dyn Runtime>>,
    route_table: SharedRouteTable,
    wasm_triggers: SharedWasmTriggers,
    acme_manager: Option<orca_proxy::acme::AcmeManager>,
    cert_resolver: Option<orca_proxy::SharedCertResolver>,
) -> anyhow::Result<()> {
    let mut app_state = AppState::new(
        cluster_config.clone(),
        container_runtime,
        wasm_runtime,
        route_table,
        wasm_triggers,
    );
    if let (Some(acme), Some(resolver)) = (acme_manager, cert_resolver) {
        app_state = app_state.with_acme(acme, resolver);
    }

    // Open persistent store
    let store_path = dirs_next::home_dir()
        .unwrap_or_else(|| ".".into())
        .join(".orca/cluster.db");
    match store::ClusterStore::open(&store_path) {
        Ok(s) => {
            info!("Persistent store opened at {}", store_path.display());
            app_state = app_state.with_store(Arc::new(s));
        }
        Err(e) => {
            tracing::warn!("Failed to open store at {}: {e}", store_path.display());
        }
    }

    let state = Arc::new(app_state);

    // Restore persisted services, re-attaching to existing containers
    if let Some(store) = &state.store {
        match store.get_all_services() {
            Ok(services) if !services.is_empty() => {
                info!("Restoring {} persisted services", services.len());
                for config in services.values() {
                    if let Err(e) = restore_or_reconcile(&state, config).await {
                        tracing::warn!(service = %config.name, "Failed to restore: {e}");
                    }
                }
            }
            Ok(_) => {}
            Err(e) => tracing::warn!("Failed to load persisted services: {e}"),
        }
    }

    // Register the master node so it appears in TUI/status.
    register_master_node(&state, cluster_config.cluster.api_port).await;
    spawn_master_heartbeat(state.clone());

    // Spawn background resilience tasks.
    watchdog::spawn_watchdog(state.clone());
    health::spawn_health_checker(state.clone());
    stats::spawn_stats_collector(state.clone());

    // Spawn scheduled backup task if configured (needs state for agent dispatch).
    if let Some(backup_cfg) = cluster_config.backup.clone()
        && backup_scheduler::spawn_backup_scheduler(backup_cfg, state.clone()).is_some()
    {
        info!("Backup scheduler started");
    }

    let app = api::router(state.clone());

    let addr = format!("0.0.0.0:{}", cluster_config.cluster.api_port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!("API server listening on {addr}");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

/// Check if Docker containers already exist for a persisted service.
/// If they do, populate in-memory state from existing containers.
/// Otherwise, fall back to full reconciliation.
async fn restore_or_reconcile(
    state: &AppState,
    config: &orca_core::config::ServiceConfig,
) -> anyhow::Result<()> {
    // Try to downcast to ContainerRuntime for find_existing
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

    reconciler::reconcile_service(state, config).await
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

/// Compute a deterministic node ID from the system hostname.
fn master_node_id() -> u64 {
    use std::hash::{Hash, Hasher};
    let hostname = std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "orca-master".to_string());
    let mut hasher = std::hash::DefaultHasher::new();
    hostname.hash(&mut hasher);
    hasher.finish()
}

/// Register the master node in the cluster node map.
async fn register_master_node(state: &state::AppState, api_port: u16) {
    let node_id = master_node_id();
    let mut labels = HashMap::new();
    labels.insert("role".to_string(), "master".to_string());
    let node = state::RegisteredNode {
        node_id,
        address: format!("localhost:{api_port}"),
        labels,
        last_heartbeat: chrono::Utc::now(),
        drain: false,
        cpu_percent: 0.0,
        memory_bytes: 0,
        memory_total: 0,
        disk_used: 0,
        disk_total: 0,
        net_rx: 0,
        net_tx: 0,
    };
    let mut nodes = state.registered_nodes.write().await;
    nodes.insert(node_id, node);
    info!(node_id, "Master node self-registered");
}

/// Spawn a periodic task that samples host stats and writes them onto the
/// master node's entry in the cluster node map. Joined nodes push their own
/// stats via the heartbeat; the master has no heartbeat to piggyback on so
/// it does this in-process instead. The same loop also prunes zombie
/// nodes — entries whose last heartbeat is older than 60s — which keeps
/// the cluster/info endpoint clean after a joined node is restarted with
/// a fresh id (until node.id persistence lands on every joined box).
fn spawn_master_heartbeat(state: Arc<state::AppState>) {
    const STALE_AFTER: chrono::Duration = chrono::Duration::seconds(60);
    let node_id = master_node_id();
    let collector = Arc::new(orca_agent::host_stats::HostStatsCollector::new());
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(2)).await;
            let sample = collector.sample();
            let now = chrono::Utc::now();
            let mut nodes = state.registered_nodes.write().await;
            if let Some(node) = nodes.get_mut(&node_id) {
                node.last_heartbeat = now;
                node.cpu_percent = sample.cpu_percent;
                node.memory_bytes = sample.memory_bytes;
                node.memory_total = sample.memory_total;
                node.disk_used = sample.disk_used;
                node.disk_total = sample.disk_total;
                node.net_rx = sample.net_rx;
                node.net_tx = sample.net_tx;
            }
            nodes.retain(|id, node| {
                if *id == node_id {
                    return true;
                }
                let age = now - node.last_heartbeat;
                age < STALE_AFTER
            });
        }
    });
}

async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("failed to install ctrl+c handler");
    info!("Shutdown signal received");
}
