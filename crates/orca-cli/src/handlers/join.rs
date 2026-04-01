use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tracing::info;

/// Handle the `orca join` command — join this node to an existing cluster.
pub async fn handle_join(
    leader_address: &str,
    node_id: Option<u64>,
    labels: HashMap<String, String>,
    setup_key: Option<String>,
) -> anyhow::Result<()> {
    let node_id = node_id.unwrap_or_else(|| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64
    });

    // If a NetBird setup key is provided, connect to the mesh first
    if let Some(key) = &setup_key {
        let nb = orca_agent::netbird::NetbirdManager::new(None);
        if let Err(e) = nb.install() {
            tracing::warn!("NetBird install failed: {e}");
        }
        nb.connect(key)?;
        if let Ok(Some(ip)) = nb.get_ip() {
            info!("NetBird mesh IP: {ip}");
        }
    }

    let leader_url = if leader_address.starts_with("http") {
        leader_address.to_string()
    } else {
        format!("http://{leader_address}")
    };

    info!("Joining cluster at {leader_url} as node {node_id}");

    // Save leader URL and token so TUI and other commands work on agent nodes
    let orca_dir = dirs_next::home_dir()
        .unwrap_or_else(|| ".".into())
        .join(".orca");
    let _ = std::fs::create_dir_all(&orca_dir);
    let _ = std::fs::write(orca_dir.join("leader.url"), &leader_url);
    if let Ok(token) = std::env::var("ORCA_TOKEN") {
        let token_path = orca_dir.join("cluster.token");
        let _ = std::fs::write(&token_path, &token);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&token_path, std::fs::Permissions::from_mode(0o600));
        }
    }

    let container_runtime: Arc<dyn orca_core::runtime::Runtime> =
        Arc::new(orca_agent::docker::ContainerRuntime::new()?);
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

    // Use NetBird IP as local address if available
    let nb = orca_agent::netbird::NetbirdManager::new(None);
    let local_ip = nb.get_ip().ok().flatten().unwrap_or_else(|| {
        hostname::get()
            .ok()
            .and_then(|h| h.into_string().ok())
            .unwrap_or_else(|| "127.0.0.1".to_string())
    });
    let local_address = format!("{local_ip}:6881");

    let agent = orca_agent::grpc::AgentClient::new(leader_url, node_id);

    // Retry registration with exponential backoff
    let mut delay = Duration::from_secs(2);
    for attempt in 1..=30 {
        match agent.register(&local_address, &labels).await {
            Ok(()) => break,
            Err(e) => {
                if attempt == 30 {
                    anyhow::bail!("Registration failed after 30 attempts: {e}");
                }
                tracing::warn!("Registration attempt {attempt} failed: {e}, retrying in {delay:?}");
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(Duration::from_secs(30));
            }
        }
    }

    info!("Registered with cluster. Running heartbeat loop...");

    tokio::select! {
        _ = agent.run_heartbeat_loop(Duration::from_secs(5), container_runtime.clone()) => {},
        _ = tokio::signal::ctrl_c() => {
            info!("Shutdown signal received");
        }
    }

    info!("Agent shutdown complete");
    Ok(())
}
