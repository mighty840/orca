//! TUI application state — k9s-style view stack navigation.

use std::time::Instant;

use crate::api::{ClusterInfo, NodeInfo, ServiceStatus, StatusResponse};

/// Full-screen views (k9s style — each replaces the entire screen).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum View {
    Services,
    Nodes,
    Logs { service: String },
    Detail { service: String },
    Help,
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
    /// Current view (top of the view stack).
    pub view: View,
    /// View stack for Esc navigation (does NOT include current view).
    pub view_stack: Vec<View>,
    /// Cluster name.
    pub cluster_name: String,
    /// Services and their status.
    pub services: Vec<ServiceStatus>,
    /// Registered nodes.
    pub nodes: Vec<NodeInfo>,
    /// Node count.
    pub node_count: u64,
    /// Currently selected service index.
    pub selected_service: usize,
    /// Log output for the selected service.
    pub logs: String,
    /// Last error message.
    pub error: Option<String>,
    /// Whether the app should quit.
    pub should_quit: bool,
    /// Filter string for services.
    pub filter: String,
    /// Current input mode.
    pub input_mode: InputMode,
    /// Command input buffer for `:` mode.
    pub command_input: String,
    /// Status message (e.g. "Deployed nginx").
    pub status_msg: Option<String>,
    /// When the status message was set (for auto-clear).
    pub status_msg_time: Option<Instant>,
    /// App start time for uptime display.
    pub start_time: Instant,
    /// Word wrap toggle for logs view.
    pub word_wrap: bool,
    /// Connection status based on API responses.
    pub connection: ConnectionStatus,
    /// Scroll offset for services list.
    pub service_scroll: usize,
    /// API base URL for display.
    pub api_url: String,
    /// Tick counter for blinking indicator.
    pub tick: u64,
    /// Whether logs should auto-refresh.
    pub auto_refresh_logs: bool,
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
        }
    }

    /// Push current view onto the stack and switch to a new view.
    pub fn push_view(&mut self, new_view: View) {
        let old = std::mem::replace(&mut self.view, new_view);
        self.view_stack.push(old);
    }

    /// Pop back to the previous view. Returns false if stack is empty.
    pub fn pop_view(&mut self) -> bool {
        if let Some(prev) = self.view_stack.pop() {
            self.view = prev;
            true
        } else {
            false
        }
    }

    /// Update from API status response.
    pub fn update_status(&mut self, resp: StatusResponse) {
        self.cluster_name = resp.cluster_name;
        self.services = resp.services;
        self.connection = ConnectionStatus::Connected;
        let filtered_len = self.filtered_services().len();
        if self.selected_service >= filtered_len && filtered_len > 0 {
            self.selected_service = filtered_len - 1;
        }
    }

    /// Mark connection as failed.
    pub fn mark_disconnected(&mut self) {
        self.connection = ConnectionStatus::Disconnected;
    }

    /// Update from cluster info response.
    pub fn update_cluster(&mut self, info: ClusterInfo) {
        self.nodes = info.nodes;
        self.node_count = info.node_count;
    }

    /// Set a flash status message with auto-clear timer.
    pub fn flash(&mut self, msg: String) {
        self.status_msg = Some(msg);
        self.status_msg_time = Some(Instant::now());
    }

    /// Clear status message if it has been visible long enough (3s).
    pub fn maybe_clear_flash(&mut self) {
        if let Some(t) = self.status_msg_time
            && t.elapsed().as_secs() >= 3
        {
            self.status_msg = None;
            self.status_msg_time = None;
        }
    }

    /// Get services filtered by the current filter string.
    pub fn filtered_services(&self) -> Vec<&ServiceStatus> {
        if self.filter.is_empty() {
            self.services.iter().collect()
        } else {
            let f = self.filter.to_lowercase();
            self.services
                .iter()
                .filter(|s| s.name.to_lowercase().contains(&f))
                .collect()
        }
    }

    /// Get the name of the currently selected service.
    pub fn selected_service_name(&self) -> Option<&str> {
        let filtered = self.filtered_services();
        filtered.get(self.selected_service).map(|s| s.name.as_str())
    }

    /// Get the currently selected service.
    pub fn selected_service_data(&self) -> Option<&ServiceStatus> {
        let filtered = self.filtered_services();
        filtered.get(self.selected_service).copied()
    }

    /// Move selection up.
    pub fn prev_service(&mut self) {
        if self.selected_service > 0 {
            self.selected_service -= 1;
        }
    }

    /// Move selection down.
    pub fn next_service(&mut self) {
        let len = self.filtered_services().len();
        if len > 0 && self.selected_service < len - 1 {
            self.selected_service += 1;
        }
    }

    /// Format uptime as HH:MM:SS.
    pub fn uptime_str(&self) -> String {
        let secs = self.start_time.elapsed().as_secs();
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        let s = secs % 60;
        format!("{h:02}:{m:02}:{s:02}")
    }

    /// Count services by aggregate status.
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
}
