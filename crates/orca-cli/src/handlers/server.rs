use std::collections::HashMap;
use std::sync::Arc;

use tracing::info;

/// Handle the `orca server` command.
pub async fn handle_server(config: &str, proxy_port: u16) -> anyhow::Result<()> {
    let cluster_config = match orca_core::config::ClusterConfig::load(config.as_ref()) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("No cluster.toml found ({e}), using defaults");
            orca_core::config::ClusterConfig::default()
        }
    };
    // Auto-generate cluster token if none configured
    let mut cluster_config = cluster_config;
    if cluster_config.api_tokens.is_empty() {
        let token = ensure_cluster_token();
        cluster_config.api_tokens = vec![token.clone()];
        println!("Cluster token: {token}");
        println!("Use this to join nodes: orca join <this-ip>:6880 --token {token}");
    }

    info!(
        "Starting orca server '{}' (API: {}, Proxy: {})",
        cluster_config.cluster.name, cluster_config.cluster.api_port, proxy_port,
    );

    setup_netbird(&cluster_config).await;

    // Create runtimes
    let container_runtime = Arc::new(orca_agent::docker::ContainerRuntime::new()?);
    let wasm_runtime = match orca_agent::wasm::WasmRuntime::new() {
        Ok(r) => {
            info!("Wasm runtime initialized (wasmtime)");
            Some(Arc::new(r))
        }
        Err(e) => {
            tracing::warn!("Wasm runtime unavailable: {e}");
            None
        }
    };

    // Shared state
    let route_table = Arc::new(tokio::sync::RwLock::new(HashMap::new()));
    let wasm_triggers: orca_proxy::SharedWasmTriggers =
        Arc::new(tokio::sync::RwLock::new(Vec::new()));

    let wasm_invoker: Option<orca_proxy::WasmInvoker> = wasm_runtime.as_ref().map(|wr| {
        let wr = wr.clone();
        Arc::new(
            move |runtime_id: String, method: String, path: String, body: String| {
                let wr = wr.clone();
                Box::pin(async move {
                    wr.invoke_http(&runtime_id, &method, &path, &body)
                        .await
                        .map_err(|e| e.to_string())
                }) as orca_proxy::WasmInvokeFuture
            },
        ) as orca_proxy::WasmInvoker
    });

    let wasm_as_trait: Option<Arc<dyn orca_core::runtime::Runtime>> =
        wasm_runtime.map(|wr| wr as Arc<dyn orca_core::runtime::Runtime>);

    // Check if any domain needs TLS — load services.toml if it exists
    let acme_email = cluster_config.cluster.acme_email.clone();
    let has_domains = std::path::Path::new("services.toml")
        .exists()
        .then(|| orca_core::config::ServicesConfig::load("services.toml".as_ref()).ok())
        .flatten()
        .map(|s| s.service.iter().any(|svc| svc.domain.is_some()))
        .unwrap_or(false);

    // Spawn proxy: HTTPS on 443 + HTTP on 80 if domains exist, else HTTP only
    let proxy_routes = route_table.clone();
    let proxy_triggers = wasm_triggers.clone();
    tokio::spawn(async move {
        let acme = if has_domains {
            acme_email.map(|email| {
                let cache = std::env::var("HOME")
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(|_| ".".into())
                    .join(".orca/certs");
                orca_proxy::acme::AcmeManager::new(email, cache)
            })
        } else {
            None
        };

        if let Err(e) = orca_proxy::run_proxy(
            proxy_routes,
            proxy_triggers,
            wasm_invoker,
            proxy_port,
            None,
            acme,
        )
        .await
        {
            tracing::error!("Proxy error: {e}");
        }
    });

    // Run API server (blocks until shutdown)
    let cleanup_runtime = container_runtime.clone();
    orca_control::run_server(
        cluster_config,
        container_runtime,
        wasm_as_trait,
        route_table,
        wasm_triggers,
    )
    .await?;

    info!("Shutting down, cleaning up containers...");
    cleanup_runtime.cleanup_all().await;
    info!("Shutdown complete");
    Ok(())
}

/// Install and configure NetBird if configured in cluster.toml.
async fn setup_netbird(config: &orca_core::config::ClusterConfig) {
    let Some(net) = &config.network else { return };
    if net.provider != "netbird" {
        return;
    }

    let nb = orca_agent::netbird::NetbirdManager::new(net.management_url.clone());

    if let Err(e) = nb.install() {
        tracing::warn!("NetBird install failed: {e}");
        return;
    }

    if let Some(key) = &net.setup_key
        && let Err(e) = nb.connect(key)
    {
        tracing::warn!("NetBird connect failed: {e}");
        return;
    }

    if let Ok(Some(ip)) = nb.get_ip() {
        info!("NetBird mesh IP: {ip}");
    }
}

/// Token file path.
fn token_path() -> std::path::PathBuf {
    std::env::var("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| ".".into())
        .join(".orca/cluster.token")
}

/// Load or generate a cluster token.
fn ensure_cluster_token() -> String {
    let path = token_path();
    if path.exists()
        && let Ok(token) = std::fs::read_to_string(&path)
    {
        let token = token.trim().to_string();
        if !token.is_empty() {
            return token;
        }
    }
    // Generate new token
    let token = format!("{:x}{:x}", rand::random::<u64>(), rand::random::<u64>());
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&path, &token);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    token
}

/// Show the cluster token.
pub fn show_token() {
    let path = token_path();
    if path.exists() {
        if let Ok(token) = std::fs::read_to_string(&path) {
            println!("{}", token.trim());
        }
    } else {
        println!("No cluster token found. Start the server first.");
    }
}

/// Read token from file, env var, or CLI flag.
pub fn read_token(flag: Option<&str>) -> Option<String> {
    if let Some(t) = flag {
        return Some(t.to_string());
    }
    if let Ok(t) = std::env::var("ORCA_TOKEN") {
        return Some(t);
    }
    let path = token_path();
    std::fs::read_to_string(path)
        .ok()
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}
