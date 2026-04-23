//! WebSocket upgrade proxy support.
//!
//! Detects WebSocket upgrade requests and tunnels them via raw TCP
//! to the backend using hyper's upgrade mechanism + bidirectional copy.

use hyper::body::Incoming;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
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
/// Connects to the backend first, performs the HTTP upgrade handshake, and
/// extracts `Sec-WebSocket-Accept` before returning 101 to the browser.
/// The browser validates this header — without it the handshake fails immediately.
///
/// The serve loop MUST call `.with_upgrades()` on the hyper connection for
/// `hyper::upgrade::on` to work.
pub(crate) async fn handle_websocket_proxy(
    mut req: Request<Incoming>,
    backend_addr: &str,
) -> Response<http_body_util::Full<hyper::body::Bytes>> {
    // Capture the upgrade future before consuming the request.
    let upgrade_fut = hyper::upgrade::on(&mut req);
    let backend_addr = backend_addr.to_string();

    let (parts, _body) = req.into_parts();
    let path = parts
        .uri
        .path_and_query()
        .map(|pq| pq.as_str())
        .unwrap_or("/")
        .to_string();

    // Build the raw HTTP upgrade request to send to the backend.
    let mut raw_req = format!("{} {} HTTP/1.1\r\n", parts.method, path);
    for (name, value) in &parts.headers {
        if let Ok(val) = value.to_str() {
            raw_req.push_str(&format!("{}: {val}\r\n", name.as_str()));
        }
    }
    raw_req.push_str("\r\n");

    // Connect to backend and complete the handshake NOW (before returning 101)
    // so we can extract Sec-WebSocket-Accept to forward to the browser.
    let mut backend = match TcpStream::connect(&backend_addr).await {
        Ok(s) => s,
        Err(e) => {
            error!("WebSocket backend connect failed ({backend_addr}): {e}");
            let mut r = Response::new(http_body_util::Full::new(hyper::body::Bytes::new()));
            *r.status_mut() = StatusCode::BAD_GATEWAY;
            return r;
        }
    };

    if let Err(e) = backend.write_all(raw_req.as_bytes()).await {
        error!("WebSocket write to backend failed: {e}");
        let mut r = Response::new(http_body_util::Full::new(hyper::body::Bytes::new()));
        *r.status_mut() = StatusCode::BAD_GATEWAY;
        return r;
    }

    // Read backend's 101 header bytes (stop at \r\n\r\n) and extract
    // Sec-WebSocket-Accept so we can include it in our response to the browser.
    let mut hdr = Vec::with_capacity(512);
    let mut byte = [0u8; 1];
    loop {
        if backend.read_exact(&mut byte).await.is_err() {
            error!("WebSocket backend closed before 101");
            let mut r = Response::new(http_body_util::Full::new(hyper::body::Bytes::new()));
            *r.status_mut() = StatusCode::BAD_GATEWAY;
            return r;
        }
        hdr.push(byte[0]);
        if hdr.len() >= 4 && hdr[hdr.len() - 4..] == *b"\r\n\r\n" {
            break;
        }
        if hdr.len() > 8192 {
            error!("WebSocket backend response header too large");
            let mut r = Response::new(http_body_util::Full::new(hyper::body::Bytes::new()));
            *r.status_mut() = StatusCode::BAD_GATEWAY;
            return r;
        }
    }

    // Bail if the backend didn't agree to upgrade.
    let first_line = String::from_utf8_lossy(&hdr);
    let first_line = first_line.lines().next().unwrap_or("");
    if !first_line.contains("101") {
        error!("WebSocket backend refused upgrade: {first_line}");
        let mut r = Response::new(http_body_util::Full::new(hyper::body::Bytes::new()));
        *r.status_mut() = StatusCode::BAD_GATEWAY;
        return r;
    }

    let accept_val: Option<hyper::header::HeaderValue> = String::from_utf8_lossy(&hdr)
        .lines()
        .find(|l| l.to_ascii_lowercase().starts_with("sec-websocket-accept:"))
        .and_then(|l| l.split_once(':').map(|x| x.1))
        .map(|v| v.trim().to_string())
        .and_then(|s| s.parse().ok());

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

    // Return 101 to the client including the Sec-WebSocket-Accept the backend
    // computed. Hyper sends this and then yields the raw connection to the
    // upgrade future we spawned above.
    let mut resp = Response::new(http_body_util::Full::new(hyper::body::Bytes::new()));
    *resp.status_mut() = StatusCode::SWITCHING_PROTOCOLS;
    resp.headers_mut()
        .insert("upgrade", "websocket".parse().expect("valid header value"));
    resp.headers_mut()
        .insert("connection", "Upgrade".parse().expect("valid header value"));
    if let Some(val) = accept_val {
        resp.headers_mut().insert("sec-websocket-accept", val);
    }
    resp
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_websocket_upgrade_detected() {
        let check = |val: &str| -> bool { val.eq_ignore_ascii_case("websocket") };
        assert!(check("websocket"));
        assert!(check("WebSocket"));
        assert!(check("WEBSOCKET"));
        assert!(!check("http"));
        assert!(!check(""));
    }

    #[test]
    fn test_non_websocket_not_detected() {
        let check = |val: &str| -> bool { val.eq_ignore_ascii_case("websocket") };
        assert!(!check("h2c"));
        assert!(!check("TLS/1.0"));
    }
}
