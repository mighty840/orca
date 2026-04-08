use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::RwLock;
use tracing::info;

use orca_proxy::RouteTarget;

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

    let docker_runtime = Arc::new(orca_agent::docker::ContainerRuntime::new()?);
    let container_runtime: Arc<dyn orca_core::runtime::Runtime> = docker_runtime.clone();
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

    // Pull acme_email from the master's cluster.toml so the node-local proxy
    // can provision certs with a valid contact. Falls back to ORCA_ACME_EMAIL
    // env var, then a placeholder (which will fail validation — that's the
    // signal to set acme_email properly on the master).
    let acme_email = match agent.fetch_cluster_info().await {
        Some(info) => info
            .get("acme_email")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .or_else(|| std::env::var("ORCA_ACME_EMAIL").ok())
            .unwrap_or_else(|| "admin@localhost".to_string()),
        None => std::env::var("ORCA_ACME_EMAIL").unwrap_or_else(|_| "admin@localhost".to_string()),
    };
    info!("Using ACME email: {acme_email}");

    // Spawn a node-local reverse proxy. Without this, services scheduled
    // onto a joined node by the master are unreachable from the outside —
    // their domain DNS already points at this box, but only the agent runs
    // here, so requests have nowhere to land. The route table is rebuilt
    // periodically by inspecting docker labels (`orca.domain` / `orca.port`)
    // on every locally-managed container.
    let route_table: Arc<RwLock<HashMap<String, Vec<RouteTarget>>>> =
        Arc::new(RwLock::new(HashMap::new()));
    spawn_local_proxy(route_table.clone(), docker_runtime.clone(), acme_email).await;

    tokio::select! {
        _ = agent.run_heartbeat_loop(Duration::from_secs(5), container_runtime.clone()) => {},
        _ = tokio::signal::ctrl_c() => {
            info!("Shutdown signal received");
        }
    }

    info!("Agent shutdown complete");
    Ok(())
}

/// Start a reverse proxy on the joined node. Always brings up HTTP+HTTPS,
/// even if no domains exist yet — `ensure_cert_for_resolver` is called
/// hot whenever the route refresher discovers a new domain, so a service
/// can be added later without restarting the agent.
async fn spawn_local_proxy(
    route_table: Arc<RwLock<HashMap<String, Vec<RouteTarget>>>>,
    runtime: Arc<orca_agent::docker::ContainerRuntime>,
    acme_email: String,
) {
    let triggers: orca_proxy::SharedWasmTriggers = Arc::new(RwLock::new(Vec::new()));
    let cache = dirs_next::home_dir()
        .unwrap_or_else(|| ".".into())
        .join(".orca/certs");
    let acme = orca_proxy::acme::AcmeManager::new(acme_email, cache);

    // Collect initial domains (may be empty on a fresh node).
    let initial_domains: Vec<String> = runtime
        .list_local_routes()
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|(d, _)| d)
        .collect();

    let acme_for_proxy = acme.clone();
    let routes_for_proxy = route_table.clone();
    let triggers_for_proxy = triggers.clone();
    let domains_for_proxy = initial_domains.clone();
    let proxy_handle = tokio::spawn(async move {
        match orca_proxy::run_proxy_with_acme_and_fallback(
            routes_for_proxy,
            triggers_for_proxy,
            None,
            acme_for_proxy,
            domains_for_proxy,
            None,
        )
        .await
        {
            Ok(_resolver) => {}
            Err(e) => tracing::error!("Node-local proxy failed: {e}"),
        }
    });
    // Detach: the proxy spawns its own listeners and we don't await it here.
    drop(proxy_handle);
    info!(
        "Node-local proxy started (HTTP :80 + HTTPS :443) with {} initial domain(s)",
        initial_domains.len()
    );

    // Background route refresher: rescans docker every 5s and rebuilds the
    // route table from `orca.domain` / `orca.port` labels. New domains are
    // added immediately so the next request lands correctly.
    let refresher = route_table.clone();
    tokio::spawn(async move {
        let mut known_domains: std::collections::HashSet<String> =
            initial_domains.into_iter().collect();
        loop {
            tokio::time::sleep(Duration::from_secs(5)).await;
            let routes = match runtime.list_local_routes().await {
                Ok(r) => r,
                Err(e) => {
                    tracing::warn!("Failed to refresh local routes: {e}");
                    continue;
                }
            };
            let mut new_table: HashMap<String, Vec<RouteTarget>> = HashMap::new();
            let mut current: std::collections::HashSet<String> = std::collections::HashSet::new();
            for (domain, host_port) in routes {
                current.insert(domain.clone());
                new_table
                    .entry(domain.clone())
                    .or_default()
                    .push(RouteTarget {
                        address: format!("127.0.0.1:{host_port}"),
                        service_name: domain,
                        path_pattern: None,
                        weight: 100,
                    });
            }
            // New domains discovered since last refresh — log them; the proxy
            // will provision certs lazily on the first matching connection.
            for d in current.difference(&known_domains) {
                info!("Discovered new domain on this node: {d}");
            }
            known_domains = current;
            *refresher.write().await = new_table;
        }
    });
}
