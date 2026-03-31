//! TUI application state.

use std::time::Instant;

use crate::api::{ClusterInfo, NodeInfo, ServiceStatus, StatusResponse};

/// Which panel is currently focused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    Services,
    Logs,
    Nodes,
    Detail,
}

/// Input mode for the TUI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputMode {
    Normal,
    Filter,
}

/// Full application state for the TUI.
pub struct AppState {
    /// Current panel focus.
    pub panel: Panel,
    /// Previous panel (for returning from Detail).
    pub prev_panel: Panel,
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
    /// Whether to show help overlay.
    pub show_help: bool,
    /// Status message (e.g. "Deployed nginx").
    pub status_msg: Option<String>,
    /// App start time for uptime display.
    pub start_time: Instant,
}

impl Default for AppState {
    fn default() -> Self {
        Self::new()
    }
}

impl AppState {
    pub fn new() -> Self {
        Self {
            panel: Panel::Services,
            prev_panel: Panel::Services,
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
            show_help: false,
            status_msg: None,
            start_time: Instant::now(),
        }
    }

    /// Update from API status response.
    pub fn update_status(&mut self, resp: StatusResponse) {
        self.cluster_name = resp.cluster_name;
        self.services = resp.services;
        let filtered_len = self.filtered_services().len();
        if self.selected_service >= filtered_len && filtered_len > 0 {
            self.selected_service = filtered_len - 1;
        }
    }

    /// Update from cluster info response.
    pub fn update_cluster(&mut self, info: ClusterInfo) {
        self.nodes = info.nodes;
        self.node_count = info.node_count;
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

    /// Cycle to the next panel.
    pub fn next_panel(&mut self) {
        self.panel = match self.panel {
            Panel::Services => Panel::Logs,
            Panel::Logs => Panel::Nodes,
            Panel::Nodes => Panel::Services,
            Panel::Detail => Panel::Services,
        };
    }

    /// Format uptime as HH:MM:SS.
    pub fn uptime_str(&self) -> String {
        let secs = self.start_time.elapsed().as_secs();
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        let s = secs % 60;
        format!("{h:02}:{m:02}:{s:02}")
    }
}
