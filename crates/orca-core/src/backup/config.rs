use serde::{Deserialize, Serialize};

/// Configuration for the backup system.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupConfig {
    /// Cron expression for scheduled backups (e.g. "0 0 2 * * *" for 2am daily).
    #[serde(default)]
    pub schedule: Option<String>,

    /// Number of days to retain backups before pruning.
    #[serde(default = "default_retention_days")]
    pub retention_days: u32,

    /// Where to store backups.
    #[serde(default)]
    pub targets: Vec<BackupTarget>,
}

fn default_retention_days() -> u32 {
    30
}

/// A backup storage destination.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum BackupTarget {
    Local {
        path: String,
    },
    S3 {
        bucket: String,
        region: String,
        #[serde(default)]
        prefix: Option<String>,
    },
}

/// Per-service backup settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceBackupConfig {
    /// Whether backups are enabled for this service.
    #[serde(default)]
    pub enabled: bool,

    /// Command to run before backup (e.g. "pg_dump -U postgres mydb > /tmp/dump.sql").
    #[serde(default)]
    pub pre_hook: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_backup_config() {
        let toml = r#"
schedule = "0 0 2 * * *"
retention_days = 7

[[targets]]
type = "local"
path = "/backups"

[[targets]]
type = "s3"
bucket = "my-backups"
region = "eu-central-1"
prefix = "orca/"
"#;
        let config: BackupConfig = toml::from_str(toml).unwrap();
        assert_eq!(config.retention_days, 7);
        assert_eq!(config.targets.len(), 2);
    }
}
