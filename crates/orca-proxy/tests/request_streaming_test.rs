//! End-to-end test: the proxy streams large *request* bodies for
//! single-target routes without buffering them (issue #72). This is the
//! symmetric counterpart to the response-body streaming covered in
//! `streaming_body_test.rs` (#63).
//!
//! A single-target route — here the `fallback.http` path, which resolves to
//! exactly one target — takes `forward::forward_streaming`, which wraps the
//! incoming hyper body as a reqwest stream instead of `collect()`-ing it into
//! `Bytes`. The backend echoes the request body straight back, so the test
//! asserts the round trip byte-for-byte for a payload far larger than any
//! sane intermediate buffer. Before this change, a 100MB+ registry blob push
//! was fully buffered in the proxy task before forwarding — a contributing
//! factor in the accept-loop starvation seen in v0.2.9-rc.2.
//!
//! This asserts *correctness* of request streaming, not perf; perf is
//! validated against a real registry push workload.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use http_body_util::{BodyExt, Full};
use hyper::body::Incoming;
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use orca_core::config::FallbackConfig;
use orca_proxy::run_proxy_with_fallback;
use tokio::net::TcpListener;
use tokio::sync::RwLock;

/// Spawn a backend that echoes the received request body back verbatim.
async fn spawn_echo_backend() -> SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let io = TokioIo::new(stream);
            tokio::spawn(async move {
                let svc = service_fn(|req: Request<Incoming>| async move {
                    let body = req.into_body().collect().await.unwrap().to_bytes();
                    Ok::<_, hyper::Error>(Response::new(Full::new(body)))
                });
                let _ = http1::Builder::new().serve_connection(io, svc).await;
            });
        }
    });
    addr
}

/// Start the orca proxy forwarding ALL traffic to `backend` via `fallback.http`
/// (a single-target route, so the streaming forward path is exercised). Polls
/// until bound so tests don't race.
async fn spawn_proxy(backend: SocketAddr) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let routes = Arc::new(RwLock::new(HashMap::new()));
    let triggers = Arc::new(RwLock::new(Vec::new()));
    let fallback = FallbackConfig {
        http: Some(backend.to_string()),
        tls: None,
    };

    tokio::spawn(async move {
        let _ =
            run_proxy_with_fallback(routes, triggers, None, port, None, None, Some(fallback)).await;
    });

    let probe = client();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(3);
    while tokio::time::Instant::now() < deadline {
        if probe
            .post(format!("http://127.0.0.1:{port}/_probe"))
            .header("Host", "any.example.com")
            .body("probe")
            .send()
            .await
            .is_ok()
        {
            return port;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("proxy on port {port} never came up");
}

/// reqwest client with no proxy env pickup and no redirect-following (matches
/// the response-streaming test's rationale).
fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

/// Deterministic byte pattern so the round trip is asserted byte-for-byte.
fn pattern(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i % 251) as u8).collect()
}

#[tokio::test]
async fn proxy_streams_large_request_body_byte_for_byte() {
    // 16 MiB request body — large enough that any buffering regression would
    // show as a memory blip, and large enough to span many TCP segments
    // through the streaming pipeline. Models a registry blob push.
    const LEN: usize = 16 * 1024 * 1024;
    let payload = pattern(LEN);

    let backend = spawn_echo_backend().await;
    let port = spawn_proxy(backend).await;

    let resp = client()
        .post(format!("http://127.0.0.1:{port}/v2/blobs/uploads/"))
        .header("Host", "registry.example.com")
        .body(payload.clone())
        .send()
        .await
        .expect("proxy request must succeed");
    assert_eq!(resp.status(), StatusCode::OK);

    let echoed = resp.bytes().await.expect("body must read cleanly");
    assert_eq!(
        echoed.len(),
        LEN,
        "streamed request body length differs from what was sent"
    );
    assert_eq!(
        &echoed[..],
        &payload[..],
        "streamed request body bytes differ from what was sent"
    );
}

#[tokio::test]
async fn proxy_streams_large_upload_arriving_over_time() {
    // Regression for the >2GB registry-upload bug: the proxy used a *total*
    // request timeout (300s) which capped uploads by wall-clock — a large blob
    // over a typical link exceeds it and died mid-transfer. The fix uses an
    // inactivity timeout instead. This models a registry blob push that streams
    // in over real time and asserts it completes byte-count-clean. It also
    // guards the no-buffer streaming path and would catch a reintroduced tight
    // total-timeout (the upload deliberately spans hundreds of ms).
    const CHUNK: usize = 1024 * 1024; // 1 MiB
    const CHUNKS: usize = 64; // 64 MiB total, streamed
    let backend = spawn_echo_backend().await;
    let port = spawn_proxy(backend).await;

    // Trickle 1 MiB frames with periodic gaps so the request body arrives over
    // wall-clock time rather than all at once — an active-but-slow transfer.
    let stream = futures_util::stream::unfold(0usize, |i| async move {
        if i >= CHUNKS {
            return None;
        }
        if i > 0 && i % 8 == 0 {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        let frame = vec![(i % 251) as u8; CHUNK];
        Some((Ok::<Vec<u8>, std::io::Error>(frame), i + 1))
    });

    let resp = client()
        .post(format!("http://127.0.0.1:{port}/v2/blobs/uploads/"))
        .header("Host", "registry.example.com")
        .body(reqwest::Body::wrap_stream(stream))
        .send()
        .await
        .expect("streamed large upload must succeed");
    assert_eq!(resp.status(), StatusCode::OK);

    let echoed = resp.bytes().await.expect("response body reads cleanly");
    assert_eq!(
        echoed.len(),
        CHUNK * CHUNKS,
        "every streamed byte must round-trip through the proxy"
    );
}

#[tokio::test]
async fn proxy_streams_empty_request_body() {
    // A bodyless GET on a single-target route must still forward cleanly
    // through the streaming path (empty stream, not a hang).
    let backend = spawn_echo_backend().await;
    let port = spawn_proxy(backend).await;

    let resp = client()
        .get(format!("http://127.0.0.1:{port}/"))
        .header("Host", "registry.example.com")
        .send()
        .await
        .expect("proxy request must succeed");
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(resp.bytes().await.unwrap().is_empty());
}
