use std::collections::HashMap;
use std::sync::Arc;

use tracing::info;

use super::port::{
    check_privileged_port, cleanup_port_redirects, cleanup_stale_redirects, is_permission_denied,
    setup_port_redirect,
};

/// Handle the `orca server` command.
pub async fn handle_server(config: &str, proxy_port: u16) -> anyhow::Result<()> {
    check_privileged_port(proxy_port);
    cleanup_stale_redirects();
    let cluster_config = load_cluster_config_or_fail(std::path::Path::new(config))?;
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

    // Collect domains for ACME cert provisioning. Skip domains whose service
    // placement explicitly targets a different node — those services are
    // edge-served by the agent, so the agent (not master) provisions and
    // serves the cert. Master attempting ACME for them would deadlock the
    // HTTPS startup loop on the LE HTTP-01 challenge, since DNS for those
    // domains points elsewhere.
    let acme_email = cluster_config.cluster.acme_email.clone();
    let master_node = master_hostname();
    let collect_master_domains = |s: orca_core::config::ServicesConfig| -> Vec<String> {
        s.service
            .iter()
            .filter(|svc| is_master_placed(svc, &master_node))
            .flat_map(|svc| svc.all_domains())
            .collect()
    };
    let services_dir = std::path::Path::new("services");
    let domains: Vec<String> = if services_dir.is_dir() {
        orca_core::config::ServicesConfig::load_dir(services_dir)
            .ok()
            .map(collect_master_domains)
            .unwrap_or_default()
    } else {
        std::path::Path::new("services.toml")
            .exists()
            .then(|| orca_core::config::ServicesConfig::load("services.toml".as_ref()).ok())
            .flatten()
            .map(collect_master_domains)
            .unwrap_or_default()
    };
    info!(
        "Domain detection: {} master-placed domain(s) found (node={master_node})",
        domains.len()
    );

    // Install the proxy's security-header policy before serving (default-on
    // safe set — HSTS on HTTPS, nosniff, Referrer-Policy; backend-set headers
    // always win). Configurable via `[security_headers]` in cluster.toml.
    orca_proxy::init_security_headers(cluster_config.security_headers.clone());

    // Start proxy and get ACME components for hot cert provisioning.
    // Brought up whenever acme_email is configured, even with zero initial
    // domains — otherwise the first service deployed with a domain after
    // startup has no HTTPS listener, no resolver, and no ACME manager, and
    // certs can't be provisioned until a restart (mirrors the joined-node
    // proxy in join.rs, which always starts HTTP+HTTPS).
    let proxy_routes = route_table.clone();
    let proxy_triggers = wasm_triggers.clone();
    let (acme_for_control, resolver_for_control) = if let Some(email) = acme_email {
        let cache = std::env::var("HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|_| ".".into())
            .join(".orca/certs");
        let acme = orca_proxy::acme::AcmeManager::new(email, cache);
        let acme_clone = acme.clone();
        let fallback = cluster_config.fallback.clone();
        match orca_proxy::run_proxy_with_acme_and_fallback(
            proxy_routes,
            proxy_triggers,
            wasm_invoker,
            acme.clone(),
            domains,
            fallback,
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

    // Spawn ACME cert renewal background task (checks every 24h)
    if let (Some(acme), Some(resolver)) = (&acme_for_control, &resolver_for_control) {
        orca_proxy::acme::renewal::spawn_renewal_task(acme.clone(), resolver.clone());
        info!("ACME certificate renewal task spawned");
    }

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
    cleanup_port_redirects();
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

/// Master's identity for placement matching. Mirrors the helper in
/// `orca_control::api::handlers::ops::networks` so master and the API report
/// the same string.
fn master_hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .map(|s| s.trim().to_string())
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "master".to_string())
}

/// True if a service should be ACME-provisioned on master.
///
/// Returns true when placement is unconstrained (`None`) or explicitly targets
/// this master node. Returns false when placement targets a different node —
/// in that case the agent serves the cert, and master would deadlock on the
/// LE HTTP-01 challenge if it tried to provision (DNS for the domain points
/// at the agent, not the master).
fn is_master_placed(svc: &orca_core::config::ServiceConfig, master_hostname: &str) -> bool {
    match svc.placement.as_ref().and_then(|p| p.node.as_deref()) {
        Some(node) => node == master_hostname,
        None => true,
    }
}

/// Load cluster config, distinguishing "file absent" from "file broken".
///
/// - File absent: log a warning and return defaults. A fresh install with no
///   cluster.toml is a legitimate scenario; auto-generated tokens + zero-config
///   defaults give the operator a starting point.
/// - File present but malformed (parse error, schema mismatch, unreadable):
///   return an error so `orca server` aborts startup. Falling back to
///   defaults when the operator clearly intended a config to take effect is
///   how prod silently lost TLS on 2026-05-16 — a v0.2.9 schema landed in
///   cluster.toml while a v0.2.8 binary was running, parse failed, defaults
///   kicked in, `acme_email` went `None`, the proxy never started HTTPS.
fn load_cluster_config_or_fail(
    path: &std::path::Path,
) -> anyhow::Result<orca_core::config::ClusterConfig> {
    if path.exists() {
        orca_core::config::ClusterConfig::load(path).map_err(|e| {
            anyhow::anyhow!(
                "refusing to start: {e}\nFix the config or remove {} to start with defaults.",
                path.display()
            )
        })
    } else {
        tracing::warn!("Config file {} not found, using defaults", path.display());
        Ok(orca_core::config::ClusterConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orca_core::config::ServicesConfig;

    fn parse_svc(toml: &str) -> orca_core::config::ServiceConfig {
        let cfg: ServicesConfig = ::toml::from_str(toml).expect("valid toml");
        cfg.service.into_iter().next().expect("one service")
    }

    #[test]
    fn unconstrained_placement_is_master_placed() {
        let svc = parse_svc(
            r#"
            [[service]]
            name = "test"
            image = "nginx:latest"
            "#,
        );
        assert!(is_master_placed(&svc, "master-host"));
    }

    #[test]
    fn matching_placement_is_master_placed() {
        let svc = parse_svc(
            r#"
            [[service]]
            name = "test"
            image = "nginx:latest"
            [service.placement]
            node = "master-host"
            "#,
        );
        assert!(is_master_placed(&svc, "master-host"));
    }

    #[test]
    fn other_node_placement_is_not_master_placed() {
        let svc = parse_svc(
            r#"
            [[service]]
            name = "test"
            image = "nginx:latest"
            [service.placement]
            node = "zeroclaw"
            "#,
        );
        assert!(!is_master_placed(&svc, "master-host"));
    }

    #[test]
    fn placement_with_only_labels_is_master_placed() {
        let svc = parse_svc(
            r#"
            [[service]]
            name = "test"
            image = "nginx:latest"
            [service.placement]
            [service.placement.labels]
            zone = "us-east"
            "#,
        );
        assert!(is_master_placed(&svc, "master-host"));
    }

    #[test]
    fn missing_config_returns_defaults() {
        let path = std::path::Path::new("/tmp/orca-test-does-not-exist-xyz.toml");
        let cfg = load_cluster_config_or_fail(path).expect("missing file should use defaults");
        assert!(cfg.api_tokens.is_empty()); // default has no tokens
    }

    #[test]
    fn valid_config_loads() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            tmp.path(),
            r#"
[cluster]
name = "test"
acme_email = "ops@example.com"
"#,
        )
        .unwrap();
        let cfg = load_cluster_config_or_fail(tmp.path()).expect("valid config should load");
        assert_eq!(cfg.cluster.name, "test");
        assert_eq!(cfg.cluster.acme_email.as_deref(), Some("ops@example.com"));
    }

    #[test]
    fn malformed_config_aborts() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(tmp.path(), "this is not toml { [ broken").unwrap();
        let result = load_cluster_config_or_fail(tmp.path());
        assert!(
            result.is_err(),
            "malformed config must NOT fall back to defaults"
        );
        let err = format!("{}", result.unwrap_err());
        assert!(
            err.contains("refusing to start"),
            "error should make the abort reason explicit: {err}"
        );
    }

    #[test]
    fn type_mismatch_in_known_section_aborts() {
        // Mirrors the prod failure: cluster_meta.acme_email is `Option<String>`
        // but we hand it a table. serde rejects with "expected string, got map"
        // — exactly the v0.2.8 vs v0.2.9 [ai.alerts.channels.email] case.
        let tmp = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(
            tmp.path(),
            r#"
[cluster]
name = "test"
[cluster.acme_email]
nested = "shouldnt-be-a-table"
"#,
        )
        .unwrap();
        let result = load_cluster_config_or_fail(tmp.path());
        assert!(
            result.is_err(),
            "type mismatch must abort, not silently default"
        );
        let err = format!("{}", result.unwrap_err());
        assert!(
            err.contains("refusing to start"),
            "abort reason must be explicit: {err}"
        );
    }
}
