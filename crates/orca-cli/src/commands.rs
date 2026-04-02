use clap::Subcommand;

pub use crate::subcommands::{
    AlertsAction, BackupAction, DbAction, ImportSource, SecretsAction, TokenAction, WebhookAction,
};

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
        /// Run in the background as a daemon
        #[arg(short, long)]
        daemon: bool,
    },

    /// Deploy services from config (file or directory)
    Deploy {
        /// Path to services dir or single .toml file
        #[arg(short, long, default_value = "services")]
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

    /// Reload: restart the server daemon and redeploy all services
    Reload,

    /// Execute a command inside a running container
    Exec {
        /// Service name
        service: String,
        /// Command to run
        #[arg(trailing_var_arg = true)]
        cmd: Vec<String>,
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

    /// Promote canary instances to stable (completes a canary deploy)
    Promote {
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
        /// Cluster token for authentication
        #[arg(long)]
        token: String,
        /// Run in the background as a daemon
        #[arg(short, long)]
        daemon: bool,
        /// NetBird setup key for mesh networking
        #[arg(long)]
        setup_key: Option<String>,
    },

    /// Manage API tokens
    Token {
        #[command(subcommand)]
        action: TokenAction,
    },

    /// Launch the TUI dashboard
    Tui,

    /// Launch the web dashboard
    Web {
        /// Port for web UI
        #[arg(short, long, default_value = "6890")]
        port: u16,
    },

    /// Manage backups
    Backup {
        #[command(subcommand)]
        action: BackupAction,
    },

    /// Clean up unused Docker resources
    Cleanup,

    /// Stop the orca daemon
    Shutdown,

    /// Create a database service
    Db {
        #[command(subcommand)]
        action: DbAction,
    },

    /// Self-update orca to the latest release
    Update,

    /// Build a Docker image from source for a service
    Build {
        /// Service name to build (builds all if omitted)
        service: Option<String>,
        /// Path to services dir or single .toml file
        #[arg(short, long, default_value = "services")]
        file: String,
    },
}
