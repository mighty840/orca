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
