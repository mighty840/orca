//! TUI application state — k9s-style view stack navigation.

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use crate::api::{
    ClusterBackupsResponse, ClusterInfo, ClusterNetworksResponse, NodeInfo, SecretUsage,
    ServiceStatus, StatusResponse, WebhookEntry, WebhookInvocation,
};
pub use crate::metrics::{MetricHistory, parse_human_bytes};

/// Full-screen views (k9s style — each replaces the entire screen).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum View {
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

/// Full application state for the TUI.
pub struct AppState {
    pub view: View,
    pub view_stack: Vec<View>,
    pub cluster_name: String,
    pub services: Vec<ServiceStatus>,
    pub nodes: Vec<NodeInfo>,
    pub node_count: u64,
    pub selected_service: usize,
    pub logs: String,
    pub error: Option<String>,
    pub should_quit: bool,
    pub filter: String,
    pub input_mode: InputMode,
    pub command_input: String,
    pub status_msg: Option<String>,
    pub status_msg_time: Option<Instant>,
    pub start_time: Instant,
    pub word_wrap: bool,
    pub connection: ConnectionStatus,
    pub service_scroll: usize,
    pub api_url: String,
    pub tick: u64,
    pub auto_refresh_logs: bool,
    /// Project filter (separate from text filter).
    pub project_filter: Option<String>,
    /// Project loaded from `~/.orca/tui-state.json` on launch, kept here until
    /// the first successful status refresh confirms (or denies) that the
    /// project still exists in the cluster.
    pub pending_restore_project: Option<String>,
    /// Set by `:sh` / `:exec` to signal the event loop to suspend the TUI
    /// and run an interactive command inside a container. Contents:
    /// (service name, optional node hostname, command argv).
    pub pending_shell: Option<(String, Option<String>, Vec<String>)>,
    /// Per-service rolling metric history (~3 minutes).
    pub history: HashMap<String, MetricHistory>,
    /// Per-node rolling metric history.
    pub node_history: HashMap<u64, MetricHistory>,
    /// Project rows the user has collapsed in the services view.
    pub collapsed_projects: HashSet<String>,
    /// Cluster version + commit hash from `/api/v1/cluster/info`.
    pub cluster_version: Option<String>,
    pub cluster_commit: Option<String>,
    /// Currently selected row in the secrets view. Indexes into the flat list
    /// of `ui::secrets::flatten(&secrets_usage)` (group headers + key rows),
    /// so use `ui::secrets::selectable_indices` when navigating to skip
    /// non-selectable header rows.
    pub selected_secret: usize,
    /// Cached cluster-backups response; refreshed when entering the view or
    /// on explicit `r` keypress. `None` means we haven't fetched yet this
    /// session — the view shows a "loading" placeholder.
    pub backups: Option<ClusterBackupsResponse>,
    /// Selected row in the backups view.
    pub selected_backup_node: usize,
    /// Selected snapshot row in the backup-snapshots drill-down.
    pub selected_backup_snapshot: usize,
    /// Cached webhook list; refreshed when entering the view or on `r`.
    pub webhooks: Vec<WebhookEntry>,
    /// Selected row in the webhooks view.
    pub selected_webhook: usize,
    /// Cached invocation history for the currently drilled-down webhook.
    pub webhook_invocations: Vec<WebhookInvocation>,
    /// Secrets organizer data (`GET /api/v1/secrets/usage`). Refreshed on
    /// entry and on `r`; never auto-polled.
    pub secrets_usage: Vec<SecretUsage>,
    /// Cluster networks data (`GET /api/v1/cluster/networks`). Refreshed on
    /// entry and on `r`. `None` means we haven't fetched yet this session.
    pub networks: Option<ClusterNetworksResponse>,
    /// Scroll offset (in rendered lines) for the Networks view. The view has
    /// no selection cursor — j/k just shift the viewport.
    pub network_scroll: usize,
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

impl AppState {
    pub fn new() -> Self {
        Self {
            view: View::Services,
            view_stack: Vec::new(),
            cluster_name: "connecting...".into(),
            services: Vec::new(),
            nodes: Vec::new(),
            node_count: 0,
            selected_service: 0,
            logs: String::new(),
            error: None,
            should_quit: false,
            filter: String::new(),
            input_mode: InputMode::Normal,
            command_input: String::new(),
            status_msg: None,
            status_msg_time: None,
            start_time: Instant::now(),
            word_wrap: false,
            connection: ConnectionStatus::Disconnected,
            service_scroll: 0,
            api_url: String::new(),
            tick: 0,
            auto_refresh_logs: true,
            project_filter: None,
            pending_restore_project: None,
            pending_shell: None,
            history: HashMap::new(),
            node_history: HashMap::new(),
            collapsed_projects: HashSet::new(),
            cluster_version: None,
            cluster_commit: None,
            selected_secret: 0,
            backups: None,
            selected_backup_node: 0,
            selected_backup_snapshot: 0,
            webhooks: Vec::new(),
            selected_webhook: 0,
            webhook_invocations: Vec::new(),
            secrets_usage: Vec::new(),
            networks: None,
            network_scroll: 0,
        }
    }

    /// Pull a fresh CPU% / mem sample from each service into its rolling
    /// history buffer. Called on every successful status refresh.
    pub fn record_service_samples(&mut self) {
        for svc in &self.services {
            let cpu = svc.cpu_percent.unwrap_or(0.0);
            let mem = svc
                .memory_usage
                .as_deref()
                .map(parse_human_bytes)
                .unwrap_or(0);
            self.history
                .entry(svc.name.clone())
                .or_default()
                .push_basic(cpu, mem);
        }
    }

    /// Append the latest node sample to the rolling buffer. Network
    /// counters are stored raw; the nodes UI diffs consecutive entries
    /// to get per-interval throughput.
    pub fn record_node_samples(&mut self) {
        for n in &self.nodes {
            self.node_history.entry(n.node_id).or_default().push_full(
                n.cpu_percent,
                n.memory_bytes,
                n.disk_used,
                n.net_rx,
                n.net_tx,
            );
        }
    }

    pub fn toggle_collapse_project(&mut self, project: &str) {
        if !self.collapsed_projects.remove(project) {
            self.collapsed_projects.insert(project.to_string());
        }
    }

    pub fn push_view(&mut self, new_view: View) {
        let old = std::mem::replace(&mut self.view, new_view);
        self.view_stack.push(old);
    }

    pub fn pop_view(&mut self) -> bool {
        if let Some(prev) = self.view_stack.pop() {
            self.view = prev;
            true
        } else {
            false
        }
    }

    pub fn update_status(&mut self, resp: StatusResponse) {
        self.cluster_name = resp.cluster_name;
        self.services = resp.services;
        self.connection = ConnectionStatus::Connected;
        self.record_service_samples();
        let visible_len = self.visible_services().len();
        if self.selected_service >= visible_len && visible_len > 0 {
            self.selected_service = visible_len - 1;
        }
    }

    pub fn mark_disconnected(&mut self) {
        self.connection = ConnectionStatus::Disconnected;
    }

    pub fn update_cluster(&mut self, info: ClusterInfo) {
        self.nodes = info.nodes;
        self.node_count = info.node_count;
        self.cluster_version = info.version;
        self.cluster_commit = info.commit;
        self.record_node_samples();
    }

    pub fn flash(&mut self, msg: String) {
        self.status_msg = Some(msg);
        self.status_msg_time = Some(Instant::now());
    }

    pub fn maybe_clear_flash(&mut self) {
        if let Some(t) = self.status_msg_time
            && t.elapsed().as_secs() >= 3
        {
            self.status_msg = None;
            self.status_msg_time = None;
        }
    }

    /// Get services filtered by both text filter and project filter.
    pub fn filtered_services(&self) -> Vec<&ServiceStatus> {
        let f = self.filter.to_lowercase();
        self.services
            .iter()
            .filter(|s| {
                if !self.filter.is_empty() && !s.name.to_lowercase().contains(&f) {
                    return false;
                }
                if let Some(ref proj) = self.project_filter {
                    return s.project.as_deref() == Some(proj.as_str());
                }
                true
            })
            .collect()
    }

    /// Services in the order they actually appear on screen: grouped by
    /// project alphabetically, services within a group in their original
    /// order, and services under collapsed projects hidden. This is what
    /// `selected_service` indexes into — using `filtered_services()` order
    /// instead made Enter open the wrong row whenever projects didn't
    /// match the input service list order.
    pub fn visible_services(&self) -> Vec<&ServiceStatus> {
        use std::collections::BTreeMap;
        let mut grouped: BTreeMap<&str, Vec<&ServiceStatus>> = BTreeMap::new();
        for svc in self.filtered_services() {
            let key = svc.project.as_deref().unwrap_or("(no project)");
            grouped.entry(key).or_default().push(svc);
        }
        let mut out: Vec<&ServiceStatus> = Vec::new();
        for (project, svcs) in grouped {
            if self.collapsed_projects.contains(project) {
                continue;
            }
            out.extend(svcs);
        }
        out
    }

    pub fn selected_service_name(&self) -> Option<&str> {
        let visible = self.visible_services();
        visible.get(self.selected_service).map(|s| s.name.as_str())
    }

    pub fn selected_service_data(&self) -> Option<&ServiceStatus> {
        let visible = self.visible_services();
        visible.get(self.selected_service).copied()
    }

    pub fn prev_service(&mut self) {
        if self.selected_service > 0 {
            self.selected_service -= 1;
        }
    }

    pub fn next_service(&mut self) {
        let len = self.visible_services().len();
        if len > 0 && self.selected_service < len - 1 {
            self.selected_service += 1;
        }
    }

    pub fn uptime_str(&self) -> String {
        let secs = self.start_time.elapsed().as_secs();
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        let s = secs % 60;
        format!("{h:02}:{m:02}:{s:02}")
    }

    pub fn status_counts(&self) -> (usize, usize, usize) {
        let running = self
            .services
            .iter()
            .filter(|s| s.status == "running")
            .count();
        let stopped = self
            .services
            .iter()
            .filter(|s| s.status == "stopped" || s.status == "failed")
            .count();
        let other = self.services.len() - running - stopped;
        (running, stopped, other)
    }

    /// View name for display in status bar.
    pub fn view_name(&self) -> &str {
        match &self.view {
            View::Services => "Services",
            View::Nodes => "Nodes",
            View::Logs { .. } => "Logs",
            View::Detail { .. } => "Detail",
            View::Help => "Help",
            View::Secrets => "Secrets",
            View::Backups => "Backups",
            View::BackupSnapshots { .. } => "Snapshots",
            View::Webhooks => "Webhooks",
            View::WebhookInvocations { .. } => "Invocations",
            View::SecretRefs { .. } => "Secret Refs",
            View::Networks => "Networks",
        }
    }
}
