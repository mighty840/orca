//! Reverse proxy with HTTP routing for containers and Wasm trigger dispatch.
//!
//! Routes HTTP traffic by `Host` header to container backends (round-robin),
//! and by path pattern to Wasm component invocations via a callback.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::net::TcpListener;
use tokio::sync::RwLock;
use tracing::{debug, error, info, warn};

/// A backend target for container routing.
#[derive(Debug, Clone)]
pub struct RouteTarget {
    /// Address in the form `ip:port`.
    pub address: String,
    /// Owning service name.
    pub service_name: String,
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
) -> anyhow::Result<()> {
    let addr = format!("0.0.0.0:{port}");
    let listener = TcpListener::bind(&addr).await?;
    info!("Reverse proxy listening on {addr}");

    let counter = Arc::new(AtomicUsize::new(0));
    let client = Arc::new(reqwest::Client::new());

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

        tokio::spawn(async move {
            let io = TokioIo::new(stream);
            let service = service_fn(move |req: Request<Incoming>| {
                let routes = routes.clone();
                let triggers = triggers.clone();
                let invoker = invoker.clone();
                let counter = counter.clone();
                let client = client.clone();
                async move {
                    handle_request(req, &routes, &triggers, invoker.as_ref(), &counter, &client)
                        .await
                }
            });

            if let Err(e) = http1::Builder::new().serve_connection(io, service).await {
                debug!("Proxy connection error from {peer}: {e}");
            }
        });
    }
}

/// Handle a single proxied request.
async fn handle_request(
    req: Request<Incoming>,
    route_table: &Arc<RwLock<HashMap<String, Vec<RouteTarget>>>>,
    wasm_triggers: &SharedWasmTriggers,
    wasm_invoker: Option<&WasmInvoker>,
    counter: &Arc<AtomicUsize>,
    client: &Arc<reqwest::Client>,
) -> Result<Response<http_body_util::Full<hyper::body::Bytes>>, hyper::Error> {
    let path = req.uri().path().to_string();
    let method = req.method().to_string();

    // Check Wasm triggers first (path-based routing takes priority)
    if let Some(invoker) = wasm_invoker {
        let triggers = wasm_triggers.read().await;
        if let Some(trigger) = find_matching_trigger(&triggers, &path) {
            let runtime_id = trigger.runtime_id.clone();
            let service_name = trigger.service_name.clone();
            drop(triggers);

            debug!("Wasm trigger matched: {path} -> {service_name}");

            // Read request body
            use http_body_util::BodyExt;
            let body_bytes = match req.into_body().collect().await {
                Ok(collected) => collected.to_bytes(),
                Err(e) => {
                    error!("Failed to read request body: {e}");
                    return Ok(error_response(
                        StatusCode::BAD_GATEWAY,
                        "failed to read request body",
                    ));
                }
            };
            let body_str = String::from_utf8_lossy(&body_bytes).to_string();

            // Invoke the Wasm component
            match invoker(runtime_id, method, path, body_str).await {
                Ok(response_body) => {
                    let body = http_body_util::Full::new(hyper::body::Bytes::from(response_body));
                    return Ok(Response::new(body));
                }
                Err(e) => {
                    error!("Wasm invocation failed: {e}");
                    return Ok(error_response(
                        StatusCode::INTERNAL_SERVER_ERROR,
                        &format!("wasm error: {e}"),
                    ));
                }
            }
        }
    }

    // Fall through to Host-based container routing
    let host = req
        .headers()
        .get("host")
        .and_then(|h| h.to_str().ok())
        .map(|h| h.split(':').next().unwrap_or(h).to_string());

    let Some(host) = host else {
        return Ok(error_response(
            StatusCode::BAD_REQUEST,
            "missing Host header",
        ));
    };

    let routes = route_table.read().await;
    let Some(targets) = routes.get(&host) else {
        return Ok(error_response(
            StatusCode::NOT_FOUND,
            &format!("no service for host: {host}"),
        ));
    };

    if targets.is_empty() {
        return Ok(error_response(
            StatusCode::SERVICE_UNAVAILABLE,
            &format!("no backends for host: {host}"),
        ));
    }

    // Round-robin selection
    let idx = counter.fetch_add(1, Ordering::Relaxed) % targets.len();
    let target = targets[idx].clone();
    drop(routes);

    // Forward the request
    let uri = format!(
        "http://{}{}",
        target.address,
        req.uri()
            .path_and_query()
            .map(|pq| pq.as_str())
            .unwrap_or("/")
    );

    debug!("Proxying {host}{} -> {uri}", req.uri().path());

    let method_reqwest = req.method().clone();
    let headers = req.headers().clone();

    use http_body_util::BodyExt;
    let body_bytes = match req.into_body().collect().await {
        Ok(collected) => collected.to_bytes(),
        Err(e) => {
            error!("Failed to read request body: {e}");
            return Ok(error_response(
                StatusCode::BAD_GATEWAY,
                "failed to read request body",
            ));
        }
    };

    let mut forward_req = client.request(method_reqwest, &uri);
    for (key, value) in &headers {
        if key != "host" {
            forward_req = forward_req.header(key, value);
        }
    }
    forward_req = forward_req.body(body_bytes);

    match forward_req.send().await {
        Ok(resp) => {
            let status = resp.status();
            let resp_body = resp.bytes().await.unwrap_or_default();
            let mut response = Response::new(http_body_util::Full::new(resp_body));
            *response.status_mut() = status;
            Ok(response)
        }
        Err(e) => {
            error!("Proxy error to {}: {e}", target.address);
            Ok(error_response(
                StatusCode::BAD_GATEWAY,
                &format!("backend error: {e}"),
            ))
        }
    }
}

/// Find a Wasm trigger matching the given path.
fn find_matching_trigger<'a>(triggers: &'a [WasmTrigger], path: &str) -> Option<&'a WasmTrigger> {
    triggers.iter().find(|t| path_matches(&t.pattern, path))
}

/// Simple glob-like path matching: "/api/edge/*" matches "/api/edge/foo/bar".
fn path_matches(pattern: &str, path: &str) -> bool {
    if let Some(prefix) = pattern.strip_suffix('*') {
        path.starts_with(prefix)
    } else {
        path == pattern
    }
}

/// Build a simple error response.
fn error_response(
    status: StatusCode,
    msg: &str,
) -> Response<http_body_util::Full<hyper::body::Bytes>> {
    let body = http_body_util::Full::new(hyper::body::Bytes::from(msg.to_string()));
    let mut resp = Response::new(body);
    *resp.status_mut() = status;
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_path_matches_exact() {
        assert!(path_matches("/api/health", "/api/health"));
        assert!(!path_matches("/api/health", "/api/status"));
    }

    #[test]
    fn test_path_matches_wildcard() {
        assert!(path_matches("/api/edge/*", "/api/edge/foo"));
        assert!(path_matches("/api/edge/*", "/api/edge/foo/bar"));
        assert!(path_matches("/api/edge/*", "/api/edge/"));
        assert!(!path_matches("/api/edge/*", "/api/other/foo"));
    }

    #[test]
    fn test_path_matches_root_wildcard() {
        assert!(path_matches("/*", "/anything"));
        assert!(path_matches("/*", "/"));
    }
}
