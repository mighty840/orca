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
    Metrics,
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
    /// Raw Prometheus metrics text.
    pub metrics_text: String,
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
            metrics_text: String::new(),
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
        let filtered_len = self.filtered_services().len();
        if self.selected_service >= filtered_len && filtered_len > 0 {
            self.selected_service = filtered_len - 1;
        }
    }

    pub fn mark_disconnected(&mut self) {
        self.connection = ConnectionStatus::Disconnected;
    }

    pub fn update_cluster(&mut self, info: ClusterInfo) {
        self.nodes = info.nodes;
        self.node_count = info.node_count;
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

    pub fn selected_service_name(&self) -> Option<&str> {
        let filtered = self.filtered_services();
        filtered.get(self.selected_service).map(|s| s.name.as_str())
    }

    pub fn selected_service_data(&self) -> Option<&ServiceStatus> {
        let filtered = self.filtered_services();
        filtered.get(self.selected_service).copied()
    }

    pub fn prev_service(&mut self) {
        if self.selected_service > 0 {
            self.selected_service -= 1;
        }
    }

    pub fn next_service(&mut self) {
        let len = self.filtered_services().len();
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
            View::Metrics => "Metrics",
        }
    }
}
