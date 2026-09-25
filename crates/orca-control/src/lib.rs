pub mod adoption;
pub mod alerts;
pub mod api;
mod api_listen;
pub mod auth;
pub mod backup_scheduler;
pub(crate) mod canary;
pub mod certs;
pub mod cleanup_scheduler;
pub(crate) mod cluster_handlers;
pub mod cluster_state;
mod config_diff;
pub mod declarative;
pub mod dependents;
pub mod deploy_history;
pub mod failures;
pub mod health;
pub(crate) mod instance;
pub(crate) mod master_node;
pub mod metrics;
pub(crate) mod operations;
pub(crate) mod placement;
pub mod proto;
pub mod raft;
pub mod reconciler;
mod relocate;
mod remote_deploy;
mod restore;
pub mod routes;
pub mod scheduler;
pub mod session;
pub mod state;
pub mod stats;
pub mod store;
pub mod topo_sort;
pub mod watchdog;
pub mod webhook;
mod webhook_auth;
pub mod webhook_invocations;
mod webhook_store;
pub mod ws_handler;

use std::sync::Arc;

use orca_core::config::ClusterConfig;
use orca_core::runtime::Runtime;
use tracing::info;

use crate::state::{AppState, SharedRouteTable, SharedWasmTriggers};

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

    if let Some(engine) = alerts::try_build_alert_engine(cluster_config.ai.as_ref()) {
        app_state = app_state.with_alerts(engine);
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
        let stopped = store.get_stopped().unwrap_or_default();
        match store.get_all_services() {
            Ok(services) if !services.is_empty() => {
                info!("Restoring {} persisted services", services.len());
                for config in services.values() {
                    // Paused services come back registered-but-not-started, so
                    // the watchdog/reconciler never auto-start them.
                    if stopped.contains(&config.name) {
                        let mut svcs = state.services.write().await;
                        let svc = svcs
                            .entry(config.name.clone())
                            .or_insert_with(|| state::ServiceState::from_config(config.clone()));
                        svc.config = config.clone();
                        svc.stopped = true;
                        svc.desired_replicas = 0;
                        info!(service = %config.name, "Restored as stopped (paused)");
                        continue;
                    }
                    if let Err(e) = restore::restore_or_reconcile(&state, config).await {
                        tracing::warn!(service = %config.name, "Failed to restore: {e}");
                    }
                }
            }
            Ok(_) => {}
            Err(e) => tracing::warn!("Failed to load persisted services: {e}"),
        }
    }

    // Register the master node so it appears in TUI/status.
    master_node::register_master_node(&state, cluster_config.cluster.api_port).await;
    master_node::spawn_master_heartbeat(state.clone());

    // Spawn background resilience tasks.
    watchdog::spawn_watchdog(state.clone());
    health::spawn_health_checker(state.clone());
    stats::spawn_stats_collector(state.clone());
    if adoption::spawn_adoption_reconciler(state.clone()) {
        info!("Orphan-adoption reconciler started");
    }
    if declarative::spawn_declarative_reconciler(state.clone()) {
        info!("Declarative reconciler started");
    }

    // Spawn scheduled backup task if configured (needs state for agent dispatch).
    if let Some(backup_cfg) = cluster_config.backup.clone()
        && backup_scheduler::spawn_backup_scheduler(backup_cfg, state.clone()).is_some()
    {
        info!("Backup scheduler started");
    }

    if let Some(cleanup_cfg) = cluster_config.cleanup.clone()
        && cleanup_scheduler::spawn_cleanup_scheduler(cleanup_cfg, state.clone()).is_some()
    {
        info!("Cleanup scheduler started");
    }

    if alerts::spawn_alert_monitor(state.clone()).is_some() {
        info!("AI alert monitor started");
    }

    let app = api::router(state.clone());

    let bind = &cluster_config.cluster.api_bind;
    let addrs = api_listen::listen_addrs(bind, cluster_config.cluster.api_port)
        .map_err(anyhow::Error::msg)?;
    if !api_listen::reachable_from_local_cli(bind) {
        tracing::warn!(
            "cluster.api_bind does not include 127.0.0.1: the `orca` CLI and TUI on this host \
             connect there by default and will not reach the API. Add \"127.0.0.1\" to \
             api_bind, or pass --api."
        );
    }
    let mut listeners = Vec::with_capacity(addrs.len());
    for addr in &addrs {
        let listener = tokio::net::TcpListener::bind(addr).await.map_err(|e| {
            anyhow::anyhow!(
                "cannot listen on {addr} (cluster.api_bind): {e}. If this is a VPN or mesh \
                 address, its interface must be up before orca starts."
            )
        })?;
        info!("API server listening on {addr}");
        listeners.push(listener);
    }

    api_listen::serve_all(app, listeners, shutdown_signal()).await
}

async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("failed to install ctrl+c handler");
    info!("Shutdown signal received");
}
