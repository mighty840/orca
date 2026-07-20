use clap::Subcommand;
use clap_complete::engine::ArgValueCandidates;

#[derive(Subcommand)]
pub enum AlertsAction {
    List {
        #[arg(short, long)]
        all: bool,
    },
    View {
        #[arg(add = ArgValueCandidates::new(crate::completion::alert_ids))]
        id: String,
    },
    Reply {
        #[arg(add = ArgValueCandidates::new(crate::completion::alert_ids))]
        id: String,
        message: Vec<String>,
    },
    Dismiss {
        #[arg(add = ArgValueCandidates::new(crate::completion::alert_ids))]
        id: String,
    },
    Resolve {
        #[arg(add = ArgValueCandidates::new(crate::completion::alert_ids))]
        id: String,
    },
    Fix {
        #[arg(add = ArgValueCandidates::new(crate::completion::alert_ids))]
        id: String,
    },
}

#[derive(Subcommand)]
pub enum SecretsAction {
    Set {
        #[arg(add = ArgValueCandidates::new(crate::completion::secret_keys))]
        key: String,
        value: String,
        /// Scope the secret to a project (stored as `<project>.<key>`, #68).
        #[arg(short, long)]
        project: Option<String>,
    },
    /// Print a secret value to stdout.
    Get {
        #[arg(add = ArgValueCandidates::new(crate::completion::secret_keys))]
        key: String,
        /// Look the key up in a project scope first.
        #[arg(short, long)]
        project: Option<String>,
    },
    Remove {
        #[arg(add = ArgValueCandidates::new(crate::completion::secret_keys))]
        key: String,
        /// Remove the project-scoped variant (`<project>.<key>`).
        #[arg(short, long)]
        project: Option<String>,
    },
    List,
    Import {
        #[arg(short, long)]
        file: String,
    },
    /// Move all secrets from the legacy machine-local store into the
    /// [secrets] encrypted file (#109). Requires [secrets] in cluster.toml.
    Migrate,
}

#[derive(Subcommand)]
pub enum ImportSource {
    DockerCompose {
        #[arg(default_value = "docker-compose.yml")]
        file: String,
        #[arg(long)]
        analyze: bool,
    },
    Coolify {
        #[arg(default_value = "/data/coolify")]
        path: String,
        #[arg(long)]
        analyze: bool,
    },
}

#[derive(Subcommand)]
pub enum BackupAction {
    /// Backup volumes + config files (secrets.json, cluster.toml, cluster.db)
    All,
    /// Backup config files only (secrets.json, cluster.toml, cluster.db)
    Basic,
    List,
    Restore {
        id: String,
    },
    /// Restore config files (secrets.json, cluster.toml) from latest backup
    RestoreBasic,
    /// Restore a Docker volume from the latest backup
    RestoreVolume {
        volume_name: String,
    },
}

#[derive(Subcommand)]
pub enum DbAction {
    Create {
        db_type: String,
        name: String,
        #[arg(long)]
        password: Option<String>,
    },
    List,
}

#[derive(Subcommand)]
pub enum TokenAction {
    /// Show the current cluster token
    Show,
    /// Create a new named API token with a role
    Create {
        /// Token name (e.g., "sharang", "gitea-ci")
        #[arg(long)]
        name: String,
        /// Role: admin, deployer, or viewer
        #[arg(long, default_value = "deployer")]
        role: String,
    },
    /// List all configured tokens
    List,
}

#[derive(Subcommand)]
pub enum WebhookAction {
    /// Register a webhook. If --secret is omitted, a random one is generated and printed.
    Add {
        #[arg(long)]
        repo: String,
        #[arg(long)]
        service: String,
        #[arg(long, default_value = "main")]
        branch: String,
        /// HMAC secret for signature verification. Generated if omitted.
        #[arg(long)]
        secret: Option<String>,
        /// Infra webhook: triggers git pull + deploy all instead of per-service redeploy.
        #[arg(long)]
        infra: bool,
    },
    List,
    /// Remove webhooks by service name. Accepts multiple names and glob
    /// patterns (`*`, `?`), e.g. `orca webhooks remove 'breakpilot-*' navidrome`.
    Remove {
        #[arg(required = true, num_args = 1..)]
        #[arg(add = ArgValueCandidates::new(crate::completion::webhook_services))]
        ids: Vec<String>,
    },
}
