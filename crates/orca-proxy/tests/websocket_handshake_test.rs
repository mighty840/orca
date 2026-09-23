//! End-to-end test: WebSocket upgrades through the proxy relay the backend's
//! negotiated handshake to the client (issue #191).
//!
//! Jitsi signals XMPP over a WebSocket with `Sec-WebSocket-Protocol: xmpp`.
//! Prosody selects it in its 101, but the proxy built its own 101 carrying
//! only `Sec-WebSocket-Accept`, so the browser — which offered a subprotocol
//! and got none back — failed the handshake and every call dropped. The
//! upgrade request also went out without X-Forwarded-* and ignored the
//! route's `strip_prefix`, unlike plain HTTP.
//!
//! The backend here is a raw TCP socket, so the test sees the exact request
//! bytes the proxy sends and controls the exact 101 it gets back.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use orca_proxy::{RouteTarget, run_proxy};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{RwLock, mpsc};

const HOST: &str = "meet.example.com";

/// Read an HTTP head (through the blank line) byte by byte, leaving any
/// following frame bytes unread.
async fn read_head(io: &mut (impl AsyncRead + Unpin)) -> String {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        io.read_exact(&mut byte).await.expect("head must arrive");
        head.push(byte[0]);
    }
    String::from_utf8(head).unwrap()
}

/// Backend that records the upgrade request it receives, answers with a
/// Prosody-style 101 selecting `xmpp`, then echoes the tunnel.
async fn spawn_ws_backend() -> (SocketAddr, mpsc::UnboundedReceiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        loop {
            let (mut stream, _) = listener.accept().await.unwrap();
            let tx = tx.clone();
            tokio::spawn(async move {
                let _ = tx.send(read_head(&mut stream).await);
                stream
                    .write_all(
                        b"HTTP/1.1 101 Switching Protocols\r\n\
                          Upgrade: websocket\r\nConnection: Upgrade\r\n\
                          Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n\
                          Sec-WebSocket-Protocol: xmpp\r\n\
                          Sec-WebSocket-Extensions: permessage-deflate\r\n\r\n",
                    )
                    .await
                    .unwrap();
                let (mut r, mut w) = stream.split();
                let _ = tokio::io::copy(&mut r, &mut w).await;
            });
        }
    });
    (addr, rx)
}

/// Start the proxy with `HOST` routed to `target`; polls until bound.
async fn spawn_proxy(target: RouteTarget) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let routes = Arc::new(RwLock::new(HashMap::from([(
        HOST.to_string(),
        vec![target],
    )])));
    let triggers = Arc::new(RwLock::new(Vec::new()));
    tokio::spawn(async move {
        let _ = run_proxy(routes, triggers, None, port, None, None).await;
    });
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while tokio::time::Instant::now() < deadline {
        if TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            return port;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("proxy on port {port} never came up");
}

fn target(backend: SocketAddr, pattern: Option<&str>, strip: Option<&str>) -> RouteTarget {
    RouteTarget {
        address: backend.to_string(),
        service_name: "jitsi-web".into(),
        path_pattern: pattern.map(String::from),
        weight: 100,
        strip_prefix: strip.map(String::from),
    }
}

/// Send a browser-style upgrade offering `xmpp` and return the client stream
/// plus the proxy's response head.
async fn upgrade(port: u16, path: &str) -> (TcpStream, String) {
    let mut client = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let req = format!(
        "GET {path} HTTP/1.1\r\nHost: {HOST}\r\nUpgrade: websocket\r\n\
         Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\n\
         Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\n\
         Sec-WebSocket-Protocol: xmpp\r\nX-Forwarded-For: 10.9.9.9\r\n\r\n"
    );
    client.write_all(req.as_bytes()).await.unwrap();
    let head = tokio::time::timeout(Duration::from_secs(5), read_head(&mut client))
        .await
        .expect("proxy must answer the upgrade");
    (client, head.to_ascii_lowercase())
}

#[tokio::test]
async fn upgrade_relays_subprotocol_and_extensions_then_tunnels() {
    let (backend, _seen) = spawn_ws_backend().await;
    let port = spawn_proxy(target(backend, None, None)).await;

    let (mut client, head) = upgrade(port, "/xmpp-websocket?room=a").await;
    assert!(head.starts_with("http/1.1 101"), "got: {head}");
    assert!(
        head.contains("sec-websocket-protocol: xmpp\r\n"),
        "selected subprotocol must reach the client (#191); got: {head}"
    );
    assert!(head.contains("sec-websocket-extensions: permessage-deflate\r\n"));
    assert!(head.contains("sec-websocket-accept: s3pplmbitxaq9kygzzhzrbk+xoo=\r\n"));

    // The tunnel carries bytes both ways after the handshake.
    client.write_all(b"<open/>").await.unwrap();
    let mut echo = [0u8; 7];
    tokio::time::timeout(Duration::from_secs(5), client.read_exact(&mut echo))
        .await
        .expect("echo must arrive")
        .unwrap();
    assert_eq!(&echo, b"<open/>");
}

#[tokio::test]
async fn upgrade_request_carries_forwarded_headers_and_strips_prefix() {
    let (backend, mut seen) = spawn_ws_backend().await;
    let port = spawn_proxy(target(backend, Some("/chat/*"), Some("/chat"))).await;

    let (_client, head) = upgrade(port, "/chat/xmpp-websocket?room=a").await;
    assert!(head.starts_with("http/1.1 101"), "got: {head}");

    let sent = seen.recv().await.unwrap().to_ascii_lowercase();
    assert!(
        sent.starts_with("get /xmpp-websocket?room=a http/1.1\r\n"),
        "strip_prefix must apply to upgrades; got: {sent}"
    );
    assert!(sent.contains("x-forwarded-proto: http\r\n"), "got: {sent}");
    assert!(sent.contains(&format!("x-forwarded-host: {HOST}\r\n")));
    assert!(
        sent.contains("x-forwarded-for: 10.9.9.9, 127.0.0.1\r\n"),
        "observed peer must be appended to the client chain; got: {sent}"
    );
    assert_eq!(sent.matches("x-forwarded-for:").count(), 1);
}
