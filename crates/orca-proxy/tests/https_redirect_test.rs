//! Regression tests for #123: the HTTP→HTTPS redirect must only fire when a
//! TLS endpoint actually exists. With ACME unconfigured the proxy runs a
//! single plain-HTTP listener — redirecting a routed host to `https://` then
//! sends clients to a closed port (and redirect-loops behind an external
//! TLS-terminating proxy).
//!
//! Fast tests (no Docker, no network beyond loopback) — not `#[ignore]`d.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use hyper::StatusCode;
use orca_proxy::acme::AcmeManager;
use orca_proxy::{RouteTarget, run_proxy_with_fallback};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::RwLock;

/// Reserve a loopback port that nothing listens on: bind, read, drop.
/// Connections to it fail instantly with ECONNREFUSED, so forwarding
/// attempts surface as a fast 502 instead of a timeout.
async fn dead_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

/// Spawn the proxy (plain HTTP listener) with the given route table and
/// optional ACME manager. Returns the bound port once it accepts TCP.
async fn spawn_proxy(
    route_table: Arc<RwLock<HashMap<String, Vec<RouteTarget>>>>,
    acme: Option<AcmeManager>,
) -> u16 {
    let port = dead_port().await;
    let triggers = Arc::new(RwLock::new(Vec::new()));
    tokio::spawn(async move {
        let _ = run_proxy_with_fallback(route_table, triggers, None, port, None, acme, None).await;
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

fn routed_table(host: &str, target_port: u16) -> Arc<RwLock<HashMap<String, Vec<RouteTarget>>>> {
    let mut routes = HashMap::new();
    routes.insert(
        host.to_string(),
        vec![RouteTarget {
            address: format!("127.0.0.1:{target_port}"),
            service_name: "app".to_string(),
            path_pattern: None,
            weight: 100,
            strip_prefix: None,
        }],
    );
    Arc::new(RwLock::new(routes))
}

fn no_redirect_client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

/// HTTP-only proxy (no TLS acceptor, no ACME): a routed host must be
/// forwarded, never redirected to an HTTPS endpoint that doesn't exist.
/// The route target is a dead port, so a forwarding attempt shows up as a
/// fast 502 — any 3xx means the redirect fired.
#[tokio::test]
async fn http_only_proxy_never_redirects_routed_host() {
    let table = routed_table("app.local", dead_port().await);
    let proxy_port = spawn_proxy(table, None).await;

    let resp = no_redirect_client()
        .get(format!("http://127.0.0.1:{proxy_port}/dashboard"))
        .header("Host", "app.local")
        .send()
        .await
        .expect("request to proxy");

    assert!(
        !resp.status().is_redirection(),
        "HTTP-only proxy must not redirect to HTTPS (#123), got {} with Location {:?}",
        resp.status(),
        resp.headers().get("location"),
    );
    assert_eq!(
        resp.status(),
        StatusCode::BAD_GATEWAY,
        "request should have been forwarded (and failed on the dead target)"
    );
}

/// With an ACME manager present (the dual-listener setup's port-80 half),
/// HTTPS does exist on 443 — the redirect must keep firing.
#[tokio::test]
async fn acme_http_listener_still_redirects_routed_host() {
    let table = routed_table("app.local", dead_port().await);
    let acme = AcmeManager::new(
        "test@example.com",
        std::env::temp_dir().join("orca-test-acme-cache"),
    );
    let proxy_port = spawn_proxy(table, Some(acme)).await;

    let resp = no_redirect_client()
        .get(format!("http://127.0.0.1:{proxy_port}/dashboard"))
        .header("Host", "app.local")
        .send()
        .await
        .expect("request to proxy");

    assert_eq!(
        resp.status(),
        StatusCode::MOVED_PERMANENTLY,
        "with ACME running, HTTP requests for routed hosts must redirect"
    );
    assert_eq!(
        resp.headers()
            .get("location")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default(),
        "https://app.local/dashboard"
    );
}

/// Unknown hosts never redirect regardless of TLS availability — they fall
/// through to 404 (no fallback configured). Guards against widening the
/// redirect condition beyond routed hosts.
#[tokio::test]
async fn unknown_host_is_never_redirected() {
    let table = routed_table("app.local", dead_port().await);
    let acme = AcmeManager::new(
        "test@example.com",
        std::env::temp_dir().join("orca-test-acme-cache"),
    );
    let proxy_port = spawn_proxy(table, Some(acme)).await;

    let resp = no_redirect_client()
        .get(format!("http://127.0.0.1:{proxy_port}/"))
        .header("Host", "unknown.local")
        .send()
        .await
        .expect("request to proxy");

    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
}
