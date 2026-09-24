//! HTTP/2 sends cookies as separate `cookie` fields ("crumbs", RFC 9113
//! §8.2.3). An HTTP/1.1 backend must receive them as ONE `Cookie` header
//! joined with "; ". Forwarded as separate headers, Apache/PHP join them
//! with ", " and the session cookie is garbled: Nextcloud's OIDC callback
//! then fails its state check with a 403 (v0.3.0-rc.2 regression).

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

/// HTTP/1.1 backend answering with every `Cookie` header it got, one per line.
async fn spawn_cookie_echo() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let svc = service_fn(|req: Request<Incoming>| async move {
                    let cookies: Vec<String> = req
                        .headers()
                        .get_all("cookie")
                        .iter()
                        .map(|v| v.to_str().unwrap_or("?").to_string())
                        .collect();
                    Ok::<_, hyper::Error>(Response::new(Full::new(Bytes::from(cookies.join("\n")))))
                });
                let _ = http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), svc)
                    .await;
            });
        }
    });
    addr
}

async fn spawn_tls_proxy(backend: SocketAddr) -> u16 {
    let port = TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let routes = Arc::new(RwLock::new(HashMap::from([(
        "localhost".to_string(),
        vec![RouteTarget {
            address: backend.to_string(),
            service_name: "nextcloud".into(),
            path_pattern: None,
            weight: 100,
            strip_prefix: None,
        }],
    )])));
    let triggers = Arc::new(RwLock::new(Vec::new()));
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let acceptor = create_tls_acceptor(&TlsMode::SelfSigned).unwrap().unwrap();
    tokio::spawn(async move {
        let _ =
            run_proxy_with_fallback(routes, triggers, None, port, Some(acceptor), None, None).await;
    });
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while TcpStream::connect(("127.0.0.1", port)).await.is_err() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "proxy did not start"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    port
}

async fn cookies_seen_by_backend(http2: bool) -> String {
    let backend = spawn_cookie_echo().await;
    let port = spawn_tls_proxy(backend).await;
    let mut builder = reqwest::Client::builder()
        .no_proxy()
        .danger_accept_invalid_certs(true);
    if !http2 {
        builder = builder.http1_only();
    }
    let resp = builder
        .build()
        .unwrap()
        .get(format!("https://localhost:{port}/apps/user_oidc/code"))
        // Two cookie fields, as a browser sends them over HTTP/2.
        .header("cookie", "oc_sessionPassphrase=abc")
        .header("cookie", "nc_session_id=xyz")
        .send()
        .await
        .unwrap();
    if http2 {
        assert_eq!(resp.version(), reqwest::Version::HTTP_2);
    }
    resp.text().await.unwrap()
}

#[tokio::test]
async fn h2_cookie_crumbs_reach_an_h1_backend_as_one_header() {
    let seen = cookies_seen_by_backend(true).await;
    assert_eq!(
        seen, "oc_sessionPassphrase=abc; nc_session_id=xyz",
        "backend must get exactly one Cookie header joined with '; '"
    );
}

#[tokio::test]
async fn h1_cookies_are_joined_the_same_way() {
    // Over HTTP/1.1 a second Cookie line is already non-standard; joining it
    // too is harmless and keeps both paths identical.
    let seen = cookies_seen_by_backend(false).await;
    assert_eq!(seen, "oc_sessionPassphrase=abc; nc_session_id=xyz");
}
