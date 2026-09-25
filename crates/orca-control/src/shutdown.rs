//! Stopping the master (#178).
//!
//! Only SIGINT (Ctrl-C) was handled. Under systemd, `stop` and `restart` send
//! SIGTERM, so the process died on the spot, and the container teardown
//! that followed a Ctrl-C never ran. Workloads survived restarts only by
//! accident. Now both signals shut the API down gracefully, the teardown is
//! opt-in (`orca server --teardown-on-exit`), and the drain is bounded,
//! because agent WebSocket sessions never close on their own.

use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tracing::{info, warn};

static DRAINED: AtomicBool = AtomicBool::new(false);

/// The API has stopped serving. Called once the listeners have returned, so
/// the drain deadline doesn't cut short what follows (an explicit teardown).
pub fn mark_drained() {
    DRAINED.store(true, Ordering::SeqCst);
}

/// How long open connections get to finish after a shutdown signal.
pub const DRAIN: Duration = Duration::from_secs(10);

/// Resolves on SIGINT or SIGTERM.
pub async fn signal() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => {
                tokio::select! {
                    _ = tokio::signal::ctrl_c() => info!("SIGINT received, shutting down"),
                    _ = term.recv() => info!("SIGTERM received, shutting down"),
                }
                return;
            }
            Err(e) => warn!("cannot listen for SIGTERM ({e}); only Ctrl-C stops orca cleanly"),
        }
    }
    let _ = tokio::signal::ctrl_c().await;
    info!("SIGINT received, shutting down");
}

/// Resolves on SIGINT or SIGTERM, and makes sure the process exits within
/// [`DRAIN`] if connections (agent WebSockets) keep the drain from finishing.
pub async fn signal_with_deadline() {
    signal().await;
    tokio::spawn(async {
        tokio::time::sleep(DRAIN).await;
        if DRAINED.load(Ordering::SeqCst) {
            return;
        }
        warn!(
            "connections still open {}s after shutdown began; exiting",
            DRAIN.as_secs()
        );
        std::process::exit(0);
    });
}

#[cfg(all(test, unix))]
mod tests {
    use std::time::Duration;

    /// systemd stops orca with SIGTERM; that must reach the graceful path.
    #[tokio::test]
    async fn sigterm_triggers_shutdown() {
        let waiting = tokio::spawn(super::signal());
        // Let the handler register before the signal is sent.
        tokio::time::sleep(Duration::from_millis(200)).await;
        let pid = std::process::id().to_string();
        let sent = std::process::Command::new("kill")
            .args(["-TERM", &pid])
            .status()
            .unwrap();
        assert!(sent.success());
        tokio::time::timeout(Duration::from_secs(5), waiting)
            .await
            .expect("SIGTERM must end the wait")
            .unwrap();
    }
}
