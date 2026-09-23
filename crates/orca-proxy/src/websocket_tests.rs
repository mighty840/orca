use super::*;

/// A backend 101 as Prosody sends it for Jitsi's XMPP-over-WebSocket (#191).
const PROSODY_101: &[u8] = b"HTTP/1.1 101 Switching Protocols\r\n\
    Upgrade: websocket\r\n\
    Connection: Upgrade\r\n\
    Sec-WebSocket-Accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n\
    Sec-WebSocket-Protocol: xmpp\r\n\
    Sec-WebSocket-Extensions: permessage-deflate; client_max_window_bits=15\r\n\
    X-Backend-Internal: secret\r\n\
    \r\n";

fn header<'a>(resp: &'a Response<crate::body::ProxyBody>, name: &str) -> Vec<&'a str> {
    resp.headers()
        .get_all(name)
        .iter()
        .map(|v| v.to_str().unwrap())
        .collect()
}

#[test]
fn response_relays_selected_subprotocol() {
    // The #191 regression: a client that offers subprotocols MUST see the
    // selected one echoed, or the browser aborts the handshake (Jitsi's
    // `xmpp` over WebSocket dropped every call).
    let resp = switching_protocols_response(PROSODY_101);
    assert_eq!(resp.status(), StatusCode::SWITCHING_PROTOCOLS);
    assert_eq!(header(&resp, "sec-websocket-protocol"), ["xmpp"]);
}

#[test]
fn response_relays_accept_and_extensions() {
    let resp = switching_protocols_response(PROSODY_101);
    assert_eq!(
        header(&resp, "sec-websocket-accept"),
        ["s3pPLMBiTxaQ9kYGzzhZRbK+xOo="]
    );
    assert_eq!(
        header(&resp, "sec-websocket-extensions"),
        ["permessage-deflate; client_max_window_bits=15"]
    );
    assert_eq!(header(&resp, "upgrade"), ["websocket"]);
    assert_eq!(header(&resp, "connection"), ["Upgrade"]);
}

#[test]
fn response_does_not_relay_unrelated_backend_headers() {
    let resp = switching_protocols_response(PROSODY_101);
    assert!(resp.headers().get("x-backend-internal").is_none());
}

#[test]
fn response_keeps_repeated_extension_lines_and_is_case_insensitive() {
    let head = b"HTTP/1.1 101 Switching Protocols\r\n\
        SEC-WEBSOCKET-EXTENSIONS: permessage-deflate\r\n\
        sec-websocket-extensions: x-custom\r\n\r\n";
    let resp = switching_protocols_response(head);
    assert_eq!(
        header(&resp, "sec-websocket-extensions"),
        ["permessage-deflate", "x-custom"]
    );
    // No subprotocol selected by the backend → none invented.
    assert!(resp.headers().get("sec-websocket-protocol").is_none());
}

#[test]
fn status_check_parses_the_code_token() {
    assert!(is_switching_protocols(PROSODY_101));
    assert!(is_switching_protocols(b"HTTP/1.1 101\r\n\r\n"));
    assert!(!is_switching_protocols(b"HTTP/1.1 200 OK-101\r\n\r\n"));
    assert!(!is_switching_protocols(b"HTTP/1.1 400 Bad Request\r\n\r\n"));
    assert!(!is_switching_protocols(b""));
}

fn upgrade_headers() -> HeaderMap {
    let mut h = HeaderMap::new();
    h.insert("host", HeaderValue::from_static("meet.example.com"));
    h.insert("upgrade", HeaderValue::from_static("websocket"));
    h.insert("sec-websocket-protocol", HeaderValue::from_static("xmpp"));
    h
}

fn request_text(headers: &HeaderMap, is_tls: bool) -> String {
    let raw = build_upgrade_request(
        &Method::GET,
        "/xmpp-websocket?room=a",
        headers,
        "meet.example.com",
        is_tls,
        "203.0.113.7",
    );
    String::from_utf8(raw).unwrap()
}

#[test]
fn request_carries_line_headers_and_forwarded_set() {
    let text = request_text(&upgrade_headers(), true);
    assert!(text.starts_with("GET /xmpp-websocket?room=a HTTP/1.1\r\n"));
    assert!(text.ends_with("\r\n\r\n"));
    assert!(text.contains("sec-websocket-protocol: xmpp\r\n"));
    assert!(text.contains("x-forwarded-proto: https\r\n"));
    assert!(text.contains("x-forwarded-host: meet.example.com\r\n"));
    assert!(text.contains("x-forwarded-for: 203.0.113.7\r\n"));
    assert!(request_text(&upgrade_headers(), false).contains("x-forwarded-proto: http\r\n"));
}

#[test]
fn request_appends_peer_to_client_xff_instead_of_passing_it_through() {
    let mut h = upgrade_headers();
    h.insert("x-forwarded-for", HeaderValue::from_static("1.2.3.4"));
    let text = request_text(&h, true);
    assert!(text.contains("x-forwarded-for: 1.2.3.4, 203.0.113.7\r\n"));
    assert_eq!(text.matches("x-forwarded-for:").count(), 1);
}

#[test]
fn request_keeps_upstream_forwarded_proto_and_host() {
    let mut h = upgrade_headers();
    h.insert("x-forwarded-proto", HeaderValue::from_static("https"));
    h.insert(
        "x-forwarded-host",
        HeaderValue::from_static("edge.example.com"),
    );
    let text = request_text(&h, false);
    assert_eq!(text.matches("x-forwarded-proto:").count(), 1);
    assert!(text.contains("x-forwarded-proto: https\r\n"));
    assert_eq!(text.matches("x-forwarded-host:").count(), 1);
    assert!(text.contains("x-forwarded-host: edge.example.com\r\n"));
}

#[test]
fn request_keeps_non_utf8_header_values() {
    let mut h = upgrade_headers();
    h.insert("x-legacy", HeaderValue::from_bytes(b"caf\xe9").unwrap());
    let raw = build_upgrade_request(&Method::GET, "/", &h, "h", false, "1.1.1.1");
    let needle = b"x-legacy: caf\xe9\r\n";
    assert!(raw.windows(needle.len()).any(|w| w == needle));
}
