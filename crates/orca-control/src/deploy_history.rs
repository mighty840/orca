//! In-memory deploy history for rollback support.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use orca_core::config::ServiceConfig;

/// Maximum number of deploy records kept per service.
const MAX_ENTRIES_PER_SERVICE: usize = 20;

/// A single deploy record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeployRecord {
    /// Unique deploy identifier.
    pub deploy_id: String,
    /// Service name.
    pub service_name: String,
    /// Image that was deployed.
    pub image: Option<String>,
    /// Full service config snapshot at deploy time.
    pub config: ServiceConfig,
    /// When this deploy happened.
    pub timestamp: DateTime<Utc>,
}

/// In-memory deploy history, keyed by service name.
#[derive(Debug, Default)]
pub struct DeployHistory {
    entries: HashMap<String, Vec<DeployRecord>>,
}

impl DeployHistory {
    /// Create an empty deploy history.
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
        }
    }

    /// Record a new deploy for a service.
    ///
    /// Keeps at most [`MAX_ENTRIES_PER_SERVICE`] entries per service,
    /// dropping the oldest when the limit is exceeded.
    pub fn record(&mut self, config: &ServiceConfig) {
        let record = DeployRecord {
            deploy_id: Uuid::now_v7().to_string(),
            service_name: config.name.clone(),
            image: config.image.clone(),
            config: config.clone(),
            timestamp: Utc::now(),
        };

        let history = self.entries.entry(config.name.clone()).or_default();
        history.push(record);

        // Trim to max entries
        if history.len() > MAX_ENTRIES_PER_SERVICE {
            let excess = history.len() - MAX_ENTRIES_PER_SERVICE;
            history.drain(..excess);
        }
    }

    /// Get the second-to-last deploy for rollback.
    ///
    /// Returns `None` if fewer than 2 deploys exist for the service.
    pub fn get_previous(&self, service_name: &str) -> Option<&DeployRecord> {
        let history = self.entries.get(service_name)?;
        if history.len() < 2 {
            return None;
        }
        Some(&history[history.len() - 2])
    }

    /// List all deploy records for a service (oldest first).
    pub fn list(&self, service_name: &str) -> &[DeployRecord] {
        self.entries
            .get(service_name)
            .map(|v| v.as_slice())
            .unwrap_or(&[])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orca_core::types::Replicas;
    use std::collections::HashMap;

    fn test_config(name: &str, image: &str) -> ServiceConfig {
        ServiceConfig {
            name: name.to_string(),
            runtime: Default::default(),
            image: Some(image.to_string()),
            module: None,
            replicas: Replicas::Fixed(1),
            port: Some(8080),
            domain: None,
            health: None,
            env: HashMap::new(),
            resources: None,
            volume: None,
            deploy: None,
            placement: None,
            network: None,
            aliases: vec![],
            mounts: vec![],
            routes: vec![],
            host_port: None,
            triggers: Vec::new(),
            assets: None,
        }
    }

    #[test]
    fn record_and_list() {
        let mut history = DeployHistory::new();
        history.record(&test_config("api", "api:v1"));
        history.record(&test_config("api", "api:v2"));
        assert_eq!(history.list("api").len(), 2);
        assert_eq!(history.list("api")[0].image.as_deref(), Some("api:v1"));
    }

    #[test]
    fn get_previous_returns_second_to_last() {
        let mut history = DeployHistory::new();
        history.record(&test_config("api", "api:v1"));
        assert!(history.get_previous("api").is_none());
        history.record(&test_config("api", "api:v2"));
        let prev = history.get_previous("api").unwrap();
        assert_eq!(prev.image.as_deref(), Some("api:v1"));
    }

    #[test]
    fn caps_at_max_entries() {
        let mut history = DeployHistory::new();
        for i in 0..25 {
            history.record(&test_config("svc", &format!("svc:v{i}")));
        }
        assert_eq!(history.list("svc").len(), MAX_ENTRIES_PER_SERVICE);
        assert_eq!(history.list("svc")[0].image.as_deref(), Some("svc:v5"));
    }
}
