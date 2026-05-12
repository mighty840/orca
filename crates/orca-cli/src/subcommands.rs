use clap::Subcommand;

#[derive(Subcommand)]
pub enum AlertsAction {
    List {
        #[arg(short, long)]
        all: bool,
    },
    View {
        id: String,
    },
    Reply {
        id: String,
        message: Vec<String>,
    },
    Dismiss {
        id: String,
    },
    Fix {
        id: String,
    },
}

#[derive(Subcommand)]
pub enum SecretsAction {
    Set {
        key: String,
        value: String,
    },
    /// Print a secret value to stdout.
    Get {
        key: String,
    },
    Remove {
        key: String,
    },
    List,
    Import {
        #[arg(short, long)]
        file: String,
    },
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
    /// Backup volumes + config files (secrets.json, cluster.toml, services.toml)
    All,
    /// Backup config files only (secrets.json, cluster.toml, services.toml)
    Basic,
    List,
    Restore {
        id: String,
    },
    /// Restore config files (secrets.json, cluster.toml, services.toml) from latest backup
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
    Remove {
        id: String,
    },
}
