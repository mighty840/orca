//! Edges of HTTP/2 support (#168) that the h2/h1 happy paths in
//! `http2_test.rs` don't cover:
//!
//! - A WebSocket upgrade over TLS still works on the h2-capable listener.
//!   Browsers open WebSockets on a separate connection that offers only
//!   `http/1.1` via ALPN (extended CONNECT, RFC 8441, is not enabled), and
//!   the backend's selected subprotocol must still reach the client (#191).
//! - The plain listener does not speak cleartext HTTP/2 (h2c prior
//!   knowledge). No browser uses it, so it would only add parser surface.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use orca_proxy::tls::{TlsMode, create_tls_acceptor};
use orca_proxy::{RouteTarget, run_proxy, run_proxy_with_fallback};
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::RwLock;

/// Read an HTTP head through the blank line.
async fn read_head(io: &mut (impl AsyncRead + Unpin)) -> String {
    let mut head = Vec::new();
    let mut byte = [0u8; 1];
    while !head.ends_with(b"\r\n\r\n") {
        io.read_exact(&mut byte).await.expect("head must arrive");
        head.push(byte[0]);
    }
    String::from_utf8(head).unwrap()
}

/// Raw backend answering any upgrade with a 101 that selects `xmpp`.
async fn spawn_ws_backend() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    tokio::spawn(async move {
        loop {
            let (mut s, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                read_head(&mut s).await;
                let _ = s
                    .write_all(
                        b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\n\
                          Connection: Upgrade\r\nSec-WebSocket-Accept: abc=\r\n\
                          Sec-WebSocket-Protocol: xmpp\r\n\r\n",
                    )
                    .await;
                let (mut r, mut w) = s.split();
                let _ = tokio::io::copy(&mut r, &mut w).await;
            });
        }
    });
    addr
}

fn routes(backend: String) -> Arc<RwLock<HashMap<String, Vec<RouteTarget>>>> {
    Arc::new(RwLock::new(HashMap::from([(
        "localhost".to_string(),
        vec![RouteTarget {
            address: backend,
            service_name: "app".into(),
            path_pattern: None,
            weight: 100,
            strip_prefix: None,
        }],
    )])))
}

async fn free_port() -> u16 {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    l.local_addr().unwrap().port()
}

async fn wait_bound(port: u16) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while tokio::time::Instant::now() < deadline {
        if TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("proxy on port {port} never came up");
}

/// Accepts the proxy's self-signed test certificate.
#[derive(Debug)]
struct AcceptAny(Arc<rustls::crypto::CryptoProvider>);

impl ServerCertVerifier for AcceptAny {
    fn verify_server_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        m: &[u8],
        c: &CertificateDer<'_>,
        d: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(m, c, d, &self.0.signature_verification_algorithms)
    }
    fn verify_tls13_signature(
        &self,
        m: &[u8],
        c: &CertificateDer<'_>,
        d: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(m, c, d, &self.0.signature_verification_algorithms)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.0.signature_verification_algorithms.supported_schemes()
    }
}

#[tokio::test]
async fn websocket_upgrade_over_tls_still_relays_subprotocol() {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let backend = spawn_ws_backend().await;
    let port = free_port().await;
    let acceptor = create_tls_acceptor(&TlsMode::SelfSigned)
        .unwrap()
        .expect("self-signed acceptor");
    let r = routes(backend);
    let triggers = Arc::new(RwLock::new(Vec::new()));
    tokio::spawn(async move {
        let _ = run_proxy_with_fallback(r, triggers, None, port, Some(acceptor), None, None).await;
    });
    wait_bound(port).await;

    // What a browser's WebSocket connection offers: ALPN http/1.1 only.
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut config = rustls::ClientConfig::builder()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AcceptAny(provider)))
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    let connector = tokio_rustls::TlsConnector::from(Arc::new(config));
    let tcp = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    let mut tls = connector
        .connect(ServerName::try_from("localhost").unwrap(), tcp)
        .await
        .expect("TLS handshake");
    assert_eq!(tls.get_ref().1.alpn_protocol(), Some(&b"http/1.1"[..]));

    tls.write_all(
        b"GET /xmpp-websocket HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\n\
          Connection: Upgrade\r\nSec-WebSocket-Version: 13\r\n\
          Sec-WebSocket-Key: dGhlIHNhbXBsZSBub25jZQ==\r\nSec-WebSocket-Protocol: xmpp\r\n\r\n",
    )
    .await
    .unwrap();
    let head = tokio::time::timeout(Duration::from_secs(5), read_head(&mut tls))
        .await
        .expect("proxy must answer")
        .to_ascii_lowercase();
    assert!(head.starts_with("http/1.1 101"), "got: {head}");
    assert!(
        head.contains("sec-websocket-protocol: xmpp\r\n"),
        "got: {head}"
    );

    tls.write_all(b"ping").await.unwrap();
    let mut echo = [0u8; 4];
    tokio::time::timeout(Duration::from_secs(5), tls.read_exact(&mut echo))
        .await
        .expect("echo must arrive")
        .unwrap();
    assert_eq!(&echo, b"ping");
}

#[tokio::test]
async fn plain_listener_does_not_speak_h2c() {
    let backend = spawn_ws_backend().await;
    let port = free_port().await;
    let r = routes(backend);
    let triggers = Arc::new(RwLock::new(Vec::new()));
    tokio::spawn(async move {
        let _ = run_proxy(r, triggers, None, port, None, None).await;
    });
    wait_bound(port).await;

    // HTTP/2 connection preface followed by an empty SETTINGS frame.
    let mut s = TcpStream::connect(("127.0.0.1", port)).await.unwrap();
    s.write_all(b"PRI * HTTP/2.0\r\n\r\nSM\r\n\r\n\x00\x00\x00\x04\x00\x00\x00\x00\x00")
        .await
        .unwrap();
    let mut buf = [0u8; 64];
    let n = tokio::time::timeout(Duration::from_secs(5), s.read(&mut buf))
        .await
        .expect("proxy must answer or close")
        .unwrap_or(0);
    // An h2 server answers with its own SETTINGS frame: type byte 0x04 at
    // offset 3 of the 9-byte frame header.
    let is_h2_settings = n >= 9 && buf[3] == 0x04;
    assert!(
        !is_h2_settings,
        "plain listener answered the h2c preface with an HTTP/2 SETTINGS frame"
    );
}
