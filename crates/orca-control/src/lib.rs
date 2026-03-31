pub mod api;
pub mod auth;
pub mod cluster_api;
pub(crate) mod cluster_handlers;
pub mod cluster_state;
pub mod deploy_history;
pub mod health;
pub(crate) mod operations;
pub mod proto;
pub mod raft;
pub mod reconciler;
pub(crate) mod routes;
pub mod scheduler;
pub mod state;
pub mod store;
pub mod webhook;

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
    let state = Arc::new(AppState::new(
        cluster_config.clone(),
        container_runtime,
        wasm_runtime,
        route_table,
        wasm_triggers,
    ));
    let app = api::router(state.clone());

    let addr = format!("0.0.0.0:{}", cluster_config.cluster.api_port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!("API server listening on {addr}");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    Ok(())
}

async fn shutdown_signal() {
    tokio::signal::ctrl_c()
        .await
        .expect("failed to install ctrl+c handler");
    info!("Shutdown signal received");
}
