//! #192: an upstream failure must not echo the backend's internal address.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use orca_proxy::{RouteTarget, run_proxy_with_fallback};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::RwLock;

async fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// #206: hostnames are case-insensitive; a mixed-case Host must route.
#[tokio::test]
async fn a_mixed_case_host_header_is_routed() {
    let dead = free_port().await;
    let routes = Arc::new(RwLock::new(HashMap::from([(
        "app.test".to_string(),
        vec![RouteTarget {
            address: format!("127.0.0.1:{dead}"),
            service_name: "app".into(),
            path_pattern: None,
            weight: 100,
            strip_prefix: None,
        }],
    )])));
    let port = free_port().await;
    let triggers = Arc::new(RwLock::new(Vec::new()));
    tokio::spawn(async move {
        let _ = run_proxy_with_fallback(routes, triggers, None, port, None, None, None).await;
    });
    while TcpStream::connect(("127.0.0.1", port)).await.is_err() {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let resp = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{port}/"))
        .header("host", "App.Test")
        .send()
        .await
        .unwrap();

    // Routed to the (dead) backend: 502, not the unknown-host 404.
    assert_eq!(resp.status(), 502);
}

#[tokio::test]
async fn a_dead_backend_gives_a_502_without_leaking_its_address() {
    let dead = free_port().await;
    let routes = Arc::new(RwLock::new(HashMap::from([(
        "app.test".to_string(),
        vec![RouteTarget {
            address: format!("127.0.0.1:{dead}"),
            service_name: "app".into(),
            path_pattern: None,
            weight: 100,
            strip_prefix: None,
        }],
    )])));
    let port = free_port().await;
    let triggers = Arc::new(RwLock::new(Vec::new()));
    tokio::spawn(async move {
        let _ = run_proxy_with_fallback(routes, triggers, None, port, None, None, None).await;
    });
    while TcpStream::connect(("127.0.0.1", port)).await.is_err() {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let resp = reqwest::Client::new()
        .get(format!("http://127.0.0.1:{port}/"))
        .header("host", "app.test")
        .send()
        .await
        .unwrap();
    let status = resp.status();
    let body = resp.text().await.unwrap();

    assert_eq!(status, 502);
    assert!(
        !body.contains("127.0.0.1"),
        "internal address leaked: {body}"
    );
}

/// Starts a proxy over `routes` and returns the status of a GET for `host`.
async fn status_for(routes: HashMap<String, Vec<RouteTarget>>, host: &str) -> u16 {
    let routes = Arc::new(RwLock::new(routes));
    let port = free_port().await;
    let triggers = Arc::new(RwLock::new(Vec::new()));
    tokio::spawn(async move {
        let _ = run_proxy_with_fallback(routes, triggers, None, port, None, None, None).await;
    });
    while TcpStream::connect(("127.0.0.1", port)).await.is_err() {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    reqwest::Client::new()
        .get(format!("http://127.0.0.1:{port}/v1/logs"))
        .header("host", host)
        .send()
        .await
        .unwrap()
        .status()
        .as_u16()
}

/// A known host with no backend right now (a redeploy's container swap)
/// answers 503, which clients retry; OTLP exporters drop data on a 404.
#[tokio::test]
async fn a_known_host_without_targets_answers_503() {
    let routes = HashMap::from([("otel.test".to_string(), Vec::new())]);
    assert_eq!(status_for(routes, "otel.test").await, 503);
}

#[tokio::test]
async fn an_unknown_host_still_answers_404() {
    let routes = HashMap::from([("otel.test".to_string(), Vec::new())]);
    assert_eq!(status_for(routes, "other.test").await, 404);
}
