//! Reverse proxy with HTTP routing for containers and Wasm trigger dispatch.
//!
//! Routes HTTP traffic by `Host` header to container backends (round-robin),
//! and by path pattern to Wasm component invocations via a callback.

pub mod acme;
mod handler;
mod routing;
pub mod tls;

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use hyper::Request;
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use acme::AcmeManager;
use handler::{handle_acme_challenge, handle_request};

/// A backend target for container routing.
#[derive(Debug, Clone)]
pub struct RouteTarget {
    /// Address in the form `ip:port`.
    pub address: String,
    /// Owning service name.
    pub service_name: String,
    /// Optional path pattern (e.g., `"/api/*"`). When `None`, this target is a
    /// catch-all for the domain. When `Some`, only requests whose path matches
    /// the pattern are routed here. Longest-prefix match wins.
    pub path_pattern: Option<String>,
}

/// A Wasm HTTP trigger: maps a path pattern to a Wasm runtime instance.
#[derive(Debug, Clone)]
pub struct WasmTrigger {
    /// Path pattern (e.g., "/api/edge/*").
    pub pattern: String,
    /// Wasm runtime instance ID.
    pub runtime_id: String,
    /// Service name for logging.
    pub service_name: String,
}

/// Callback invoked when a request matches a Wasm trigger.
/// Receives (runtime_id, method, path, body) and returns the response body string.
pub type WasmInvoker =
    Arc<dyn Fn(String, String, String, String) -> WasmInvokeFuture + Send + Sync>;

/// Future type returned by the Wasm invoker.
pub type WasmInvokeFuture =
    std::pin::Pin<Box<dyn std::future::Future<Output = Result<String, String>> + Send>>;

/// Shared Wasm trigger table type.
pub type SharedWasmTriggers = Arc<RwLock<Vec<WasmTrigger>>>;

/// Run the reverse proxy on the given port.
///
/// Routes by Host header to container backends, and by path pattern to Wasm components.
///
/// # Errors
///
/// Returns an error if the proxy fails to bind to the port.
pub async fn run_proxy(
    route_table: Arc<RwLock<HashMap<String, Vec<RouteTarget>>>>,
    wasm_triggers: SharedWasmTriggers,
    wasm_invoker: Option<WasmInvoker>,
    port: u16,
    tls_acceptor: Option<tokio_rustls::TlsAcceptor>,
    acme_manager: Option<AcmeManager>,
) -> anyhow::Result<()> {
    let addr = format!("0.0.0.0:{port}");
    let listener = TcpListener::bind(&addr).await?;
    let proto = if tls_acceptor.is_some() {
        "HTTPS"
    } else {
        "HTTP"
    };
    info!("Reverse proxy listening on {addr} ({proto})");

    let counter = Arc::new(AtomicUsize::new(0));
    let client = Arc::new(reqwest::Client::new());
    let acme = acme_manager.map(Arc::new);

    loop {
        let (stream, peer) = match listener.accept().await {
            Ok(conn) => conn,
            Err(e) => {
                warn!("Proxy accept error: {e}");
                continue;
            }
        };

        let routes = route_table.clone();
        let triggers = wasm_triggers.clone();
        let invoker = wasm_invoker.clone();
        let counter = counter.clone();
        let client = client.clone();
        let acme = acme.clone();

        let tls = tls_acceptor.clone();
        tokio::spawn(async move {
            let service = service_fn(move |req: Request<Incoming>| {
                let routes = routes.clone();
                let triggers = triggers.clone();
                let invoker = invoker.clone();
                let counter = counter.clone();
                let client = client.clone();
                let acme = acme.clone();
                async move {
                    // Intercept ACME challenge requests before normal routing
                    if let Some(resp) = handle_acme_challenge(&req, acme.as_deref()).await {
                        return Ok(resp);
                    }
                    handle_request(req, &routes, &triggers, invoker.as_ref(), &counter, &client)
                        .await
                }
            });

            if let Some(acceptor) = tls {
                match acceptor.accept(stream).await {
                    Ok(tls_stream) => {
                        let io = TokioIo::new(tls_stream);
                        if let Err(e) = http1::Builder::new().serve_connection(io, service).await {
                            debug!("TLS proxy error from {peer}: {e}");
                        }
                    }
                    Err(e) => debug!("TLS handshake failed from {peer}: {e}"),
                }
            } else {
                let io = TokioIo::new(stream);
                if let Err(e) = http1::Builder::new().serve_connection(io, service).await {
                    debug!("Proxy connection error from {peer}: {e}");
                }
            }
        });
    }
}
