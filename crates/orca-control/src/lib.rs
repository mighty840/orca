pub mod api;
pub mod auth;
pub mod cluster_api;
pub(crate) mod cluster_handlers;
pub mod cluster_state;
pub mod deploy_history;
pub mod health;
pub(crate) mod instance;
pub(crate) mod operations;
pub mod proto;
pub mod raft;
pub mod reconciler;
pub mod routes;
pub mod scheduler;
pub mod state;
pub mod store;
pub mod watchdog;
pub mod webhook;

use std::collections::HashMap;
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
    let state = Arc::new(app_state);

    // Register the master node so it appears in TUI/status.
    register_master_node(&state, cluster_config.cluster.api_port).await;
    spawn_master_heartbeat(state.clone());

    // Spawn background resilience tasks.
    watchdog::spawn_watchdog(state.clone());
    health::spawn_health_checker(state.clone());

    let app = api::router(state.clone());

    let addr = format!("0.0.0.0:{}", cluster_config.cluster.api_port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!("API server listening on {addr}");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
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
    };
    let mut nodes = state.registered_nodes.write().await;
    nodes.insert(node_id, node);
    info!(node_id, "Master node self-registered");
}

/// Spawn a periodic task that updates the master node's heartbeat timestamp.
fn spawn_master_heartbeat(state: Arc<state::AppState>) {
    let node_id = master_node_id();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            let mut nodes = state.registered_nodes.write().await;
            if let Some(node) = nodes.get_mut(&node_id) {
                node.last_heartbeat = chrono::Utc::now();
            }
        }
    });
}

async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("failed to install ctrl+c handler");
    info!("Shutdown signal received");
}
