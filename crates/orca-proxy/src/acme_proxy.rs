//! The proxy's HTTP (ACME challenges, redirect) and HTTPS listeners with
//! automatic Let's Encrypt certificates.

use std::collections::HashMap;
use std::sync::Arc;

use tokio::net::TcpListener;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

use crate::acme::{self, AcmeManager};
use crate::{
    FallbackConfig, RouteTarget, SharedCertResolver, SharedWasmTriggers, WasmInvoker,
    serve_loop_with_fallback, tls,
};

/// Run HTTP on port 80 (for ACME challenges + redirect) and HTTPS on port 443.
///
/// Automatically provisions certs for all given domains via Let's Encrypt.
/// Returns a `SharedCertResolver` that can be used to hot-provision certs
/// for new domains added later via `orca deploy`.
pub async fn run_proxy_with_acme(
    route_table: Arc<RwLock<HashMap<String, Vec<RouteTarget>>>>,
    wasm_triggers: SharedWasmTriggers,
    wasm_invoker: Option<WasmInvoker>,
    acme_manager: AcmeManager,
    domains: Vec<String>,
) -> anyhow::Result<SharedCertResolver> {
    run_proxy_with_acme_and_fallback(
        route_table,
        wasm_triggers,
        wasm_invoker,
        acme_manager,
        domains,
        None,
    )
    .await
}

/// Run HTTP+HTTPS with ACME and optional fallback to another reverse proxy.
#[allow(clippy::too_many_arguments)]
pub async fn run_proxy_with_acme_and_fallback(
    route_table: Arc<RwLock<HashMap<String, Vec<RouteTarget>>>>,
    wasm_triggers: SharedWasmTriggers,
    wasm_invoker: Option<WasmInvoker>,
    acme_manager: AcmeManager,
    domains: Vec<String>,
    fallback: Option<FallbackConfig>,
) -> anyhow::Result<SharedCertResolver> {
    run_acme_proxy_on(
        (80, 443),
        route_table,
        wasm_triggers,
        wasm_invoker,
        acme_manager,
        domains,
        fallback,
    )
    .await
}

/// [`run_proxy_with_acme_and_fallback`] on the given (HTTP, HTTPS) ports.
///
/// Both ports are bound before anything is spawned, and a failure is
/// returned (#189). Binding used to happen inside the background tasks, so
/// a port that couldn't be bound was one error line while startup reported
/// success. With port 80 gone, every certificate order fails and the HTTPS
/// redirect disappears.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn run_acme_proxy_on(
    (http_port, https_port): (u16, u16),
    route_table: Arc<RwLock<HashMap<String, Vec<RouteTarget>>>>,
    wasm_triggers: SharedWasmTriggers,
    wasm_invoker: Option<WasmInvoker>,
    acme_manager: AcmeManager,
    domains: Vec<String>,
    fallback: Option<FallbackConfig>,
) -> anyhow::Result<SharedCertResolver> {
    let http_listener = TcpListener::bind(("0.0.0.0", http_port))
        .await
        .map_err(|e| {
            anyhow::anyhow!(
                "cannot listen on port {http_port} ({e}): ACME HTTP-01 validation and the \
                 HTTP-to-HTTPS redirect need it, so no certificate can be issued"
            )
        })?;
    let https_listener = TcpListener::bind(("0.0.0.0", https_port))
        .await
        .map_err(|e| {
            anyhow::anyhow!(
                "cannot listen on port {https_port} ({e}): no HTTPS traffic can be served"
            )
        })?;
    info!("Reverse proxy listening on 0.0.0.0:{http_port} (HTTP) and 0.0.0.0:{https_port} (HTTPS)");
    // With a fallback certificate, so a handshake without SNI or for a
    // domain with no certificate yet still reaches HTTP (#206).
    let resolver = Arc::new(acme::DynCertResolver::with_fallback()?);

    let acme_mgr = acme_manager.clone();
    let routes_clone = route_table.clone();
    let triggers_clone = wasm_triggers.clone();
    let invoker_clone = wasm_invoker.clone();
    let fallback_http = fallback.clone();
    let fallback_tls = fallback.clone();

    // Start HTTP on port 80 first (needed for ACME challenge validation)
    let http_handle = tokio::spawn({
        let acme = acme_mgr.clone();
        let routes = routes_clone.clone();
        let triggers = triggers_clone.clone();
        let invoker = invoker_clone.clone();
        async move {
            if let Err(e) = serve_loop_with_fallback(
                http_listener,
                routes,
                triggers,
                invoker,
                None,
                Some(acme),
                fallback_http,
            )
            .await
            {
                error!("HTTP listener failed: {e}");
            }
        }
    });

    // Provision certs for initial domains, then start HTTPS with SNI resolver
    let resolver_clone = resolver.clone();
    let https_handle = tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        // Provision all initial domain certs. Each call gets its own 60s
        // timeout: without it, a single domain whose LE HTTP-01 challenge
        // hangs (DNS pointing elsewhere, port 80 firewalled, LE rate limit
        // backoff) blocks the entire HTTPS listener startup forever. 60s is
        // generous — a healthy LE order completes in 5-15s — so a timeout
        // here is a real problem, but we'd rather serve the other domains
        // than serve nothing.
        const PER_DOMAIN_PROVISION_TIMEOUT: std::time::Duration =
            std::time::Duration::from_secs(60);
        for domain in &domains {
            // Register with the manager first: the renewal task's 24h sweep
            // and fast-retry loop only iterate registered domains, so a
            // failed or timed-out provision here is retried instead of
            // staying broken until the next restart.
            acme_mgr.add_domain(domain).await;
            let fut = acme_mgr.ensure_cert_for_resolver(domain, &resolver_clone);
            match tokio::time::timeout(PER_DOMAIN_PROVISION_TIMEOUT, fut).await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    error!(domain = %domain, error = %e, "Failed to provision cert");
                }
                Err(_) => {
                    warn!(
                        domain = %domain,
                        timeout_secs = PER_DOMAIN_PROVISION_TIMEOUT.as_secs(),
                        "Cert provisioning timed out — skipping (HTTPS will start without this cert; reconciler may retry on demand)"
                    );
                }
            }
        }

        // Build TlsAcceptor with SNI resolver for multi-domain support
        let config = tls::with_h2_alpn(
            rustls::ServerConfig::builder()
                .with_no_client_auth()
                .with_cert_resolver(resolver_clone),
        );

        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
        info!(
            "Starting HTTPS with SNI resolver ({} domains)",
            domains.len()
        );

        let routes = routes_clone;
        let triggers = triggers_clone;
        let invoker = invoker_clone;
        if let Err(e) = serve_loop_with_fallback(
            https_listener,
            routes,
            triggers,
            invoker,
            Some(acceptor),
            Some(acme_mgr),
            fallback_tls,
        )
        .await
        {
            error!("HTTPS listener failed: {e}");
        }
    });

    // Don't block — return the resolver so the control plane can hot-add certs.
    // The HTTP and HTTPS listeners run in the background.
    tokio::spawn(async move {
        tokio::select! {
            _ = http_handle => warn!("HTTP listener exited"),
            _ = https_handle => warn!("HTTPS listener exited"),
        }
    });

    Ok(resolver)
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn start_on(ports: (u16, u16)) -> anyhow::Result<SharedCertResolver> {
        let cache = tempfile::tempdir().unwrap();
        run_acme_proxy_on(
            ports,
            Arc::new(RwLock::new(HashMap::new())),
            Arc::new(RwLock::new(Vec::new())),
            None,
            AcmeManager::new("ops@example.com", cache.path()),
            Vec::new(),
            None,
        )
        .await
    }

    fn free_port() -> u16 {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port()
    }

    /// #189: an HTTP port that can't be bound must be reported, not
    /// swallowed in a background task while startup says OK.
    #[tokio::test]
    async fn a_taken_http_port_is_an_error_naming_the_consequence() {
        let held = std::net::TcpListener::bind("0.0.0.0:0").unwrap();
        let port = held.local_addr().unwrap().port();

        let Err(err) = start_on((port, free_port())).await else {
            panic!("must fail");
        };

        let msg = format!("{err:#}");
        assert!(msg.contains(&format!("port {port}")), "{msg}");
        assert!(msg.contains("no certificate can be issued"), "{msg}");
    }

    #[tokio::test]
    async fn a_taken_https_port_is_an_error_too() {
        let held = std::net::TcpListener::bind("0.0.0.0:0").unwrap();
        let port = held.local_addr().unwrap().port();

        assert!(start_on((free_port(), port)).await.is_err());
    }

    #[tokio::test]
    async fn free_ports_start() {
        assert!(start_on((free_port(), free_port())).await.is_ok());
    }
}
