use clap::Subcommand;

#[derive(Subcommand)]
pub enum Command {
    /// Start the orca server (control plane + agent + proxy)
    Server {
        /// Path to cluster.toml
        #[arg(short, long, default_value = "cluster.toml")]
        config: String,
        /// Proxy port for HTTP traffic
        #[arg(long, default_value = "80")]
        proxy_port: u16,
    },

    /// Deploy services from config
    Deploy {
        /// Path to services.toml
        #[arg(short, long, default_value = "services.toml")]
        file: String,
    },

    /// Show cluster and service status
    Status,

    /// Stream logs from a service
    Logs {
        /// Service name
        service: String,
        /// Number of lines to show
        #[arg(long, default_value = "100")]
        tail: u64,
        /// Follow log output
        #[arg(short, long)]
        follow: bool,
        /// Summarize logs using AI
        #[arg(long)]
        summarize: bool,
    },

    /// Scale a service
    Scale {
        /// Service name
        service: String,
        /// Number of replicas
        replicas: u32,
    },

    /// Stop a service or all services
    Stop {
        /// Service name (omit for all services)
        service: Option<String>,
    },

    /// Rollback a service to previous version
    Rollback {
        /// Service name
        service: String,
    },

    /// Ask the AI assistant about the cluster
    Ask {
        /// Your question (e.g., "why is the API returning 503s?")
        question: Vec<String>,
    },

    /// Generate service config from natural language
    Generate {
        /// Description of what you need
        description: Vec<String>,
    },

    /// Manage conversational alerts
    Alerts {
        #[command(subcommand)]
        action: AlertsAction,
    },

    /// Manage secrets
    Secrets {
        #[command(subcommand)]
        action: SecretsAction,
    },

    /// Import from external tools
    Import {
        #[command(subcommand)]
        source: ImportSource,
    },

    /// Manage webhooks for git-push deploy
    Webhooks {
        #[command(subcommand)]
        action: WebhookAction,
    },

    /// List or inspect nodes
    Nodes {
        /// Show GPU details
        #[arg(long)]
        gpus: bool,
    },

    /// Show GPU status across the cluster
    Gpus,

    /// Join this node to an existing cluster
    Join {
        /// Address of an existing cluster node
        address: String,
    },

    /// Launch the TUI dashboard
    Tui,

    /// Launch the web dashboard
    Web {
        /// Port for web UI
        #[arg(short, long, default_value = "6890")]
        port: u16,
    },
}

#[derive(Subcommand)]
pub enum AlertsAction {
    /// List active alert conversations
    List {
        #[arg(short, long)]
        all: bool,
    },
    /// View an alert conversation
    View { id: String },
    /// Reply to an alert conversation
    Reply { id: String, message: Vec<String> },
    /// Dismiss an alert
    Dismiss { id: String },
    /// Apply the AI's suggested fix for an alert
    Fix { id: String },
}

#[derive(Subcommand)]
pub enum SecretsAction {
    /// Set a secret
    Set { key: String, value: String },
    /// Remove a secret
    Remove { key: String },
    /// List all secret keys
    List,
    /// Import secrets from env file
    Import {
        #[arg(short, long)]
        file: String,
    },
}

#[derive(Subcommand)]
pub enum ImportSource {
    /// Import from a docker-compose.yml
    DockerCompose {
        #[arg(default_value = "docker-compose.yml")]
        file: String,
        #[arg(long)]
        analyze: bool,
    },
    /// Import from a Coolify installation
    Coolify {
        #[arg(default_value = "/data/coolify")]
        path: String,
        #[arg(long)]
        analyze: bool,
    },
}

#[derive(Subcommand)]
pub enum WebhookAction {
    /// Add a webhook
    Add {
        #[arg(long)]
        repo: String,
        #[arg(long)]
        service: String,
        #[arg(long, default_value = "main")]
        branch: String,
    },
    /// List webhooks
    List,
    /// Remove a webhook
    Remove { id: String },
}
