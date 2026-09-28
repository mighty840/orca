use std::time::{Duration, Instant};

use super::*;
use crate::api::ApiClient;

fn status() -> anyhow::Result<StatusResponse> {
    Ok(StatusResponse {
        cluster_name: "c".into(),
        services: vec![],
    })
}

fn cluster() -> anyhow::Result<ClusterInfo> {
    Ok(ClusterInfo {
        cluster_name: "c".into(),
        node_count: 0,
        nodes: vec![],
        version: None,
        commit: None,
    })
}

fn poll(status: anyhow::Result<StatusResponse>, cluster: anyhow::Result<ClusterInfo>) -> Fetched {
    Fetched::Poll {
        status,
        cluster,
        alerts: None,
    }
}

/// A server that accepts connections and never answers.
async fn silent_server() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let mut held = Vec::new();
        while let Ok((sock, _)) = listener.accept().await {
            held.push(sock);
        }
    });
    format!("http://{addr}")
}

#[tokio::test]
async fn a_hung_master_does_not_block_the_loop() {
    // Before #263 the loop awaited status then cluster/info, 10 s each.
    let client = ApiClient::new(&silent_server().await);
    let mut state = AppState::new();
    let started = std::time::Instant::now();
    spawn_poll(&client, &mut state);
    spawn_logs(&client, &mut state, "api");
    spawn_backups(&client, &mut state);
    assert!(started.elapsed() < Duration::from_millis(100));
    // One of each kind at a time: a second poll while one hangs is a no-op.
    assert!(state.bg.start(Kind::Poll).is_none());
    assert!(state.bg.start(Kind::Logs("api".into())).is_none());
    assert!(state.bg.start(Kind::Logs("db".into())).is_some());
}

#[test]
fn a_successful_poll_clears_a_connection_error_only() {
    let mut state = AppState::new();
    apply(&mut state, poll(Err(anyhow::anyhow!("refused")), cluster()));
    assert_eq!(state.error.as_deref(), Some("API error: refused"));
    apply(&mut state, poll(status(), cluster()));
    assert!(state.error.is_none());

    state.error = Some("Stop failed: 500".into());
    apply(&mut state, poll(status(), cluster()));
    assert_eq!(
        state.error.as_deref(),
        Some("Stop failed: 500"),
        "not wiped by a poll"
    );
}

#[test]
fn a_cluster_info_failure_is_shown() {
    let mut state = AppState::new();
    apply(&mut state, poll(status(), Err(anyhow::anyhow!("timeout"))));
    assert_eq!(state.error.as_deref(), Some("Cluster info failed: timeout"));
}

#[test]
fn polled_alerts_are_applied() {
    let mut state = AppState::new();
    let fetched = Fetched::Poll {
        status: status(),
        cluster: cluster(),
        alerts: Some(Ok(None)),
    };
    apply(&mut state, fetched);
    assert!(state.alerts_unavailable);
}

#[test]
fn a_log_tail_for_a_service_no_longer_shown_is_dropped() {
    let mut state = AppState::new();
    state.view = View::Logs {
        service: "db".into(),
    };
    state.logs = "db logs".into();
    apply(
        &mut state,
        Fetched::Logs {
            service: "api".into(),
            result: Ok("api logs".into()),
        },
    );
    assert_eq!(state.logs, "db logs");
    apply(
        &mut state,
        Fetched::Logs {
            service: "db".into(),
            result: Ok("new db logs".into()),
        },
    );
    assert_eq!(state.logs, "new db logs");
}

#[test]
fn an_error_stays_visible_then_expires() {
    let mut state = AppState::new();
    let t0 = Instant::now();
    state.error = Some("Stop failed: 500".into());
    expire_error(&mut state, t0);
    expire_error(&mut state, t0 + Duration::from_secs(9));
    assert!(state.error.is_some(), "still readable after 9 s");
    expire_error(&mut state, t0 + ERROR_VISIBLE);
    assert!(state.error.is_none());
}

#[test]
fn a_new_error_restarts_the_clock() {
    let mut state = AppState::new();
    let t0 = Instant::now();
    state.error = Some("first".into());
    expire_error(&mut state, t0);
    state.error = Some("second".into());
    expire_error(&mut state, t0 + Duration::from_secs(8));
    expire_error(&mut state, t0 + Duration::from_secs(12));
    assert_eq!(state.error.as_deref(), Some("second"));
}
