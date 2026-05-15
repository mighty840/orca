//! Regression tests asserting the TUI's HTTP layer never hangs.
//!
//! The TUI event loop is single-threaded — every `.await` it makes runs
//! to completion before the next keystroke is read. So any HTTP call in
//! the loop's path that can hang for an unbounded time hangs the entire
//! UI: no Ctrl+C, no navigation, no rendering. That's been the underlying
//! cause of every "TUI stuck" report this cycle.
//!
//! These tests pin the contract:
//!
//!   1. Connection failures resolve fast (ECONNREFUSED is immediate; the
//!      handler must not paper over it with retries or fall back to a
//!      blocking call without a timeout).
//!   2. A server that accepts the TCP connection but never replies must
//!      cause the client to give up at its configured request timeout —
//!      not hang waiting for bytes forever.
//!
//! Run normally for the connection-refused tests (fast). Run with
//! `--ignored` for the slow blackhole test which actually waits ~10s for
//! the request timeout to fire.

use std::time::{Duration, Instant};

use orca_tui::api::ApiClient;
use tokio::net::TcpListener;

/// Maximum wall-time we accept for a "fail-fast" path. Plenty of slack
/// over the actual sub-second ECONNREFUSED so this isn't flaky on a
/// loaded CI runner, but tight enough to catch a missing timeout that
/// would let the call hang for the full 10s.
const FAIL_FAST_BUDGET: Duration = Duration::from_secs(2);

/// Build a client pointed at a port nothing listens on. Connect attempts
/// return ECONNREFUSED immediately on Linux/BSD.
fn client_to_dead_port() -> ApiClient {
    ApiClient::new("http://127.0.0.1:1")
}

#[tokio::test]
async fn status_fails_fast_on_connection_refused() {
    // The global 2s refresh in `event_loop` calls `status()` first. If
    // this hangs, every other key handler is starved. Most critical
    // budget in the TUI.
    let client = client_to_dead_port();
    let start = Instant::now();
    let result = client.status().await;
    let elapsed = start.elapsed();
    assert!(result.is_err(), "expected transport error, got {result:?}");
    assert!(
        elapsed < FAIL_FAST_BUDGET,
        "status() must fail fast on ECONNREFUSED, took {elapsed:?}"
    );
}

#[tokio::test]
async fn cluster_info_fails_fast_on_connection_refused() {
    // Second call in the global refresh. Same constraint as status().
    let client = client_to_dead_port();
    let start = Instant::now();
    let result = client.cluster_info().await;
    let elapsed = start.elapsed();
    assert!(result.is_err());
    assert!(
        elapsed < FAIL_FAST_BUDGET,
        "cluster_info() must fail fast, took {elapsed:?}"
    );
}

#[tokio::test]
async fn alerts_list_fails_fast_on_connection_refused() {
    // Driven by pressing `7` and by the `r` key in the Alerts view.
    let client = client_to_dead_port();
    let start = Instant::now();
    let result = client.alerts_list(false).await;
    let elapsed = start.elapsed();
    assert!(result.is_err());
    assert!(
        elapsed < FAIL_FAST_BUDGET,
        "alerts_list() must fail fast, took {elapsed:?}"
    );
}

#[tokio::test]
async fn cluster_networks_fails_fast_on_connection_refused() {
    // Driven by pressing `6` and by the `r` key in the Networks view.
    let client = client_to_dead_port();
    let start = Instant::now();
    let result = client.cluster_networks().await;
    let elapsed = start.elapsed();
    assert!(result.is_err());
    assert!(
        elapsed < FAIL_FAST_BUDGET,
        "cluster_networks() must fail fast, took {elapsed:?}"
    );
}

#[tokio::test]
async fn cluster_backups_fails_fast_on_connection_refused() {
    let client = client_to_dead_port();
    let start = Instant::now();
    let result = client.cluster_backups().await;
    let elapsed = start.elapsed();
    assert!(result.is_err());
    assert!(
        elapsed < FAIL_FAST_BUDGET,
        "cluster_backups() must fail fast, took {elapsed:?}"
    );
}

#[tokio::test]
async fn secrets_usage_fails_fast_on_connection_refused() {
    let client = client_to_dead_port();
    let start = Instant::now();
    let result = client.secrets_usage().await;
    let elapsed = start.elapsed();
    assert!(result.is_err());
    assert!(
        elapsed < FAIL_FAST_BUDGET,
        "secrets_usage() must fail fast, took {elapsed:?}"
    );
}

#[tokio::test]
async fn list_webhooks_fails_fast_on_connection_refused() {
    let client = client_to_dead_port();
    let start = Instant::now();
    let result = client.list_webhooks().await;
    let elapsed = start.elapsed();
    assert!(result.is_err());
    assert!(
        elapsed < FAIL_FAST_BUDGET,
        "list_webhooks() must fail fast, took {elapsed:?}"
    );
}

#[tokio::test]
async fn ask_fails_fast_on_connection_refused() {
    let client = client_to_dead_port();
    let start = Instant::now();
    let result = client.ask("hi", &[]).await;
    let elapsed = start.elapsed();
    assert!(result.is_err());
    assert!(
        elapsed < FAIL_FAST_BUDGET,
        "ask() must fail fast, took {elapsed:?}"
    );
}

/// Black-hole server: accepts TCP connections but never writes a byte.
/// The client sends its HTTP request and waits for a response that never
/// arrives — must give up at the configured request timeout (10s).
async fn spawn_blackhole_server() -> std::net::SocketAddr {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((stream, _)) = listener.accept().await else {
                continue;
            };
            // Hold the socket; never write. Drop after 2 min so we don't
            // leak FDs if the test suite somehow keeps the server alive
            // past its useful life.
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(120)).await;
                drop(stream);
            });
        }
    });
    addr
}

/// Long-running by design: waits ~10s for the request timeout to fire.
/// Marked `#[ignore]` so the normal `cargo test` cycle stays fast; run
/// via `cargo test --test never_hang_test -- --ignored` (and the nightly
/// E2E suite picks it up).
#[tokio::test]
#[ignore]
async fn status_respects_request_timeout_against_blackhole() {
    let addr = spawn_blackhole_server().await;
    let client = ApiClient::new(&format!("http://{addr}"));
    let start = Instant::now();
    let result = client.status().await;
    let elapsed = start.elapsed();
    assert!(result.is_err(), "expected timeout error, got {result:?}");
    // The configured request timeout is 10s; allow some slack but cap
    // the test at 15s so a regression that disables the timeout shows
    // up as a clear "took 60s, must timeout" failure instead of an
    // indefinite hang.
    assert!(
        elapsed < Duration::from_secs(15),
        "client must give up at the request timeout, took {elapsed:?}"
    );
    assert!(
        elapsed >= Duration::from_secs(8),
        "client should be hitting its own 10s timeout, not returning early on something else — took {elapsed:?}"
    );
}
