//! The views, input modes and connection state the TUI switches between.

/// Full-screen views (k9s style — each replaces the entire screen).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum View {
    /// Default landing page — `orca ask`-style chat with the cluster AI.
    /// Conversation lives only for the session; quitting wipes it.
    Chat,
    Services,
    Nodes,
    Logs {
        service: String,
    },
    Detail {
        service: String,
    },
    Help,
    Secrets,
    Backups,
    /// Drill-down: snapshot list for the node at `node_idx` in
    /// `AppState::backups.nodes`. The index is captured at push-time
    /// rather than a node identifier so master (which has no `node_id`)
    /// can be the target too.
    BackupSnapshots {
        node_idx: usize,
    },
    Webhooks,
    /// Drill-down: invocation history for one webhook, keyed by service name
    /// (same identifier the API uses).
    WebhookInvocations {
        service: String,
    },
    /// Drill-down: list of services that reference one secret key.
    SecretRefs {
        key: String,
    },
    Networks,
    /// AI alert conversations (`/api/v1/alerts`). Acts as a list view; press
    /// Enter on a row to drill down to [`View::AlertDetail`].
    Alerts,
    /// Cluster-token rotation: start, per-agent progress, finish (#265).
    Token,
    /// Drill-down: the full transcript for one alert conversation.
    AlertDetail {
        id: String,
    },
}

/// Input mode for the TUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    Command,
    Filter,
}

/// Connection status based on API responses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConnectionStatus {
    Connected,
    Disconnected,
}
