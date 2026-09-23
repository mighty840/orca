//! WebSocket upgrade proxy support.
//!
//! Detects WebSocket upgrade requests and tunnels them via raw TCP
//! to the backend using hyper's upgrade mechanism + bidirectional copy.

use hyper::body::Incoming;
use hyper::header::{CONNECTION, HeaderMap, HeaderName, HeaderValue, UPGRADE};
use hyper::{Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tracing::{debug, error};

use crate::RouteTarget;

/// Check if a request is a WebSocket upgrade.
pub(crate) fn is_websocket_upgrade(req: &Request<Incoming>) -> bool {
    req.headers()
        .get("upgrade")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("websocket"))
}

/// Handle a WebSocket upgrade by tunneling to `target`.
///
/// Connects to the backend first, performs the HTTP upgrade handshake, and
/// relays the backend's handshake headers in the 101 returned to the browser
/// (see [`switching_protocols_response`]). The browser validates them — a
/// missing `Sec-WebSocket-Accept`, or a missing `Sec-WebSocket-Protocol` when
/// it offered subprotocols, fails the handshake.
///
/// The serve loop MUST call `.with_upgrades()` on the hyper connection for
/// `hyper::upgrade::on` to work.
pub(crate) async fn handle_websocket_proxy(
    mut req: Request<Incoming>,
    target: &RouteTarget,
    host: &str,
    is_tls: bool,
    client_ip: &str,
) -> Response<crate::body::ProxyBody> {
    // Capture the upgrade future before consuming the request.
    let upgrade_fut = hyper::upgrade::on(&mut req);
    let backend_addr = target.address.clone();

    let (parts, _body) = req.into_parts();
    let path_and_query = parts
        .uri
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/");
    let path = crate::forward::strip_target_prefix(target, path_and_query);
    let raw_req = build_upgrade_request(
        &parts.method,
        &path,
        &parts.headers,
        host,
        is_tls,
        client_ip,
    );

    // Connect to backend and complete the handshake NOW (before returning 101)
    // so we can extract Sec-WebSocket-Accept to forward to the browser.
    // Bounded so a dead/slow backend can't park this task indefinitely.
    let mut backend = match tokio::time::timeout(
        std::time::Duration::from_secs(5),
        TcpStream::connect(&backend_addr),
    )
    .await
    {
        Ok(Ok(s)) => s,
        Ok(Err(e)) => {
            error!("WebSocket backend connect failed ({backend_addr}): {e}");
            let mut r = Response::new(crate::body::empty_body());
            *r.status_mut() = StatusCode::BAD_GATEWAY;
            return r;
        }
        Err(_) => {
            error!("WebSocket backend connect timed out ({backend_addr})");
            let mut r = Response::new(crate::body::empty_body());
            *r.status_mut() = StatusCode::GATEWAY_TIMEOUT;
            return r;
        }
    };

    if let Err(e) = backend.write_all(&raw_req).await {
        error!("WebSocket write to backend failed: {e}");
        let mut r = Response::new(crate::body::empty_body());
        *r.status_mut() = StatusCode::BAD_GATEWAY;
        return r;
    }

    // Read backend's 101 header bytes (stop at \r\n\r\n) and extract
    // Sec-WebSocket-Accept so we can include it in our response to the browser.
    // The whole header read is bounded so a backend that accepts but never
    // replies can't hang this task.
    let mut hdr = Vec::with_capacity(512);
    let read_result = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        let mut byte = [0u8; 1];
        loop {
            backend.read_exact(&mut byte).await?;
            hdr.push(byte[0]);
            if hdr.len() >= 4 && hdr[hdr.len() - 4..] == *b"\r\n\r\n" {
                return Ok::<(), std::io::Error>(());
            }
            if hdr.len() > 8192 {
                return Err(std::io::Error::other("response header too large"));
            }
        }
    })
    .await;
    match read_result {
        Ok(Ok(())) => {}
        Ok(Err(e)) => {
            error!("WebSocket backend header read failed: {e}");
            let mut r = Response::new(crate::body::empty_body());
            *r.status_mut() = StatusCode::BAD_GATEWAY;
            return r;
        }
        Err(_) => {
            error!("WebSocket backend header read timed out ({backend_addr})");
            let mut r = Response::new(crate::body::empty_body());
            *r.status_mut() = StatusCode::GATEWAY_TIMEOUT;
            return r;
        }
    }

    // Bail if the backend didn't agree to upgrade.
    if !is_switching_protocols(&hdr) {
        let head = String::from_utf8_lossy(&hdr);
        let first_line = head.lines().next().unwrap_or("");
        error!("WebSocket backend refused upgrade: {first_line}");
        let mut r = Response::new(crate::body::empty_body());
        *r.status_mut() = StatusCode::BAD_GATEWAY;
        return r;
    }
    let resp = switching_protocols_response(&hdr);

    // Spawn the bidirectional copy. Hyper will resolve `upgrade_fut` once it
    // has sent the 101 response we return below.
    tokio::spawn(async move {
        let upgraded = match upgrade_fut.await {
            Ok(u) => u,
            Err(e) => {
                error!("WebSocket client upgrade failed: {e}");
                return;
            }
        };
        debug!("WebSocket tunnel established to {backend_addr}");
        let mut client_io = TokioIo::new(upgraded);
        let _ = tokio::io::copy_bidirectional(&mut client_io, &mut backend).await;
    });

    // Hyper sends this 101 and then yields the raw connection to the upgrade
    // future spawned above.
    resp
}

/// Handshake headers relayed from the backend's 101 to the client. After the
/// upgrade the proxy is a byte pipe, so whatever the backend negotiated —
/// accept key, selected subprotocol (#191: Prosody's `xmpp`), and extensions
/// such as `permessage-deflate` — is exactly what the client must be told.
const RELAYED_HANDSHAKE_HEADERS: [&str; 3] = [
    "sec-websocket-accept",
    "sec-websocket-protocol",
    "sec-websocket-extensions",
];

/// Serialize the upgrade request sent to the backend: the client's method,
/// the (prefix-stripped) path, its headers, and the same X-Forwarded-* set
/// the HTTP path injects. A client-sent X-Forwarded-For is never passed
/// through verbatim — the peer we observed is appended to it, so the
/// right-most entry is authoritative (see `forward::forwarded_for_value`).
pub(crate) fn build_upgrade_request(
    method: &Method,
    path: &str,
    headers: &HeaderMap,
    host: &str,
    is_tls: bool,
    client_ip: &str,
) -> Vec<u8> {
    let mut out = format!("{method} {path} HTTP/1.1\r\n").into_bytes();
    let mut incoming_xff: Option<String> = None;
    let (mut saw_proto, mut saw_fhost) = (false, false);
    for (name, value) in headers {
        match name.as_str() {
            "x-forwarded-for" => {
                if let Ok(v) = value.to_str() {
                    incoming_xff = Some(match incoming_xff.take() {
                        Some(prev) => format!("{prev}, {v}"),
                        None => v.to_string(),
                    });
                }
                continue;
            }
            "x-forwarded-proto" => saw_proto = true,
            "x-forwarded-host" => saw_fhost = true,
            _ => {}
        }
        // Raw bytes, not `to_str`: a header carrying obs-text is still valid
        // HTTP and must not be silently dropped from the handshake.
        push_header(&mut out, name.as_str(), value.as_bytes());
    }
    if !saw_proto {
        let scheme = if is_tls { "https" } else { "http" };
        push_header(&mut out, "x-forwarded-proto", scheme.as_bytes());
    }
    if !saw_fhost {
        push_header(&mut out, "x-forwarded-host", host.as_bytes());
    }
    let xff = crate::forward::forwarded_for_value(incoming_xff.as_deref(), client_ip);
    push_header(&mut out, "x-forwarded-for", xff.as_bytes());
    out.extend_from_slice(b"\r\n");
    out
}

fn push_header(out: &mut Vec<u8>, name: &str, value: &[u8]) {
    out.extend_from_slice(name.as_bytes());
    out.extend_from_slice(b": ");
    out.extend_from_slice(value);
    out.extend_from_slice(b"\r\n");
}

/// Whether the backend's response head has status 101. Parses the status
/// code token rather than searching the line, so e.g. `HTTP/1.1 200 OK-101`
/// is not mistaken for an upgrade.
pub(crate) fn is_switching_protocols(head: &[u8]) -> bool {
    let line = head.split(|&b| b == b'\n').next().unwrap_or_default();
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    line.split(|&b| b == b' ').nth(1) == Some(b"101".as_slice())
}

/// Build the 101 returned to the client from the backend's response head,
/// relaying every [`RELAYED_HANDSHAKE_HEADERS`] line (repeated lines are
/// kept, as RFC 6455 allows for extensions).
pub(crate) fn switching_protocols_response(head: &[u8]) -> Response<crate::body::ProxyBody> {
    let mut resp = Response::new(crate::body::empty_body());
    *resp.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
    let headers = resp.headers_mut();
    headers.insert(UPGRADE, HeaderValue::from_static("websocket"));
    headers.insert(CONNECTION, HeaderValue::from_static("Upgrade"));
    for line in head.split(|&b| b == b'\n').skip(1) {
        let Some(colon) = line.iter().position(|&b| b == b':') else {
            continue;
        };
        let Ok(name) = HeaderName::from_bytes(line[..colon].trim_ascii()) else {
            continue;
        };
        if !RELAYED_HANDSHAKE_HEADERS.contains(&name.as_str()) {
            continue;
        }
        if let Ok(value) = HeaderValue::from_bytes(line[colon + 1..].trim_ascii()) {
            headers.append(name, value);
        }
    }
    resp
}

#[cfg(test)]
#[path = "websocket_tests.rs"]
mod tests;
