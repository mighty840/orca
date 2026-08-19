//! Backend forwarding with retry logic and HTTPS redirect helpers.

use http_body_util::BodyDataStream;
use hyper::body::Incoming;
use hyper::{Response, StatusCode};
use tracing::{debug, error, warn};

use crate::RouteTarget;
use crate::body::{ProxyBody, full_body, stream_body};

/// First-byte latency beyond which the backend is called out explicitly.
///
/// A backend that stalls until the client read timeout otherwise surfaces
/// only as a proxy-side 502, which reads as a proxy fault. 2026-08-10:
/// black-holed session-store connections made login POSTs hang for the full
/// 120s read timeout, and the resulting 502s were initially blamed on the
/// proxy. 30s is far above any healthy backend's first byte but well below
/// the read timeout, so the log points at the backend while the request is
/// still pending.
const SLOW_BACKEND_WARN_AFTER: std::time::Duration = std::time::Duration::from_secs(30);

/// Await `send()`, emitting a warning *while the request is still pending* if
/// the backend takes longer than [`SLOW_BACKEND_WARN_AFTER`] to respond, then
/// continuing to wait for the eventual result. Racing the send against a timer
/// (rather than measuring after it resolves) means the log lands at the 30s
/// mark during a live stall — not at ~120s alongside the read-timeout 502,
/// where it would add nothing over the error itself.
async fn send_with_slow_warn(
    forward_req: reqwest::RequestBuilder,
    address: &str,
    path_and_query: &str,
) -> Result<reqwest::Response, reqwest::Error> {
    let send = forward_req.send();
    tokio::pin!(send);
    tokio::select! {
        result = &mut send => result,
        _ = tokio::time::sleep(SLOW_BACKEND_WARN_AFTER) => {
            warn!(
                backend = %address,
                path = %path_and_query,
                threshold_secs = SLOW_BACKEND_WARN_AFTER.as_secs(),
                "slow backend: no response after threshold and still waiting — \
                 if this surfaces as a 502, the backend stalled, not the proxy"
            );
            send.await
        }
    }
}

/// Select a target index using weighted round-robin.
///
/// When all weights are equal (the common case), falls back to simple
/// round-robin. Otherwise uses the counter to distribute requests
/// proportionally to each target's weight.
pub(crate) fn weighted_index(targets: &[RouteTarget], counter: usize) -> usize {
    if targets.len() <= 1 {
        return 0;
    }
    // Fast path: all weights equal -> simple round-robin
    let first_w = targets[0].weight;
    if targets.iter().all(|t| t.weight == first_w) {
        return counter % targets.len();
    }
    // Weighted round-robin: map counter into total weight space
    let total: u32 = targets.iter().map(|t| t.weight).sum();
    if total == 0 {
        return counter % targets.len();
    }
    let pos = (counter as u32) % total;
    let mut cumulative = 0u32;
    for (i, t) in targets.iter().enumerate() {
        cumulative += t.weight;
        if pos < cumulative {
            return i;
        }
    }
    targets.len() - 1
}

/// Apply per-target prefix stripping so backends that expect a clean URL
/// (e.g. `/users`) don't see the full routed path (e.g. `/admin/users`).
/// No-op when `strip_prefix` is `None`.
fn strip_target_prefix<'a>(
    target: &RouteTarget,
    path_and_query: &'a str,
) -> std::borrow::Cow<'a, str> {
    match &target.strip_prefix {
        Some(prefix) if path_and_query.starts_with(prefix.as_str()) => {
            let rest = &path_and_query[prefix.len()..];
            if rest.is_empty() {
                std::borrow::Cow::Borrowed("/")
            } else if rest.starts_with('/') {
                std::borrow::Cow::Borrowed(rest)
            } else {
                std::borrow::Cow::Owned(format!("/{rest}"))
            }
        }
        _ => std::borrow::Cow::Borrowed(path_and_query),
    }
}

/// Build the forwarded reqwest request for `target` — method, URI (with
/// prefix stripping), client headers, and the X-Forwarded-* set — but WITHOUT
/// a body attached. Shared by the buffered retry path and the single-target
/// streaming path. Returns the builder and the resolved upstream URI (for
/// logging). The caller attaches the body.
#[allow(clippy::too_many_arguments)]
fn build_forward_request(
    client: &reqwest::Client,
    target: &RouteTarget,
    method: &reqwest::Method,
    headers: &hyper::HeaderMap,
    path_and_query: &str,
    host: &str,
    is_tls: bool,
    client_ip: &str,
) -> (reqwest::RequestBuilder, String) {
    let forwarded_path = strip_target_prefix(target, path_and_query);
    let uri = format!("http://{}{}", target.address, forwarded_path);

    let mut forward_req = client.request(method.clone(), &uri);
    let mut incoming_xff: Option<String> = None;
    let mut saw_proto = false;
    let mut saw_fhost = false;
    for (key, value) in headers {
        let name = key.as_str().to_lowercase();
        if name == "host" {
            // Skip the original Host header — reqwest sets it from URI.
            // We preserve the original host via X-Forwarded-Host below,
            // but also set Host to the original so backends (litellm,
            // keycloak) that build redirect URLs from Host see the
            // external domain, not the internal 127.0.0.1:port.
            continue;
        }
        if name == "content-length" || name == "transfer-encoding" {
            // Body framing is reqwest's job: it sets Content-Length from a
            // buffered `Bytes` body, or chunked Transfer-Encoding for the
            // streamed (`wrap_stream`) single-target path. Forwarding the
            // client's framing headers double-sets Content-Length on the
            // buffered path and, worse, pairs a stale Content-Length with a
            // chunked body on the streaming path (a framing conflict that
            // can truncate or stall large registry blob pushes).
            continue;
        }
        if name == "x-forwarded-for" {
            // Capture but do NOT forward the client's value verbatim. A client
            // can send any X-Forwarded-For it likes; passing it through would
            // let it choose the address downstream consumers key on (rate
            // limits, one-vote-per-visitor). We re-emit a single header below
            // with our observed peer appended, so the right-most entry — the
            // one hop-counting consumers read — is always the address WE saw.
            //
            // Accumulate across repeated header lines rather than overwriting:
            // RFC 7230 allows a header to arrive as several lines, each a
            // segment of the same chain, so joining preserves a legitimate
            // multi-hop chain (only the last line would otherwise survive).
            if let Ok(v) = value.to_str() {
                incoming_xff = Some(match incoming_xff.take() {
                    Some(prev) => format!("{prev}, {v}"),
                    None => v.to_string(),
                });
            }
            continue;
        } else if name == "x-forwarded-proto" {
            saw_proto = true;
        } else if name == "x-forwarded-host" {
            saw_fhost = true;
        }
        forward_req = forward_req.header(key, value);
    }
    // Override the Host header to the original external host so backends that
    // build redirect URLs from Host (litellm /ui, keycloak OIDC) use the
    // correct public-facing domain.
    forward_req = forward_req.header("Host", host);
    // Inject X-Forwarded-* for upstream apps behind TLS termination.
    let scheme = if is_tls { "https" } else { "http" };
    if !saw_proto {
        forward_req = forward_req.header("X-Forwarded-Proto", scheme);
    }
    if !saw_fhost {
        forward_req = forward_req.header("X-Forwarded-Host", host);
    }
    // X-Forwarded-For: append the peer we actually saw to any prior chain and
    // emit it as ONE header. Consumers count hops in from the right, so the
    // appended peer is authoritative and a client-supplied prefix is inert.
    // (A second header line would not do — HeaderMap::get reads the first, so
    // the client's forged value would win; this replaces rather than adds.)
    forward_req = forward_req.header(
        "X-Forwarded-For",
        forwarded_for_value(incoming_xff.as_deref(), client_ip),
    );
    (forward_req, uri)
}

/// Build the outgoing `X-Forwarded-For` value: the peer address we observed,
/// appended to whatever chain arrived (if any non-empty one did). Kept pure so
/// the append semantics are unit-testable without a live client.
fn forwarded_for_value(incoming: Option<&str>, client_ip: &str) -> String {
    match incoming.map(str::trim).filter(|s| !s.is_empty()) {
        Some(prev) => format!("{prev}, {client_ip}"),
        None => client_ip.to_string(),
    }
}

/// Convert an upstream reqwest response into the proxy's streaming response,
/// copying backend headers (minus hop-by-hop) and streaming the body straight
/// through instead of buffering with `resp.bytes().await`. For a Docker
/// registry blob pull (100MB+) buffering doubles end-to-end latency AND parks
/// ~100MB in the proxy task — when several pile up, the accept loop loses the
/// headroom to complete new TLS handshakes (`docker login`, browser hits) and
/// they time out. See the v0.2.9-rc.2 streaming-bodies memory.
fn build_response(resp: reqwest::Response) -> Response<ProxyBody> {
    let status = resp.status();
    let backend_headers = resp.headers().clone();
    let body = stream_body(resp.bytes_stream());
    let mut response = Response::new(body);
    *response.status_mut() = status;
    // Forward backend headers (skip hop-by-hop only). content-length is
    // preserved — clients need it.
    for (k, v) in backend_headers.iter() {
        let name = k.as_str().to_lowercase();
        if !matches!(
            name.as_str(),
            "connection"
                | "keep-alive"
                | "proxy-authenticate"
                | "proxy-authorization"
                | "te"
                | "trailers"
                | "transfer-encoding"
                | "upgrade"
        ) {
            response.headers_mut().append(k.clone(), v.clone());
        }
    }
    response
}

/// Forward a request to a backend, retrying once on 502 with a different
/// backend if multiple exist. Buffers the request body (`body`) so it can be
/// replayed on retry — used for multi-target routes. Single-target routes use
/// [`forward_streaming`] instead, which avoids buffering entirely.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn forward_with_retry(
    client: &reqwest::Client,
    matched: &[RouteTarget],
    base_idx: usize,
    method: &reqwest::Method,
    headers: &hyper::HeaderMap,
    body: &hyper::body::Bytes,
    path_and_query: &str,
    host: &str,
    is_tls: bool,
    client_ip: String,
) -> Response<ProxyBody> {
    let max_attempts = if matched.len() > 1 { 2 } else { 1 };

    for attempt in 0..max_attempts {
        let idx = if attempt == 0 {
            weighted_index(matched, base_idx)
        } else {
            // Retry with next backend on failure
            (weighted_index(matched, base_idx) + 1) % matched.len()
        };
        let target = &matched[idx];
        let (forward_req, uri) = build_forward_request(
            client,
            target,
            method,
            headers,
            path_and_query,
            host,
            is_tls,
            &client_ip,
        );
        debug!("Proxying {host}{path_and_query} -> {uri} (attempt {attempt})");
        let forward_req = forward_req.body(body.clone());

        match send_with_slow_warn(forward_req, &target.address, path_and_query).await {
            Ok(resp) if resp.status() == StatusCode::BAD_GATEWAY && attempt + 1 < max_attempts => {
                debug!("Got 502 from {}, retrying", target.address);
                continue;
            }
            // 502 retry is preserved: status is checked above before we
            // consume the body, so a 502 falls through to the next attempt
            // without writing anything to the client.
            Ok(resp) => return build_response(resp),
            Err(e) if attempt + 1 < max_attempts => {
                debug!("Backend error from {}: {e}, retrying", target.address);
                continue;
            }
            Err(e) => {
                error!("Proxy error to {}: {e}", target.address);
                return super::handler::error_response(
                    StatusCode::BAD_GATEWAY,
                    &format!("backend error: {e}"),
                );
            }
        }
    }

    super::handler::error_response(StatusCode::BAD_GATEWAY, "all backends failed")
}

/// Forward a single-target request, streaming the request body straight to the
/// backend with no intermediate buffer (symmetric to the response streaming in
/// [`build_response`] / #63).
///
/// A single-target route has no alternate backend, so the loss of 502-retry —
/// a streamed body is single-use — costs nothing. This covers the >90% of
/// routes with exactly one target, notably Docker registry blob *pushes*
/// (100MB+ layers) which previously buffered fully in the proxy task and were a
/// contributing factor in the accept-loop starvation seen in v0.2.9-rc.2.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn forward_streaming(
    client: &reqwest::Client,
    target: &RouteTarget,
    method: &reqwest::Method,
    headers: &hyper::HeaderMap,
    body: Incoming,
    path_and_query: &str,
    host: &str,
    is_tls: bool,
    client_ip: String,
) -> Response<ProxyBody> {
    let (forward_req, uri) = build_forward_request(
        client,
        target,
        method,
        headers,
        path_and_query,
        host,
        is_tls,
        &client_ip,
    );
    debug!("Proxying (stream) {host}{path_and_query} -> {uri}");

    // Attach the body. An *empty* body (e.g. `Content-Length: 0` — a session-
    // refresh POST, a bodyless POST) must be forwarded as a zero-length body so
    // reqwest emits `Content-Length: 0`, NOT `Transfer-Encoding: chunked`:
    // `wrap_stream` produces an unknown-length (chunked) body, and a chunked
    // *empty* request makes strict backends (Fastify/Infisical) treat it as a
    // body to parse and reject it with `415 Unsupported Media Type` when there's
    // no `Content-Type` — which broke every empty-body POST through the proxy.
    // Only non-empty bodies are streamed (the large-blob case #102 targets).
    use hyper::body::Body as _;
    let forward_req = if body.size_hint().exact() == Some(0) {
        forward_req.body(reqwest::Body::from(Vec::<u8>::new()))
    } else {
        forward_req.body(reqwest::Body::wrap_stream(BodyDataStream::new(body)))
    };

    match send_with_slow_warn(forward_req, &target.address, path_and_query).await {
        Ok(resp) => build_response(resp),
        Err(e) => {
            error!("Proxy error to {}: {e}", target.address);
            super::handler::error_response(StatusCode::BAD_GATEWAY, &format!("backend error: {e}"))
        }
    }
}

/// Build a 301 redirect response to HTTPS.
pub(crate) fn redirect_to_https(host: &str, path: &str) -> Response<ProxyBody> {
    let location = format!("https://{host}{path}");
    let body = full_body(hyper::body::Bytes::from(format!("Moved to {location}")));
    let mut resp = Response::new(body);
    *resp.status_mut() = StatusCode::MOVED_PERMANENTLY;
    resp.headers_mut().insert(
        hyper::header::LOCATION,
        location.parse().expect("valid location header"),
    );
    resp
}

#[cfg(test)]
mod tests;
