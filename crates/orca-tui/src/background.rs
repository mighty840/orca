//! API calls that run off the event loop (#263).
//!
//! The 2 s refresh, log tails and the backups fan-out used to be awaited
//! inside the loop, so a slow master froze input and redraws for up to
//! ~20 s. They now run as tokio tasks; results come back over a channel and
//! [`drain`] applies them on the next tick. At most one task of each kind is
//! in flight, so a slow master can't pile up requests.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

use crate::api::{
    AlertConversation, ApiClient, ClusterBackupsResponse, ClusterInfo, StatusResponse,
};
use crate::state::{AppState, View};

/// Tail length for log views.
pub(crate) const LOG_TAIL: u64 = 200;

/// An error stays in the footer this long, then clears. A failed poll
/// replaces it, and a successful one clears a connection error at once.
pub const ERROR_VISIBLE: Duration = Duration::from_secs(10);

/// Prefix of the error a failed status poll leaves.
const POLL_ERROR: &str = "API error: ";

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Kind {
    Poll,
    /// Per service: a slow tail of one service must not hold up another's.
    Logs(String),
    Backups,
}

/// A finished background fetch.
pub(crate) enum Fetched {
    Poll {
        status: anyhow::Result<StatusResponse>,
        cluster: anyhow::Result<ClusterInfo>,
        /// `None` when alerts weren't asked for (no alert view open).
        alerts: Option<anyhow::Result<Option<Vec<AlertConversation>>>>,
        /// `None` unless the Token view is open.
        rotation: Option<anyhow::Result<crate::api::RotationStatus>>,
    },
    Logs {
        service: String,
        result: anyhow::Result<String>,
    },
    Backups(anyhow::Result<ClusterBackupsResponse>),
    /// New text from a followed log (`crate::log_follow`).
    LogChunk {
        service: String,
        text: String,
    },
    /// A followed log ended; `error` when it never started.
    FollowEnded {
        service: String,
        error: Option<String>,
    },
}

/// Channel and bookkeeping for background fetches; lives on `AppState`.
pub struct Background {
    pub(crate) tx: UnboundedSender<Fetched>,
    rx: UnboundedReceiver<Fetched>,
    in_flight: HashSet<Kind>,
    /// The error on screen and when it appeared, to expire it.
    shown_error: Option<(String, Instant)>,
    pub(crate) follow: crate::log_follow::FollowState,
}

impl Default for Background {
    fn default() -> Self {
        let (tx, rx) = unbounded_channel();
        Self {
            tx,
            rx,
            in_flight: HashSet::new(),
            shown_error: None,
            follow: Default::default(),
        }
    }
}

impl Background {
    /// Start `kind` unless one is already running.
    fn start(&mut self, kind: Kind) -> Option<UnboundedSender<Fetched>> {
        self.in_flight.insert(kind).then(|| self.tx.clone())
    }
}

/// Refresh status and cluster info together, plus alerts when an alert view
/// is open (they weren't polled at all before, so a `:reply` answer never
/// showed up without `r`).
pub(crate) fn spawn_poll(client: &ApiClient, state: &mut AppState) {
    let alerts_all = matches!(state.view, View::Alerts | View::AlertDetail { .. })
        .then_some(state.alerts_show_all);
    let rotation = matches!(state.view, View::Token);
    let Some(tx) = state.bg.start(Kind::Poll) else {
        return;
    };
    let client = client.clone();
    tokio::spawn(async move {
        let alerts = async {
            match alerts_all {
                Some(all) => Some(client.alerts_list(all).await),
                None => None,
            }
        };
        let rotation = async {
            match rotation {
                true => Some(client.token_rotation().await),
                false => None,
            }
        };
        let (status, cluster, alerts, rotation) =
            tokio::join!(client.status(), client.cluster_info(), alerts, rotation);
        let _ = tx.send(Fetched::Poll {
            status,
            cluster,
            alerts,
            rotation,
        });
    });
}

/// Fetch the log tail of `service` for the Logs and Detail views.
pub(crate) fn spawn_logs(client: &ApiClient, state: &mut AppState, service: &str) {
    let Some(tx) = state.bg.start(Kind::Logs(service.to_string())) else {
        return;
    };
    let client = client.clone();
    let service = service.to_string();
    tokio::spawn(async move {
        let result = client.logs(&service, LOG_TAIL).await;
        let _ = tx.send(Fetched::Logs { service, result });
    });
}

/// Fetch the cluster-wide backup status (a fan-out to every agent).
pub(crate) fn spawn_backups(client: &ApiClient, state: &mut AppState) {
    let Some(tx) = state.bg.start(Kind::Backups) else {
        return;
    };
    let client = client.clone();
    tokio::spawn(async move {
        let _ = tx.send(Fetched::Backups(client.cluster_backups().await));
    });
}

/// Apply every finished fetch. Called once per event-loop tick.
pub(crate) fn drain(state: &mut AppState) {
    while let Ok(fetched) = state.bg.rx.try_recv() {
        apply(state, fetched);
    }
}

pub(crate) fn apply(state: &mut AppState, fetched: Fetched) {
    match fetched {
        Fetched::Poll {
            status,
            cluster,
            alerts,
            rotation,
        } => {
            if let Some(result) = rotation {
                crate::token_actions::apply(state, result, None);
            }
            state.bg.in_flight.remove(&Kind::Poll);
            match status {
                Ok(resp) => {
                    if state
                        .error
                        .as_deref()
                        .is_some_and(|e| e.starts_with(POLL_ERROR))
                    {
                        state.error = None;
                    }
                    state.update_status(resp);
                    crate::try_restore_project_filter(state);
                }
                Err(e) => {
                    state.mark_disconnected();
                    state.error = Some(format!("{POLL_ERROR}{e}"));
                }
            }
            match cluster {
                Ok(info) => state.update_cluster(info),
                Err(e) => state.error = Some(format!("Cluster info failed: {e}")),
            }
            if let Some(result) = alerts {
                crate::apply_alerts(state, result);
            }
        }
        Fetched::Logs { service, result } => {
            state.bg.in_flight.remove(&Kind::Logs(service.clone()));
            // Drop a tail for a service the operator has since left.
            // Also drop a polled tail while the log is followed live: it
            // would replace what the stream appended.
            let showing = match &state.view {
                View::Logs { service: s } => {
                    s == &service && !crate::log_follow::is_followed(state, s)
                }
                View::Detail { service: s } => s == &service,
                _ => false,
            };
            if showing {
                state.logs = match result {
                    Ok(logs) => logs,
                    Err(e) => format!("Failed to fetch logs: {e}"),
                };
            }
        }
        Fetched::Backups(result) => {
            state.bg.in_flight.remove(&Kind::Backups);
            crate::apply_backups(state, result);
        }
        Fetched::LogChunk { service, text } => {
            crate::log_follow::apply_chunk(state, &service, &text)
        }
        Fetched::FollowEnded { service, error } => {
            crate::log_follow::apply_ended(state, &service, error)
        }
    }
}

/// Clear an error that has been on screen for [`ERROR_VISIBLE`]. Before,
/// every 2 s refresh wiped it, so "Stop failed: ..." was gone before it
/// could be read.
pub(crate) fn expire_error(state: &mut AppState, now: Instant) {
    let Some(err) = state.error.clone() else {
        state.bg.shown_error = None;
        return;
    };
    match &state.bg.shown_error {
        Some((shown, since)) if *shown == err => {
            if now.duration_since(*since) >= ERROR_VISIBLE {
                state.error = None;
                state.bg.shown_error = None;
            }
        }
        _ => state.bg.shown_error = Some((err, now)),
    }
}

#[cfg(test)]
#[path = "background_tests.rs"]
mod tests;
