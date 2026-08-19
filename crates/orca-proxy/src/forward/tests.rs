use super::*;

#[test]
fn test_redirect_to_https_returns_301() {
    let resp = redirect_to_https("example.com", "/some/path");
    assert_eq!(resp.status(), StatusCode::MOVED_PERMANENTLY);
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
    assert_eq!(resp.status(), StatusCode::MOVED_PERMANENTLY);
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
