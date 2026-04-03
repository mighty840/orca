//! HTTP request handler for the reverse proxy.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use http_body_util::BodyExt;
use hyper::body::Incoming;
use hyper::{Request, Response, StatusCode};
use tokio::sync::RwLock;
use tracing::{debug, error, info};

use crate::acme::AcmeManager;
use crate::forward::{forward_with_retry, redirect_to_https};
use crate::rate_limit::RateLimiter;
use crate::routing::{find_matching_trigger, select_path_targets};
use crate::{RouteTarget, SharedWasmTriggers, WasmInvoker};

/// ACME challenge path prefix.
const ACME_CHALLENGE_PREFIX: &str = "/.well-known/acme-challenge/";

/// Handle ACME HTTP-01 challenge requests.
///
/// Returns `Some(response)` if the request is an ACME challenge, `None` otherwise.
pub(crate) async fn handle_acme_challenge(
    req: &Request<Incoming>,
    acme: Option<&AcmeManager>,
) -> Option<Response<http_body_util::Full<hyper::body::Bytes>>> {
    let path = req.uri().path();
    if !path.starts_with(ACME_CHALLENGE_PREFIX) {
        return None;
    }

    let token = &path[ACME_CHALLENGE_PREFIX.len()..];
    debug!("ACME challenge request for token: {token}");

    let Some(manager) = acme else {
        return Some(error_response(StatusCode::NOT_FOUND, "ACME not configured"));
    };

    match manager.get_challenge_response(token).await {
        Some(authorization) => {
            let body = http_body_util::Full::new(hyper::body::Bytes::from(authorization));
            Some(Response::new(body))
        }
        None => {
            // Also check the webroot directory for certbot-placed challenge files
            let webroot_path = format!("/tmp/orca-acme/.well-known/acme-challenge/{token}");
            match tokio::fs::read_to_string(&webroot_path).await {
                Ok(content) => {
                    let body = http_body_util::Full::new(hyper::body::Bytes::from(content));
                    Some(Response::new(body))
                }
                Err(_) => Some(error_response(
                    StatusCode::NOT_FOUND,
                    "ACME challenge token not found",
                )),
            }
        }
    }
}

/// Handle a single proxied request.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn handle_request(
    req: Request<Incoming>,
    route_table: &Arc<RwLock<HashMap<String, Vec<RouteTarget>>>>,
    wasm_triggers: &SharedWasmTriggers,
    wasm_invoker: Option<&WasmInvoker>,
    counter: &Arc<AtomicUsize>,
    client: &Arc<reqwest::Client>,
    is_tls: bool,
    rate_limiter: &RateLimiter,
    peer: SocketAddr,
) -> Result<Response<http_body_util::Full<hyper::body::Bytes>>, hyper::Error> {
    let start = Instant::now();
    let path = req.uri().path().to_string();
    let method = req.method().to_string();

    // Rate limiting per IP
    if !rate_limiter.check(peer.ip()) {
        return Ok(error_response(
            StatusCode::TOO_MANY_REQUESTS,
            "rate limit exceeded",
        ));
    }

    // Check Wasm triggers first (path-based routing takes priority)
    if let Some(invoker) = wasm_invoker {
        let triggers = wasm_triggers.read().await;
        if let Some(trigger) = find_matching_trigger(&triggers, &path) {
            let runtime_id = trigger.runtime_id.clone();
            let service_name = trigger.service_name.clone();
            drop(triggers);

            debug!("Wasm trigger matched: {path} -> {service_name}");

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

    // Extract host header
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

    // HTTP -> HTTPS redirect: if not TLS and host has routes, redirect
    if !is_tls {
        let routes = route_table.read().await;
        if routes.contains_key(&host) {
            drop(routes);
            return Ok(redirect_to_https(&host, &path));
        }
    }

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

    let matched = select_path_targets(targets, &path);
    if matched.is_empty() {
        return Ok(error_response(
            StatusCode::NOT_FOUND,
            &format!("no backend for path: {path} on host: {host}"),
        ));
    }
    let base_idx = counter.fetch_add(1, Ordering::Relaxed);
    drop(routes);

    // WebSocket upgrade: tunnel via raw TCP instead of HTTP proxying
    if crate::websocket::is_websocket_upgrade(&req) {
        let idx = crate::forward::weighted_index(&matched, base_idx);
        let target = &matched[idx];
        debug!("WebSocket upgrade: {host}{path} -> {}", target.address);
        return Ok(crate::websocket::handle_websocket_proxy(req, &target.address).await);
    }

    // Read body once for forwarding (and potential retry)
    let method_reqwest: reqwest::Method = req.method().clone();
    let headers = req.headers().clone();
    let pq = req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/")
        .to_string();

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

    // Forward with retry on 502
    let resp = forward_with_retry(
        client,
        &matched,
        base_idx,
        &method_reqwest,
        &headers,
        &body_bytes,
        &pq,
        &host,
    )
    .await;

    let elapsed_ms = start.elapsed().as_millis();
    let status = resp.status().as_u16();
    info!(
        method = %method,
        host = %host,
        path = %path,
        status = status,
        latency_ms = elapsed_ms,
        "proxy request"
    );

    Ok(resp)
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
