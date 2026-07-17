//! E2E test: proxy forwards unmatched-host requests to `fallback.http`.
//!
//! Configure the orca proxy with an empty route table but a `fallback.http`
//! pointing at a small upstream HTTP server, then send a request with a Host
//! header the proxy doesn't know about. The response body must come from the
//! upstream — that's the only signal the request reached the fallback rather
//! than getting 404'd at the proxy.
//!
//! Regression coverage for `handler::fallback_target` and the no-route
//! branches in `handle_request`. Until this wiring was added, the
//! `FallbackConfig.http` config was dead — accepted in `cluster.toml` but
//! never consulted by the request handler.
//!
//! Run with: `cargo test -p orca-proxy --test e2e_fallback_test -- --ignored`

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use http_body_util::Full;
use hyper::body::{Bytes, Incoming};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use orca_core::config::FallbackConfig;
use orca_proxy::{RouteTarget, run_proxy_with_fallback};
use tokio::net::TcpListener;
use tokio::sync::RwLock;

const FALLBACK_MARKER: &str = "from-fallback-upstream";

/// Spawn a single-handler hyper server that returns `FALLBACK_MARKER` for
/// every request. Returns the bound address.
async fn spawn_fallback_upstream() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (stream, _) = match listener.accept().await {
                Ok(v) => v,
                Err(_) => continue,
            };
            tokio::spawn(async move {
                let io = TokioIo::new(stream);
                let svc = service_fn(|_req: Request<Incoming>| async move {
                    Ok::<_, hyper::Error>(Response::new(Full::new(Bytes::from(FALLBACK_MARKER))))
                });
                let _ = http1::Builder::new().serve_connection(io, svc).await;
            });
        }
    });
    addr
}

/// Spawn the orca proxy with the given route table and fallback config.
/// Returns the bound port.
async fn spawn_proxy(
    route_table: Arc<RwLock<HashMap<String, Vec<RouteTarget>>>>,
    fallback: Option<FallbackConfig>,
) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener); // run_proxy_with_fallback binds itself on the same port

    let triggers = Arc::new(RwLock::new(Vec::new()));
    tokio::spawn(async move {
        let _ =
            run_proxy_with_fallback(route_table, triggers, None, port, None, None, fallback).await;
    });

    // Wait for the proxy to bind. The fn re-binds inside, so we poll with a
    // plain TCP connect. An HTTP probe would be routed through the fallback
    // — which the hung-upstream test points at a black hole, so the probe
    // itself would stall for the full read_timeout before the test begins.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while tokio::time::Instant::now() < deadline {
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            return port;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("orca proxy did not bind on port {port} within 3s");
}

#[tokio::test]
#[ignore]
async fn e2e_unmatched_host_forwards_to_http_fallback() {
    let upstream = spawn_fallback_upstream().await;
    let route_table = Arc::new(RwLock::new(HashMap::new()));
    let fallback = FallbackConfig {
        http: Some(upstream.to_string()),
        tls: None,
    };
    let proxy_port = spawn_proxy(route_table, Some(fallback)).await;

    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let resp = client
        .get(format!("http://127.0.0.1:{proxy_port}/anything"))
        .header("Host", "unknown.example.com")
        .send()
        .await
        .expect("request to proxy");
    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "expected fallback to forward the request (200), got {}",
        resp.status()
    );
    let body = resp.text().await.unwrap();
    assert_eq!(
        body, FALLBACK_MARKER,
        "expected response body from the upstream fallback server, got {body:?}"
    );
}

#[tokio::test]
#[ignore]
async fn e2e_unmatched_host_404s_without_fallback() {
    let route_table = Arc::new(RwLock::new(HashMap::new()));
    let proxy_port = spawn_proxy(route_table, None).await;

    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let resp = client
        .get(format!("http://127.0.0.1:{proxy_port}/anything"))
        .header("Host", "unknown.example.com")
        .send()
        .await
        .expect("request to proxy");
    assert_eq!(
        resp.status(),
        StatusCode::NOT_FOUND,
        "without fallback the proxy must 404, got {}",
        resp.status()
    );
}

/// Spawn a TCP listener that accepts connections but never reads or writes.
/// Simulates a hung backend whose `connect()` succeeds (so connect_timeout
/// doesn't catch it) but which never sends a response.
async fn spawn_blackhole_upstream() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                continue;
            };
            // Hold the socket so the kernel doesn't RST. Drop after a long
            // delay; the test asserts the proxy gives up well before this.
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(900)).await;
                drop(stream);
            });
        }
    });
    addr
}

/// Regression test for the missing-timeouts bug that required restarting the
/// proxy to recover from a hung upstream. Routes an unmatched-host request
/// through `fallback.http` to a black-hole backend and asserts the proxy
/// returns an error once the inactivity window elapses, instead of parking
/// the request indefinitely.
///
/// The recovery bound is the client's `read_timeout` (120s): #49 introduced
/// a 300s total request timeout, #102 replaced it with the inactivity
/// timeout so large blob transfers that keep making progress are never
/// capped. A black-holed upstream sends no bytes, so it must surface as
/// 502 at ~120s — much earlier means some other error path fired, later
/// means the timeout is miswired.
///
/// Long-running by design: ~2 minutes. Marked `#[ignore]` so it only runs
/// in the nightly E2E suite.
#[tokio::test]
#[ignore]
async fn e2e_hung_fallback_recovers_within_read_timeout() {
    let blackhole = spawn_blackhole_upstream().await;
    let route_table = Arc::new(RwLock::new(HashMap::new()));
    let fallback = FallbackConfig {
        http: Some(blackhole.to_string()),
        tls: None,
    };
    let proxy_port = spawn_proxy(route_table, Some(fallback)).await;

    let client = reqwest::Client::builder().no_proxy().build().unwrap();
    let start = std::time::Instant::now();
    let resp = tokio::time::timeout(
        Duration::from_secs(150),
        client
            .get(format!("http://127.0.0.1:{proxy_port}/anything"))
            .header("Host", "unknown-host.example.com")
            .send(),
    )
    .await
    .expect("proxy must return within 150s, not hang forever")
    .expect("the proxy itself must respond");
    let elapsed = start.elapsed();

    assert_eq!(
        resp.status(),
        StatusCode::BAD_GATEWAY,
        "hung upstream should surface as 502, got {}",
        resp.status()
    );
    assert!(
        elapsed >= Duration::from_secs(110),
        "expected ~120s wait (read_timeout inactivity window), got {elapsed:?} — \
         the 502 fired too early to have come from the read_timeout"
    );
}
