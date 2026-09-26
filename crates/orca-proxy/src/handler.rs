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
use crate::body::{ProxyBody, full_body};
use crate::forward::{forward_streaming, forward_with_retry, redirect_to_https};
use crate::rate_limit::RateLimiter;
use crate::routing::{find_matching_trigger, select_path_targets};
use crate::{RouteTarget, SharedWasmTriggers, WasmInvoker};

/// Largest request body buffered so it can be replayed against a second
/// backend on a 502 (#190). Anything bigger, or of unknown size, is
/// streamed to one backend: a multi-GB registry push must never sit in the
/// proxy's memory, and retrying a large upload isn't safe anyway.
const RETRY_BUFFER_MAX: usize = 1024 * 1024;
/// Largest request body a Wasm trigger accepts; it is handed over in full.
const WASM_BODY_MAX: usize = 10 * 1024 * 1024;
use orca_core::config::FallbackConfig;

/// ACME challenge path prefix.
const ACME_CHALLENGE_PREFIX: &str = "/.well-known/acme-challenge/";

/// Synthesize a one-element `Vec<RouteTarget>` pointing at the configured
/// `fallback.http` target, used when the route table doesn't know about the
/// requested host (or path) and we want to forward to another reverse proxy
/// instead of returning 404. Returns `None` when no HTTP fallback is set.
fn fallback_target(fallback: Option<&FallbackConfig>) -> Option<RouteTarget> {
    let addr = fallback?.http.as_ref()?.clone();
    Some(RouteTarget {
        address: addr,
        service_name: "<fallback>".to_string(),
        path_pattern: None,
        weight: 100,
        strip_prefix: None,
    })
}

/// Handle ACME HTTP-01 challenge requests.
///
/// Returns `Some(response)` if the request is an ACME challenge, `None` otherwise.
pub(crate) async fn handle_acme_challenge(
    req: &Request<Incoming>,
    acme: Option<&AcmeManager>,
) -> Option<Response<ProxyBody>> {
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
        Some(authorization) => Some(Response::new(full_body(hyper::body::Bytes::from(
            authorization,
        )))),
        None => {
            // Also check the webroot directory for certbot-placed challenge files
            let webroot_path = format!("/tmp/orca-acme/.well-known/acme-challenge/{token}");
            match tokio::fs::read_to_string(&webroot_path).await {
                Ok(content) => Some(Response::new(full_body(hyper::body::Bytes::from(content)))),
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
    https_enabled: bool,
    rate_limiter: &RateLimiter,
    peer: SocketAddr,
    fallback: Option<&FallbackConfig>,
) -> Result<Response<ProxyBody>, hyper::Error> {
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

            // Capped: the body is handed to the trigger in full (#190).
            let limited = http_body_util::Limited::new(req.into_body(), WASM_BODY_MAX);
            let body_bytes = match limited.collect().await {
                Ok(collected) => collected.to_bytes(),
                Err(e) if e.is::<http_body_util::LengthLimitError>() => {
                    return Ok(error_response(
                        StatusCode::PAYLOAD_TOO_LARGE,
                        "request body too large for a Wasm trigger",
                    ));
                }
                Err(e) => {
                    error!("Failed to read request body: {e}");
                    return Ok(error_response(
                        StatusCode::BAD_GATEWAY,
                        "failed to read request body",
                    ));
                }
            };
            let body_str = String::from_utf8_lossy(&body_bytes).into_owned();

            match invoker(runtime_id, method, path, body_str).await {
                Ok(response_body) => {
                    return Ok(Response::new(full_body(hyper::body::Bytes::from(
                        response_body,
                    ))));
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

    // Extract the host: the Host header on HTTP/1.1, the `:authority`
    // pseudo-header (surfaced on the URI) on HTTP/2, which carries no Host.
    let host = req
        .headers()
        .get("host")
        .and_then(|h| h.to_str().ok())
        .or_else(|| req.uri().authority().map(|a| a.as_str()))
        .map(|h| h.split(':').next().unwrap_or(h).to_string());

    let Some(host) = host else {
        return Ok(error_response(
            StatusCode::BAD_REQUEST,
            "missing Host header",
        ));
    };

    // HTTP -> HTTPS redirect: if not TLS and host has routes, redirect —
    // but only when a TLS endpoint actually exists (#123: with ACME
    // unconfigured there is no HTTPS listener, so redirecting sends
    // clients into a wall). Take the read in a tight scope so the lock
    // releases before further awaits — see comment below.
    if !is_tls && https_enabled {
        let known = {
            let routes = route_table.read().await;
            routes.contains_key(&host)
        };
        if known {
            return Ok(redirect_to_https(&host, &path));
        }
    }

    // Resolve the request to either matched route targets or a synthetic
    // fallback target. When the route table doesn't know about this host (or
    // path) but a `fallback.http` is configured, build a one-element target
    // vec pointing at the fallback so the existing forward path handles it
    // uniformly. This is what makes orca able to coexist with another
    // reverse proxy on the same edge.
    //
    // Lock discipline: take a snapshot of the matched targets inside a
    // tight scope and release the read guard BEFORE any further await (body
    // collect / forward / websocket upgrade). Holding the guard across
    // those awaits lets a long-lived request (Matrix `/sync`'s 30s long-
    // poll, gitea CI runner long-polls) keep the lock for tens of seconds.
    // Once a writer queues behind that reader, tokio::sync::RwLock's
    // write-priority semantics make every new reader wait too — which on
    // a busy master manifests as 17-20-second stalls on the TLS handshake
    // path (the serve-loop also reads route_table before acceptor.accept).
    let lookup: Option<Vec<RouteTarget>> = {
        let routes = route_table.read().await;
        match routes.get(&host) {
            Some(targets) if targets.is_empty() => None, // explicit-empty sentinel
            Some(targets) => Some(select_path_targets(targets, &path)),
            None => Some(Vec::new()), // host unknown — try fallback
        }
    };
    let matched: Vec<RouteTarget> = match lookup {
        None => {
            return Ok(crate::error_page::branded_error(
                StatusCode::SERVICE_UNAVAILABLE,
                &host,
            ));
        }
        Some(sel) if sel.is_empty() => {
            if let Some(target) = fallback_target(fallback) {
                vec![target]
            } else {
                return Ok(crate::error_page::branded_error(
                    StatusCode::NOT_FOUND,
                    &host,
                ));
            }
        }
        Some(sel) => sel,
    };
    let base_idx = counter.fetch_add(1, Ordering::Relaxed);

    // WebSocket upgrade: tunnel via raw TCP instead of HTTP proxying
    if crate::websocket::is_websocket_upgrade(&req) {
        let idx = crate::forward::weighted_index(&matched, base_idx);
        let target = &matched[idx];
        debug!("WebSocket upgrade: {host}{path} -> {}", target.address);
        let client_ip = peer.ip().to_string();
        return Ok(crate::websocket::handle_websocket_proxy(
            req, target, &host, is_tls, &client_ip,
        )
        .await);
    }

    // Snapshot request metadata before consuming the body below.
    let method_reqwest: reqwest::Method = req.method().clone();
    let headers = req.headers().clone();
    let pq = req
        .uri()
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/")
        .to_string();

    // Stream the request body straight through unless it is small enough to
    // buffer for a 502 retry on a second backend: a single-target route has
    // no second backend, and a large body must never materialize in the
    // proxy task (#190; a streamed body is single-use, so it isn't retried).
    // A multi-target route buffers only a small body of known size, for the
    // 502 retry; everything else streams to one backend (#190).
    use hyper::body::Body as _;
    let small = req
        .body()
        .size_hint()
        .upper()
        .is_some_and(|n| n <= RETRY_BUFFER_MAX as u64);
    let resp = if matched.len() == 1 || !small {
        let target = if matched.len() == 1 {
            &matched[0]
        } else {
            &matched[crate::forward::weighted_index(&matched, base_idx)]
        };
        forward_streaming(
            client,
            target,
            &method_reqwest,
            &headers,
            req.into_body(),
            &pq,
            &host,
            is_tls,
            peer.ip().to_string(),
        )
        .await
    } else {
        // `Limited` guards against a body longer than its declared size.
        let limited = http_body_util::Limited::new(req.into_body(), RETRY_BUFFER_MAX);
        let body_bytes = match limited.collect().await {
            Ok(collected) => collected.to_bytes(),
            Err(e) => {
                error!("Failed to read request body: {e}");
                return Ok(error_response(
                    StatusCode::BAD_GATEWAY,
                    "failed to read request body",
                ));
            }
        };
        forward_with_retry(
            client,
            &matched,
            base_idx,
            &method_reqwest,
            &headers,
            &body_bytes,
            &pq,
            &host,
            is_tls,
            peer.ip().to_string(),
        )
        .await
    };

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
pub(crate) fn error_response(status: StatusCode, msg: &str) -> Response<ProxyBody> {
    let mut resp = Response::new(full_body(hyper::body::Bytes::from(msg.to_string())));
    *resp.status_mut() = status;
    resp
}
