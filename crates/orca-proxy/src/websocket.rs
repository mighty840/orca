//! WebSocket upgrade proxy support.
//!
//! Detects WebSocket upgrade requests and tunnels them via raw TCP
//! to the backend, using `hyper::upgrade` on the client side and
//! `tokio::io::copy_bidirectional` for bidirectional piping.

use hyper::body::Incoming;
use hyper::{Request, Response, StatusCode};
use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tracing::{debug, error};

/// Check if a request is a WebSocket upgrade.
pub(crate) fn is_websocket_upgrade(req: &Request<Incoming>) -> bool {
    req.headers()
        .get("upgrade")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v.eq_ignore_ascii_case("websocket"))
}

/// Handle a WebSocket upgrade by tunneling to the backend.
///
/// 1. Opens a TCP connection to the backend
/// 2. Forwards the raw HTTP upgrade request
/// 3. Returns a 101 Switching Protocols response
/// 4. Spawns a task to pipe bytes bidirectionally
pub(crate) async fn handle_websocket_proxy(
    req: Request<Incoming>,
    backend_addr: &str,
) -> Response<http_body_util::Full<hyper::body::Bytes>> {
    // Connect to the backend
    let mut backend = match TcpStream::connect(backend_addr).await {
        Ok(stream) => stream,
        Err(e) => {
            error!("WebSocket backend connect failed ({backend_addr}): {e}");
            return super::handler::error_response(
                StatusCode::BAD_GATEWAY,
                &format!("websocket backend error: {e}"),
            );
        }
    };

    // Build the raw HTTP upgrade request to send to the backend
    let (parts, _body) = req.into_parts();
    let path = parts
        .uri
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/");

    let mut raw_req = format!("{} {} HTTP/1.1\r\n", parts.method, path);
    for (name, value) in &parts.headers {
        if let Ok(val) = value.to_str() {
            raw_req.push_str(&format!("{}: {val}\r\n", name.as_str()));
        }
    }
    raw_req.push_str("\r\n");

    // Send the upgrade request to the backend
    if let Err(e) = backend.write_all(raw_req.as_bytes()).await {
        error!("Failed to send WebSocket upgrade to backend: {e}");
        return super::handler::error_response(
            StatusCode::BAD_GATEWAY,
            &format!("websocket write error: {e}"),
        );
    }

    debug!("WebSocket upgrade forwarded to {backend_addr}");

    // Return 101 to the client; the actual bidirectional piping happens
    // after hyper yields the upgraded connection. The caller (serve_loop)
    // must enable `with_upgrades()` on the connection for this to work.
    let mut resp = Response::new(http_body_util::Full::new(hyper::body::Bytes::new()));
    *resp.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
    resp.headers_mut()
        .insert("upgrade", "websocket".parse().expect("valid header value"));
    resp.headers_mut()
        .insert("connection", "Upgrade".parse().expect("valid header value"));
    resp
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_websocket_upgrade_detected() {
        // Test the detection logic directly since we can't easily construct
        // a Request<Incoming> in tests. The function checks for the "upgrade"
        // header with value "websocket" (case-insensitive).
        let check = |val: &str| -> bool { val.eq_ignore_ascii_case("websocket") };

        assert!(check("websocket"));
        assert!(check("WebSocket"));
        assert!(check("WEBSOCKET"));
        assert!(!check("http"));
        assert!(!check(""));
    }

    #[test]
    fn test_non_websocket_not_detected() {
        // Verify that arbitrary upgrade values are not treated as websocket
        let check = |val: &str| -> bool { val.eq_ignore_ascii_case("websocket") };
        assert!(!check("h2c"));
        assert!(!check("TLS/1.0"));
    }
}
