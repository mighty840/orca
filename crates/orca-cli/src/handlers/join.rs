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
    // Persist the node_id to disk so repeated `orca join` calls from the
    // same host reuse it. Without this each restart creates a fresh id and
    // the master accumulates zombie entries from every previous join.
    let node_id = node_id.unwrap_or_else(|| {
        let path = dirs_next::home_dir()
            .unwrap_or_else(|| ".".into())
            .join(".orca/node.id");
        if let Ok(s) = std::fs::read_to_string(&path)
            && let Ok(id) = s.trim().parse::<u64>()
        {
            return id;
        }
        let id = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&path, id.to_string());
        id
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

    let agent = orca_agent::grpc::AgentClient::new(leader_url.clone(), node_id);

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
    let (acme, resolver) =
        spawn_local_proxy(route_table.clone(), docker_runtime.clone(), acme_email).await;

    // Channel for domain discovery notifications (WS deploy → proxy route + cert)
    let (domain_tx, mut domain_rx) = tokio::sync::mpsc::channel::<(String, String, u16)>(32);

    // Spawn domain discovery listener that updates the proxy route table
    // AND hot-provisions TLS certs for newly discovered domains.
    let route_table_for_domains = route_table.clone();
    tokio::spawn(async move {
        while let Some((service, domain, port)) = domain_rx.recv().await {
            info!("WS domain notification: {domain} for {service} (port {port})");
            let target = RouteTarget {
                address: format!("127.0.0.1:{port}"),
                service_name: service,
                path_pattern: Some("/*".to_string()),
                strip_prefix: None,
                weight: 100,
            };
            let mut routes = route_table_for_domains.write().await;
            routes.insert(domain.clone(), vec![target]);
            drop(routes);

            // Hot-provision TLS cert for the new domain
            if let Some(res) = &resolver
                && !res.has_cert(&domain)
            {
                match acme.ensure_cert_for_resolver(&domain, res).await {
                    Ok(()) => info!("Hot-provisioned TLS cert for {domain}"),
                    Err(e) => tracing::warn!("Cert provisioning failed for {domain}: {e}"),
                }
            }
        }
    });

    // Read cluster token for WS auth
    let token = std::env::var("ORCA_TOKEN").unwrap_or_default();
    let agent_arc = Arc::new(agent);

    tokio::select! {
        // Try WS streaming (auto-reconnects, sends heartbeats over WS)
        _ = orca_agent::ws_client::run_ws_loop(
            &leader_url,
            node_id,
            &token,
            container_runtime.clone(),
            agent_arc.clone(),
            domain_tx,
        ) => {},
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
/// Returns the ACME manager and cert resolver for hot cert provisioning.
async fn spawn_local_proxy(
    route_table: Arc<RwLock<HashMap<String, Vec<RouteTarget>>>>,
    runtime: Arc<orca_agent::docker::ContainerRuntime>,
    acme_email: String,
) -> (
    orca_proxy::acme::AcmeManager,
    Option<orca_proxy::SharedCertResolver>,
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
        .map(|r| r.domain)
        .collect();

    let acme_for_proxy = acme.clone();
    let routes_for_proxy = route_table.clone();
    let triggers_for_proxy = triggers.clone();
    let domains_for_proxy = initial_domains.clone();
    // Start the proxy and capture the cert resolver for hot provisioning.
    let resolver_for_refresh = match orca_proxy::run_proxy_with_acme_and_fallback(
        routes_for_proxy,
        triggers_for_proxy,
        None,
        acme_for_proxy.clone(),
        domains_for_proxy,
        None,
    )
    .await
    {
        Ok(resolver) => {
            info!(
                "Node-local proxy started (HTTP :80 + HTTPS :443) with {} initial domain(s)",
                initial_domains.len()
            );
            Some(resolver)
        }
        Err(e) => {
            tracing::error!("Node-local proxy failed: {e}");
            None
        }
    };

    let resolver_out = resolver_for_refresh.clone();

    // Background route refresher: rescans docker every 5s and rebuilds the
    // route table from `orca.domain` / `orca.port` labels. New domains get
    // their TLS cert hot-provisioned immediately.
    let refresher = route_table.clone();
    let acme_for_refresh = acme.clone();
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
            for r in routes {
                current.insert(r.domain.clone());
                // Emit one RouteTarget per declared path pattern; services
                // without any `routes` entry get a single catch-all.
                let patterns: Vec<Option<String>> = if r.routes.is_empty() {
                    vec![None]
                } else {
                    r.routes.iter().map(|p| Some(p.clone())).collect()
                };
                for pattern in patterns {
                    new_table
                        .entry(r.domain.clone())
                        .or_default()
                        .push(RouteTarget {
                            address: format!("127.0.0.1:{}", r.host_port),
                            service_name: r.service.clone(),
                            path_pattern: pattern,
                            strip_prefix: r.strip_prefix.clone(),
                            weight: 100,
                        });
                }
            }
            // New domains discovered since last refresh — hot-provision TLS certs.
            for d in current.difference(&known_domains) {
                info!("Discovered new domain on this node: {d}");
                if let Some(resolver) = &resolver_for_refresh
                    && !resolver.has_cert(d)
                {
                    if let Err(e) = acme_for_refresh.ensure_cert_for_resolver(d, resolver).await {
                        tracing::warn!("Failed to provision cert for {d}: {e}");
                    } else {
                        info!("Hot-provisioned TLS cert for {d}");
                    }
                }
            }
            known_domains = current;
            *refresher.write().await = new_table;
        }
    });

    (acme, resolver_out)
}
