use std::time::Duration;

use axum::routing::get;

use super::*;

fn ips(list: &[&str]) -> Vec<IpAddr> {
    list.iter().map(|s| s.parse().unwrap()).collect()
}

// --- listen_addrs -----------------------------------------------------------

#[test]
fn default_wildcard_binds_every_ipv4_interface_as_before() {
    assert_eq!(
        listen_addrs(&ips(&["0.0.0.0"]), 6880).unwrap(),
        vec!["0.0.0.0:6880".parse::<SocketAddr>().unwrap()]
    );
}

#[test]
fn loopback_plus_mesh_binds_both_on_the_api_port() {
    // The intended breakpilot setup: local CLI plus the NetBird address.
    let got = listen_addrs(&ips(&["127.0.0.1", "100.80.5.14"]), 6880).unwrap();
    let got: Vec<String> = got.iter().map(ToString::to_string).collect();
    assert_eq!(got, ["127.0.0.1:6880", "100.80.5.14:6880"]);
}

#[test]
fn an_empty_list_is_rejected() {
    let err = listen_addrs(&[], 6880).unwrap_err();
    assert!(err.contains("api_bind"), "{err}");
}

#[test]
fn a_duplicate_is_rejected() {
    let err = listen_addrs(&ips(&["127.0.0.1", "127.0.0.1"]), 6880).unwrap_err();
    assert!(err.contains("127.0.0.1"), "{err}");
}

#[test]
fn a_wildcard_combined_with_other_addresses_is_rejected() {
    for list in [&["0.0.0.0", "127.0.0.1"][..], &["::", "100.80.5.14"][..]] {
        let err = listen_addrs(&ips(list), 6880).unwrap_err();
        assert!(err.contains("wildcard"), "{list:?}: {err}");
    }
}

#[test]
fn a_wildcard_on_its_own_is_fine() {
    assert!(listen_addrs(&ips(&["::"]), 6880).is_ok());
}

// --- reachable_from_local_cli -----------------------------------------------

#[test]
fn the_local_cli_reaches_wildcards_and_ipv4_loopback() {
    for list in [
        &["0.0.0.0"][..],
        &["127.0.0.1", "100.80.5.14"][..],
        &["127.0.0.2"][..], // all of 127.0.0.0/8 is loopback
        &["::"][..],        // dual-stack by default on Linux
    ] {
        assert!(reachable_from_local_cli(&ips(list)), "{list:?}");
    }
}

#[test]
fn the_local_cli_cannot_reach_a_mesh_only_or_v6_loopback_bind() {
    for list in [&["100.80.5.14"][..], &["::1"][..], &["10.0.0.1"][..]] {
        assert!(!reachable_from_local_cli(&ips(list)), "{list:?}");
    }
}

// --- serve_all --------------------------------------------------------------

fn ping_app() -> Router {
    Router::new().route("/ping", get(|| async { "pong" }))
}

async fn loopback_listener() -> (TcpListener, SocketAddr) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    (listener, addr)
}

#[tokio::test]
async fn serves_on_every_listener_and_stops_them_all_on_one_signal() {
    let (a, addr_a) = loopback_listener().await;
    let (b, addr_b) = loopback_listener().await;
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let server = tokio::spawn(serve_all(ping_app(), vec![a, b], async move {
        let _ = stopped.await;
    }));

    for addr in [addr_a, addr_b] {
        let body = reqwest::get(format!("http://{addr}/ping"))
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert_eq!(body, "pong", "{addr}");
    }

    stop.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), server)
        .await
        .expect("every server stops on the one signal")
        .unwrap()
        .unwrap();
}

#[tokio::test]
async fn a_signal_that_fires_before_the_servers_start_waiting_still_stops_them() {
    // The shutdown future is already complete, so the signal is sent before
    // the servers subscribe. A Notify would drop it and hang forever here; a
    // watch channel keeps the value.
    let (a, _) = loopback_listener().await;
    let (b, _) = loopback_listener().await;
    tokio::time::timeout(
        Duration::from_secs(5),
        serve_all(ping_app(), vec![a, b], std::future::ready(())),
    )
    .await
    .expect("servers must see a signal that fired before they subscribed")
    .unwrap();
}
