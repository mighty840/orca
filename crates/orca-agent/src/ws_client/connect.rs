//! Building the agent's connection to the master's control channel.
//!
//! The cluster token used to travel in the URL query string (#182), so it was
//! written to the journal on every reconnect and to any proxy access log on
//! the way. It now travels in an `Authorization` header, and the URL carries
//! only the node's identity. Upgrade the master before its agents: an older
//! master reads the token only from the query string and refuses this request.

use std::net::IpAddr;

use tokio_tungstenite::tungstenite;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::header::AUTHORIZATION;
use tokio_tungstenite::tungstenite::http::{HeaderValue, Request};

/// The master's agent-channel URL. It carries no credential, so it is safe
/// to log.
pub fn build_ws_url(leader_url: &str, node_id: u64, address: &str) -> String {
    let base = leader_url
        .replace("https://", "wss://")
        .replace("http://", "ws://");
    let encoded_addr = address.replace(':', "%3A");
    format!("{base}/api/v1/ws/agent?node_id={node_id}&address={encoded_addr}")
}

/// A WebSocket upgrade request for `url`, authenticated with `token` in an
/// `Authorization: Bearer` header.
///
/// The header value is marked sensitive, so it is redacted if the request is
/// ever debug-printed.
// `tungstenite::Error` is at least 136 bytes, so clippy 1.98 flags this under
// `result_large_err`. It is not our type, and the caller matches it alongside
// `connect_async`'s own unboxed `tungstenite::Error`, so boxing here would be
// unboxed again immediately. Called once per reconnect attempt.
#[allow(clippy::result_large_err)]
pub fn build_ws_request(url: &str, token: &str) -> Result<Request<()>, tungstenite::Error> {
    let mut request = url.into_client_request()?;
    let mut value = HeaderValue::from_str(&format!("Bearer {token}"))
        .map_err(|e| tungstenite::Error::HttpFormat(e.into()))?;
    value.set_sensitive(true);
    request.headers_mut().insert(AUTHORIZATION, value);
    Ok(request)
}

/// The master host, if `url` would carry the token and this node's resolved
/// secrets in cleartext over a network that may be public.
///
/// `wss://` is never exposed. `ws://` is accepted to loopback, RFC 1918
/// private ranges, link-local, IPv6 unique-local, and `100.64.0.0/10` — the
/// range NetBird and Tailscale assign, whose traffic is already encrypted by
/// WireGuard. A hostname cannot be classified without resolving it, so it
/// counts as exposed unless it is `localhost`.
pub(super) fn plaintext_exposure(url: &str) -> Option<String> {
    let rest = url.strip_prefix("ws://")?;
    let authority = rest.split(['/', '?']).next().unwrap_or(rest);
    let host = host_of(authority);
    if host.eq_ignore_ascii_case("localhost") {
        return None;
    }
    match host.parse::<IpAddr>() {
        Ok(ip) if is_private_or_mesh(ip) => None,
        _ => Some(host.to_string()),
    }
}

/// The host part of a URL authority: `host:port`, `host`, or `[v6]:port`.
fn host_of(authority: &str) -> &str {
    if let Some(bracketed) = authority.strip_prefix('[') {
        return bracketed.split(']').next().unwrap_or(bracketed);
    }
    authority
        .rsplit_once(':')
        .map_or(authority, |(host, _port)| host)
}

fn is_private_or_mesh(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let [a, b, ..] = v4.octets();
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                // 100.64.0.0/10, the shared/CGNAT range used by WireGuard meshes.
                || (a == 100 && (b & 0xC0) == 64)
        }
        IpAddr::V6(v6) => v6.is_loopback() || v6.is_unique_local() || v6.is_unicast_link_local(),
    }
}

#[cfg(test)]
#[path = "connect_tests.rs"]
mod tests;
