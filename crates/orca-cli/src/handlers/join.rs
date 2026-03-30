use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tracing::info;

/// Handle the `orca join` command — join this node to an existing cluster.
pub async fn handle_join(
    leader_address: &str,
    node_id: Option<u64>,
    labels: HashMap<String, String>,
) -> anyhow::Result<()> {
    // Generate a node ID if not provided (use timestamp-based)
    let node_id = node_id.unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    });

    let leader_url = if leader_address.starts_with("http") {
        leader_address.to_string()
    } else {
        format!("http://{leader_address}")
    };

    info!("Joining cluster at {leader_url} as node {node_id}");

    // Create container and wasm runtimes (will be used for workload execution)
    let _container_runtime = Arc::new(orca_agent::docker::ContainerRuntime::new()?);
    let _wasm_runtime = match orca_agent::wasm::WasmRuntime::new() {
        Ok(r) => {
            info!("Wasm runtime initialized");
            Some(Arc::new(r))
        }
        Err(e) => {
            tracing::warn!("Wasm runtime unavailable: {e}");
            None
        }
    };

    // Detect this node's address (use hostname:grpc_port)
    let hostname = hostname::get()
        .ok()
        .and_then(|h| h.into_string().ok())
        .unwrap_or_else(|| "127.0.0.1".to_string());
    let local_address = format!("{hostname}:6881");

    // Create agent client and register with the leader
    let agent = orca_agent::grpc::AgentClient::new(leader_url, node_id);
    agent.register(&local_address, &labels).await?;

    info!("Registered with cluster. Running heartbeat loop...");

    // Run heartbeat loop (blocks until shutdown)
    tokio::select! {
        _ = agent.run_heartbeat_loop(Duration::from_secs(5)) => {},
        _ = tokio::signal::ctrl_c() => {
            info!("Shutdown signal received");
        }
    }

    info!("Agent shutdown complete");
    Ok(())
}
