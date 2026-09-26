//! #190: with more than one backend, the proxy read every request body fully
//! into memory before forwarding it. A large or open-ended upload must be
//! streamed instead. Observable here: the backend answers as soon as the
//! request starts, while the client is still sending, and that answer has
//! to reach the client without waiting for the whole body.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use futures_util::StreamExt as _;
use http_body_util::{BodyExt as _, Full};
use hyper::body::{Bytes, Incoming};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response};
use hyper_util::rt::TokioIo;
use orca_proxy::{RouteTarget, run_proxy_with_fallback};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::RwLock;

/// A backend that answers 200 after the first body chunk and keeps reading
/// the rest in the background, like a registry acknowledging an upload.
async fn spawn_eager_backend() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            tokio::spawn(async move {
                let svc = service_fn(|req: Request<Incoming>| async {
                    let mut body = req.into_body();
                    let _ = body.frame().await;
                    tokio::spawn(async move { while body.frame().await.is_some() {} });
                    Ok::<_, hyper::Error>(Response::new(Full::new(Bytes::from_static(b"ok"))))
                });
                let _ = http1::Builder::new()
                    .serve_connection(TokioIo::new(stream), svc)
                    .await;
            });
        }
    });
    addr
}

async fn spawn_proxy(targets: Vec<SocketAddr>) -> u16 {
    let port = TcpListener::bind("127.0.0.1:0")
        .await
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let routes = Arc::new(RwLock::new(HashMap::from([(
        "registry.test".to_string(),
        targets
            .into_iter()
            .map(|a| RouteTarget {
                address: a.to_string(),
                service_name: "registry".into(),
                path_pattern: None,
                weight: 100,
                strip_prefix: None,
            })
            .collect(),
    )])));
    let triggers = Arc::new(RwLock::new(Vec::new()));
    tokio::spawn(async move {
        let _ = run_proxy_with_fallback(routes, triggers, None, port, None, None, None).await;
    });
    while TcpStream::connect(("127.0.0.1", port)).await.is_err() {
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    port
}

#[tokio::test]
async fn a_large_upload_to_a_multi_target_route_is_streamed() {
    let backend = spawn_eager_backend().await;
    // Two targets: the route that used to buffer every body.
    let port = spawn_proxy(vec![backend, backend]).await;

    // Unknown length: one chunk now, the rest in 10 s.
    let body =
        futures_util::stream::iter([Ok::<_, std::io::Error>(Bytes::from(vec![b'x'; 64 * 1024]))])
            .chain(futures_util::stream::once(async {
                tokio::time::sleep(Duration::from_secs(10)).await;
                Ok(Bytes::from_static(b"end"))
            }));
    let request = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{port}/v2/blobs/uploads/"))
        .header("host", "registry.test")
        .body(reqwest::Body::wrap_stream(body))
        .send();

    let resp = tokio::time::timeout(Duration::from_secs(3), request)
        .await
        .expect("buffering the whole body would wait for the client to finish")
        .unwrap();
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    assert_eq!(status, 200, "{text}");
}
