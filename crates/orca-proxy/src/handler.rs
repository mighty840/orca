//! HTTP request handler for the reverse proxy.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper::{Request, Response, StatusCode};
use tokio::sync::RwLock;
use tracing::{debug, error};

use crate::routing::find_matching_trigger;
use crate::{RouteTarget, SharedWasmTriggers, WasmInvoker};

/// Handle a single proxied request.
pub(crate) async fn handle_request(
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

/// Build a simple error response.
pub(crate) fn error_response(
    status: StatusCode,
    msg: &str,
) -> Response<http_body_util::Full<hyper::body::Bytes>> {
    let body = http_body_util::Full::new(hyper::body::Bytes::from(msg.to_string()));
    let mut resp = Response::new(body);
    *resp.status_mut() = status;
    resp
}
