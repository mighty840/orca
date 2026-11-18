//! `[mcp]` in `cluster.toml` (#80): the master's MCP endpoint at `/mcp`.
//! Off by default.
//!
//! ```toml
//! [mcp]
//! enabled = true
//! allowed_hosts = ["orca.example.com", "100.80.5.14:6880"]
//! allowed_origins = []      # browser origins; none by default
//! reads_per_minute = 60
//! ```

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct McpConfig {
    /// Serve `/mcp`. It is never served on a cluster with no API tokens.
    #[serde(default)]
    pub enabled: bool,
    /// `Host` headers accepted on `/mcp` (DNS-rebinding protection), in
    /// addition to loopback and `cluster.domain`. A host may carry a port.
    #[serde(default)]
    pub allowed_hosts: Vec<String>,
    /// Browser origins allowed to call `/mcp` (`https://app.example.com:443`).
    /// MCP clients that are not browsers send no `Origin`; one that is
    /// present and not listed is refused.
    #[serde(default)]
    pub allowed_origins: Vec<String>,
    /// Accept the cluster token and other legacy `api_tokens` entries on
    /// `/mcp`. They are admin and, for the cluster token, the agents' join
    /// credential, so they are refused by default; use a `[[token]]`.
    #[serde(default)]
    pub allow_cluster_token: bool,
    /// API reads each token may make through `/mcp` per minute.
    #[serde(default = "default_reads_per_minute")]
    pub reads_per_minute: u32,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            allowed_hosts: Vec::new(),
            allowed_origins: Vec::new(),
            allow_cluster_token: false,
            reads_per_minute: default_reads_per_minute(),
        }
    }
}

fn default_reads_per_minute() -> u32 {
    60
}

#[cfg(test)]
mod tests {
    use crate::config::ClusterConfig;

    #[test]
    fn off_by_default() {
        let c: ClusterConfig = toml::from_str("[cluster]\n").unwrap();
        assert!(!c.mcp.enabled);
        assert_eq!(c.mcp.reads_per_minute, 60);
        let c: ClusterConfig = toml::from_str(
            "[cluster]\n[mcp]\nenabled = true\nallowed_hosts = [\"orca.example.com\"]\n",
        )
        .unwrap();
        assert!(c.mcp.enabled);
        assert_eq!(c.mcp.allowed_hosts, ["orca.example.com"]);
    }
}
