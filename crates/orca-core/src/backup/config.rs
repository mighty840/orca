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

    /// age public keys (`age1…`). When set, every config artifact
    /// (`cluster.toml`, `secrets.json`, `master.key`, `webhooks.json`, TLS
    /// certificates, …) is encrypted to them before it is stored or uploaded
    /// (#117, #199). Keep the matching private key out of band. When empty,
    /// artifacts that hold key material are left out rather than stored in
    /// the clear.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub age_recipients: Vec<String>,

    /// Host bind-mount sources larger than this (MiB) are reported but not
    /// archived (#185), so a mount of a big data directory can't balloon
    /// every snapshot. Default 512.
    #[serde(default = "default_bind_mount_max_mb")]
    pub bind_mount_max_mb: u64,
}

fn default_bind_mount_max_mb() -> u64 {
    512
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
        /// Optional S3-compatible endpoint (for Minio, R2, B2, etc.).
        #[serde(default)]
        endpoint: Option<String>,
        /// AWS access key. Falls back to `AWS_ACCESS_KEY_ID` env var.
        #[serde(default)]
        access_key: Option<String>,
        /// AWS secret key. Falls back to `AWS_SECRET_ACCESS_KEY` env var.
        #[serde(default)]
        secret_key: Option<String>,
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
