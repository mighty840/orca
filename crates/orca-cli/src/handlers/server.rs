use std::collections::HashMap;
use std::sync::Arc;

use tracing::info;

use super::port::{check_privileged_port, is_permission_denied, setup_port_redirect};

/// Handle the `orca server` command.
pub async fn handle_server(config: &str, proxy_port: u16) -> anyhow::Result<()> {
    check_privileged_port(proxy_port);
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

    // Collect domains for ACME cert provisioning
    let acme_email = cluster_config.cluster.acme_email.clone();
    let services_dir = std::path::Path::new("services");
    let domains: Vec<String> = if services_dir.is_dir() {
        orca_core::config::ServicesConfig::load_dir(services_dir)
            .ok()
            .map(|s| {
                s.service
                    .iter()
                    .filter_map(|svc| svc.domain.clone())
                    .collect()
            })
            .unwrap_or_default()
    } else {
        std::path::Path::new("services.toml")
            .exists()
            .then(|| orca_core::config::ServicesConfig::load("services.toml".as_ref()).ok())
            .flatten()
            .map(|s| {
                s.service
                    .iter()
                    .filter_map(|svc| svc.domain.clone())
                    .collect()
            })
            .unwrap_or_default()
    };
    info!("Domain detection: {} domains found", domains.len());

    // Start proxy and get ACME components for hot cert provisioning
    let proxy_routes = route_table.clone();
    let proxy_triggers = wasm_triggers.clone();
    let (acme_for_control, resolver_for_control) = if !domains.is_empty()
        && let Some(email) = acme_email
    {
        let cache = std::env::var("HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| ".".into())
            .join(".orca/certs");
        let acme = orca_proxy::acme::AcmeManager::new(email, cache);
        let acme_clone = acme.clone();
        match orca_proxy::run_proxy_with_acme(
            proxy_routes,
            proxy_triggers,
            wasm_invoker,
            acme.clone(),
            domains,
        )
        .await
        {
            Ok(resolver) => (Some(acme_clone), Some(resolver)),
            Err(e) => {
                tracing::error!("Proxy with ACME failed: {e}");
                (None, None)
            }
        }
    } else {
        // Fallback: HTTP only proxy
        let proxy_routes_c = proxy_routes.clone();
        let proxy_triggers_c = proxy_triggers.clone();
        let wasm_invoker_c = wasm_invoker.clone();
        tokio::spawn(async move {
            let actual_port = match orca_proxy::run_proxy(
                proxy_routes_c.clone(),
                proxy_triggers_c.clone(),
                wasm_invoker_c.clone(),
                proxy_port,
                None,
                None,
            )
            .await
            {
                Ok(()) => return,
                Err(e) if is_permission_denied(&e) && (proxy_port == 80 || proxy_port == 443) => {
                    let high_port = setup_port_redirect(proxy_port);
                    if high_port == proxy_port {
                        tracing::error!("Proxy error: {e}");
                        tracing::error!(
                            "Port {proxy_port} requires root. Run with sudo or use --proxy-port {}",
                            if proxy_port == 80 { 8080 } else { 8443 }
                        );
                        return;
                    }
                    high_port
                }
                Err(e) => {
                    tracing::error!("Proxy error: {e}");
                    return;
                }
            };

            info!("Retrying proxy on port {actual_port} (iptables redirect from {proxy_port})");
            if let Err(e) = orca_proxy::run_proxy(
                proxy_routes_c,
                proxy_triggers_c,
                wasm_invoker_c,
                actual_port,
                None,
                None,
            )
            .await
            {
                tracing::error!("Proxy error on fallback port {actual_port}: {e}");
            }
        });
        (None, None)
    };

    // Run API server (blocks until shutdown)
    let cleanup_runtime = container_runtime.clone();
    orca_control::run_server_with_acme(
        cluster_config,
        container_runtime,
        wasm_as_trait,
        route_table,
        wasm_triggers,
        acme_for_control,
        resolver_for_control,
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
