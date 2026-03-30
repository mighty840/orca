//! TUI application state.

use crate::api::{ClusterInfo, NodeInfo, ServiceStatus, StatusResponse};

/// Which panel is currently focused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    Services,
    Logs,
    Nodes,
}

/// Full application state for the TUI.
pub struct AppState {
    /// Current panel focus.
    pub panel: Panel,
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
}

impl AppState {
    pub fn new() -> Self {
        Self {
            panel: Panel::Services,
            cluster_name: "connecting...".into(),
            services: Vec::new(),
            nodes: Vec::new(),
            node_count: 0,
            selected_service: 0,
            logs: String::new(),
            error: None,
            should_quit: false,
        }
    }

    /// Update from API status response.
    pub fn update_status(&mut self, resp: StatusResponse) {
        self.cluster_name = resp.cluster_name;
        self.services = resp.services;
        if self.selected_service >= self.services.len() && !self.services.is_empty() {
            self.selected_service = self.services.len() - 1;
        }
    }

    /// Update from cluster info response.
    pub fn update_cluster(&mut self, info: ClusterInfo) {
        self.nodes = info.nodes;
        self.node_count = info.node_count;
    }

    /// Get the name of the currently selected service.
    pub fn selected_service_name(&self) -> Option<&str> {
        self.services
            .get(self.selected_service)
            .map(|s| s.name.as_str())
    }

    /// Move selection up.
    pub fn prev_service(&mut self) {
        if self.selected_service > 0 {
            self.selected_service -= 1;
        }
    }

    /// Move selection down.
    pub fn next_service(&mut self) {
        if !self.services.is_empty() && self.selected_service < self.services.len() - 1 {
            self.selected_service += 1;
        }
    }

    /// Cycle to the next panel.
    pub fn next_panel(&mut self) {
        self.panel = match self.panel {
            Panel::Services => Panel::Logs,
            Panel::Logs => Panel::Nodes,
            Panel::Nodes => Panel::Services,
        };
    }
}
