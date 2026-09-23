use super::*;

// --- build_ws_url -----------------------------------------------------------

#[test]
fn url_carries_identity_but_never_the_token() {
    let url = build_ws_url("http://46.225.100.82:6880", 123, "10.0.0.5:6881");
    assert_eq!(
        url,
        "ws://46.225.100.82:6880/api/v1/ws/agent?node_id=123&address=10.0.0.5%3A6881"
    );
    assert!(!url.contains("token"));
}

#[test]
fn https_leader_becomes_wss() {
    let url = build_ws_url("https://orca.example.com", 42, "192.168.1.5:6881");
    assert_eq!(
        url,
        "wss://orca.example.com/api/v1/ws/agent?node_id=42&address=192.168.1.5%3A6881"
    );
}

// --- build_ws_request -------------------------------------------------------

#[test]
fn request_carries_the_token_as_a_bearer_header() {
    let url = build_ws_url("http://127.0.0.1:6880", 1, "a:1");
    let req = build_ws_request(&url, "s3cret").unwrap();
    let auth = req.headers().get(AUTHORIZATION).unwrap();
    assert_eq!(auth.to_str().unwrap(), "Bearer s3cret");
    assert!(!req.uri().to_string().contains("s3cret"));
}

#[test]
fn bearer_header_is_marked_sensitive() {
    let url = build_ws_url("http://127.0.0.1:6880", 1, "a:1");
    let req = build_ws_request(&url, "s3cret").unwrap();
    assert!(req.headers().get(AUTHORIZATION).unwrap().is_sensitive());
    // Sensitive values are redacted from Debug output.
    assert!(!format!("{req:?}").contains("s3cret"));
}

#[test]
fn request_keeps_the_websocket_handshake_headers() {
    let url = build_ws_url("http://127.0.0.1:6880", 1, "a:1");
    let req = build_ws_request(&url, "t").unwrap();
    for header in [
        "upgrade",
        "connection",
        "sec-websocket-key",
        "sec-websocket-version",
    ] {
        assert!(req.headers().contains_key(header), "missing {header}");
    }
}

#[test]
fn a_token_that_cannot_be_a_header_is_an_error_not_a_panic() {
    let url = build_ws_url("http://127.0.0.1:6880", 1, "a:1");
    assert!(build_ws_request(&url, "bad\ntoken").is_err());
}

// --- plaintext_exposure -----------------------------------------------------

#[test]
fn plaintext_to_a_public_ip_is_exposed() {
    // The breakpilot agent's actual join target today.
    assert_eq!(
        plaintext_exposure("ws://46.225.100.82:6880/api/v1/ws/agent?node_id=1"),
        Some("46.225.100.82".to_string())
    );
    assert_eq!(
        plaintext_exposure("ws://[2a01:4f8:1c19:e903::1]:6880/x"),
        Some("2a01:4f8:1c19:e903::1".to_string())
    );
}

#[test]
fn plaintext_to_a_hostname_is_treated_as_exposed() {
    assert_eq!(
        plaintext_exposure("ws://orca.example.com:6880/x"),
        Some("orca.example.com".to_string())
    );
}

#[test]
fn private_mesh_and_loopback_addresses_are_not_exposed() {
    for url in [
        "ws://100.80.5.14:6880/x", // the master's NetBird address
        "ws://100.64.0.1/x",
        "ws://100.127.255.254/x",
        "ws://10.0.0.1:6880/x",
        "ws://172.16.5.4/x",
        "ws://192.168.1.10/x",
        "ws://169.254.1.1/x",
        "ws://127.0.0.1:6880/x",
        "ws://localhost:6880/x",
        "ws://[::1]:6880/x",
        "ws://[fd00::1]:6880/x",
        "ws://[fe80::1]:6880/x",
    ] {
        assert_eq!(plaintext_exposure(url), None, "{url}");
    }
}

#[test]
fn the_cgnat_range_boundary_is_exact() {
    // 100.64.0.0/10 spans 100.64 through 100.127; its neighbors are public.
    assert!(plaintext_exposure("ws://100.63.255.255/x").is_some());
    assert!(plaintext_exposure("ws://100.128.0.0/x").is_some());
}

#[test]
fn tls_is_never_exposed() {
    assert_eq!(plaintext_exposure("wss://46.225.100.82:6880/x"), None);
    assert_eq!(plaintext_exposure("wss://orca.example.com/x"), None);
}
