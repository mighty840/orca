//! `[api_rate_limit]` in `cluster.toml` (#80): a per-token limit on mutating
//! API requests, applied to every client (CLI, TUI, CI, MCP). Off by default.
//!
//! ```toml
//! [api_rate_limit]
//! enabled = true
//! mutations_per_minute = 60
//! ```

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiRateLimitConfig {
    #[serde(default)]
    pub enabled: bool,
    /// Mutating requests each token may make per minute, as a bucket that
    /// refills continuously: a burst of this many, then one every
    /// `60 / mutations_per_minute` seconds.
    #[serde(default = "default_per_minute")]
    pub mutations_per_minute: u32,
}

impl Default for ApiRateLimitConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            mutations_per_minute: default_per_minute(),
        }
    }
}

fn default_per_minute() -> u32 {
    60
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_by_default_at_60_per_minute() {
        let c: crate::config::ClusterConfig = toml::from_str("[cluster]\n").unwrap();
        assert_eq!(c.api_rate_limit, ApiRateLimitConfig::default());
        assert!(!c.api_rate_limit.enabled);
        assert_eq!(c.api_rate_limit.mutations_per_minute, 60);

        let c: crate::config::ClusterConfig =
            toml::from_str("[cluster]\n[api_rate_limit]\nenabled = true\n").unwrap();
        assert!(c.api_rate_limit.enabled);
        assert_eq!(c.api_rate_limit.mutations_per_minute, 60);
    }
}
