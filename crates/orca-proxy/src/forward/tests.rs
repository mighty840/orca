use super::*;

#[test]
fn test_redirect_to_https_returns_308() {
    let resp = redirect_to_https("example.com", "/some/path");
    assert_eq!(resp.status(), StatusCode::PERMANENT_REDIRECT);
}

#[test]
fn test_redirect_to_https_location_header() {
    let resp = redirect_to_https("example.com", "/foo?bar=1");
    let location = resp
        .headers()
        .get(hyper::header::LOCATION)
        .expect("should have Location header")
        .to_str()
        .unwrap();
    assert_eq!(location, "https://example.com/foo?bar=1");
}

#[test]
fn test_redirect_to_https_root_path() {
    let resp = redirect_to_https("myapp.dev", "/");
    let location = resp
        .headers()
        .get(hyper::header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(location, "https://myapp.dev/");
    assert_eq!(resp.status(), StatusCode::PERMANENT_REDIRECT);
}

#[test]
fn test_redirect_preserves_complex_path() {
    let resp = redirect_to_https("sub.example.com", "/a/b/c?x=1&y=2");
    let location = resp
        .headers()
        .get(hyper::header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(location, "https://sub.example.com/a/b/c?x=1&y=2");
}

fn make_target(addr: &str, weight: u32) -> RouteTarget {
    RouteTarget {
        address: addr.to_string(),
        service_name: addr.to_string(),
        path_pattern: None,
        strip_prefix: None,
        weight,
    }
}

#[test]
fn weighted_index_equal_weights_round_robins() {
    let targets = vec![make_target("a:80", 100), make_target("b:80", 100)];
    assert_eq!(weighted_index(&targets, 0), 0);
    assert_eq!(weighted_index(&targets, 1), 1);
    assert_eq!(weighted_index(&targets, 2), 0);
}

#[test]
fn weighted_index_single_target() {
    let targets = vec![make_target("a:80", 50)];
    assert_eq!(weighted_index(&targets, 0), 0);
    assert_eq!(weighted_index(&targets, 99), 0);
}

#[test]
fn weighted_index_80_20_distribution() {
    let targets = vec![make_target("old:80", 80), make_target("new:80", 20)];
    let mut counts = [0u32; 2];
    for i in 0..100 {
        counts[weighted_index(&targets, i)] += 1;
    }
    assert_eq!(counts[0], 80);
    assert_eq!(counts[1], 20);
}

#[test]
fn weighted_index_zero_total_falls_back() {
    let targets = vec![make_target("a:80", 0), make_target("b:80", 0)];
    // Should not panic, falls back to round-robin
    let idx = weighted_index(&targets, 0);
    assert!(idx < targets.len());
}

#[test]
fn forwarded_for_value_appends_peer_to_a_client_chain() {
    // The forged prefix is preserved but ends up to the LEFT of our peer,
    // where hop-counting consumers ignore it.
    assert_eq!(
        forwarded_for_value(Some("9.9.9.9"), "203.0.113.7"),
        "9.9.9.9, 203.0.113.7"
    );
    // A multi-entry chain is preserved wholesale, peer still appended last.
    assert_eq!(
        forwarded_for_value(Some("9.9.9.9, 10.0.0.1"), "203.0.113.7"),
        "9.9.9.9, 10.0.0.1, 203.0.113.7"
    );
}

#[test]
fn forwarded_for_value_is_peer_only_when_chain_absent_or_blank() {
    assert_eq!(forwarded_for_value(None, "203.0.113.7"), "203.0.113.7");
    assert_eq!(forwarded_for_value(Some(""), "203.0.113.7"), "203.0.113.7");
    assert_eq!(
        forwarded_for_value(Some("   "), "203.0.113.7"),
        "203.0.113.7"
    );
}

#[tokio::test]
async fn build_forward_request_appends_peer_and_never_trusts_client_xff() {
    // Regression guard for the spoof: a client pre-fills X-Forwarded-For,
    // and the proxy must (a) NOT forward that value as its own header and
    // (b) emit exactly ONE X-Forwarded-For whose right-most entry is the
    // peer we observed. Two header lines would let HeaderMap::get read the
    // client's forged copy first.
    let client = reqwest::Client::new();
    let target = make_target("127.0.0.1:8080", 100);
    let mut headers = hyper::HeaderMap::new();
    headers.insert(
        hyper::header::HeaderName::from_static("x-forwarded-for"),
        hyper::header::HeaderValue::from_static("9.9.9.9"),
    );

    let (builder, _uri) = build_forward_request(
        &client,
        &target,
        &reqwest::Method::GET,
        &headers,
        "/",
        "example.com",
        true,
        "203.0.113.7", // the peer the proxy actually saw
    );
    let req = builder.build().expect("request should build");

    let values: Vec<_> = req
        .headers()
        .get_all("x-forwarded-for")
        .iter()
        .map(|v| v.to_str().unwrap().to_string())
        .collect();
    assert_eq!(
        values,
        vec!["9.9.9.9, 203.0.113.7".to_string()],
        "must be a single merged header ending in the observed peer"
    );
}

#[tokio::test]
async fn build_forward_request_merges_multiple_xff_header_lines() {
    // A legitimate multi-hop chain can arrive as several X-Forwarded-For
    // lines (RFC 7230). All segments must survive, in order, with the
    // observed peer appended last — not be collapsed to the final line.
    let client = reqwest::Client::new();
    let target = make_target("127.0.0.1:8080", 100);
    let mut headers = hyper::HeaderMap::new();
    headers.append(
        hyper::header::HeaderName::from_static("x-forwarded-for"),
        hyper::header::HeaderValue::from_static("1.1.1.1"),
    );
    headers.append(
        hyper::header::HeaderName::from_static("x-forwarded-for"),
        hyper::header::HeaderValue::from_static("2.2.2.2"),
    );

    let (builder, _uri) = build_forward_request(
        &client,
        &target,
        &reqwest::Method::GET,
        &headers,
        "/",
        "example.com",
        true,
        "203.0.113.7",
    );
    let req = builder.build().expect("request should build");

    let values: Vec<_> = req
        .headers()
        .get_all("x-forwarded-for")
        .iter()
        .map(|v| v.to_str().unwrap().to_string())
        .collect();
    assert_eq!(
        values,
        vec!["1.1.1.1, 2.2.2.2, 203.0.113.7".to_string()],
        "every incoming segment must be preserved, peer appended last"
    );
}

#[tokio::test]
async fn build_forward_request_sets_peer_when_client_sends_no_xff() {
    let client = reqwest::Client::new();
    let target = make_target("127.0.0.1:8080", 100);
    let headers = hyper::HeaderMap::new();

    let (builder, _uri) = build_forward_request(
        &client,
        &target,
        &reqwest::Method::GET,
        &headers,
        "/",
        "example.com",
        true,
        "203.0.113.7",
    );
    let req = builder.build().expect("request should build");
    assert_eq!(
        req.headers()
            .get("x-forwarded-for")
            .unwrap()
            .to_str()
            .unwrap(),
        "203.0.113.7"
    );
}

/// A backend that answers the first request on a connection, then closes
/// the connection under the second one, as an idle timeout does when the
/// proxy reuses the pooled connection at that moment (#301). Every later
/// connection is answered normally. Returns the address and a counter of
/// accepted connections.
async fn closes_reused_connection() -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn read_request(sock: &mut tokio::net::TcpStream) -> bool {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 1024];
        while !buf.windows(4).any(|w| w == b"\r\n\r\n") {
            match sock.read(&mut chunk).await {
                Ok(0) | Err(_) => return false,
                Ok(n) => buf.extend_from_slice(&chunk[..n]),
            }
        }
        true
    }
    const OK: &[u8] = b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\n\r\nok";

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let accepted = std::sync::Arc::new(AtomicUsize::new(0));
    let count = accepted.clone();
    tokio::spawn(async move {
        loop {
            let (mut sock, _) = listener.accept().await.unwrap();
            let first = count.fetch_add(1, Ordering::SeqCst) == 0;
            tokio::spawn(async move {
                if !read_request(&mut sock).await {
                    return;
                }
                sock.write_all(OK).await.unwrap();
                if first {
                    // Read the reused request, then close without answering.
                    read_request(&mut sock).await;
                    return;
                }
                while read_request(&mut sock).await {
                    if sock.write_all(OK).await.is_err() {
                        return;
                    }
                }
            });
        }
    });
    (addr, accepted)
}

#[tokio::test]
async fn a_get_on_a_connection_the_backend_closed_is_retried_once() {
    let (addr, accepted) = closes_reused_connection().await;
    let client = crate::backend_timeouts::client();
    let url = format!("http://{addr}/healthz");
    // Put a keep-alive connection in the pool.
    assert_eq!(client.get(&url).send().await.unwrap().status(), 200);

    let resp = send_bodyless(
        client.get(&url),
        || client.get(&url),
        &reqwest::Method::GET,
        &addr,
        "/healthz",
    )
    .await
    .expect("the retry on a fresh connection must succeed");

    assert_eq!(resp.status(), 200);
    assert_eq!(accepted.load(std::sync::atomic::Ordering::SeqCst), 2);
}

#[tokio::test]
async fn an_empty_post_on_a_connection_the_backend_closed_is_not_retried() {
    let (addr, accepted) = closes_reused_connection().await;
    let client = crate::backend_timeouts::client();
    let url = format!("http://{addr}/session/refresh");
    assert_eq!(client.get(&url).send().await.unwrap().status(), 200);

    let err = send_bodyless(
        client.post(&url),
        || client.post(&url),
        &reqwest::Method::POST,
        &addr,
        "/session/refresh",
    )
    .await
    .expect_err("a POST must not be sent twice");

    assert!(crate::backend_timeouts::is_stale_connection(&err), "{err}");
    assert_eq!(err.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(accepted.load(std::sync::atomic::Ordering::SeqCst), 1);
}
