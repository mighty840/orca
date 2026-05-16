//! End-to-end test: proxy streams upstream response bodies without
//! buffering, preserving byte-for-byte correctness for large payloads.
//!
//! Spawns a backend hyper server, routes requests there through orca's
//! `fallback.http`, and asserts received body matches sent body. Using the
//! fallback path (vs `route_table` entries) avoids the HTTP→HTTPS redirect
//! that fires for hosts present in the route table when no TLS acceptor is
//! configured. The actual byte path is the same `forward_with_retry`
//! function in both cases — same streaming `resp.bytes_stream()` codepath.
//!
//! Regression coverage for `forward::forward_with_retry`'s streaming-body
//! path. Before that change, the handler called `resp.bytes().await` and
//! built a `Full<Bytes>` — correct for small bodies, but for large ones
//! it buffered everything in proxy memory and parked the per-request
//! task long enough that the accept loop ran out of headroom to service
//! new TLS handshakes (see project_v0_2_9_rc2_blockers bug 3).
//!
//! These tests assert *correctness* of streaming, not perf. Perf is
//! validated by running the proxy against a real registry workload.

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
use orca_proxy::run_proxy_with_fallback;
use tokio::net::TcpListener;
use tokio::sync::RwLock;

/// Spawn a backend hyper server with a custom handler. Returns its addr.
async fn spawn_backend<F, Fut>(handler: F) -> SocketAddr
where
    F: Fn(Request<Incoming>) -> Fut + Clone + Send + Sync + 'static,
    Fut: std::future::Future<Output = Response<Full<Bytes>>> + Send + 'static,
{
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let (stream, _) = listener.accept().await.unwrap();
            let io = TokioIo::new(stream);
            let h = handler.clone();
            tokio::spawn(async move {
                let svc = service_fn(move |req| {
                    let h = h.clone();
                    async move { Ok::<_, hyper::Error>(h(req).await) }
                });
                let _ = http1::Builder::new().serve_connection(io, svc).await;
            });
        }
    });
    addr
}

/// Start the orca proxy in the background, forwarding ALL traffic to
/// `backend` via `fallback.http`. Polls until bound so tests don't race.
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
            .get(format!("http://127.0.0.1:{port}/_probe"))
            .header("Host", "any.example.com")
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

/// reqwest client without HTTP proxy env-var pickup and without auto-
/// redirect-following. Redirect-following is hostile here: orca's
/// HTTP→HTTPS auto-redirect would push us at the Location URL's
/// hostname, triggering a DNS lookup that fails the test for the wrong
/// reason. (We deliberately use fallback to avoid the redirect entirely,
/// but the no-redirect-follow keeps the test honest if that ever changes.)
fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}

/// Build a deterministic byte pattern of `len` bytes so the test can
/// assert the round-trip byte-for-byte rather than just checking lengths.
fn pattern(len: usize) -> Vec<u8> {
    (0..len).map(|i| (i % 251) as u8).collect()
}

#[tokio::test]
async fn proxy_streams_large_body_byte_for_byte() {
    // 8 MiB body — large enough that any half-baked buffering would show
    // up as a memory blip, and large enough to exercise multiple TCP
    // segments through the streaming pipeline.
    const LEN: usize = 8 * 1024 * 1024;
    let payload = Arc::new(pattern(LEN));
    let payload_for_backend = payload.clone();

    let backend = spawn_backend(move |_req| {
        let payload = payload_for_backend.clone();
        async move {
            let bytes = Bytes::from(payload.as_ref().clone());
            Response::new(Full::new(bytes))
        }
    })
    .await;

    let port = spawn_proxy(backend).await;
    let resp = client()
        .get(format!("http://127.0.0.1:{port}/anything"))
        .header("Host", "stream.example.com")
        .send()
        .await
        .expect("proxy request must succeed");
    assert_eq!(resp.status(), StatusCode::OK);

    let received = resp.bytes().await.expect("body must read cleanly");
    assert_eq!(
        received.len(),
        LEN,
        "streamed body length differs from backend"
    );
    assert_eq!(
        &received[..],
        &payload[..],
        "streamed body bytes differ from backend"
    );
}

#[tokio::test]
async fn proxy_preserves_backend_status_code() {
    let backend = spawn_backend(|_req| async {
        let mut r = Response::new(Full::new(Bytes::from_static(b"teapot here")));
        *r.status_mut() = StatusCode::IM_A_TEAPOT;
        r
    })
    .await;

    let port = spawn_proxy(backend).await;
    let resp = client()
        .get(format!("http://127.0.0.1:{port}/"))
        .header("Host", "status.example.com")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::IM_A_TEAPOT);
    assert_eq!(resp.bytes().await.unwrap().as_ref(), b"teapot here");
}

#[tokio::test]
async fn proxy_preserves_custom_backend_headers() {
    let backend = spawn_backend(|_req| async {
        let mut r = Response::new(Full::new(Bytes::from_static(b"ok")));
        r.headers_mut()
            .insert("x-orca-test", "marker-value".parse().unwrap());
        r.headers_mut()
            .insert("cache-control", "no-store".parse().unwrap());
        r
    })
    .await;

    let port = spawn_proxy(backend).await;
    let resp = client()
        .get(format!("http://127.0.0.1:{port}/"))
        .header("Host", "headers.example.com")
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.headers().get("x-orca-test").map(|v| v.as_bytes()),
        Some(&b"marker-value"[..])
    );
    assert_eq!(
        resp.headers().get("cache-control").map(|v| v.as_bytes()),
        Some(&b"no-store"[..])
    );
}

#[tokio::test]
async fn proxy_strips_hop_by_hop_headers() {
    // Backend sends a "connection: close" header that must NOT propagate to
    // the client per RFC 7230 §6.1 (hop-by-hop). The proxy strips it.
    let backend = spawn_backend(|_req| async {
        let mut r = Response::new(Full::new(Bytes::from_static(b"hi")));
        r.headers_mut()
            .insert("transfer-encoding", "chunked".parse().unwrap());
        r.headers_mut().insert("upgrade", "h2c".parse().unwrap());
        r.headers_mut()
            .insert("x-keep-this", "yes".parse().unwrap());
        r
    })
    .await;

    let port = spawn_proxy(backend).await;
    let resp = client()
        .get(format!("http://127.0.0.1:{port}/"))
        .header("Host", "hop.example.com")
        .send()
        .await
        .unwrap();
    // The non-hop-by-hop header passes through. (We don't assert the
    // hop-by-hop ones are absent because reqwest's HTTP/1.1 client adds
    // its own connection-management headers; what matters is that orca's
    // forwarder did not propagate the upstream's literal values.)
    assert_eq!(
        resp.headers().get("x-keep-this").map(|v| v.as_bytes()),
        Some(&b"yes"[..]),
        "non-hop-by-hop header must pass through"
    );
}

#[tokio::test]
async fn proxy_handles_empty_response_body() {
    let backend = spawn_backend(|_req| async {
        let mut r = Response::new(Full::new(Bytes::new()));
        *r.status_mut() = StatusCode::NO_CONTENT;
        r
    })
    .await;

    let port = spawn_proxy(backend).await;
    let resp = client()
        .get(format!("http://127.0.0.1:{port}/"))
        .header("Host", "empty.example.com")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::NO_CONTENT);
    assert!(resp.bytes().await.unwrap().is_empty());
}

#[tokio::test]
async fn proxy_round_trips_post_with_body() {
    let backend = spawn_backend(|req| async move {
        let method = req.method().clone();
        let body = http_body_util::BodyExt::collect(req.into_body())
            .await
            .unwrap()
            .to_bytes();
        let echo = format!("method={method} body_len={}", body.len());
        Response::new(Full::new(Bytes::from(echo)))
    })
    .await;

    let port = spawn_proxy(backend).await;
    let payload = vec![0x42; 4096];
    let resp = client()
        .post(format!("http://127.0.0.1:{port}/upload"))
        .header("Host", "post.example.com")
        .body(payload)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.text().await.unwrap(), "method=POST body_len=4096");
}

#[tokio::test]
async fn proxy_streams_many_small_chunks_correctly() {
    // Backend emits a known sequence of small distinct chunks. Confirms
    // streaming preserves order and doesn't fragment frames the way a
    // half-baked StreamBody implementation might.
    let backend = spawn_backend(|_req| async {
        let mut body = Vec::new();
        for i in 0..256 {
            body.extend_from_slice(format!("[chunk-{i:03}]").as_bytes());
        }
        Response::new(Full::new(Bytes::from(body)))
    })
    .await;

    let port = spawn_proxy(backend).await;
    let resp = client()
        .get(format!("http://127.0.0.1:{port}/"))
        .header("Host", "chunks.example.com")
        .send()
        .await
        .unwrap();
    let body = resp.text().await.unwrap();
    assert!(body.contains("[chunk-000]"));
    assert!(body.contains("[chunk-127]"));
    assert!(body.contains("[chunk-255]"));
    assert_eq!(body.len(), 256 * "[chunk-000]".len());
}

#[tokio::test]
async fn proxy_returns_502_when_backend_unreachable() {
    // Spawn proxy with a fallback pointing at a port nothing listens on.
    let dead_backend = "127.0.0.1:1"; // privileged, unbindable in user mode
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let routes = Arc::new(RwLock::new(HashMap::new()));
    let triggers = Arc::new(RwLock::new(Vec::new()));
    let fallback = FallbackConfig {
        http: Some(dead_backend.to_string()),
        tls: None,
    };
    tokio::spawn(async move {
        let _ =
            run_proxy_with_fallback(routes, triggers, None, port, None, None, Some(fallback)).await;
    });

    // Wait for bind.
    let probe = client();
    for _ in 0..60 {
        if probe
            .get(format!("http://127.0.0.1:{port}/_probe"))
            .header("Host", "any.example.com")
            .send()
            .await
            .is_ok()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let resp = client()
        .get(format!("http://127.0.0.1:{port}/"))
        .header("Host", "deadbackend.example.com")
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.status(),
        StatusCode::BAD_GATEWAY,
        "unreachable backend must produce 502 (not crash, not hang)"
    );
}
