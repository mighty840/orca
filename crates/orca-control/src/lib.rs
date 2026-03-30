pub mod api;
pub mod reconciler;
pub mod state;

use std::sync::Arc;

use orca_core::config::ClusterConfig;
use orca_core::runtime::Runtime;
use tracing::info;

use crate::state::{AppState, SharedRouteTable};

/// Start the orca control plane (API server) with a shared route table.
///
/// The route table is shared with the reverse proxy so both can
/// read/write the same routing state.
///
/// # Errors
///
/// Returns an error if the server fails to bind or encounters a fatal error.
pub async fn run_server(
    cluster_config: ClusterConfig,
    runtime: Arc<dyn Runtime>,
    route_table: SharedRouteTable,
) -> anyhow::Result<()> {
    let state = Arc::new(AppState::with_shared_routes(
        cluster_config.clone(),
        runtime,
        route_table,
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
