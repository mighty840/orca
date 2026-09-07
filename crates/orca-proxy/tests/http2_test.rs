//! HTTP/2 over TLS: the proxy advertises `h2` via ALPN and serves both h1 and
//! h2 on the same listener. Under h2 there is no `Host` header — the host
//! arrives as the `:authority` pseudo-header — so routing must still find
//! the route and the backend must still receive a proper `Host`.
//!
//! Fast tests (loopback only, self-signed cert) — not `#[ignore]`d.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use http_body_util::Full;
use hyper::body::{Bytes, Incoming};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use orca_proxy::tls::{TlsMode, create_tls_acceptor};
use orca_proxy::{RouteTarget, run_proxy_with_fallback};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::RwLock;

/// Plain-HTTP/1.1 backend that echoes the `Host` header it received.
async fn spawn_echo_backend() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let svc = service_fn(|req: Request<Incoming>| async move {
                    let host = req
                        .headers()
                        .get("host")
                        .and_then(|h| h.to_str().ok())
                        .unwrap_or("<none>")
                        .to_string();
                    Ok::<_, hyper::Error>(Response::new(Full::new(Bytes::from(host))))
                });
                let _ = http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), svc)
                    .await;
            });
        }
    });
    addr
}

/// TLS proxy (self-signed cert for `localhost`) routing `localhost` → backend.
async fn spawn_tls_proxy(backend: SocketAddr) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let mut routes = HashMap::new();
    routes.insert(
        "localhost".to_string(),
        vec![RouteTarget {
            address: backend.to_string(),
            service_name: "app".to_string(),
            path_pattern: None,
            weight: 100,
            strip_prefix: None,
        }],
    );
    let routes = Arc::new(RwLock::new(routes));
    let triggers = Arc::new(RwLock::new(Vec::new()));
    // Same provider the orca binary installs at startup; the dependency
    // graph carries both ring and aws-lc-rs, so rustls cannot pick one.
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let acceptor = create_tls_acceptor(&TlsMode::SelfSigned)
        .unwrap()
        .expect("self-signed acceptor");
    tokio::spawn(async move {
        let _ =
            run_proxy_with_fallback(routes, triggers, None, port, Some(acceptor), None, None).await;
    });

    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while tokio::time::Instant::now() < deadline {
        if TcpStream::connect(("127.0.0.1", port)).await.is_ok() {
            return port;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("orca proxy did not bind on port {port} within 3s");
}

fn client() -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .no_proxy()
        .danger_accept_invalid_certs(true)
}

#[tokio::test]
async fn negotiates_http2_and_routes_by_authority() {
    let backend = spawn_echo_backend().await;
    let port = spawn_tls_proxy(backend).await;

    let resp = client()
        .build()
        .unwrap()
        .get(format!("https://localhost:{port}/"))
        .send()
        .await
        .expect("request to proxy");

    assert_eq!(
        resp.version(),
        reqwest::Version::HTTP_2,
        "ALPN must pick h2"
    );
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.text().await.unwrap(),
        "localhost",
        "backend must see the external host, taken from :authority"
    );
}

#[tokio::test]
async fn still_serves_http1_clients() {
    let backend = spawn_echo_backend().await;
    let port = spawn_tls_proxy(backend).await;

    let resp = client()
        .http1_only()
        .build()
        .unwrap()
        .get(format!("https://localhost:{port}/"))
        .send()
        .await
        .expect("request to proxy");

    assert_eq!(resp.version(), reqwest::Version::HTTP_11);
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), "localhost");
}
