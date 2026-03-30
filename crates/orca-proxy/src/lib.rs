//! Simple reverse proxy that routes HTTP requests by `Host` header to backend containers.
//!
//! For M0 this is a straightforward hyper-based proxy. It shares a route table
//! with the control plane's reconciler via `Arc<RwLock<HashMap<String, Vec<RouteTarget>>>>`.

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

/// A backend target for routing.
#[derive(Debug, Clone)]
pub struct RouteTarget {
    /// Address in the form `ip:port`.
    pub address: String,
    /// Owning service name.
    pub service_name: String,
}

/// Run the reverse proxy on the given port.
///
/// The `route_table` is shared with the reconciler. Domains map to a list of
/// backend addresses; the proxy round-robins across them.
///
/// # Errors
///
/// Returns an error if the proxy fails to bind to the port.
pub async fn run_proxy(
    route_table: Arc<RwLock<HashMap<String, Vec<RouteTarget>>>>,
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
        let counter = counter.clone();
        let client = client.clone();

        tokio::spawn(async move {
            let io = TokioIo::new(stream);
            let service = service_fn(move |req: Request<Incoming>| {
                let routes = routes.clone();
                let counter = counter.clone();
                let client = client.clone();
                async move { handle_request(req, &routes, &counter, &client).await }
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
    counter: &Arc<AtomicUsize>,
    client: &Arc<reqwest::Client>,
) -> Result<Response<http_body_util::Full<hyper::body::Bytes>>, hyper::Error> {
    // Extract the Host header
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

    // Look up backends
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

    // Build the forwarded request using reqwest
    let method = req.method().clone();
    let headers = req.headers().clone();

    // Read the incoming body
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

    let mut forward_req = client.request(method, &uri);
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
